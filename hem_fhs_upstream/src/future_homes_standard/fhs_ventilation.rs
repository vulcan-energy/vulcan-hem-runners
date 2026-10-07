use crate::future_homes_standard::input::{json_error, InputForProcessing};
use anyhow::bail;
use home_energy_model::core::space_heat_demand::building_element::{
    pitch_class, HeatFlowDirection,
};
use serde_json::{json, Map, Value};

/// Returns a JSON Value containing vents that provide background ventilation
/// Each vent's height, pitch and orientation is based on it being
/// located in one of the building's windows or walls.
pub(crate) fn create_background_vents(
    input: &InputForProcessing,
    minimum_vent_area: f64,
    minimum_vent_count: usize,
) -> anyhow::Result<Value> {
    let building_elements = input.all_building_element_values()?;
    let window_vent_placements = sorted_windows_by_area(&building_elements)?;
    let mut num_remaining_vents =
        minimum_vent_count as isize - window_vent_placements.len() as isize;
    let mut wall_vent_placements = Vec::new();

    // Place any remaining vents in walls in order of decreasing wall area,
    // i.e.starting with the wall with the largest area
    if num_remaining_vents > 0 {
        let mut walls = sorted_walls_by_area(&building_elements)?;

        if walls.is_empty() {
            bail!("Unable to place {num_remaining_vents} remaining background vent(s). Dwelling lacks suitable walls.");
        }

        walls.reverse();

        let mut wall_index = 0;
        // keep looping through the walls until all vents are placed
        while num_remaining_vents > 0 {
            wall_vent_placements.push(walls[wall_index].clone());
            num_remaining_vents -= 1;
            wall_index = (wall_index + 1) % walls.len();
        }
    }

    let vent_placements = [window_vent_placements, wall_vent_placements].concat();
    let mut background_vents = Map::new();
    let ventilation_zone_base_height = input.ventilation_zone_base_height()?;
    let vent_area = minimum_vent_area / vent_placements.len() as f64;

    for (i, vent_placement) in vent_placements.iter().enumerate() {
        let vent_key = format!("vent_{i}");
        let vent_value =
            create_background_vent(ventilation_zone_base_height, vent_area, vent_placement)?;

        background_vents.insert(vent_key, vent_value);
    }

    Ok(Value::from(background_vents))
}

fn create_background_vent(
    ventilation_zone_base_height: f64,
    vent_area: f64,
    building_element: &Value,
) -> anyhow::Result<Value> {
    let mid_height_air_flow_path =
        calc_vent_mid_height_airflow_path(ventilation_zone_base_height, building_element)?;

    Ok(json!({
        "area_cm2": vent_area,
        "pitch": building_element["pitch"],
        "orientation360": building_element["orientation360"],
        "mid_height_air_flow_path": mid_height_air_flow_path,
        "pressure_difference_ref": 20,
    }))
}

/// Returns a JSON Value containing mechanical ventilation objects with vent_type
/// "Decentralised continuous MEV", according to the following rules:
///     - Create one dMEV per wet room
///     - Assign dMEVs first to the smallest windows, then to the largest walls if needed
///     - Ensure total summed air flow rate equals the specified minimum
pub(crate) fn create_mechanical_ventilation(
    input: &InputForProcessing,
    minimum_air_flow_rate: f64,
) -> anyhow::Result<Value> {
    let building_elements = input.all_building_element_values()?;

    // Position vents in smallest windows that aren't rooflights
    let mut windows_excluding_rooflights = Vec::new();

    for window in sorted_windows_by_area(&building_elements)? {
        let pitch = window
            .as_object()
            .and_then(|el| el.get("pitch"))
            .and_then(|pitch| pitch.as_f64())
            .ok_or_else(|| json_error("Building element 'pitch' missing or not a number"))?;

        if pitch_class(pitch) == HeatFlowDirection::Horizontal {
            windows_excluding_rooflights.push(window);
        }
    }

    let number_of_wet_rooms = input
        .number_of_wet_rooms()?
        .ok_or_else(|| json_error("Expected NumberOfWetRooms to be provided"))?;
    let mut vent_placements: Vec<Value> = windows_excluding_rooflights
        .into_iter()
        .take(number_of_wet_rooms)
        .collect();
    // If needed, position remaining vents in largest walls
    let mut num_remaining_vent_placements =
        number_of_wet_rooms as isize - vent_placements.len() as isize;

    if num_remaining_vent_placements > 0 {
        let mut walls = sorted_walls_by_area(&building_elements)?;

        if walls.is_empty() {
            bail!("Unable to place {num_remaining_vent_placements} remaining vent(s). Dwelling lacks suitable walls.");
        }

        walls.reverse();

        let mut wall_index = 0;
        // keep looping through the walls until all vents are placed
        while num_remaining_vent_placements > 0 {
            vent_placements.push(walls[wall_index].clone());
            num_remaining_vent_placements -= 1;
            wall_index = (wall_index + 1) % walls.len();
        }
    }

    let ventilation_zone_base_height = input.ventilation_zone_base_height()?;
    let airflow_rate_per_vent = minimum_air_flow_rate / number_of_wet_rooms as f64;

    let mut dmevs = Map::new();
    for (i, vent_placement) in vent_placements.iter().enumerate() {
        let vent_mid_height_airflow_path =
            calc_vent_mid_height_airflow_path(ventilation_zone_base_height, vent_placement)?;
        let (orientation, pitch) = vent_placement
            .as_object()
            .and_then(|el| {
                let orientation = el.get("orientation360")?.as_f64()?;
                let pitch = el.get("pitch")?.as_f64()?;

                Some((orientation, pitch))
            })
            .ok_or_else(|| json_error("One or more building element fields missing or invalid: 'orientation360', 'pitch'"))?;

        let dmev_key = format!("Decentralised_Continuous_MEV_{i}");
        let dmev_value = create_dmev(
            airflow_rate_per_vent,
            vent_mid_height_airflow_path,
            orientation,
            pitch,
        );
        dmevs.insert(dmev_key, dmev_value);
    }

    Ok(Value::from(dmevs))
}

