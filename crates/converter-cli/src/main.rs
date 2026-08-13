use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde::Serialize;
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};
use u1_a1mini_adapter::{
    build_a1mini_qualification_candidates, discover_bambu_studio, inspect_a1mini_macos_application,
    validate_a1mini_output, validate_a1mini_round_trip,
};
use u1_application::{
    CmyxCalibrationProjectSpec, PreliminaryPlanOptions, analyze_and_plan,
    analyze_and_plan_with_options, source_dialect_approval_fingerprint,
    validate_cmyx_calibration_project_candidate, validate_source_dialect_for_conversion,
    write_cmyx_calibration_project_candidate,
};
use u1_orca_adapter::{
    U1DirectConversionControl, U1FullSpectrumCandidateWriteReport,
    U1FullSpectrumNormalizedSubstrateReport,
    build_u1_full_spectrum_project_settings_with_physical_profiles, discover_installation,
    inspect_macos_application, inspect_u1_direct_macos_application,
    inspect_u1_full_spectrum_macos_application, prepare_u1_full_spectrum_conversion_with_support,
    qualified_u1_physical_profiles_root, validate_u1_full_spectrum_candidate,
    validate_u1_full_spectrum_gui_round_trip, validate_u1_gui_round_trip,
    write_u1_full_spectrum_normalized_substrate, write_u1_full_spectrum_qualification_candidate,
};
use u1_three_mf::{OutputValidationPolicy, analyze_project, validate_staged_output};

