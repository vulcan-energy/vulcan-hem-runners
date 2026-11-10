// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Every registered target x every shipped base model x every shipped snippet.
//!
//! A snippet either prepares to a valid model that differs from the base in the
//! part its category claims, or it is listed in EXPECTED with the reason it
//! blocks, leaves the model as it is, or is refused by the engine. Preparation
//! only: no engine runs.

use hem_target_core::prepare_target_scenario;
use serde_json::Value;
use std::path::{Path, PathBuf};
use vulcan_model_transform::{ModelProfile, TransformRequest, VersionMetadata};

/// Snippets that must NOT change the model, keyed `category/name` or `category/*`.
/// `Blocked` holds a substring of the preparation error or validation message.
/// `SameAs(model, why)`: that shipped model already is what the snippet states;
/// every value the snippet sets must be in it (an empty one: absent). On any
/// other model the snippet must change something.
/// `EngineRefuses(model, message)`: preparation is valid, and the a7 engine's
/// `actual` run of that model stops with this message because the scenario is
/// not allowed there. Recorded from a manual engine run; this test runs no engine.
#[allow(dead_code)] // The reason strings are documentation.
enum Expect {
    Blocked(&'static str),
    SameAs(&'static str, &'static str),
    EngineRefuses(&'static str, &'static str),
}
use Expect::*;

const NOT_FHS: &str = "is not supported by HEM FHS targets";
const HOST_ONLY: &str = "is not supported by target preparation";
const SAMPLE: &str = "example_semi_detached";
const EXPECTED: &[(&str, Expect)] = &[
    ("additional_outputs/*", Blocked(HOST_ONLY)),
    ("sap_xml/*", Blocked(HOST_ONLY)),
    ("controls/*", Blocked(NOT_FHS)),
    ("events/*", Blocked(NOT_FHS)),
    ("glazing/*", Blocked(NOT_FHS)),
    ("internal_gains/*", Blocked(NOT_FHS)),
    ("lighting/*", Blocked(NOT_FHS)),
    ("space_heat_systems/*", Blocked(NOT_FHS)),
    (
        "battery_systems/none",
        SameAs(SAMPLE, "the sample has no battery"),
    ),
    ("solar_systems/none", SameAs(SAMPLE, "the sample has no PV")),
    (
        "compliance_settings/compliance_enabled",
        SameAs(SAMPLE, "the sample already sets these switches"),
    ),
    // Four bedrooms need 37 l/s under Part F; the sample's MEV gives 33.3 l/s.
    (
        "compliance_settings/compliance_4bed_3bath",
        EngineRefuses(
            SAMPLE,
            "FHS input validation failed, see part F of the building regulations.\nFailure(s):\nDwelling lacks sufficient continuous mechanical extract rate.",
        ),
    ),
    (
        "heat_source_wet/hp_large",
        SameAs(SAMPLE, "the sample's heat pump is this one"),
    ),
    (
        "space_cooling_systems/no_cooling",
        SameAs(SAMPLE, "the sample has no cooling"),
    ),
    // The sample's cylinder and radiators are wet; this heat pump heats air.
    ("heat_source_wet/a2a_heat_pump", Blocked("sink_type Air")),
    // Only for a model that already has a combi boiler; the sample's cylinder is a different type.
    (
        "hot_water_source/combi_boiler",
        Blocked("The hot_water_source scenario changes /HotWaterSource/hw cylinder/type, which a scenario cannot change"),
    ),
    // These cylinders name a plant the sample does not have.
    (
        "hot_water_source/gas_boiler_cylinder",
        Blocked("wet heat source 'boiler'"),
    ),
    (
        "hot_water_source/heat_battery",
        Blocked("wet heat source 'heat_battery_source'"),
    ),
];

/// Every value `snippet` sets is already at the same place in `model`.
fn already(model: Option<&Value>, snippet: &Value) -> bool {
    match snippet {
        Value::Object(map) if !map.is_empty() => map
            .iter()
            .all(|(k, v)| already(model.and_then(|m| m.get(k)), v)),
        other => same(model, Some(other)),
    }
}

/// Equal as the engine reads it: 5 is 5.0, and an absent section is an empty one.
fn same(a: Option<&Value>, b: Option<&Value>) -> bool {
    let empty = |v: Option<&Value>| {
        v.is_none_or(|v| v.is_null() || v.as_object().is_some_and(|o| o.is_empty()))
    };
    match (a, b) {
        _ if empty(a) && empty(b) => true,
        (Some(Value::Number(x)), Some(Value::Number(y))) => x.as_f64() == y.as_f64(),
        (Some(Value::Object(x)), Some(Value::Object(y))) => {
            x.keys().chain(y.keys()).all(|k| same(x.get(k), y.get(k)))
        }
        (Some(Value::Array(x)), Some(Value::Array(y))) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(Some(p), Some(q)))
        }
        _ => a == b,
    }
}

