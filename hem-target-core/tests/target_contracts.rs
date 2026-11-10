// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Converter contracts driven through the real CSV parser and merge.

use serde_json::{json, Value};
use vulcan_model_transform::{
    transform_geometry_csv, ConversionProfile, ModelProfile, TransformOutput, TransformRequest,
    VersionMetadata,
};

const FIXTURE: &str = include_str!("fixtures/el_all.csv");
const A7_SCHEMA: &str = include_str!("../../community/data/schemas/input_fhs.schema.json");
const A8_SCHEMA: &str = include_str!(
    "../../fixtures/hem_target_schemas/python-fhs-a8-f2ab6cf7-8ca182b0-v1.schema.json"
);
const A9_SCHEMA: &str = include_str!(
    "../../fixtures/hem_target_schemas/python-fhs-a9-918addad-b9b90138-v1.schema.json"
);
const DEFAULTS: &str = include_str!("../../community/data/defaults/defaults_template.json");

const A7: ConversionProfile = ConversionProfile::CurrentRustFhs;
const A8: ConversionProfile = ConversionProfile::PythonFhsA8;
const A9: ConversionProfile = ConversionProfile::PythonFhsA9;

fn prepare(csv: &str, profile: ConversionProfile) -> TransformOutput {
    prepare_with(csv, profile, DEFAULTS)
}

fn prepare_with(csv: &str, profile: ConversionProfile, defaults: &str) -> TransformOutput {
    let (version, schema) = match profile {
        ConversionProfile::PythonFhsA8 => ("1.0.0a8", A8_SCHEMA),
        ConversionProfile::PythonFhsA9 => ("1.0.0a9", A9_SCHEMA),
        _ => ("1.0.0a7", A7_SCHEMA),
    };
    transform_geometry_csv(TransformRequest {
        csv: csv.into(),
        schema_json: schema.into(),
        defaults_json: defaults.into(),
        profile: ModelProfile::Fhs,
        conversion_profile: profile,
        version_metadata: VersionMetadata {
            hem_core_version: version.into(),
            fhs_wrapper_version: Some(version.into()),
        },
    })
    .expect("transform")
}

fn element<'a>(output: &'a TransformOutput, name: &str) -> &'a Value {
    &output.model["Zone"]["Room"]["BuildingElement"][name]
}

fn window_csv(row: &str) -> String {
    let lines: Vec<String> = FIXTURE
        .lines()
        .map(|line| {
            if line.starts_with("Window,Room,") {
                row.to_string()
            } else {
                line.to_string()
            }
        })
        .collect();
    lines.join("\n")
}

/// Replace a whole CSV section's rows: `header` and `rows` follow the section title.
fn with_section(csv: &str, section: &str, rows: &[&str]) -> String {
    let lines: Vec<&str> = csv.lines().collect();
    let start = lines.iter().position(|l| *l == section).expect(section);
    let mut end = start + 1;
    while end < lines.len() && !lines[end].is_empty() {
        end += 1;
    }
    let mut out: Vec<&str> = lines[..=start].to_vec();
    out.extend_from_slice(rows);
    out.extend_from_slice(&lines[end..]);
    out.join("\n")
}

fn errors_on<'a>(output: &'a TransformOutput, path: &str) -> Vec<&'a str> {
    output
        .validation
        .errors
        .iter()
        .filter(|e| e.path == path)
        .map(|e| e.code.as_str())
        .collect()
}

const WINDOW_COLUMNS_NO_LIST: &str =
    "Window,Room,BuildingElementTransparent,1.2,90,1,1.2,0,0.8,0.2,1,1.4,0.6,";

#[test]
fn party_floor_marker_reaches_the_mappers_and_never_the_model() {
    for profile in [A8, A9] {
        let output = prepare(FIXTURE, profile);
        let floor = element(&output, "PartyFloor");
        assert_eq!(
            floor["is_adjacent_space_within_dwelling"],
            json!(false),
            "{profile:?}"
        );
        assert_eq!(floor["area"].as_f64(), Some(10.0), "{profile:?}");
    }
    let a7 = prepare(FIXTURE, A7);
    assert_eq!(element(&a7, "PartyFloor")["area"].as_f64(), Some(10.0));
    for profile in [A7, A8, A9] {
        let model = serde_json::to_string(&prepare(FIXTURE, profile).model).unwrap();
        assert!(!model.contains("_vulcan_ui_party_element"), "{profile:?}");
    }
}