fn create_dmev(
    design_outdoor_air_flow_rate: f64,
    mid_height_air_flow_path: f64,
    orientation360: f64,
    pitch: f64,
) -> Value {
    let default_dmev_sfp = 0.15;

    json!({
        "sup_air_flw_ctrl": "ODA",
        "sup_air_temp_ctrl": "NO_CTRL",
        "vent_type": "Decentralised continuous MEV",
        "SFP": default_dmev_sfp,
        "EnergySupply": "mains elec",
        "design_outdoor_air_flow_rate": design_outdoor_air_flow_rate,
        "mid_height_air_flow_path": mid_height_air_flow_path,
        "orientation360": orientation360,
        "pitch": pitch,
    })
}

fn calc_vent_mid_height_airflow_path(
    ventilation_zone_base_height: f64,
    building_element: &Value,
) -> anyhow::Result<f64> {
    let (el_type, base_height, height, pitch) = building_element
        .as_object()
        .and_then(|el| {
            let el_type = el.get("type")?.as_str()?;
            let base_height = el.get("base_height")?.as_f64()?;
            let height = el.get("height")?.as_f64()?;
            let pitch = el.get("pitch")?.as_f64()?;

            Some((el_type, base_height, height, pitch))
        })
        .ok_or_else(|| json_error("One or more building element fields missing or invalid: 'type', 'base_height', 'height', 'pitch'"))?;

    Ok(if el_type == "BuildingElementOpaque" {
        base_height + (height * pitch.to_radians().sin() / 2.) - ventilation_zone_base_height
    } else {
        base_height + height * pitch.to_radians().sin() - ventilation_zone_base_height
    })
}

fn sorted_windows_by_area(building_elements: &[&Value]) -> anyhow::Result<Vec<Value>> {
    let mut windows = Vec::new();

    for building_element in building_elements {
        let el_type = building_element
            .as_object()
            .ok_or_else(|| json_error("Building element was not an object"))?
            .get("type")
            .and_then(|el_type| el_type.as_str())
            .ok_or_else(|| json_error("Building element 'type' missing or not a string"))?;

        if el_type == "BuildingElementTransparent" {
            let width = building_element
                .get("width")
                .and_then(|width| width.as_f64())
                .ok_or_else(|| json_error("Building element 'width' missing or not a number"))?;
            let height = building_element
                .get("height")
                .and_then(|height| height.as_f64())
                .ok_or_else(|| json_error("Building element 'height' missing or not a number"))?;
            windows.push((width * height, (*building_element).clone()));
        }
    }

    windows.sort_by(|a, b| a.0.total_cmp(&b.0));

    Ok(windows.into_iter().map(|(_, element)| element).collect())
}

fn sorted_walls_by_area(building_elements: &[&Value]) -> anyhow::Result<Vec<Value>> {
    let mut walls = Vec::new();

    for building_element in building_elements {
        let element = building_element
            .as_object()
            .ok_or_else(|| json_error("Building element was not an object"))?;
        let el_type = element
            .get("type")
            .and_then(|el_type| el_type.as_str())
            .ok_or_else(|| json_error("Building element 'type' missing or not a string"))?;

        if el_type == "BuildingElementOpaque" {
            let pitch = element
                .get("pitch")
                .and_then(|pitch| pitch.as_f64())
                .ok_or_else(|| json_error("Building element 'pitch' missing or not a number"))?;

            if pitch_class(pitch) == HeatFlowDirection::Horizontal {
                let is_external_door = element
                    .get("is_external_door")
                    .and_then(|v| v.as_bool())
                    .ok_or_else(|| {
                        json_error("Building element 'is_external_door' missing or not a boolean")
                    })?;
                let area = element
                    .get("area")
                    .and_then(|area| area.as_f64())
                    .ok_or_else(|| json_error("Building element 'area' missing or not a number"))?;

                if !is_external_door {
                    walls.push((area, (*building_element).clone()));
                }
            }
        }
    }
    walls.sort_by(|a, b| a.0.total_cmp(&b.0));

    Ok(walls.into_iter().map(|(_, element)| element).collect())
}

#[cfg(test)]
mod test {
    use super::*;
    use approx::assert_relative_eq;
    use rstest::*;
    use serde_json::json;

