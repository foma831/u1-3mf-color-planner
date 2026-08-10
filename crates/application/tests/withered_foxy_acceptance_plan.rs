use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};
use u1_application::{PreliminaryPlanOptions, analyze_and_plan_with_options};
use u1_planner::PrinterLoadout;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AcceptancePlan {
    source: FileReference,
    planning_options: FileReference,
    expected_plan: ExpectedPlan,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileReference {
    path: String,
    sha256: String,
    #[serde(default)]
    byte_size: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExpectedPlan {
    hard_error_count: usize,
    job_count: usize,
    target_plate_count: usize,
    batch_count: usize,
    t4_swap_count: u32,
    a1_spool_change_count: u32,
    strategy_counts: BTreeMap<String, usize>,
    warning_counts: BTreeMap<String, usize>,
    batches: Vec<ExpectedBatch>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExpectedBatch {
    id: String,
    target_plate_ids: Vec<String>,
    #[serde(rename = "T4")]
    t4: Option<String>,
    swap_before: bool,
}

fn workspace_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn sha256(path: &Path) -> String {
    let mut file = File::open(path).unwrap();
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    format!("{:x}", digest.finalize())
}

#[test]
fn versioned_withered_foxy_acceptance_plan_is_reproducible() {
    let acceptance_path = workspace_path("fixtures/Withered_Foxy.acceptance-plan.json");
    let acceptance: AcceptancePlan =
        serde_json::from_reader(File::open(&acceptance_path).unwrap()).unwrap();
    let source_path = workspace_path(&acceptance.source.path);
    let options_path = workspace_path(&acceptance.planning_options.path);
    assert_eq!(sha256(&source_path), acceptance.source.sha256);
    assert_eq!(sha256(&options_path), acceptance.planning_options.sha256);

    let options: PreliminaryPlanOptions =
        serde_json::from_reader(File::open(&options_path).unwrap()).unwrap();
    let report = analyze_and_plan_with_options(&source_path, &options).unwrap();
    if let Some(expected_size) = acceptance.source.byte_size {
        assert_eq!(report.analysis.input.byte_size, expected_size);
    }
    let expected = acceptance.expected_plan;
    assert_eq!(report.plan.errors.len(), expected.hard_error_count);
    assert_eq!(report.plan.jobs.len(), expected.job_count);
    assert_eq!(report.plan.plates.len(), expected.target_plate_count);
    assert_eq!(report.plan.batches.len(), expected.batch_count);
    assert_eq!(report.plan.t4_swap_count, expected.t4_swap_count);
    assert_eq!(
        report.plan.a1_spool_change_count,
        expected.a1_spool_change_count
    );

    let mut strategy_counts = BTreeMap::new();
    for job in &report.plan.jobs {
        let key = serde_json::to_value(job.strategy)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
        *strategy_counts.entry(key).or_insert(0) += 1;
    }
    strategy_counts.retain(|_, count| *count > 0);
    let expected_strategy_counts = expected
        .strategy_counts
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .collect::<BTreeMap<_, _>>();
    assert_eq!(strategy_counts, expected_strategy_counts);

    let mut warning_counts = BTreeMap::new();
    for warning in &report.plan.warnings {
        let key = serde_json::to_value(warning.code)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
        *warning_counts.entry(key).or_insert(0) += 1;
    }
    assert_eq!(warning_counts, expected.warning_counts);

    for (actual, expected) in report.plan.batches.iter().zip(expected.batches) {
        assert_eq!(actual.id, expected.id);
        assert_eq!(actual.plate_ids, expected.target_plate_ids);
        assert_eq!(actual.t4_swap_before, expected.swap_before);
        let actual_t4 = match &actual.loadout {
            PrinterLoadout::U1 { loadout } => loadout.slots[3].clone(),
            PrinterLoadout::A1Mini { .. } => None,
        };
        assert_eq!(actual_t4, expected.t4);
    }
}
