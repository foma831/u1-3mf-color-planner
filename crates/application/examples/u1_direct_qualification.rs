use std::path::PathBuf;

use u1_application::{PreliminaryPlanOptions, analyze_and_plan_with_options};
use u1_orca_adapter::build_u1_direct_qualification_candidate;
use u1_planner::ScopeStrategy;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let source = PathBuf::from(arguments.next().ok_or("missing source 3MF path")?);
    let destination = PathBuf::from(arguments.next().ok_or("missing destination directory")?);
    let application = PathBuf::from(
        arguments
            .next()
            .unwrap_or_else(|| "/Applications/Snapmaker Orca.app".into()),
    );
    let options = PreliminaryPlanOptions {
        scope_strategy: ScopeStrategy::DirectSpools,
        restore_cmy_after_direct: false,
        ..PreliminaryPlanOptions::default()
    };
    let report = analyze_and_plan_with_options(&source, &options)?;
    if report.plan.has_hard_errors() {
        return Err(format!("planner errors: {:#?}", report.plan.errors).into());
    }
    let input = report.planning_input.clone();
    let output = build_u1_direct_qualification_candidate(
        &application,
        &source,
        &report.analysis,
        &input,
        &report.plan,
        &destination,
    )?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}