    #[fixture]
    fn mech_vent_input() -> InputForProcessing {
        let input_json = json!({
            "NumberOfWetRooms": 2,
            "InfiltrationVentilation": {
                "ventilation_zone_base_height": 0,
                "MechanicalVentilation": {
                    "mechvent1": {
                        "vent_type": "Centralised continuous MEV",
                        "measured_fan_power": 12.26,
                        "measured_air_flow_rate": 37,
                        "EnergySupply": "mains elec",
                        "design_outdoor_air_flow_rate": 80,
                        "mid_height_air_flow_path": 1.5,
                        "orientation360": 90,
                        "pitch": 60,
                    }
                },
            },
            "Zone": {
                "whole dwelling": {
                    "BuildingElement": {
                        "window 1": {
                            "type": "BuildingElementTransparent",
                            "thermal_resistance_construction": 0.4,
                            "pitch": 90,
                            "orientation360": 270,
                            "g_value": 0.75,
                            "frame_area_fraction": 0.25,
                            "base_height": 1,
                            "height": 1.25,
                            "width": 4,
                            "free_area_height": 1.6,
                            "mid_height": 1.5,
                            "max_window_open_area": 3,
                            "security_risk": true,
                            "window_part_list": [{"mid_height_air_flow_path": 1.5}],
                            "shading": [
                                {"type": "overhang", "depth": 0.5, "distance": 0.5},
                                {"type": "sidefinleft", "depth": 0.25, "distance": 0.1},
                                {"type": "sidefinright", "depth": 0.25, "distance": 0.1},
                            ],
                        },
                        "window 2": {
                            "type": "BuildingElementTransparent",
                            "thermal_resistance_construction": 0.4,
                            "pitch": 90,
                            "orientation360": 90,
                            "g_value": 0.75,
                            "frame_area_fraction": 0.25,
                            "base_height": 1,
                            "height": 1.25,
                            "width": 3,
                            "free_area_height": 1.6,
                            "mid_height": 1.5,
                            "max_window_open_area": 3,
                            "security_risk": true,
                            "window_part_list": [{"mid_height_air_flow_path": 1.5}],
                            "shading": [
                                {"type": "overhang", "depth": 0.5, "distance": 0.5},
                                {"type": "sidefinleft", "depth": 0.25, "distance": 0.1},
                                {"type": "sidefinright", "depth": 0.25, "distance": 0.1},
                            ],
                        },
                        "wall 0": {
                            "type": "BuildingElementOpaque",
                            "thermal_resistance_construction": 0.71,
                            "areal_heat_capacity": "Very light",
                            "mass_distribution_class": "M: Mass concentrated inside",
                            "pitch": 90,
                            "is_external_door": false,
                            "orientation360": 270,
                            "base_height": 0,
                            "height": 2.5,
                            "width": 10,
                            "area": 25.0,
                        },
                        "wall 1": {
                            "type": "BuildingElementOpaque",
                            "colour": "Intermediate",
                            "thermal_resistance_construction": 0.72,
                            "areal_heat_capacity": "Very light",
                            "mass_distribution_class": "E: Mass concentrated at external side",
                            "pitch": 90,
                            "is_external_door": false,
                            "orientation360": 0,
                            "base_height": 0,
                            "height": 2.5,
                            "width": 8,
                            "area": 20.0,
                        },
                    }
                }
            },
        });

        InputForProcessing { input: input_json }
    }

    #[rstest]
    fn test_two_vents_assigned_to_windows(mech_vent_input: InputForProcessing) {
        // Given an input with a dwelling with two wet rooms, two windows
        // and a part f minimum air flow rate of 100
        let minimum_air_flow_rate = 100.;

        // When the new mechanical vents are created
        let results =
            create_mechanical_ventilation(&mech_vent_input, minimum_air_flow_rate).unwrap();

        // Then there are two dMEVs created: one for each wet room
        // the total air flow rate of the dMEVs is equal to the part f minimum
        // the positions of the two vents are correctly taken from the windows
        let expected = json!({
            "Decentralised_Continuous_MEV_0": {
                "sup_air_flw_ctrl": "ODA",
                "sup_air_temp_ctrl": "NO_CTRL",
                "vent_type": "Decentralised continuous MEV",
                "SFP": 0.15,
                "EnergySupply": "mains elec",
                "design_outdoor_air_flow_rate": 50.,
                "mid_height_air_flow_path": 2.25,
                "orientation360": 90.,
                "pitch": 90.,
            },
            "Decentralised_Continuous_MEV_1": {
                "sup_air_flw_ctrl": "ODA",
                "sup_air_temp_ctrl": "NO_CTRL",
                "vent_type": "Decentralised continuous MEV",
                "SFP": 0.15,
                "EnergySupply": "mains elec",
                "design_outdoor_air_flow_rate": 50.,
                "mid_height_air_flow_path": 2.25,
                "orientation360": 270.,
                "pitch": 90.,
            },
        });

        assert_eq!(results, expected);
    }