#[derive(Debug, Parser)]
#[command(
    name = "u1-converter",
    version,
    about = "Analyze, plan, and validate Bambu/Orca 3MF projects for Snapmaker U1 and Bambu Lab A1 mini"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FullSpectrumQualificationBuildReport {
    adapter_id: String,
    source_sha256: String,
    plan_fingerprint: String,
    output_directory: PathBuf,
    artifacts: Vec<FullSpectrumQualificationArtifactReport>,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FullSpectrumQualificationArtifactReport {
    batch_id: String,
    file_name: String,
    substrate: FullSpectrumSubstrateEvidence,
    candidate: U1FullSpectrumCandidateWriteReport,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FullSpectrumSubstrateEvidence {
    byte_size: u64,
    sha256: String,
    plate_count: usize,
    source_unit_ids: Vec<String>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Safely inspect a project 3MF and print the analysis as JSON.
    Analyze {
        input: PathBuf,
        /// Write one-line JSON instead of indented JSON.
        #[arg(long)]
        compact: bool,
    },
    /// Analyze the project and build a preliminary physical-loadout plan.
    Plan {
        input: PathBuf,
        /// JSON file containing confirmed spools, scope overrides, and printer policy.
        #[arg(long, value_name = "FILE")]
        options: Option<PathBuf>,
        /// Write one-line JSON instead of indented JSON.
        #[arg(long)]
        compact: bool,
        /// Return a failing exit status if the planner reports hard errors.
        #[arg(long)]
        fail_on_errors: bool,
    },
    /// Verify the installed Snapmaker Orca version and required U1 profiles.
    Doctor {
        /// Path to Snapmaker Orca.app. Conventional locations are searched when omitted.
        #[arg(long)]
        orca_app: Option<PathBuf>,
        /// Write one-line JSON instead of indented JSON.
        #[arg(long)]
        compact: bool,
    },
    /// Verify the exact Snapmaker Orca Full Spectrum baseline and qualification gate.
    DoctorFullSpectrum {
        /// Path to Snapmaker Orca.app. Conventional locations are searched when omitted.
        #[arg(long)]
        orca_app: Option<PathBuf>,
        /// Write one-line JSON instead of indented JSON.
        #[arg(long)]
        compact: bool,
    },
    /// Verify the exact Bambu Studio installation and A1 mini profile baseline.
    DoctorA1 {
        /// Path to BambuStudio.app. Conventional locations are searched when omitted.
        #[arg(long)]
        bambu_app: Option<PathBuf>,
        /// Write one-line JSON instead of indented JSON.
        #[arg(long)]
        compact: bool,
    },
    /// Structurally validate an unsliced project 3MF before it is opened in a slicer.
    ValidateOutput {
        input: PathBuf,
        /// Write one-line JSON instead of indented JSON.
        #[arg(long)]
        compact: bool,
    },
    /// Validate an unsliced native A1 mini single-spool Project 3MF.
    ValidateA1 {
        input: PathBuf,
        /// Write one-line JSON instead of indented JSON.
        #[arg(long)]
        compact: bool,
    },
    /// Semantically validate an unsliced Snapmaker U1 Full Spectrum Project 3MF.
    ValidateFullSpectrum {
        input: PathBuf,
        /// Write one-line JSON instead of indented JSON.
        #[arg(long)]
        compact: bool,
    },
    /// Build A1 mini candidates for the mandatory Bambu Studio GUI qualification.
    BuildA1Qualification {
        /// Immutable source Project 3MF.
        input: PathBuf,
        /// JSON planning options that enable A1 mini and select real physical spools.
        #[arg(long, value_name = "FILE")]
        options: PathBuf,
        /// Existing output directory. Existing candidate files are never overwritten.
        #[arg(long, value_name = "DIRECTORY")]
        output: PathBuf,
        /// Path to BambuStudio.app. Conventional locations are searched when omitted.
        #[arg(long)]
        bambu_app: Option<PathBuf>,
        /// Exact fingerprint printed by analysis tooling when the source dialect is Experimental.
        #[arg(long, value_name = "SHA256")]
        approve_experimental_source_fingerprint: Option<String>,
        /// Write one-line JSON instead of indented JSON.
        #[arg(long)]
        compact: bool,
    },
    /// Build U1 Full Spectrum candidates for the explicit Snapmaker Orca GUI qualification.
    BuildFullSpectrumQualification {
        /// Immutable source Project 3MF.
        source: PathBuf,
        /// JSON planning options containing confirmed CMY+X inventory and decisions.
        #[arg(long, value_name = "FILE")]
        options: PathBuf,
        /// Existing output directory. Existing candidate files are never overwritten.
        #[arg(long, value_name = "DIRECTORY")]
        output: PathBuf,
        /// Path to Snapmaker Orca.app. Conventional locations are searched when omitted.
        #[arg(long)]
        orca_app: Option<PathBuf>,
        /// Exact fingerprint printed by analysis tooling when the source dialect is Experimental.
        #[arg(long, value_name = "SHA256")]
        approve_experimental_source_fingerprint: Option<String>,
        /// Write one-line JSON instead of indented JSON.
        #[arg(long)]
        compact: bool,
    },
    /// Build an unsliced, numbered CMY+X calibration qualification candidate.
    BuildCalibrationProject {
        /// JSON chart specification with the exact T1-T4 physical loadout.
        #[arg(long, value_name = "FILE")]
        spec: PathBuf,
        /// New .3mf destination. Existing files are never overwritten.
        #[arg(long, value_name = "FILE")]
        output: PathBuf,
        /// Path to Snapmaker Orca.app. Conventional locations are searched when omitted.
        #[arg(long)]
        orca_app: Option<PathBuf>,
        /// Write one-line JSON instead of indented JSON.
        #[arg(long)]
        compact: bool,
    },
    /// Validate a numbered CMY+X calibration candidate and its embedded manifest.
    ValidateCalibrationProject {
        input: PathBuf,
        /// Write one-line JSON instead of indented JSON.
        #[arg(long)]
        compact: bool,
    },
    /// Compare a U1 writer candidate with two Snapmaker Orca 2.3.5 GUI saves.
    ValidateU1GuiRoundTrip {
        /// Immutable source fixture used to prove source identify_id preservation.
        #[arg(long, value_name = "FILE")]
        source: PathBuf,
        /// Unsliced U1 writer candidate.
        writer_candidate: PathBuf,
        /// First project saved by the Snapmaker Orca GUI after slicing.
        first_gui_save: PathBuf,
        /// Project saved after closing, reopening, and slicing again.
        reopened_gui_save: PathBuf,
        /// Write one-line JSON instead of indented JSON.
        #[arg(long)]
        compact: bool,
    },
    /// Compare an A1 mini candidate with its first and reopened Bambu Studio saves.
    ValidateA1RoundTrip {
        /// Unsliced A1 mini writer candidate.
        candidate: PathBuf,
        /// Project saved by Bambu Studio after the first open/slice/save cycle.
        first_save: PathBuf,
        /// Project saved after closing, reopening, slicing, and saving again.
        reopened_save: PathBuf,
        /// Write one-line JSON instead of indented JSON.
        #[arg(long)]
        compact: bool,
    },
    /// Compare a Full Spectrum candidate with two Snapmaker Orca GUI saves.
    ValidateFullSpectrumRoundTrip {
        /// Unsliced U1 Full Spectrum writer candidate.
        candidate: PathBuf,
        /// Project saved by Snapmaker Orca after the first open/slice/save cycle.
        first_save: PathBuf,
        /// Project saved after closing, reopening, slicing, and saving again.
        reopened_save: PathBuf,
        /// Write one-line JSON instead of indented JSON.
        #[arg(long)]
        compact: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Analyze { input, compact } => {
            let analysis = analyze_project(&input)
                .with_context(|| format!("failed to analyze {}", input.display()))?;
            print_json(&analysis, compact)?;
        }
        Command::Plan {
            input,
            options,
            compact,
            fail_on_errors,
        } => {
            let report = if let Some(options_path) = options {
                let options = read_plan_options(&options_path)?;
                analyze_and_plan_with_options(&input, &options)
            } else {
                analyze_and_plan(&input)
            }
            .with_context(|| format!("failed to plan {}", input.display()))?;
            let has_hard_errors = report.plan.has_hard_errors();
            print_json(&report, compact)?;
            if fail_on_errors && has_hard_errors {
                bail!("the preliminary plan contains hard errors");
            }
        }
        Command::Doctor { orca_app, compact } => {
            let application_path = orca_app
                .or_else(discover_installation)
                .context("Snapmaker Orca was not found; pass --orca-app <path>")?;
            let report =
                inspect_u1_direct_macos_application(&application_path).with_context(|| {
                    format!(
                        "failed to inspect the Snapmaker U1 Direct capability at {}",
                        application_path.display()
                    )
                })?;
            let conversion_available = report.conversion_available;
            print_json(&report, compact)?;
            if !conversion_available {
                bail!("Snapmaker U1 Direct conversion is unavailable on this installation");
            }
        }
        Command::DoctorFullSpectrum { orca_app, compact } => {
            let application_path = orca_app
                .or_else(discover_installation)
                .context("Snapmaker Orca was not found; pass --orca-app <path>")?;
            let report = inspect_u1_full_spectrum_macos_application(&application_path)
                .with_context(|| {
                    format!(
                        "failed to inspect the Snapmaker U1 Full Spectrum capability at {}",
                        application_path.display()
                    )
                })?;
            let conversion_available = report.conversion_available;
            print_json(&report, compact)?;
            if !conversion_available {
                bail!(
                    "Snapmaker U1 Full Spectrum conversion is not production-qualified on this installation"
                );
            }
        }
        Command::DoctorA1 { bambu_app, compact } => {
            let application_path = bambu_app
                .or_else(discover_bambu_studio)
                .context("Bambu Studio was not found; pass --bambu-app <path>")?;
            let report =
                inspect_a1mini_macos_application(&application_path).with_context(|| {
                    format!(
                        "failed to inspect the Bambu Lab A1 mini capability at {}",
                        application_path.display()
                    )
                })?;
            let conversion_available = report.conversion_available;
            print_json(&report, compact)?;
            if !conversion_available {
                bail!(
                    "Bambu Lab A1 mini conversion is not production-qualified on this installation"
                );
            }
        }
        Command::ValidateOutput { input, compact } => {
            let report = validate_staged_output(&input, &OutputValidationPolicy::strict_unsliced())
                .with_context(|| format!("failed to validate {}", input.display()))?;
            let is_valid = report.is_valid;
            print_json(&report, compact)?;
            if !is_valid {
                bail!("the output package contains blocking structural errors");
            }
        }
        Command::ValidateA1 { input, compact } => {
            let report = validate_a1mini_output(&input).with_context(|| {
                format!(
                    "failed to validate {} as an A1 mini project",
                    input.display()
                )
            })?;
            let is_valid = report.valid;
            print_json(&report, compact)?;
            if !is_valid {
                bail!("the A1 mini project contains blocking target-contract errors");
            }
        }
        Command::ValidateFullSpectrum { input, compact } => {
            let report = validate_u1_full_spectrum_candidate(&input, None).with_context(|| {
                format!(
                    "failed to validate {} as a Snapmaker U1 Full Spectrum project",
                    input.display()
                )
            })?;
            let is_valid = report.valid;
            print_json(&report, compact)?;
            if !is_valid {
                bail!("the Full Spectrum project contains blocking target-contract errors");
            }
        }
        Command::BuildA1Qualification {
            input,
            options,
            output,
            bambu_app,
            approve_experimental_source_fingerprint,
            compact,
        } => {
            let application_path = bambu_app
                .or_else(discover_bambu_studio)
                .context("Bambu Studio was not found; pass --bambu-app <path>")?;
            let capability =
                inspect_a1mini_macos_application(&application_path).with_context(|| {
                    format!(
                        "failed to inspect the Bambu Lab A1 mini capability at {}",
                        application_path.display()
                    )
                })?;
            if !capability.installation_supported() {
                bail!("the Bambu Lab A1 mini adapter does not support this installation");
            }
            if !output.is_dir() {
                bail!(
                    "A1 mini qualification output {} is not an existing directory",
                    output.display()
                );
            }
            let options = read_plan_options(&options)?;
            let report = analyze_and_plan_with_options(&input, &options).with_context(|| {
                format!("failed to prepare an A1 mini plan for {}", input.display())
            })?;
            if report.plan.has_hard_errors() {
                let errors = serde_json::to_string(&report.plan.errors)
                    .context("failed to serialize planning errors")?;
                bail!("the prepared A1 mini plan contains hard errors: {errors}");
            }
            validate_source_dialect_for_conversion(
                &report.analysis,
                approve_experimental_source_fingerprint.as_deref(),
            )
            .map_err(|error| {
                anyhow::anyhow!(
                    "source dialect is not approved for the A1 mini writer adapter: {error}; current source fingerprint is {}",
                    source_dialect_approval_fingerprint(&report.analysis)
                )
            })?;
            let conversion = build_a1mini_qualification_candidates(
                &application_path,
                &input,
                &report.analysis,
                &report.planning_input,
                &report.plan,
                &output,
            )
            .with_context(|| {
                format!(
                    "failed to build A1 mini qualification candidates in {}",
                    output.display()
                )
            })?;
            print_json(&conversion, compact)?;
        }
        Command::BuildFullSpectrumQualification {
            source,
            options,
            output,
            orca_app,
            approve_experimental_source_fingerprint,
            compact,
        } => {
            let application_path = orca_app
                .or_else(discover_installation)
                .context("Snapmaker Orca was not found; pass --orca-app <path>")?;
            let build = build_full_spectrum_qualification(
                &application_path,
                &source,
                &options,
                &output,
                approve_experimental_source_fingerprint.as_deref(),
            )?;
            print_json(&build, compact)?;
        }
        Command::BuildCalibrationProject {
            spec,
            output,
            orca_app,
            compact,
        } => {
            let application_path = orca_app
                .or_else(discover_installation)
                .context("Snapmaker Orca was not found; pass --orca-app <path>")?;
            let spec = read_calibration_project_spec(&spec)?;
            let report =
                write_cmyx_calibration_project_candidate(&application_path, &spec, &output)
                    .with_context(|| {
                        format!(
                            "failed to build CMY+X calibration candidate {}",
                            output.display()
                        )
                    })?;
            print_json(&report, compact)?;
        }
        Command::ValidateCalibrationProject { input, compact } => {
            let report =
                validate_cmyx_calibration_project_candidate(&input).with_context(|| {
                    format!(
                        "failed to validate {} as a CMY+X calibration candidate",
                        input.display()
                    )
                })?;
            let is_valid = report.valid;
            print_json(&report, compact)?;
            if !is_valid {
                bail!("the CMY+X calibration project contains blocking contract errors");
            }
        }
        Command::ValidateU1GuiRoundTrip {
            source,
            writer_candidate,
            first_gui_save,
            reopened_gui_save,
            compact,
        } => {
            let report = validate_u1_gui_round_trip(
                &source,
                &writer_candidate,
                &first_gui_save,
                &reopened_gui_save,
            )
            .with_context(|| {
                format!(
                    "failed to validate the Snapmaker Orca 2.3.5 GUI round trip for {}",
                    writer_candidate.display()
                )
            })?;
            let is_valid = report.is_valid;
            print_json(&report, compact)?;
            if !is_valid {
                bail!("the U1 GUI round trip contains blocking semantic errors");
            }
        }
        Command::ValidateA1RoundTrip {
            candidate,
            first_save,
            reopened_save,
            compact,
        } => {
            let report = validate_a1mini_round_trip(&candidate, &first_save, &reopened_save)
                .with_context(|| {
                    format!(
                        "failed to validate the A1 mini Bambu Studio round trip for {}",
                        candidate.display()
                    )
                })?;
            let is_valid = report.is_valid;
            print_json(&report, compact)?;
            if !is_valid {
                bail!("the A1 mini GUI round trip contains blocking semantic errors");
            }
        }
        Command::ValidateFullSpectrumRoundTrip {
            candidate,
            first_save,
            reopened_save,
            compact,
        } => {
            let report =
                validate_u1_full_spectrum_gui_round_trip(&candidate, &first_save, &reopened_save)
                    .with_context(|| {
                    format!(
                        "failed to validate the Snapmaker Orca Full Spectrum round trip for {}",
                        candidate.display()
                    )
                })?;
            let is_valid = report.is_valid;
            print_json(&report, compact)?;
            if !is_valid {
                bail!("the Full Spectrum GUI round trip contains blocking semantic errors");
            }
        }
    }
    Ok(())
}

fn build_full_spectrum_qualification(
    application_path: &Path,
    source: &Path,
    options_path: &Path,
    output: &Path,
    approved_experimental_source_fingerprint: Option<&str>,
) -> Result<FullSpectrumQualificationBuildReport> {
    let capability =
        inspect_u1_full_spectrum_macos_application(application_path).with_context(|| {
            format!(
                "failed to inspect the Snapmaker U1 Full Spectrum capability at {}",
                application_path.display()
            )
        })?;
    if !capability.installation_supported() || !capability.qualification_candidate_available {
        bail!("the Snapmaker U1 Full Spectrum adapter does not support this installation");
    }
    if !output.is_dir() {
        bail!(
            "Full Spectrum qualification output {} is not an existing directory",
            output.display()
        );
    }
    let options = read_plan_options(options_path)?;
    let report = analyze_and_plan_with_options(source, &options).with_context(|| {
        format!(
            "failed to prepare a Full Spectrum plan for {}",
            source.display()
        )
    })?;
    if report.plan.has_hard_errors() {
        let errors = serde_json::to_string(&report.plan.errors)
            .context("failed to serialize planning errors")?;
        bail!("the prepared Full Spectrum plan contains hard errors: {errors}");
    }
    validate_source_dialect_for_conversion(
        &report.analysis,
        approved_experimental_source_fingerprint,
    )
    .map_err(|error| {
        anyhow::anyhow!(
            "source dialect is not approved for the Full Spectrum writer adapter: {error}; current source fingerprint is {}",
            source_dialect_approval_fingerprint(&report.analysis)
        )
    })?;
    let preparation = prepare_u1_full_spectrum_conversion_with_support(
        &report.planning_input,
        &report.plan,
        &report.analysis.process.support,
    )
    .with_context(|| {
        format!(
            "failed to prepare Full Spectrum artifacts for {}",
            source.display()
        )
    })?;
    for artifact in &preparation.artifacts {
        let destination = output.join(&artifact.file_name);
        if destination.try_exists().with_context(|| {
            format!(
                "failed to inspect qualification destination {}",
                destination.display()
            )
        })? {
            bail!(
                "Full Spectrum qualification candidate already exists: {}",
                destination.display()
            );
        }
    }

    let installation = inspect_macos_application(application_path).with_context(|| {
        format!(
            "failed to resolve Snapmaker Orca profiles at {}",
            application_path.display()
        )
    })?;
    let profiles_root = installation.resources_path.join("profiles/Snapmaker");
    let physical_profiles_root = qualified_u1_physical_profiles_root(application_path)
        .context("failed to resolve the qualified physical U1 profile pack")?;
    let scratch = tempfile::Builder::new()
        .prefix(".u1-full-spectrum-qualification-")
        .tempdir_in(output)
        .with_context(|| {
            format!(
                "failed to create private Full Spectrum scratch space in {}",
                output.display()
            )
        })?;
    let control = U1DirectConversionControl::new();
    let mut artifacts = Vec::with_capacity(preparation.artifacts.len());
    for artifact in &preparation.artifacts {
        let project_settings = build_u1_full_spectrum_project_settings_with_physical_profiles(
            &profiles_root,
            &physical_profiles_root,
            artifact,
        )
        .with_context(|| {
            format!(
                "failed to build Full Spectrum project settings for batch {}",
                artifact.batch_id
            )
        })?;
        let substrate_name = format!(
            "{}.normalized-substrate.3mf",
            artifact.file_name.trim_end_matches(".3mf")
        );
        let substrate_path = scratch.path().join(substrate_name);
        let substrate = write_u1_full_spectrum_normalized_substrate(
            source,
            &report.analysis,
            &report.planning_input,
            artifact,
            &project_settings,
            &substrate_path,
            &control,
        )
        .with_context(|| {
            format!(
                "failed to build normalized Full Spectrum substrate for batch {}",
                artifact.batch_id
            )
        })?;
        let destination = output.join(&artifact.file_name);
        let candidate = write_u1_full_spectrum_qualification_candidate(
            &substrate.path,
            &destination,
            &project_settings,
            artifact,
        )
        .with_context(|| {
            format!(
                "failed to write Full Spectrum qualification candidate {}",
                destination.display()
            )
        })?;
        artifacts.push(FullSpectrumQualificationArtifactReport {
            batch_id: artifact.batch_id.clone(),
            file_name: artifact.file_name.clone(),
            substrate: substrate_evidence(substrate),
            candidate,
        });
    }
    Ok(FullSpectrumQualificationBuildReport {
        adapter_id: preparation.adapter_id,
        source_sha256: report.analysis.input.sha256,
        plan_fingerprint: preparation.plan_fingerprint,
        output_directory: output.to_owned(),
        artifacts,
        warnings: preparation.warnings,
    })
}

fn substrate_evidence(
    report: U1FullSpectrumNormalizedSubstrateReport,
) -> FullSpectrumSubstrateEvidence {
    FullSpectrumSubstrateEvidence {
        byte_size: report.byte_size,
        sha256: report.sha256,
        plate_count: report.plate_count,
        source_unit_ids: report.source_unit_ids,
    }
}

fn read_plan_options(path: &Path) -> Result<PreliminaryPlanOptions> {
    const MAX_OPTIONS_BYTES: u64 = 1024 * 1024;
    let file = File::open(path)
        .with_context(|| format!("failed to open planning options {}", path.display()))?;
    if !file
        .metadata()
        .with_context(|| format!("failed to inspect planning options {}", path.display()))?
        .is_file()
    {
        bail!("planning options {} is not a regular file", path.display());
    }
    let mut bytes = Vec::new();
    file.take(MAX_OPTIONS_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read planning options {}", path.display()))?;
    if bytes.len() as u64 > MAX_OPTIONS_BYTES {
        bail!(
            "planning options {} exceeds the {} byte limit",
            path.display(),
            MAX_OPTIONS_BYTES
        );
    }
    serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid planning options JSON in {}", path.display()))
}

fn read_calibration_project_spec(path: &Path) -> Result<CmyxCalibrationProjectSpec> {
    const MAX_SPEC_BYTES: u64 = 1024 * 1024;
    let file = File::open(path).with_context(|| {
        format!(
            "failed to open calibration specification {}",
            path.display()
        )
    })?;
    if !file
        .metadata()
        .with_context(|| {
            format!(
                "failed to inspect calibration specification {}",
                path.display()
            )
        })?
        .is_file()
    {
        bail!(
            "calibration specification {} is not a regular file",
            path.display()
        );
    }
    let mut bytes = Vec::new();
    file.take(MAX_SPEC_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| {
            format!(
                "failed to read calibration specification {}",
                path.display()
            )
        })?;
    if bytes.len() as u64 > MAX_SPEC_BYTES {
        bail!(
            "calibration specification {} exceeds the {} byte limit",
            path.display(),
            MAX_SPEC_BYTES
        );
    }
    serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "invalid calibration specification JSON in {}",
            path.display()
        )
    })
}

