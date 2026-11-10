// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

//! Post-FHS operating sensitivities. Upstream sources and schemas stay unchanged.
use crate::{runtime::format_hem_error_report, InMemoryOutputWriter};
use serde_json::{json, Value};

/// Only the known redundant Rust window default is removable, and only while the
/// selected core schema forbids it. A corrected exporter or schema needs no shim.
fn normalize_window_default(model: &mut Value, schema: &Value) -> Result<Vec<String>, String> {
    let definition = &schema["$defs"]["BuildingElementTransparent"];
    if definition["additionalProperties"] != false
        || definition["properties"]
            .get("areal_heat_capacity")
            .is_some()
    {
        return Ok(vec![]);
    }
    let mut removed = vec![];
    if let Some(zones) = model["Zone"].as_object_mut() {
        for (zone_name, zone) in zones {
            if let Some(elements) = zone["BuildingElement"].as_object_mut() {
                for (name, element) in elements {
                    if element["type"] == "BuildingElementTransparent" {
                        if let Some(value) = element.get("areal_heat_capacity") {
                            if value.as_f64() != Some(1000.0) {
                                return Err(format!("Unsupported exported window heat capacity at {zone_name}/{name}; refusing to discard a non-default value"));
                            }
                            element
                                .as_object_mut()
                                .unwrap()
                                .remove("areal_heat_capacity");
                            removed.push(format!(
                                "Zone/{zone_name}/BuildingElement/{name}/areal_heat_capacity"
                            ));
                        }
                    }
                }
            }
        }
    }
    Ok(removed)
}

pub fn run_target_stage_inner(
    model_json: &str,
    stage: &str,
    weather_epw: Option<&str>,
) -> Result<Value, String> {
    let mut model: Value =
        serde_json::from_str(model_json).map_err(|e| format!("Invalid stage input: {e}"))?;
    if !model.is_object() {
        return Err("Stage input must be a JSON object".into());
    }
    let writer = InMemoryOutputWriter::new();
    match stage {
        "preprocess" => {
            let runtime_input = vulcan_model_transform::clone_for_hem_validation(&model);
            let weather = weather_epw
                .map(|epw| {
                    hem_upstream::read_weather_file::epw_weather_data_to_external_conditions(
                        std::io::Cursor::new(epw.as_bytes().to_vec()),
                    )
                })
                .transpose()
                .map_err(|e| format!("Invalid selected EPW weather: {e:#}"))?;
            hem_fhs_upstream::run_wrappers(
                std::io::Cursor::new(runtime_input.to_string()),
                writer.clone(),
                weather,
                None,
                &hem_fhs_upstream::FhsFlags::FHS,
                true,
                false,
                false,
                &[hem_upstream::OutputFormat::Csv],
            )
            .map_err(|e| format_hem_error_report(&e))?;
            Ok(json!({"status":"calculated", "files":writer.into_string_map()}))
        }
        "core" => {
            // Weather was materialised by preprocessing. Replacing it here could
            // undo the wrapper's shading and weather assumptions.
            if weather_epw.is_some() {
                return Err("Core replay uses weather in the FHS export".into());
            }
            let schema: Value = serde_json::from_str(include_str!(
                "../../hem_engine_upstream/schemas/core-input.schema.json"
            ))
            .map_err(|e| e.to_string())?;
            let removed = normalize_window_default(&mut model, &schema)?;
            let core_input = serde_json::to_string_pretty(&model).map_err(|e| e.to_string())?;
            hem_upstream::run_project_from_input_file(
                hem_upstream::RunInput::Json(model),
                &writer,
                None,
                Some(&vec![hem_upstream::OutputFormat::Csv]),
                None,
                true,
                true,
            )
            .map_err(|e| format!("Core replay failed: {e:#}"))?;
            let mut files = writer.into_string_map();
            files.insert("core-input.json".into(), core_input);
            files.insert("input-compatibility.json".into(), json!({
                "removed_redundant_window_defaults":removed,
                "reason":"Remove only the 1000 J/m2.K Rust window default when forbidden by the selected core schema"
            }).to_string());
            Ok(json!({"status":"calculated", "files":files}))
        }
        _ => Err(format!("Unknown target execution stage: {stage}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn model() -> Value {
        json!({"Zone":{"z":{"BuildingElement":{"w":{"type":"BuildingElementTransparent","areal_heat_capacity":1000.0},"wall":{"type":"BuildingElementOpaque","areal_heat_capacity":1000.0}}}}})
    }
    fn schema() -> Value {
        json!({"$defs":{"BuildingElementTransparent":{"additionalProperties":false,"properties":{}}}})
    }
    #[test]
    fn exported_default_shim_is_specific_and_idempotent() {
        let mut input = model();
        assert_eq!(
            normalize_window_default(&mut input, &schema())
                .unwrap()
                .len(),
            1
        );
        assert!(input["Zone"]["z"]["BuildingElement"]["w"]
            .get("areal_heat_capacity")
            .is_none());
        assert_eq!(
            input["Zone"]["z"]["BuildingElement"]["wall"]["areal_heat_capacity"],
            1000.0
        );
        assert!(normalize_window_default(&mut input, &schema())
            .unwrap()
            .is_empty());
    }
    #[test]
    fn corrected_schema_preserves_the_field_and_nondefault_values() {
        let mut schema = schema();
        schema["$defs"]["BuildingElementTransparent"]["properties"]["areal_heat_capacity"] =
            json!({"type":"number"});
        let mut input = model();
        input["Zone"]["z"]["BuildingElement"]["w"]["areal_heat_capacity"] = json!(2000);
        let original = input.clone();
        assert!(normalize_window_default(&mut input, &schema)
            .unwrap()
            .is_empty());
        assert_eq!(input, original);
    }
    #[test]
    fn unexpected_export_is_not_silently_cleaned() {
        let mut input = model();
        input["Zone"]["z"]["BuildingElement"]["w"]["areal_heat_capacity"] = json!(2000);
        assert!(normalize_window_default(&mut input, &schema())
            .unwrap_err()
            .contains("non-default"));
    }
    #[test]
    fn other_invalid_inputs_still_fail_core_validation() {
        assert!(run_target_stage_inner("{}", "core", None)
            .unwrap_err()
            .contains("invalid JSON"));
        assert!(run_target_stage_inner("{}", "unknown", None)
            .unwrap_err()
            .contains("Unknown"));
    }
}
