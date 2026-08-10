//! Semantic validation for the Snapmaker Orca 2.3.5 U1 GUI qualification loop.
//!
//! A normal GUI save is allowed to regenerate a narrowly versioned set of
//! preview and slice-metadata parts. It is also allowed to renumber Orca's
//! process-local `identify_id` values. Neither exception weakens the semantic
//! comparison performed here.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use thiserror::Error;
use u1_three_mf::{
    AnalysisError, AxisAlignedBounds, MAIN_MODEL_PATH, OutputValidationPolicy,
    OutputValidationReport, ProjectAnalysis, StagedOutputValidationError, Transform3mf, VolumeType,
    analyze_project, decode_paint_annotation, validate_staged_output,
};
use zip::ZipArchive;

use crate::{SUPPORTED_ORCA_VERSION, U1_DIRECT_ADAPTER_ID, U1_DIRECT_MACHINE_PROFILE};

const PROJECT_SETTINGS_PATH: &str = "Metadata/project_settings.config";
const MODEL_SETTINGS_PATH: &str = "Metadata/model_settings.config";
const MAX_CONFIG_BYTES: u64 = 64 * 1024 * 1024;
const MAX_MODEL_BYTES: u64 = 1024 * 1024 * 1024;
const TRANSFORM_FLOAT_SCALE: f64 = 1_000_000_000.0;
// Snapmaker Orca 2.3.5 may rebase an editable text mesh while preserving its
// world-space geometry and may rewrite mesh floats with sub-micrometre noise.
// Ten-micrometre buckets are still twenty times finer than the qualified
// 0.20 mm process and keep that representation-only rewrite out of the
// semantic comparison.
const GEOMETRY_FLOAT_SCALE: f64 = 100_000.0;
const BOUNDS_FLOAT_SCALE: f64 = GEOMETRY_FLOAT_SCALE;
const TEXT_MESH_REBASE_TOLERANCE_MM: f64 = 0.0001;
const MAX_REBASED_TEXT_MESH_VERTICES: u64 = 10_000;
const MAX_REBASED_TEXT_MESH_TRIANGLES: u64 = 20_000;
const EXPECTED_FILAMENT_MAP_MODE: &str = "Auto For Flush";
const EXPECTED_FILAMENT_MAP: &str = "1 1 1 1";
const EXPECTED_WIPE_TOWER_X_MM: f64 = 14.5;
const EXPECTED_WIPE_TOWER_Y_MM: f64 = 212.0;

type SemanticMultiset = BTreeMap<String, usize>;
type ObjectSignatureById = BTreeMap<u32, String>;

/// Role of one artifact in the qualification sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum U1GuiArtifactRole {
    WriterCandidate,
    FirstGuiSave,
    ReopenedGuiSave,
}

