use jsonschema::Validator;
use serde_json::Value;
use std::sync::LazyLock;
use thiserror::Error;

static FHS_SCHEMA_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    let schema = serde_json::from_str(include_str!("../../schema/input_fhs.schema.json")).unwrap();
    jsonschema::validator_for(&schema).unwrap()
});

#[derive(Debug, Error)]
#[error("FHS input validation failed: {errors}")]
pub struct SchemaValidationError {
    pub errors: String,
}

pub(crate) fn apply_schema_validation(input: &Value) -> Result<(), SchemaValidationError> {
    let evaluation = FHS_SCHEMA_VALIDATOR.evaluate(input);
    if evaluation.flag().valid {
        Ok(())
    } else {
        Err(SchemaValidationError {
            errors: evaluation
                .iter_errors()
                .map(|e| format!("{}: {}", e.instance_location, e.error))
                .collect::<Vec<_>>()
                .join("\n"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::*;
    use serde_json::json;

    #[fixture]
    fn project() -> Value {
        serde_json::from_str(include_str!(
            "./test_assets/fixtures/minimal_FHS_input.json"
        ))
        .unwrap()
    }

    #[rstest]
    fn test_valid_input_does_not_error(project: Value) {
        assert!(apply_schema_validation(&project).is_ok());
    }

    #[rstest]
    fn test_string_instead_of_number_errors(mut project: Value) {
        project["NumberOfBedrooms"] = json!("2");
        assert!(apply_schema_validation(&project).is_err());
    }

    #[rstest]
    fn test_value_instead_of_array_errors(mut project: Value) {
        project["ExternalConditions"]["shading_segments"] = json!({"start360": 0, "end360": 45});
        assert!(apply_schema_validation(&project).is_err());
    }

    #[rstest]
    fn test_insufficient_array_length_errors(mut project: Value) {
        project["ExternalConditions"]["shading_segments"] = json!([
            {"start360": 0, "end360": 45}
        ]);
        assert!(apply_schema_validation(&project).is_err());
    }

    #[rstest]
    fn test_multiple_problems_all_included_in_message(mut project: Value) {
        project["NumberOfBedrooms"] = json!("2");
        project["ExternalConditions"]["shading_segments"] = json!(
            {"number": 1, "start360": 0, "end360": 45}
        );
        let result = apply_schema_validation(&project);
        if let Err(SchemaValidationError { errors }) = result {
            let error_message = errors.to_string();
            assert!(error_message.contains("NumberOfBedrooms"));
            assert!(error_message.contains("shading_segments"));
        } else {
            panic!("Expected validation error, got {:?}", result);
        }
    }

    #[rstest]
    fn test_incorrect_enum_value_errors(mut project: Value) {
        project["HeatingControlType"] = json!("something invalid");
        let result = apply_schema_validation(&project);
        if let Err(SchemaValidationError { errors }) = result {
            let error_message = errors.to_string();
            assert!(error_message.contains("SeparateTempControl"));
        } else {
            panic!("Expected validation error, got {:?}", result);
        }
    }

    #[rstest]
    fn test_missing_required_value_errors(mut project: Value) {
        project["ExternalConditions"]
            .as_object_mut()
            .unwrap()
            .remove("shading_segments");
        let result = apply_schema_validation(&project);
        if let Err(SchemaValidationError { errors }) = result {
            let error_message = errors.to_string();
            assert!(error_message.contains("shading_segments"));
            assert!(error_message.contains("required"));
        } else {
            panic!("Expected validation error, got {:?}", result);
        }
    }

    #[rstest]
    fn test_missing_one_of_errors(mut project: Value) {
        project["ColdWaterSource"]
            .as_object_mut()
            .unwrap()
            .remove("header tank");
        assert!(apply_schema_validation(&project).is_err());
    }

    #[rstest]
    fn test_both_one_of_errors(mut project: Value) {
        project["Zone"]["zone 1"]["BuildingElement"]["window 0"]["u_value"] = json!(1.2);
        project["Zone"]["zone 1"]["BuildingElement"]["window 0"]
            ["thermal_resistance_construction"] = json!(0.4);
        assert!(apply_schema_validation(&project).is_err());
    }

    #[rstest]
    fn test_if_then_conditions(mut project: Value) {
        project["HotWaterDemand"]["Shower"]["mixer"]
            .as_object_mut()
            .unwrap()
            .remove("flowrate");
        assert!(apply_schema_validation(&project).is_err());
    }

    #[rstest]
    fn test_if_then_conditions_negation(mut project: Value) {
        let mixer = project["HotWaterDemand"]["Shower"]["mixer"]
            .as_object_mut()
            .unwrap();
        mixer["type"] = json!("InstantElecShower");
        mixer.remove("allow_low_flowrate");
        let result = apply_schema_validation(&project);
        if let Err(SchemaValidationError { errors }) = result {
            assert!(errors.contains("flowrate"));
        } else {
            panic!("Expected validation error, got {:?}", result);
        }
    }

    #[rstest]
    fn test_flowrate_min_based_on_allow_low_flowrate_false(mut project: Value) {
        // Given a MixerShower with allow_low_flow_rate set to False
        project["HotWaterDemand"]["Shower"]["mixer"]["allow_low_flowrate"] = json!(false);
        // And a flowrate below the standard mininum of 8 l/min
        project["HotWaterDemand"]["Shower"]["mixer"]["flowrate"] = json!(5.0);
        let result = apply_schema_validation(&project);
        if let Err(SchemaValidationError { errors }) = result {
            assert!(errors.contains("flowrate"));
        } else {
            panic!("Expected validation error, got {:?}", result);
        }
    }

    #[rstest]
    fn test_if_else_conditions(mut project: Value) {
        // If a shower is not of type MixerShower then it must have a `rated_power` property
        project["HotWaterDemand"]["Shower"]["other"] = json!({
            "type": "InstantElecShower",
            "EnergySupply": "something",
            "ColdWaterSource": "header tank",
        });
        assert!(apply_schema_validation(&project).is_err());
    }

    #[rstest]
    fn test_all_of_conditions(mut project: Value) {
        // Within SpaceHeatSystem all systems much meet the requirements for their specific type
        // InstantElecHeater with a missing EnergySupply property
        project["SpaceHeatSystem"]["main 1"] = json!({
            "type": "InstantElecHeater",
            "rated_power": 6.0,
            "convective_type": "Wall heating, radiant ceiling panels, accumulation stoves",
        });
        // WetDistribution with missing dry_core_min_output and dry_core_max_output properties
        project["SpaceHeatSystem"]["main 2"] = json!({
            "type": "ElecStorageHeater",
            "pwr_in": 12,
            "rated_power_instant": 12,
            "storage_capacity": 12,
            "air_flow_type": "fan-assisted",
            "frac_convective": 12,
            "fan_pwr": 12,
            "n_units": 12,
            "EnergySupply": "thing",
            "Zone": "thing",
        });
        let result = apply_schema_validation(&project);
        if let Err(SchemaValidationError { errors }) = result {
            assert!(errors.contains("convective_type"));
            assert!(errors.contains("EnergySupply"));
            assert!(errors.contains("dry_core_min_output"));
            assert!(errors.contains("dry_core_max_output"));
            assert!(errors.contains("required"));
        } else {
            panic!("Expected validation error, got {:?}", result);
        }
    }

    #[rstest]
    fn test_any_of_conditions(mut project: Value) {
        // BuildingElementOpaque must have either `u_value` or `thermal_resistance_construction`
        project["Zone"]["zone 1"]["BuildingElement"]["roof 0"]
            .as_object_mut()
            .unwrap()
            .remove("thermal_resistance_construction");
        assert!(apply_schema_validation(&project).is_err());
    }

    #[rstest]
    fn test_conditionally_required(mut project: Value) {
        // Given a unfilled_sealed party wall without party_wall_lining_type
        project["Zone"]["zone 1"]["BuildingElement"]["wall 0"]
            .as_object_mut()
            .unwrap()
            .remove("party_wall_lining_type");
        let result = apply_schema_validation(&project);
        if let Err(SchemaValidationError { errors }) = result {
            assert!(errors.contains("party_wall_lining_type"));
            assert!(errors.contains("required"));
        } else {
            panic!("Expected validation error, got {:?}", result);
        }
    }

    #[rstest]
    fn test_nested_conditions(mut project: Value) {
        // SpaceHeatSystem->type=WetDistribution->emitters->wet_emitter_type=radiator
        // has different required properties to
        // `SpaceHeatSystem->type=WetDistribution->emitters->wet_emitter_type=ufh
        // Radiator wet emitter expects `n` as a property
        project["SpaceHeatSystem"]["main 1"] = json!({
            "type": "WetDistribution",
            "temp_diff_emit_dsgn": 12,
            "variable_flow": true,
            "HeatSource": {"name": "something"},
            "ecodesign_controller": {"ecodesign_control_class": 1},
            "design_flow_temp": 20,
            "Zone": "zone 1",
            "min_flow_rate": 12,
            "max_flow_rate": 12,
            "thermal_mass": 12,
            "emitters": [{"wet_emitter_type": "radiator", "frac_convective": 12, "c": 1}],
        });
        // Whereas ufh emitter expects `emitter_floor_area` as a property
        project["SpaceHeatSystem"]["main 2"] = json!({
            "type": "WetDistribution",
            "temp_diff_emit_dsgn": 12,
            "variable_flow": true,
            "HeatSource": {"name": "something"},
            "ecodesign_controller": {"ecodesign_control_class": 1},
            "design_flow_temp": 20,
            "Zone": "zone 1",
            "min_flow_rate": 12,
            "max_flow_rate": 12,
            "thermal_mass": 12,
            "emitters": [
                {
                    "wet_emitter_type": "ufh",
                    "frac_convective": 12,
                    "equivalent_specific_thermal_mass": 12,
                    "system_performance_factor": 12,
                }
            ],
        });
        let result = apply_schema_validation(&project);
        if let Err(SchemaValidationError { errors }) = result {
            assert!(errors.contains("Unevaluated"));
            assert!(errors.contains("emitter_floor_area"));
            assert!(errors.contains("required"));
        } else {
            panic!("Expected validation error, got {:?}", result);
        }
    }

    fn merge_json_values(a: &mut Value, b: Value) {
        match (a, b) {
            (a @ &mut Value::Object(_), Value::Object(b)) => {
                let a = a.as_object_mut().unwrap();
                // clobber target object if item being merged is empty object
                if b.is_empty() {
                    a.clear();
                    return;
                }
                for (k, v) in b {
                    merge_json_values(a.entry(k).or_insert(Value::Null), v);
                }
            }
            (a, b) => *a = b,
        }
    }

    #[rstest]
    fn test_conditions_apply_to_multiple_values(mut project: Value) {
        // MechanicalVents must always have:
        project["InfiltrationVentilation"]["MechanicalVentilation"] = json!({
            "vent1": {
                "EnergySupply": "mains elec",
                "design_outdoor_air_flow_rate": 1,
            },
            "vent2": {
                "EnergySupply": "mains elec",
                "design_outdoor_air_flow_rate": 1,
            },
        });
        // Centralised continuous MEV and MVHR must have
        // measured_fan_power and measured_air_flow_rate, or SFP
        // This stacks with MVHR individual requirements of mvhr_eff, mvhr_location, ductwork,
        // position exhaust and location
        merge_json_values(
            &mut project["InfiltrationVentilation"]["MechanicalVentilation"]["vent1"],
            json!({
                "vent_type": "MVHR",
                "mvhr_eff": 1,
                "mvhr_location": "inside",
                "measured_air_flow_rate": 1,
                "position_intake": {"mid_height_air_flow_path": 1.5, "orientation360": 90, "pitch": 60},
                "position_exhaust": {
                    "mid_height_air_flow_path": 1.5,
                    "orientation360": 90,
                    "pitch": 60,
                },
            }),
        );
        // whereas Decentralised continuous MEV and Intermittent MEV
        // must provide SFP, mid_height_airflow_path, orientation360 and pitch directly
        merge_json_values(
            &mut project["InfiltrationVentilation"]["MechanicalVentilation"]["vent2"],
            json!({
                "vent_type": "Intermittent MEV",
                "mid_height_air_flow_path": 1.5,
                "orientation360": 90,
                "pitch": 60,
            }),
        );

        let result = apply_schema_validation(&project);
        if let Err(SchemaValidationError { errors }) = result {
            assert!(errors.contains("Unevaluated"));
            assert!(errors.contains("measured_air_flow_rate"));
            assert!(errors.contains("ductwork"));
            assert!(errors.contains("required"));
            assert!(errors.contains("SFP"));
        } else {
            panic!("Expected validation error, got {:?}", result);
        }
    }

    #[rstest]
    fn test_value_out_of_range_errors(mut project: Value) {
        project["NumberOfBedrooms"] = json!(-1);
        assert!(apply_schema_validation(&project).is_err());
    }

    #[rstest]
    fn test_additional_top_level_property_raises(mut project: Value) {
        project["SomeOtherTopLevelProperty"] = json!("a_value");
        let result = apply_schema_validation(&project);
        if let Err(SchemaValidationError { errors }) = result {
            assert!(errors.contains("Unevaluated"));
        } else {
            panic!("Expected validation error, got {:?}", result);
        }
    }

    #[rstest]
    fn test_additional_property_lower_level_errors(mut project: Value) {
        project["ExternalConditions"]["SomeNewThing"] = json!("val");
        let result = apply_schema_validation(&project);
        if let Err(SchemaValidationError { errors }) = result {
            assert!(errors.contains("Unevaluated"));
            assert!(errors.contains("SomeNewThing"));
        } else {
            panic!("Expected validation error, got {:?}", result);
        }
    }

    #[rstest]
    fn test_exclusive_minimum_errors(mut project: Value) {
        // Given an electric battery with 0 charge/discharge efficiency
        project["EnergySupply"]["mains elec"] = json!({
            "fuel": "electricity",
            "ElectricBattery": {
                "capacity": 2,
                "charge_discharge_efficiency_round_trip": 0,
                "minimum_charge_rate_one_way_trip": 0.001,
                "maximum_charge_rate_one_way_trip": 1.5,
                "maximum_discharge_rate_one_way_trip": 1.25,
                "battery_location": "inside",
            },
        });
        assert!(apply_schema_validation(&project).is_err());
    }

    #[rstest]
    fn test_dependency_at_offset_levels(mut project: Value) {
        // Given PartO_active_cooling_required at the top level
        // But no SpaceCoolSystem defined within a zone
        project["PartO_active_cooling_required"] = json!(true);
        project["Zone"]["zone 1"]
            .as_object_mut()
            .unwrap()
            .remove("SpaceCoolSystem");
        let result = apply_schema_validation(&project);
        if let Err(SchemaValidationError { errors }) = result {
            assert!(errors.contains("SpaceCoolSystem"));
            assert!(errors.contains("required"));
        } else {
            panic!("Expected validation error, got {:?}", result);
        }
    }

    #[rstest]
    fn test_mutually_absent_appliance_settings(mut project: Value) {
        // Given a specification with neither Fridge nor Fridge-Freezer installed
        project["Appliances"]["Fridge"] = json!("Not Installed");
        project["Appliances"]["Fridge-Freezer"] = json!("Not Installed");
        assert!(apply_schema_validation(&project).is_err());
    }

    #[rstest]
    fn test_must_have_mains_elec(mut project: Value) {
        // Given input without a "mains elec" EnergySupply (which will be used for Appliances)
        project["EnergySupply"]
            .as_object_mut()
            .unwrap()
            .remove("mains elec");
        let result = apply_schema_validation(&project);
        if let Err(SchemaValidationError { errors }) = result {
            assert!(errors.contains("mains elec"));
            assert!(errors.contains("required"));
        } else {
            panic!("Expected validation error, got {:?}", result);
        }
    }

    #[rstest]
    fn test_mains_elec_must_use_electricity(mut project: Value) {
        project["EnergySupply"]["mains elec"]["fuel"] = json!("mains gas");
        assert!(apply_schema_validation(&project).is_err());
    }

    #[rstest]
    fn test_heat_pump_backup_properties_not_allowed_without_backup_ctrl_type(mut project: Value) {
        // Given a HeatSourceWet of type HeatPump with backup_ctrl_type = None
        project["HeatSourceWet"] = json!({
            "heatpump": {
                "type": "HeatPump",
                "is_heat_network": false,
                "EnergySupply": "mains elec",
                "source_type": "OutsideAir",
                "sink_type": "Water",
                "backup_ctrl_type": "None",
                "modulating_control": true,
                "min_modulation_rate_35": 0.2,
                "min_modulation_rate_55": 0.2,
                "temp_return_feed_max": 40,
                "temp_lower_operating_limit": -10,
                "min_temp_diff_flow_return_for_hp_to_operate": 5,
                "var_flow_temp_ctrl_during_test": false,
                "power_heating_circ_pump": 0.1,
                "power_source_circ_pump": 0.1,
                "power_standby": 0.01,
                "power_crankcase_heater": 0.01,
                "power_off": 0.0,
                "test_data_EN14825": [],
                "time_constant_onoff_operation": 1,
                "power_max_backup": 1,
            }
        });
        let result = apply_schema_validation(&project);
        if let Err(SchemaValidationError { errors }) = result {
            assert!(errors.contains("power_max_backup"));
            assert!(errors.contains("unexpected"));
        } else {
            panic!("Expected validation error, got {:?}", result);
        }
    }

    #[rstest]
    fn test_heat_pump_backup_properties_required_with_backup_ctrl_type(mut project: Value) {
        // Given a HeatSourceWet of type HeatPump with a numerical power_max_backup
        project["HeatSourceWet"] = json!({
            "heatpump": {
                "type": "HeatPump",
                "is_heat_network": false,
                "EnergySupply": "mains elec",
                "source_type": "OutsideAir",
                "sink_type": "Water",
                "backup_ctrl_type": "TopUp",
                "modulating_control": true,
                "min_modulation_rate_35": 0.2,
                "min_modulation_rate_55": 0.2,
                "temp_return_feed_max": 40,
                "temp_lower_operating_limit": -10,
                "min_temp_diff_flow_return_for_hp_to_operate": 5,
                "var_flow_temp_ctrl_during_test": false,
                "power_heating_circ_pump": 0.1,
                "power_source_circ_pump": 0.1,
                "power_standby": 0.01,
                "power_crankcase_heater": 0.01,
                "power_off": 0.0,
                "test_data_EN14825": [],
                "time_constant_onoff_operation": 1,
            }
        });
        assert!(apply_schema_validation(&project).is_err());
    }

    #[rstest]
    fn test_heat_network_energy_supply_may_be_custom(mut project: Value) {
        // Given a top level HeatSourceWet that is a heat network (so here the
        // "EnergySupply" of  "heat network" is the DHN generator site. It is not
        // the electrical power to the HIU)
        project["HeatSourceWet"] = json!({
            "heat network": {
                "type": "HIU",
                "is_heat_network": true,
                "heat_network_type": "sleeved DHN",
                "HIU_daily_loss": 1,
                "power_max": 1,
                "building_level_distribution_losses": 1,
                "EnergySupply": {
                    "name": "custom_heat_network_supply",
                    "factor": {
                        "Emissions Factor kgCO2e/kWh": 1,
                        "Emissions Factor kgCO2e/kWh including out-of-scope emissions": 1,
                        "Primary Energy Factor kWh/kWh delivered": 1,
                    },
                    "is_export_capable": false,
                },
            }
        });
        assert!(apply_schema_validation(&project).is_ok());
        // And a reference to an existing EnergySupply is also fine
        project["HeatSourceWet"]["heat network"]["EnergySupply"] = json!("mains elec");
        assert!(apply_schema_validation(&project).is_ok());
    }

    #[rstest]
    fn test_heat_pump_may_have_heat_network_energy_supply_that_is_custom(mut project: Value) {
        // Given a locally powered heat pump, sourcing energy from a heat network
        // i.e. a 5th gen heat pump
        project["HeatSourceWet"] = json!({
            "heat pump": {
                "type": "HeatPump",
                "is_heat_network": false,
                "source_type": "HeatNetwork",
                "sink_type": "Water",
                "backup_ctrl_type": "TopUp",
                "modulating_control": true,
                "min_modulation_rate_35": 0.35,
                "min_modulation_rate_55": 0.4,
                "time_constant_onoff_operation": 140,
                "temp_return_feed_max": 70.0,
                "temp_lower_operating_limit": -5.0,
                "min_temp_diff_flow_return_for_hp_to_operate": 0.0,
                "var_flow_temp_ctrl_during_test": true,
                "power_source_circ_pump": 0.010,
                "power_standby": 0.015,
                "power_crankcase_heater": 0.01,
                "power_off": 0.015,
                "power_max_backup": 3.0,
                "test_data_EN14825": [],
                "EnergySupply": "mains elec",
                "EnergySupply_heat_network": {
                    "name": "custom_heat_network_supply",
                    "factor": {
                        "Emissions Factor kgCO2e/kWh": 1,
                        "Emissions Factor kgCO2e/kWh including out-of-scope emissions": 1,
                        "Primary Energy Factor kWh/kWh delivered": 1,
                    },
                    "is_export_capable": false,
                },
                "temp_distribution_heat_network": 60,
            }
        });
        // When the validation is run there is no validation error
        // Because it is legitimate to have "EnergySupply_heat_network" using a custom fuel
        // as it refers to the heat network generator site
        assert!(apply_schema_validation(&project).is_ok());
    }

    #[rstest]
    fn test_non_heat_network_may_not_have_custom_energy_supply(mut project: Value) {
        // Given a heat pump with a heat network source, with is_heat_network = False
        project["HeatSourceWet"] = json!({
            "heat pump": {
                "type": "HeatPump",
                "is_heat_network": false,
                "source_type": "HeatNetwork",
                "sink_type": "Water",
                "backup_ctrl_type": "TopUp",
                "modulating_control": true,
                "min_modulation_rate_35": 0.35,
                "min_modulation_rate_55": 0.4,
                "time_constant_onoff_operation": 140,
                "temp_return_feed_max": 70.0,
                "temp_lower_operating_limit": -5.0,
                "min_temp_diff_flow_return_for_hp_to_operate": 0.0,
                "var_flow_temp_ctrl_during_test": true,
                "power_source_circ_pump": 0.010,
                "power_standby": 0.015,
                "power_crankcase_heater": 0.01,
                "power_off": 0.015,
                "power_max_backup": 3.0,
                "test_data_EN14825": [],
                "EnergySupply": {
                    "name": "custom_heat_pump_supply",
                    "factor": {
                        "Emissions Factor kgCO2e/kWh": 1,
                        "Emissions Factor kgCO2e/kWh including out-of-scope emissions": 1,
                        "Primary Energy Factor kWh/kWh delivered": 1,
                    },
                    "is_export_capable": false,
                },
                "EnergySupply_heat_network": "mains gas",
                "temp_distribution_heat_network": 60,
            }
        });
        // Then the "EnergySupply" of the HeatPump may NOT be custom because it is not itself the
        // energy supply of the heat network (it's just to power the pump). Only when the
        // top level HeatSourceWet is a heat network (i.e. is_heat_network is true), should
        // a custom "EnergySupply" be permitted
        assert!(apply_schema_validation(&project).is_err());
    }

    #[rstest]
    fn test_air_sourced_heat_pump_test_subschema(mut project: Value) {
        // Given a heat pump with outside air as its source
        project["HeatSourceWet"] = json!({
            "heat pump": {
                "type": "HeatPump",
                "is_heat_network": false,
                "source_type": "OutsideAir",
                "sink_type": "Water",
                "backup_ctrl_type": "TopUp",
                "modulating_control": true,
                "min_modulation_rate_35": 0.35,
                "min_modulation_rate_55": 0.4,
                "time_constant_onoff_operation": 140,
                "temp_return_feed_max": 70.0,
                "temp_lower_operating_limit": -5.0,
                "min_temp_diff_flow_return_for_hp_to_operate": 0.0,
                "var_flow_temp_ctrl_during_test": true,
                "power_source_circ_pump": 0.010,
                "power_standby": 0.015,
                "power_crankcase_heater": 0.01,
                "power_off": 0.015,
                "power_max_backup": 3.0,
                "EnergySupply": "mains elec",
                "eahp_mixed_max_temp": 60,
                "eahp_mixed_min_temp": 0,
                "test_data_EN14825": [
                {
                    "test_letter": "A",
                    "capacity": 8.4,
                    "cop": 4.6,
                    "design_flow_temp": 35,
                    "temp_outlet": 34,
                    "temp_source": 0,
                    "temp_test": -7,
                    "eahp_mixed_ext_air_ratio": 1,
                    "air_flow_rate": 1,
                }
                ],
            }
        });
        // Then it errors that test data must not have eahp_mixed_ext_air_ratio
        // And the heat pump itself must not have eahp_mixed_max_temp and eahp_mixed_min_temp
        // And because the subschema is invalid, none of the other properties are evaluated either
        let result = apply_schema_validation(&project);
        if let Err(SchemaValidationError { errors }) = result {
            assert!(errors.contains("eahp_mixed_ext_air_ratio"));
            assert!(errors.contains("eahp_mixed_max_temp"));
            assert!(errors.contains("eahp_mixed_min_temp"));
            assert!(errors.contains("Unevaluated"));
        } else {
            panic!("Expected validation error, got {:?}", result);
        }
    }

    #[rstest]
    fn test_exhaust_air_source_heat_pump_subschema_valid(mut project: Value) {
        // Given a heat pump with mixed exhaust air as its source
        project["HeatSourceWet"] = json!({
            "heat pump": {
                "type": "HeatPump",
                "is_heat_network": false,
                "source_type": "ExhaustAirMixed",
                "sink_type": "Water",
                "backup_ctrl_type": "TopUp",
                "modulating_control": true,
                "min_modulation_rate_35": 0.35,
                "min_modulation_rate_55": 0.4,
                "time_constant_onoff_operation": 140,
                "temp_return_feed_max": 70.0,
                "temp_lower_operating_limit": -5.0,
                "min_temp_diff_flow_return_for_hp_to_operate": 0.0,
                "var_flow_temp_ctrl_during_test": true,
                "power_source_circ_pump": 0.010,
                "power_standby": 0.015,
                "power_crankcase_heater": 0.01,
                "power_off": 0.015,
                "power_max_backup": 3.0,
                "eahp_mixed_max_temp": 60,
                "eahp_mixed_min_temp": 50,
                "EnergySupply": "mains elec",
                "test_data_EN14825": [
                {
                    "air_flow_rate": 1,
                    "test_letter": "A",
                    "capacity": 8.4,
                    "cop": 4.6,
                    "design_flow_temp": 35,
                    "temp_outlet": 34,
                    "temp_source": 0,
                    "temp_test": -7,
                    "eahp_mixed_ext_air_ratio": 1,
                }
                ],
            }
        });
        assert!(apply_schema_validation(&project).is_ok());
    }

    #[rstest]
    fn test_exhaust_air_source_heat_pump_subschema_invalid(mut project: Value) {
        // Given a heat pump with mixed exhaust air as its source
        project["HeatSourceWet"] = json!({
            "heat pump": {
                "type": "HeatPump",
                "is_heat_network": false,
                "source_type": "ExhaustAirMixed",
                "sink_type": "Water",
                "backup_ctrl_type": "TopUp",
                "modulating_control": true,
                "min_modulation_rate_35": 0.35,
                "min_modulation_rate_55": 0.4,
                "time_constant_onoff_operation": 140,
                "temp_return_feed_max": 70.0,
                "temp_lower_operating_limit": -5.0,
                "min_temp_diff_flow_return_for_hp_to_operate": 0.0,
                "var_flow_temp_ctrl_during_test": true,
                "power_source_circ_pump": 0.010,
                "power_standby": 0.015,
                "power_crankcase_heater": 0.01,
                "power_off": 0.015,
                "power_max_backup": 3.0,
                "EnergySupply": "mains elec",
                "test_data_EN14825": [
                {
                    "test_letter": "A",
                    "capacity": 8.4,
                    "cop": 4.6,
                    "design_flow_temp": 35,
                    "temp_outlet": 34,
                    "temp_source": 0,
                    "temp_test": -7,
                }
                ],
            }
        });
        // Then it errors that test data must have eahp_mixed_ext_air_ratio and air_flow_rate
        // And the heat pump itself must have eahp_mixed_max_temp and eahp_mixed_min_temp
        // Because the subschema is invalid then none of the properties are evaluated
        assert!(apply_schema_validation(&project).is_err());
    }

    #[rstest]
    fn test_if_not_anyof_condition_for_flat_opaque_elements(mut project: Value) {
        // If a BuildingElementOpaque is not either pitch 0 or pitch 180 then an orientation360
        // is required. (Else the wrapper will create an orientation360 = 180)
        project["Zone"]["zone 1"]["BuildingElement"]["floor 0"]["pitch"] = json!(180);
        let result = apply_schema_validation(&project);
        if let Err(SchemaValidationError { errors }) = result {
            assert!(errors.contains("orientation360"));
            assert!(errors.contains("unexpected"));
        } else {
            panic!("Expected validation error, got {:?}", result);
        }
    }

    #[rstest]
    fn test_if_not_anyof_condition_for_flat_transparent_elements(mut project: Value) {
        // Given a BuildingElementTransparent with pitch 180, indicating a skylight
        project["Zone"]["zone 1"]["BuildingElement"]["window 0"]["pitch"] = json!(180);
        let result = apply_schema_validation(&project);
        if let Err(SchemaValidationError { errors }) = result {
            assert!(errors.contains("orientation360"));
            assert!(errors.contains("unexpected"));
        } else {
            panic!("Expected validation error, got {:?}", result);
        }
    }

    #[rstest]
    fn test_edge_insulation_subschema_valid(mut project: Value) {
        // Given a BuildingElementGround containing horizontal edge_insulation with width property
        project["Zone"]["zone 1"]["BuildingElement"]["ground"] = json!({
            "type": "BuildingElementGround",
            "total_area": 15.0,
            "area": 15.0,
            "u_value": 1.4,
            "thermal_resistance_floor_construction": 0.2,
            "areal_heat_capacity": "Very light",
            "mass_distribution_class": "D: Mass equally distributed",
            "floor_type": "Slab_edge_insulation",
            "thickness_walls": 0.2,
            "perimeter": 16.0,
            "psi_wall_floor_junc": 0.5,
            "edge_insulation": [
            {"edge_thermal_resistance": 10, "type": "horizontal", "width": 0.1}
            ],
        });
        assert!(apply_schema_validation(&project).is_ok());
    }

    #[rstest]
    fn test_edge_insulation_subschema_invalid(mut project: Value) {
        // Given a BuildingElementGround containing horizontal edge_insulation with depth property
        project["Zone"]["zone 1"]["BuildingElement"]["ground"] = json!({
            "type": "BuildingElementGround",
            "total_area": 15.0,
            "area": 15.0,
            "u_value": 1.4,
            "thermal_resistance_floor_construction": 0.2,
            "areal_heat_capacity": "Very light",
            "mass_distribution_class": "D: Mass equally distributed",
            "floor_type": "Slab_edge_insulation",
            "thickness_walls": 0.2,
            "perimeter": 16.0,
            "psi_wall_floor_junc": 0.5,
            "edge_insulation": [
            {"edge_thermal_resistance": 10, "type": "horizontal", "depth": 0.1}
            ],
        });
        assert!(apply_schema_validation(&project).is_err());
    }

    #[rstest]
    fn test_energy_supply_with_priority(mut project: Value) {
        // Given an energy supply with a battery and diverter with a full set of properties,
        // including a priority array
        project["EnergySupply"] = json!({
            "mains elec": {
                "fuel": "electricity",
                "is_export_capable": false,
                "priority": ["ElectricBattery", "diverter"],
                "ElectricBattery": {
                    "capacity": 2,
                    "charge_discharge_efficiency_round_trip": 0.8,
                    "minimum_charge_rate_one_way_trip": 0.001,
                    "maximum_charge_rate_one_way_trip": 1.5,
                    "maximum_discharge_rate_one_way_trip": 1.25,
                    "battery_location": "inside",
                },
                "diverter": {"HeatSource": "immersion"},
            }
        });
        assert!(apply_schema_validation(&project).is_ok());
    }

    #[rstest]
    fn test_heat_pump_without_modulating_control(mut project: Value) {
        // Given a heat pump without modulating_control
        project["HeatSourceWet"] = json!({
            "heat pump": {
                "type": "HeatPump",
                "is_heat_network": false,
                "EnergySupply": "mains elec",
                "source_type": "OutsideAir",
                "sink_type": "Air",
                "backup_ctrl_type": "None",
                "modulating_control": false,
                "temp_return_feed_max": 40,
                "temp_lower_operating_limit": -10,
                "min_temp_diff_flow_return_for_hp_to_operate": 5,
                "var_flow_temp_ctrl_during_test": false,
                "power_source_circ_pump": 0.1,
                "power_standby": 0.01,
                "power_crankcase_heater": 0.01,
                "power_off": 0.0,
                "test_data_EN14825": [],
                "time_constant_onoff_operation": 1,
            }
        });
        assert!(apply_schema_validation(&project).is_ok());
    }

    #[rstest]
    fn test_heat_pump_hw_only_heat_exchanger(mut project: Value) {
        // Given a StorageTank fed by a HP for exclusive HW use
        project["HotWaterSource"]["hw cylinder"]["HeatSource"] = json!({
            "hwo_hp": {
                "EnergySupply": "mains elec",
                "daily_losses_declared": 1.05,
                "heat_exchanger_surface_area_declared": 1.5,
                "heater_position": 0.1,
                "in_use_factor_mismatch": 0.6,
                "power_max": 5.0,
                "tank_volume_declared": 100.0,
                "test_data": {
                    "M": {
                        "cop_dhw": 2.5,
                        "energy_input_measured": 2.338,
                        "hw_tapping_prof_daily_total": 5.845,
                        "hw_vessel_loss_daily": 2.0,
                        "power_standby": 0.02,
                    }
                },
                "thermostat_position": 0.33,
                "type": "HeatPump_HWOnly",
            }
        });
        // Then it should raise an error indicating that a heat_exchanger_surface_area is required
        let result = apply_schema_validation(&project);
        if let Err(SchemaValidationError { errors }) = result {
            assert!(errors.contains("heat_exchanger_surface_area"));
            assert!(errors.contains("required"));
        } else {
            panic!("Expected validation error, got {:?}", result);
        }
    }

    #[rstest]
    fn test_heat_source_of_storage_tank_must_have_thermostat_position(mut project: Value) {
        // Given a StorageTank whose HeatSource does not have a thermostat_position
        project["HotWaterSource"]["hw cylinder"]["HeatSource"]["immersion"]
            .as_object_mut()
            .unwrap()
            .remove("thermostat_position");
        let result = apply_schema_validation(&project);
        if let Err(SchemaValidationError { errors }) = result {
            assert!(errors.contains("thermostat_position"));
            assert!(errors.contains("required"));
        } else {
            panic!("Expected validation error, got {:?}", result);
        }
    }

    #[rstest]
    fn test_preheated_tanks_may_not_themselves_be_preheated(mut project: Value) {
        // Given a PreHeatedWaterSource, with a ColdWaterSource of itself
        project["PreHeatedWaterSource"] = json!({
            "preheated tank": {
                "volume": 80.0,
                "daily_losses": 1.68,
                "ColdWaterSource": "preheated tank",
                "HeatSource": {
                    "immersion": {
                        "type": "ImmersionHeater",
                        "power": 3.0,
                        "EnergySupply": "mains elec",
                        "heater_position": 0.1,
                        "thermostat_position": 0.33,
                    }
                },
            }
        });
        assert!(apply_schema_validation(&project).is_err());
    }

    #[rstest]
    fn test_storage_tanks_may_use_preheated_source(mut project: Value) {
        // Given a PreHeatedWaterSource, with a ColdWaterSource of itself
        project["PreHeatedWaterSource"] = json!({
            "preheated tank": {
                "volume": 80.0,
                "daily_losses": 1.68,
                "ColdWaterSource": "mains water",
                "HeatSource": {
                    "immersion": {
                        "type": "ImmersionHeater",
                        "power": 3.0,
                        "EnergySupply": "mains elec",
                        "heater_position": 0.1,
                        "thermostat_position": 0.33,
                    }
                },
            }
        });
        assert!(apply_schema_validation(&project).is_ok());
    }

    #[rstest]
    fn test_storage_tanks_may_use_wwhrs_source(mut project: Value) {
        // Given a WWHRS used by a StorageTank as its ColdWaterSource
        project["WWHRS"] = json!({
            "wwhrs system": {
                "type": "WWHRS_Instantaneous",
                "ColdWaterSource": "mains water",
                "flow_rates": [1],
                "system_a_efficiencies": [100],
                "system_a_utilisation_factor": 0.972,
            }
        });
        assert!(apply_schema_validation(&project).is_ok());
    }

    #[rstest]
    fn test_preheated_tanks_may_use_wwhrs_source(mut project: Value) {
        // Given a WWHRS used by a PreHeatedWaterSource as its ColdWaterSource
        project["WWHRS"] = json!({
            "wwhrs system": {
                "type": "WWHRS_Instantaneous",
                "ColdWaterSource": "mains water",
                "flow_rates": [1],
                "system_a_efficiencies": [100],
                "system_a_utilisation_factor": 0.972,
            }
        });
        project["PreHeatedWaterSource"] = json!({
            "preheated tank": {
                "volume": 80.0,
                "daily_losses": 1.68,
                "ColdWaterSource": "wwhrs system",
                "HeatSource": {
                    "immersion": {
                        "type": "ImmersionHeater",
                        "power": 3.0,
                        "EnergySupply": "mains elec",
                        "heater_position": 0.1,
                        "thermostat_position": 0.33,
                    }
                },
            }
        });
        assert!(apply_schema_validation(&project).is_ok());
    }
    #[rstest]
    fn test_showers_may_not_use_preheated_source(mut project: Value) {
        // Given a preheated tank used by a mixer shower as its source
        project["PreHeatedWaterSource"] = json!({
            "preheated tank": {
                "volume": 80.0,
                "daily_losses": 1.68,
                "ColdWaterSource": "mains water",
                "HeatSource": {
                    "immersion": {
                        "type": "ImmersionHeater",
                        "power": 3.0,
                        "EnergySupply": "mains elec",
                        "heater_position": 0.1,
                        "thermostat_position": 0.33,
                    }
                },
            }
        });
        project["HotWaterDemand"]["Shower"]["mixer"]["ColdWaterSource"] = json!("preheated tank");
        let result = apply_schema_validation(&project);
        if let Err(SchemaValidationError { errors }) = result {
            assert!(errors.contains("preheated tank"));
            assert!(errors.contains("header tank"));
        } else {
            panic!("Expected validation error, got {:?}", result);
        }
    }

    #[rstest]
    fn test_showers_may_not_use_wwhrs(mut project: Value) {
        // Given a WWHRS system, used by a IES for its ColdWaterSource
        project["WWHRS"] = json!({
            "wwhrs system": {
                "type": "WWHRS_Instantaneous",
                "ColdWaterSource": "mains water",
                "flow_rates": [1],
                "system_a_efficiencies": [100],
                "system_a_utilisation_factor": 0.972,
            }
        });
        project["HotWaterDemand"]["Shower"]["mixer"]["ColdWaterSource"] = json!("wwhrs system");
        let result = apply_schema_validation(&project);
        if let Err(SchemaValidationError { errors }) = result {
            assert!(errors.contains("wwhrs system"));
            assert!(errors.contains("header tank"));
        } else {
            panic!("Expected validation error, got {:?}", result);
        }
    }
}