#[test]
fn closed_physical_windows_become_one_zero_area_opening_inside_the_window() {
    let empty_list = "Window,Room,BuildingElementTransparent,1.2,90,1,1.2,0,0.8,0.2,1,1.4,0.6,\"{\"\"window_part_list\"\":[]}\"";
    let zero_area = "Window,Room,BuildingElementTransparent,1.2,90,1,1.2,0,0.8,0.2,1,1.4,0,";
    for row in [empty_list, zero_area] {
        for profile in [A8, A9] {
            let output = prepare(&window_csv(row), profile);
            let parts = element(&output, "Window")["window_part_list"]
                .as_array()
                .unwrap_or_else(|| panic!("{row}: {:?}", element(&output, "Window")));
            assert_eq!(parts.len(), 1, "{row}");
            assert_eq!(parts[0]["max_window_open_area"], json!(0), "{row}");
            assert_eq!(parts[0]["free_area_height"].as_f64(), Some(1.2), "{row}");
            assert_eq!(parts[0]["mid_height"].as_f64(), Some(1.4), "{row}");
            assert!(
                errors_on(
                    &output,
                    "/Zone/Room/BuildingElement/Window/window_part_list"
                )
                .is_empty(),
                "{row}"
            );
        }
    }
}

#[test]
fn legacy_contract_window_without_a_list_uses_its_own_mid_height() {
    let output = prepare(&window_csv(WINDOW_COLUMNS_NO_LIST), A7);
    assert_eq!(
        element(&output, "Window")["window_part_list"],
        json!([{"mid_height_air_flow_path": 1.4}])
    );
    // An authored list stays exactly as authored.
    let authored = prepare(FIXTURE, A7);
    assert_eq!(
        element(&authored, "Window")["window_part_list"],
        json!([{"mid_height_air_flow_path": 1.4},{"mid_height_air_flow_path": 1.4}])
    );
}

#[test]
fn window_heights_the_runtime_rejects_are_blocking_target_input() {
    let cases = [
        // free height above the 1.2 m window
        "Window,Room,BuildingElementTransparent,1.2,90,1,1.2,0,0.8,0.2,1.5,1.4,0.6,",
        // open area above width * height
        "Window,Room,BuildingElementTransparent,1.2,90,1,1.2,0,0.8,0.2,1,1.4,1.3,",
        // mid height above the window head (0.8 + 1.2 = 2.0)
        "Window,Room,BuildingElementTransparent,1.2,90,1,1.2,0,0.8,0.2,1,2.5,0.6,",
    ];
    for row in cases {
        for profile in [A8, A9] {
            let output = prepare(&window_csv(row), profile);
            assert_eq!(
                errors_on(
                    &output,
                    "/Zone/Room/BuildingElement/Window/window_part_list"
                ),
                vec!["E_TARGET_INPUT"],
                "{row}"
            );
            assert!(!output.validation.is_valid);
        }
    }
    // The boundary stays legal: mid height exactly at the head.
    let boundary = "Window,Room,BuildingElementTransparent,1.2,90,1,1.2,0,0.8,0.2,1.2,2,1.2,";
    let output = prepare(&window_csv(boundary), A9);
    assert!(errors_on(
        &output,
        "/Zone/Room/BuildingElement/Window/window_part_list"
    )
    .is_empty());
}

fn wet_distribution_row(name: &str) -> String {
    format!(
        "{name},Room,System,SpaceHeatSystem,\"{{\"\"SpaceHeatSystem\"\":{{\"\"{name}\"\":{{\"\"type\"\":\"\"WetDistribution\"\",\"\"emitters\"\":[],\"\"temp_diff_emit_dsgn\"\":10,\"\"variable_flow\"\":true,\"\"min_flow_rate\"\":3,\"\"max_flow_rate\"\":18,\"\"HeatSource\"\":{{\"\"name\"\":\"\"hp\"\",\"\"temp_flow_limit_upper\"\":65}},\"\"ecodesign_controller\"\":{{\"\"ecodesign_control_class\"\":2,\"\"min_outdoor_temp\"\":-4,\"\"max_outdoor_temp\"\":20,\"\"min_flow_temp\"\":30}},\"\"design_flow_temp\"\":55}}}}}}\""
    )
}

#[test]
fn authored_wet_distribution_without_emitters_is_blocking_on_every_path() {
    let systems = |csv: &str, name: &str| {
        let rows = [
            "Name,Zone,Type,subcategory,extra_json".to_string(),
            wet_distribution_row(name),
        ];
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        with_section(csv, "Systems", &rows)
    };
    let no_section = systems(FIXTURE, "Space heating");
    let empty_section = format!(
        "{no_section}\n\nWet Emitters\nName,Zone,Type,subcategory,area,unit_number,coords,extra_json\n"
    );
    for csv in [no_section, empty_section] {
        for profile in [A7, A8, A9] {
            let output = prepare(&csv, profile);
            assert!(
                errors_on(&output, "/SpaceHeatSystem/Space heating").contains(&"E059"),
                "{profile:?}: {:?}",
                output.validation.errors
            );
            assert!(!output.validation.is_valid, "{profile:?}");
        }
    }
}

const FANCOIL_RUN_DATA: &str = "{\"\"fan_power_W\"\":[15,19],\"\"fan_speed_data\"\":[{\"\"temperature_diff\"\":80,\"\"power_output\"\":[2.7,3.6]},{\"\"temperature_diff\"\":10,\"\"power_output\"\":[0,0]}]}";

