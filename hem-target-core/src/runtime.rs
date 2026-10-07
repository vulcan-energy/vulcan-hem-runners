// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

//! Exact pinned FHS execution shared by the host and standalone runner.
use hem_fhs_upstream::HemError;

pub fn prepared_rust_target_flags(
    modes_json: &str,
) -> Result<crate::project_flags::ProjectFlags, String> {
    use crate::project_flags::ProjectFlags;
    let modes: Vec<String> =
        serde_json::from_str(modes_json).map_err(|e| format!("Invalid target modes: {e}"))?;
    let selected: std::collections::BTreeSet<&str> = modes.iter().map(String::as_str).collect();
    if selected.len() != modes.len() {
        return Err("Calculation modes must not be repeated".into());
    }
    if selected == std::collections::BTreeSet::from(["actual"]) {
        return Ok(ProjectFlags::FHS_ASSUMPTIONS);
    }
    if selected
        == std::collections::BTreeSet::from(["actual", "actual-fee", "notional", "notional-fee"])
    {
        return Ok(ProjectFlags::FHS_COMPLIANCE);
    }
    Err("This Rust target supports actual alone or the complete actual, actual-fee, notional, notional-fee compliance set".into())
}

pub fn run_target_input_inner(
    model_json: &str,
    output_dir: &str,
    modes_json: &str,
    weather_epw: Option<&str>,
) -> Result<serde_json::Value, String> {
    let flags = prepared_rust_target_flags(modes_json)?;
    let model: serde_json::Value = serde_json::from_str(model_json)
        .map_err(|e| format!("Invalid prepared input JSON: {e}"))?;
    if !model.is_object() {
        return Err("Prepared FHS input must be a JSON object".into());
    }
    // The established product-only built_form projection is shared with the
    // existing runtime. No schema cleanup or CSV translation occurs here.
    let runtime_input = vulcan_model_transform::clone_for_hem_validation(&model);
    let writer = crate::in_memory_output::InMemoryOutputWriter::new();
    let weather = weather_epw
        .map(external_conditions_from_epw_str)
        .transpose()
        .map_err(|error| format!("Invalid selected EPW weather: {error:#}"))?;
    let response = hem_fhs_upstream::run_wrappers(
        std::io::Cursor::new(runtime_input.to_string()),
        writer.clone(),
        weather,
        None,
        &if flags.contains(crate::project_flags::ProjectFlags::FHS_COMPLIANCE) {
            hem_fhs_upstream::FhsFlags::FHS_COMPLIANCE
        } else {
            hem_fhs_upstream::FhsFlags::FHS
        },
        false,
        false,
        false,
        &[hem_upstream::OutputFormat::Csv],
    )
    .map_err(|error| format_hem_error_report(&error))?;
    let mut files = writer.into_string_map();
    if flags.contains(crate::project_flags::ProjectFlags::FHS_COMPLIANCE) {
        let response =
            response.ok_or("FHS compliance calculation returned no compliance response")?;
        files.insert(
            "fhs_compliance_report.json".into(),
            serde_json::to_string_pretty(&response)
                .map_err(|error| format!("Cannot serialize FHS compliance response: {error}"))?,
        );
    }
    Ok(serde_json::json!({"status":"calculated", "output_directory":output_dir, "files":files}))
}

fn external_conditions_from_epw_str(
    epw: &str,
) -> anyhow::Result<hem_upstream::read_weather_file::ExternalConditions> {
    Ok(
        hem_upstream::read_weather_file::epw_weather_data_to_external_conditions(
            std::io::Cursor::new(epw.as_bytes().to_vec()),
        )?,
    )
}

pub fn format_hem_error_report(err: &HemError) -> String {
    match err {
        HemError::InvalidRequest(e) => {
            format!("Request was considered invalid due to error:\n{:#}", e)
        }
        HemError::FailureInCalculation(e) => {
            format!("Error identified during HEM calculation:\n{:#}", e)
        }
        HemError::ErrorInPostprocessing(e) => {
            format!("Error during wrapper postprocessing:\n{:#}", e)
        }
        HemError::PanicInWrapper(_)
        | HemError::PanicInCalculation(_)
        | HemError::GeneralPanic(_) => err.to_string(),
        HemError::NotImplemented(_) => err.to_string(),
    }
}