    #[rstest]
    fn test_two_vents_assigned_to_window_and_wall(mut mech_vent_input: InputForProcessing) {
        // Given an input with a dwelling with two wet rooms, one window, two walls
        // and a part f minimum air flow rate of 100
        mech_vent_input.input["Zone"]["whole dwelling"]["BuildingElement"]
            .as_object_mut()
            .unwrap()
            .remove("window 1");
        let minimum_air_flow_rate = 100.;

        // When the mechanical vents are created
        let results =
            create_mechanical_ventilation(&mech_vent_input, minimum_air_flow_rate).unwrap();

        // Then there are two dMEVs created: one for each wet room
        // the total air flow rate of the dMEVs is equal to the part f minimum
        // the positions of the two vents are correctly taken from the window
        // and the largest wall
        let expected = json!({
            "Decentralised_Continuous_MEV_0": {
                "sup_air_flw_ctrl": "ODA",
                "sup_air_temp_ctrl": "NO_CTRL",
                "vent_type": "Decentralised continuous MEV",
                "SFP": 0.15,
                "EnergySupply": "mains elec",
                "design_outdoor_air_flow_rate": 50.0,
                "mid_height_air_flow_path": 2.25,
                "orientation360": 90.,
                "pitch": 90.,
            },
            "Decentralised_Continuous_MEV_1": {
                "sup_air_flw_ctrl": "ODA",
                "sup_air_temp_ctrl": "NO_CTRL",
                "vent_type": "Decentralised continuous MEV",
                "SFP": 0.15,
                "EnergySupply": "mains elec",
                "design_outdoor_air_flow_rate": 50.0,
                "mid_height_air_flow_path": 1.25,
                "orientation360": 270.,
                "pitch": 90.,
            },
        });

        assert_eq!(results, expected);
    }

    #[rstest]
    fn test_vent_not_assigned_to_rooflight(mut mech_vent_input: InputForProcessing) {
        // Given an input with a dwelling with two wet rooms, a rooflight, two walls
        // and a part f minimum air flow rate of 100
        mech_vent_input.input["Zone"]["whole dwelling"]["BuildingElement"]
            .as_object_mut()
            .unwrap()
            .remove("window 1");
        mech_vent_input.input["Zone"]["whole dwelling"]["BuildingElement"]["window 2"]["pitch"] =
            json!(180);
        mech_vent_input.input["Zone"]["whole dwelling"]["BuildingElement"]["window 2"]
            ["orientation360"] = json!(180);
        let minimum_air_flow_rate = 100.;

        // When the mechanical vents are created
        let results =
            create_mechanical_ventilation(&mech_vent_input, minimum_air_flow_rate).unwrap();

        // Then there are two dMEVs created: one for each wet room
        // the total air flow rate of the dMEVs is equal to the part f minimum
        // the positions of the two vents are correctly taken from the two
        // walls because dMEVs are not placed in rooflights
        let expected = json!({
            "Decentralised_Continuous_MEV_0": {
                "sup_air_flw_ctrl": "ODA",
                "sup_air_temp_ctrl": "NO_CTRL",
                "vent_type": "Decentralised continuous MEV",
                "SFP": 0.15,
                "EnergySupply": "mains elec",
                "design_outdoor_air_flow_rate": 50.0,
                "mid_height_air_flow_path": 1.25,
                "orientation360": 270.,
                "pitch": 90.,
            },
            "Decentralised_Continuous_MEV_1": {
                "sup_air_flw_ctrl": "ODA",
                "sup_air_temp_ctrl": "NO_CTRL",
                "vent_type": "Decentralised continuous MEV",
                "SFP": 0.15,
                "EnergySupply": "mains elec",
                "design_outdoor_air_flow_rate": 50.0,
                "mid_height_air_flow_path": 1.25,
                "orientation360": 0.,
                "pitch": 90.,
            },
        });

        assert_eq!(results, expected);
    }

