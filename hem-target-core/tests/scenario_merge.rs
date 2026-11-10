// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Scenario merges against models with more than one fan or a missing fuel supply.

use hem_target_core::{merge_model, prepare_effective_scenario};
use serde_json::{json, Value};
use vulcan_model_transform::{TransformOutput, ValidationResult, VersionMetadata};

fn fans(names: &[(&str, &str)]) -> Value {
    let mut out = serde_json::Map::new();
    for (name, vent_type) in names {
        out.insert(
            (*name).to_string(),
            json!({"vent_type": vent_type, "SFP": 1.0, "EnergySupply": "mains elec",
                   "design_outdoor_air_flow_rate": 54, "mid_height_air_flow_path": 1.5,
                   "orientation360": 90, "pitch": 90}),
        );
    }
    json!({"InfiltrationVentilation": {"MechanicalVentilation": out}})
}

fn snippet(name: &str, vent_type: &str) -> Value {
    json!({"InfiltrationVentilation":{"MechanicalVentilation":{name:{
        "vent_type": vent_type, "SFP": 2.0, "EnergySupply": "mains elec",
        "design_outdoor_air_flow_rate": 100, "mid_height_air_flow_path": 1.5,
        "orientation360": 180, "pitch": 90}}}})
}

#[test]
fn local_fan_scenario_replaces_only_the_matching_fan() {
    let mut model = fans(&[
        ("C1", "Centralised continuous MEV"),
        ("Bath fan", "Intermittent MEV"),
    ]);
    merge_model(
        &mut model,
        "mechanical_ventilation_unit",
        &snippet("mev_intermittent", "Intermittent MEV"),
    )
    .unwrap();
    let fans = &model["InfiltrationVentilation"]["MechanicalVentilation"];
    assert_eq!(fans.as_object().unwrap().len(), 2);
    assert_eq!(fans["Bath fan"]["SFP"], 2.0);
    assert_eq!(fans["C1"]["SFP"], 1.0);
}

#[test]
fn local_fan_scenario_adds_a_fan_beside_a_different_one_and_says_so() {
    let (model, notices) = prepare_noticed(
        fans(&[("C1", "Centralised continuous MEV")]),
        &[(
            "mechanical_ventilation_unit",
            snippet("mev_intermittent", "Intermittent MEV"),
        )],
    )
    .unwrap();
    let fans = &model["InfiltrationVentilation"]["MechanicalVentilation"];
    assert_eq!(fans["C1"]["SFP"], 1.0);
    assert_eq!(fans["mev_intermittent"]["vent_type"], "Intermittent MEV");
    assert!(
        notices.iter().any(|n| n.contains(
            "adds 'mev_intermittent' alongside the model's 'C1', which stays in the run."
        )),
        "{notices:?}"
    );
}

#[test]
fn ambiguous_fan_match_stops_and_names_the_fans() {
    let mut model = fans(&[
        ("Kitchen", "Intermittent MEV"),
        ("Bathroom", "Intermittent MEV"),
    ]);
    let before = model.clone();
    let err = merge_model(
        &mut model,
        "mechanical_ventilation_unit",
        &snippet("mev_intermittent", "Intermittent MEV"),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("Kitchen") && err.contains("Bathroom"), "{err}");
    assert_eq!(model, before);
}

#[test]
fn single_fan_scenario_keeps_replacing_it() {
    let mut model = fans(&[("Only", "Centralised continuous MEV")]);
    merge_model(
        &mut model,
        "mechanical_ventilation_unit",
        &snippet("mev_system", "Centralised continuous MEV"),
    )
    .unwrap();
    assert_eq!(
        model["InfiltrationVentilation"]["MechanicalVentilation"]["Only"]["SFP"],
        2.0
    );
}

#[test]
fn gas_boiler_scenario_adds_the_mains_gas_supply() {
    let mut model = json!({"EnergySupply": {"mains elec": {"fuel": "electricity"}},
        "HeatSourceWet": {}});
    let boiler = json!({"HeatSourceWet": {"boiler": {"type": "Boiler", "EnergySupply": "mains gas",
        "EnergySupply_aux": "mains elec"}}});
    merge_model(&mut model, "heat_source_wet", &boiler).unwrap();
    assert_eq!(model["EnergySupply"]["mains gas"]["fuel"], "mains_gas");
    assert_eq!(model["EnergySupply"]["mains elec"]["fuel"], "electricity");
}

