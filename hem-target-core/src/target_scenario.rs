// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

//! Shared FHS target preparation and in-memory output primitives.
//!
//! This crate deliberately excludes legacy batch orchestration and
//! product-only runtime. The canonical Geometry CSV transform remains in
//! `vulcan-model-transform`.

use anyhow::Result;
use serde_json::{Map, Value};
use std::collections::HashMap;
use vulcan_model_transform::{
    transform_geometry_csv, TransformError as PipelineError, TransformRequest, ValidationResult,
};

const WALL_PITCH_DEG: f64 = 90.0;
const WALL_PITCH_EPS: f64 = 1e-3;

fn pitch_is_wall_band(pitch: Option<f64>) -> bool {
    pitch
        .map(|p| (p - WALL_PITCH_DEG).abs() < WALL_PITCH_EPS)
        .unwrap_or(false)
}

pub fn update_zone_fabric(zone: &mut Value, fabric_map: &HashMap<String, Map<String, Value>>) {
    if let Some(bes) = zone
        .get_mut("BuildingElement")
        .and_then(Value::as_object_mut)
    {
        for be_props in bes.values_mut() {
            let (element_type, pitch) = (
                be_props
                    .get("type")
                    .and_then(Value::as_str)
                    .map(String::from),
                be_props.get("pitch").and_then(Value::as_f64),
            );

            // Current pipeline emits a boolean; legacy saved models used "TRUE".
            let is_external_door = match be_props.get("is_external_door") {
                Some(Value::Bool(b)) => *b,
                Some(Value::String(s)) => s == "TRUE",
                _ => false,
            };

            let is_wall_pitch = pitch_is_wall_band(pitch);
            // Full-partition targets mark party walls/floors modelled as adjacent
            // conditioned space; half-partition targets carry no such signal.
            let party_partition =
                be_props.get("is_adjacent_space_within_dwelling") == Some(&Value::Bool(false));
            let fabric_key = match element_type.as_deref() {
                Some("BuildingElementOpaque") if is_wall_pitch && is_external_door => Some("door"),
                Some("BuildingElementOpaque") if is_wall_pitch => Some("wall"),
                Some("BuildingElementTransparent") => Some("window"),
                Some("BuildingElementOpaque") if pitch.map(|p| p < 60.0).unwrap_or(false) => {
                    Some("roof")
                }
                Some("BuildingElementGround") => Some("ground"),
                // Party elements only. Within-dwelling partitions and walls to
                // unheated space inherited this key from the pre-PartyWall
                // AdjacentZTC era; the package never described them.
                Some("BuildingElementPartyWall") => Some("party_wall"),
                Some("BuildingElementAdjacentConditionedSpace") if party_partition => {
                    Some("party_wall")
                }
                _ => None,
            };

            if let Some(key) = fabric_key {
                if let Some(fabric_props) = fabric_map.get(key) {
                    if let Some(obj) = be_props.as_object_mut() {
                        obj.remove("r_c");
                        obj.remove("u_value");
                        for (k, v) in fabric_props {
                            // Whole-wall U is target-specific (a9 PartyWall);
                            // prepare_effective_scenario applies it by schema.
                            if k == "u_value_whole_wall" {
                                continue;
                            }
                            obj.insert(k.clone(), v.clone());
                        }
                        // The package is in PartyWall terms (construction to the
                        // midpoint); a full-partition element spans the whole
                        // depth. Qualitative capacity classes are already full.
                        if party_partition {
                            for k in ["thermal_resistance_construction", "areal_heat_capacity"] {
                                if let Some(n) = obj.get(k).and_then(Value::as_f64) {
                                    obj.insert(k.into(), Value::from(n * 2.0));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Update Thermal Bridging separately
    if let Some(tbs) = zone
        .get_mut("ThermalBridging")
        .and_then(Value::as_object_mut)
    {
        if let Some(tb_params) = fabric_map.get("ThermalBridging") {
            if let Some(tb_linear) = tb_params.get("TB_linear") {
                for tb_props in tbs.values_mut() {
                    if let Some(tb_type) = tb_props.get("type").and_then(Value::as_str) {
                        if tb_type == "ThermalBridgeLinear" {
                            update_object(tb_props, tb_linear);
                        }
                    }
                }
            }
            if let Some(tb_point) = tb_params.get("TB_point") {
                for tb_props in tbs.values_mut() {
                    if let Some(tb_type) = tb_props.get("type").and_then(Value::as_str) {
                        if tb_type == "ThermalBridgePoint" {
                            update_object(tb_props, tb_point);
                        }
                    }
                }
            }
        }
    }
}

fn update_object(target: &mut Value, source: &Value) {
    if let (Some(target_obj), Some(source_obj)) = (target.as_object_mut(), source.as_object()) {
        for (key, value) in source_obj {
            target_obj.insert(key.clone(), value.clone());
        }
    }
}

pub fn build_space_heat_system_from_template(
    existing_system: &Value,
    template_system: &Value,
) -> Value {
    let existing_obj = existing_system.as_object();
    let existing_control = existing_obj.and_then(|obj| obj.get("Control")).cloned();
    let existing_zone = existing_obj.and_then(|obj| obj.get("Zone")).cloned();
    let existing_heat_source = existing_obj.and_then(|obj| obj.get("HeatSource")).cloned();

    let mut merged_system = template_system.clone();
    if let Some(merged_obj) = merged_system.as_object_mut() {
        if merged_obj.contains_key("Control") {
            if let Some(control) = existing_control {
                merged_obj.insert("Control".to_string(), control);
            }
        } else {
            merged_obj.remove("Control");
        }
        if merged_obj.contains_key("Zone") {
            if let Some(zone) = existing_zone {
                merged_obj.insert("Zone".to_string(), zone);
            }
        } else {
            merged_obj.remove("Zone");
        }
        // HeatSource is only retained for templates that explicitly use it.
        if merged_obj.contains_key("HeatSource") {
            if let Some(heat_source) = existing_heat_source {
                merged_obj.insert("HeatSource".to_string(), heat_source);
            }
        } else {
            merged_obj.remove("HeatSource");
        }
    }

    merged_system
}

pub fn merge_model(base_model: &mut Value, category: &str, params: &Value) -> Result<()> {
    match category {
        "orientation" => {
            // Params shape: { "orientation": { "add_degrees": number } }
            let add = params
                .get("orientation")
                .and_then(|o| o.get("add_degrees"))
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            if let Some(zones_obj) = base_model.get_mut("Zone").and_then(Value::as_object_mut) {
                for (_zone_name, zone_val) in zones_obj.iter_mut() {
                    if let Some(bes) = zone_val
                        .get_mut("BuildingElement")
                        .and_then(Value::as_object_mut)
                    {
                        for (_elem_key, elem_val) in bes.iter_mut() {
                            if let Some(ori) = elem_val.get_mut("orientation360") {
                                if let Some(v) = ori.as_f64() {
                                    let mut next = v + add;
                                    // Normalize into [0, 360)
                                    while next >= 360.0 {
                                        next -= 360.0;
                                    }
                                    while next < 0.0 {
                                        next += 360.0;
                                    }
                                    *ori = serde_json::json!(next);
                                }
                            }
                        }
                    }
                }
            }
        }
        "glazing" => {
            // Params shape: { "glazing": { "glazing_ratio": 0..1, "start_angle": f64, "end_angle": f64 } }
            let g = params
                .get("glazing")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let ratio = g
                .get("glazing_ratio")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let start = g.get("start_angle").and_then(Value::as_f64).unwrap_or(0.0);
            let end = g.get("end_angle").and_then(Value::as_f64).unwrap_or(360.0);

            // Helper: test orientation in [start, end) with wraparound support
            let in_range = |ori: f64| -> bool {
                let s = ((start % 360.0) + 360.0) % 360.0;
                let e = ((end % 360.0) + 360.0) % 360.0;
                let o = ((ori % 360.0) + 360.0) % 360.0;
                if (s - e).abs() < f64::EPSILON {
                    return true;
                } // full circle
                if s < e {
                    o >= s && o < e
                } else {
                    o >= s || o < e
                }
            };

            if let Some(zones_obj) = base_model.get_mut("Zone").and_then(Value::as_object_mut) {
                for (_zone_name, zone_val) in zones_obj.iter_mut() {
                    // Collect elements in range by type
                    let mut opaque_indices: Vec<(String, f64)> = Vec::new();
                    let mut transp_indices: Vec<(String, f64)> = Vec::new();
                    let mut total_o = 0.0f64;
                    let mut total_t = 0.0f64;
                    if let Some(bes) = zone_val.get("BuildingElement").and_then(|v| v.as_object()) {
                        for (elem_key, elem_val) in bes.iter() {
                            let ori = elem_val
                                .get("orientation360")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(-1.0);
                            if !in_range(ori) {
                                continue;
                            }
                            let width = elem_val.get("width").and_then(|v| v.as_f64());
                            let height = elem_val.get("height").and_then(|v| v.as_f64());
                            let area_field = elem_val.get("area").and_then(|v| v.as_f64());
                            let derived = area_field
                                .or_else(|| width.and_then(|w| height.map(|h| w * h)))
                                .unwrap_or(0.0);
                            let t = elem_val.get("type").and_then(|v| v.as_str()).unwrap_or("");
                            if t == "BuildingElementTransparent" {
                                // Include all windows (any pitch is fine for windows)
                                total_t += derived;
                                transp_indices.push((elem_key.clone(), derived));
                            } else if t == "BuildingElementOpaque" {
                                // Only include walls: pitch ~= 90 AND is_external_door != "TRUE"
                                // Exclude doors (pitch ~= 90 && is_external_door == "TRUE")
                                // Exclude roofs (pitch < 60)
                                // Banded on as_f64() (see pitch_is_wall_band): as_i64() returns
                                // None for any fractional pitch, which silently dropped
                                // fractional-pitch walls out of the glazing-ratio sweep entirely.
                                let pitch = elem_val.get("pitch").and_then(|v| v.as_f64());
                                // Current pipeline emits a boolean; legacy saved models used "TRUE".
                                let is_external_door = match elem_val.get("is_external_door") {
                                    Some(Value::Bool(b)) => *b,
                                    Some(Value::String(s)) => s == "TRUE",
                                    _ => false,
                                };

                                // Only include walls: pitch ~= 90 and not an external door
                                if pitch_is_wall_band(pitch) && !is_external_door {
                                    total_o += derived;
                                    opaque_indices.push((elem_key.clone(), derived));
                                }
                                // Skip doors (pitch == 90 && is_external_door == true)
                                // Skip roofs (pitch < 60)
                                // Skip other opaque elements
                            }
                        }
                    }

                    let total = total_o + total_t;
                    if total <= 0.0 {
                        continue;
                    }
                    // If either bucket is empty, skip (cannot achieve ratio without creating new elements)
                    if total_o <= 0.0 || total_t <= 0.0 {
                        continue;
                    }

                    let target_t = ratio.clamp(0.0, 1.0) * total;
                    let target_o = total - target_t;
                    let scale_t = if total_t > 0.0 {
                        target_t / total_t
                    } else {
                        1.0
                    };
                    let scale_o = if total_o > 0.0 {
                        target_o / total_o
                    } else {
                        1.0
                    };

                    if let Some(bes_mut) = zone_val
                        .get_mut("BuildingElement")
                        .and_then(Value::as_object_mut)
                    {
                        // Helper to scale width/height (preferred) or area as fallback
                        let scale_elem =
                            |elem: &mut Value,
                             scale: f64,
                             is_transparent: bool,
                             original_area: f64| {
                                if scale <= 0.0 {
                                    return;
                                }
                                let s = scale.sqrt();
                                if let Some(obj) = elem.as_object_mut() {
                                    let mut had_dims = false;
                                    if let Some(w) = obj.get_mut("width") {
                                        if let Some(wf) = w.as_f64() {
                                            *w = serde_json::json!(wf * s);
                                            had_dims = true;
                                        }
                                    }
                                    if let Some(h) = obj.get_mut("height") {
                                        if let Some(hf) = h.as_f64() {
                                            *h = serde_json::json!(hf * s);
                                            had_dims = true;
                                        }
                                    }
                                    if is_transparent {
                                        if let Some(fah) = obj.get_mut("free_area_height") {
                                            if let Some(fahf) = fah.as_f64() {
                                                *fah = serde_json::json!(fahf * s);
                                            }
                                        }
                                    }
                                    // Area handling: transparent uses width*height; opaque uses net area scaling
                                    if is_transparent {
                                        if had_dims {
                                            let nw = obj
                                                .get("width")
                                                .and_then(|v| v.as_f64())
                                                .unwrap_or(0.0);
                                            let nh = obj
                                                .get("height")
                                                .and_then(|v| v.as_f64())
                                                .unwrap_or(0.0);
                                            obj.insert(
                                                "area".to_string(),
                                                serde_json::json!(nw * nh),
                                            );
                                        } else {
                                            // No dims: scale area directly; fallback to original derived area
                                            let new_area = obj
                                                .get("area")
                                                .and_then(|v| v.as_f64())
                                                .map(|a| a * scale)
                                                .unwrap_or(original_area * scale);
                                            obj.insert(
                                                "area".to_string(),
                                                serde_json::json!(new_area),
                                            );
                                        }
                                    } else {
                                        // Opaque: area is net-of-openings; scale it directly regardless of dims
                                        let new_area = obj
                                            .get("area")
                                            .and_then(|v| v.as_f64())
                                            .map(|a| a * scale)
                                            .unwrap_or(original_area * scale);
                                        obj.insert("area".to_string(), serde_json::json!(new_area));
                                    }
                                }
                            };

                        for (k, a0) in transp_indices {
                            if let Some(elem_mut) = bes_mut.get_mut(&k) {
                                scale_elem(elem_mut, scale_t, true, a0);
                            }
                        }
                        for (k, a0) in opaque_indices {
                            if let Some(elem_mut) = bes_mut.get_mut(&k) {
                                scale_elem(elem_mut, scale_o, false, a0);
                            }
                        }
                    }
                }
            }
        }
        "location" => {
            if let (Some(infiltration), Some(param_infiltration)) = (
                base_model.get_mut("InfiltrationVentilation"),
                params.get("InfiltrationVentilation"),
            ) {
                let allowed_keys = ["shield_class", "terrain_class", "altitude"];
                if let (Some(inf_obj), Some(param_obj)) =
                    (infiltration.as_object_mut(), param_infiltration.as_object())
                {
                    for key in &allowed_keys {
                        if let Some(value) = param_obj.get(*key) {
                            inf_obj.insert((*key).to_string(), value.clone());
                        }
                    }
                }
            }
        }
        "airtightness" => {
            if let Some(param_infiltration) = params.get("InfiltrationVentilation") {
                if let Some(param_leaks) = param_infiltration.get("Leaks") {
                    if let Some(infiltration) = base_model.get_mut("InfiltrationVentilation") {
                        if let Some(leaks) = infiltration.get_mut("Leaks") {
                            update_object(leaks, param_leaks);
                        }
                    }
                }
            }
        }
        "space_heat_emitters" => {
            if let Some(param_systems) = params.get("SpaceHeatSystem").and_then(Value::as_object) {
                if let Some(base_systems) = base_model
                    .get_mut("SpaceHeatSystem")
                    .and_then(Value::as_object_mut)
                {
                    // Exact-name matches first (legacy snippets authored against a
                    // specific model's system names).
                    let mut matched_any = false;
                    for (key, param_system) in param_systems {
                        if let Some(base_system) = base_systems.get_mut(key) {
                            update_object(base_system, param_system);
                            matched_any = true;
                        }
                    }
                    // System names in current models are zone-derived ("Zone 1 radiator"),
                    // so name-keyed snippets can never match them. When nothing matched,
                    // apply the snippet's emitter configuration to every wet-distribution
                    // system, preserving each system's identity wiring — the same
                    // semantics as the zone-scoped path (process_zone_space_heat_emitters).
                    if !matched_any {
                        if let Some(template) = param_systems.values().next() {
                            for system in base_systems.values_mut() {
                                if system.get("type").and_then(Value::as_str)
                                    != Some("WetDistribution")
                                {
                                    continue;
                                }
                                let preserved: Vec<(String, Value)> =
                                    ["Control", "Zone", "HeatSource"]
                                        .iter()
                                        .filter_map(|k| {
                                            system.get(*k).map(|v| (k.to_string(), v.clone()))
                                        })
                                        .collect();
                                update_object(system, template);
                                if let Some(obj) = system.as_object_mut() {
                                    for (k, v) in preserved {
                                        obj.insert(k, v);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        "space_heat_systems" => {
            if let Some(param_systems) = params.get("SpaceHeatSystem").and_then(Value::as_object) {
                if let Some(template_system) = param_systems.values().next() {
                    if let Some(base_systems) = base_model
                        .get_mut("SpaceHeatSystem")
                        .and_then(Value::as_object_mut)
                    {
                        for system in base_systems.values_mut() {
                            *system =
                                build_space_heat_system_from_template(system, template_system);
                        }
                    } else if let Some(base_model_obj) = base_model.as_object_mut() {
                        base_model_obj.insert(
                            "SpaceHeatSystem".to_string(),
                            params
                                .get("SpaceHeatSystem")
                                .cloned()
                                .unwrap_or_else(|| serde_json::json!({})),
                        );
                    }
                }
            }
        }
        "controls" => {
            // Extract only hot water controls for global processing
            // Space heating controls (space_heating_setpoint, heating_timer) should be zone-specific only
            if let Some(param_control) = params.get("Control").and_then(Value::as_object) {
                if let Some(base_control) =
                    base_model.get_mut("Control").and_then(Value::as_object_mut)
                {
                    for (key, value) in param_control {
                        if key == "HotWaterMin" || key == "HotWaterMax" || key == "hot_water_timer"
                        {
                            base_control.insert(key.clone(), value.clone());
                        }
                        // Note: Space heating controls are handled in zone-specific processing
                    }
                }
            }

            // Update HotWaterSource control references
            if let Some(hot_water_source) = base_model.get_mut("HotWaterSource") {
                if let Some(hw_source_obj) = hot_water_source.as_object_mut() {
                    for hw_source in hw_source_obj.values_mut() {
                        if let Some(hw_source_obj) = hw_source.as_object_mut() {
                            if let Some(heat_source) = hw_source_obj.get_mut("HeatSource") {
                                if let Some(heat_source_obj) = heat_source.as_object_mut() {
                                    for heat_source in heat_source_obj.values_mut() {
                                        if let Some(heat_source_obj) = heat_source.as_object_mut() {
                                            heat_source_obj.insert(
                                                "Controlmin".to_string(),
                                                serde_json::Value::String(
                                                    "HotWaterMin".to_string(),
                                                ),
                                            );
                                            heat_source_obj.insert(
                                                "Controlmax".to_string(),
                                                serde_json::Value::String(
                                                    "HotWaterMax".to_string(),
                                                ),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        "hot_water_source" => {
            if let Some(param_hw_source) = params.get("HotWaterSource") {
                if let Some(base_model_obj) = base_model.as_object_mut() {
                    base_model_obj.insert("HotWaterSource".to_string(), param_hw_source.clone());
                }
            }
        }
        "tariff" => {
            if let Some(param_tariff) = params.get("Tariff") {
                if base_model.get("Tariff").is_none() {
                    if let Some(base_model_obj) = base_model.as_object_mut() {
                        base_model_obj.insert("Tariff".to_string(), serde_json::json!({}));
                    }
                }
                if let Some(base_tariff) = base_model.get_mut("Tariff") {
                    update_object(base_tariff, param_tariff);
                }
            }
        }
        "internal_gains" => {
            if let Some(param_gains) = params.get("InternalGains") {
                if let Some(base_gains) = base_model.get_mut("InternalGains") {
                    update_object(base_gains, param_gains);
                }
            }
        }
        "events" => {
            if let Some(param_events) = params.get("Events") {
                // Expand event patterns before merging
                let expanded_events = match expand_event_patterns(param_events) {
                    Ok(ev) => ev,
                    Err(e) => {
                        return Err(anyhow::anyhow!(format!(
                            "Failed to expand event patterns: {}",
                            e
                        )))
                    }
                };
                if base_model.get("Events").is_none() {
                    if let Some(base_model_obj) = base_model.as_object_mut() {
                        base_model_obj.insert("Events".to_string(), serde_json::json!({}));
                    }
                }
                if let Some(base_events) = base_model.get_mut("Events") {
                    // Preserve existing structure and merge new events
                    if let Some(base_events_obj) = base_events.as_object_mut() {
                        if let Some(param_events_obj) = expanded_events.as_object() {
                            for (event_type, event_data) in param_events_obj {
                                if let Some(existing_event) = base_events_obj.get_mut(event_type) {
                                    // Merge with existing event data, preserving structure
                                    if let Some(existing_event_obj) = existing_event.as_object_mut()
                                    {
                                        if let Some(new_event_obj) = event_data.as_object() {
                                            for (key, value) in new_event_obj {
                                                existing_event_obj
                                                    .insert(key.clone(), value.clone());
                                            }
                                        }
                                    }
                                } else {
                                    // Add new event type
                                    base_events_obj.insert(event_type.clone(), event_data.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
        "solar_systems" => {
            if let Some(param_solar) = params.get("OnSiteGeneration") {
                if base_model.get("OnSiteGeneration").is_none() {
                    if let Some(base_model_obj) = base_model.as_object_mut() {
                        base_model_obj
                            .insert("OnSiteGeneration".to_string(), serde_json::json!({}));
                    }
                }
                if let Some(base_solar) = base_model.get_mut("OnSiteGeneration") {
                    update_object(base_solar, param_solar);
                }
            }
        }
        "battery_systems" => {
            if let Some(param_energy_supply) = params.get("EnergySupply").and_then(Value::as_object)
            {
                if let Some(base_energy_supply) = base_model
                    .get_mut("EnergySupply")
                    .and_then(Value::as_object_mut)
                {
                    for (key, value) in param_energy_supply {
                        if let Some(base_supply) = base_energy_supply.get_mut(key) {
                            update_object(base_supply, value);
                        } else {
                            base_energy_supply.insert(key.clone(), value.clone());
                        }
                    }
                }
            }
        }
        "mechanical_ventilation_unit" => {
            if let Some(param_mech_vent_outer) = params
                .get("InfiltrationVentilation")
                .and_then(|iv| iv.get("MechanicalVentilation"))
                .and_then(Value::as_object)
            {
                if let Some(base_mech_vent_outer) = base_model
                    .get_mut("InfiltrationVentilation")
                    .and_then(|iv| iv.get_mut("MechanicalVentilation"))
                    .and_then(Value::as_object_mut)
                {
                    // Get the existing mechanical ventilation system name from the base model
                    if let Some(existing_system_name) = base_mech_vent_outer.keys().next().cloned()
                    {
                        // Extract the ductwork attribute from the existing system (if it exists)
                        let ductwork_attr = base_mech_vent_outer
                            .get(&existing_system_name)
                            .and_then(|sys| sys.get("ductwork"))
                            .cloned();

                        // Get the new attributes from the parameter file
                        if let Some(param_system) = param_mech_vent_outer.values().next() {
                            // Create the merged system with new attributes and preserved ductwork
                            let mut merged_system = param_system.clone();
                            if let Some(merged_system_obj) = merged_system.as_object_mut() {
                                let existing_system_obj = base_mech_vent_outer
                                    .get(&existing_system_name)
                                    .and_then(Value::as_object);
                                let target_vent_type =
                                    merged_system_obj.get("vent_type").and_then(Value::as_str);
                                let is_mvhr = target_vent_type == Some("MVHR");

                                let preserve_keys: &[&str] = if is_mvhr {
                                    &["ductwork", "position_intake", "position_exhaust"]
                                } else {
                                    &[
                                        "position_exhaust",
                                        "mid_height_air_flow_path",
                                        "orientation360",
                                        "pitch",
                                    ]
                                };

                                if let Some(existing_system_obj) = existing_system_obj {
                                    for key in preserve_keys {
                                        if !merged_system_obj.contains_key(*key) {
                                            if let Some(value) = existing_system_obj.get(*key) {
                                                merged_system_obj
                                                    .insert((*key).to_string(), value.clone());
                                            }
                                        }
                                    }
                                }

                                if is_mvhr {
                                    // Add ductwork back if it existed in the original system.
                                    if !merged_system_obj.contains_key("ductwork") {
                                        if let Some(ductwork) = ductwork_attr {
                                            merged_system_obj
                                                .insert("ductwork".to_string(), ductwork);
                                        }
                                    }
                                    for key in
                                        ["mid_height_air_flow_path", "orientation360", "pitch"]
                                    {
                                        merged_system_obj.remove(key);
                                    }
                                } else {
                                    merged_system_obj.remove("ductwork");
                                    merged_system_obj.remove("position_intake");
                                    let has_flat_position =
                                        ["mid_height_air_flow_path", "orientation360", "pitch"]
                                            .iter()
                                            .any(|key| merged_system_obj.contains_key(*key));
                                    if has_flat_position {
                                        merged_system_obj.remove("position_exhaust");
                                    }
                                }
                            }

                            // Update the merged_json with the new system attributes while preserving the system name
                            base_mech_vent_outer.clear();
                            base_mech_vent_outer.insert(existing_system_name, merged_system);
                        }
                    }
                }
            }
        }
        "simplified_fabric" => {
            // Use the same logic as fabric for simplified_fabric
            let fabric: HashMap<String, Value> = serde_json::from_value(params.clone())?;
            let fabric_map = fabric
                .into_iter()
                .filter_map(|(k, v)| v.as_object().map(|obj| (k, obj.clone())))
                .collect::<HashMap<_, _>>();

            if let Some(zones) = base_model.get_mut("Zone").and_then(Value::as_object_mut) {
                for zone in zones.values_mut() {
                    update_zone_fabric(zone, &fabric_map);
                }
            }
        }
        "heat_source_wet" => {
            // Replace entire HeatSourceWet object (like hot_water_source does)
            // This ensures unused heat sources are removed, preventing panics in output writing
            if let Some(param_heat_source) = params.get("HeatSourceWet") {
                if let Some(base_model_obj) = base_model.as_object_mut() {
                    base_model_obj.insert("HeatSourceWet".to_string(), param_heat_source.clone());
                }
            }
        }
        "space_cooling_systems" => {
            if let Some(param_cooling) = params.get("SpaceCoolSystem").and_then(Value::as_object) {
                if let Some(base_cooling) = base_model
                    .get_mut("SpaceCoolSystem")
                    .and_then(Value::as_object_mut)
                {
                    for (key, value) in param_cooling {
                        base_cooling.insert(key.clone(), value.clone());
                    }
                } else {
                    // If SpaceCoolSystem doesn't exist, create it
                    if let Some(base_model_obj) = base_model.as_object_mut() {
                        base_model_obj.insert(
                            "SpaceCoolSystem".to_string(),
                            params.get("SpaceCoolSystem").unwrap().clone(),
                        );
                    }
                }
            }
            // Note: Automatic zone assignment is handled in run_native_batch after all parameters are processed
        }
        "lighting" => {
            if let Some(param_lighting) = params.get("Lighting") {
                if let Some(base_lighting) = base_model.get_mut("Lighting") {
                    update_object(base_lighting, param_lighting);
                } else {
                    // If Lighting doesn't exist, create it
                    if let Some(base_model_obj) = base_model.as_object_mut() {
                        base_model_obj.insert("Lighting".to_string(), param_lighting.clone());
                    }
                }
            }
        }
        "compliance_settings" => {
            // Handle compliance settings - these can be at the root level
            if let Some(param_compliance) = params.as_object() {
                for (key, value) in param_compliance {
                    if let Some(base_model_obj) = base_model.as_object_mut() {
                        base_model_obj.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        "model_wrappers" => {
            // Model wrappers are used internally for compliance detection but should NOT be merged into output JSON
            // The shell script skips merging this parameter - we should do the same
            // This parameter is only used to detect compliance settings and trigger automatic zone control assignment
        }
        _ => {
            // For now, we do nothing for other categories
        }
    }
    // Either snippet may arrive last. Only repair references when there is one
    // unambiguous plant; independent cylinder heaters are never replaced.
    if matches!(category, "heat_source_wet" | "hot_water_source") {
        let plant = base_model
            .get("HeatSourceWet")
            .and_then(Value::as_object)
            .filter(|plants| plants.len() == 1)
            .and_then(|plants| plants.keys().next())
            .cloned();
        if let Some(plant) = plant {
            if category == "heat_source_wet" {
                let air_sink = base_model["HeatSourceWet"][&plant]["sink_type"] == "Air";
                if let Some(systems) = base_model
                    .get_mut("SpaceHeatSystem")
                    .and_then(Value::as_object_mut)
                {
                    for system in systems.values_mut() {
                        // WarmAir may follow an air-to-air replacement, never water plant.
                        if system.get("type").and_then(Value::as_str) == Some("WarmAir")
                            && !air_sink
                        {
                            continue;
                        }
                        if let Some(source) =
                            system.get_mut("HeatSource").and_then(Value::as_object_mut)
                        {
                            source.insert("name".into(), Value::String(plant.clone()));
                        }
                    }
                }
            }
            if let Some(cylinders) = base_model
                .get_mut("HotWaterSource")
                .and_then(Value::as_object_mut)
            {
                for cylinder in cylinders.values_mut() {
                    match cylinder.get("type").and_then(Value::as_str) {
                        Some("CombiBoiler" | "HIU" | "HeatBattery") => {
                            cylinder["HeatSourceWet"] = Value::String(plant.clone());
                        }
                        Some("StorageTank" | "SmartHotWaterTank") => {
                            if let Some(sources) = cylinder
                                .get_mut("HeatSource")
                                .and_then(Value::as_object_mut)
                            {
                                for source in sources.values_mut() {
                                    if source.get("type").and_then(Value::as_str)
                                        == Some("HeatSourceWet")
                                    {
                                        source["name"] = Value::String(plant.clone());
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    Ok(())
}

/// Read the CLI flag from a model-wrapper parameter file.

pub fn detect_and_apply_compliance_settings(
    model: &mut Value,
    processed_params: &[String],
) -> Result<()> {
    let mut compliance_detected = false;

    // Check if compliance_settings parameter was processed
    if processed_params.contains(&"compliance_settings".to_string()) {
        compliance_detected = true;
    }

    // Check if model_wrappers contains fhs_compliance
    if !compliance_detected && processed_params.contains(&"model_wrappers".to_string()) {
        // This would need to check the actual value, but for now we'll assume it's fhs_compliance
        compliance_detected = true;
    }

    if compliance_detected {
        // Fix opaque building elements for compliance - add is_external_door field if missing
        if let Some(zones) = model.get_mut("Zone") {
            if let Some(zones_obj) = zones.as_object_mut() {
                for zone in zones_obj.values_mut() {
                    if let Some(zone_obj) = zone.as_object_mut() {
                        if let Some(building_elements) = zone_obj.get_mut("BuildingElement") {
                            if let Some(bes) = building_elements.as_object_mut() {
                                for be in bes.values_mut() {
                                    if let Some(be_obj) = be.as_object_mut() {
                                        if let Some(be_type) = be_obj.get("type") {
                                            if be_type.as_str() == Some("BuildingElementOpaque")
                                                && !be_obj.contains_key("is_external_door")
                                            {
                                                be_obj.insert(
                                                    "is_external_door".to_string(),
                                                    serde_json::Value::Bool(false),
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn auto_assign_cooling_systems(model: &mut Value) -> Result<()> {
    // Get cooling system names from the model
    let cooling_system_names = if let Some(space_cool_systems) = model.get("SpaceCoolSystem") {
        if let Some(systems_obj) = space_cool_systems.as_object() {
            let mut names: Vec<_> = systems_obj.keys().cloned().collect();
            names.sort();
            names
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    if !cooling_system_names.is_empty() {
        // Get zone names from the model
        let zone_names = if let Some(zones) = model.get("Zone") {
            if let Some(zones_obj) = zones.as_object() {
                let mut zone_names: Vec<_> = zones_obj.keys().cloned().collect();
                zone_names.sort();
                zone_names
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        // Assign cooling systems to zones in order
        for (idx, zone_name) in zone_names.iter().enumerate() {
            if idx < cooling_system_names.len() {
                let cooling_system = &cooling_system_names[idx];
                if let Some(zones) = model.get_mut("Zone") {
                    if let Some(zones_obj) = zones.as_object_mut() {
                        if let Some(zone) = zones_obj.get_mut(zone_name) {
                            if let Some(zone_obj) = zone.as_object_mut() {
                                zone_obj.insert(
                                    "SpaceCoolSystem".to_string(),
                                    serde_json::Value::String(cooling_system.to_string()),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Expands compact event patterns in the Events section of a model.
/// Returns a Result with the expanded events or an error message.
pub fn expand_event_patterns(events: &serde_json::Value) -> Result<serde_json::Value, String> {
    use serde_json::{json, Value};

    // Helper to check if a value is a pattern (has 'repeat' field)
    fn is_pattern(obj: &serde_json::Map<String, Value>) -> bool {
        obj.contains_key("repeat")
    }

    // Helper to expand a single pattern object
    fn expand_pattern(obj: &serde_json::Map<String, Value>) -> Result<Vec<Value>, String> {
        // Required fields
        let repeat = obj
            .get("repeat")
            .and_then(Value::as_i64)
            .ok_or("Missing or invalid 'repeat'")?;
        let start = obj
            .get("start")
            .and_then(Value::as_f64)
            .ok_or("Missing or invalid 'start'")?;
        let duration = obj
            .get("duration")
            .and_then(Value::as_f64)
            .ok_or("Missing or invalid 'duration'")?;
        let temperature = obj
            .get("temperature")
            .and_then(Value::as_f64)
            .ok_or("Missing or invalid 'temperature'")?;
        let volume = obj
            .get("volume")
            .and_then(Value::as_f64)
            .ok_or("Missing or invalid 'volume'")?;
        let start_increment = obj
            .get("start_increment")
            .and_then(Value::as_f64)
            .unwrap_or(24.0);

        if repeat <= 0 {
            return Err("'repeat' must be positive".to_string());
        }
        if start_increment <= 0.0 {
            return Err("'start_increment' must be positive".to_string());
        }

        let mut events = Vec::new();
        for i in 0..repeat {
            let mut event = serde_json::Map::new();
            event.insert(
                "start".to_string(),
                json!(start + (i as f64) * start_increment),
            );
            event.insert("duration".to_string(), json!(duration));
            event.insert("temperature".to_string(), json!(temperature));
            event.insert("volume".to_string(), json!(volume));
            events.push(Value::Object(event));
        }
        Ok(events)
    }

    // Recursively process the events structure
    fn process_node(node: &Value) -> Result<Value, String> {
        match node {
            Value::Object(map) => {
                let mut out = serde_json::Map::new();
                for (k, v) in map.iter() {
                    out.insert(k.clone(), process_node(v)?);
                }
                Ok(Value::Object(out))
            }
            Value::Array(arr) => {
                let mut expanded = Vec::new();
                for item in arr {
                    match item {
                        Value::Object(obj) if is_pattern(obj) => {
                            let events = expand_pattern(obj)?;
                            expanded.extend(events);
                        }
                        Value::Object(_) | Value::Array(_) => {
                            // Nested patterns/arrays not supported
                            if let Value::Object(obj) = item {
                                if is_pattern(obj) {
                                    // already handled above
                                } else {
                                    expanded.push(item.clone());
                                }
                            } else {
                                return Err("Nested arrays/patterns not supported".to_string());
                            }
                        }
                        _ => expanded.push(item.clone()),
                    }
                }
                Ok(Value::Array(expanded))
            }
            _ => Ok(node.clone()),
        }
    }

    process_node(events)
}

/// Explicit target-aware preparation seam for browser, MCP and native callers.
/// The host resolves and verifies the immutable bundle before supplying content.
pub fn prepare_target_scenario(
    request: TransformRequest,
    snippets: &[(String, Value)],
) -> Result<vulcan_model_transform::TransformOutput, PipelineError> {
    let schema: Value = serde_json::from_str(&request.schema_json)
        .map_err(|error| PipelineError::Schema(error.to_string()))?;
    let conversion_profile = request.conversion_profile;
    let output = transform_geometry_csv(request)?;
    let mut output = prepare_effective_scenario(output, &schema, snippets)?;
    if !snippets.is_empty() {
        // A snippet can restore the converter's exact empty 36-sector sky.
        // Unsupported authored fields have already been rejected by finalization.
        vulcan_model_transform::map_target_unobstructed_shading(
            &mut output.model,
            conversion_profile,
        );
        // Recompute source-verified wrapper readiness after snippets, including
        // corrections to scalar bridges or a missing main hot-water source.
        output.validation.errors.retain(|error| {
            !matches!(
                error.code.as_str(),
                "E_A8_THERMAL_BRIDGING" | "E_PYTHON_FHS_HOT_WATER_SOURCE"
            )
        });
        output
            .validation
            .errors
            .extend(vulcan_model_transform::validate_target_input(
                &output.model,
                conversion_profile,
                &output.version_metadata,
            ));
        output.validation.is_valid = output.validation.errors.is_empty();
    }
    Ok(output)
}

pub fn prepare_effective_scenario(
    mut output: vulcan_model_transform::TransformOutput,
    schema: &Value,
    snippets: &[(String, Value)],
) -> Result<vulcan_model_transform::TransformOutput, PipelineError> {
    // No scenario changes: reuse exactly the prepared standalone input.
    if snippets.is_empty() {
        return Ok(output);
    }
    let mut candidate = output.model.clone();
    let mut warnings = Vec::new();
    for (category, snippet) in snippets {
        // No FHS target schema accepts what these merges write, or the wrapper
        // overwrites it, so they would fail obscurely or silently change nothing.
        let unsupported = match category.as_str() {
            "lighting" => Some("FHS takes lighting from each zone's bulbs; edit them in the model"),
            "tariff" => Some("FHS input has no tariff"),
            "internal_gains" => Some("FHS derives internal gains itself"),
            "controls" => Some("FHS sets heating and hot-water controls itself"),
            "glazing" => Some("FHS windows take width and height, not a scaled area; edit the windows in the model"),
            _ => None,
        };
        if let Some(reason) = unsupported {
            return Err(PipelineError::InvalidRequest(format!(
                "Scenario category '{category}' is not supported by HEM FHS targets: {reason}."
            )));
        }
        if !matches!(
            category.as_str(),
            "orientation"
                | "location"
                | "airtightness"
                | "space_heat_emitters"
                | "space_heat_systems"
                | "hot_water_source"
                | "events"
                | "solar_systems"
                | "battery_systems"
                | "mechanical_ventilation_unit"
                | "simplified_fabric"
                | "heat_source_wet"
                | "space_cooling_systems"
                | "compliance_settings"
        ) {
            return Err(PipelineError::InvalidRequest(format!("Scenario category '{category}' is not supported by target preparation; execution flags and weather must be resolved by the host.")));
        }
        let before = candidate.clone();
        merge_model(&mut candidate, category, snippet)
            .map_err(|e| PipelineError::InvalidRequest(e.to_string()))?;
        reject_structural_change(&before, &candidate, "")?;
        if category == "simplified_fabric" {
            if let Some(whole) = snippet.pointer("/party_wall/u_value_whole_wall") {
                if declares_property(schema, "u_value_whole_wall") {
                    set_party_wall_whole_u(&mut candidate, whole);
                }
            } else {
                let version = &output.version_metadata.hem_core_version;
                for (zone, name) in stale_whole_wall_u(&before, &candidate) {
                    let segments = [
                        "Zone",
                        &zone,
                        "BuildingElement",
                        &name,
                        "u_value_whole_wall",
                    ];
                    warnings.push(vulcan_model_transform::finalization::SchemaOmission {
                    code: "W_TARGET_INPUT".into(),
                    path: segments
                        .iter()
                        .map(|s| format!("/{}", s.replace('~', "~0").replace('/', "~1")))
                        .collect(),
                    message: format!("HEM {version}: the fabric package changes party wall '{name}' but not its whole-wall U-value, which is reported in the compliance report only and cannot be derived from the construction resistance. Add u_value_whole_wall to the package's party_wall section to update it."),
                });
                }
            }
        }
    }
    let processed = snippets
        .iter()
        .map(|(category, _)| category.clone())
        .collect::<Vec<_>>();
    detect_and_apply_compliance_settings(&mut candidate, &processed)
        .map_err(|e| PipelineError::InvalidRequest(e.to_string()))?;
    if processed
        .iter()
        .any(|category| category == "space_cooling_systems")
    {
        auto_assign_cooling_systems(&mut candidate)
            .map_err(|e| PipelineError::InvalidRequest(e.to_string()))?;
    }
    if candidate == output.model {
        return Ok(output);
    }
    let finalized = vulcan_model_transform::finalization::finalize_model(&candidate, schema)
        .map_err(PipelineError::Schema)?;
    if !finalized.omissions.is_empty() {
        let paths = finalized
            .omissions
            .iter()
            .map(|omission| omission.path.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(PipelineError::InvalidRequest(format!(
            "Scenario fields are incompatible with the selected target schema: {paths}. Update the snippet for this target; no partial scenario was applied."
        )));
    }
    // Merge/schema errors are recomputed; source parsing/mapping errors remain
    // actionable unless a snippet authored the very value the source lacked.
    let mut errors = output
        .validation
        .errors
        .into_iter()
        .filter(|e| e.keyword.is_none())
        .filter(|e| {
            e.code != "E_TARGET_INPUT"
                || output.model.pointer(&e.path) == finalized.model.pointer(&e.path)
        })
        .collect::<Vec<_>>();
    output.model = finalized.model;
    output.schema_omissions.extend(warnings);
    errors.extend(finalized.validation.errors);
    output.validation = ValidationResult {
        is_valid: errors.is_empty(),
        errors,
    };
    Ok(output)
}

/// (zone, element) of each party wall carrying a whole-wall U-value whose
/// construction or cavity the merge changed. Whole-wall U is an independent
/// declared value (a9: compliance report only), so the old one may now
/// describe a different wall.
fn stale_whole_wall_u(before: &Value, after: &Value) -> Vec<(String, String)> {
    const DESCRIBES: [&str; 4] = [
        "thermal_resistance_construction",
        "thermal_resistance_cavity",
        "party_wall_cavity_type",
        "party_wall_lining_type",
    ];
    let mut stale = Vec::new();
    for (zone, zone_before) in before
        .get("Zone")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let elements = zone_before
            .get("BuildingElement")
            .and_then(Value::as_object);
        for (name, element) in elements.into_iter().flatten() {
            let merged = &after["Zone"][zone]["BuildingElement"][name];
            if element.get("u_value_whole_wall").is_some()
                && DESCRIBES
                    .iter()
                    .any(|k| !same_value(element.get(k), merged.get(k)))
            {
                stale.push((zone.clone(), name.clone()));
            }
        }
    }
    stale
}

/// Restating 3 as 3.0 is no change.
fn same_value(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a.and_then(Value::as_f64), b.and_then(Value::as_f64)) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// Whether any object in the target schema declares `key` as a property.
fn declares_property(node: &Value, key: &str) -> bool {
    match node {
        Value::Object(map) => {
            map.get("properties")
                .and_then(Value::as_object)
                .is_some_and(|p| p.contains_key(key))
                || map.values().any(|v| declares_property(v, key))
        }
        Value::Array(items) => items.iter().any(|v| declares_property(v, key)),
        _ => false,
    }
}

fn set_party_wall_whole_u(model: &mut Value, whole: &Value) {
    let zones = model.get_mut("Zone").and_then(Value::as_object_mut);
    for zone in zones.into_iter().flat_map(|z| z.values_mut()) {
        let elements = zone
            .get_mut("BuildingElement")
            .and_then(Value::as_object_mut);
        for element in elements.into_iter().flat_map(|e| e.values_mut()) {
            if element.get("type").and_then(Value::as_str) == Some("BuildingElementPartyWall") {
                element["u_value_whole_wall"] = whole.clone();
            }
        }
    }
}

pub fn reject_structural_change(
    before: &Value,
    after: &Value,
    path: &str,
) -> Result<(), PipelineError> {
    match (before, after) {
        (Value::Object(left), Value::Object(right)) => {
            for discriminator in ["type", "vent_type", "floor_type", "cross_section_shape"] {
                if left.contains_key(discriminator)
                    && right.get(discriminator) != left.get(discriminator)
                {
                    return Err(PipelineError::InvalidRequest(format!("Scenario changes {path}/{discriminator}; edit the source model or supply a separately qualified complete replacement. Existing target defaults cannot be reused for this structural change.")));
                }
            }
            for (key, value) in left {
                if let Some(next) = right.get(key) {
                    reject_structural_change(value, next, &format!("{path}/{key}"))?;
                }
            }
        }
        (Value::Array(left), Value::Array(right)) => {
            for (index, (value, next)) in left.iter().zip(right).enumerate() {
                reject_structural_change(value, next, &format!("{path}/{index}"))?;
            }
        }
        _ => {}
    }
    Ok(())
}
