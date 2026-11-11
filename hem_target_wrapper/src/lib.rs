// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

//! Thin FHS target runtime boundary. Hosts convert with their own WASM (`wasm_wrapper`).
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn metadata_hem_core_version() -> String {
    hem_upstream::HEM_VERSION.to_string()
}
#[wasm_bindgen]
pub fn metadata_fhs_wrapper_version() -> String {
    hem_fhs_upstream::FHS_VERSION.to_string()
}
#[wasm_bindgen]
pub fn target_weather_protocol_version() -> u32 {
    1
}
#[wasm_bindgen]
pub fn runtime_origin_descriptor() -> String {
    "Vulcan browser HEM runtime; source origin: Home Energy Foundry Limited; marker: HEF-VULCAN-RUNTIME-2026".into()
}
#[wasm_bindgen]
pub fn get_build_info() -> String {
    serde_json::json!({
        "build_id": option_env!("VULCAN_WASM_BUILD_ID"),
        "source_git_commit": option_env!("VULCAN_SOURCE_GIT_COMMIT"),
        "source_tree_sha256": option_env!("VULCAN_SOURCE_TREE_SHA256"),
        "source_tree_dirty": option_env!("VULCAN_SOURCE_TREE_DIRTY"),
        "runtime_origin": runtime_origin_descriptor(),
    })
    .to_string()
}
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn initialize_rayon_thread_pool(num_threads: usize) -> js_sys::Promise {
    wasm_bindgen_rayon::init_thread_pool(num_threads)
}
#[wasm_bindgen]
pub fn setup_panic_hook() {
    #[cfg(target_arch = "wasm32")]
    {
        std::panic::set_hook(Box::new(|panic_info| {
            web_sys::console::error_1(&format!("[WASM] PANIC: {:?}", panic_info).into());
            if let Some(location) = panic_info.location() {
                web_sys::console::error_1(
                    &format!(
                        "[WASM] PANIC LOCATION: {}:{}",
                        location.file(),
                        location.line()
                    )
                    .into(),
                );
                web_sys::console::error_1(
                    &format!("[WASM] PANIC FILE: {}", location.file()).into(),
                );
                web_sys::console::error_1(
                    &format!("[WASM] PANIC LINE: {}", location.line()).into(),
                );
            }
            if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
                web_sys::console::error_1(&format!("[WASM] PANIC PAYLOAD: {}", s).into());
            }
            if let Some(s) = panic_info.payload().downcast_ref::<String>() {
                web_sys::console::error_1(&format!("[WASM] PANIC PAYLOAD: {}", s).into());
            }
        }));
    }
}

#[wasm_bindgen]
pub fn run_target_input(
    model_json: &str,
    output_dir: &str,
    modes_json: &str,
    weather_epw: Option<String>,
) -> String {
    setup_panic_hook();
    match hem_target_core::runtime::run_target_input_inner(
        model_json,
        output_dir,
        modes_json,
        weather_epw.as_deref(),
    ) {
        Ok(result) => result.to_string(),
        Err(error) => serde_json::json!({"status":"failed", "error":error}).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_prepared_input_fails_without_calculation() {
        assert!(run_target_input("[]", "model", r#"["actual"]"#, None).contains("JSON object"));
    }
}

/// Protocol 1: FHS actual export followed by core replay for operating sensitivities.
#[wasm_bindgen]
pub fn target_heating_profile_protocol_version() -> u32 { 1 }

#[wasm_bindgen]
pub fn run_target_stage(model_json: &str, stage: &str, weather_epw: Option<String>) -> String {
    setup_panic_hook();
    match hem_target_core::operating::run_target_stage_inner(model_json, stage, weather_epw.as_deref()) {
        Ok(result) => result.to_string(),
        Err(error) => serde_json::json!({"status":"failed", "error":error}).to_string(),
    }
}