fn with_fancoil(extra: &str) -> String {
    let heat_pump = "Pump,Room,System,HeatSourceWet,\"{\"\"HeatSourceWet\"\":{\"\"hp\"\":{\"\"type\"\":\"\"HeatPump\"\",\"\"EnergySupply\"\":\"\"mains elec\"\"}}}\"";
    let systems = with_section(
        FIXTURE,
        "Systems",
        &["Name,Zone,Type,subcategory,extra_json", heat_pump],
    );
    format!(
        "{systems}\n\nWet Emitters\nName,Zone,Type,subcategory,area,unit_number,coords,extra_json\nFan,Room,WetEmitter,fancoil,,1,\"0,0,0|1,0,0\",\"{extra}\"\n"
    )
}

#[test]
fn fancoil_without_test_data_is_a_validation_error_not_a_stub() {
    for extra in [
        "{\"\"frac_convective\"\":0.9}".to_string(),
        "{\"\"frac_convective\"\":0.9,\"\"fancoil_test_data\"\":{\"\"fan_power_W\"\":[15],\"\"fan_speed_data\"\":[]}}".to_string(),
    ] {
        for profile in [A7, A8, A9] {
            let output = prepare(&with_fancoil(&extra), profile);
            assert!(
                !errors_on(&output, "/Wet Emitters/Fan/fancoil_test_data").is_empty(),
                "{extra} {profile:?}: {:?}",
                output.validation.errors
            );
            assert!(!output.validation.is_valid);
        }
    }
    let serialized =
        serde_json::to_string(&prepare(&with_fancoil("{\"\"frac_convective\"\":0.9}"), A9).model)
            .unwrap();
    assert!(!serialized.contains("\"fan_power_W\":[15]"));
}

#[test]
fn physical_opening_fancoil_systems_get_their_energy_supply_from_the_heat_source() {
    let extra =
        format!("{{\"\"frac_convective\"\":0.9,\"\"fancoil_test_data\"\":{FANCOIL_RUN_DATA}}}");
    for profile in [A8, A9] {
        let output = prepare(&with_fancoil(&extra), profile);
        let system = &output.model["SpaceHeatSystem"]["Room fancoil"];
        assert_eq!(
            system["EnergySupply"],
            json!("mains elec"),
            "{profile:?}: {system}"
        );
    }
    let a7 = prepare(&with_fancoil(&extra), A7);
    assert!(a7.model["SpaceHeatSystem"]["Room fancoil"]
        .get("EnergySupply")
        .is_none());
}

const PV_INVERTER_FIELDS: [&str; 4] = [
    "inverter_peak_power_dc",
    "inverter_peak_power_ac",
    "inverter_is_inside",
    "inverter_type",
];

fn with_extra_section(section: &str, header: &str, rows: &[&str]) -> String {
    format!("{FIXTURE}\n\n{section}\n{header}\n{}\n", rows.join("\n"))
}

#[test]
fn blank_wet_emitter_links_never_remove_heating_silently() {
    let heat_pump = "Pump,Room,System,HeatSourceWet,\"{\"\"HeatSourceWet\"\":{\"\"hp\"\":{\"\"type\"\":\"\"HeatPump\"\",\"\"EnergySupply\"\":\"\"mains elec\"\"}}}\"";
    let systems = with_section(
        FIXTURE,
        "Systems",
        &["Name,Zone,Type,subcategory,extra_json", heat_pump],
    );
    let header = "Wet Emitters\nName,Zone,Type,subcategory,area,unit_number,space_heat_system,coords,extra_json";
    let rad = "Rad,Room,WetEmitter,radiator,,1,,\"0,0,0|1,0,0\",";
    let all_blank = format!("{systems}\n\n{header}\n{rad}\n");
    let linked = format!("{}\n{}", heat_pump, wet_distribution_row("Space heating"));
    let linked = with_section(
        FIXTURE,
        "Systems",
        &["Name,Zone,Type,subcategory,extra_json", &linked],
    );
    let one_blank = format!(
        "{linked}\n\n{header}\n{rad}\nRad2,Room,WetEmitter,radiator,,1,Space heating,\"0,0,0|1,0,0\",\n"
    );
    for csv in [all_blank, one_blank] {
        for profile in [A7, A8, A9] {
            let output = prepare(&csv, profile);
            assert!(
                errors_on(&output, "/Wet Emitters/Rad/space_heat_system").contains(&"E059"),
                "{profile:?}: {:?}",
                output.validation.errors
            );
            assert!(!output.validation.is_valid);
        }
    }
}