#[test]
fn orientation_rotates_pv_and_ventilation_but_not_surroundings() {
    let segments = json!([{"start360": 0, "end360": 180}, {"start360": 180, "end360": 360}]);
    let mut model = json!({
        "Zone": {"Z": {"BuildingElement": {"Wall": {"orientation360": 300}}}},
        "OnSiteGeneration": {"PV": {"orientation360": 180}},
        "InfiltrationVentilation": {
            "Vents": {"V": {"orientation360": 300}},
            "MechanicalVentilation": {
                "Fan": {"orientation360": 90},
                "Unit": {"position_intake": {"orientation360": 10},
                          "position_exhaust": {"orientation360": 350}}
            }
        },
        "ExternalConditions": {"shading_segments": segments.clone()}
    });
    merge_model(
        &mut model,
        "orientation",
        &json!({"orientation": {"add_degrees": 90}}),
    )
    .unwrap();
    let vent = &model["InfiltrationVentilation"];
    assert_eq!(
        model["Zone"]["Z"]["BuildingElement"]["Wall"]["orientation360"],
        30.0
    );
    assert_eq!(model["OnSiteGeneration"]["PV"]["orientation360"], 270.0);
    assert_eq!(vent["Vents"]["V"]["orientation360"], 30.0);
    assert_eq!(
        vent["MechanicalVentilation"]["Fan"]["orientation360"],
        180.0
    );
    let unit = &vent["MechanicalVentilation"]["Unit"];
    assert_eq!(unit["position_intake"]["orientation360"], 100.0);
    assert_eq!(unit["position_exhaust"]["orientation360"], 80.0);
    assert_eq!(model["ExternalConditions"]["shading_segments"], segments);
}

#[test]
fn events_scenarios_are_rejected_for_targets() {
    let output = TransformOutput {
        model: json!({"Events": {}}),
        validation: ValidationResult {
            errors: vec![],
            is_valid: true,
        },
        schema_omissions: vec![],
        version_metadata: VersionMetadata {
            hem_core_version: "1.0.0a9".into(),
            fhs_wrapper_version: Some("1.0.0a9".into()),
        },
    };
    let err = prepare_effective_scenario(
        output,
        &json!({}),
        &[("events".to_string(), json!({"Events": {}}))],
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("'events'"), "{err}");
    assert!(err.contains("not supported"), "{err}");
}

