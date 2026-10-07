//! Functionality to support looking up information from the project input to help interpret returns
//! from the core

use home_energy_model::input::{FuelType, Input, InputForCalcHtcHlp};
use home_energy_model::output::OutputSummary;
use indexmap::IndexMap;
use std::sync::Arc;

#[derive(thiserror::Error, Debug)]
enum ProjectLookupError {
    #[error("No unit prices for fuel type {0}")]
    NoUnitPricesForFuel(FuelType),
}

fn unit_price_lookup(fuel: FuelType) -> Result<Option<f64>, ProjectLookupError> {
    // From SAP 10.2 (17-03-2022) Table 12
    // Units are p/kWh
    Ok(match fuel {
        FuelType::Electricity => 16.49.into(),
        FuelType::MainsGas => 3.64.into(),
        FuelType::Custom => None,
        FuelType::LpgBulk => 6.74.into(),
        FuelType::LpgBottled => 9.46.into(),
        FuelType::LpgCondition11F => 3.46.into(),
        _ => return Err(ProjectLookupError::NoUnitPricesForFuel(fuel)),
        // additional values here for fuel types not captured:
        // "gas": 9.46
        // coal: 5.58
        // oil: 4.94
        // wood: 5.12
    })
}

fn standing_charge_lookup(fuel: FuelType) -> Option<u32> {
    match fuel {
        FuelType::MainsGas => 92.into(),
        FuelType::LpgBulk => 62.into(),
        FuelType::LpgCondition11F => 92.into(),
        FuelType::Custom => None,
        _ => 0.into(),
    }
}