#[test]
fn schema_errors_on_a_linked_emitter_or_a_pv_system_name_the_csv_row() {
    let heat_pump = "Pump,Room,System,HeatSourceWet,\"{\"\"HeatSourceWet\"\":{\"\"hp\"\":{\"\"type\"\":\"\"HeatPump\"\",\"\"EnergySupply\"\":\"\"mains elec\"\"}}}\"";
    let systems = with_section(
        FIXTURE,
        "Systems",
        &[
            "Name,Zone,Type,subcategory,extra_json",
            heat_pump,
            &wet_distribution_row("Space heating"),
        ],
    );
    // The radiator has no n; the PV row has a blank base_height.
    let csv = format!(
        "{systems}\n\nWet Emitters\nName,Zone,Type,subcategory,area,unit_number,space_heat_system,coords,extra_json\nRad,Room,WetEmitter,radiator,,1,Space heating,\"0,0,0|1,0,0\",\"{{\"\"c\"\":0.1,\"\"thermal_mass\"\":0.5,\"\"frac_convective\"\":0.7}}\"\n\nOn-Site Generation\nName,generation_type,pitch,orientation360,peak_power,base_height,width,height,coords,extra_json\nPV,PhotovoltaicSystem,30,180,4,,3,2,\"0,0,0|3,0,0\",\n"
    );
    for profile in [A7, A8, A9] {
        let output = prepare(&csv, profile);
        let message = |path: &str, field: &str| {
            output
                .validation
                .errors
                .iter()
                .find(|e| e.path == path && e.message.contains(&format!("\"{field}\"")))
                .map(|e| e.message.clone())
                .unwrap_or_else(|| panic!("{profile:?} {path}: {:?}", output.validation.errors))
        };
        assert!(
            message("/SpaceHeatSystem/Space heating/emitters/0", "n")
                .starts_with("Wet Emitters row 'Rad' (system 'Space heating'): "),
            "{profile:?}"
        );
        assert!(
            message("/OnSiteGeneration/PV", "base_height")
                .starts_with("On-Site Generation row 'PV': "),
            "{profile:?}"
        );
    }
}

const CONTEXT_HEADER: &str = "Name,Type,shading_type,start_angle,end_angle,distance,height";

fn sectors(output: &TransformOutput) -> Vec<Value> {
    output.model["ExternalConditions"]["shading_segments"]
        .as_array()
        .unwrap()
        .clone()
}

/// Python core's accumulation for a flat element no obstacle reaches.
fn flat_sky_fraction(sectors: &[Value]) -> f64 {
    let mut total = 0.0;
    for sector in sectors {
        let width = sector["end360"].as_f64().unwrap() - sector["start360"].as_f64().unwrap();
        total += 1.0 * (width / 360.0);
    }
    total
}

#[test]
fn shaded_physical_sectors_cannot_sum_above_one_and_keep_the_obstacle() {
    let csv = with_extra_section(
        "Context Shading",
        CONTEXT_HEADER,
        &["Low,ContextShading,obstacle,170,190,10,1"],
    );
    for profile in [A8, A9] {
        let list = sectors(&prepare(&csv, profile));
        assert!(list.len() >= 8 && list.len() <= 36, "{}", list.len());
        assert!(flat_sky_fraction(&list) <= 1.0, "{list:?}");
        let covered = |deg: f64| {
            list.iter().any(|s| {
                s["start360"].as_f64().unwrap() <= deg
                    && deg < s["end360"].as_f64().unwrap()
                    && s.get("shading").is_some()
            })
        };
        assert!(covered(175.0) && covered(185.0) && !covered(195.0) && !covered(165.0));
        assert_eq!(list.last().unwrap()["end360"], json!(360));
    }
    assert_eq!(sectors(&prepare(&csv, A7)).len(), 36);
}

#[test]
fn full_circle_context_shading_shades_every_sector() {
    let csv = with_extra_section(
        "Context Shading",
        CONTEXT_HEADER,
        &["Ring,ContextShading,obstacle,0,360,10,8"],
    );
    for profile in [A7, A8, A9] {
        let list = sectors(&prepare(&csv, profile));
        assert!(
            list.iter().all(|s| s.get("shading").is_some()),
            "{profile:?}: {list:?}"
        );
    }
}

