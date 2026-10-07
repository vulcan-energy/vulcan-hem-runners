pub(crate) mod part_f {
    use anyhow::{anyhow, bail};
    use home_energy_model::{
        compare_floats::max_of_2,
        core::units::{LITRES_PER_CUBIC_METRE, SECONDS_PER_HOUR},
    };

    use serde_json::Value as JsonValue;

    fn litres_per_second_to_cubic_metres_per_hour(flow_rate_in_litres_per_second: f64) -> f64 {
        flow_rate_in_litres_per_second * SECONDS_PER_HOUR as f64 / LITRES_PER_CUBIC_METRE as f64
    }

    pub fn minimum_whole_dwelling_ventilation_rate_continuous(
        total_floor_area: f64,
        bedrooms: usize,
    ) -> f64 {
        let ventilation_per_m2_floor_area = 0.3; // l/s.m2
        let ventilation_rate_floor_area = total_floor_area * ventilation_per_m2_floor_area;

        // See Table 1.3 Part F building regulations
        let bedroom_ventilation_rate = 13 + bedrooms * 6; // l/s
        litres_per_second_to_cubic_metres_per_hour(max_of_2(
            ventilation_rate_floor_area,
            bedroom_ventilation_rate as f64,
        )) // m3/hr
    }

    pub fn minimum_kitchen_vent_flow_rate(is_kitchen_vent_external: bool) -> f64 {
        // the property needs one large fan for cooking events
        // 30+ l/s fan if it extracts to outside the dwelling, 60+ l/s fan otherwise
        let min_kitchen_vent_flow_rate_external = 30.0;
        let min_kitchen_vent_flow_rate_not_external = 60.0;

        if is_kitchen_vent_external {
            min_kitchen_vent_flow_rate_external
        } else {
            min_kitchen_vent_flow_rate_not_external
        } // l/s
    }

    pub fn minimum_whole_dwelling_ventilation_rate_intermittent(
        bathrooms: usize,
        utility_rooms: usize,
        sanitary_accommodations: usize,
        is_kitchen_vent_external: bool,
    ) -> f64 {
        let minimum_rate_per_kitchen = minimum_kitchen_vent_flow_rate(is_kitchen_vent_external); // l/s
        let minimum_rate_per_bathroom = 15f64; // l/s
        let minimum_rate_per_utility_room = 30f64; // l/s
        let minimum_rate_per_sanitary_accommodation = 6f64; // l/s
        let minimum_rate = bathrooms as f64 * minimum_rate_per_bathroom
            + utility_rooms as f64 * minimum_rate_per_utility_room
            + sanitary_accommodations as f64 * minimum_rate_per_sanitary_accommodation
            + minimum_rate_per_kitchen; // Assume all dwellings have one kitchen
        litres_per_second_to_cubic_metres_per_hour(minimum_rate) // m3/hr
    }

    pub fn minimum_background_ventilation_area_continuous(habitable_rooms: usize) -> f64 {
        let minimum_equivalent_area_per_habitable_room = 40f64; // cm2
        habitable_rooms as f64 * minimum_equivalent_area_per_habitable_room
    }

    pub fn minimum_background_ventilation_area_intermittent(
        habitable_rooms: usize,
        bathrooms: usize,
        storeys: usize,
    ) -> f64 {
        let minimum_area_per_bathroom = 40f64; // cm2
                                               // Different requirements for single-storey dwellings

        let (minimum_area_per_habitable_room, minimum_area_per_kitchen) = match storeys {
            1 => (100, 100),
            _ => (80, 80),
        };

        habitable_rooms as f64 * minimum_area_per_habitable_room as f64
            + bathrooms as f64 * minimum_area_per_bathroom
            + minimum_area_per_kitchen as f64 // Assume all dwellings have one kitchen
    }

    pub fn minimum_background_vent_count_continuous(bedrooms: usize) -> usize {
        bedrooms + 2 //As per part F section 1.64
    }

    fn total_design_flow_from_vents(vents: &[&JsonValue]) -> anyhow::Result<f64> {
        vents
            .iter()
            .map(|v| {
                anyhow::Ok(v.get("design_outdoor_air_flow_rate").and_then(|v| v.as_f64()).ok_or_else(|| anyhow!("design_outdoor_air_flow_rate provided as a number is expected for MechanicalVentilation"))?)
            })
            .sum()
    }

    fn sufficient_whole_dwelling_ventilation_rate_continuous(
        vents: Vec<&JsonValue>,
        total_floor_area: f64,
        bedrooms: usize,
    ) -> anyhow::Result<bool> {
        let total_design_flow: f64 = vents
            .iter()
            .map(|v| {
                anyhow::Ok(v.get("design_outdoor_air_flow_rate").and_then(|v| v.as_f64()).ok_or_else(|| anyhow!("design_outdoor_air_flow_rate provided as a number is expected for MechanicalVentilation"))?)
            })
            .sum::<Result<f64, _>>()?;

        let min_ventilation =
            minimum_whole_dwelling_ventilation_rate_continuous(total_floor_area, bedrooms);
        Ok(total_design_flow >= min_ventilation)
    }

    fn sufficient_whole_dwelling_ventilation_rate_intermittent(
        vents: &[&JsonValue],
        bathrooms: usize,
        utility_rooms: usize,
        sanitary_accommodations: usize,
        is_kitchen_vent_external: bool,
    ) -> anyhow::Result<bool> {
        let total_design_flow = total_design_flow_from_vents(vents)?;
        let min_ventilation = minimum_whole_dwelling_ventilation_rate_intermittent(
            bathrooms,
            utility_rooms,
            sanitary_accommodations,
            is_kitchen_vent_external,
        );
        Ok(total_design_flow >= min_ventilation)
    }

    fn sufficient_background_ventilation_area_continuous(
        vents: &Vec<&JsonValue>,
        habitable_rooms: usize,
    ) -> anyhow::Result<bool> {
        let total_vent_area = total_vent_area_from_vents(vents)?;
        let min_area = minimum_background_ventilation_area_continuous(habitable_rooms);
        Ok(total_vent_area >= min_area)
    }

    fn sufficient_background_vent_count_continuous(vents: &[&JsonValue], bedrooms: usize) -> bool {
        vents.len() >= minimum_background_vent_count_continuous(bedrooms)
    }

    fn sufficient_background_ventilation_area_intermittent(
        vents: &Vec<&JsonValue>,
        habitable_rooms: usize,
        bathrooms: usize,
        storeys: usize,
    ) -> anyhow::Result<bool> {
        let total_vent_area = total_vent_area_from_vents(vents)?;
        let min_area =
            minimum_background_ventilation_area_intermittent(habitable_rooms, bathrooms, storeys);
        Ok(total_vent_area >= min_area)
    }

    fn sufficient_mev_count(vents: &[&JsonValue], wet_rooms: usize) -> bool {
        vents.len() >= wet_rooms
    }

    fn sufficient_large_imev(
        vents: &[&JsonValue],
        is_kitchen_vent_external: bool,
    ) -> anyhow::Result<bool> {
        let min_flow_rate = minimum_kitchen_vent_flow_rate(is_kitchen_vent_external); // l / s
        let min_flow_rate_m3hr = litres_per_second_to_cubic_metres_per_hour(min_flow_rate); // m3/hr

        let vent_flow_rates: Vec<f64> = vents.iter().map(|v| anyhow::Ok(v.get("design_outdoor_air_flow_rate").and_then(|v| v.as_f64()).ok_or_else(|| anyhow!("design_outdoor_air_flow_rate provided as a number is expected for MechanicalVentilation"))?)).collect::<Result<_, _>>()?;

        Ok(vent_flow_rates
            .into_iter()
            .any(|rate| rate >= min_flow_rate_m3hr))
    }

    fn sufficient_background_vent_count_intermittent(
        background_vents: &[&JsonValue],
        bedrooms: usize,
    ) -> bool {
        // As per part F section 1.57
        let background_vents_required = if bedrooms < 2 { 4 } else { 5 };

        background_vents.len() >= background_vents_required
    }

    fn total_vent_area_from_vents(vents: &[&JsonValue]) -> anyhow::Result<f64> {
        vents
            .iter()
            .map(|v| {
                anyhow::Ok(v.get("area_cm2").and_then(|v| v.as_f64()).ok_or_else(|| {
                    anyhow!("area_cm2 provided as a number is expected for MechanicalVentilation")
                })?)
            })
            .sum()
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_intermittent_vents(
        intermittent_mev_vents: Vec<&JsonValue>,
        background_vents: &Vec<&JsonValue>,
        habitable_rooms: usize,
        wet_rooms: usize,
        bathrooms: usize,
        utility_rooms: usize,
        sanitary_accommodations: usize,
        storeys: usize,
        is_kitchen_vent_external: bool,
        bedrooms: usize,
    ) -> anyhow::Result<Vec<String>> {
        let background_compliant = sufficient_background_ventilation_area_intermittent(
            background_vents,
            habitable_rooms,
            bathrooms,
            storeys,
        )?;
        let mech_compliant = sufficient_whole_dwelling_ventilation_rate_intermittent(
            &intermittent_mev_vents,
            bathrooms,
            utility_rooms,
            sanitary_accommodations,
            is_kitchen_vent_external,
        )?;
        let mev_count_compliant = sufficient_mev_count(&intermittent_mev_vents, wet_rooms);
        let large_compliant =
            sufficient_large_imev(&intermittent_mev_vents, is_kitchen_vent_external)?;
        let background_count_compliant =
            sufficient_background_vent_count_intermittent(background_vents, bedrooms);

        let checks = [
            (
                background_compliant,
                "Dwelling lacks sufficient background vent area for intermittent ventilation.",
            ),
            (mech_compliant, "Dwelling lacks sufficient intermittent mechanical extract rate."),
            (mev_count_compliant, "Dwelling lacks sufficient number of intermittent mechanical vents."),
            (
                large_compliant,
                "Dwelling lacks a large enough intermittent mechanical vent for cooking events.",
            ),
            (
                background_count_compliant,
                "Dwelling lacks sufficient number of background vents for intermittent ventilation.",
            ),
        ];

        Ok(checks
            .iter()
            .filter(|(passed, _)| !passed)
            .map(|(_, message)| message.to_string())
            .collect())
    }

    /// Validates continuous ventilation systems.
    ///
    ///    A dwelling may satisfy continuous ventilation requirements via either:
    ///        - Continuous MEV (centralised or decentralised), or
    ///        - MVHR.
    ///
    ///    If multiple system types are present, at least one validation pathway
    ///    must pass. However, whole-dwelling mechanical extract rate compliance
    ///    is always required and evaluated independently of pathway success.
    ///
    ///    Returns:
    ///        A list of validation error messages.
    #[allow(clippy::too_many_arguments)]
    fn validate_continuous_vents(
        mvhr_vents: &[&JsonValue],
        centralised_mev_vents: &Vec<&JsonValue>,
        decentralised_mev_vents: &Vec<&JsonValue>,
        background_vents: &Vec<&JsonValue>,
        total_floor_area: f64,
        bedrooms: usize,
        habitable_rooms: usize,
        wet_rooms: usize,
    ) -> anyhow::Result<Vec<String>> {
        let mut all_errors: Vec<String> = Default::default();

        let mut continuous_mev_vents = centralised_mev_vents.clone();
        continuous_mev_vents.append(&mut decentralised_mev_vents.clone());

        // Pathway specific checks for continuous MEV and MVHR systems
        let mut continuous_mev_errors: Vec<String> = Default::default();

        if !continuous_mev_vents.is_empty() {
            continuous_mev_errors.append(&mut validate_continuous_mev_vents(
                centralised_mev_vents,
                decentralised_mev_vents,
                background_vents,
                bedrooms,
                habitable_rooms,
                wet_rooms,
            )?);
        }

        let mut mvhr_errors: Vec<String> = Default::default();

        if !mvhr_vents.is_empty() && !background_vents.is_empty() {
            mvhr_errors.push(
                "Dwelling with MVHR ventilation system should not have background vents.".into(),
            );
        }

        // Only one of the validation pathways (continuous MEV or MVHR) needs to pass
        let mut validation_pathway_errors: Vec<Vec<String>> = Default::default();
        if !continuous_mev_vents.is_empty() {
            validation_pathway_errors.push(continuous_mev_errors);
        }
        if !mvhr_vents.is_empty() {
            validation_pathway_errors.push(mvhr_errors);
        }
        if validation_pathway_errors.iter().all(|v| !v.is_empty()) {
            all_errors.append(&mut validation_pathway_errors.into_iter().flatten().collect());
        }

        // Whole dwelling mechanical extract rate validation always applies
        let whole_building_vents = {
            let mut whole_building_vents = continuous_mev_vents.clone();
            whole_building_vents.append(&mut mvhr_vents.to_vec());
            whole_building_vents
        };
        let mech_compliant = sufficient_whole_dwelling_ventilation_rate_continuous(
            whole_building_vents,
            total_floor_area,
            bedrooms,
        )?;
        if !mech_compliant {
            all_errors.push("Dwelling lacks sufficient continuous mechanical extract rate.".into());
        }

        Ok(all_errors)
    }

    fn validate_continuous_mev_vents(
        centralised_mev_vents: &[&JsonValue],
        decentralised_mev_vents: &Vec<&JsonValue>,
        background_vents: &Vec<&JsonValue>,
        bedrooms: usize,
        habitable_rooms: usize,
        wet_rooms: usize,
    ) -> anyhow::Result<Vec<String>> {
        let background_area_compliant =
            sufficient_background_ventilation_area_continuous(background_vents, habitable_rooms)?;

        let background_count_compliant =
            sufficient_background_vent_count_continuous(background_vents, bedrooms);

        // the number of decentralised continuous vents only needs to be validated for
        // decentralised systems (when no centralised vents exist)
        let decentralised_vent_count_compliant =
            if !decentralised_mev_vents.is_empty() && centralised_mev_vents.is_empty() {
                sufficient_mev_count(decentralised_mev_vents, wet_rooms)
            } else {
                true
            };

        let checks = [
            (
                background_area_compliant,
                "Dwelling lacks sufficient background vent area for continuous ventilation."
            ),
            (
                background_count_compliant,
                "Dwelling lacks sufficient number of background vents for continuous ventilation."
            ),
            (
                decentralised_vent_count_compliant,
                "Dwelling lacks sufficient number of decentralised mechanical vents for continuous ventilation."
            )
        ];

        Ok(checks
            .iter()
            .filter(|(passed, _)| !passed)
            .map(|(_, message)| message.to_string())
            .collect())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn validate_dwelling_ventilation(
        ventilation: &JsonValue,
        total_floor_area: f64,
        bedrooms: usize,
        habitable_rooms: usize,
        wet_rooms: usize,
        bathrooms: usize,
        utility_rooms: usize,
        sanitary_accommodations: usize,
        storeys: usize,
        is_kitchen_vent_external: bool,
    ) -> anyhow::Result<()> {
        let mech_vents = ventilation
            .get("MechanicalVentilation")
            .and_then(|v| v.as_object());
        let mech_vents = match mech_vents {
            Some(mech_vents) if !mech_vents.is_empty() => mech_vents,
            _ => bail!("FHS input validation failed, see part F of the building regulations.\nDwelling lacks any mechanical vents."),
        };

        let background_vents: Vec<&JsonValue> = ventilation
            .get("Vents")
            .and_then(|v| v.as_object())
            .ok_or_else(|| anyhow!("Vents object is required for InfiltrationVentilation"))?
            .values()
            .collect();

        let intermittent_mev_vents: Vec<&JsonValue> = mech_vents
            .values()
            .filter(|mech_vent| {
                mech_vent.get("vent_type").and_then(|v| v.as_str()) == Some("Intermittent MEV")
            })
            .collect();
        let mvhr_vents: Vec<&JsonValue> = mech_vents
            .values()
            .filter(|mech_vent| mech_vent.get("vent_type").and_then(|v| v.as_str()) == Some("MVHR"))
            .collect();
        let centralised_mev_vents: Vec<&JsonValue> = mech_vents
            .values()
            .filter(|mech_vent| {
                mech_vent.get("vent_type").and_then(|v| v.as_str())
                    == Some("Centralised continuous MEV")
            })
            .collect();
        let decentralised_mev_vents: Vec<&JsonValue> = mech_vents
            .values()
            .filter(|mech_vent| {
                mech_vent.get("vent_type").and_then(|v| v.as_str())
                    == Some("Decentralised continuous MEV")
            })
            .collect();

        let has_intermittent_vents = !intermittent_mev_vents.is_empty();
        let has_continuous_vents = !mvhr_vents.is_empty()
            || !centralised_mev_vents.is_empty()
            || !decentralised_mev_vents.is_empty();

        let mut intermittent_errors: Vec<String> = Default::default();
        if has_intermittent_vents {
            intermittent_errors = validate_intermittent_vents(
                intermittent_mev_vents,
                &background_vents,
                habitable_rooms,
                wet_rooms,
                bathrooms,
                utility_rooms,
                sanitary_accommodations,
                storeys,
                is_kitchen_vent_external,
                bedrooms,
            )?;
        }

        let mut continuous_errors: Vec<String> = Default::default();
        if has_continuous_vents {
            continuous_errors = validate_continuous_vents(
                &mvhr_vents,
                &centralised_mev_vents,
                &decentralised_mev_vents,
                &background_vents,
                total_floor_area,
                bedrooms,
                habitable_rooms,
                wet_rooms,
            )?;
        }

        // note - important to clone() here so that `append` doesn't modify intermittent_errors or continuous_errors
        let mut all_collected_errors = intermittent_errors.clone();
        all_collected_errors.append(&mut continuous_errors.clone());

        if all_collected_errors.is_empty() {
            return Ok(());
        }

        if has_intermittent_vents && has_continuous_vents {
            // Dwellings only have to pass either intermittent or continuous validation
            if intermittent_errors.is_empty() || continuous_errors.is_empty() {
                return Ok(());
            }
        }

        bail!(
            "FHS input validation failed, see part F of the building regulations.\nFailure(s):\n{}",
            all_collected_errors.join("\n")
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::future_homes_standard::fhs_part_f_validation::part_f;
    use serde_json::json;

    #[test]
    fn test_mwdvr_when_bedroom_based_value_is_greater() {
        // TFA criteria = 100 * 0.3 = 30 l/s
        // Bedroom criteria = 13 + 4 * 6 = 37 l/s

        let mwdvr = part_f::minimum_whole_dwelling_ventilation_rate_continuous(100.0, 4);
        assert_eq!(mwdvr, 133.2) // 37 * 3.6 m3/hr
    }

    #[test]
    fn test_mwdvr_when_floor_area_based_value_is_greater() {
        // TFA criteria = 150 * 0.3 = 45 l/s
        // Bedroom criteria = 13 + 4 * 6 = 37 l/s
        let mwdvr = part_f::minimum_whole_dwelling_ventilation_rate_continuous(150.0, 4);
        assert_eq!(mwdvr, 162.0) // 45 * 3.6 m3/hr
    }

    #[test]
    fn test_one_habitable_room() {
        let mbva = part_f::minimum_background_ventilation_area_continuous(1);
        assert_eq!(mbva, 40.0); // 1 * 40 = 40 cm2
    }

    #[test]
    fn test_five_habitable_rooms() {
        let mbva = part_f::minimum_background_ventilation_area_continuous(5);
        assert_eq!(mbva, 200.0); // 5 * 40 = 200 cm2
    }

    #[test]
    fn test_no_bedrooms() {
        let bedrooms = 0;
        let minimum_vent_count = part_f::minimum_background_vent_count_continuous(bedrooms);
        assert_eq!(2, minimum_vent_count);
    }

    #[test]
    fn test_five_bedrooms() {
        let bedrooms = 5;
        let minimum_vent_count = part_f::minimum_background_vent_count_continuous(bedrooms);
        assert_eq!(7, minimum_vent_count);
    }

    // test_does_not_raise_if_sufficient_continuous_MEV_and_background_vents
    #[test]
    fn test_does_not_raise_if_sufficient_cmev_and_background_vents() {
        let json = r#"{ 
            "Vents": {
                "vent1": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 100,
                    "pressure_difference_ref": 20,
                    "orientation360": 180,
                    "pitch": 60
                },
                "vent2": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 100,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60
                },
                "vent3": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 100,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60
                }
            },
            "MechanicalVentilation": {
                "mechvent1": {
                    "vent_type": "Centralised continuous MEV",
                    "measured_fan_power": 12.26,
                    "measured_air_flow_rate": 37,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 133.2
                }
            }
        }"#;
        let ventilation = serde_json::from_str(json).unwrap();

        let result =
            part_f::validate_dwelling_ventilation(&ventilation, 100., 1, 5, 3, 2, 0, 0, 2, true);
        assert!(result.is_ok());
    }

    // test_raises_if_sufficient_continuous_MEV_but_insufficient_background_vents
    #[test]
    fn test_raises_if_sufficient_cmev_but_insufficient_background_vents() {
        let json = r#"{
            "Vents": {},
            "MechanicalVentilation": {
                "mechvent1": {
                    "vent_type": "Centralised continuous MEV",
                    "measured_fan_power": 12.26,
                    "measured_air_flow_rate": 37,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 133.2
                }
            }
        }"#;

        let ventilation = serde_json::from_str(json).unwrap();
        let result =
            part_f::validate_dwelling_ventilation(&ventilation, 100., 4, 5, 3, 2, 0, 0, 2, true);

        assert!(result.is_err());
        let _ = result.inspect_err(|e| {
            assert!(e.to_string().contains(
                "Dwelling lacks sufficient background vent area for continuous ventilation."
            ));
            assert!(e.to_string().contains(
                "Dwelling lacks sufficient number of background vents for continuous ventilation."
            ));
        });
    }

    #[test]
    fn test_raises_if_no_mechanical_vents() {
        let json = r#"{
            "Vents": {
                "vent1": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 100,
                    "pressure_difference_ref": 20,
                    "orientation360": 180,
                    "pitch": 60
                },
                "vent2": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 100,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60
                }
            }
        }"#;

        let ventilation = serde_json::from_str(json).unwrap();
        let result =
            part_f::validate_dwelling_ventilation(&ventilation, 100., 4, 5, 3, 2, 0, 0, 2, true);

        assert!(result.is_err());
        let _ = result.inspect_err(|e| {
            assert!(e
                .to_string()
                .contains("Dwelling lacks any mechanical vents"));
        });
    }

    // test_raises_if_neither_background_nor_continuous_MEV_ventilation_sufficient
    #[test]
    fn test_raises_if_neither_background_nor_cmev_ventilation_sufficient() {
        let json = r#"{
            "Vents": {},
            "MechanicalVentilation": {
                "mechvent1": {
                    "vent_type": "Centralised continuous MEV",
                    "measured_fan_power": 12.26,
                    "measured_air_flow_rate": 37,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 50
                }
            }
        }"#;

        // "design_outdoor_air_flow_rate": 50,  # not enough for 100 floor area/4 beds

        let ventilation = serde_json::from_str(json).unwrap();
        let result =
            part_f::validate_dwelling_ventilation(&ventilation, 100., 4, 5, 3, 2, 0, 0, 2, true);

        assert!(result.is_err());
        let _ = result.inspect_err(|e| {
            let error_message = e.to_string();
            assert!(error_message
                .contains("Dwelling lacks sufficient continuous mechanical extract rate."));
            assert!(error_message.contains(
                "Dwelling lacks sufficient background vent area for continuous ventilation."
            ));
            assert!(error_message.contains(
                "Dwelling lacks sufficient number of background vents for continuous ventilation."
            ));
        });
    }

    // test_does_not_raise_if_sufficient_MVHR_and_no_background_vents
    #[test]
    fn test_does_not_raise_if_sufficient_mvhr_and_no_background_vents() {
        let json = r#"{
            "Vents": {},
            "MechanicalVentilation": {
                "mechvent1": {
                    "vent_type": "MVHR",
                    "measured_fan_power": 12.26,
                    "measured_air_flow_rate": 37,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 133.2,
                    "mvhr_eff": 0.0,
                    "mvhr_location": "inside",
                    "position_intake": {
                        "mid_height_air_flow_path": 1.5,
                        "orientation360": 90,
                        "pitch": 60
                    },
                    "position_exhaust": {
                        "mid_height_air_flow_path": 1.5,
                        "orientation360": 90,
                        "pitch": 60
                    },
                    "ductwork": [
                        {
                            "cross_section_shape": "circular",
                            "external_diameter_mm": 160,
                            "insulation_thermal_conductivity": 0.04,
                            "insulation_thickness_mm": 25,
                            "internal_diameter_mm": 150,
                            "length": 5.0,
                            "reflective": false,
                            "duct_type": "supply"
                        }
                    ]
                }
            }
        }"#;

        let ventilation = serde_json::from_str(json).unwrap();
        let result =
            part_f::validate_dwelling_ventilation(&ventilation, 100., 4, 5, 3, 2, 0, 0, 2, true);

        assert!(result.is_ok());
    }

    #[test]
    fn test_raises_if_insufficient_mvhr() {
        let json = r#"{
            "Vents": {},
            "MechanicalVentilation": {
                "mechvent1": {
                    "vent_type": "MVHR",
                    "measured_fan_power": 12.26,
                    "measured_air_flow_rate": 37,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 10,
                    "mvhr_eff": 0.0,
                    "mvhr_location": "inside",
                    "position_intake": {
                        "mid_height_air_flow_path": 1.5,
                        "orientation360": 90,
                        "pitch": 60
                    },
                    "position_exhaust": {
                        "mid_height_air_flow_path": 1.5,
                        "orientation360": 90,
                        "pitch": 60
                    },
                    "ductwork": [
                        {
                            "cross_section_shape": "circular",
                            "external_diameter_mm": 160,
                            "insulation_thermal_conductivity": 0.04,
                            "insulation_thickness_mm": 25,
                            "internal_diameter_mm": 150,
                            "length": 5.0,
                            "reflective": false,
                            "duct_type": "supply"
                        }
                    ]
                }
            }
        }"#;

        let ventilation = serde_json::from_str(json).unwrap();
        let result =
            part_f::validate_dwelling_ventilation(&ventilation, 100., 4, 5, 3, 2, 0, 0, 2, true);

        assert!(result.is_err());
        let _ = result.inspect_err(|e| {
            assert!(e
                .to_string()
                .contains("Dwelling lacks sufficient continuous mechanical extract rate."));
        });
    }

    #[test]
    fn test_does_not_raise_if_sufficient_imev_and_background_vents() {
        let json = r#"{
            "Vents": {
                "vent1": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 180,
                    "pitch": 60
                },
                "vent2": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60
                },
                "vent3": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60
                },
                "vent4": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60
                },
                "vent5": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60
                }
            },
            "MechanicalVentilation": {
                "mechvent1": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 600
                },
                "mechvent2": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 200
                },
                "mechvent3": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 200
                }
            }
        }"#;

        let ventilation = serde_json::from_str(json).unwrap();
        let result =
            part_f::validate_dwelling_ventilation(&ventilation, 100., 4, 5, 3, 2, 0, 0, 2, true);

        assert!(result.is_ok());
    }

    #[test]
    fn test_raises_if_sufficient_imev_but_insufficient_background_vents() {
        let json = r#"{
            "Vents": {},
            "MechanicalVentilation": {
                "mechvent1": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 700
                },
                "mechvent2": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 200
                },
                "mechvent3": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 200
                }
            }
        }"#;

        let ventilation = serde_json::from_str(json).unwrap();
        let result =
            part_f::validate_dwelling_ventilation(&ventilation, 100., 4, 5, 3, 2, 0, 0, 2, true);

        assert!(result.is_err());
        let _ = result.inspect_err(|e| {
            let error_message = e.to_string();
            assert!(error_message.contains("Dwelling lacks sufficient background vent area for intermittent ventilation."));
            assert!(error_message.contains("Dwelling lacks sufficient number of background vents for intermittent ventilation."));
        });
    }

    // test_raises_if_sufficient_background_vents_but_insufficient_iMEV
    #[test]
    #[ignore = "not yet implemented"]
    fn test_raises_if_sufficient_background_vents_but_insufficient_imev() {
        let json = r#"{
            "Vents": {
                "vent1": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 180,
                    "pitch": 60,
                },
                "vent2": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60,
                },
            },
            "MechanicalVentilation": {
                "mechvent1": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 10,  # insufficient flow rate
                },
                "mechvent2": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 10,
                },
                "mechvent3": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 10,
                },
            },
        }"#;

        let ventilation = serde_json::from_str(json).unwrap();
        let result =
            part_f::validate_dwelling_ventilation(&ventilation, 100., 4, 5, 3, 2, 0, 0, 2, true);

        assert!(result.is_err());
        let _ = result.inspect_err(|e| {
            let error_message = e.to_string();
            assert!(error_message
                .contains("Dwelling lacks sufficient intermittent mechanical extract rate."));
            assert!(error_message.contains(
                "Dwelling lacks a large enough intermittent mechanical vent for cooking events."
            ));
        });
    }

    // test_raises_if_iMEV_sufficient_but_lacking_a_large_enough_kitchen_non_external_vent
    #[test]
    #[ignore = "not yet implemented"]
    fn test_raises_if_imev_sufficient_but_lacking_a_large_enough_kitchen_non_external_vent() {
        // Given a dwelling with sufficient iMEV vents and background vents
        // But no individual fan of sufficient size for a kitchen without ventilation to the
        // outside (216+ m3/hr)

        let json = r#"{
            "Vents": {
                "vent1": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 180,
                    "pitch": 60,
                },
                "vent2": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60,
                },
                "vent3": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60,
                },
                "vent4": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60,
                },
                "vent5": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60,
                },
            },
            "MechanicalVentilation": {
                "mechvent1": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 215,
                },
                "mechvent2": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 215,
                },
                "mechvent3": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 215,
                },
                "mechvent4": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 215,
                },
                "mechvent5": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 215,
                },
            },
        }"#;

        let ventilation = serde_json::from_str(json).unwrap();
        let result =
            part_f::validate_dwelling_ventilation(&ventilation, 100., 4, 5, 3, 2, 0, 0, 2, true);

        assert!(result.is_err());
        let _ = result.inspect_err(|e| {
            assert!(e.to_string().contains(
                "Dwelling lacks a large enough intermittent mechanical vent for cooking events."
            ));
        });
    }

    #[test]
    #[ignore = "not yet implemented"]
    fn test_raises_if_lacking_a_large_enough_kitchen_external_vent() {
        // Given a dwelling with sufficient iMEV vents and background vents
        // But no individual fan of sufficient size for a kitchen with ventilation to the
        // outside (108+ m3/hr)

        let json = r#"{
            "Vents": {
                "vent1": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 180,
                    "pitch": 60,
                },
                "vent2": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60,
                },
                "vent3": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60,
                },
                "vent4": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60,
                },
            },
            "MechanicalVentilation": {
                "mechvent1": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 107,
                },
                "mechvent2": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 107,
                },
                "mechvent3": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 107,
                },
            },
        }"#;

        let ventilation = serde_json::from_str(json).unwrap();
        let result =
            part_f::validate_dwelling_ventilation(&ventilation, 100., 4, 5, 3, 2, 0, 0, 2, false);

        assert!(result.is_err());
        let _ = result.inspect_err(|e| {
            assert!(e.to_string().contains(
                "Dwelling lacks a large enough intermittent mechanical vent for cooking events."
            ));
        });
    }

    // test_raises_if_neither_background_nor_iMEV_ventilation_sufficient
    #[test]
    #[ignore = "not yet implemented"]
    fn test_raises_if_neither_background_nor_imev_ventilation_sufficient() {
        // Given a dwelling that neither has enough background vents nor
        // intermittent mechanical vents

        let json = r#"{
            "Vents": {},
            "MechanicalVentilation": {
                "mechvent1": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 10,  # insufficient flow rate
                },
                "mechvent2": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 10,
                },
                "mechvent3": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 10,
                },
            },
        }"#;

        let ventilation = serde_json::from_str(json).unwrap();
        let result =
            part_f::validate_dwelling_ventilation(&ventilation, 100., 4, 5, 3, 2, 0, 0, 2, true);

        assert!(result.is_err());
        let _ = result.inspect_err(|e| {
            let error_message = e.to_string();
            assert!(error_message.contains("Dwelling lacks sufficient background vent area for intermittent ventilation."));
            assert!(error_message.contains("Dwelling lacks sufficient intermittent mechanical extract rate."));
            assert!(error_message.contains("Dwelling lacks a large enough intermittent mechanical vent for cooking events."));
            assert!(error_message.contains("Dwelling lacks sufficient number of background vents for intermittent ventilation."));
        });
    }

    // test_raises_if_insufficient_number_of_iMEV
    #[test]
    #[ignore = "not yet implemented"]
    fn test_raises_if_insufficient_number_of_imev() {
        // Given a dwelling with sufficient background vents but insufficient number of iMEV
        let json = r#"{
            "Vents": {
                "vent1": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 180,
                    "pitch": 60,
                },
                "vent2": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60,
                },
                "vent3": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60,
                },
                "vent4": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60,
                },
                "vent5": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60,
                },
            },
            "MechanicalVentilation": {
                "mechvent1": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 600,
                },
                "mechvent2": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 600,
                },
            },
        }"#;

        let ventilation = serde_json::from_str(json).unwrap();
        let result =
            part_f::validate_dwelling_ventilation(&ventilation, 100., 4, 5, 3, 2, 0, 0, 2, true);

        assert!(result.is_err());
        let _ = result.inspect_err(|e| {
            assert!(e
                .to_string()
                .contains("Dwelling lacks sufficient number of intermittent mechanical vents."));
        });
    }

    // test_does_not_raise_if_sufficient_iMEV_and_continuous_MEV_and_background_vents
    #[test]
    fn test_does_not_raise_if_sufficient_imev_and_continuous_mev_and_background_vents() {
        // Given a dwelling with sufficient iMEV vents and continuous MEV vents and background vents
        let json = r#"{
            "Vents": {
                "vent1": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 180,
                    "pitch": 60
                },
                "vent2": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60
                },
                "vent3": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60
                },
                "vent4": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60
                },
                "vent5": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60
                }
            },
            "MechanicalVentilation": {
                "mechvent1": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 240
                },
                "mechvent2": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 60
                },
                "mechvent3": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 60
                },
                "mechvent4": {
                    "vent_type": "Centralised continuous MEV",
                    "measured_fan_power": 12.26,
                    "measured_air_flow_rate": 37,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 133.2
                }
            }
        }"#;

        let ventilation = serde_json::from_str(json).unwrap();
        let result =
            part_f::validate_dwelling_ventilation(&ventilation, 100., 4, 5, 3, 2, 0, 0, 2, true);

        assert!(result.is_ok());
    }

    // test_does_not_raise_if_sufficient_continuous_MEV_but_insufficient_background_vents_for_iMEV
    #[test]
    fn test_does_not_raise_if_sufficient_continuous_mev_but_insufficient_background_vents_for_imev()
    {
        // Given a dwelling with sufficient continuous MEV ventilation but
        // insufficient background vents for intermittent ventilation.
        // The dwelling passes because only one route needs to pass.
        let json = r#"{
            "Vents": {
                "vent1": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 180,
                    "pitch": 60
                },
                "vent2": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 180,
                    "pitch": 60
                },
                "vent3": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 180,
                    "pitch": 60
                }
            },
            "MechanicalVentilation": {
                "mechvent1": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 240
                },
                "mechvent2": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 60
                },
                "mechvent3": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 60
                },
                "mechvent4": {
                    "vent_type": "Centralised continuous MEV",
                    "measured_fan_power": 12.26,
                    "measured_air_flow_rate": 37,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 133.2
                }
            }
        }"#;

        let ventilation = serde_json::from_str(json).unwrap();
        let result =
            part_f::validate_dwelling_ventilation(&ventilation, 100., 1, 5, 3, 2, 0, 0, 2, true);

        assert!(result.is_ok());
    }

    // test_does_not_raise_if_sufficient_iMEV_and_background_vents_but_insufficient_continuous_MEV
    #[test]
    fn test_does_not_raise_if_sufficient_imev_and_background_vents_but_insufficient_continuous_mev()
    {
        // Given a dwelling with sufficient iMEV vents and background vents but insufficient
        // continuous MEV vents. The dwelling passes because only one route needs to pass.
        let json = r#"{
            "Vents": {
                "vent1": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 180,
                    "pitch": 60
                },
                "vent2": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 180,
                    "pitch": 60
                },
                "vent3": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 180,
                    "pitch": 60
                },
                "vent4": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 180,
                    "pitch": 60
                },
                "vent5": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 300,
                    "pressure_difference_ref": 20,
                    "orientation360": 180,
                    "pitch": 60
                }
            },
            "MechanicalVentilation": {
                "mechvent1": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 600
                },
                "mechvent2": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 200
                },
                "mechvent3": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 200
                },
                "mechvent4": {
                    "vent_type": "Centralised continuous MEV",
                    "measured_fan_power": 12.26,
                    "measured_air_flow_rate": 37,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 10
                }
            }
        }"#;

        let ventilation = serde_json::from_str(json).unwrap();
        let result =
            part_f::validate_dwelling_ventilation(&ventilation, 100., 4, 5, 3, 2, 0, 0, 2, true);

        assert!(result.is_ok());
    }

    // test_raises_if_insufficient_iMEV_and_continuous_MEV_and_background_vents
    #[test]
    fn test_raises_if_insufficient_imev_and_continuous_mev_and_background_vents() {
        // Given a dwelling with insufficient iMEV vents and continuousMEV vents and background vents
        let json = r#"{
            "Vents": {
                "vent1": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 100,
                    "pressure_difference_ref": 20,
                    "orientation360": 180,
                    "pitch": 60
                }
            },
            "MechanicalVentilation": {
                "mechvent1": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 240
                },
                "mechvent2": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 60
                },
                "mechvent4": {
                    "vent_type": "Centralised continuous MEV",
                    "measured_fan_power": 12.26,
                    "measured_air_flow_rate": 37,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 10
                }
            }
        }"#;

        let ventilation = serde_json::from_str(json).unwrap();
        let result =
            part_f::validate_dwelling_ventilation(&ventilation, 100., 4, 5, 3, 2, 0, 0, 2, false);

        assert!(result.is_err());
        let _ = result.inspect_err(|e| {
            let error_message = e.to_string();
            assert!(error_message.contains("Dwelling lacks sufficient background vent area for intermittent ventilation."));
            assert!(error_message.contains("Dwelling lacks sufficient intermittent mechanical extract rate."));
            assert!(error_message.contains("Dwelling lacks sufficient number of intermittent mechanical vents."));
            assert!(error_message.contains("Dwelling lacks sufficient number of background vents for intermittent ventilation."));
            assert!(error_message.contains("Dwelling lacks sufficient continuous mechanical extract rate."));
            assert!(error_message.contains("Dwelling lacks sufficient background vent area for continuous ventilation."));
            assert!(error_message.contains("Dwelling lacks sufficient number of background vents for continuous ventilation."));
        });
    }

    // test_does_not_raise_if_sufficient_decentralised_continuous_MEV_and_background_vents
    #[test]
    fn test_does_not_raise_if_sufficient_decentralised_continuous_mev_and_background_vents() {
        // Given a dwelling with sufficient decentralised continuous MEV vents and background vents
        let json = r#"{
            "Vents": {
                "vent1": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 100,
                    "pressure_difference_ref": 20,
                    "orientation360": 180,
                    "pitch": 60
                },
                "vent2": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 100,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60
                },
                "vent3": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 100,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60
                }
            },
            "MechanicalVentilation": {
                "mechvent1": {
                    "vent_type": "Decentralised continuous MEV",
                    "measured_fan_power": 12.26,
                    "measured_air_flow_rate": 37,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 133.2
                }
            }
        }"#;

        let ventilation = serde_json::from_str(json).unwrap();
        let result =
            part_f::validate_dwelling_ventilation(&ventilation, 100., 1, 5, 1, 2, 0, 0, 2, false);

        assert!(result.is_ok());
    }

    // test_raises_if_sufficient_iMEV_but_insufficient_background_vent_count
    #[test]
    fn test_raises_if_sufficient_imev_but_insufficient_background_vent_count() {
        // Given a dwelling with sufficient iMEV vents and background ventilator area, but
        // insufficient background ventilator count
        let json = r#"{
            "Vents": {
                "vent1": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 1000,
                    "pressure_difference_ref": 20,
                    "orientation360": 180,
                    "pitch": 60
                }
            },
            "MechanicalVentilation": {
                "mechvent1": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 700
                },
                "mechvent2": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 200
                },
                "mechvent3": {
                    "vent_type": "Intermittent MEV",
                    "SFP": 1.5,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 200
                }
            }
        }"#;

        let ventilation = serde_json::from_str(json).unwrap();
        let result =
            part_f::validate_dwelling_ventilation(&ventilation, 100., 4, 5, 3, 2, 0, 0, 2, true);

        assert!(result.is_err());
        let _ = result.inspect_err(|e| {
            assert!(e.to_string().contains("Dwelling lacks sufficient number of background vents for intermittent ventilation."));
        });
    }

    // test_raises_if_insufficient_decentralised_continuous_MEV_count
    #[test]
    fn test_raises_if_insufficient_decentralised_continuous_mev_count() {
        // Given a dwelling with insufficient decentralised continuous MEV vents
        let json = r#"{
            "Vents": {
                "vent1": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 100,
                    "pressure_difference_ref": 20,
                    "orientation360": 180,
                    "pitch": 60
                },
                "vent2": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 100,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60
                },
                "vent3": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 100,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60
                }
            },
            "MechanicalVentilation": {
                "mechvent1": {
                    "vent_type": "Decentralised continuous MEV",
                    "measured_fan_power": 12.26,
                    "measured_air_flow_rate": 37,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 133.2
                }
            }
        }"#;

        let ventilation = serde_json::from_str(json).unwrap();
        let result =
            part_f::validate_dwelling_ventilation(&ventilation, 100., 4, 5, 3, 2, 0, 0, 2, true);

        assert!(result.is_err());
        let _ = result.inspect_err(|e| {
            assert!(e.to_string().contains("Dwelling lacks sufficient number of decentralised mechanical vents for continuous ventilation."));
        });
    }

    /// Given a dwelling with only MVHR mech vents and one background vent
    #[test]
    fn test_raises_if_mvhr_system_with_background_vents() {
        let vents = json!({
            "Vents": {
                "vent1": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 1000,
                    "pressure_difference_ref": 20,
                    "orientation360": 180,
                    "pitch": 60,
                }
            },
            "MechanicalVentilation": {
                "mechvent1": {
                    "sup_air_flw_ctrl": "ODA",
                    "sup_air_temp_ctrl": "NO_CTRL",
                    "vent_type": "MVHR",
                    "measured_fan_power": 12.26,
                    "measured_air_flow_rate": 37,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 133.2,
                    "mvhr_eff": 0.0,
                    "mvhr_location": "inside",
                    "position_intake": {
                        "mid_height_air_flow_path": 1.5,
                        "orientation360": 90,
                        "pitch": 60,
                    },
                    "position_exhaust": {
                        "mid_height_air_flow_path": 1.5,
                        "orientation360": 90,
                        "pitch": 60,
                    },
                    "ductwork": [
                        {
                            "cross_section_shape": "circular",
                            "external_diameter_mm": 160,
                            "insulation_thermal_conductivity": 0.04,
                            "insulation_thickness_mm": 25,
                            "internal_diameter_mm": 150,
                            "length": 5.0,
                            "reflective": false,
                            "duct_type": "supply",
                        }
                    ],
                },
                "mechvent2": {
                    "sup_air_flw_ctrl": "ODA",
                    "sup_air_temp_ctrl": "NO_CTRL",
                    "vent_type": "MVHR",
                    "measured_fan_power": 12.26,
                    "measured_air_flow_rate": 37,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 133.2,
                    "mvhr_eff": 0.0,
                    "mvhr_location": "inside",
                    "position_intake": {
                        "mid_height_air_flow_path": 1.5,
                        "orientation360": 90,
                        "pitch": 60,
                    },
                    "position_exhaust": {
                        "mid_height_air_flow_path": 1.5,
                        "orientation360": 90,
                        "pitch": 60,
                    },
                    "ductwork": [
                        {
                            "cross_section_shape": "circular",
                            "external_diameter_mm": 160,
                            "insulation_thermal_conductivity": 0.04,
                            "insulation_thickness_mm": 25,
                            "internal_diameter_mm": 150,
                            "length": 5.0,
                            "reflective": false,
                            "duct_type": "supply",
                        }
                    ],
                },
            },
        });
        let result = part_f::validate_dwelling_ventilation(&vents, 100., 4, 5, 3, 2, 0, 0, 2, true);
        assert!(result.is_err());
        let _ = result.inspect_err(|e| {
            assert!(e.to_string().contains(
                "Dwelling with MVHR ventilation system should not have background vents"
            ));
        });
    }

    /// Given a dwelling with a MVHR mech vent, a decentralised continuous MEV
    /// and background vents
    #[test]
    fn test_does_not_raise_if_mixed_mvhr_system_with_background_vents() {
        let vents = json!({
            "Vents": {
                "vent1": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 100,
                    "pressure_difference_ref": 20,
                    "orientation360": 180,
                    "pitch": 60,
                },
                "vent2": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 100,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60,
                },
                "vent3": {
                    "mid_height_air_flow_path": 1.5,
                    "area_cm2": 100,
                    "pressure_difference_ref": 20,
                    "orientation360": 0,
                    "pitch": 60,
                },
            },
            "MechanicalVentilation": {
                "mechvent1": {
                    "sup_air_flw_ctrl": "ODA",
                    "sup_air_temp_ctrl": "NO_CTRL",
                    "vent_type": "MVHR",
                    "measured_fan_power": 12.26,
                    "measured_air_flow_rate": 37,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 133.2,
                    "mvhr_eff": 0.0,
                    "mvhr_location": "inside",
                    "position_intake": {
                        "mid_height_air_flow_path": 1.5,
                        "orientation360": 90,
                        "pitch": 60,
                    },
                    "position_exhaust": {
                        "mid_height_air_flow_path": 1.5,
                        "orientation360": 90,
                        "pitch": 60,
                    },
                    "ductwork": [
                        {
                            "cross_section_shape": "circular",
                            "external_diameter_mm": 160,
                            "insulation_thermal_conductivity": 0.04,
                            "insulation_thickness_mm": 25,
                            "internal_diameter_mm": 150,
                            "length": 5.0,
                            "reflective": false,
                            "duct_type": "supply",
                        }
                    ],
                },
                "mechvent2": {
                    "sup_air_flw_ctrl": "ODA",
                    "sup_air_temp_ctrl": "NO_CTRL",
                    "vent_type": "Decentralised continuous MEV",
                    "measured_fan_power": 12.26,
                    "measured_air_flow_rate": 37,
                    "EnergySupply": "mains elec",
                    "design_outdoor_air_flow_rate": 133.2,
                },
            },
        });
        assert!(
            part_f::validate_dwelling_ventilation(&vents, 100., 1, 5, 1, 2, 0, 0, 2, true).is_ok()
        );
    }
}
