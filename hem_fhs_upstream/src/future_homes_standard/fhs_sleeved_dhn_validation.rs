use crate::future_homes_standard::input::{json_error, InputForProcessing};
use anyhow::bail;
use serde::Deserialize;
use std::cmp::PartialEq;

#[derive(Deserialize, PartialEq)]
pub(crate) enum HeatNetworkType {
    #[serde(rename = "sleeved DHN")]
    SleevedDhn,
    #[serde(rename = "unsleeved DHN")]
    UnsleevedDhn,
    #[serde(rename = "communal")]
    Communal,
}

pub(crate) fn validate_sleeved_dhn(input: &InputForProcessing) -> anyhow::Result<()> {
    // Sleeved DHNs must be used for space heating and DHW
    for (heat_source_wet_key, heat_source_wet) in input.heat_source_wet()? {
        let is_heat_network = heat_source_wet
            .get("is_heat_network")
            .and_then(|v| v.as_bool())
            .ok_or_else(|| json_error("is_heat_network missing or not a boolean"))?;

        if is_heat_network {
            let heat_network_type: HeatNetworkType = heat_source_wet
                .get("heat_network_type")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .ok_or_else(|| json_error("heat_network_type missing or invalid"))?;

            if heat_network_type == HeatNetworkType::SleevedDhn {
                let mut used_for_space_heating = false;

                for space_heat_system_key in input.space_heat_system_keys()? {
                    if let Some(heat_source) =
                        input.heat_source_for_space_heat_system(&space_heat_system_key)?
                    {
                        if heat_source.get("name").and_then(|v| v.as_str())
                            == Some(heat_source_wet_key.as_str())
                        {
                            used_for_space_heating = true;
                            break;
                        }
                    }
                }

                let mut used_for_hot_water = false;

                for hot_water_source in input.hot_water_source()?.values() {
                    if let Some(heat_source_map) = hot_water_source
                        .get("HeatSource")
                        .and_then(|v| v.as_object())
                    {
                        for heat_source in heat_source_map.values() {
                            if heat_source.get("name").and_then(|v| v.as_str())
                                == Some(heat_source_wet_key.as_str())
                            {
                                used_for_hot_water = true;
                                break;
                            }
                        }

                        if used_for_hot_water {
                            break;
                        }
                    }

                    if hot_water_source
                        .get("HeatSourceWet")
                        .and_then(|v| v.as_str())
                        == Some(heat_source_wet_key.as_str())
                    {
                        used_for_hot_water = true;
                        break;
                    }
                }

                if !(used_for_space_heating && used_for_hot_water) {
                    bail!(format!("HeatSourceWet '{heat_source_wet_key}' is a sleeved DHN which must be used for both space heating and hot water"));
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use rstest::*;
    use serde_json::json;

    #[fixture]
    fn dhn_sleeved() -> InputForProcessing {
        let input_json = json!({
            "HeatSourceWet": {
                "sleeved_dhn": {"is_heat_network": true, "heat_network_type": "sleeved DHN"}
            },
            "HotWaterSource": {
                "hw cylinder": {"HeatSource": {"heat network": {"name": "sleeved_dhn"}}}
            },
            "SpaceHeatSystem": {"radiators": {"HeatSource": {"name": "sleeved_dhn"}}}
        });

        InputForProcessing { input: input_json }
    }

    #[fixture]
    fn dhn_sleeved_with_hiu() -> InputForProcessing {
        let input_json = json!({
            "HeatSourceWet": {
                "sleeved_dhn": {"is_heat_network": true, "heat_network_type": "sleeved DHN"}
            },
            "HotWaterSource": {
                "hw cylinder": {
                "type": "HIU",
                "ColdWaterSource": "mains water",
                "HeatSourceWet": "sleeved_dhn",
            }
            },
            "SpaceHeatSystem": {"radiators": {"HeatSource": {"name": "sleeved_dhn"}}}
        });

        InputForProcessing { input: input_json }
    }

    #[rstest]
    fn test_sleeved_dhn_used_for_space_heating_and_dhw(dhn_sleeved: InputForProcessing) {
        // Given an input where a sleeved DHN is used for both HotWater and SpaceHeating
        // When validation is called then no error
        assert!(validate_sleeved_dhn(&dhn_sleeved).is_ok());
    }

    #[rstest]
    fn test_sleeved_dhn_used_for_space_heating_only(mut dhn_sleeved: InputForProcessing) {
        // Given an input where a sleeved DHN is used for SpaceHeating only
        dhn_sleeved.input["HotWaterSource"]["hw cylinder"]["HeatSource"]["heat network"]["name"] =
            json!("blah");

        // When validation is called
        let result = validate_sleeved_dhn(&dhn_sleeved);

        // Then validation error is raised that the sleeved DHN must be used for both
        assert_eq!(result.unwrap_err().to_string(), "HeatSourceWet 'sleeved_dhn' is a sleeved DHN which must be used for both space heating and hot water");
    }

    #[rstest]
    fn test_sleeved_dhn_used_for_hot_water_only(mut dhn_sleeved: InputForProcessing) {
        // Given an input where a sleeved DHN is used for HotWater only
        dhn_sleeved.input["SpaceHeatSystem"]["radiators"]["HeatSource"]["name"] = json!("blah");

        // When validation is called
        let result = validate_sleeved_dhn(&dhn_sleeved);

        // Then validation error is raised that the sleeved DHN must be used for both
        assert_eq!(result.unwrap_err().to_string(), "HeatSourceWet 'sleeved_dhn' is a sleeved DHN which must be used for both space heating and hot water");
    }

    #[rstest]
    fn test_sleeved_dhn_used_by_one_of_many_heating_systems(mut dhn_sleeved: InputForProcessing) {
        // Given an input where a sleeved DHN is used for both HotWater
        // and one of the many SpaceHeating systems (add another system with different source)
        dhn_sleeved.input["SpaceHeatSystem"]["ufh"] =
            json!({"HeatSource": {"name": "another heat source"}});

        // When validation is called then no error
        assert!(validate_sleeved_dhn(&dhn_sleeved).is_ok());
    }

    #[rstest]
    fn test_multiple_sleeved_dhn_heat_sources(mut dhn_sleeved: InputForProcessing) {
        // Given an input where a sleeved DHN is used for both HotWater but another DHN is not
        dhn_sleeved.input["HeatSourceWet"]["sleeved_dhn_2"] = json!({
            "is_heat_network": true,
            "heat_network_type": "sleeved DHN",
        });

        // When validation is called
        let result = validate_sleeved_dhn(&dhn_sleeved);

        // Then validation error is raised that the second sleeved DHN must be used for both
        assert_eq!(result.unwrap_err().to_string(), "HeatSourceWet 'sleeved_dhn_2' is a sleeved DHN which must be used for both space heating and hot water");
    }

    #[rstest]
    fn test_unsleeved_dhn_used_for_hot_water_only(mut dhn_sleeved: InputForProcessing) {
        // Given an input where an unsleeved DHN is used for HotWater only
        dhn_sleeved.input["HeatSourceWet"]["sleeved_dhn"]["heat_network_type"] =
            json!("unsleeved DHN");
        dhn_sleeved.input["SpaceHeatSystem"]["radiators"]["HeatSource"]["name"] = json!("blah");

        // When validation is called then no error
        assert!(validate_sleeved_dhn(&dhn_sleeved).is_ok());
    }

    #[rstest]
    fn test_sleeved_dhn_with_hiu_hot_water_source(dhn_sleeved_with_hiu: InputForProcessing) {
        // Given an input where a sleeved DHN is used for both HotWater and SpaceHeating
        // and the HotWaterSource is of type HIU referencing the sleeved DHN HeatSourceWet
        // When validation is called then no error
        assert!(validate_sleeved_dhn(&dhn_sleeved_with_hiu).is_ok());
    }

    #[rstest]
    fn test_sleeved_dhn_with_hiu_hot_water_source_not_using_the_sleeved_dhn(
        mut dhn_sleeved_with_hiu: InputForProcessing,
    ) {
        // Given an input where a sleeved DHN is used for SpaceHeating but not
        // HotWater because the HotWaterSource of type HIU references another HeatSourceWet
        dhn_sleeved_with_hiu.input["HotWaterSource"]["hw cylinder"]["HeatSourceWet"] =
            json!("another heat source wet");

        // When validation is called
        let result = validate_sleeved_dhn(&dhn_sleeved_with_hiu);

        // Then a validation error is raised that the sleeved DHN must be used for both
        assert_eq!(result.unwrap_err().to_string(), "HeatSourceWet 'sleeved_dhn' is a sleeved DHN which must be used for both space heating and hot water");
    }
}