#[test]
fn pv_without_inverter_fields_is_blocked_not_invented() {
    let header = "Name,generation_type,pitch,orientation360,peak_power,base_height,width,height,coords,extra_json";
    let bare = with_extra_section(
        "On-Site Generation",
        header,
        &["PV,PhotovoltaicSystem,30,180,4,2.5,3,2,\"0,0,0|3,0,0\",\"{\"\"ventilation_strategy\"\":\"\"moderately_ventilated\"\"}\""],
    );
    for profile in [A7, A8, A9] {
        let output = prepare(&bare, profile);
        for field in PV_INVERTER_FIELDS {
            assert!(
                missing_message(&output, "/OnSiteGeneration/PV", field),
                "{profile:?} {field}: {:?}",
                output.validation.errors
            );
        }
        assert!(output.model["OnSiteGeneration"]["PV"]
            .get("inverter_peak_power_ac")
            .is_none());
    }
    let authored = with_extra_section(
        "On-Site Generation",
        header,
        &["PV,PhotovoltaicSystem,30,180,4,2.5,3,2,\"0,0,0|3,0,0\",\"{\"\"ventilation_strategy\"\":\"\"moderately_ventilated\"\",\"\"inverter_peak_power_dc\"\":4,\"\"inverter_peak_power_ac\"\":3.6,\"\"inverter_is_inside\"\":false,\"\"inverter_type\"\":\"\"string_inverter\"\"}\""],
    );
    let output = prepare(&authored, A9);
    assert_eq!(
        output.model["OnSiteGeneration"]["PV"]["inverter_peak_power_ac"],
        json!(3.6)
    );
    assert!(!output
        .validation
        .errors
        .iter()
        .any(|e| e.path.starts_with("/OnSiteGeneration")));
}

#[test]
fn consolidated_zones_keep_every_zones_lighting() {
    let zoned = with_section(
        FIXTURE,
        "Zone",
        &[
            "Name,Type,volume,floor_area,height,simplified thermal bridging,livingroom_area,restofdwelling_area",
            "Room,Zone,50,20,2.5,FALSE,10,10",
            "Room2,Zone,50,20,2.5,FALSE,10,10",
        ],
    );
    let csv = with_section(
        &zoned,
        "Lighting",
        &[
            "Name,Zone,efficacy,count,power",
            "Lamp,Room,80,1,10",
            "Lamp2,Room2,80,2,10",
        ],
    );
    for profile in [A7, A9] {
        let output = prepare(&csv, profile);
        let zones = output.model["Zone"].as_object().unwrap();
        assert_eq!(zones.len(), 1, "{profile:?}");
        let bulbs = zones["Room"]["Lighting"]["bulbs"].as_array().unwrap();
        assert_eq!(bulbs.len(), 2, "{profile:?}: {bulbs:?}");
    }
}

#[test]
fn blank_linear_bridge_fields_fail_instead_of_taking_template_values() {
    let row = "Floor edge,Room,ThermalBridgeLinear,18,0.05,\"{\"\"junction_type\"\":\"\"E5\"\"}\"";
    let without = |replacement: &str| FIXTURE.replace(row, replacement);
    let blank_length =
        "Floor edge,Room,ThermalBridgeLinear,,0.05,\"{\"\"junction_type\"\":\"\"E5\"\"}\"";
    let blank_psi =
        "Floor edge,Room,ThermalBridgeLinear,18,,\"{\"\"junction_type\"\":\"\"E5\"\"}\"";
    for (replacement, field) in [
        (blank_length, "length"),
        (blank_psi, "linear_thermal_transmittance"),
    ] {
        for profile in [A7, A8, A9] {
            let output = prepare(&without(replacement), profile);
            let bridge = &output.model["Zone"]["Room"]["ThermalBridging"]["Floor edge"];
            assert!(bridge.get(field).is_none(), "{profile:?}: {bridge}");
            assert!(!output.validation.is_valid, "{profile:?} {field}");
        }
    }
}

fn missing_message<'a>(output: &'a TransformOutput, path: &str, field: &str) -> bool {
    output
        .validation
        .errors
        .iter()
        .any(|e| e.path == path && e.message.contains(&format!("\"{field}\"")))
}