    #[rstest]
    fn test_five_vents_assigned_to_window_and_walls_recursively(
        mut mech_vent_input: InputForProcessing,
    ) {
        // Given an input with a dwelling with 5 wet rooms, one window, two walls
        // and a part f minimum air flow rate of 100
        mech_vent_input.input["NumberOfWetRooms"] = json!(5);
        mech_vent_input.input["Zone"]["whole dwelling"]["BuildingElement"]
            .as_object_mut()
            .unwrap()
            .remove("window 1");
        let minimum_air_flow_rate = 100.;

        // When the mechanical vents are created
        let results =
            create_mechanical_ventilation(&mech_vent_input, minimum_air_flow_rate).unwrap();

        // Then there are ten dMEVs created: one for each wet room
        // the total air flow rate of the dMEVs is equal to the part f minimum
        // the positions of the two vents are correctly taken from the window
        // and then assigned to walls in descending size order, looping through
        // the walls again when we run out of walls
        // the single window has an orientation 90
        // the smallest wall has an orientation 0
        // the largest wall has an orientation 270
        let expected = json!({
            "Decentralised_Continuous_MEV_0": {
                "sup_air_flw_ctrl": "ODA",
                "sup_air_temp_ctrl": "NO_CTRL",
                "vent_type": "Decentralised continuous MEV",
                "SFP": 0.15,
                "EnergySupply": "mains elec",
                "design_outdoor_air_flow_rate": 20.0,
                "mid_height_air_flow_path": 2.25,
                "orientation360": 90.,  // first vent assigned to window
                "pitch": 90.,
            },
            "Decentralised_Continuous_MEV_1": {
                "sup_air_flw_ctrl": "ODA",
                "sup_air_temp_ctrl": "NO_CTRL",
                "vent_type": "Decentralised continuous MEV",
                "SFP": 0.15,
                "EnergySupply": "mains elec",
                "design_outdoor_air_flow_rate": 20.0,
                "mid_height_air_flow_path": 1.25,
                "orientation360": 270.,  // second vent assigned to largest wall
                "pitch": 90.,
            },
            "Decentralised_Continuous_MEV_2": {
                "sup_air_flw_ctrl": "ODA",
                "sup_air_temp_ctrl": "NO_CTRL",
                "vent_type": "Decentralised continuous MEV",
                "SFP": 0.15,
                "EnergySupply": "mains elec",
                "design_outdoor_air_flow_rate": 20.0,
                "mid_height_air_flow_path": 1.25,
                "orientation360": 0.,  // next vent assigned to smallest wall
                "pitch": 90.,
            },
            "Decentralised_Continuous_MEV_3": {
                "sup_air_flw_ctrl": "ODA",
                "sup_air_temp_ctrl": "NO_CTRL",
                "vent_type": "Decentralised continuous MEV",
                "SFP": 0.15,
                "EnergySupply": "mains elec",
                "design_outdoor_air_flow_rate": 20.0,
                "mid_height_air_flow_path": 1.25,
                "orientation360": 270.,  // we've run out of walls so loop back and the next vent is assigned to largest wall
                "pitch": 90.,
            },
            "Decentralised_Continuous_MEV_4": {
                "sup_air_flw_ctrl": "ODA",
                "sup_air_temp_ctrl": "NO_CTRL",
                "vent_type": "Decentralised continuous MEV",
                "SFP": 0.15,
                "EnergySupply": "mains elec",
                "design_outdoor_air_flow_rate": 20.0,
                "mid_height_air_flow_path": 1.25,
                "orientation360": 0.,  // next vent assigned to smallest wall
                "pitch": 90.,
            },
        });

        assert_eq!(results, expected);
    }

    #[rstest]
    fn test_raises_error_when_insufficient_walls(mut mech_vent_input: InputForProcessing) {
        // Given an input with a dwelling with five wet rooms, two windows but no walls
        mech_vent_input.input["NumberOfWetRooms"] = json!(5);
        let building_element = mech_vent_input.input["Zone"]["whole dwelling"]["BuildingElement"]
            .as_object_mut()
            .unwrap();
        building_element.remove("wall 0");
        building_element.remove("wall 1");

        let minimum_air_flow_rate = 100.;

        // When the mechanical vents are created
        let results = create_mechanical_ventilation(&mech_vent_input, minimum_air_flow_rate);

        // Then an error is raised describing the lack of walls
        assert_eq!(
            results.unwrap_err().to_string(),
            "Unable to place 3 remaining vent(s). Dwelling lacks suitable walls."
        );
    }

    #[fixture]
    fn background_vents_input() -> InputForProcessing {
        let input_json = json!({
            "NumberOfBedrooms": 0,
            "NumberOfHabitableRooms": 1,
            "InfiltrationVentilation": {"ventilation_zone_base_height": 1.0},
            "Zone": {
                "zone 1": {
                    "BuildingElement": {
                        "window 1": {
                            "type": "BuildingElementTransparent",
                            "pitch": 70,
                            "orientation360": 180,
                            "width": 1.0,
                            "height": 1.2,
                            "base_height": 7.0,
                        },
                        "wall 1": {
                            "type": "BuildingElementOpaque",
                            "colour": "Intermediate",
                            "thermal_resistance_construction": 0.72,
                            "areal_heat_capacity": "Very light",
                            "mass_distribution_class": "E: Mass concentrated at external side",
                            "pitch": 80,
                            "is_external_door": false,
                            "orientation360": 0,
                            "base_height": 0.2,
                            "height": 2.5,
                            "width": 8,
                            "area": 20.0,
                        },
                    }
                }
            },
        });

        InputForProcessing { input: input_json }
    }

    #[rstest]
    fn test_mid_height_air_flow_path_uses_base_height_and_zone_base_height(
        background_vents_input: InputForProcessing,
    ) {
        // Given a dwelling with a ventilation zone base height and one window and one wall,
        // and a requirement for at least two vents
        let minimum_vent_area = 40.;
        let minimum_vent_count = 2;

        // When the background vents are generated
        let vents = create_background_vents(
            &background_vents_input,
            minimum_vent_area,
            minimum_vent_count,
        )
        .unwrap();

        // Then the mid_height_air_flow_path for the window vent has the expected value of
        // height * sin(pitch) + base_height - ventilation_zone_height
        // = 1.2 * sin(radians(70)) + 7.0 - 1.0
        // = 7.1276311449430896
        #[allow(clippy::excessive_precision)]
        let expected_window = 7.1276311449430896;

        assert_relative_eq!(
            vents["vent_0"]["mid_height_air_flow_path"]
                .as_f64()
                .unwrap(),
            expected_window
        );

        // And the mid_height_air_flow_path for the wall vent has the expected value of
        // (height * sin(pitch)) / 2 + base_height - ventilation_zone_height
        // = 2.5 * sin(radians(80)) * 0.5 + 0.2 - 1.0
        // = 0.4310096912652599
        let expected_wall = 0.4310096912652599;

        assert_relative_eq!(
            vents["vent_1"]["mid_height_air_flow_path"]
                .as_f64()
                .unwrap(),
            expected_wall
        )
    }

