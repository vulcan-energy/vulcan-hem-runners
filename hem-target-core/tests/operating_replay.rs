//! Explicit manual annual qualification; no customer data is committed.
use hem_target_core::{operating::run_target_stage_inner, runtime::run_target_input_inner};
use serde_json::Value;

#[test]
#[ignore = "manual annual engine qualification"]
fn annual_fhs_export_replays_through_core() {
    let source = std::env::var("HEM_REPLAY_INPUT").unwrap_or_else(|_| "../hem_fhs_upstream/examples/input/future_homes_standard/demo_FHS_hp_with_advancedstart.json".into());
    let input = std::fs::read_to_string(source).unwrap();
    let baseline = run_target_input_inner(&input, "model", r#"["actual"]"#, None).unwrap();
    let export = run_target_stage_inner(&input, "preprocess", None).unwrap();
    let files = export["files"].as_object().unwrap();
    assert_eq!(files.len(), 1);
    let exported = files.values().next().unwrap().as_str().unwrap();
    let replay = run_target_stage_inner(exported, "core", None).unwrap();
    let mut numeric_cells = 0;
    let mut max_delta = 0.0_f64;
    for suffix in ["__results.csv", "__results_static.csv"] {
        let find = |result: &Value| {
            result["files"]
                .as_object()
                .unwrap()
                .iter()
                .find(|(name, _)| name.ends_with(suffix))
                .unwrap()
                .1
                .as_str()
                .unwrap()
                .to_owned()
        };
        let original = find(&baseline);
        let repeated = find(&replay);
        let a: Vec<_> = original.lines().collect();
        let b: Vec<_> = repeated.lines().collect();
        assert_eq!(a.len(), b.len(), "{suffix} rows");
        for (line, (a, b)) in a.iter().zip(&b).enumerate() {
            let ac: Vec<_> = a.split(',').collect();
            let bc: Vec<_> = b.split(',').collect();
            assert_eq!(ac.len(), bc.len(), "{suffix} line {line}");
            for (a, b) in ac.iter().zip(bc) {
                match (a.parse::<f64>(), b.parse::<f64>()) {
                    (Ok(x), Ok(y)) if x.is_finite() && y.is_finite() => {
                        let delta = (x - y).abs();
                        max_delta = max_delta.max(delta);
                        numeric_cells += 1;
                        assert!(delta <= 1e-8, "{suffix} line {line}: {x} != {y}");
                    }
                    _ => assert_eq!(*a, b, "{suffix} line {line}"),
                }
            }
        }
    }
    assert!(numeric_cells > 100_000);
    println!(
        "Annual FHS/core replay: {numeric_cells} numeric cells, max absolute delta {max_delta}"
    );
    if let Ok(path) = std::env::var("HEM_REPLAY_OUTPUT") {
        std::fs::create_dir_all(&path).unwrap();
        for (name, result) in [
            ("baseline", baseline),
            ("export", export),
            ("replay", replay),
        ] {
            std::fs::write(
                format!("{path}/{name}.json"),
                serde_json::to_string(&result).unwrap(),
            )
            .unwrap();
        }
    }
}

#[test]
#[ignore = "manual changed-profile annual engine qualification"]
fn changed_heating_profile_runs_core() {
    let path = std::env::var("HEM_ADJUSTED_INPUT")
        .expect("Set HEM_ADJUSTED_INPUT to the shared transform output");
    let input = std::fs::read_to_string(path).unwrap();
    let results = run_target_stage_inner(&input, "core", None).unwrap();
    let files = results["files"].as_object().unwrap();
    let csv = files
        .iter()
        .find(|(name, _)| name.ends_with("__results.csv"))
        .unwrap()
        .1
        .as_str()
        .unwrap();
    assert!(csv.lines().count() > 17520);
    assert!(!csv.contains(",NaN") && !csv.contains(",inf"));
    let output =
        std::env::var("HEM_REPLAY_OUTPUT").expect("Set HEM_REPLAY_OUTPUT for retained results");
    std::fs::write(output, serde_json::to_string(&results).unwrap()).unwrap();
    println!(
        "Changed annual controls: {} timestep rows; {} output files",
        csv.lines().count(),
        files.len()
    );
}