/// A column the CSV format has belongs to the CSV: a blank cell, or a column missing from the
/// file's header, never takes the defaults template's value. The target schema names the gap.
#[test]
fn blank_or_missing_csv_columns_never_take_template_values() {
    let floor = "Ground Elements";
    let ground_without_perimeter = with_section(
        FIXTURE,
        floor,
        &[
            "Name,Zone,Type,area,floor_type,thickness_walls",
            "Floor,Room,BuildingElementGround,20,Slab_no_edge_insulation,0.3",
        ],
    );
    let pipe = with_extra_section(
        "Water Pipework",
        "Name,Type,length,location,pipework_type",
        &["Pipe,WaterPipework,,internal,primary"],
    );
    let cases = [
        (
            FIXTURE.replace(
                "West,Room,BuildingElementOpaque,4,90,1.6,2.5,270,0,",
                "West,Room,BuildingElementOpaque,4,90,1.6,2.5,,0,",
            ),
            "/Zone/Room/BuildingElement/West",
            "orientation360",
        ),
        (
            FIXTURE.replace(
                "Window,Room,BuildingElementTransparent,1.2,90,1,1.2,0,0.8,0.2,",
                "Window,Room,BuildingElementTransparent,1.2,90,,1.2,0,0.8,0.2,",
            ),
            "/Zone/Room/BuildingElement/Window",
            "width",
        ),
        (
            ground_without_perimeter,
            "/Zone/Room/BuildingElement/Floor",
            "perimeter",
        ),
        (
            FIXTURE.replace("Vent N,Vents,1.5,60,", "Vent N,Vents,1.5,,"),
            "/InfiltrationVentilation/Vents/Vent N",
            "area_cm2",
        ),
        (
            pipe,
            "/HotWaterSource/hw cylinder/primary_pipework/0",
            "length",
        ),
    ];
    for (csv, path, field) in &cases {
        for profile in [A7, A8, A9] {
            let output = prepare(csv, profile);
            assert!(
                output.model.pointer(path).unwrap().get(*field).is_none(),
                "{profile:?} {path}: {}",
                output.model.pointer(path).unwrap()
            );
            assert!(
                missing_message(&output, path, field),
                "{profile:?} {path} {field}: {:?}",
                output.validation.errors
            );
        }
    }
    // An existing derivation still fills a blank cell, and a field with no column still comes
    // from the template.
    let blank_area = FIXTURE.replace(
        "West,Room,BuildingElementOpaque,4,90,",
        "West,Room,BuildingElementOpaque,,90,",
    );
    let derived = prepare(&blank_area, A7);
    let west = element(&derived, "West").clone();
    assert_eq!(west["area"].as_f64(), Some(1.6 * 2.5));
    assert_eq!(west["u_value"], json!(0.18));
    // The derived area is the one the air-permeability envelope counts (the cell was 4).
    let env = |output: &TransformOutput| {
        output.model["InfiltrationVentilation"]["Leaks"]["env_area"].clone()
    };
    assert_eq!(env(&derived), env(&prepare(FIXTURE, A7)));
}

#[test]
fn a_blank_metadata_row_is_an_error_and_an_absent_one_keeps_the_template() {
    let blank = FIXTURE.replace(
        "General_storeys_in_dwelling,1",
        "General_storeys_in_dwelling,",
    );
    for profile in [A7, A8, A9] {
        let output = prepare(&blank, profile);
        assert!(
            output.validation.errors.iter().any(|e| e.code == "E066"
                && e.message.contains("General_storeys_in_dwelling")
                && e.message.contains("Enter the value")
                && !e.message.contains("delete")),
            "{profile:?}: {:?}",
            output.validation.errors
        );
        assert!(!output.validation.is_valid);
    }
    let output = prepare(FIXTURE, A7);
    assert!(!output.validation.errors.iter().any(|e| e.code == "E066"));
    assert_eq!(
        output.model["InfiltrationVentilation"]["shield_class"],
        json!("Normal")
    );
}

#[test]
fn systems_rows_with_an_unknown_or_blank_route_are_blocking() {
    let heater = "Heater,Room,System,SpaceHeatSystem,";
    for (replacement, bad) in [
        ("Heater,Room,System,SpaceHeatSystm,", "SpaceHeatSystm"),
        ("Heater,Room,System,,", "''"),
        ("Heater,Room,Sytem,SpaceHeatSystem,", "Sytem"),
        (
            "Heater,Room,,SpaceHeatSystm,",
            "blank Type, and its subcategory 'SpaceHeatSystm'",
        ),
    ] {
        let csv = FIXTURE.replace(heater, replacement);
        for profile in [A7, A8, A9] {
            let output = prepare(&csv, profile);
            assert!(
                output.validation.errors.iter().any(|e| e.code == "E065"
                    && e.message.contains("Heater")
                    && e.message.contains(bad)),
                "{replacement} {profile:?}: {:?}",
                output.validation.errors
            );
            assert!(!output.validation.is_valid);
        }
    }
}

/// A party floor marked in the editor and the same floor written as `Type=Party` are the same
/// building: the envelope area derived for air permeability must not depend on the spelling.
#[test]
fn a_marked_party_floor_counts_in_env_area_like_type_party() {
    let marked = "PartyFloor,Room,BuildingElementAdjacentConditionedSpace,10,0,\"{\"\"thermal_resistance_construction\"\":2,\"\"construction_basis\"\":\"\"full\"\",\"\"internal_partition_area_basis\"\":\"\"single_face\"\",\"\"areal_heat_capacity\"\":\"\"Medium\"\",\"\"_vulcan_ui_party_element\"\":true}\"";
    assert!(FIXTURE.contains(marked));
    let typed = FIXTURE.replace(
        marked,
        "PartyFloor,Room,Party,10,0,\"{\"\"thermal_resistance_construction\"\":2,\"\"construction_basis\"\":\"\"full\"\",\"\"internal_partition_area_basis\"\":\"\"single_face\"\",\"\"areal_heat_capacity\"\":\"\"Medium\"\"}\"",
    );
    let internal = FIXTURE.replace(
        "Internal,Room,BuildingElementAdjacentConditionedSpace,",
        "Internal,Room,Internal,",
    );
    for profile in [A7, A9] {
        let env = |csv: &str| {
            prepare(csv, profile).model["InfiltrationVentilation"]["Leaks"]["env_area"].clone()
        };
        assert_eq!(env(FIXTURE), env(&typed), "{profile:?}");
        assert_eq!(env(FIXTURE), env(&internal), "{profile:?}");
        assert_eq!(
            prepare(FIXTURE, profile).model,
            prepare(&typed, profile).model,
            "{profile:?}"
        );
    }
}