/// Categories the host resolves itself; they never reach target preparation.
const HOST: &[&str] = &[
    "base_json",
    "weather_files",
    "model_wrappers",
    "heating_profiles",
    "element_presets",
];

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `field: 'value'` from one registry entry line.
fn field<'a>(line: &'a str, name: &str) -> &'a str {
    let start = line
        .find(&format!("{name}: '"))
        .unwrap_or_else(|| panic!("registry entry lacks {name}: {line}"))
        + name.len()
        + 3;
    &line[start..start + line[start..].find('\'').unwrap()]
}

struct Target {
    id: String,
    request: TransformRequest,
    /// The host drops a hot-water snippet's cylinder init_temp where the schema has none.
    omits_init_temp: bool,
}

/// The registry is `HEM_TARGETS` in hem-target-runtime; each entry is one
/// `{ id: ...` line followed by the line holding its conversion profile.
fn targets() -> Vec<Target> {
    let registry = read(&repo().join("hem-target-runtime/src/hem/targets.ts"));
    let body = &registry[registry.find("export const HEM_TARGETS").unwrap()..];
    let body = &body[..body.find("] as const").unwrap()];
    let lines: Vec<&str> = body.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !line.trim_start().starts_with("{ id: '") {
            continue;
        }
        let entry = format!("{line}{}", lines[i + 1]);
        let id = field(&entry, "id").to_string();
        let fixture = repo().join(format!("fixtures/hem_target_schemas/{id}.schema.json"));
        // The a7 schema is the pinned upstream wrapper checkout itself.
        let schema_path = if fixture.exists() {
            fixture
        } else {
            let upstream = std::process::Command::new("git")
                .args(["-C", "hem_fhs_upstream", "rev-parse", "HEAD"])
                .current_dir(repo())
                .output()
                .unwrap();
            assert_eq!(
                String::from_utf8_lossy(&upstream.stdout).trim(),
                field(&entry, "wrapperCommit"),
                "{id}: add fixtures/hem_target_schemas/{id}.schema.json"
            );
            repo().join("hem_fhs_upstream/schema/input_fhs.schema.json")
        };
        // The raw pinned schema: the a8/a9 ones wrongly require
        // is_adjacent_space_within_dwelling on unconditioned elements, which only the
        // TypeScript hosts correct (targetSchemaCorrections.ts). A shipped model with an
        // unheated element would fail a8/a9 here until the pin carries the upstream fix.
        let schema_json = read(&schema_path);
        let schema: Value = serde_json::from_str(&schema_json).unwrap();
        let tank = &schema["$defs"]["Tank"];
        let version = field(&entry, "engineVersion").to_string();
        out.push(Target {
            omits_init_temp: tank.is_object() && tank["properties"].get("init_temp").is_none(),
            request: TransformRequest {
                csv: String::new(),
                schema_json,
                defaults_json: read(&repo().join("community/data/defaults/defaults_template.json")),
                profile: ModelProfile::Fhs,
                conversion_profile: serde_json::from_value(Value::from(field(
                    &entry,
                    "conversionProfile",
                )))
                .unwrap(),
                version_metadata: VersionMetadata {
                    hem_core_version: version,
                    fhs_wrapper_version: Some(field(&entry, "wrapperVersion").into()),
                },
            },
            id,
        });
    }
    assert!(out.len() >= 3, "registry parse found {} targets", out.len());
    out
}

/// The model sections a category's merge writes.
fn claimed(category: &str, snippet: &Value) -> Vec<String> {
    match category {
        "simplified_fabric" | "orientation" => vec!["/Zone".into()],
        _ => snippet
            .as_object()
            .unwrap()
            .keys()
            .map(|k| format!("/{k}"))
            .collect(),
    }
}