/// Stable issue categories returned by the three-file validator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum U1GuiRoundTripIssueCode {
    StructuralArtifactPolicyFailed,
    UnsupportedU1ProjectIdentity,
    FilamentIdentityChanged,
    FilamentMapChanged,
    InvalidStageBTargetGlobals,
    StageBTargetGlobalsChanged,
    InvalidPrimeTowerContract,
    PrimeTowerContractChanged,
    EmbeddedPresetFound,
    MissingCandidateIdentifyId,
    InvalidCandidateIdentifyId,
    SourceIdentifyIdMismatch,
    InvalidGuiId,
    ObjectOrPartBijectionChanged,
    InstanceBijectionChanged,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct U1GuiRoundTripIssue {
    pub code: U1GuiRoundTripIssueCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<U1GuiArtifactRole>,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct U1GuiArtifactSummary {
    pub role: U1GuiArtifactRole,
    pub byte_size: u64,
    pub sha256: String,
    pub plate_count: usize,
    pub object_count: usize,
    pub instance_count: usize,
    pub part_count: usize,
    pub geometry_resource_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_sha256: Option<String>,
    pub structural_validation: OutputValidationReport,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct U1GuiRoundTripChecks {
    pub candidate_structurally_valid: bool,
    pub first_gui_save_structurally_valid: bool,
    pub reopened_gui_save_structurally_valid: bool,
    pub exact_snapmaker_orca_2_3_5_u1_identity: bool,
    pub t1_t4_profile_identity_stable: bool,
    pub plate_filament_map_stable: bool,
    pub candidate_stage_b_target_globals_exact: bool,
    pub stage_b_target_globals_stable_across_gui_saves: bool,
    pub candidate_prime_tower_contract_exact: bool,
    pub prime_tower_contract_stable_across_gui_saves: bool,
    pub no_embedded_presets: bool,
    pub candidate_identify_ids_present: bool,
    pub candidate_identify_ids_positive_and_unique: bool,
    pub writer_candidate_source_identify_ids_preserved: bool,
    pub gui_ids_positive_unique_and_consistent: bool,
    pub object_and_part_bijection_stable: bool,
    pub instance_bijection_stable: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct U1GuiRoundTripReport {
    pub schema_version: u32,
    pub adapter_id: &'static str,
    pub application_version: &'static str,
    pub is_valid: bool,
    pub source_sha256: String,
    pub artifacts: Vec<U1GuiArtifactSummary>,
    pub checks: U1GuiRoundTripChecks,
    pub issues: Vec<U1GuiRoundTripIssue>,
}

#[derive(Debug, Error)]
pub enum U1GuiRoundTripError {
    #[error("failed to analyze {path}: {source}")]
    Analyze {
        path: PathBuf,
        #[source]
        source: AnalysisError,
    },
    #[error("failed to structurally validate {path}: {source}")]
    Structural {
        path: PathBuf,
        #[source]
        source: StagedOutputValidationError,
    },
    #[error("failed to open {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read 3MF archive {path}: {source}")]
    Zip {
        path: PathBuf,
        #[source]
        source: zip::result::ZipError,
    },
    #[error("required entry {entry} is missing from {path}")]
    MissingEntry { path: PathBuf, entry: &'static str },
    #[error("entry {entry} in {path} exceeds the {limit}-byte limit")]
    EntryTooLarge {
        path: PathBuf,
        entry: String,
        limit: u64,
    },
    #[error("invalid JSON in {entry} from {path}: {source}")]
    Json {
        path: PathBuf,
        entry: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid XML in {entry} from {path}: {message}")]
    Xml {
        path: PathBuf,
        entry: String,
        message: String,
    },
    #[error("invalid U1 GUI round-trip data in {path}: {message}")]
    Semantic { path: PathBuf, message: String },
    #[error("qualification input changed while it was being validated: {path}")]
    InputChanged { path: PathBuf },
}

/// Validate a writer candidate and both successive GUI saves.
///
/// Every candidate instance is matched by source object/instance identity and
/// its original Orca `identify_id` is checked exactly. The three GUI artifacts
/// are never accepted without the immutable source fixture bytes.
pub fn validate_u1_gui_round_trip(
    source: impl AsRef<Path>,
    writer_candidate: impl AsRef<Path>,
    first_gui_save: impl AsRef<Path>,
    reopened_gui_save: impl AsRef<Path>,
) -> Result<U1GuiRoundTripReport, U1GuiRoundTripError> {
    let source_analysis = analyze(source.as_ref())?;
    let paths = [
        writer_candidate.as_ref(),
        first_gui_save.as_ref(),
        reopened_gui_save.as_ref(),
    ];
    let roles = [
        U1GuiArtifactRole::WriterCandidate,
        U1GuiArtifactRole::FirstGuiSave,
        U1GuiArtifactRole::ReopenedGuiSave,
    ];

    let analyses = paths
        .iter()
        .map(|path| analyze(path))
        .collect::<Result<Vec<_>, _>>()?;
    let structural = vec![
        structurally_validate(paths[0], &OutputValidationPolicy::strict_unsliced())?,
        structurally_validate(
            paths[1],
            &OutputValidationPolicy::snapmaker_orca_2_3_5_gui_save(
                analyses[1].plates.iter().map(|plate| plate.id),
            ),
        )?,
        structurally_validate(
            paths[2],
            &OutputValidationPolicy::snapmaker_orca_2_3_5_gui_save(
                analyses[2].plates.iter().map(|plate| plate.id),
            ),
        )?,
    ];

    let mut issues = Vec::new();
    for ((role, path), report) in roles.iter().zip(paths).zip(&structural) {
        if !report.is_valid {
            issues.push(U1GuiRoundTripIssue {
                code: U1GuiRoundTripIssueCode::StructuralArtifactPolicyFailed,
                artifact: Some(*role),
                message: format!(
                    "{} failed the version-scoped structural artifact policy with {} issue(s).",
                    path.display(),
                    report.issues.len()
                ),
            });
        }
    }
    verify_analysis_identity(source.as_ref(), &source_analysis)?;
    for (path, analysis) in paths.iter().zip(&analyses) {
        verify_analysis_identity(path, analysis)?;
    }

    if structural.iter().any(|report| !report.is_valid) {
        return Ok(early_structural_report(
            &analyses,
            structural,
            source_analysis,
            issues,
        ));
    }

    let snapshots = paths
        .iter()
        .zip(&analyses)
        .map(|(path, analysis)| SemanticSnapshot::load(path, analysis))
        .collect::<Result<Vec<_>, _>>()?;
    verify_analysis_identity(source.as_ref(), &source_analysis)?;
    for (path, analysis) in paths.iter().zip(&analyses) {
        verify_analysis_identity(path, analysis)?;
    }

    let mut exact_u1_identity = true;
    for ((snapshot, analysis), role) in snapshots.iter().zip(&analyses).zip(roles) {
        let valid = snapshot.profile.is_exact_u1_direct()
            && analysis.printer.model.as_deref() == Some("Snapmaker U1")
            && analysis.printer.variant.as_deref() == Some("0.4")
            && analysis.printer.nozzle_diameters_mm.len() == 4
            && analysis
                .printer
                .nozzle_diameters_mm
                .iter()
                .all(|diameter| (*diameter - 0.4).abs() <= f64::EPSILON);
        if !valid {
            exact_u1_identity = false;
            issues.push(U1GuiRoundTripIssue {
                code: U1GuiRoundTripIssueCode::UnsupportedU1ProjectIdentity,
                artifact: Some(role),
                message: "Project does not identify the exact Snapmaker Orca 2.3.5 U1 Direct machine/process contract.".to_owned(),
            });
        }
    }

    let profile_stable = snapshots[1].profile == snapshots[0].profile
        && snapshots[2].profile == snapshots[0].profile;
    if !profile_stable {
        issues.push(U1GuiRoundTripIssue {
            code: U1GuiRoundTripIssueCode::FilamentIdentityChanged,
            artifact: None,
            message: "T1-T4 profile names, setting IDs, colors, or materials changed across the GUI saves.".to_owned(),
        });
    }

    let filament_map_stable = snapshots
        .iter()
        .all(SemanticSnapshot::has_exact_filament_maps)
        && snapshots[1].plate_map_count == snapshots[0].plate_map_count
        && snapshots[2].plate_map_count == snapshots[0].plate_map_count;
    if !filament_map_stable {
        issues.push(U1GuiRoundTripIssue {
            code: U1GuiRoundTripIssueCode::FilamentMapChanged,
            artifact: None,
            message: "One or more plates changed the four-slot Auto For Flush filament map."
                .to_owned(),
        });
    }

    let candidate_stage_b_target_globals_exact = snapshots[0].target_globals.is_exact();
    if !candidate_stage_b_target_globals_exact {
        issues.push(U1GuiRoundTripIssue {
            code: U1GuiRoundTripIssueCode::InvalidStageBTargetGlobals,
            artifact: Some(U1GuiArtifactRole::WriterCandidate),
            message: "Writer candidate does not contain the exact Textured PEI, by-layer, non-spiral, traditional-timelapse Stage-B globals.".to_owned(),
        });
    }
    let mut stage_b_target_globals_stable = true;
    for (snapshot, role) in snapshots[1..].iter().zip([
        U1GuiArtifactRole::FirstGuiSave,
        U1GuiArtifactRole::ReopenedGuiSave,
    ]) {
        if snapshot.target_globals != snapshots[0].target_globals {
            stage_b_target_globals_stable = false;
            issues.push(U1GuiRoundTripIssue {
                code: U1GuiRoundTripIssueCode::StageBTargetGlobalsChanged,
                artifact: Some(role),
                message: "Stage-B target globals changed from the normalized writer-candidate values during a GUI save.".to_owned(),
            });
        }
    }

    let candidate_prime_tower_contract_exact =
        snapshots[0].prime_tower.is_exact(analyses[0].plates.len());
    if !candidate_prime_tower_contract_exact {
        issues.push(U1GuiRoundTripIssue {
            code: U1GuiRoundTripIssueCode::InvalidPrimeTowerContract,
            artifact: Some(U1GuiArtifactRole::WriterCandidate),
            message: "Writer candidate does not contain the exact qualified ribbed prime-tower profile and per-plate 14.5/212 lower-left coordinates.".to_owned(),
        });
    }
    let mut prime_tower_contract_stable = true;
    for (snapshot, role) in snapshots[1..].iter().zip([
        U1GuiArtifactRole::FirstGuiSave,
        U1GuiArtifactRole::ReopenedGuiSave,
    ]) {
        if snapshot.prime_tower != snapshots[0].prime_tower {
            prime_tower_contract_stable = false;
            issues.push(U1GuiRoundTripIssue {
                code: U1GuiRoundTripIssueCode::PrimeTowerContractChanged,
                artifact: Some(role),
                message: "Qualified prime-tower settings or finite per-plate coordinates changed during a GUI save.".to_owned(),
            });
        }
    }

    let mut no_embedded_presets = true;
    for (snapshot, role) in snapshots.iter().zip(roles) {
        if !snapshot.embedded_presets.is_empty() {
            no_embedded_presets = false;
            issues.push(U1GuiRoundTripIssue {
                code: U1GuiRoundTripIssueCode::EmbeddedPresetFound,
                artifact: Some(role),
                message: format!(
                    "Embedded preset entries are forbidden: {:?}.",
                    snapshot.embedded_presets
                ),
            });
        }
    }

    let candidate_ids_present = candidate_identify_ids_present(&analyses[0]);
    if !candidate_ids_present {
        issues.push(U1GuiRoundTripIssue {
            code: U1GuiRoundTripIssueCode::MissingCandidateIdentifyId,
            artifact: Some(U1GuiArtifactRole::WriterCandidate),
            message: "Every candidate instance must retain a source identify_id.".to_owned(),
        });
    }
    let candidate_ids_positive_and_unique =
        candidate_ids_present && candidate_identify_ids_positive_and_unique(&analyses[0]);
    if candidate_ids_present && !candidate_ids_positive_and_unique {
        issues.push(U1GuiRoundTripIssue {
            code: U1GuiRoundTripIssueCode::InvalidCandidateIdentifyId,
            artifact: Some(U1GuiArtifactRole::WriterCandidate),
            message: "Writer-candidate identify_id values must be positive and globally unique across the artifact.".to_owned(),
        });
    }

    let source_identify_ids_preserved = compare_source_identify_ids(&source_analysis, &analyses[0]);
    if !source_identify_ids_preserved {
        issues.push(U1GuiRoundTripIssue {
            code: U1GuiRoundTripIssueCode::SourceIdentifyIdMismatch,
            artifact: Some(U1GuiArtifactRole::WriterCandidate),
            message: "Candidate identify_id values do not match the available source instances."
                .to_owned(),
        });
    }

    let mut gui_ids_valid = true;
    for (index, role) in [1_usize, 2].into_iter().zip([
        U1GuiArtifactRole::FirstGuiSave,
        U1GuiArtifactRole::ReopenedGuiSave,
    ]) {
        if !gui_ids_are_valid(&analyses[index]) {
            gui_ids_valid = false;
            issues.push(U1GuiRoundTripIssue {
                code: U1GuiRoundTripIssueCode::InvalidGuiId,
                artifact: Some(role),
                message: "GUI object, plate, instance, and identify IDs must be positive, unique, and internally consistent.".to_owned(),
            });
        }
    }

    let objects_stable = snapshots[1].geometry_resource_multiset
        == snapshots[0].geometry_resource_multiset
        && snapshots[2].geometry_resource_multiset == snapshots[0].geometry_resource_multiset
        && snapshots[1].object_multiset == snapshots[0].object_multiset
        && snapshots[2].object_multiset == snapshots[0].object_multiset;
    if !objects_stable {
        issues.push(U1GuiRoundTripIssue {
            code: U1GuiRoundTripIssueCode::ObjectOrPartBijectionChanged,
            artifact: None,
            message: "Object/part geometry, bounds, or extruder assignments do not form the same semantic multiset.".to_owned(),
        });
    }

    let instances_stable = snapshots[1].plate_multiset == snapshots[0].plate_multiset
        && snapshots[2].plate_multiset == snapshots[0].plate_multiset;
    if !instances_stable {
        issues.push(U1GuiRoundTripIssue {
            code: U1GuiRoundTripIssueCode::InstanceBijectionChanged,
            artifact: None,
            message: "Instance transforms, bounds, object identity, or plate membership changed across the three files.".to_owned(),
        });
    }

    issues.sort();
    issues.dedup();
    let checks = U1GuiRoundTripChecks {
        candidate_structurally_valid: structural[0].is_valid,
        first_gui_save_structurally_valid: structural[1].is_valid,
        reopened_gui_save_structurally_valid: structural[2].is_valid,
        exact_snapmaker_orca_2_3_5_u1_identity: exact_u1_identity,
        t1_t4_profile_identity_stable: profile_stable,
        plate_filament_map_stable: filament_map_stable,
        candidate_stage_b_target_globals_exact,
        stage_b_target_globals_stable_across_gui_saves: stage_b_target_globals_stable,
        candidate_prime_tower_contract_exact,
        prime_tower_contract_stable_across_gui_saves: prime_tower_contract_stable,
        no_embedded_presets,
        candidate_identify_ids_present: candidate_ids_present,
        candidate_identify_ids_positive_and_unique: candidate_ids_positive_and_unique,
        writer_candidate_source_identify_ids_preserved: source_identify_ids_preserved,
        gui_ids_positive_unique_and_consistent: gui_ids_valid,
        object_and_part_bijection_stable: objects_stable,
        instance_bijection_stable: instances_stable,
    };
    let source_sha256 = source_analysis.input.sha256;
    let artifacts = roles
        .into_iter()
        .zip(analyses)
        .zip(structural)
        .zip(snapshots)
        .map(|(((role, analysis), validation), snapshot)| {
            artifact_summary(role, analysis, validation, Some(snapshot))
        })
        .collect();

    Ok(U1GuiRoundTripReport {
        schema_version: 1,
        adapter_id: U1_DIRECT_ADAPTER_ID,
        application_version: SUPPORTED_ORCA_VERSION,
        is_valid: issues.is_empty(),
        source_sha256,
        artifacts,
        checks,
        issues,
    })
}

fn analyze(path: &Path) -> Result<ProjectAnalysis, U1GuiRoundTripError> {
    analyze_project(path).map_err(|source| U1GuiRoundTripError::Analyze {
        path: path.to_path_buf(),
        source,
    })
}

fn structurally_validate(
    path: &Path,
    policy: &OutputValidationPolicy,
) -> Result<OutputValidationReport, U1GuiRoundTripError> {
    validate_staged_output(path, policy).map_err(|source| U1GuiRoundTripError::Structural {
        path: path.to_path_buf(),
        source,
    })
}

pub(crate) fn verify_analysis_identity(
    path: &Path,
    analysis: &ProjectAnalysis,
) -> Result<(), U1GuiRoundTripError> {
    let mut file = File::open(path).map_err(|source| U1GuiRoundTripError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    let metadata = file
        .metadata()
        .map_err(|source| U1GuiRoundTripError::Open {
            path: path.to_path_buf(),
            source,
        })?;
    if !metadata.is_file() || metadata.len() != analysis.input.byte_size {
        return Err(U1GuiRoundTripError::InputChanged {
            path: path.to_path_buf(),
        });
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 256 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|source| U1GuiRoundTripError::Open {
                path: path.to_path_buf(),
                source,
            })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual != analysis.input.sha256 {
        return Err(U1GuiRoundTripError::InputChanged {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

fn early_structural_report(
    analyses: &[ProjectAnalysis],
    structural: Vec<OutputValidationReport>,
    source: ProjectAnalysis,
    mut issues: Vec<U1GuiRoundTripIssue>,
) -> U1GuiRoundTripReport {
    issues.sort();
    issues.dedup();
    let roles = [
        U1GuiArtifactRole::WriterCandidate,
        U1GuiArtifactRole::FirstGuiSave,
        U1GuiArtifactRole::ReopenedGuiSave,
    ];
    let artifacts = roles
        .into_iter()
        .zip(analyses.iter().cloned())
        .zip(structural.iter().cloned())
        .map(|((role, analysis), validation)| artifact_summary(role, analysis, validation, None))
        .collect();
    U1GuiRoundTripReport {
        schema_version: 1,
        adapter_id: U1_DIRECT_ADAPTER_ID,
        application_version: SUPPORTED_ORCA_VERSION,
        is_valid: false,
        source_sha256: source.input.sha256,
        artifacts,
        checks: U1GuiRoundTripChecks {
            candidate_structurally_valid: structural[0].is_valid,
            first_gui_save_structurally_valid: structural[1].is_valid,
            reopened_gui_save_structurally_valid: structural[2].is_valid,
            exact_snapmaker_orca_2_3_5_u1_identity: false,
            t1_t4_profile_identity_stable: false,
            plate_filament_map_stable: false,
            candidate_stage_b_target_globals_exact: false,
            stage_b_target_globals_stable_across_gui_saves: false,
            candidate_prime_tower_contract_exact: false,
            prime_tower_contract_stable_across_gui_saves: false,
            no_embedded_presets: false,
            candidate_identify_ids_present: false,
            candidate_identify_ids_positive_and_unique: false,
            writer_candidate_source_identify_ids_preserved: false,
            gui_ids_positive_unique_and_consistent: false,
            object_and_part_bijection_stable: false,
            instance_bijection_stable: false,
        },
        issues,
    }
}

fn artifact_summary(
    role: U1GuiArtifactRole,
    analysis: ProjectAnalysis,
    structural_validation: OutputValidationReport,
    snapshot: Option<SemanticSnapshot>,
) -> U1GuiArtifactSummary {
    U1GuiArtifactSummary {
        role,
        byte_size: analysis.input.byte_size,
        sha256: analysis.input.sha256,
        plate_count: analysis.summary.plate_count,
        object_count: analysis.summary.object_count,
        instance_count: analysis.summary.instance_count,
        part_count: analysis.summary.part_count,
        geometry_resource_count: snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.geometry_resource_count),
        semantic_sha256: snapshot.map(|snapshot| snapshot.semantic_sha256),
        structural_validation,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct U1ProfileIdentity {
    version: String,
    machine_profile: String,
    process_profile: String,
    filament_profile_names: [String; 4],
    filament_setting_ids: [String; 4],
    filament_colors: [String; 4],
    filament_materials: [String; 4],
}

impl U1ProfileIdentity {
    fn is_exact_u1_direct(&self) -> bool {
        self.version == SUPPORTED_ORCA_VERSION
            && self.machine_profile == U1_DIRECT_MACHINE_PROFILE
            && self.process_profile == crate::U1_DIRECT_PROCESS_PROFILE
            && self
                .filament_profile_names
                .iter()
                .chain(&self.filament_setting_ids)
                .chain(&self.filament_materials)
                .all(|value| !value.trim().is_empty())
            && self
                .filament_colors
                .iter()
                .all(|color| valid_filament_color(color))
    }
}

fn valid_filament_color(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('#') else {
        return false;
    };
    matches!(digits.len(), 6 | 8) && digits.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
struct FiniteNumber(u64);

impl FiniteNumber {
    fn new(value: f64) -> Option<Self> {
        value
            .is_finite()
            .then(|| Self(if value == 0.0 { 0.0 } else { value }.to_bits()))
    }

    fn is(self, expected: f64) -> bool {
        Self::new(expected) == Some(self)
    }

    fn is_zero(self) -> bool {
        self.is(0.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct StageBTargetGlobals {
    curr_bed_type: Option<String>,
    print_sequence: Option<String>,
    first_layer_print_sequence: Option<Vec<FiniteNumber>>,
    other_layers_print_sequence: Option<Vec<FiniteNumber>>,
    other_layers_print_sequence_nums: Option<FiniteNumber>,
    spiral_mode: Option<bool>,
    spiral_mode_smooth: Option<bool>,
    timelapse_type: Option<FiniteNumber>,
}

impl StageBTargetGlobals {
    fn from_json(settings: &serde_json::Map<String, Value>) -> Self {
        Self {
            curr_bed_type: contract_string(settings, "curr_bed_type"),
            print_sequence: contract_string(settings, "print_sequence"),
            first_layer_print_sequence: contract_number_array(
                settings,
                "first_layer_print_sequence",
            ),
            other_layers_print_sequence: contract_number_array(
                settings,
                "other_layers_print_sequence",
            ),
            other_layers_print_sequence_nums: contract_number(
                settings,
                "other_layers_print_sequence_nums",
                false,
            ),
            spiral_mode: contract_bool(settings, "spiral_mode"),
            spiral_mode_smooth: contract_bool(settings, "spiral_mode_smooth"),
            timelapse_type: contract_number(settings, "timelapse_type", false),
        }
    }

    fn is_exact(&self) -> bool {
        let zero_array = |values: &Option<Vec<FiniteNumber>>| {
            values.as_ref().is_some_and(|values| {
                !values.is_empty() && values.iter().all(|value| value.is_zero())
            })
        };
        self.curr_bed_type.as_deref() == Some("Textured PEI Plate")
            && self.print_sequence.as_deref() == Some("by layer")
            && zero_array(&self.first_layer_print_sequence)
            && zero_array(&self.other_layers_print_sequence)
            && self
                .other_layers_print_sequence_nums
                .is_some_and(FiniteNumber::is_zero)
            && self.spiral_mode == Some(false)
            && self.spiral_mode_smooth == Some(false)
            && self.timelapse_type.is_some_and(FiniteNumber::is_zero)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct PrimeTowerContract {
    enabled: Option<bool>,
    width_mm: Option<FiniteNumber>,
    brim_width_mm: Option<FiniteNumber>,
    prime_volume_mm3: Option<FiniteNumber>,
    cone_angle_degrees: Option<FiniteNumber>,
    extra_rib_length_mm: Option<FiniteNumber>,
    extra_spacing_percent: Option<FiniteNumber>,
    rotation_angle_degrees: Option<FiniteNumber>,
    wall_type: Option<String>,
    wipe_tower_x_mm: Option<Vec<FiniteNumber>>,
    wipe_tower_y_mm: Option<Vec<FiniteNumber>>,
}

impl PrimeTowerContract {
    fn from_json(settings: &serde_json::Map<String, Value>) -> Self {
        Self {
            enabled: contract_bool(settings, "enable_prime_tower"),
            width_mm: contract_number(settings, "prime_tower_width", false),
            brim_width_mm: contract_number(settings, "prime_tower_brim_width", false),
            prime_volume_mm3: contract_number(settings, "prime_volume", false),
            cone_angle_degrees: contract_number(settings, "wipe_tower_cone_angle", false),
            extra_rib_length_mm: contract_number(settings, "wipe_tower_extra_rib_length", false),
            extra_spacing_percent: contract_number(settings, "wipe_tower_extra_spacing", true),
            rotation_angle_degrees: contract_number(settings, "wipe_tower_rotation_angle", false),
            wall_type: contract_string(settings, "wipe_tower_wall_type"),
            wipe_tower_x_mm: contract_number_array(settings, "wipe_tower_x"),
            wipe_tower_y_mm: contract_number_array(settings, "wipe_tower_y"),
        }
    }

    fn is_exact(&self, plate_count: usize) -> bool {
        let exact_coordinates = |values: &Option<Vec<FiniteNumber>>, expected| {
            plate_count > 0
                && values.as_ref().is_some_and(|values| {
                    values.len() == plate_count && values.iter().all(|value| value.is(expected))
                })
        };
        self.enabled == Some(true)
            && self.width_mm.is_some_and(|value| value.is(30.0))
            && self.brim_width_mm.is_some_and(|value| value.is(5.0))
            && self.prime_volume_mm3.is_some_and(|value| value.is(45.0))
            && self.cone_angle_degrees.is_some_and(|value| value.is(15.0))
            && self.extra_rib_length_mm.is_some_and(|value| value.is(8.0))
            && self
                .extra_spacing_percent
                .is_some_and(|value| value.is(120.0))
            && self
                .rotation_angle_degrees
                .is_some_and(FiniteNumber::is_zero)
            && self.wall_type.as_deref() == Some("rib")
            && exact_coordinates(&self.wipe_tower_x_mm, EXPECTED_WIPE_TOWER_X_MM)
            && exact_coordinates(&self.wipe_tower_y_mm, EXPECTED_WIPE_TOWER_Y_MM)
    }
}

fn contract_string(settings: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    settings.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn contract_number(
    settings: &serde_json::Map<String, Value>,
    key: &str,
    allow_percent: bool,
) -> Option<FiniteNumber> {
    settings
        .get(key)
        .and_then(|value| finite_number(value, allow_percent))
}

fn contract_number_array(
    settings: &serde_json::Map<String, Value>,
    key: &str,
) -> Option<Vec<FiniteNumber>> {
    settings
        .get(key)
        .and_then(Value::as_array)
        .and_then(|values| {
            values
                .iter()
                .map(|value| finite_number(value, false))
                .collect::<Option<Vec<_>>>()
        })
}

fn finite_number(value: &Value, allow_percent: bool) -> Option<FiniteNumber> {
    let value = match value {
        Value::Number(value) => value.as_f64()?,
        Value::String(value) => {
            let value = value.trim();
            let value = if allow_percent {
                value.strip_suffix('%').unwrap_or(value)
            } else {
                value
            };
            value.parse::<f64>().ok()?
        }
        _ => return None,
    };
    FiniteNumber::new(value)
}

fn contract_bool(settings: &serde_json::Map<String, Value>, key: &str) -> Option<bool> {
    match settings.get(key)? {
        Value::Bool(value) => Some(*value),
        Value::Number(value) => match value.as_i64()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        },
        Value::String(value) if value.eq_ignore_ascii_case("false") || value == "0" => Some(false),
        Value::String(value) if value.eq_ignore_ascii_case("true") || value == "1" => Some(true),
        _ => None,
    }
}

struct ProjectSettingsSemantics {
    profile: U1ProfileIdentity,
    target_globals: StageBTargetGlobals,
    prime_tower: PrimeTowerContract,
}

#[derive(Clone, Debug)]
struct SemanticSnapshot {
    profile: U1ProfileIdentity,
    target_globals: StageBTargetGlobals,
    prime_tower: PrimeTowerContract,
    geometry_resource_multiset: SemanticMultiset,
    object_multiset: SemanticMultiset,
    plate_multiset: SemanticMultiset,
    plate_maps: BTreeMap<u32, PlateFilamentMap>,
    plate_map_count: usize,
    embedded_presets: Vec<String>,
    geometry_resource_count: usize,
    semantic_sha256: String,
}

impl SemanticSnapshot {
    fn load(path: &Path, analysis: &ProjectAnalysis) -> Result<Self, U1GuiRoundTripError> {
        let project_settings = read_project_settings_semantics(path)?;
        let profile = project_settings.profile;
        let target_globals = project_settings.target_globals;
        let prime_tower = project_settings.prime_tower;
        let plate_maps = read_plate_filament_maps(path)?;
        let analyzed_plate_ids = analysis
            .plates
            .iter()
            .map(|plate| plate.id)
            .collect::<BTreeSet<_>>();
        let mapped_plate_ids = plate_maps.keys().copied().collect::<BTreeSet<_>>();
        if analyzed_plate_ids != mapped_plate_ids {
            return Err(U1GuiRoundTripError::Semantic {
                path: path.to_path_buf(),
                message: format!(
                    "filament-map plate IDs {mapped_plate_ids:?} do not match analyzed plates {analyzed_plate_ids:?}"
                ),
            });
        }
        let embedded_presets = embedded_preset_entries(path)?;
        let model = load_u1_semantic_model_snapshot(path, analysis)?;
        let geometry_resource_multiset = model.geometry_resource_multiset;
        let object_multiset = model.object_multiset;
        let plate_multiset = model.plate_multiset;
        let geometry_resource_count = model.geometry_resource_count;
        let semantic_sha256 = digest_serializable(&(
            &profile,
            &target_globals,
            &prime_tower,
            &geometry_resource_multiset,
            &object_multiset,
            &plate_multiset,
        ))?;
        Ok(Self {
            profile,
            target_globals,
            prime_tower,
            geometry_resource_multiset,
            object_multiset,
            plate_multiset,
            plate_map_count: plate_maps.len(),
            plate_maps,
            embedded_presets,
            geometry_resource_count,
            semantic_sha256,
        })
    }

    fn has_exact_filament_maps(&self) -> bool {
        !self.plate_maps.is_empty()
            && self.plate_maps.values().all(|map| {
                map.mode == EXPECTED_FILAMENT_MAP_MODE && map.mapping == EXPECTED_FILAMENT_MAP
            })
    }
}

/// ID-independent semantic model snapshot shared by the Direct and Full
/// Spectrum GUI round-trip validators. Orca may renumber process-local IDs,
/// but it may not change meshes, per-facet paint trees, object/part metadata,
/// transforms, printable bounds, or plate membership.
#[derive(Clone, Debug)]
pub(crate) struct U1SemanticModelSnapshot {
    pub(crate) geometry_resource_multiset: SemanticMultiset,
    pub(crate) object_multiset: SemanticMultiset,
    pub(crate) plate_multiset: SemanticMultiset,
    pub(crate) geometry_resource_count: usize,
    pub(crate) semantic_sha256: String,
    placements: Vec<PlacementPlate>,
    resources: BTreeMap<ResourceKey, RawResource>,
    resource_fingerprints: BTreeMap<ResourceKey, ResourceFingerprint>,
}

pub(crate) fn load_u1_semantic_model_snapshot(
    path: &Path,
    analysis: &ProjectAnalysis,
) -> Result<U1SemanticModelSnapshot, U1GuiRoundTripError> {
    let resources = read_resource_graph(path)?;
    let resource_hashes = fingerprint_resources(path, &resources)?;
    let geometry_resource_multiset =
        resource_hashes
            .values()
            .fold(SemanticMultiset::new(), |mut multiset, fingerprint| {
                *multiset.entry(fingerprint.sha256.clone()).or_insert(0) += 1;
                multiset
            });
    let (object_multiset, object_signatures) = semantic_objects(path, analysis, &resource_hashes)?;
    let (plate_multiset, placements) = semantic_plates(path, analysis, &object_signatures)?;
    let semantic_sha256 = digest_serializable(&(
        &geometry_resource_multiset,
        &object_multiset,
        &plate_multiset,
    ))?;
    Ok(U1SemanticModelSnapshot {
        geometry_resource_multiset,
        object_multiset,
        plate_multiset,
        geometry_resource_count: resource_hashes.len(),
        semantic_sha256,
        placements,
        resources,
        resource_fingerprints: resource_hashes,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ComparableResourceComponent {
    target_uuid: String,
    linear_transform: [i64; 9],
    anchor_in_parent: [i64; 3],
}

pub(crate) fn u1_semantic_geometry_graphs_equivalent(
    first_path: &Path,
    first: &U1SemanticModelSnapshot,
    second_path: &Path,
    second: &U1SemanticModelSnapshot,
) -> Result<bool, U1GuiRoundTripError> {
    if first.geometry_resource_multiset == second.geometry_resource_multiset {
        return Ok(true);
    }
    let first_unmatched = unmatched_resource_keys(first, second);
    let second_unmatched = unmatched_resource_keys(second, first);
    let Some(first_by_uuid) = resources_by_production_uuid(first, &first_unmatched) else {
        return Ok(false);
    };
    let Some(second_by_uuid) = resources_by_production_uuid(second, &second_unmatched) else {
        return Ok(false);
    };
    if first_by_uuid.keys().collect::<BTreeSet<_>>()
        != second_by_uuid.keys().collect::<BTreeSet<_>>()
    {
        return Ok(false);
    }

    for (uuid, first_key) in first_by_uuid {
        let second_key = second_by_uuid
            .get(&uuid)
            .expect("matching UUID key sets checked above");
        let first_resource = &first.resources[&first_key];
        let second_resource = &second.resources[second_key];
        let first_fingerprint = &first.resource_fingerprints[&first_key];
        let second_fingerprint = &second.resource_fingerprints[second_key];
        if first_fingerprint.sha256 == second_fingerprint.sha256 {
            continue;
        }

        match (&first_resource.mesh, &second_resource.mesh) {
            (Some(first_mesh), Some(second_mesh)) => {
                if first_mesh.vertex_count != second_mesh.vertex_count
                    || first_mesh.triangle_count != second_mesh.triangle_count
                    || first_mesh.paint_sha256 != second_mesh.paint_sha256
                    || first_mesh.vertex_count > MAX_REBASED_TEXT_MESH_VERTICES
                    || first_mesh.triangle_count > MAX_REBASED_TEXT_MESH_TRIANGLES
                    || !bounded_meshes_are_translation_equivalent(
                        first_path,
                        &first_key,
                        first_mesh,
                        second_path,
                        second_key,
                        second_mesh,
                    )?
                {
                    return Ok(false);
                }
            }
            (None, None) => {}
            _ => return Ok(false),
        }

        let Some(first_components) = comparable_components(first, &first_key, first_resource)
        else {
            return Ok(false);
        };
        let Some(second_components) = comparable_components(second, second_key, second_resource)
        else {
            return Ok(false);
        };
        if first_components != second_components {
            return Ok(false);
        }
    }
    Ok(true)
}

fn resources_by_production_uuid(
    snapshot: &U1SemanticModelSnapshot,
    keys: &[ResourceKey],
) -> Option<BTreeMap<String, ResourceKey>> {
    let mut by_uuid = BTreeMap::new();
    for key in keys {
        let resource = snapshot.resources.get(key)?;
        let uuid = resource.production_uuid.as_ref()?.clone();
        if by_uuid.insert(uuid, key.clone()).is_some() {
            return None;
        }
    }
    Some(by_uuid)
}

fn unmatched_resource_keys(
    snapshot: &U1SemanticModelSnapshot,
    other: &U1SemanticModelSnapshot,
) -> Vec<ResourceKey> {
    let mut remaining = other.resource_fingerprints.values().fold(
        BTreeMap::<&str, usize>::new(),
        |mut counts, fingerprint| {
            *counts.entry(fingerprint.sha256.as_str()).or_default() += 1;
            counts
        },
    );
    snapshot
        .resource_fingerprints
        .iter()
        .filter_map(|(key, fingerprint)| {
            let count = remaining
                .get_mut(fingerprint.sha256.as_str())
                .filter(|count| **count > 0);
            if let Some(count) = count {
                *count -= 1;
                None
            } else {
                Some(key.clone())
            }
        })
        .collect()
}

fn comparable_components(
    snapshot: &U1SemanticModelSnapshot,
    source_key: &ResourceKey,
    resource: &RawResource,
) -> Option<Vec<ComparableResourceComponent>> {
    let mut components = resource
        .components
        .iter()
        .map(|component| {
            let target = ResourceKey {
                path: component.path.as_deref().map_or_else(
                    || source_key.path.clone(),
                    |path| resolve_component_path(&source_key.path, path),
                ),
                id: component.object_id,
            };
            let target_resource = snapshot.resources.get(&target)?;
            let target_uuid = target_resource.production_uuid.clone()?;
            let target_fingerprint = snapshot.resource_fingerprints.get(&target)?;
            let anchor = component
                .transform
                .transform_point(target_fingerprint.anchor)?;
            Some(ComparableResourceComponent {
                target_uuid,
                linear_transform: quantized_linear_transform(component.transform),
                anchor_in_parent: quantized_point(anchor, GEOMETRY_FLOAT_SCALE),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    components.sort();
    Some(components)
}

#[derive(Clone, Debug)]
struct BoundedMeshPayload {
    vertices: Vec<[f64; 3]>,
    triangles: Vec<[u32; 3]>,
}

fn bounded_meshes_are_translation_equivalent(
    first_path: &Path,
    first_key: &ResourceKey,
    first_mesh: &MeshFingerprint,
    second_path: &Path,
    second_key: &ResourceKey,
    second_mesh: &MeshFingerprint,
) -> Result<bool, U1GuiRoundTripError> {
    let first = read_bounded_mesh_payload(first_path, first_key, first_mesh)?;
    let second = read_bounded_mesh_payload(second_path, second_key, second_mesh)?;
    Ok(bounded_mesh_payloads_are_translation_equivalent(
        &first, &second,
    ))
}

fn bounded_mesh_payloads_are_translation_equivalent(
    first: &BoundedMeshPayload,
    second: &BoundedMeshPayload,
) -> bool {
    if first.triangles != second.triangles
        || first.vertices.len() != second.vertices.len()
        || first.vertices.is_empty()
    {
        return false;
    }
    let translation = [
        first.vertices[0][0] - second.vertices[0][0],
        first.vertices[0][1] - second.vertices[0][1],
        first.vertices[0][2] - second.vertices[0][2],
    ];
    first
        .vertices
        .iter()
        .zip(&second.vertices)
        .all(|(first, second)| {
            (0..3).all(|axis| {
                ((first[axis] - second[axis]) - translation[axis]).abs()
                    <= TEXT_MESH_REBASE_TOLERANCE_MM
            })
        })
}

fn read_bounded_mesh_payload(
    package_path: &Path,
    key: &ResourceKey,
    expected: &MeshFingerprint,
) -> Result<BoundedMeshPayload, U1GuiRoundTripError> {
    let file = File::open(package_path).map_err(|source| U1GuiRoundTripError::Open {
        path: package_path.to_path_buf(),
        source,
    })?;
    let mut archive = ZipArchive::new(file).map_err(|source| U1GuiRoundTripError::Zip {
        path: package_path.to_path_buf(),
        source,
    })?;
    let entry = archive
        .by_name(&key.path)
        .map_err(|source| U1GuiRoundTripError::Zip {
            path: package_path.to_path_buf(),
            source,
        })?;
    if entry.size() > MAX_MODEL_BYTES {
        return Err(U1GuiRoundTripError::EntryTooLarge {
            path: package_path.to_path_buf(),
            entry: key.path.clone(),
            limit: MAX_MODEL_BYTES,
        });
    }
    let mut reader = Reader::from_reader(BufReader::new(entry));
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut inside_target = false;
    let mut vertices = Vec::with_capacity(expected.vertex_count as usize);
    let mut triangles = Vec::with_capacity(expected.triangle_count as usize);
    loop {
        let event =
            reader
                .read_event_into(&mut buffer)
                .map_err(|error| U1GuiRoundTripError::Xml {
                    path: package_path.to_path_buf(),
                    entry: key.path.clone(),
                    message: error.to_string(),
                })?;
        match event {
            Event::Start(event) if local_name(event.name().as_ref()) == b"object" => {
                let id = required_positive_u32_attribute(
                    &reader,
                    &event,
                    b"id",
                    package_path,
                    &key.path,
                )?;
                inside_target = id == key.id;
            }
            Event::Empty(event) if inside_target => match local_name(event.name().as_ref()) {
                b"vertex" => {
                    if vertices.len() >= MAX_REBASED_TEXT_MESH_VERTICES as usize {
                        return Ok(BoundedMeshPayload {
                            vertices: Vec::new(),
                            triangles: Vec::new(),
                        });
                    }
                    vertices.push([
                        required_f64_attribute(&reader, &event, b"x", package_path, &key.path)?,
                        required_f64_attribute(&reader, &event, b"y", package_path, &key.path)?,
                        required_f64_attribute(&reader, &event, b"z", package_path, &key.path)?,
                    ]);
                }
                b"triangle" => {
                    if triangles.len() >= MAX_REBASED_TEXT_MESH_TRIANGLES as usize {
                        return Ok(BoundedMeshPayload {
                            vertices: Vec::new(),
                            triangles: Vec::new(),
                        });
                    }
                    triangles.push([
                        required_u32_attribute(&reader, &event, b"v1", package_path, &key.path)?,
                        required_u32_attribute(&reader, &event, b"v2", package_path, &key.path)?,
                        required_u32_attribute(&reader, &event, b"v3", package_path, &key.path)?,
                    ]);
                }
                _ => {}
            },
            Event::End(event) if local_name(event.name().as_ref()) == b"object" => {
                if inside_target {
                    break;
                }
                inside_target = false;
            }
            Event::DocType(_) => {
                return Err(U1GuiRoundTripError::Xml {
                    path: package_path.to_path_buf(),
                    entry: key.path.clone(),
                    message: "DOCTYPE is forbidden".to_owned(),
                });
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if vertices.len() as u64 != expected.vertex_count
        || triangles.len() as u64 != expected.triangle_count
    {
        return Ok(BoundedMeshPayload {
            vertices: Vec::new(),
            triangles: Vec::new(),
        });
    }
    Ok(BoundedMeshPayload {
        vertices,
        triangles,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlateFilamentMap {
    mode: String,
    mapping: String,
}

fn read_project_settings_semantics(
    path: &Path,
) -> Result<ProjectSettingsSemantics, U1GuiRoundTripError> {
    let bytes = read_named_entry(path, PROJECT_SETTINGS_PATH, MAX_CONFIG_BYTES)?;
    let root: Value =
        serde_json::from_slice(&bytes).map_err(|source| U1GuiRoundTripError::Json {
            path: path.to_path_buf(),
            entry: PROJECT_SETTINGS_PATH,
            source,
        })?;
    let object = root
        .as_object()
        .ok_or_else(|| U1GuiRoundTripError::Semantic {
            path: path.to_path_buf(),
            message: "project settings root is not an object".to_owned(),
        })?;
    let profile = U1ProfileIdentity {
        version: required_string(path, object, "version")?,
        machine_profile: required_string(path, object, "printer_settings_id")?,
        process_profile: required_string(path, object, "print_settings_id")?,
        filament_profile_names: required_string_array4(path, object, "filament_settings_id")?,
        filament_setting_ids: required_string_array4(path, object, "filament_ids")?,
        filament_colors: required_string_array4(path, object, "filament_colour")?
            .map(|value| value.trim().to_ascii_uppercase()),
        filament_materials: required_string_array4(path, object, "filament_type")?,
    };
    Ok(ProjectSettingsSemantics {
        profile,
        target_globals: StageBTargetGlobals::from_json(object),
        prime_tower: PrimeTowerContract::from_json(object),
    })
}

fn required_string(
    path: &Path,
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<String, U1GuiRoundTripError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| U1GuiRoundTripError::Semantic {
            path: path.to_path_buf(),
            message: format!("project setting {key:?} must be a non-empty string"),
        })
}

fn required_string_array4(
    path: &Path,
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<[String; 4], U1GuiRoundTripError> {
    let values =
        object
            .get(key)
            .and_then(Value::as_array)
            .ok_or_else(|| U1GuiRoundTripError::Semantic {
                path: path.to_path_buf(),
                message: format!("project setting {key:?} must be a four-string array"),
            })?;
    if values.len() != 4 {
        return Err(U1GuiRoundTripError::Semantic {
            path: path.to_path_buf(),
            message: format!(
                "project setting {key:?} has {} entries instead of four",
                values.len()
            ),
        });
    }
    let values = values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| U1GuiRoundTripError::Semantic {
                    path: path.to_path_buf(),
                    message: format!("project setting {key:?} contains an empty value"),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    values
        .try_into()
        .map_err(|_| U1GuiRoundTripError::Semantic {
            path: path.to_path_buf(),
            message: format!("project setting {key:?} could not be normalized"),
        })
}

#[derive(Default)]
struct PendingPlateMap {
    id: Option<u32>,
    mode: Option<String>,
    mapping: Option<String>,
}

fn read_plate_filament_maps(
    path: &Path,
) -> Result<BTreeMap<u32, PlateFilamentMap>, U1GuiRoundTripError> {
    let bytes = read_named_entry(path, MODEL_SETTINGS_PATH, MAX_CONFIG_BYTES)?;
    let mut reader = Reader::from_reader(bytes.as_slice());
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    let mut plate_depth = None;
    let mut current: Option<PendingPlateMap> = None;
    let mut maps = BTreeMap::new();
    loop {
        let event =
            reader
                .read_event_into(&mut buffer)
                .map_err(|error| U1GuiRoundTripError::Xml {
                    path: path.to_path_buf(),
                    entry: MODEL_SETTINGS_PATH.to_owned(),
                    message: error.to_string(),
                })?;
        match event {
            Event::Start(event) => {
                if local_name(event.name().as_ref()) == b"plate" && current.is_none() {
                    plate_depth = Some(depth);
                    current = Some(PendingPlateMap::default());
                }
                depth += 1;
            }
            Event::Empty(event)
                if current.is_some() && local_name(event.name().as_ref()) == b"metadata" =>
            {
                let key = optional_attribute(&reader, &event, b"key", path, MODEL_SETTINGS_PATH)?;
                let value =
                    optional_attribute(&reader, &event, b"value", path, MODEL_SETTINGS_PATH)?;
                let current = current.as_mut().expect("checked above");
                match key.as_deref() {
                    Some("plater_id") => {
                        let id = value
                            .as_deref()
                            .and_then(|value| value.parse::<u32>().ok())
                            .filter(|id| *id > 0)
                            .ok_or_else(|| U1GuiRoundTripError::Semantic {
                                path: path.to_path_buf(),
                                message: "plate has no positive plater_id".to_owned(),
                            })?;
                        if current.id.replace(id).is_some() {
                            return Err(U1GuiRoundTripError::Semantic {
                                path: path.to_path_buf(),
                                message: "plate contains duplicate plater_id metadata".to_owned(),
                            });
                        }
                    }
                    Some("filament_map_mode") => {
                        if current.mode.replace(value.unwrap_or_default()).is_some() {
                            return Err(U1GuiRoundTripError::Semantic {
                                path: path.to_path_buf(),
                                message: "plate contains duplicate filament_map_mode metadata"
                                    .to_owned(),
                            });
                        }
                    }
                    Some("filament_maps") => {
                        let normalized = value
                            .unwrap_or_default()
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ");
                        if current.mapping.replace(normalized).is_some() {
                            return Err(U1GuiRoundTripError::Semantic {
                                path: path.to_path_buf(),
                                message: "plate contains duplicate filament_maps metadata"
                                    .to_owned(),
                            });
                        }
                    }
                    _ => {}
                }
            }
            Event::End(event) => {
                depth = depth.saturating_sub(1);
                if local_name(event.name().as_ref()) == b"plate"
                    && plate_depth == Some(depth)
                    && current.is_some()
                {
                    let current = current.take().expect("checked above");
                    plate_depth = None;
                    let id = current.id.ok_or_else(|| U1GuiRoundTripError::Semantic {
                        path: path.to_path_buf(),
                        message: "plate is missing plater_id metadata".to_owned(),
                    })?;
                    let contract = PlateFilamentMap {
                        mode: current.mode.unwrap_or_default(),
                        mapping: current.mapping.unwrap_or_default(),
                    };
                    if maps.insert(id, contract).is_some() {
                        return Err(U1GuiRoundTripError::Semantic {
                            path: path.to_path_buf(),
                            message: format!("model settings contain duplicate plate {id}"),
                        });
                    }
                }
            }
            Event::DocType(_) => {
                return Err(U1GuiRoundTripError::Xml {
                    path: path.to_path_buf(),
                    entry: MODEL_SETTINGS_PATH.to_owned(),
                    message: "DOCTYPE is forbidden".to_owned(),
                });
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(maps)
}

pub(crate) fn embedded_preset_entries(path: &Path) -> Result<Vec<String>, U1GuiRoundTripError> {
    let file = File::open(path).map_err(|source| U1GuiRoundTripError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    let archive = ZipArchive::new(file).map_err(|source| U1GuiRoundTripError::Zip {
        path: path.to_path_buf(),
        source,
    })?;
    let mut entries = archive
        .file_names()
        .filter(|name| is_embedded_preset_entry(name))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    entries.sort();
    entries.dedup();
    Ok(entries)
}

fn is_embedded_preset_entry(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    [
        "metadata/process_settings_",
        "metadata/filament_settings_",
        "metadata/machine_settings_",
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix) && path.ends_with(".config"))
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ResourceKey {
    path: String,
    id: u32,
}

#[derive(Clone, Debug)]
struct RawResource {
    production_uuid: Option<String>,
    mesh: Option<MeshFingerprint>,
    components: Vec<RawComponent>,
}

#[derive(Clone, Debug)]
struct RawComponent {
    path: Option<String>,
    object_id: u32,
    transform: Transform3mf,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MeshFingerprint {
    sha256: String,
    paint_sha256: String,
    vertex_count: u64,
    triangle_count: u64,
    #[serde(skip)]
    anchor: [f64; 3],
}

struct PendingResource {
    id: u32,
    object_depth: usize,
    production_uuid: Option<String>,
    has_mesh: bool,
    geometry: Sha256,
    paint: Sha256,
    mesh_anchor: Option<[f64; 3]>,
    vertex_count: u64,
    triangle_count: u64,
    components: Vec<RawComponent>,
}

impl PendingResource {
    fn new(id: u32, object_depth: usize, production_uuid: Option<String>) -> Self {
        let mut geometry = Sha256::new();
        geometry.update(b"u1-gui-semantic-mesh-v1\0");
        let mut paint = Sha256::new();
        paint.update(b"u1-gui-semantic-paint-v1\0");
        Self {
            id,
            object_depth,
            production_uuid,
            has_mesh: false,
            geometry,
            paint,
            mesh_anchor: None,
            vertex_count: 0,
            triangle_count: 0,
            components: Vec::new(),
        }
    }

    fn finish(self) -> RawResource {
        RawResource {
            production_uuid: self.production_uuid,
            mesh: self.has_mesh.then(|| MeshFingerprint {
                sha256: format!("{:x}", self.geometry.finalize()),
                paint_sha256: format!("{:x}", self.paint.finalize()),
                vertex_count: self.vertex_count,
                triangle_count: self.triangle_count,
                anchor: self.mesh_anchor.unwrap_or([0.0; 3]),
            }),
            components: self.components,
        }
    }
}

fn read_resource_graph(
    package_path: &Path,
) -> Result<BTreeMap<ResourceKey, RawResource>, U1GuiRoundTripError> {
    let file = File::open(package_path).map_err(|source| U1GuiRoundTripError::Open {
        path: package_path.to_path_buf(),
        source,
    })?;
    let mut archive = ZipArchive::new(file).map_err(|source| U1GuiRoundTripError::Zip {
        path: package_path.to_path_buf(),
        source,
    })?;
    let model_indices = (0..archive.len())
        .filter_map(|index| {
            let entry = archive.by_index(index).ok()?;
            let name = entry.name().to_owned();
            (name.starts_with("3D/") && name.to_ascii_lowercase().ends_with(".model"))
                .then_some((index, name))
        })
        .collect::<Vec<_>>();
    let mut resources = BTreeMap::new();
    for (index, model_path) in model_indices {
        let entry = archive
            .by_index(index)
            .map_err(|source| U1GuiRoundTripError::Zip {
                path: package_path.to_path_buf(),
                source,
            })?;
        if entry.size() > MAX_MODEL_BYTES {
            return Err(U1GuiRoundTripError::EntryTooLarge {
                path: package_path.to_path_buf(),
                entry: model_path,
                limit: MAX_MODEL_BYTES,
            });
        }
        parse_model_resources(package_path, &model_path, entry, &mut resources)?;
    }
    Ok(resources)
}

fn parse_model_resources<R: Read>(
    package_path: &Path,
    model_path: &str,
    input: R,
    resources: &mut BTreeMap<ResourceKey, RawResource>,
) -> Result<(), U1GuiRoundTripError> {
    let mut reader = Reader::from_reader(BufReader::new(input));
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    let mut unit_scale = 1.0_f64;
    let mut current: Option<PendingResource> = None;
    loop {
        let event =
            reader
                .read_event_into(&mut buffer)
                .map_err(|error| U1GuiRoundTripError::Xml {
                    path: package_path.to_path_buf(),
                    entry: model_path.to_owned(),
                    message: error.to_string(),
                })?;
        match event {
            Event::Start(event) => {
                let event_name = event.name();
                let name = local_name(event_name.as_ref());
                if name == b"model" {
                    unit_scale =
                        optional_attribute(&reader, &event, b"unit", package_path, model_path)?
                            .as_deref()
                            .map_or(Ok(1.0), model_unit_scale)
                            .map_err(|message| U1GuiRoundTripError::Semantic {
                                path: package_path.to_path_buf(),
                                message: format!("{model_path}: {message}"),
                            })?;
                } else if name == b"object" {
                    if current.is_some() {
                        return Err(U1GuiRoundTripError::Semantic {
                            path: package_path.to_path_buf(),
                            message: format!("{model_path} contains nested object resources"),
                        });
                    }
                    let id = required_positive_u32_attribute(
                        &reader,
                        &event,
                        b"id",
                        package_path,
                        model_path,
                    )?;
                    let production_uuid =
                        optional_attribute(&reader, &event, b"UUID", package_path, model_path)?;
                    current = Some(PendingResource::new(id, depth, production_uuid));
                } else if name == b"mesh" {
                    if let Some(current) = current.as_mut() {
                        current.has_mesh = true;
                    }
                } else if let Some(current) = current.as_mut() {
                    record_resource_child(
                        package_path,
                        model_path,
                        &reader,
                        &event,
                        name,
                        unit_scale,
                        current,
                    )?;
                }
                depth += 1;
            }
            Event::Empty(event) => {
                if let Some(current) = current.as_mut() {
                    let event_name = event.name();
                    let name = local_name(event_name.as_ref());
                    record_resource_child(
                        package_path,
                        model_path,
                        &reader,
                        &event,
                        name,
                        unit_scale,
                        current,
                    )?;
                }
            }
            Event::End(event) => {
                depth = depth.saturating_sub(1);
                if local_name(event.name().as_ref()) == b"object"
                    && current
                        .as_ref()
                        .is_some_and(|current| current.object_depth == depth)
                {
                    let current = current.take().expect("checked above");
                    let key = ResourceKey {
                        path: normalize_model_path(model_path),
                        id: current.id,
                    };
                    if resources.insert(key.clone(), current.finish()).is_some() {
                        return Err(U1GuiRoundTripError::Semantic {
                            path: package_path.to_path_buf(),
                            message: format!("duplicate model resource {}/{}", key.path, key.id),
                        });
                    }
                }
            }
            Event::DocType(_) => {
                return Err(U1GuiRoundTripError::Xml {
                    path: package_path.to_path_buf(),
                    entry: model_path.to_owned(),
                    message: "DOCTYPE is forbidden".to_owned(),
                });
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(())
}

fn record_resource_child<R: std::io::BufRead>(
    package_path: &Path,
    model_path: &str,
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    name: &[u8],
    unit_scale: f64,
    current: &mut PendingResource,
) -> Result<(), U1GuiRoundTripError> {
    match name {
        b"vertex" => {
            current.geometry.update(b"v");
            let mut point = [0.0; 3];
            for (index, axis) in [b"x".as_slice(), b"y".as_slice(), b"z".as_slice()]
                .into_iter()
                .enumerate()
            {
                point[index] =
                    required_f64_attribute(reader, event, axis, package_path, model_path)?
                        * unit_scale;
            }
            let anchor = *current.mesh_anchor.get_or_insert(point);
            for index in 0..3 {
                current.geometry.update(
                    quantized_float(point[index] - anchor[index], GEOMETRY_FLOAT_SCALE)
                        .to_le_bytes(),
                );
            }
            current.vertex_count = current.vertex_count.saturating_add(1);
        }
        b"triangle" => {
            current.geometry.update(b"t");
            for vertex in [b"v1".as_slice(), b"v2".as_slice(), b"v3".as_slice()] {
                let value =
                    required_u32_attribute(reader, event, vertex, package_path, model_path)?;
                current.geometry.update(value.to_le_bytes());
            }
            current.paint.update(b"t");
            for attribute_name in [b"paint_color".as_slice(), b"mmu_segmentation".as_slice()] {
                let Some(encoded) =
                    optional_attribute(reader, event, attribute_name, package_path, model_path)?
                else {
                    continue;
                };
                let tree = decode_paint_annotation(&encoded).map_err(|error| {
                    U1GuiRoundTripError::Semantic {
                        path: package_path.to_path_buf(),
                        message: format!(
                            "{model_path} contains an invalid {} annotation: {error}",
                            String::from_utf8_lossy(attribute_name)
                        ),
                    }
                })?;
                let tree =
                    serde_json::to_vec(&tree).map_err(|source| U1GuiRoundTripError::Semantic {
                        path: package_path.to_path_buf(),
                        message: format!(
                            "failed to canonicalize a paint annotation in {model_path}: {source}"
                        ),
                    })?;
                current.paint.update(attribute_name);
                current.paint.update((tree.len() as u64).to_le_bytes());
                current.paint.update(tree);
            }
            current.triangle_count = current.triangle_count.saturating_add(1);
        }
        b"component" => {
            let object_id = required_positive_u32_attribute(
                reader,
                event,
                b"objectid",
                package_path,
                model_path,
            )?;
            let path = optional_attribute(reader, event, b"path", package_path, model_path)?;
            let mut transform =
                optional_attribute(reader, event, b"transform", package_path, model_path)?
                    .as_deref()
                    .map(parse_transform)
                    .transpose()
                    .map_err(|message| U1GuiRoundTripError::Semantic {
                        path: package_path.to_path_buf(),
                        message: format!("{model_path}: {message}"),
                    })?
                    .unwrap_or(Transform3mf::IDENTITY);
            transform.values[9] *= unit_scale;
            transform.values[10] *= unit_scale;
            transform.values[11] *= unit_scale;
            current.components.push(RawComponent {
                path,
                object_id,
                transform,
            });
        }
        _ => {}
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResourceFingerprintDocument<'a> {
    mesh: Option<&'a MeshFingerprint>,
    components: Vec<ResourceComponentFingerprint>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SemanticResourceFingerprintDocument {
    mesh_identity: Option<String>,
    components: Vec<ResourceComponentFingerprint>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResourceComponentFingerprint {
    resource_sha256: String,
    linear_transform: [i64; 9],
    anchor_offset: [i64; 3],
}

#[derive(Clone, Debug)]
struct ResourceFingerprint {
    sha256: String,
    semantic_identity: String,
    anchor: [f64; 3],
}

struct ResolvedResourceComponent {
    resource_sha256: String,
    resource_semantic_identity: String,
    linear_transform: [i64; 9],
    anchor: [f64; 3],
}

fn fingerprint_resources(
    package_path: &Path,
    resources: &BTreeMap<ResourceKey, RawResource>,
) -> Result<BTreeMap<ResourceKey, ResourceFingerprint>, U1GuiRoundTripError> {
    let mut memo = BTreeMap::new();
    let mut visiting = BTreeSet::new();
    for key in resources.keys() {
        fingerprint_resource(package_path, key, resources, &mut memo, &mut visiting)?;
    }
    Ok(memo)
}

fn fingerprint_resource(
    package_path: &Path,
    key: &ResourceKey,
    resources: &BTreeMap<ResourceKey, RawResource>,
    memo: &mut BTreeMap<ResourceKey, ResourceFingerprint>,
    visiting: &mut BTreeSet<ResourceKey>,
) -> Result<ResourceFingerprint, U1GuiRoundTripError> {
    if let Some(fingerprint) = memo.get(key) {
        return Ok(fingerprint.clone());
    }
    if !visiting.insert(key.clone()) {
        return Err(U1GuiRoundTripError::Semantic {
            path: package_path.to_path_buf(),
            message: format!(
                "component graph contains a cycle at {}/{}",
                key.path, key.id
            ),
        });
    }
    let resource = resources
        .get(key)
        .ok_or_else(|| U1GuiRoundTripError::Semantic {
            path: package_path.to_path_buf(),
            message: format!("model resource {}/{} is missing", key.path, key.id),
        })?;
    let mut resolved_components = Vec::with_capacity(resource.components.len());
    for component in &resource.components {
        let target = ResourceKey {
            path: component.path.as_deref().map_or_else(
                || key.path.clone(),
                |path| resolve_component_path(&key.path, path),
            ),
            id: component.object_id,
        };
        let target_fingerprint =
            fingerprint_resource(package_path, &target, resources, memo, visiting)?;
        let anchor = component
            .transform
            .transform_point(target_fingerprint.anchor)
            .ok_or_else(|| U1GuiRoundTripError::Semantic {
                path: package_path.to_path_buf(),
                message: format!(
                    "component transform produces a non-finite anchor at {}/{}",
                    key.path, key.id
                ),
            })?;
        resolved_components.push(ResolvedResourceComponent {
            resource_sha256: target_fingerprint.sha256,
            resource_semantic_identity: target_fingerprint.semantic_identity,
            linear_transform: quantized_linear_transform(component.transform),
            anchor,
        });
    }
    let anchor = resource
        .mesh
        .as_ref()
        .map(|mesh| mesh.anchor)
        .unwrap_or_else(|| {
            resolved_components
                .iter()
                .min_by_key(|component| {
                    (
                        component.resource_sha256.as_str(),
                        component.linear_transform,
                        quantized_point(component.anchor, GEOMETRY_FLOAT_SCALE),
                    )
                })
                .map_or([0.0; 3], |component| component.anchor)
        });
    let mut components = Vec::with_capacity(resolved_components.len());
    let mut semantic_components = Vec::with_capacity(resolved_components.len());
    for component in resolved_components {
        let anchor_offset = quantized_point(
            [
                component.anchor[0] - anchor[0],
                component.anchor[1] - anchor[1],
                component.anchor[2] - anchor[2],
            ],
            GEOMETRY_FLOAT_SCALE,
        );
        components.push(ResourceComponentFingerprint {
            resource_sha256: component.resource_sha256,
            linear_transform: component.linear_transform,
            anchor_offset,
        });
        semantic_components.push(ResourceComponentFingerprint {
            resource_sha256: component.resource_semantic_identity,
            linear_transform: component.linear_transform,
            anchor_offset,
        });
    }
    components.sort();
    semantic_components.sort();
    let sha256 = digest_serializable(&ResourceFingerprintDocument {
        mesh: resource.mesh.as_ref(),
        components,
    })?;
    let mesh_identity = resource.mesh.as_ref().map(|mesh| {
        resource.production_uuid.as_ref().map_or_else(
            || format!("mesh-sha256:{}:paint:{}", mesh.sha256, mesh.paint_sha256),
            |uuid| {
                format!(
                    "production-mesh:{uuid}:{}:{}:{}",
                    mesh.vertex_count, mesh.triangle_count, mesh.paint_sha256
                )
            },
        )
    });
    let semantic_identity = digest_serializable(&SemanticResourceFingerprintDocument {
        mesh_identity,
        components: semantic_components,
    })?;
    visiting.remove(key);
    let fingerprint = ResourceFingerprint {
        sha256,
        semantic_identity,
        anchor,
    };
    memo.insert(key.clone(), fingerprint.clone());
    Ok(fingerprint)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SemanticObjectDescriptor {
    resource_sha256: String,
    object_extruder_slot: Option<u16>,
    effective_slots: Vec<u16>,
    printable_bounds: Option<[i64; 6]>,
    parts: Vec<SemanticPartDescriptor>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
struct SemanticPartDescriptor {
    geometry_sha256: String,
    volume_type: String,
    printable: bool,
    extruder_slot: Option<u16>,
    inherited_extruder_slot: Option<u16>,
    painted_slots: Vec<u16>,
    effective_slots: Vec<u16>,
    object_space_bounds: Option<[i64; 6]>,
}

fn semantic_objects(
    package_path: &Path,
    analysis: &ProjectAnalysis,
    resource_hashes: &BTreeMap<ResourceKey, ResourceFingerprint>,
) -> Result<(SemanticMultiset, ObjectSignatureById), U1GuiRoundTripError> {
    let mut multiset = BTreeMap::new();
    let mut by_id = BTreeMap::new();
    for object in &analysis.objects {
        let object_path = object
            .source_model_path
            .as_deref()
            .map_or_else(|| MAIN_MODEL_PATH.to_owned(), normalize_model_path);
        let source_object_id = object.source_object_id.unwrap_or(object.id);
        let resource_sha256 = resource_hashes
            .get(&ResourceKey {
                path: object_path.clone(),
                id: source_object_id,
            })
            .map(|fingerprint| fingerprint.semantic_identity.clone())
            .ok_or_else(|| U1GuiRoundTripError::Semantic {
                path: package_path.to_path_buf(),
                message: format!(
                    "analyzed object {} has no geometry resource {object_path}/{source_object_id}",
                    object.id
                ),
            })?;
        let mut parts = object
            .parts
            .iter()
            .map(|part| {
                let part_path = part
                    .component_path
                    .as_deref()
                    .map_or_else(|| object_path.clone(), normalize_model_path);
                let geometry_sha256 = resource_hashes
                    .get(&ResourceKey {
                        path: part_path.clone(),
                        id: part.id,
                    })
                    .map(|fingerprint| fingerprint.semantic_identity.clone())
                    .ok_or_else(|| U1GuiRoundTripError::Semantic {
                        path: package_path.to_path_buf(),
                        message: format!(
                            "analyzed part {} has no geometry resource {part_path}/{}",
                            part.id, part.id
                        ),
                    })?;
                Ok(SemanticPartDescriptor {
                    geometry_sha256,
                    volume_type: volume_type_name(&part.volume_type),
                    printable: part.printable,
                    extruder_slot: part.extruder_slot,
                    inherited_extruder_slot: part.inherited_extruder_slot,
                    painted_slots: sorted_unique(&part.painted_slots),
                    effective_slots: sorted_unique(&part.effective_slots),
                    object_space_bounds: part.object_space_bounds.map(quantized_bounds),
                })
            })
            .collect::<Result<Vec<_>, U1GuiRoundTripError>>()?;
        parts.sort();
        let signature = digest_serializable(&SemanticObjectDescriptor {
            resource_sha256,
            object_extruder_slot: object.object_extruder_slot,
            effective_slots: sorted_unique(&object.effective_slots),
            printable_bounds: object.printable_bounds.map(quantized_bounds),
            parts,
        })?;
        *multiset.entry(signature.clone()).or_insert(0) += 1;
        if by_id.insert(object.id, signature).is_some() {
            return Err(U1GuiRoundTripError::Semantic {
                path: package_path.to_path_buf(),
                message: format!("analysis contains duplicate object ID {}", object.id),
            });
        }
    }
    Ok((multiset, by_id))
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
struct SemanticInstanceDescriptor {
    object_sha256: String,
    printable: bool,
    transform: [i64; 12],
    printable_bounds: Option<[i64; 6]>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SemanticPlateDescriptor {
    effective_slots: Vec<u16>,
    instances: Vec<SemanticInstanceDescriptor>,
}

#[derive(Clone, Debug)]
struct PlacementInstance {
    object_sha256: String,
    printable: bool,
    transform: Transform3mf,
    printable_bounds: Option<AxisAlignedBounds>,
}

#[derive(Clone, Debug)]
struct PlacementPlate {
    effective_slots: Vec<u16>,
    instances: Vec<PlacementInstance>,
}

fn semantic_plates(
    package_path: &Path,
    analysis: &ProjectAnalysis,
    object_signatures: &ObjectSignatureById,
) -> Result<(SemanticMultiset, Vec<PlacementPlate>), U1GuiRoundTripError> {
    let mut multiset = BTreeMap::new();
    let mut placements = Vec::with_capacity(analysis.plates.len());
    for plate in &analysis.plates {
        let resolved_instances = plate
            .instances
            .iter()
            .map(|instance| {
                let object_sha256 = object_signatures
                    .get(&instance.object_id)
                    .cloned()
                    .ok_or_else(|| U1GuiRoundTripError::Semantic {
                        path: package_path.to_path_buf(),
                        message: format!(
                            "plate {} references unknown analyzed object {}",
                            plate.id, instance.object_id
                        ),
                    })?;
                let transform = instance.transform.unwrap_or(Transform3mf::IDENTITY);
                Ok((
                    SemanticInstanceDescriptor {
                        object_sha256: object_sha256.clone(),
                        printable: instance.printable,
                        transform: quantized_placement_transform(transform),
                        printable_bounds: instance.printable_bounds.map(quantized_bounds),
                    },
                    PlacementInstance {
                        object_sha256,
                        printable: instance.printable,
                        transform,
                        printable_bounds: instance.printable_bounds,
                    },
                ))
            })
            .collect::<Result<Vec<_>, U1GuiRoundTripError>>()?;
        let (mut instances, placement_instances): (Vec<_>, Vec<_>) =
            resolved_instances.into_iter().unzip();
        instances.sort();
        let effective_slots = sorted_unique(&plate.effective_slots);
        let signature = digest_serializable(&SemanticPlateDescriptor {
            effective_slots: effective_slots.clone(),
            instances,
        })?;
        *multiset.entry(signature).or_insert(0) += 1;
        placements.push(PlacementPlate {
            effective_slots,
            instances: placement_instances,
        });
    }
    Ok((multiset, placements))
}

const GUI_LINEAR_TRANSFORM_TOLERANCE: f64 = 1.0e-9;
const GUI_PLACEMENT_TOLERANCE_MM: f64 = 1.0e-5;

pub(crate) fn u1_semantic_placements_equivalent(
    left: &U1SemanticModelSnapshot,
    right: &U1SemanticModelSnapshot,
) -> bool {
    placement_plates_equivalent(&left.placements, &right.placements)
}

fn placement_plates_equivalent(left: &[PlacementPlate], right: &[PlacementPlate]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut right_matches = vec![None; right.len()];
    (0..left.len()).all(|left_index| {
        let mut visited = vec![false; right.len()];
        augment_plate_match(left_index, left, right, &mut visited, &mut right_matches)
    })
}

fn augment_plate_match(
    left_index: usize,
    left: &[PlacementPlate],
    right: &[PlacementPlate],
    visited: &mut [bool],
    right_matches: &mut [Option<usize>],
) -> bool {
    for right_index in 0..right.len() {
        if visited[right_index]
            || !placement_plates_are_compatible(&left[left_index], &right[right_index])
        {
            continue;
        }
        visited[right_index] = true;
        if right_matches[right_index].is_none_or(|previous_left| {
            augment_plate_match(previous_left, left, right, visited, right_matches)
        }) {
            right_matches[right_index] = Some(left_index);
            return true;
        }
    }
    false
}

fn placement_plates_are_compatible(left: &PlacementPlate, right: &PlacementPlate) -> bool {
    left.effective_slots == right.effective_slots
        && placement_instances_equivalent(&left.instances, &right.instances)
}

fn placement_instances_equivalent(left: &[PlacementInstance], right: &[PlacementInstance]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut right_matches = vec![None; right.len()];
    (0..left.len()).all(|left_index| {
        let mut visited = vec![false; right.len()];
        augment_placement_match(left_index, left, right, &mut visited, &mut right_matches)
    })
}

fn augment_placement_match(
    left_index: usize,
    left: &[PlacementInstance],
    right: &[PlacementInstance],
    visited: &mut [bool],
    right_matches: &mut [Option<usize>],
) -> bool {
    for right_index in 0..right.len() {
        if visited[right_index]
            || !placement_instances_are_compatible(&left[left_index], &right[right_index])
        {
            continue;
        }
        visited[right_index] = true;
        if right_matches[right_index].is_none_or(|previous_left| {
            augment_placement_match(previous_left, left, right, visited, right_matches)
        }) {
            right_matches[right_index] = Some(left_index);
            return true;
        }
    }
    false
}

fn placement_instances_are_compatible(left: &PlacementInstance, right: &PlacementInstance) -> bool {
    left.object_sha256 == right.object_sha256
        && left.printable == right.printable
        && left
            .transform
            .values
            .iter()
            .zip(right.transform.values)
            .enumerate()
            .all(|(index, (left, right))| {
                let tolerance = if index < 9 {
                    GUI_LINEAR_TRANSFORM_TOLERANCE
                } else {
                    GUI_PLACEMENT_TOLERANCE_MM
                };
                (*left - right).abs() <= tolerance
            })
        && match (left.printable_bounds, right.printable_bounds) {
            (Some(left), Some(right)) => left
                .min
                .into_iter()
                .chain(left.max)
                .zip(right.min.into_iter().chain(right.max))
                .all(|(left, right)| (left - right).abs() <= GUI_PLACEMENT_TOLERANCE_MM),
            (None, None) => true,
            _ => false,
        }
}

fn candidate_identify_ids_present(analysis: &ProjectAnalysis) -> bool {
    let mut instances = analysis
        .plates
        .iter()
        .flat_map(|plate| &plate.instances)
        .peekable();
    instances.peek().is_some() && instances.all(|instance| instance.identify_id.is_some())
}

fn candidate_identify_ids_positive_and_unique(analysis: &ProjectAnalysis) -> bool {
    let mut identify_ids = BTreeSet::new();
    analysis
        .plates
        .iter()
        .flat_map(|plate| &plate.instances)
        .all(|instance| {
            instance
                .identify_id
                .is_some_and(|identify_id| identify_id > 0 && identify_ids.insert(identify_id))
        })
}

fn compare_source_identify_ids(source: &ProjectAnalysis, candidate: &ProjectAnalysis) -> bool {
    let mut source_ids = BTreeMap::new();
    for instance in source.plates.iter().flat_map(|plate| &plate.instances) {
        if source_ids
            .insert(
                (instance.object_id, instance.instance_id),
                instance.identify_id,
            )
            .is_some()
        {
            return false;
        }
    }
    let mut candidate_keys = BTreeSet::new();
    candidate
        .plates
        .iter()
        .flat_map(|plate| &plate.instances)
        .all(|instance| {
            candidate_keys.insert((instance.object_id, instance.instance_id))
                && matches!(
                    source_ids.get(&(instance.object_id, instance.instance_id)),
                    Some(Some(source_id)) if Some(*source_id) == instance.identify_id
                )
        })
}

pub(crate) fn gui_ids_are_valid(analysis: &ProjectAnalysis) -> bool {
    let mut plate_ids = BTreeSet::new();
    let mut object_ids = BTreeSet::new();
    let mut identify_ids = BTreeSet::new();
    let mut instance_keys = BTreeSet::new();
    if analysis
        .objects
        .iter()
        .any(|object| object.id == 0 || !object_ids.insert(object.id))
    {
        return false;
    }
    for object in &analysis.objects {
        let mut part_ids = BTreeSet::new();
        if object
            .parts
            .iter()
            .any(|part| part.id == 0 || !part_ids.insert(part.id))
        {
            return false;
        }
    }
    for plate in &analysis.plates {
        if plate.id == 0 || !plate_ids.insert(plate.id) {
            return false;
        }
        for instance in &plate.instances {
            let Some(identify_id) = instance.identify_id else {
                return false;
            };
            if identify_id == 0
                || !identify_ids.insert(identify_id)
                || !instance_keys.insert((instance.object_id, instance.instance_id))
                || !object_ids.contains(&instance.object_id)
            {
                return false;
            }
        }
    }
    true
}

fn sorted_unique(values: &[u16]) -> Vec<u16> {
    values
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn volume_type_name(volume_type: &VolumeType) -> String {
    match volume_type {
        VolumeType::NormalPart => "normal_part".to_owned(),
        VolumeType::NegativePart => "negative_part".to_owned(),
        VolumeType::Modifier => "modifier".to_owned(),
        VolumeType::SupportBlocker => "support_blocker".to_owned(),
        VolumeType::SupportEnforcer => "support_enforcer".to_owned(),
        VolumeType::Unknown(value) => format!("unknown:{value}"),
    }
}

fn quantized_placement_transform(transform: Transform3mf) -> [i64; 12] {
    std::array::from_fn(|index| {
        let scale = if index < 9 {
            TRANSFORM_FLOAT_SCALE
        } else {
            BOUNDS_FLOAT_SCALE
        };
        quantized_float(transform.values[index], scale)
    })
}

fn quantized_linear_transform(transform: Transform3mf) -> [i64; 9] {
    std::array::from_fn(|index| quantized_float(transform.values[index], TRANSFORM_FLOAT_SCALE))
}

fn quantized_point(point: [f64; 3], scale: f64) -> [i64; 3] {
    point.map(|value| quantized_float(value, scale))
}

fn quantized_bounds(bounds: AxisAlignedBounds) -> [i64; 6] {
    [
        quantized_float(bounds.min[0], BOUNDS_FLOAT_SCALE),
        quantized_float(bounds.min[1], BOUNDS_FLOAT_SCALE),
        quantized_float(bounds.min[2], BOUNDS_FLOAT_SCALE),
        quantized_float(bounds.max[0], BOUNDS_FLOAT_SCALE),
        quantized_float(bounds.max[1], BOUNDS_FLOAT_SCALE),
        quantized_float(bounds.max[2], BOUNDS_FLOAT_SCALE),
    ]
}

fn quantized_float(value: f64, scale: f64) -> i64 {
    let scaled = (value * scale).round();
    if scaled > i64::MAX as f64 {
        i64::MAX
    } else if scaled < i64::MIN as f64 {
        i64::MIN
    } else {
        scaled as i64
    }
}

fn read_named_entry(
    package_path: &Path,
    entry_name: &'static str,
    limit: u64,
) -> Result<Vec<u8>, U1GuiRoundTripError> {
    let file = File::open(package_path).map_err(|source| U1GuiRoundTripError::Open {
        path: package_path.to_path_buf(),
        source,
    })?;
    let mut archive = ZipArchive::new(file).map_err(|source| U1GuiRoundTripError::Zip {
        path: package_path.to_path_buf(),
        source,
    })?;
    let entry = match archive.by_name(entry_name) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => {
            return Err(U1GuiRoundTripError::MissingEntry {
                path: package_path.to_path_buf(),
                entry: entry_name,
            });
        }
        Err(source) => {
            return Err(U1GuiRoundTripError::Zip {
                path: package_path.to_path_buf(),
                source,
            });
        }
    };
    if entry.size() > limit {
        return Err(U1GuiRoundTripError::EntryTooLarge {
            path: package_path.to_path_buf(),
            entry: entry_name.to_owned(),
            limit,
        });
    }
    let mut bytes = Vec::with_capacity(usize::try_from(entry.size()).unwrap_or(0));
    entry
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| U1GuiRoundTripError::Open {
            path: package_path.to_path_buf(),
            source,
        })?;
    if bytes.len() as u64 > limit {
        return Err(U1GuiRoundTripError::EntryTooLarge {
            path: package_path.to_path_buf(),
            entry: entry_name.to_owned(),
            limit,
        });
    }
    Ok(bytes)
}

fn optional_attribute<R: std::io::BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    local: &[u8],
    package_path: &Path,
    entry: &str,
) -> Result<Option<String>, U1GuiRoundTripError> {
    let mut value = None;
    for attribute in event.attributes() {
        let attribute = attribute.map_err(|error| U1GuiRoundTripError::Xml {
            path: package_path.to_path_buf(),
            entry: entry.to_owned(),
            message: error.to_string(),
        })?;
        if local_name(attribute.key.as_ref()) != local {
            continue;
        }
        let decoded = attribute
            .decode_and_unescape_value(reader.decoder())
            .map_err(|error| U1GuiRoundTripError::Xml {
                path: package_path.to_path_buf(),
                entry: entry.to_owned(),
                message: error.to_string(),
            })?
            .into_owned();
        if value.replace(decoded).is_some() {
            return Err(U1GuiRoundTripError::Xml {
                path: package_path.to_path_buf(),
                entry: entry.to_owned(),
                message: format!("duplicate attribute {}", String::from_utf8_lossy(local)),
            });
        }
    }
    Ok(value)
}

fn required_u32_attribute<R: std::io::BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    local: &[u8],
    package_path: &Path,
    entry: &str,
) -> Result<u32, U1GuiRoundTripError> {
    optional_attribute(reader, event, local, package_path, entry)?
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| U1GuiRoundTripError::Semantic {
            path: package_path.to_path_buf(),
            message: format!(
                "{entry} contains no valid {} attribute",
                String::from_utf8_lossy(local)
            ),
        })
}

fn required_positive_u32_attribute<R: std::io::BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    local: &[u8],
    package_path: &Path,
    entry: &str,
) -> Result<u32, U1GuiRoundTripError> {
    let value = required_u32_attribute(reader, event, local, package_path, entry)?;
    if value == 0 {
        Err(U1GuiRoundTripError::Semantic {
            path: package_path.to_path_buf(),
            message: format!(
                "{entry} contains no positive {} attribute",
                String::from_utf8_lossy(local)
            ),
        })
    } else {
        Ok(value)
    }
}

fn required_f64_attribute<R: std::io::BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    local: &[u8],
    package_path: &Path,
    entry: &str,
) -> Result<f64, U1GuiRoundTripError> {
    optional_attribute(reader, event, local, package_path, entry)?
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .ok_or_else(|| U1GuiRoundTripError::Semantic {
            path: package_path.to_path_buf(),
            message: format!(
                "{entry} contains no finite {} attribute",
                String::from_utf8_lossy(local)
            ),
        })
}

fn parse_transform(value: &str) -> Result<Transform3mf, String> {
    let values = value
        .split_whitespace()
        .map(str::parse::<f64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| format!("invalid component transform {value:?}"))?;
    if values.len() != 12 || values.iter().any(|value| !value.is_finite()) {
        return Err(format!("invalid component transform {value:?}"));
    }
    Ok(Transform3mf {
        values: values
            .try_into()
            .map_err(|_| format!("invalid component transform {value:?}"))?,
    })
}

fn model_unit_scale(value: &str) -> Result<f64, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "micron" => Ok(0.001),
        "millimeter" => Ok(1.0),
        "centimeter" => Ok(10.0),
        "inch" => Ok(25.4),
        "foot" => Ok(304.8),
        "meter" => Ok(1000.0),
        _ => Err(format!("unsupported 3MF unit {value:?}")),
    }
}

fn normalize_model_path(value: &str) -> String {
    value.trim().trim_start_matches('/').to_owned()
}

fn resolve_component_path(source_model_path: &str, component_path: &str) -> String {
    let component_path = component_path.trim();
    if component_path.starts_with('/') || component_path.starts_with("3D/") {
        return normalize_model_path(component_path);
    }
    let parent = source_model_path
        .rsplit_once('/')
        .map_or("", |(parent, _)| parent);
    normalize_model_path(&format!("{parent}/{component_path}"))
}

fn local_name(value: &[u8]) -> &[u8] {
    value
        .iter()
        .rposition(|byte| *byte == b':')
        .map_or(value, |index| &value[index + 1..])
}

fn digest_serializable(value: &impl Serialize) -> Result<String, U1GuiRoundTripError> {
    let bytes = serde_json::to_vec(value).map_err(|source| U1GuiRoundTripError::Semantic {
        path: PathBuf::from("<semantic-comparison>"),
        message: format!("failed to serialize semantic fingerprint: {source}"),
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
 <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
 <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
 <Override PartName="/Metadata/project_settings.config" ContentType="application/json"/>
 <Override PartName="/Metadata/model_settings.config" ContentType="application/xml"/>
</Types>"#;

    const ROOT_RELATIONSHIPS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
 <Relationship Target="/3D/3dmodel.model" Id="model" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>"#;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum SettingsMutation {
        None,
        StageBPrintSequence,
        PrimeTowerWidth,
        WipeTowerX,
    }

    fn project_settings(color: &str, mutation: SettingsMutation) -> String {
        let print_sequence = if mutation == SettingsMutation::StageBPrintSequence {
            "by object"
        } else {
            "by layer"
        };
        let prime_tower_width = if mutation == SettingsMutation::PrimeTowerWidth {
            "31"
        } else {
            "30"
        };
        let wipe_tower_x = if mutation == SettingsMutation::WipeTowerX {
            "15"
        } else {
            "14.5"
        };
        format!(
            r##"{{
 "version":"2.3.5",
 "printer_model":"Snapmaker U1",
 "printer_variant":"0.4",
 "printer_settings_id":"Snapmaker U1 (0.4 nozzle)",
 "print_settings_id":"0.20 Standard @Snapmaker U1 (0.4 nozzle)",
 "nozzle_diameter":["0.4","0.4","0.4","0.4"],
 "curr_bed_type":"Textured PEI Plate",
 "print_sequence":"{print_sequence}",
 "first_layer_print_sequence":["0"],
 "other_layers_print_sequence":["0"],
 "other_layers_print_sequence_nums":"0",
 "spiral_mode":"0",
 "spiral_mode_smooth":"0",
 "timelapse_type":"0",
 "enable_prime_tower":"1",
 "prime_tower_width":"{prime_tower_width}",
 "prime_tower_brim_width":"5",
 "prime_volume":"45",
 "wipe_tower_cone_angle":"15",
 "wipe_tower_extra_rib_length":"8",
 "wipe_tower_extra_spacing":"120%",
 "wipe_tower_rotation_angle":"0",
 "wipe_tower_wall_type":"rib",
 "wipe_tower_x":["{wipe_tower_x}"],
 "wipe_tower_y":["212"],
 "filament_settings_id":["Cyan Profile","Magenta Profile","Yellow Profile","Grey Profile"],
 "filament_ids":["cyan-id","magenta-id","yellow-id","grey-id"],
 "filament_colour":["{color}","#D93B90","#F9ED3D","#9199A4"],
 "filament_type":["PLA","PLA","PLA","PLA"]
}}"##
        )
    }

    fn model(vertex_x: f64, two_instances: bool) -> String {
        let second_item = if two_instances {
            r#"<item objectid="1" transform="1 0 0 0 1 0 0 0 1 50 30 1"/>"#
        } else {
            ""
        };
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" unit="millimeter">
 <metadata name="Application">BambuStudio-2.3.5</metadata>
 <resources>
  <object id="1" type="model"><mesh><vertices>
   <vertex x="0" y="0" z="0"/><vertex x="{vertex_x}" y="0" z="0"/><vertex x="0" y="10" z="0"/>
  </vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object>
 </resources>
 <build><item objectid="1" transform="1 0 0 0 1 0 0 0 1 20 30 1"/>{second_item}</build>
</model>"#
        )
    }

    fn model_settings(
        identify_id: Option<u64>,
        second_identify_id: Option<u64>,
        filament_map: &str,
        extruder: u16,
    ) -> String {
        let identify_id = identify_id.map_or_else(String::new, |identify_id| {
            format!(r#"<metadata key="identify_id" value="{identify_id}"/>"#)
        });
        let second_instance = second_identify_id.map_or_else(String::new, |identify_id| {
            format!(
                r#"<model_instance><metadata key="object_id" value="1"/><metadata key="instance_id" value="1"/><metadata key="identify_id" value="{identify_id}"/></model_instance>"#
            )
        });
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
 <object id="1"><metadata key="name" value="Qualification Triangle"/><metadata key="extruder" value="{extruder}"/></object>
 <plate>
  <metadata key="plater_id" value="1"/><metadata key="plater_name" value="Qualification Plate"/>
  <metadata key="filament_map_mode" value="Auto For Flush"/><metadata key="filament_maps" value="{filament_map}"/>
  <model_instance><metadata key="object_id" value="1"/><metadata key="instance_id" value="0"/>{identify_id}</model_instance>
  {second_instance}
 </plate>
</config>"#
        )
    }

    struct ArchiveOptions<'a> {
        identify_id: u64,
        second_identify_id: Option<u64>,
        vertex_x: f64,
        cyan: &'a str,
        filament_map: &'a str,
        extruder: u16,
        embedded_preset: bool,
    }

    fn write_archive(path: &Path, options: ArchiveOptions<'_>) {
        write_archive_custom(path, options, SettingsMutation::None, true);
    }

    fn write_archive_custom(
        path: &Path,
        options: ArchiveOptions<'_>,
        settings_mutation: SettingsMutation,
        include_primary_identify_id: bool,
    ) {
        let file = File::create(path).unwrap();
        let mut writer = ZipWriter::new(file);
        let zip_options =
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let content_types = if options.embedded_preset {
            CONTENT_TYPES.replace(
                "</Types>",
                "<Override PartName=\"/Metadata/filament_settings_1.config\" ContentType=\"application/json\"/></Types>",
            )
        } else {
            CONTENT_TYPES.to_owned()
        };
        let mut entries = vec![
            ("[Content_Types].xml", content_types.into_bytes()),
            ("_rels/.rels", ROOT_RELATIONSHIPS.as_bytes().to_vec()),
            (
                "3D/3dmodel.model",
                model(options.vertex_x, options.second_identify_id.is_some()).into_bytes(),
            ),
            (
                PROJECT_SETTINGS_PATH,
                project_settings(options.cyan, settings_mutation).into_bytes(),
            ),
            (
                MODEL_SETTINGS_PATH,
                model_settings(
                    include_primary_identify_id.then_some(options.identify_id),
                    options.second_identify_id,
                    options.filament_map,
                    options.extruder,
                )
                .into_bytes(),
            ),
        ];
        if options.embedded_preset {
            entries.push(("Metadata/filament_settings_1.config", b"{}".to_vec()));
        }
        for (name, bytes) in entries {
            writer.start_file(name, zip_options).unwrap();
            writer.write_all(&bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    fn fixture_set(directory: &TempDir) -> [PathBuf; 4] {
        let paths = [
            directory.path().join("source.3mf"),
            directory.path().join("candidate.3mf"),
            directory.path().join("save-1.3mf"),
            directory.path().join("save-2.3mf"),
        ];
        for (index, path) in paths.iter().enumerate() {
            write_archive(
                path,
                ArchiveOptions {
                    identify_id: [41, 41, 101, 7][index],
                    second_identify_id: None,
                    vertex_x: 10.0,
                    cyan: "#08ABFB",
                    filament_map: EXPECTED_FILAMENT_MAP,
                    extruder: 1,
                    embedded_preset: false,
                },
            );
        }
        paths
    }

    #[test]
    fn accepts_semantic_bijection_when_gui_identify_ids_are_renumbered() {
        let directory = TempDir::new().unwrap();
        let [source, candidate, first, second] = fixture_set(&directory);

        let report = validate_u1_gui_round_trip(source, candidate, first, second).unwrap();

        assert!(report.is_valid, "{:#?}", report.issues);
        assert!(report.checks.writer_candidate_source_identify_ids_preserved);
        assert!(report.checks.candidate_identify_ids_positive_and_unique);
        assert!(report.checks.gui_ids_positive_unique_and_consistent);
        assert!(report.checks.object_and_part_bijection_stable);
        assert!(report.checks.instance_bijection_stable);
        assert!(report.checks.candidate_stage_b_target_globals_exact);
        assert!(report.checks.stage_b_target_globals_stable_across_gui_saves);
        assert!(report.checks.candidate_prime_tower_contract_exact);
        assert!(report.checks.prime_tower_contract_stable_across_gui_saves);
        assert_eq!(
            report.artifacts[0].semantic_sha256,
            report.artifacts[1].semantic_sha256
        );
        assert_eq!(
            report.artifacts[1].semantic_sha256,
            report.artifacts[2].semantic_sha256
        );
    }

    #[test]
    fn rejects_nonexact_candidate_stage_b_target_globals_even_when_stable() {
        let directory = TempDir::new().unwrap();
        let [source, candidate, first, second] = fixture_set(&directory);
        for (path, identify_id) in [(&candidate, 41), (&first, 101), (&second, 7)] {
            write_archive_custom(
                path,
                ArchiveOptions {
                    identify_id,
                    second_identify_id: None,
                    vertex_x: 10.0,
                    cyan: "#08ABFB",
                    filament_map: EXPECTED_FILAMENT_MAP,
                    extruder: 1,
                    embedded_preset: false,
                },
                SettingsMutation::StageBPrintSequence,
                true,
            );
        }

        let report = validate_u1_gui_round_trip(source, candidate, first, second).unwrap();

        assert!(!report.is_valid);
        assert!(!report.checks.candidate_stage_b_target_globals_exact);
        assert!(report.checks.stage_b_target_globals_stable_across_gui_saves);
        assert!(report.issues.iter().any(|issue| {
            issue.code == U1GuiRoundTripIssueCode::InvalidStageBTargetGlobals
                && issue.artifact == Some(U1GuiArtifactRole::WriterCandidate)
        }));
    }

    #[test]
    fn rejects_stage_b_target_global_drift_in_a_gui_save() {
        let directory = TempDir::new().unwrap();
        let [source, candidate, first, second] = fixture_set(&directory);
        write_archive_custom(
            &second,
            ArchiveOptions {
                identify_id: 7,
                second_identify_id: None,
                vertex_x: 10.0,
                cyan: "#08ABFB",
                filament_map: EXPECTED_FILAMENT_MAP,
                extruder: 1,
                embedded_preset: false,
            },
            SettingsMutation::StageBPrintSequence,
            true,
        );

        let report = validate_u1_gui_round_trip(source, candidate, first, second).unwrap();

        assert!(!report.is_valid);
        assert!(report.checks.candidate_stage_b_target_globals_exact);
        assert!(!report.checks.stage_b_target_globals_stable_across_gui_saves);
        assert!(report.issues.iter().any(|issue| {
            issue.code == U1GuiRoundTripIssueCode::StageBTargetGlobalsChanged
                && issue.artifact == Some(U1GuiArtifactRole::ReopenedGuiSave)
        }));
        assert_ne!(
            report.artifacts[0].semantic_sha256,
            report.artifacts[2].semantic_sha256
        );
    }

    #[test]
    fn rejects_nonexact_candidate_prime_tower_even_when_stable() {
        let directory = TempDir::new().unwrap();
        let [source, candidate, first, second] = fixture_set(&directory);
        for (path, identify_id) in [(&candidate, 41), (&first, 101), (&second, 7)] {
            write_archive_custom(
                path,
                ArchiveOptions {
                    identify_id,
                    second_identify_id: None,
                    vertex_x: 10.0,
                    cyan: "#08ABFB",
                    filament_map: EXPECTED_FILAMENT_MAP,
                    extruder: 1,
                    embedded_preset: false,
                },
                SettingsMutation::PrimeTowerWidth,
                true,
            );
        }

        let report = validate_u1_gui_round_trip(source, candidate, first, second).unwrap();

        assert!(!report.is_valid);
        assert!(!report.checks.candidate_prime_tower_contract_exact);
        assert!(report.checks.prime_tower_contract_stable_across_gui_saves);
        assert!(report.issues.iter().any(|issue| {
            issue.code == U1GuiRoundTripIssueCode::InvalidPrimeTowerContract
                && issue.artifact == Some(U1GuiArtifactRole::WriterCandidate)
        }));
    }

    #[test]
    fn rejects_prime_tower_coordinate_drift_in_a_gui_save() {
        let directory = TempDir::new().unwrap();
        let [source, candidate, first, second] = fixture_set(&directory);
        write_archive_custom(
            &second,
            ArchiveOptions {
                identify_id: 7,
                second_identify_id: None,
                vertex_x: 10.0,
                cyan: "#08ABFB",
                filament_map: EXPECTED_FILAMENT_MAP,
                extruder: 1,
                embedded_preset: false,
            },
            SettingsMutation::WipeTowerX,
            true,
        );

        let report = validate_u1_gui_round_trip(source, candidate, first, second).unwrap();

        assert!(!report.is_valid);
        assert!(report.checks.candidate_prime_tower_contract_exact);
        assert!(!report.checks.prime_tower_contract_stable_across_gui_saves);
        assert!(report.issues.iter().any(|issue| {
            issue.code == U1GuiRoundTripIssueCode::PrimeTowerContractChanged
                && issue.artifact == Some(U1GuiArtifactRole::ReopenedGuiSave)
        }));
        assert_ne!(
            report.artifacts[0].semantic_sha256,
            report.artifacts[2].semantic_sha256
        );
    }

    #[test]
    fn rejects_geometry_and_t1_t4_profile_drift() {
        let directory = TempDir::new().unwrap();
        let [source, candidate, first, second] = fixture_set(&directory);
        write_archive(
            &second,
            ArchiveOptions {
                identify_id: 7,
                second_identify_id: None,
                vertex_x: 11.0,
                cyan: "#00FFFF",
                filament_map: EXPECTED_FILAMENT_MAP,
                extruder: 1,
                embedded_preset: false,
            },
        );

        let report = validate_u1_gui_round_trip(source, candidate, first, second).unwrap();

        assert!(!report.is_valid);
        assert!(!report.checks.t1_t4_profile_identity_stable);
        assert!(!report.checks.object_and_part_bijection_stable);
        assert!(!report.checks.instance_bijection_stable);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| { issue.code == U1GuiRoundTripIssueCode::FilamentIdentityChanged })
        );
        assert!(
            report.issues.iter().any(|issue| {
                issue.code == U1GuiRoundTripIssueCode::ObjectOrPartBijectionChanged
            })
        );
    }

    #[test]
    fn rejects_object_extruder_assignment_drift() {
        let directory = TempDir::new().unwrap();
        let [source, candidate, first, second] = fixture_set(&directory);
        write_archive(
            &second,
            ArchiveOptions {
                identify_id: 7,
                second_identify_id: None,
                vertex_x: 10.0,
                cyan: "#08ABFB",
                filament_map: EXPECTED_FILAMENT_MAP,
                extruder: 2,
                embedded_preset: false,
            },
        );

        let report = validate_u1_gui_round_trip(source, candidate, first, second).unwrap();

        assert!(!report.is_valid);
        assert!(!report.checks.object_and_part_bijection_stable);
        assert!(
            report.issues.iter().any(|issue| {
                issue.code == U1GuiRoundTripIssueCode::ObjectOrPartBijectionChanged
            })
        );
    }

    #[test]
    fn rejects_source_identify_mismatch_and_nonpositive_gui_id() {
        let directory = TempDir::new().unwrap();
        let [source, candidate, first, second] = fixture_set(&directory);
        write_archive(
            &candidate,
            ArchiveOptions {
                identify_id: 42,
                second_identify_id: None,
                vertex_x: 10.0,
                cyan: "#08ABFB",
                filament_map: EXPECTED_FILAMENT_MAP,
                extruder: 1,
                embedded_preset: false,
            },
        );
        write_archive(
            &first,
            ArchiveOptions {
                identify_id: 0,
                second_identify_id: None,
                vertex_x: 10.0,
                cyan: "#08ABFB",
                filament_map: EXPECTED_FILAMENT_MAP,
                extruder: 1,
                embedded_preset: false,
            },
        );

        let report = validate_u1_gui_round_trip(source, candidate, first, second).unwrap();

        assert!(!report.is_valid);
        assert!(!report.checks.writer_candidate_source_identify_ids_preserved);
        assert!(!report.checks.gui_ids_positive_unique_and_consistent);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| { issue.code == U1GuiRoundTripIssueCode::SourceIdentifyIdMismatch })
        );
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.code == U1GuiRoundTripIssueCode::InvalidGuiId)
        );
    }

    #[test]
    fn rejects_candidate_identity_proof_when_source_identify_id_is_missing() {
        let directory = TempDir::new().unwrap();
        let [source, candidate, first, second] = fixture_set(&directory);
        write_archive_custom(
            &source,
            ArchiveOptions {
                identify_id: 41,
                second_identify_id: None,
                vertex_x: 10.0,
                cyan: "#08ABFB",
                filament_map: EXPECTED_FILAMENT_MAP,
                extruder: 1,
                embedded_preset: false,
            },
            SettingsMutation::None,
            false,
        );

        let report = validate_u1_gui_round_trip(source, candidate, first, second).unwrap();

        assert!(!report.is_valid);
        assert!(report.checks.candidate_identify_ids_present);
        assert!(!report.checks.writer_candidate_source_identify_ids_preserved);
        assert!(report.issues.iter().any(|issue| {
            issue.code == U1GuiRoundTripIssueCode::SourceIdentifyIdMismatch
                && issue.artifact == Some(U1GuiArtifactRole::WriterCandidate)
        }));
    }

    #[test]
    fn rejects_zero_candidate_identify_id_even_when_source_preserved() {
        let directory = TempDir::new().unwrap();
        let [source, candidate, first, second] = fixture_set(&directory);
        for path in [&source, &candidate] {
            write_archive(
                path,
                ArchiveOptions {
                    identify_id: 0,
                    second_identify_id: None,
                    vertex_x: 10.0,
                    cyan: "#08ABFB",
                    filament_map: EXPECTED_FILAMENT_MAP,
                    extruder: 1,
                    embedded_preset: false,
                },
            );
        }

        let report = validate_u1_gui_round_trip(source, candidate, first, second).unwrap();

        assert!(!report.is_valid);
        assert!(report.checks.candidate_identify_ids_present);
        assert!(report.checks.writer_candidate_source_identify_ids_preserved);
        assert!(!report.checks.candidate_identify_ids_positive_and_unique);
        assert!(report.issues.iter().any(|issue| {
            issue.code == U1GuiRoundTripIssueCode::InvalidCandidateIdentifyId
                && issue.artifact == Some(U1GuiArtifactRole::WriterCandidate)
        }));
    }

    #[test]
    fn rejects_duplicate_candidate_identify_ids_even_when_source_preserved() {
        let directory = TempDir::new().unwrap();
        let paths = [
            directory.path().join("source.3mf"),
            directory.path().join("candidate.3mf"),
            directory.path().join("save-1.3mf"),
            directory.path().join("save-2.3mf"),
        ];
        for (path, (first_id, second_id)) in
            paths.iter().zip([(41, 41), (41, 41), (101, 102), (7, 8)])
        {
            write_archive(
                path,
                ArchiveOptions {
                    identify_id: first_id,
                    second_identify_id: Some(second_id),
                    vertex_x: 10.0,
                    cyan: "#08ABFB",
                    filament_map: EXPECTED_FILAMENT_MAP,
                    extruder: 1,
                    embedded_preset: false,
                },
            );
        }

        let report = validate_u1_gui_round_trip(&paths[0], &paths[1], &paths[2], &paths[3])
            .expect("synthetic package must be readable");

        assert!(!report.is_valid);
        assert!(report.checks.candidate_identify_ids_present);
        assert!(report.checks.writer_candidate_source_identify_ids_preserved);
        assert!(!report.checks.candidate_identify_ids_positive_and_unique);
        assert!(report.checks.gui_ids_positive_unique_and_consistent);
        assert!(report.issues.iter().any(|issue| {
            issue.code == U1GuiRoundTripIssueCode::InvalidCandidateIdentifyId
                && issue.artifact == Some(U1GuiArtifactRole::WriterCandidate)
        }));
    }

    #[test]
    fn rejects_duplicate_gui_identify_ids_without_requiring_numeric_stability() {
        let directory = TempDir::new().unwrap();
        let paths = [
            directory.path().join("source.3mf"),
            directory.path().join("candidate.3mf"),
            directory.path().join("save-1.3mf"),
            directory.path().join("save-2.3mf"),
        ];
        for (path, (first_id, second_id)) in
            paths.iter().zip([(41, 42), (41, 42), (101, 101), (7, 8)])
        {
            write_archive(
                path,
                ArchiveOptions {
                    identify_id: first_id,
                    second_identify_id: Some(second_id),
                    vertex_x: 10.0,
                    cyan: "#08ABFB",
                    filament_map: EXPECTED_FILAMENT_MAP,
                    extruder: 1,
                    embedded_preset: false,
                },
            );
        }

        let report = validate_u1_gui_round_trip(&paths[0], &paths[1], &paths[2], &paths[3])
            .expect("synthetic package must be readable");

        assert!(!report.is_valid);
        assert!(!report.checks.gui_ids_positive_unique_and_consistent);
        assert!(report.checks.instance_bijection_stable);
        assert!(report.issues.iter().any(|issue| {
            issue.code == U1GuiRoundTripIssueCode::InvalidGuiId
                && issue.artifact == Some(U1GuiArtifactRole::FirstGuiSave)
        }));
    }

    #[test]
    fn rejects_plate_filament_map_drift() {
        let directory = TempDir::new().unwrap();
        let [source, candidate, first, second] = fixture_set(&directory);
        write_archive(
            &second,
            ArchiveOptions {
                identify_id: 7,
                second_identify_id: None,
                vertex_x: 10.0,
                cyan: "#08ABFB",
                filament_map: "1 1 1",
                extruder: 1,
                embedded_preset: false,
            },
        );

        let report = validate_u1_gui_round_trip(source, candidate, first, second).unwrap();

        assert!(!report.is_valid);
        assert!(!report.checks.plate_filament_map_stable);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.code == U1GuiRoundTripIssueCode::FilamentMapChanged)
        );
    }

    #[test]
    fn invokes_gui_artifact_allowlist_and_rejects_embedded_presets() {
        let directory = TempDir::new().unwrap();
        let [source, candidate, first, second] = fixture_set(&directory);
        write_archive(
            &first,
            ArchiveOptions {
                identify_id: 101,
                second_identify_id: None,
                vertex_x: 10.0,
                cyan: "#08ABFB",
                filament_map: EXPECTED_FILAMENT_MAP,
                extruder: 1,
                embedded_preset: true,
            },
        );

        let report = validate_u1_gui_round_trip(source, candidate, first, second).unwrap();

        assert!(!report.is_valid);
        assert!(!report.checks.first_gui_save_structurally_valid);
        assert!(!report.checks.no_embedded_presets);
        assert!(report.issues.iter().any(|issue| {
            issue.code == U1GuiRoundTripIssueCode::StructuralArtifactPolicyFailed
                && issue.artifact == Some(U1GuiArtifactRole::FirstGuiSave)
        }));
    }

    #[test]
    fn rejects_an_embedded_preset_in_the_writer_candidate() {
        let directory = TempDir::new().unwrap();
        let [source, candidate, first, second] = fixture_set(&directory);
        write_archive(
            &candidate,
            ArchiveOptions {
                identify_id: 41,
                second_identify_id: None,
                vertex_x: 10.0,
                cyan: "#08ABFB",
                filament_map: EXPECTED_FILAMENT_MAP,
                extruder: 1,
                embedded_preset: true,
            },
        );

        let report = validate_u1_gui_round_trip(source, candidate, first, second).unwrap();

        assert!(!report.is_valid);
        assert!(!report.checks.no_embedded_presets);
        assert!(report.issues.iter().any(|issue| {
            issue.code == U1GuiRoundTripIssueCode::EmbeddedPresetFound
                && issue.artifact == Some(U1GuiArtifactRole::WriterCandidate)
        }));
    }

    #[test]
    fn source_fixture_bytes_are_required_for_identify_id_proof() {
        let directory = TempDir::new().unwrap();
        let [source, candidate, first, second] = fixture_set(&directory);
        fs::remove_file(&source).unwrap();

        let error = validate_u1_gui_round_trip(source, candidate, first, second).unwrap_err();
        assert!(matches!(error, U1GuiRoundTripError::Analyze { .. }));
    }

    #[test]
    fn accepts_only_uniform_bounded_mesh_rebases() {
        let first = BoundedMeshPayload {
            vertices: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]],
            triangles: vec![[0, 1, 2]],
        };
        let mut second = BoundedMeshPayload {
            vertices: vec![
                [1.0, 2.0, 3.0],
                [11.0 + TEXT_MESH_REBASE_TOLERANCE_MM / 2.0, 2.0, 3.0],
                [1.0, 12.0, 3.0],
            ],
            triangles: vec![[0, 1, 2]],
        };

        assert!(bounded_mesh_payloads_are_translation_equivalent(
            &first, &second
        ));

        second.vertices[1][0] = 11.0 + TEXT_MESH_REBASE_TOLERANCE_MM * 2.0;
        assert!(!bounded_mesh_payloads_are_translation_equivalent(
            &first, &second
        ));

        second.vertices[1][0] = 11.0;
        second.triangles[0] = [0, 2, 1];
        assert!(!bounded_mesh_payloads_are_translation_equivalent(
            &first, &second
        ));
    }

    #[test]
    fn placement_matching_is_order_independent_but_rejects_real_drift() {
        fn instance(
            object_sha256: &str,
            translation_x: f64,
            bounds_delta: f64,
        ) -> PlacementInstance {
            let mut transform = Transform3mf::IDENTITY;
            transform.values[9] = translation_x;
            PlacementInstance {
                object_sha256: object_sha256.to_owned(),
                printable: true,
                transform,
                printable_bounds: Some(AxisAlignedBounds {
                    min: [translation_x + bounds_delta, 20.0, 0.0],
                    max: [translation_x + bounds_delta + 10.0, 30.0, 5.0],
                }),
            }
        }

        let left = vec![
            PlacementPlate {
                effective_slots: vec![1, 2],
                instances: vec![
                    instance("object-a", 10.0, 0.0),
                    instance("object-b", 40.0, 0.0),
                ],
            },
            PlacementPlate {
                effective_slots: vec![4],
                instances: vec![instance("object-c", 70.0, 0.0)],
            },
        ];
        let within_tolerance = GUI_PLACEMENT_TOLERANCE_MM / 2.0;
        let right = vec![
            PlacementPlate {
                effective_slots: vec![4],
                instances: vec![instance(
                    "object-c",
                    70.0 + within_tolerance,
                    -within_tolerance,
                )],
            },
            PlacementPlate {
                effective_slots: vec![1, 2],
                instances: vec![
                    instance("object-b", 40.0 - within_tolerance, within_tolerance),
                    instance("object-a", 10.0 + within_tolerance, -within_tolerance),
                ],
            },
        ];

        assert!(placement_plates_equivalent(&left, &right));

        let mut moved = right.clone();
        moved[0].instances[0].transform.values[9] += GUI_PLACEMENT_TOLERANCE_MM * 2.0;
        assert!(!placement_plates_equivalent(&left, &moved));

        let mut reassigned = right;
        reassigned[0].effective_slots = vec![3];
        assert!(!placement_plates_equivalent(&left, &reassigned));
    }
}