    #[test]
    fn test_only_places_vents_in_windows_if_there_are_enough_windows() {
        // Given a dwelling with two windows and one wall, and a requirement for at least two vents
        let input = InputForProcessing {
            input: json!({
            "NumberOfBedrooms": 0,
            "NumberOfHabitableRooms": 1,
            "InfiltrationVentilation": {"ventilation_zone_base_height": 0.0},
            "Zone": {
                "zone 1": {
                    "BuildingElement": {
                        "window 0": {
                            "type": "BuildingElementTransparent",
                            "pitch": 90,
                            "orientation360": 180,
                            "width": 1.0,
                            "height": 1.2,
                            "base_height": 1.0,
                        },
                        "window 1": {
                            "type": "BuildingElementTransparent",
                            "pitch": 90,
                            "orientation360": 180,
                            "width": 1.0,
                            "height": 1.6,
                            "base_height": 1.3,
                        },
                        "wall 0": {
                            "type": "BuildingElementOpaque",
                            "colour": "Intermediate",
                            "thermal_resistance_construction": 0.72,
                            "areal_heat_capacity": "Very light",
                            "mass_distribution_class": "E: Mass concentrated at external side",
                            "pitch": 90,
                            "is_external_door": false,
                            "orientation360": 0,
                            "base_height": 0.0,
                            "height": 2.5,
                            "width": 8,
                            "area": 20.0,
                        },
                    }
                }
            }}),
        };
        let minimum_vent_area = 40.;
        let minimum_vent_count = 2;

        // When background vents are generated
        let vents = create_background_vents(&input, minimum_vent_area, minimum_vent_count).unwrap();

        // Then the mid_height_air_flow_path for the vents correspond to them being placed
        // at the top of the two windows (not the wall)
        // height * sin(pitch) + base_height - ventilation_zone_height
        // = 1.2 + 1.0 = 2.2  (window 0)
        // = 1.6 + 1.3 = 2.9  (window 1)
        let expected_num_vents = 2; // max(number_of_windows, minimum_vent_count)
        let expected_window_0 = 2.2;
        let expected_window_1 = 2.9;

        assert_eq!(vents.as_object().unwrap().len(), expected_num_vents);
        assert_relative_eq!(
            vents["vent_0"]["mid_height_air_flow_path"]
                .as_f64()
                .unwrap(),
            expected_window_0
        );
        assert_relative_eq!(
            vents["vent_1"]["mid_height_air_flow_path"]
                .as_f64()
                .unwrap(),
            expected_window_1
        );
    }

    #[test]
    fn test_places_vents_in_windows_and_walls_if_there_are_not_enough_windows() {
        // Given a dwelling with one window and two walls, and a requirement at least two vents
        let input = InputForProcessing {
            input: json!({
            "NumberOfBedrooms": 0,
            "NumberOfHabitableRooms": 1,
            "InfiltrationVentilation": {"ventilation_zone_base_height": 0.0},
            "Zone": {
                "zone 1": {
                    "BuildingElement": {
                        "window 0": {
                            "type": "BuildingElementTransparent",
                            "pitch": 90,
                            "orientation360": 180,
                            "width": 1.0,
                            "height": 1.2,
                            "base_height": 1.0,
                        },
                        "wall 0": {
                            "type": "BuildingElementOpaque",
                            "colour": "Intermediate",
                            "thermal_resistance_construction": 0.72,
                            "areal_heat_capacity": "Very light",
                            "mass_distribution_class": "E: Mass concentrated at external side",
                            "pitch": 90,
                            "is_external_door": false,
                            "orientation360": 0,
                            "base_height": 0.0,
                            "height": 2.5,
                            "width": 8,
                            "area": 20.0,
                        },
                        "wall 1": {
                            "type": "BuildingElementOpaque",
                            "colour": "Intermediate",
                            "thermal_resistance_construction": 0.72,
                            "areal_heat_capacity": "Very light",
                            "mass_distribution_class": "E: Mass concentrated at external side",
                            "pitch": 90,
                            "is_external_door": false,
                            "orientation360": 0,
                            "base_height": 0.0,
                            "height": 3.0,
                            "width": 10,
                            "area": 30.0,
                        },
                    }
                }
            }}),
        };
        let minimum_vent_area = 40.;
        let minimum_vent_count = 2;

        // When background vents are generated
        let vents = create_background_vents(&input, minimum_vent_area, minimum_vent_count).unwrap();

        // Then the mid_height_air_flow_path for the vents correspond to them being placed
        // at the top of the window and the middle of the wall with the greatest area
        // height * sin(pitch) + base_height - ventilation_zone_height
        // = 1.2 + 1.0 = 2.2  (window 0)
        // = 2.5 / 2 = 1.25  (wall 0)
        // = 3.0 / 2 = 1.5  (wall 1)
        let expected_num_vents = 2; // max(number_of_windows, minimum_vent_count)
        let expected_window_0 = 2.2;
        let expected_wall_1 = 1.5;

        assert_eq!(vents.as_object().unwrap().len(), expected_num_vents);
        assert_relative_eq!(
            vents["vent_0"]["mid_height_air_flow_path"]
                .as_f64()
                .unwrap(),
            expected_window_0
        );
        assert_relative_eq!(
            vents["vent_1"]["mid_height_air_flow_path"]
                .as_f64()
                .unwrap(),
            expected_wall_1
        );
    }