fn with_target(csv: &str, id: &str) -> String {
    let line = format!("TargetBundleId,{id}");
    match csv.lines().find(|l| l.starts_with("TargetBundleId,")) {
        Some(existing) => csv.replace(existing, &line),
        None => csv.replacen("Metadata", &format!("Metadata\n{line}"), 1),
    }
}

#[test]
fn every_shipped_snippet_does_what_it_says_on_every_target() {
    let library = repo().join("resources/sample_project/batch_parameters");
    let manifest: Value = serde_json::from_str(&read(&library.join("manifest.json"))).unwrap();
    let models_dir = repo().join("resources/sample_project/input/base_models");
    let mut models: Vec<PathBuf> = std::fs::read_dir(&models_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "csv"))
        .collect();
    models.sort();
    assert!(!models.is_empty());
    let mut failures = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for target in targets() {
        for model in &models {
            let model_name = model.file_stem().unwrap().to_string_lossy().to_string();
            let request = TransformRequest {
                csv: with_target(&read(model), &target.id),
                ..target.request.clone()
            };
            let base = prepare_target_scenario(request.clone(), &[]).unwrap();
            if !base.validation.is_valid {
                failures.push(format!(
                    "{} {model_name}: base model invalid: {:?}",
                    target.id, base.validation.errors
                ));
                continue;
            }
            for (category, files) in manifest.as_object().unwrap() {
                if HOST.contains(&category.as_str()) {
                    continue;
                }
                for file in files.as_array().unwrap() {
                    let file = file.as_str().unwrap();
                    let key = format!("{category}/{}", file.trim_end_matches(".json"));
                    seen.insert(key.clone());
                    seen.insert(format!("{category}/*"));
                    let mut snippet: Value =
                        serde_json::from_str(&read(&library.join(category).join(file))).unwrap();
                    if category == "hot_water_source" && target.omits_init_temp {
                        if let Some(c) = snippet.pointer_mut("/HotWaterSource/hw cylinder") {
                            c.as_object_mut().unwrap().remove("init_temp");
                        }
                    }
                    let wildcard = format!("{category}/*");
                    let expected = EXPECTED
                        .iter()
                        .find(|(k, _)| *k == key || *k == wildcard)
                        .map(|(_, e)| e);
                    let at = format!("{} {model_name} {key}", target.id);
                    let result = prepare_target_scenario(
                        request.clone(),
                        &[(category.clone(), snippet.clone())],
                    );
                    let outcome = match &result {
                        Err(e) => format!("blocked: {e}"),
                        Ok(o) if !o.validation.is_valid => format!(
                            "invalid: {}",
                            o.validation
                                .errors
                                .iter()
                                .map(|e| format!("{} {} {}", e.code, e.path, e.message))
                                .collect::<Vec<_>>()
                                .join(" | ")
                        ),
                        Ok(o) => {
                            let changed = claimed(category, &snippet)
                                .iter()
                                .any(|p| !same(o.model.pointer(p), base.model.pointer(p)));
                            if changed {
                                "changed".into()
                            } else {
                                "unchanged".into()
                            }
                        }
                    };
                    let ok = match (expected, outcome.as_str()) {
                        (None | Some(EngineRefuses(..)), "changed") => true,
                        (Some(SameAs(m, _)), "changed") => *m != model_name,
                        (Some(SameAs(m, _)), "unchanged") => {
                            *m == model_name
                                && already(Some(&result.as_ref().unwrap().model), &snippet)
                        }
                        (Some(Blocked(why)), o) => {
                            (o.starts_with("blocked") || o.starts_with("invalid"))
                                && o.contains(why)
                        }
                        _ => false,
                    };
                    if !ok {
                        failures.push(format!(
                            "{at}: {}",
                            outcome.chars().take(400).collect::<String>()
                        ));
                    }
                }
            }
        }
    }
    for (key, _) in EXPECTED {
        if !seen.contains(*key) {
            failures.push(format!("EXPECTED names a snippet the library lacks: {key}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{}\n{} failures",
        failures.join("\n"),
        failures.len()
    );
}