fn print_json(value: &impl Serialize, compact: bool) -> Result<()> {
    let json = if compact {
        serde_json::to_string(value)
    } else {
        serde_json::to_string_pretty(value)
    }
    .context("failed to serialize JSON output")?;
    println!("{json}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_documented_commands() {
        let analyze = Cli::try_parse_from(["u1-converter", "analyze", "model.3mf"]);
        assert!(analyze.is_ok());
        let plan = Cli::try_parse_from([
            "u1-converter",
            "plan",
            "model.3mf",
            "--options",
            "choices.json",
            "--fail-on-errors",
        ]);
        assert!(plan.is_ok());
        let doctor = Cli::try_parse_from([
            "u1-converter",
            "doctor",
            "--orca-app",
            "/Applications/Snapmaker Orca.app",
        ]);
        assert!(doctor.is_ok());
        let doctor_a1 = Cli::try_parse_from([
            "u1-converter",
            "doctor-a1",
            "--bambu-app",
            "/Applications/BambuStudio.app",
            "--compact",
        ]);
        assert!(doctor_a1.is_ok());
        let doctor_full_spectrum = Cli::try_parse_from([
            "u1-converter",
            "doctor-full-spectrum",
            "--orca-app",
            "/Applications/Snapmaker Orca.app",
            "--compact",
        ]);
        assert!(doctor_full_spectrum.is_ok());
        let validate = Cli::try_parse_from([
            "u1-converter",
            "validate-output",
            "converted.3mf",
            "--compact",
        ]);
        assert!(validate.is_ok());
        let validate_a1 = Cli::try_parse_from([
            "u1-converter",
            "validate-a1",
            "a1-candidate.3mf",
            "--compact",
        ]);
        assert!(validate_a1.is_ok());
        let validate_full_spectrum = Cli::try_parse_from([
            "u1-converter",
            "validate-full-spectrum",
            "full-spectrum-candidate.3mf",
            "--compact",
        ]);
        assert!(validate_full_spectrum.is_ok());
        let build_a1 = Cli::try_parse_from([
            "u1-converter",
            "build-a1-qualification",
            "source.3mf",
            "--options",
            "a1-options.json",
            "--output",
            "qualification-output",
            "--bambu-app",
            "/Applications/BambuStudio.app",
            "--compact",
        ])
        .expect("documented A1 qualification command must parse");
        match build_a1.command {
            Command::BuildA1Qualification {
                input,
                options,
                output,
                bambu_app,
                approve_experimental_source_fingerprint,
                compact,
            } => {
                assert_eq!(input, PathBuf::from("source.3mf"));
                assert_eq!(options, PathBuf::from("a1-options.json"));
                assert_eq!(output, PathBuf::from("qualification-output"));
                assert_eq!(
                    bambu_app,
                    Some(PathBuf::from("/Applications/BambuStudio.app"))
                );
                assert!(approve_experimental_source_fingerprint.is_none());
                assert!(compact);
            }
            other => panic!("unexpected parsed command: {other:?}"),
        }
        let build_full_spectrum = Cli::try_parse_from([
            "u1-converter",
            "build-full-spectrum-qualification",
            "source.3mf",
            "--options",
            "full-spectrum-options.json",
            "--output",
            "qualification-output",
            "--orca-app",
            "/Applications/Snapmaker Orca.app",
            "--compact",
        ]);
        assert!(build_full_spectrum.is_ok());
        let build_calibration = Cli::try_parse_from([
            "u1-converter",
            "build-calibration-project",
            "--spec",
            "calibration.json",
            "--output",
            "calibration.3mf",
            "--orca-app",
            "/Applications/Snapmaker Orca.app",
            "--compact",
        ]);
        assert!(build_calibration.is_ok());
        let validate_calibration = Cli::try_parse_from([
            "u1-converter",
            "validate-calibration-project",
            "calibration.3mf",
            "--compact",
        ]);
        assert!(validate_calibration.is_ok());
        let gui_round_trip = Cli::try_parse_from([
            "u1-converter",
            "validate-u1-gui-round-trip",
            "candidate.3mf",
            "first-save.3mf",
            "reopened-save.3mf",
            "--source",
            "source.3mf",
            "--compact",
        ]);
        assert!(gui_round_trip.is_ok());
        let a1_round_trip = Cli::try_parse_from([
            "u1-converter",
            "validate-a1-round-trip",
            "candidate.3mf",
            "first-save.3mf",
            "reopened-save.3mf",
            "--compact",
        ]);
        assert!(a1_round_trip.is_ok());
        let full_spectrum_round_trip = Cli::try_parse_from([
            "u1-converter",
            "validate-full-spectrum-round-trip",
            "candidate.3mf",
            "first-save.3mf",
            "reopened-save.3mf",
            "--compact",
        ]);
        assert!(full_spectrum_round_trip.is_ok());
    }

    #[test]
    fn a1_round_trip_command_requires_all_three_artifacts() {
        let missing_reopened_save = Cli::try_parse_from([
            "u1-converter",
            "validate-a1-round-trip",
            "candidate.3mf",
            "first-save.3mf",
        ]);
        assert!(missing_reopened_save.is_err());
    }

    #[test]
    fn a1_qualification_command_requires_explicit_options_and_output() {
        let missing_options = Cli::try_parse_from([
            "u1-converter",
            "build-a1-qualification",
            "source.3mf",
            "--output",
            "qualification-output",
        ]);
        assert!(missing_options.is_err());

        let missing_output = Cli::try_parse_from([
            "u1-converter",
            "build-a1-qualification",
            "source.3mf",
            "--options",
            "a1-options.json",
        ]);
        assert!(missing_output.is_err());
    }

    #[test]
    fn full_spectrum_qualification_command_requires_explicit_options_and_output() {
        let missing_options = Cli::try_parse_from([
            "u1-converter",
            "build-full-spectrum-qualification",
            "source.3mf",
            "--output",
            "qualification-output",
        ]);
        assert!(missing_options.is_err());

        let missing_output = Cli::try_parse_from([
            "u1-converter",
            "build-full-spectrum-qualification",
            "source.3mf",
            "--options",
            "full-spectrum-options.json",
        ]);
        assert!(missing_output.is_err());
    }

    #[test]
    fn reads_backward_compatible_empty_plan_options() {
        let mut file = tempfile::NamedTempFile::new().expect("temporary options file");
        file.write_all(b"{}").expect("write options");

        let options = read_plan_options(file.path()).expect("empty options use safe defaults");
        assert!(options.confirmed_spools.is_empty());
        assert!(options.scope_overrides.is_empty());
        assert!(!options.a1_mini.enabled);
    }

    #[test]
    fn rejects_plan_options_larger_than_one_mebibyte() {
        let mut file = tempfile::NamedTempFile::new().expect("temporary options file");
        file.write_all(&vec![b' '; 1024 * 1024 + 1])
            .expect("write oversized options");

        let error = read_plan_options(file.path()).expect_err("oversized input must be rejected");
        assert!(error.to_string().contains("exceeds the 1048576 byte limit"));
    }

    #[test]
    fn documented_plan_options_example_stays_deserializable() {
        let options: PreliminaryPlanOptions =
            serde_json::from_str(include_str!("../../../examples/plan-options.example.json"))
                .expect("documented plan options must deserialize");

        assert_eq!(options.confirmed_spools.len(), 1);
        assert_eq!(options.scope_overrides.len(), 1);
    }
}