    #[test]
    fn test_places_vents_in_all_windows() {
        // Given a dwelling with three windows and a minimum requirement for at least two vents
        let input = InputForProcessing {
            input: json!({
            "NumberOfBedrooms": 0,
            "NumberOfHabitableRooms": 1,
            "InfiltrationVentilation": {"ventilation_zone_base_height": 0.0},
            "Zone": {
                "zone 1": {
                    "BuildingElement": {
                        "window 0": {
                            "type": "BuildingElementTransparent",
                            "pitch": 90,
                            "orientation360": 180,
                            "width": 1.0,
                            "height": 1.2,
                            "base_height": 1.0,
                        },
                        "window 1": {
                            "type": "BuildingElementTransparent",
                            "pitch": 90,
                            "orientation360": 180,
                            "width": 1.0,
                            "height": 1.6,
                            "base_height": 1.3,
                        },
                        "window 2": {
                            "type": "BuildingElementTransparent",
                            "pitch": 90,
                            "orientation360": 180,
                            "width": 0.2,
                            "height": 0.2,
                            "base_height": 1.5,
                        },
                    }
                }
            }}),
        };
        let minimum_vent_area = 40.;
        let minimum_vent_count = 2;

        // When background vents are generated
        let vents = create_background_vents(&input, minimum_vent_area, minimum_vent_count).unwrap();

        // Then the mid_height_air_flow_path for the vents correspond to them being placed
        // at the top of all three windows
        // height * sin(pitch) + base_height - ventilation_zone_height
        // = 1.2 + 1.0 = 2.2  (window 0)
        // = 1.6 + 1.3 = 2.9  (window 1)
        // = 0.2 + 1.5 = 1.7  (window 2)
        // Note that even though the minimum_vent_count is two, we expect
        // create_background_vents() to add a vent to every window (i.e. all three
        // windows), even if there are more windows that minimum_vent_count.
        let expected_num_vents = 3; // max(number_of_windows, minimum_vent_count)
        let expected_window_0 = 2.2;
        let expected_window_1 = 2.9;
        let expected_window_2 = 1.7;

        assert_eq!(vents.as_object().unwrap().len(), expected_num_vents);

        // Ordered from smallest area to largest (window 2 (0.4), window 0 (1.2), window1 (1.6))
        assert_relative_eq!(
            vents["vent_0"]["mid_height_air_flow_path"]
                .as_f64()
                .unwrap(),
            expected_window_2
        );
        assert_relative_eq!(
            vents["vent_1"]["mid_height_air_flow_path"]
                .as_f64()
                .unwrap(),
            expected_window_0
        );
        assert_relative_eq!(
            vents["vent_2"]["mid_height_air_flow_path"]
                .as_f64()
                .unwrap(),
            expected_window_1
        );
    }

    #[rstest]
    fn test_matches_pitch_and_orientation(background_vents_input: InputForProcessing) {
        // Given a dwelling with a ventilation zone base height and one window and one wall,
        // and a requirement for at least two vents
        let minimum_vent_area = 40.;
        let minimum_vent_count = 2;

        // When the background vents are generated
        let vents = create_background_vents(
            &background_vents_input,
            minimum_vent_area,
            minimum_vent_count,
        )
        .unwrap();

        // Then the vents have pitch and orientation of the corresponding window/wall
        assert_eq!(vents["vent_0"]["pitch"], 70); // window 1 pitch
        assert_eq!(vents["vent_0"]["orientation360"], 180); // window 1 orientation
        assert_eq!(vents["vent_1"]["pitch"], 80); // wall 1 pitch
        assert_eq!(vents["vent_1"]["orientation360"], 0); // wall 1 orientation
    }

    #[rstest]
    fn test_distributes_vent_area(background_vents_input: InputForProcessing) {
        // Given a dwelling with a ventilation zone base height and one window and one wall,
        // and a requirement for at least two vents
        let minimum_vent_area = 40.;
        let minimum_vent_count = 2;

        // When the background vents are generated
        let vents = create_background_vents(
            &background_vents_input,
            minimum_vent_area,
            minimum_vent_count,
        )
        .unwrap();

        // Then the minimum_vent_area is distributed equally across the vents
        let expected_vent_area_per_vent = 20.; // 40 / 2

        assert_eq!(vents["vent_0"]["area_cm2"], expected_vent_area_per_vent);
        assert_eq!(vents["vent_1"]["area_cm2"], expected_vent_area_per_vent);
    }