fn select_eer_applicable_usage(project: &Input, usage: &IndexMap<Arc<str>, f64>) -> f64 {
    fn is_vent(source: &str, project: &Input) -> bool {
        project
            .infiltration_ventilation()
            .mechanical_ventilation()
            .contains_key(source)
    }

    fn is_space_heating(source: &str, project: &Input) -> bool {
        // The premise is that the source either is a space heat system itself, directly identifies
        // itself as space_heating or is an auxiliary pump
        project
            .space_heat_system()
            .is_some_and(|systems| systems.contains_key(source))
            || source.contains("space_heating")
            || source.contains("auxiliary")
    }

    fn is_water_heating(source: &str, project: &Input) -> bool {
        // The premise is that any cylinder drawn water will always reference hw cylinder (the only
        // allowed name for a storage tank), or the source will be the heat source or heat source
        // wet of the cylinder or the source will directly state that it is water_heating
        source.contains("hw cylinder")
            || project
                .hot_water_source()
                .get("hw_cylinder")
                .is_some_and(|s| {
                    s.contains_heat_source(source) || s.contains_heat_source_wet_reference(source)
                })
            || source.contains("water_heating")
    }

    fn is_lighting(source: &str) -> bool {
        // Lighting is either exactly "lighting" or exactly "topup" (which refers to additional
        // non fixture based lighting)
        ["lighting", "topup"].contains(&source)
    }

    usage
        .iter()
        .filter(|(source, _)| {
            is_vent(source.as_ref(), project)
                || is_space_heating(source.as_ref(), project)
                || is_water_heating(source.as_ref(), project)
                || is_lighting(source.as_ref())
        })
        .map(|(_, &kwh)| kwh)
        .sum()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FuelOutput {
    pub(crate) fuel: FuelType,
    pub(crate) eer_energy: f64,
    pub(crate) unit_price: Option<f64>,
    pub(crate) standing_charge: Option<u32>,
}

pub(crate) fn by_fuel(
    project: &Input,
    output_summary: &OutputSummary,
) -> anyhow::Result<Vec<FuelOutput>> {
    output_summary
        .delivered_energy()
        .iter()
        .filter_map(|(supply, usage)| {
            if let Some(energy_supply) = project.energy_supply().get(supply.as_ref()) {
                let fuel = energy_supply.fuel;
                Some(Ok(FuelOutput {
                    fuel,
                    eer_energy: select_eer_applicable_usage(project, usage),
                    unit_price: match unit_price_lookup(fuel) {
                        Ok(price) => price,
                        Err(e) => {
                            return Some(Err(e.into()));
                        }
                    },
                    standing_charge: standing_charge_lookup(fuel),
                }))
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use home_energy_model::input::ExternalConditionsInput;
    use home_energy_model::{load_weather_data, WeatherFileType};
    use rstest::*;
    use serde_json::{json, Value};
    use std::io::Cursor;

    #[fixture]
    fn instant_elec_project() -> Input {
        // Thing that matters here is that there are two EnergySupply entries
        // One called "mains elec" with fuel = electricity
        // One called "mains gas" with fuel = mains_gas
        // Two mechanical vents "mechvent1", "mechvent2"
        // One SpaceHeatSystem called "main"
        // A "hw cylinder" with a heat source called "immersion"

        let mut project = serde_json::from_str::<Value>(include_str!(
            "../../examples/input/future_homes_standard/demo_FHS.json"
        ))
        .unwrap();
        let weather = ExternalConditionsInput::from(
            load_weather_data(
                Cursor::new(include_str!("./RAF_Bedford_01.epw")),
                WeatherFileType::Epw,
            )
            .unwrap(),
        );
        let shading_segments = project["ExternalConditions"]["shading_segments"].clone();
        project["ExternalConditions"] = serde_json::to_value(weather).unwrap();
        project["ExternalConditions"]["shading_segments"] = shading_segments;
        // TODO complete when functions are implemented during migration to 1.0.0a4
        // initial_preprocessing(&mut project);
        // final_preprocessing(&mut project);

        let _input: Input = serde_json::from_value(project).unwrap();

        todo!()
    }

    #[fixture]
    fn instant_elec_output_summary() -> OutputSummary {
        // delivered energy matters here

        // make OutputSummary derive Deserialize, and uncomment
        serde_json::from_value(json!({
            "total_floor_area": 100,
            "space_heat_demand_total": 1000,
            "space_cool_demand_total": -100,
            "electricity_peak_consumption": {
                "peak": 10,
                "index": 1,
                "month": 1,
                "day": 1,
                "hour": 1,
            },
            "energy_supply": {
                "mains elec": {
                    "generation": 0,
                    "consumption": 1100,
                    "generation_to_consumption": 0,
                    "generation_to_grid": 0,
                    "generation_to_diverter": 0,
                    "generation_to_storage": 0,
                    "grid_to_consumption": 1100,
                    "grid_to_storage": 0,
                    "storage_to_consumption": 0,
                    "storage_efficiency": null,
                    "net_import": 1100,
                    "total_gross_import": 1100,
                    "total_gross_export": 0,
                },
                "delivered_energy": {
                    "mains elec": {
                        "mechvent1": 100,  // vent
                        "mechvent3": 200,  // not a vent
                        "lighting": 300,  // counts as lighting
                        "something random": 400,  // not relevant for EER
                        "hw cylinder": 500,  // counts as water heating
                        "immersion": 600,  // counts as water heating
                        "something_water_heating": 700,  // counts as water heating
                    },
                    "mains gas": {
                        "main": 1000,  // heating system counts
                        "Hobs": 100,  // not relevant for EER,
                        "something_space_heating": 200,  // counts as space heating
                        "auxiliary_pump_thing": 300,  // counts as a space heating system pump
                    }
                },
                "hot_water_demand_daily_75th_percentile": {
                    "hw cylinder": 1000,
                }
            }
        }))
        .unwrap()
    }

    #[fixture]
    fn heat_network_project() -> Input {
        // Thing that matters here is that there is two EnergySupply's
        // One called "mains elec" with fuel = electricity
        // One called "custom_heat_network_supply" with fuel = custom
        // One mechanical vent "cMEV"
        // One SpaceHeatSystem called "SpaceHeatSystem1"
        // A "hw cylinder" with a heat source called "heat network"

        let mut project = serde_json::from_str::<Value>(include_str!(
            "../../examples/input/future_homes_standard/DESN-H-End-02-HN-cMEV.json"
        ))
        .unwrap();
        let weather = ExternalConditionsInput::from(
            load_weather_data(
                Cursor::new(include_str!("./RAF_Bedford_01.epw")),
                WeatherFileType::Epw,
            )
            .unwrap(),
        );
        let shading_segments = project["ExternalConditions"]["shading_segments"].clone();
        project["ExternalConditions"] = serde_json::to_value(weather).unwrap();
        project["ExternalConditions"]["shading_segments"] = shading_segments;
        // TODO complete when functions are implemented during migration to 1.0.0a4
        // initial_preprocessing(&mut project);
        // final_preprocessing(&mut project);

        let _input: Input = serde_json::from_value(project).unwrap();

        todo!()
    }

    #[fixture]
    fn heat_network_output_summary() -> OutputSummary {
        // delivered energy matters here

        // uncomment below once OutputSummary derives Deserialize
        serde_json::from_value(json!({
            "total_floor_area": 100,
            "space_heat_demand_total": 1000,
            "space_cool_demand_total": -100,
            "electricity_peak_consumption": {
                "peak": 10,
                "index": 1,
                "month": 1,
                "day": 1,
                "hour": 1,
            },
            "energy_supply": {
                "mains elec": {
                    "generation": 0,
                    "consumption": 1100,
                    "generation_to_consumption": 0,
                    "generation_to_grid": 0,
                    "generation_to_diverter": 0,
                    "generation_to_storage": 0,
                    "grid_to_consumption": 1100,
                    "grid_to_storage": 0,
                    "storage_to_consumption": 0,
                    "storage_efficiency": null,
                    "net_import": 1100,
                    "total_gross_import": 1100,
                    "total_gross_export": 0,
                }
            },
            "delivered_energy": {
                "mains elec": {
                    "cMEV": 100,  // vent
                    "mechvent3": 200,  // not a vent
                    "topup": 300,  // counts as lighting
                    "something random": 400,  // not relevant for EER
                    "hw cylinder": 500,  // counts as water heating
                    "heat network": 600,  // counts as water heating
                    "something_water_heating": 700,  // counts as water heating
                },
                "custom_heat_network_supply": {
                    "SpaceHeatSystem1": 1000,  // heating system counts
                    "Hobs": 100,  // not relevant for EER,
                    "something_space_heating": 200,  // counts as space heating
                    "auxiliary_pump_thing": 300,  // counts as a space heating system pump
                }
            },
            "hot_water_demand_daily_75th_percentile": {
                "hw cylinder": 1000
            }
        }))
        .unwrap()
    }

    #[fixture]
    fn electricity_and_gas_project() -> Input {
        // Thing that matters here is that there are three EnergySupply objects
        // One called "mains elec" with fuel = electricity
        // One called "mains gas" with fuel = mains_gas
        // One called "LPG_bulk" with fuel = LPG_bulk
        // One mechanical vent "cMEV"
        // One SpaceHeatSystem called "SpaceHeatSystem1"
        // A "hw cylinder" with a heat source called "boiler"

        let mut project = serde_json::from_str::<Value>(include_str!(
            "../../examples/input/future_homes_standard/DESN-H-End-02-Blr-cMEV-Combi-lpg-bulk.json"
        ))
        .unwrap();
        let weather = ExternalConditionsInput::from(
            load_weather_data(
                Cursor::new(include_str!("./RAF_Bedford_01.epw")),
                WeatherFileType::Epw,
            )
            .unwrap(),
        );
        let shading_segments = project["ExternalConditions"]["shading_segments"].clone();
        project["ExternalConditions"] = serde_json::to_value(weather).unwrap();
        project["ExternalConditions"]["shading_segments"] = shading_segments;
        // TODO complete when functions are implemented during migration to 1.0.0a4
        // initial_preprocessing(&mut project);
        // final_preprocessing(&mut project);

        let _input: Input = serde_json::from_value(project).unwrap();

        todo!()
    }

    #[fixture]
    fn electricity_and_gas_output_summary() -> OutputSummary {
        // delivered energy matters here

        // uncomment below once OutputSummary derives Deserialize
        serde_json::from_value(json!({
            "total_floor_area": 100,
            "space_heat_demand_total": 1000,
            "space_cool_demand_total": -100,
            "electricity_peak_consumption": {
                "peak": 10,
                "index": 1,
                "month": 1,
                "day": 1,
                "hour": 1,
            },
            "energy_supply": {
                "mains elec": {
                    "generation": 0,
                    "consumption": 1100,
                    "generation_to_consumption": 0,
                    "generation_to_grid": 0,
                    "generation_to_diverter": 0,
                    "generation_to_storage": 0,
                    "grid_to_consumption": 1100,
                    "grid_to_storage": 0,
                    "storage_to_consumption": 0,
                    "storage_efficiency": null,
                    "net_import": 1100,
                    "total_gross_import": 1100,
                    "total_gross_export": 0,
                }
            },
            "delivered_energy": {
                "mains elec": {
                    "cMEV": 100,  // vent
                    "lighting": 300,  // counts as lighting
                    "something random": 400,  // not relevant for EER
                    "hw cylinder": 20,  // counts as water heating
                },
                "LPG_bulk": {
                    "boiler": 600,  // counts as water heating
                    "Hobs": 100,  // not relevant for EER,
                    "something_space_heating": 200,  // counts as space heating
                },
                "mains gas": {},
            },
            "hot_water_demand_daily_75th_percentile": {
                "hw cylinder": 1000
            }
        }))
        .unwrap()
    }

    #[rstest]
    #[ignore = "ignored until fixtures can be fully implemented"]
    fn test_without_custom_fuel(
        instant_elec_project: Input,
        instant_elec_output_summary: OutputSummary,
    ) {
        // Given a by fuel lookup is called
        let fuels = by_fuel(&instant_elec_project, &instant_elec_output_summary).unwrap();
        // then the total delivered energy is calculated based on the floor area normalised
        // calculated properties
        assert_eq!(
            fuels,
            vec![
                FuelOutput {
                    fuel: FuelType::Electricity,
                    eer_energy: 2200.,
                    unit_price: 16.49.into(),
                    standing_charge: 0.into(),
                },
                FuelOutput {
                    fuel: FuelType::MainsGas,
                    eer_energy: 1500.,
                    unit_price: 3.64.into(),
                    standing_charge: 92.into(),
                }
            ]
        );
    }

    #[rstest]
    #[ignore = "ignored until fixtures can be fully implemented"]
    fn test_with_custom_fuel(
        heat_network_project: Input,
        heat_network_output_summary: OutputSummary,
    ) {
        // Given a by fuel lookup is called
        let fuels = by_fuel(&heat_network_project, &heat_network_output_summary).unwrap();
        // then the total delivered energy is calculated based on the floor area normalised
        // calculated properties
        assert_eq!(
            fuels,
            vec![
                FuelOutput {
                    fuel: FuelType::Electricity,
                    eer_energy: 2200.,
                    unit_price: 16.49.into(),
                    standing_charge: 0.into(),
                },
                FuelOutput {
                    fuel: FuelType::MainsGas,
                    eer_energy: 1500.,
                    unit_price: None,
                    standing_charge: None,
                }
            ]
        );
    }

    #[rstest]
    #[ignore = "ignored until fixtures can be fully implemented"]
    fn test_electricity_and_gas(
        electricity_and_gas_project: Input,
        electricity_and_gas_output_summary: OutputSummary,
    ) {
        // Given a project that has electricity, LPG_bulk and mains_gas energy supplies
        // When a by_fuel lookup is called
        let fuels = by_fuel(
            &electricity_and_gas_project,
            &electricity_and_gas_output_summary,
        )
        .unwrap();
        // Then the total delivered energy is calculated from the
        // floor-area-normalised results for all three energy supplies.
        // Note that nothing uses the mains_gas supply, so it has zero delivered energy.
        assert_eq!(
            fuels,
            vec![
                FuelOutput {
                    fuel: FuelType::Electricity,
                    eer_energy: 420.,
                    unit_price: 16.49.into(),
                    standing_charge: 0.into(),
                },
                FuelOutput {
                    fuel: FuelType::LpgBulk,
                    eer_energy: 800.,
                    unit_price: 6.74.into(),
                    standing_charge: 62.into(),
                },
                FuelOutput {
                    fuel: FuelType::MainsGas,
                    eer_energy: 0.,
                    unit_price: 3.64.into(),
                    standing_charge: 92.into(),
                }
            ]
        );
    }
}
