use crate::future_homes_standard::input::{json_error, InputForProcessing};
use anyhow::bail;

/// Validate that the dwelling contains at least one BuildingElementTransparent which represents a window
pub(crate) fn validate_existence_of_window(input: &InputForProcessing) -> anyhow::Result<()> {
    for (zone_name, zone) in input.zone_node()? {
        let elements = zone
            .get("BuildingElement")
            .and_then(|v| v.as_object())
            .ok_or_else(|| json_error("BuildingElement node not present"))?;

        if !elements.values().any(|el| {
            el.get("type").and_then(|el_type| el_type.as_str())
                == Some("BuildingElementTransparent")
        }) {
            bail!("Zone '{zone_name}' must contain at least one BuildingElementTransparent");
        }
    }

    Ok(())
}

/// Validate that each window's base_height is >= ventilation_zone_base_height.
pub(crate) fn validate_window_base_height_within_ventilation_zone(
    input: &InputForProcessing,
) -> anyhow::Result<()> {
    let ventilation_zone_base_height = input.ventilation_zone_base_height()?;

    for (zone_name, zone) in input.zone_node()? {
        let elements = zone
            .get("BuildingElement")
            .and_then(|v| v.as_object())
            .ok_or_else(|| json_error("BuildingElement node not present"))?;

        for (element_name, element) in elements {
            if element.get("type").and_then(|el_type| el_type.as_str())
                == Some("BuildingElementTransparent")
            {
                let base_height = element
                    .get("base_height")
                    .and_then(|height| height.as_f64())
                    .ok_or_else(|| json_error("Window base_height missing or not a number"))?;

                if base_height < ventilation_zone_base_height {
                    bail!("Window '{element_name}' in Zone '{zone_name}' has base_height ({base_height} m) below ventilation_zone_base_height ({ventilation_zone_base_height} m)");
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::*;
    use serde_json::json;

    #[fixture]
    fn dwelling_zone() -> InputForProcessing {
        let input_json = json!({
            "InfiltrationVentilation": {"ventilation_zone_base_height": 1.0},
            "Zone": {
                "whole dwelling": {
                    "SpaceHeatSystem": "main 1",
                    "SpaceCoolSystem": "cooling system",
                    "livingroom_area": 4.0,
                    "restofdwelling_area": 4.0,
                    "volume": 250.0,
                    "Lighting": {"bulbs": [{"count": 10, "power": 3, "efficacy": 150}]},
                    "BuildingElement": {
                        "wall 1": {
                            "type": "BuildingElementOpaque",
                            "colour": "Intermediate",
                            "thermal_resistance_construction": 0.71,
                            "areal_heat_capacity": "Very light",
                            "mass_distribution_class": "M: Mass concentrated inside",
                            "pitch": 90,
                            "is_external_door": true,
                            "orientation360": 270,
                            "base_height": 0,
                            "height": 2.5,
                            "width": 10,
                            "area": 25.0,
                        },
                        "window 0": {
                            "type": "BuildingElementTransparent",
                            "thermal_resistance_construction": 0.4,
                            "pitch": 90,
                            "orientation360": 90,
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
                            "treatment": [
                                {
                                    "type": "blinds",
                                    "controls": "manual",
                                    "delta_r": 0.05,
                                    "trans_red": 0.5,
                                }
                            ],
                        },
                    },
                    "ThermalBridging": {},
                }
            },
        });

        InputForProcessing { input: input_json }
    }

    #[rstest]
    fn test_dwelling_has_at_least_one_window(dwelling_zone: InputForProcessing) {
        // Given an input where a window is existed
        // When validation is called then no error
        assert!(validate_existence_of_window(&dwelling_zone).is_ok());
    }

    #[rstest]
    fn test_dwelling_has_no_window(mut dwelling_zone: InputForProcessing) {
        // Given an input where not a single window exists
        dwelling_zone.input["Zone"]["whole dwelling"]["BuildingElement"]
            .as_object_mut()
            .unwrap()
            .remove("window 0");

        // When validation is called
        let result = validate_existence_of_window(&dwelling_zone);

        // Then validation error is raised that there is no window exists
        assert_eq!(
            result.unwrap_err().to_string(),
            "Zone 'whole dwelling' must contain at least one BuildingElementTransparent"
        );
    }

    #[rstest]
    fn test_ok_when_base_height_equal(dwelling_zone: InputForProcessing) {
        // Given a window at the same base height as the ventilation zone
        // When the validation function is run then no error raised
        assert!(validate_window_base_height_within_ventilation_zone(&dwelling_zone).is_ok());
    }

    #[rstest]
    fn test_raises_when_base_height_below(mut dwelling_zone: InputForProcessing) {
        // Given a window below the base height of the ventilation zone
        dwelling_zone.input["Zone"]["whole dwelling"]["BuildingElement"]["window 0"]
            ["base_height"] = json!(0.5);

        // When the validation function is run
        let result = validate_window_base_height_within_ventilation_zone(&dwelling_zone);

        // Then an error is raised that windows must be within the ventilation zone
        assert_eq!(result.unwrap_err().to_string(), "Window 'window 0' in Zone 'whole dwelling' has base_height (0.5 m) below ventilation_zone_base_height (1 m)");
    }
}