/// A blank cell never takes a value the converter makes up: the row's own CSV column or
/// extra_json gives the value, otherwise it is absent and the target schema names it.
#[test]
fn blank_cells_never_take_converter_constants() {
    let heat_pump = "Pump,Room,System,HeatSourceWet,\"{\"\"HeatSourceWet\"\":{\"\"hp\"\":{\"\"type\"\":\"\"HeatPump\"\",\"\"EnergySupply\"\":\"\"mains elec\"\"}}}\"";
    let systems = format!("{heat_pump}\n{}", wet_distribution_row("Space heating"));
    let systems = with_section(
        FIXTURE,
        "Systems",
        &["Name,Zone,Type,subcategory,extra_json", &systems],
    );
    let emitter = |row: &str| {
        format!("{systems}\n\nWet Emitters\nName,Zone,Type,subcategory,area,unit_number,space_heat_system,coords,extra_json\n{row}\n")
    };
    let path = "/SpaceHeatSystem/Space heating/emitters/0";
    let ufh = emitter(
        "Ufh,Room,WetEmitter,ufh,,,Space heating,\"0,0,0|1,0,0\",\"{\"\"emitter_floor_area\"\":8.89}\"",
    );
    let output = prepare(&ufh, A7);
    assert_eq!(
        output.model.pointer(path).unwrap()["emitter_floor_area"],
        json!(8.89)
    );
    for field in [
        "equivalent_specific_thermal_mass",
        "system_performance_factor",
        "frac_convective",
    ] {
        assert!(
            missing_message(&output, path, field),
            "{field}: {:?}",
            output.validation.errors
        );
    }
    let radiator = emitter("Rad,Room,WetEmitter,radiator,,1,Space heating,\"0,0,0|1,0,0\",");
    let output = prepare(&radiator, A7);
    for field in ["c", "n", "frac_convective"] {
        assert!(
            missing_message(&output, path, field),
            "{field}: {:?}",
            output.validation.errors
        );
    }

    let pv = with_extra_section(
        "On-Site Generation",
        "Name,generation_type,pitch,orientation360,peak_power,base_height,width,height,coords,extra_json",
        &["PV,PhotovoltaicSystem,30,180,4,2.5,3,2,\"0,0,0|3,0,0\",\"{\"\"inverter_peak_power_dc\"\":4,\"\"inverter_peak_power_ac\"\":3.6,\"\"inverter_is_inside\"\":false,\"\"inverter_type\"\":\"\"string_inverter\"\"}\""],
    );
    let showers = |row: &str| FIXTURE.replace("Shower,HotWaterDemand,MixerShower,8,FALSE", row);
    let cases = [
        (pv, "/OnSiteGeneration/PV", "ventilation_strategy"),
        (
            showers("Shower,HotWaterDemand,InstantElecShower,,"),
            "/HotWaterDemand/Shower/Shower",
            "rated_power",
        ),
        (
            showers("Shower,HotWaterDemand,MixerShower,8,"),
            "/HotWaterDemand/Shower/mixer",
            "allow_low_flowrate",
        ),
    ];
    for (csv, path, field) in &cases {
        for profile in [A7, A9] {
            let output = prepare(csv, profile);
            assert!(
                missing_message(&output, path, field),
                "{profile:?} {path} {field}: {:?}",
                output.validation.errors
            );
        }
    }

    let pipe = with_extra_section(
        "Water Pipework",
        "Name,Type,length,location,pipework_type",
        &["Pipe,WaterPipework,3,internal,"],
    );
    let output = prepare(&pipe, A7);
    assert!(
        errors_on(&output, "/Water Pipework/Pipe/pipework_type").contains(&"E027"),
        "{:?}",
        output.validation.errors
    );
}

#[test]
fn context_shading_rows_without_angles_or_type_are_blocking() {
    for (row, column) in [
        ("Obs,ContextShading,obstacle,,190,10,1", "start_angle"),
        ("Obs,ContextShading,obstacle,170,,10,1", "end_angle"),
        ("Obs,,,170,190,10,1", "shading_type"),
        ("Obs,ContextShading,Overhang,170,190,10,1", "shading_type"),
    ] {
        let csv = with_extra_section("Context Shading", CONTEXT_HEADER, &[row]);
        for profile in [A7, A9] {
            let output = prepare(&csv, profile);
            let path = format!("/Context Shading/Obs/{column}");
            assert!(
                errors_on(&output, &path).contains(&"E027"),
                "{row} {profile:?}: {:?}",
                output.validation.errors
            );
            assert!(!output.validation.is_valid);
        }
    }
}