fn library(path: &str) -> Value {
    let file = format!(
        "{}/../resources/sample_project/batch_parameters/{path}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap()
}

/// Whole-scenario preparation of a hand-written model (no target schema).
fn prepare_noticed(
    model: Value,
    snippets: &[(&str, Value)],
) -> Result<(Value, Vec<String>), String> {
    let output = TransformOutput {
        model,
        validation: ValidationResult {
            errors: vec![],
            is_valid: true,
        },
        schema_omissions: vec![],
        version_metadata: VersionMetadata {
            hem_core_version: "1.0.0a7".into(),
            fhs_wrapper_version: Some("1.0.0a7".into()),
        },
    };
    let snippets: Vec<(String, Value)> = snippets
        .iter()
        .map(|(c, s)| (c.to_string(), s.clone()))
        .collect();
    prepare_effective_scenario(output, &json!({}), &snippets)
        .map(|o| {
            let notices = o
                .schema_omissions
                .iter()
                .map(|w| w.message.clone())
                .collect();
            (o.model, notices)
        })
        .map_err(|e| e.to_string())
}

fn prepare(model: Value, snippets: &[(&str, Value)]) -> Result<Value, String> {
    prepare_noticed(model, snippets).map(|(model, _)| model)
}

/// A heat pump feeding radiators and a cylinder coil.
fn wet_model() -> Value {
    json!({
        "EnergySupply": {"mains elec": {"fuel": "electricity"}},
        "HeatSourceWet": {"hp": {"type": "HeatPump", "sink_type": "Water", "EnergySupply": "mains elec"}},
        "SpaceHeatSystem": {"Zone 1 circuit": {"type": "WetDistribution", "emitters": [{"wet_emitter_type": "radiator"}],
            "HeatSource": {"name": "hp"}}},
        "HotWaterSource": {"hw cylinder": {"type": "StorageTank", "volume": 80,
            "HeatSource": {"hp": {"type": "HeatSourceWet", "name": "hp"}}}}
    })
}

/// Direct electric heating and an immersion cylinder: nothing wet.
fn electric_model() -> Value {
    json!({
        "EnergySupply": {"mains elec": {"fuel": "electricity"}},
        "SpaceHeatSystem": {"heater": {"type": "InstantElecHeater", "rated_power": 2.5}},
        "HotWaterSource": {"hw cylinder": {"type": "StorageTank", "volume": 80,
            "HeatSource": {"immersion": {"type": "ImmersionHeater", "power": 3}}}}
    })
}

#[test]
fn air_to_air_heat_pump_cannot_serve_radiators_or_a_cylinder() {
    let err = prepare(
        wet_model(),
        &[("heat_source_wet", library("heat_source_wet/a2a_heat_pump"))],
    )
    .unwrap_err();
    assert!(
        err.contains("a2a_hp") && err.contains("sink_type Air"),
        "{err}"
    );
}

#[test]
fn heat_source_with_nothing_to_heat_is_blocked() {
    for snippet in ["hp", "gas_boiler", "heat_battery", "a2a_heat_pump"] {
        let err = prepare(
            electric_model(),
            &[(
                "heat_source_wet",
                library(&format!("heat_source_wet/{snippet}")),
            )],
        )
        .unwrap_err();
        assert!(
            err.contains("nothing in the model uses"),
            "{snippet}: {err}"
        );
    }
}

#[test]
fn scenario_that_changes_a_type_says_to_change_the_model() {
    let combi =
        json!({"HotWaterSource": {"hw cylinder": {"type": "CombiBoiler", "HeatSourceWet": "hp"}}});
    let err = prepare(wet_model(), &[("hot_water_source", combi)]).unwrap_err();
    assert_eq!(
        err,
        "The hot_water_source scenario changes /HotWaterSource/hw cylinder/type, which a scenario cannot change; change this in the model instead."
    );
}

#[test]
fn emitters_need_a_wet_distribution_system() {
    let err = prepare(
        electric_model(),
        &[(
            "space_heat_emitters",
            library("space_heat_emitters/radiators"),
        )],
    )
    .unwrap_err();
    assert!(err.contains("no wet distribution system"), "{err}");
}

#[test]
fn hot_water_coil_never_moves_to_a_different_heat_source() {
    // The boiler cylinder names plant "boiler"; the model has only "hp".
    let err = prepare(
        wet_model(),
        &[(
            "hot_water_source",
            library("hot_water_source/gas_boiler_cylinder"),
        )],
    )
    .unwrap_err();
    assert!(err.contains("'boiler'") && err.contains("hp"), "{err}");
    let err = prepare(
        electric_model(),
        &[("hot_water_source", library("hot_water_source/heat_battery"))],
    )
    .unwrap_err();
    assert!(err.contains("'heat_battery_source'"), "{err}");
    // With its plant in the same scenario, in either order, it prepares.
    for order in [[0, 1], [1, 0]] {
        let pair = [
            (
                "hot_water_source",
                library("hot_water_source/gas_boiler_cylinder"),
            ),
            ("heat_source_wet", library("heat_source_wet/gas_boiler")),
        ];
        let model = prepare(
            wet_model(),
            &[pair[order[0]].clone(), pair[order[1]].clone()],
        )
        .unwrap();
        assert_eq!(
            model["HotWaterSource"]["hw cylinder"]["HeatSource"]["boiler"]["name"],
            "boiler"
        );
        assert_eq!(
            model["SpaceHeatSystem"]["Zone 1 circuit"]["HeatSource"]["name"],
            "boiler"
        );
    }
}

#[test]
fn replacing_the_plant_repoints_what_used_it_in_either_order() {
    let cylinder = library("hot_water_source/heat_pump_cylinder");
    let plant = library("heat_source_wet/exhaust_air_mev_heat_pump");
    for order in [[0, 1], [1, 0]] {
        let pair = [
            ("hot_water_source", cylinder.clone()),
            ("heat_source_wet", plant.clone()),
        ];
        let model = prepare(
            wet_model(),
            &[pair[order[0]].clone(), pair[order[1]].clone()],
        )
        .unwrap();
        assert_eq!(
            model["HotWaterSource"]["hw cylinder"]["HeatSource"]["hp"]["name"],
            "eahp"
        );
        assert_eq!(
            model["SpaceHeatSystem"]["Zone 1 circuit"]["HeatSource"]["name"],
            "eahp"
        );
        assert!(model["HeatSourceWet"].get("hp").is_none());
    }
    // An independent cylinder heater is never touched.
    let hwo = library("hot_water_source/hwo_heat_pump_cylinder");
    let model = prepare(
        wet_model(),
        &[
            ("hot_water_source", hwo.clone()),
            ("heat_source_wet", plant),
        ],
    )
    .unwrap();
    assert_eq!(model["HotWaterSource"], hwo["HotWaterSource"]);
}

#[test]
fn warm_air_follows_only_an_air_sink_plant() {
    let air = json!({
        "EnergySupply": {"mains elec": {"fuel": "electricity"}},
        "HeatSourceWet": {"old_air_hp": {"type": "HeatPump", "sink_type": "Air"}},
        "SpaceHeatSystem": {"air": {"type": "WarmAir", "HeatSource": {"name": "old_air_hp"}}}
    });
    let model = prepare(
        air.clone(),
        &[("heat_source_wet", library("heat_source_wet/a2a_heat_pump"))],
    )
    .unwrap();
    assert_eq!(
        model["SpaceHeatSystem"]["air"]["HeatSource"]["name"],
        "a2a_hp"
    );
    let err = prepare(air, &[("heat_source_wet", library("heat_source_wet/hp"))]).unwrap_err();
    assert!(err.contains("WarmAir") && err.contains("'hp'"), "{err}");
}

#[test]
fn two_new_plants_are_never_guessed_between() {
    let err = prepare(
        wet_model(),
        &[(
            "heat_source_wet",
            json!({"HeatSourceWet": {"a": {"type": "Boiler"}, "b": {"type": "Boiler"}}}),
        )],
    )
    .unwrap_err();
    assert!(err.contains("'hp'"), "{err}");
}

#[test]
fn whole_house_unit_replaces_every_fan_and_says_so() {
    for unit in ["mvhr_standard", "mev_standard"] {
        let mut model = fans(&[
            ("C1", "Centralised continuous MEV"),
            ("Bath fan", "Intermittent MEV"),
        ]);
        model["InfiltrationVentilation"]["Vents"] = json!({"Trickle": {"area_cm2": 60}});
        let (model, notices) = prepare_noticed(
            model,
            &[(
                "mechanical_ventilation_unit",
                library(&format!("mechanical_ventilation_unit/{unit}")),
            )],
        )
        .unwrap();
        let fans = model["InfiltrationVentilation"]["MechanicalVentilation"]
            .as_object()
            .unwrap();
        assert_eq!(fans.len(), 1, "{unit}: {fans:?}");
        assert!(
            notices.iter().any(|n| n.contains("'Bath fan'")),
            "{unit}: {notices:?}"
        );
        // FHS Part F: MVHR dwellings have no background vents; continuous MEV keeps them.
        let mvhr = unit.starts_with("mvhr");
        let vents = model["InfiltrationVentilation"]["Vents"]
            .as_object()
            .unwrap();
        assert_eq!(vents.is_empty(), mvhr, "{unit}");
        assert_eq!(
            notices.iter().any(|n| n.contains("'Trickle'")),
            mvhr,
            "{unit}"
        );
    }
}

#[test]
fn none_snippets_remove_what_the_category_covers() {
    let model = json!({
        "EnergySupply": {"mains elec": {"fuel": "electricity", "ElectricBattery": {"capacity": 5}}},
        "OnSiteGeneration": {"Roof PV": {"type": "PhotovoltaicSystem", "peak_power": 4}},
        "InfiltrationVentilation": {"MechanicalVentilation": {"C1": {"vent_type": "Centralised continuous MEV"}}}
    });
    for (category, pointer, removed) in [
        ("solar_systems", "/OnSiteGeneration", "'Roof PV'"),
        (
            "battery_systems",
            "/EnergySupply/mains elec/ElectricBattery",
            "'mains elec'",
        ),
    ] {
        let (prepared, notices) = prepare_noticed(
            model.clone(),
            &[(category, library(&format!("{category}/none")))],
        )
        .unwrap();
        assert!(
            prepared.pointer(pointer).is_none(),
            "{category}: {prepared}"
        );
        assert!(
            notices.iter().any(|n| n.contains(removed)),
            "{category}: {notices:?}"
        );
    }
}

#[test]
fn ventilation_scenario_cannot_remove_every_fan() {
    // FHS Part F stops the run of a dwelling with no mechanical vents.
    let err = prepare(
        fans(&[("C1", "Centralised continuous MEV")]),
        &[(
            "mechanical_ventilation_unit",
            json!({"InfiltrationVentilation": {"MechanicalVentilation": {}}}),
        )],
    )
    .unwrap_err();
    // The shape of the deleted mechanical_ventilation_unit/none.json, which old workspaces still hold.
    assert_eq!(err, "This mechanical ventilation scenario removes every fan, and FHS needs at least one mechanical vent in the dwelling. Vulcan no longer ships it (mechanical_ventilation_unit/none). Delete this scenario file or choose another ventilation option.");
}

#[test]
fn pv_scenario_replaces_the_installation() {
    let model = json!({
        "EnergySupply": {"mains elec": {"fuel": "electricity"}},
        "OnSiteGeneration": {"Roof PV": {"type": "PhotovoltaicSystem", "peak_power": 4}}
    });
    let (prepared, notices) = prepare_noticed(
        model,
        &[("solar_systems", library("solar_systems/pv_system"))],
    )
    .unwrap();
    let arrays = prepared["OnSiteGeneration"].as_object().unwrap();
    assert_eq!(arrays.keys().collect::<Vec<_>>(), ["PV 1"]);
    assert!(
        notices.iter().any(|n| n.contains("'Roof PV'")),
        "{notices:?}"
    );
}

#[test]
fn orientation_rotates_solar_thermal_collectors() {
    let mut model = json!({"HotWaterSource": {"hw cylinder": {"type": "StorageTank",
        "HeatSource": {"solar": {"type": "SolarThermalSystem", "orientation360": 300, "tilt": 30}}}}});
    merge_model(
        &mut model,
        "orientation",
        &json!({"orientation": {"add_degrees": 90}}),
    )
    .unwrap();
    assert_eq!(
        model["HotWaterSource"]["hw cylinder"]["HeatSource"]["solar"]["orientation360"],
        30.0
    );
}

#[test]
fn scenario_order_never_changes_the_prepared_input() {
    let mut model = wet_model();
    model["InfiltrationVentilation"] =
        fans(&[("Bath fan", "Intermittent MEV")])["InfiltrationVentilation"].clone();
    let snippets = [
        ("orientation", library("orientation/add_25deg")),
        ("solar_systems", library("solar_systems/pv_system")),
        (
            "mechanical_ventilation_unit",
            library("mechanical_ventilation_unit/mvhr_standard"),
        ),
        ("heat_source_wet", library("heat_source_wet/gas_boiler")),
        (
            "hot_water_source",
            library("hot_water_source/gas_boiler_cylinder"),
        ),
    ];
    let mut first = None;
    // Every permutation of the five: each digit of i in base 5 picks one snippet.
    for i in 0..5usize.pow(5) {
        let order: Vec<usize> = (0..5).map(|d| i / 5usize.pow(d) % 5).collect();
        if (0..5).any(|k| !order.contains(&k)) {
            continue;
        }
        let picked: Vec<_> = order.iter().map(|&k| snippets[k].clone()).collect();
        let prepared = prepare(model.clone(), &picked).unwrap();
        match &first {
            None => {
                // Turned once, after the scenario's own PV and MVHR are in place.
                assert_eq!(
                    prepared["OnSiteGeneration"]["PV 1"]["orientation360"],
                    205.0
                );
                let unit =
                    &prepared["InfiltrationVentilation"]["MechanicalVentilation"]["mvhr_standard"];
                assert_eq!(unit["position_exhaust"]["orientation360"], 205.0);
                first = Some(prepared);
            }
            Some(first) => assert_eq!(&prepared, first, "order {order:?}"),
        }
    }
}
