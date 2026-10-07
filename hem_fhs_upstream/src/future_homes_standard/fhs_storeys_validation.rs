use crate::future_homes_standard::input::InputForProcessing;
use anyhow::bail;

///  Validate that the number of storeys in the building, if specified, is greater
///  than or equal to the number of storeys in the dwelling.
pub(crate) fn validate_storeys_in_building_and_dwelling(
    input_for_processing: &InputForProcessing,
) -> anyhow::Result<()> {
    let storeys_in_dwelling = input_for_processing.storeys_in_dwelling()?;
    let storeys_in_building = input_for_processing.storeys_in_building()?;

    if let Some(storeys_in_building) = storeys_in_building {
        if storeys_in_building < storeys_in_dwelling {
            bail!("The 'storeys_in_building' property, {}, must be greater than or equal to the 'storeys_in_dwelling' property, {} ", storeys_in_building, storeys_in_dwelling);
        }
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use rstest::*;
    use serde_json::json;

    fn storeys_input(storeys_in_building: usize) -> InputForProcessing {
        let storeys_input_json = json!({
            "General":
                {
                    "build_type": "flat",
                    "storeys_in_building": storeys_in_building,
                    "storeys_in_dwelling": 2
                }
        });

        InputForProcessing {
            input: storeys_input_json,
        }
    }

    #[rstest]
    fn test_does_not_error_when_sufficient_storeys_in_building() {
        // Given a two storey flat in a 30 storey building
        let input = storeys_input(30);
        // When validation is called
        assert!(validate_storeys_in_building_and_dwelling(&input).is_ok())
        // Then no error is raised
    }

    #[rstest]
    fn test_does_not_error_when_storeys_in_building_and_storeys_in_dwelling_are_equal() {
        // Given a two storey flat in a two storey building
        let input = storeys_input(2);
        // When validation is called
        assert!(validate_storeys_in_building_and_dwelling(&input).is_ok())
        // Then no error is raised
    }

    #[rstest]
    fn test_error_when_insufficient_storeys_in_building() {
        // Given a two storey flat in a single storey building
        let input = storeys_input(1);
        // When validation is called
        // Then an error is raised
        assert!(validate_storeys_in_building_and_dwelling(&input).is_err())
    }
}