/// A window row placed in Exposed Elements has no window columns there; it still never takes
/// the template's window values for them.
#[test]
fn template_clearing_follows_the_element_not_the_section() {
    let window = FIXTURE
        .lines()
        .find(|l| l.starts_with("Window,Room,"))
        .unwrap();
    let csv = FIXTURE.replace(&format!("{window}\n"), "").replace(
        "Door,Room,BuildingElementOpaque,1.8,90,0.9,2,180,0,TRUE,",
        "Door,Room,BuildingElementOpaque,1.8,90,0.9,2,180,0,TRUE,\nWindow,Room,BuildingElementTransparent,1.2,90,1,1.2,0,0.8,,",
    );
    let path = "/Zone/Room/BuildingElement/Window";
    let output = prepare(&csv, A7);
    for field in [
        "frame_area_fraction",
        "free_area_height",
        "mid_height",
        "max_window_open_area",
    ] {
        assert!(
            output.model.pointer(path).unwrap().get(field).is_none()
                && missing_message(&output, path, field),
            "{field}: {:?}",
            output.validation.errors
        );
    }
}

/// A message names the row and the column the user has to fill.
#[test]
fn messages_name_the_row_and_the_column() {
    let kitchen = "Kitchen fan,MechanicalVentilation,1.5,,180,90,";
    for (row, columns) in [
        (
            "Kitchen fan,MechanicalVentilation,,,,,",
            "mid_height_air_flow_path, orientation360, pitch",
        ),
        ("Kitchen fan,MechanicalVentilation,1.5,,180,,", "pitch"),
    ] {
        let csv = FIXTURE.replace(kitchen, row);
        for profile in [A7, A9] {
            let output = prepare(&csv, profile);
            assert!(
                output.validation.errors.iter().any(|e| e.code == "E027"
                    && e.path == "/Ventilation Systems/Kitchen fan"
                    && e.message.contains(&format!("has no {columns}."))),
                "{row} {profile:?}: {:?}",
                output.validation.errors
            );
        }
    }

    let error = transform_geometry_csv(TransformRequest {
        csv: FIXTURE.replace("Lamp,Room,80,1,10", "Lamp,Room,80,,10"),
        schema_json: A7_SCHEMA.into(),
        defaults_json: DEFAULTS.into(),
        profile: ModelProfile::Fhs,
        conversion_profile: A7,
        version_metadata: VersionMetadata {
            hem_core_version: "1.0.0a7".into(),
            fhs_wrapper_version: Some("1.0.0a7".into()),
        },
    })
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("Lighting row 'Lamp'") && error.contains("'count'"),
        "{error}"
    );
}

/// A caller's defaults document may carry a PV system. A PV row, under its own name or the
/// template's "Default PV" name, never takes that template's inverter or a CSV-column value.
#[test]
fn pv_rows_never_take_a_template_pv_installation() {
    let mut defaults: Value = serde_json::from_str(DEFAULTS).unwrap();
    defaults["OnSiteGeneration"] = json!({"Default PV": {
        "type": "PhotovoltaicSystem", "peak_power": 9, "pitch": 45, "orientation360": 90,
        "base_height": 1, "width": 9, "height": 9, "ventilation_strategy": "moderately_ventilated",
        "EnergySupply": "mains elec", "shading": [], "inverter_peak_power_dc": 9,
        "inverter_peak_power_ac": 9, "inverter_is_inside": true, "inverter_type": "optimised_inverter"
    }});
    let defaults = defaults.to_string();
    for name in ["PV", "Default PV"] {
        let csv = with_extra_section(
            "On-Site Generation",
            "Name,generation_type,pitch,orientation360,peak_power,base_height,width,height,coords,extra_json",
            &[&format!("{name},PhotovoltaicSystem,30,180,,2.5,3,2,\"0,0,0|3,0,0\",")],
        );
        let output = prepare_with(&csv, A7, &defaults);
        let path = format!("/OnSiteGeneration/{name}");
        for field in ["peak_power", "inverter_peak_power_ac", "inverter_type"] {
            assert!(
                missing_message(&output, &path, field),
                "{name} {field}: {}",
                output.model["OnSiteGeneration"]
            );
        }
    }
}

/// CSV rows are hash maps; nothing built from them may depend on their iteration order.
#[test]
fn the_prepared_model_and_its_messages_are_the_same_on_every_call() {
    let csv = include_str!("../../fixtures/base_models/H283_target.csv");
    for profile in [A7, A9] {
        let render = || {
            let output = prepare(csv, profile);
            (
                serde_json::to_string(&output.model).unwrap(),
                serde_json::to_string(&output.validation.errors).unwrap(),
                serde_json::to_string(&output.schema_omissions).unwrap(),
            )
        };
        let first = render();
        for _ in 0..8 {
            assert!(render() == first, "{profile:?}");
        }
    }
}