    #[test]
    fn test_places_vents_in_all_windows_including_rooflights() {
        // Given a dwelling with three windows, one of which is a rooflight, and a minimum
        // requirement for at least two vents
        let input = InputForProcessing {
            input: json!({
            "NumberOfBedrooms": 0,
            "NumberOfHabitableRooms": 1,
            "InfiltrationVentilation": {"ventilation_zone_base_height": 1.0},
            "Zone": {
                "zone 1": {
                    "BuildingElement": {
                        "window 0": {
                            "type": "BuildingElementTransparent",
                            "pitch": 90,
                            "orientation360": 180,
                            "width": 1.0,
                            "height": 1.2,
                            "base_height": 1.0,
                        },
                        "window 1": {
                            "type": "BuildingElementTransparent",
                            "pitch": 90,
                            "orientation360": 180,
                            "width": 1.0,
                            "height": 1.6,
                            "base_height": 1.3,
                        },
                        "rooflight 0": {
                            "type": "BuildingElementTransparent",
                            "pitch": 180,
                            "orientation360": 10,
                            "width": 0.2,
                            "height": 0.2,
                            "base_height": 1.5,
                        },
                    }
                }
            }}),
        };
        let minimum_vent_area = 40.;
        let minimum_vent_count = 2;

        // When background vents are generated
        let vents = create_background_vents(&input, minimum_vent_area, minimum_vent_count).unwrap();

        // Then a vent is placed in the rooflight and the other windows
        let expected_num_vents = 3; // max(number_of_windows, minimum_vent_count)

        assert_eq!(vents.as_object().unwrap().len(), expected_num_vents);

        // And the mid_height_air_flow_path of the rooflight vent with 180 deg pitch is as expected
        // height * sin(pitch) + base_height - ventilation_zone_height
        // = 0.2 * 0 + 1.5 - 1.0 = 0.5  (rooflight 0)
        assert_relative_eq!(
            vents["vent_0"]["mid_height_air_flow_path"]
                .as_f64()
                .unwrap(),
            0.5
        );
    }

    #[test]
    fn test_raises_error_when_insufficient_walls_background_vents() {
        // Given a dwelling with one window but no walls, and a minimum
        // requirement for at least two vents
        let input = InputForProcessing {
            input: json!({
            "NumberOfBedrooms": 0,
            "NumberOfHabitableRooms": 1,
            "InfiltrationVentilation": {"ventilation_zone_base_height": 1.0},
            "Zone": {
                "zone 1": {
                    "BuildingElement": {
                        "window 0": {
                            "type": "BuildingElementTransparent",
                            "pitch": 90,
                            "orientation360": 180,
                            "width": 1.0,
                            "height": 1.2,
                            "base_height": 1.0,
                        }
                    }
                }
            }}),
        };
        let minimum_vent_area = 40.;
        let minimum_vent_count = 2;

        // When background vents are generated
        let vents = create_background_vents(&input, minimum_vent_area, minimum_vent_count);

        // Then an error is raised describing the lack of walls
        assert_eq!(
            vents.unwrap_err().to_string(),
            "Unable to place 1 remaining background vent(s). Dwelling lacks suitable walls."
        );
    }

    #[fixture]
    fn no_windows_input() -> InputForProcessing {
        let input_json = json!({
        "NumberOfBedrooms": 0,
        "NumberOfHabitableRooms": 1,
        "InfiltrationVentilation": {"ventilation_zone_base_height": 1.0},
        "Zone": {
            "zone 1": {
                "BuildingElement": {
                    "wall 1": {
                        "type": "BuildingElementOpaque",
                        "colour": "Intermediate",
                        "thermal_resistance_construction": 0.72,
                        "areal_heat_capacity": "Very light",
                        "mass_distribution_class": "E: Mass concentrated at external side",
                        "pitch": 45,
                        "is_external_door": false,
                        "orientation360": 0,
                        "base_height": 0.2,
                        "height": 2.5,
                        "width": 8,
                        "area": 20.0,
                    }
                }
            }
        }});

        InputForProcessing { input: input_json }
    }

    #[rstest]
    fn test_raises_error_when_insufficient_vertical_walls(no_windows_input: InputForProcessing) {
        // Given a dwelling with no windows and no near-vertical walls, and a minimum
        // requirement for at least two vents (where near-vertical refers to a
        // building elements with a pitch such that the pitch class is equal to
        // HeatFlowDirection.HORIZONTAL))
        let minimum_vent_area = 40.;
        let minimum_vent_count = 2;

        // When background vents are generated
        let vents =
            create_background_vents(&no_windows_input, minimum_vent_area, minimum_vent_count);

        // Then an error is raised describing the lack of windows and/or walls
        assert_eq!(
            vents.unwrap_err().to_string(),
            "Unable to place 2 remaining background vent(s). Dwelling lacks suitable walls."
        );
    }

    #[rstest]
    fn test_places_no_vents_when_minimum_vent_count_is_zero_and_there_are_no_windows(
        no_windows_input: InputForProcessing,
    ) {
        // Given a dwelling with no windows, one wall and a minimum
        // requirement for zero vents
        let minimum_vent_area = 40.;
        let minimum_vent_count = 0;

        // When background vents are generated
        let vents =
            create_background_vents(&no_windows_input, minimum_vent_area, minimum_vent_count)
                .unwrap();

        // Then no vents are returned
        let expected_num_vents = 0; // max(number_of_windows, minimum_vent_count)

        assert_eq!(vents.as_object().unwrap().len(), expected_num_vents);
    }
}
