//! Strict semantic validation for the Bambu Studio A1 mini qualification loop.
//!
//! Bambu Studio may renumber process-local object IDs and regenerate a small,
//! version-scoped set of preview/config entries when it saves a project. This
//! validator compares the stable geometry graph, part membership, transforms,
//! placements, machine/process identity, and physical one-spool contract.

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
    AnalysisError, AxisAlignedBounds, MAIN_MODEL_PATH, OutputIssueCode, OutputValidationPolicy,
    OutputValidationReport, ProjectAnalysis, SourceApplication, StagedOutputValidationError,
    Transform3mf, VolumeType, analyze_project, validate_staged_output,
};
use zip::ZipArchive;

use crate::{
    A1MINI_ADAPTER_ID, A1MINI_APPLICATION_VERSION, A1MINI_MACHINE_PROFILE, A1MINI_PROCESS_PROFILE,
    A1MiniMaterial,
};

const PROJECT_SETTINGS_PATH: &str = "Metadata/project_settings.config";
const MODEL_SETTINGS_PATH: &str = "Metadata/model_settings.config";
const MAX_CONFIG_BYTES: u64 = 64 * 1024 * 1024;
const MAX_MODEL_BYTES: u64 = 1024 * 1024 * 1024;
const TRANSFORM_FLOAT_SCALE: f64 = 1_000_000_000.0;
// Analyzer bounds can inherit the same text-mesh float rewrite. Keep them at
// the same ten-micrometre evidence resolution as the canonical geometry graph.
const BOUNDS_FLOAT_SCALE: f64 = 100_000.0;
// Bambu Studio 02.02.00.85 rebases editable text meshes and rewrites their
// float coordinates with residual noise below 0.06 micrometres. Ten-micrometre
// buckets remain substantially finer than the qualified 0.20 mm process while
// avoiding unstable rounding boundaries in that exact versioned normalization.
const GEOMETRY_FLOAT_SCALE: f64 = 100_000.0;
const TEXT_MESH_REBASE_TOLERANCE_MM: f64 = 0.0001;
const MAX_REBASED_TEXT_MESH_VERTICES: u64 = 10_000;
const MAX_REBASED_TEXT_MESH_TRIANGLES: u64 = 20_000;
const EXPECTED_FILAMENT_MAP_MODE: &str = "Auto For Flush";
const EXPECTED_FILAMENT_MAP: &str = "1";

type SemanticMultiset = BTreeMap<String, usize>;
type ObjectSignatureById = BTreeMap<u32, String>;

/// Role of one file in the three-step qualification sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum A1MiniRoundTripArtifactRole {
    WriterCandidate,
    FirstGuiSave,
    ReopenedGuiSave,
}

/// Stable issue categories returned by the A1 mini round-trip validator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum A1MiniRoundTripIssueCode {
    StructuralArtifactPolicyFailed,
    UnsupportedApplicationIdentity,
    UnsupportedMachineIdentity,
    UnsupportedProcessIdentity,
    InvalidSinglePlateContract,
    InvalidExternalSpoolContract,
    InvalidSingleSlotAssignment,
    EmbeddedPresetFound,
    ForbiddenStaleArtifact,
    MachineProcessIdentityChanged,
    MaterialIdentityChanged,
    GeometryChanged,
    ObjectPartMembershipChanged,
    PlacementChanged,
    InvalidInternalIds,
}

/// One deterministic finding in a round-trip report.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct A1MiniRoundTripIssue {
    pub code: A1MiniRoundTripIssueCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<A1MiniRoundTripArtifactRole>,
    pub message: String,
}

/// Per-file evidence retained by the qualification report.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct A1MiniRoundTripArtifactSummary {
    pub role: A1MiniRoundTripArtifactRole,
    pub path: PathBuf,
    pub byte_size: u64,
    pub sha256: String,
    pub plate_count: usize,
    pub object_count: usize,
    pub instance_count: usize,
    pub part_count: usize,
    pub geometry_resource_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub material: Option<A1MiniMaterial>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub geometry_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_part_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placement_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_sha256: Option<String>,
    pub structural_validation: OutputValidationReport,
}

/// Machine-readable boolean gates for the complete qualification loop.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct A1MiniRoundTripChecks {
    pub candidate_structurally_valid: bool,
    pub first_gui_save_structurally_valid: bool,
    pub reopened_gui_save_structurally_valid: bool,
    pub all_files_are_exact_a1_mini_projects: bool,
    pub one_plate_contract_stable: bool,
    pub one_external_spool_no_ams_stable: bool,
    pub single_slot_assignments_stable: bool,
    pub machine_process_identity_stable: bool,
    pub material_identity_stable: bool,
    pub no_embedded_presets: bool,
    pub no_forbidden_stale_artifacts: bool,
    pub internal_ids_valid: bool,
    pub geometry_stable: bool,
    pub object_part_membership_stable: bool,
    pub placement_stable: bool,
}

/// Typed result for candidate -> first save -> reopened save qualification.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct A1MiniRoundTripReport {
    pub schema_version: u32,
    pub adapter_id: &'static str,
    pub application_version: &'static str,
    pub is_valid: bool,
    pub artifacts: Vec<A1MiniRoundTripArtifactSummary>,
    pub checks: A1MiniRoundTripChecks,
    pub issues: Vec<A1MiniRoundTripIssue>,
}

#[derive(Debug, Error)]
pub enum A1MiniRoundTripError {
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
    MissingEntry { path: PathBuf, entry: String },
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
    #[error("invalid A1 mini round-trip data in {path}: {message}")]
    Semantic { path: PathBuf, message: String },
    #[error("qualification input changed while it was being validated: {path}")]
    InputChanged { path: PathBuf },
}

/// Validates a writer candidate and two successive Bambu Studio GUI saves.
///
/// The candidate must remain an unsliced native A1 mini project. GUI saves may
/// contain only the existing version-scoped regenerated preview/config dialect;
/// toolpaths, embedded presets, and unexpected derived artifacts remain errors.
pub fn validate_a1mini_round_trip(
    writer_candidate: impl AsRef<Path>,
    first_gui_save: impl AsRef<Path>,
    reopened_gui_save: impl AsRef<Path>,
) -> Result<A1MiniRoundTripReport, A1MiniRoundTripError> {
    let paths = [
        writer_candidate.as_ref(),
        first_gui_save.as_ref(),
        reopened_gui_save.as_ref(),
    ];
    let roles = [
        A1MiniRoundTripArtifactRole::WriterCandidate,
        A1MiniRoundTripArtifactRole::FirstGuiSave,
        A1MiniRoundTripArtifactRole::ReopenedGuiSave,
    ];
    let analyses = paths
        .iter()
        .map(|path| analyze(path))
        .collect::<Result<Vec<_>, _>>()?;
    let structural = vec![
        structurally_validate(paths[0], &OutputValidationPolicy::strict_unsliced())?,
        structurally_validate(
            paths[1],
            &OutputValidationPolicy::bambu_studio_2_2_0_85_gui_save([1]),
        )?,
        structurally_validate(
            paths[2],
            &OutputValidationPolicy::bambu_studio_2_2_0_85_gui_save([1]),
        )?,
    ];

    let mut issues = Vec::new();
    for (((role, path), report), analysis) in
        roles.iter().zip(paths).zip(&structural).zip(&analyses)
    {
        if !report.is_valid {
            push_issue(
                &mut issues,
                A1MiniRoundTripIssueCode::StructuralArtifactPolicyFailed,
                Some(*role),
                format!(
                    "{} failed the bounded GUI-save artifact policy with {} issue(s).",
                    path.display(),
                    report.issues.len()
                ),
            );
        }
        if has_forbidden_stale_artifact(report) {
            push_issue(
                &mut issues,
                A1MiniRoundTripIssueCode::ForbiddenStaleArtifact,
                Some(*role),
                "Project contains a toolpath, stale slice payload, or unexpected derived artifact.",
            );
        }
        verify_analysis_identity(path, analysis)?;
    }

    if structural.iter().any(|report| !report.is_valid) {
        return Ok(early_structural_report(
            paths, roles, analyses, structural, issues,
        ));
    }

    let snapshots = paths
        .iter()
        .zip(&analyses)
        .zip(roles)
        .map(|((path, analysis), role)| SemanticSnapshot::load(path, analysis, role))
        .collect::<Result<Vec<_>, _>>()?;
    for (path, analysis) in paths.iter().zip(&analyses) {
        verify_analysis_identity(path, analysis)?;
    }

    let target_flags = snapshots
        .iter()
        .zip(&analyses)
        .zip(roles)
        .map(|((snapshot, analysis), role)| {
            validate_target_contract(snapshot, analysis, role, &mut issues)
        })
        .collect::<Vec<_>>();
    let all_files_are_exact_a1_mini_projects = target_flags.iter().all(TargetFlags::all);
    let one_plate_contract_stable = target_flags.iter().all(|flags| flags.one_plate);
    let one_external_spool_no_ams_stable = target_flags.iter().all(|flags| flags.external_spool);
    let single_slot_assignments_stable = target_flags.iter().all(|flags| flags.single_slot);
    let no_embedded_presets = target_flags.iter().all(|flags| flags.no_embedded_presets);
    let no_forbidden_stale_artifacts = structural
        .iter()
        .all(|report| !has_forbidden_stale_artifact(report));
    let internal_ids_valid = analyses.iter().all(internal_ids_are_valid);
    if !internal_ids_valid {
        push_issue(
            &mut issues,
            A1MiniRoundTripIssueCode::InvalidInternalIds,
            None,
            "Object, part, plate, or instance IDs are zero, duplicated, or internally inconsistent.",
        );
    }

    let machine_process_identity_stable = snapshots[1].profile.machine_process_key()
        == snapshots[0].profile.machine_process_key()
        && snapshots[2].profile.machine_process_key() == snapshots[0].profile.machine_process_key();
    if !machine_process_identity_stable {
        push_issue(
            &mut issues,
            A1MiniRoundTripIssueCode::MachineProcessIdentityChanged,
            None,
            "A1 mini machine or 0.20 mm process identity changed across GUI saves.",
        );
    }
    let material_identity_stable = snapshots[1].profile.material_key()
        == snapshots[0].profile.material_key()
        && snapshots[2].profile.material_key() == snapshots[0].profile.material_key()
        && gui_filament_serialization_is_expected(
            &snapshots[0].profile,
            &snapshots[1].profile,
            &snapshots[2].profile,
        );
    if !material_identity_stable {
        push_issue(
            &mut issues,
            A1MiniRoundTripIssueCode::MaterialIdentityChanged,
            None,
            "External-spool profile, material, setting ID, or color changed across GUI saves.",
        );
    }
    let geometry_stable =
        geometry_graphs_equivalent(paths[0], &snapshots[0], paths[1], &snapshots[1])?
            && geometry_graphs_equivalent(paths[0], &snapshots[0], paths[2], &snapshots[2])?;
    if !geometry_stable {
        push_issue(
            &mut issues,
            A1MiniRoundTripIssueCode::GeometryChanged,
            None,
            "Mesh coordinates, triangle topology, component geometry, or resource membership changed.",
        );
    }
    let object_part_membership_stable = snapshots[1].object_multiset
        == snapshots[0].object_multiset
        && snapshots[2].object_multiset == snapshots[0].object_multiset;
    if !object_part_membership_stable {
        push_issue(
            &mut issues,
            A1MiniRoundTripIssueCode::ObjectPartMembershipChanged,
            None,
            "Object/part membership, volume role, transform, bounds, or slot semantics changed.",
        );
    }
    let placement_stable =
        placement_snapshots_equivalent(&snapshots[0].placements, &snapshots[1].placements)
            && placement_snapshots_equivalent(&snapshots[0].placements, &snapshots[2].placements);
    if !placement_stable {
        push_issue(
            &mut issues,
            A1MiniRoundTripIssueCode::PlacementChanged,
            None,
            "Plate membership, instance transforms, or printable bounds changed across GUI saves.",
        );
    }

    issues.sort();
    issues.dedup();
    let checks = A1MiniRoundTripChecks {
        candidate_structurally_valid: structural[0].is_valid,
        first_gui_save_structurally_valid: structural[1].is_valid,
        reopened_gui_save_structurally_valid: structural[2].is_valid,
        all_files_are_exact_a1_mini_projects,
        one_plate_contract_stable,
        one_external_spool_no_ams_stable,
        single_slot_assignments_stable,
        machine_process_identity_stable,
        material_identity_stable,
        no_embedded_presets,
        no_forbidden_stale_artifacts,
        internal_ids_valid,
        geometry_stable,
        object_part_membership_stable,
        placement_stable,
    };
    let artifacts = roles
        .into_iter()
        .zip(paths)
        .zip(analyses)
        .zip(structural)
        .zip(snapshots)
        .map(|((((role, path), analysis), validation), snapshot)| {
            artifact_summary(role, path, analysis, validation, Some(snapshot))
        })
        .collect();

    Ok(A1MiniRoundTripReport {
        schema_version: 1,
        adapter_id: A1MINI_ADAPTER_ID,
        application_version: A1MINI_APPLICATION_VERSION,
        is_valid: issues.is_empty(),
        artifacts,
        checks,
        issues,
    })
}

fn push_issue(
    issues: &mut Vec<A1MiniRoundTripIssue>,
    code: A1MiniRoundTripIssueCode,
    artifact: Option<A1MiniRoundTripArtifactRole>,
    message: impl Into<String>,
) {
    issues.push(A1MiniRoundTripIssue {
        code,
        artifact,
        message: message.into(),
    });
}

fn analyze(path: &Path) -> Result<ProjectAnalysis, A1MiniRoundTripError> {
    analyze_project(path).map_err(|source| A1MiniRoundTripError::Analyze {
        path: path.to_path_buf(),
        source,
    })
}

fn structurally_validate(
    path: &Path,
    policy: &OutputValidationPolicy,
) -> Result<OutputValidationReport, A1MiniRoundTripError> {
    validate_staged_output(path, policy).map_err(|source| A1MiniRoundTripError::Structural {
        path: path.to_path_buf(),
        source,
    })
}

fn verify_analysis_identity(
    path: &Path,
    analysis: &ProjectAnalysis,
) -> Result<(), A1MiniRoundTripError> {
    let mut file = File::open(path).map_err(|source| A1MiniRoundTripError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    let metadata = file
        .metadata()
        .map_err(|source| A1MiniRoundTripError::Open {
            path: path.to_path_buf(),
            source,
        })?;
    if !metadata.is_file() || metadata.len() != analysis.input.byte_size {
        return Err(A1MiniRoundTripError::InputChanged {
            path: path.to_path_buf(),
        });
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 256 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|source| A1MiniRoundTripError::Open {
                path: path.to_path_buf(),
                source,
            })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    if format!("{:x}", hasher.finalize()) != analysis.input.sha256 {
        return Err(A1MiniRoundTripError::InputChanged {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

fn has_forbidden_stale_artifact(report: &OutputValidationReport) -> bool {
    report.issues.iter().any(|issue| {
        matches!(
            issue.code,
            OutputIssueCode::ForbiddenSlicedArtifact
                | OutputIssueCode::StaleDerivedArtifact
                | OutputIssueCode::UnexpectedGuiSaveArtifact
                | OutputIssueCode::DanglingThumbnail
        )
    })
}

fn early_structural_report(
    paths: [&Path; 3],
    roles: [A1MiniRoundTripArtifactRole; 3],
    analyses: Vec<ProjectAnalysis>,
    structural: Vec<OutputValidationReport>,
    mut issues: Vec<A1MiniRoundTripIssue>,
) -> A1MiniRoundTripReport {
    issues.sort();
    issues.dedup();
    let no_forbidden_stale_artifacts = structural
        .iter()
        .all(|report| !has_forbidden_stale_artifact(report));
    let artifacts = roles
        .into_iter()
        .zip(paths)
        .zip(analyses)
        .zip(structural.iter().cloned())
        .map(|(((role, path), analysis), validation)| {
            artifact_summary(role, path, analysis, validation, None)
        })
        .collect();
    A1MiniRoundTripReport {
        schema_version: 1,
        adapter_id: A1MINI_ADAPTER_ID,
        application_version: A1MINI_APPLICATION_VERSION,
        is_valid: false,
        artifacts,
        checks: A1MiniRoundTripChecks {
            candidate_structurally_valid: structural[0].is_valid,
            first_gui_save_structurally_valid: structural[1].is_valid,
            reopened_gui_save_structurally_valid: structural[2].is_valid,
            all_files_are_exact_a1_mini_projects: false,
            one_plate_contract_stable: false,
            one_external_spool_no_ams_stable: false,
            single_slot_assignments_stable: false,
            machine_process_identity_stable: false,
            material_identity_stable: false,
            no_embedded_presets: false,
            no_forbidden_stale_artifacts,
            internal_ids_valid: false,
            geometry_stable: false,
            object_part_membership_stable: false,
            placement_stable: false,
        },
        issues,
    }
}

fn artifact_summary(
    role: A1MiniRoundTripArtifactRole,
    path: &Path,
    analysis: ProjectAnalysis,
    structural_validation: OutputValidationReport,
    snapshot: Option<SemanticSnapshot>,
) -> A1MiniRoundTripArtifactSummary {
    A1MiniRoundTripArtifactSummary {
        role,
        path: path.to_path_buf(),
        byte_size: analysis.input.byte_size,
        sha256: analysis.input.sha256,
        plate_count: analysis.summary.plate_count,
        object_count: analysis.summary.object_count,
        instance_count: analysis.summary.instance_count,
        part_count: analysis.summary.part_count,
        geometry_resource_count: snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.geometry_resource_count),
        material: snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.profile.material()),
        geometry_sha256: snapshot
            .as_ref()
            .map(|snapshot| snapshot.geometry_sha256.clone()),
        object_part_sha256: snapshot
            .as_ref()
            .map(|snapshot| snapshot.object_part_sha256.clone()),
        placement_sha256: snapshot
            .as_ref()
            .map(|snapshot| snapshot.placement_sha256.clone()),
        semantic_sha256: snapshot.map(|snapshot| snapshot.semantic_sha256),
        structural_validation,
    }
}

#[derive(Clone, Copy, Debug)]
struct TargetFlags {
    application: bool,
    machine: bool,
    process: bool,
    one_plate: bool,
    external_spool: bool,
    single_slot: bool,
    no_embedded_presets: bool,
}

impl TargetFlags {
    fn all(&self) -> bool {
        self.application
            && self.machine
            && self.process
            && self.one_plate
            && self.external_spool
            && self.single_slot
            && self.no_embedded_presets
    }
}

fn validate_target_contract(
    snapshot: &SemanticSnapshot,
    analysis: &ProjectAnalysis,
    role: A1MiniRoundTripArtifactRole,
    issues: &mut Vec<A1MiniRoundTripIssue>,
) -> TargetFlags {
    let application = analysis.source.application == SourceApplication::BambuStudio
        && analysis.source.application_version.as_deref() == Some(A1MINI_APPLICATION_VERSION)
        && snapshot.profile.version == A1MINI_APPLICATION_VERSION;
    if !application {
        push_issue(
            issues,
            A1MiniRoundTripIssueCode::UnsupportedApplicationIdentity,
            Some(role),
            "Project is not an exact Bambu Studio 02.02.00.85 project.",
        );
    }
    let machine = snapshot.profile.has_exact_machine()
        && analysis.printer.model.as_deref() == Some("Bambu Lab A1 mini")
        && analysis.printer.variant.as_deref() == Some("0.4")
        && analysis.printer.nozzle_diameters_mm == [0.4];
    if !machine {
        push_issue(
            issues,
            A1MiniRoundTripIssueCode::UnsupportedMachineIdentity,
            Some(role),
            "Project does not use the exact Bambu Lab A1 mini 0.4 mm machine contract.",
        );
    }
    let process = snapshot.profile.process_profile == A1MINI_PROCESS_PROFILE
        && analysis.process.name.as_deref() == Some(A1MINI_PROCESS_PROFILE);
    if !process {
        push_issue(
            issues,
            A1MiniRoundTripIssueCode::UnsupportedProcessIdentity,
            Some(role),
            "Project does not use the exact qualified 0.20 mm A1 mini process.",
        );
    }
    let one_plate = analysis.plates.len() == 1
        && analysis.plates[0].id == 1
        && !analysis.plates[0].instances.is_empty()
        && snapshot.plate_maps.len() == 1
        && snapshot.plate_maps.contains_key(&1);
    if !one_plate {
        push_issue(
            issues,
            A1MiniRoundTripIssueCode::InvalidSinglePlateContract,
            Some(role),
            "A1 mini artifact must contain exactly target plate 1 with at least one instance.",
        );
    }
    let external_spool = snapshot.profile.has_exact_external_spool()
        && snapshot.plate_maps.values().all(PlateFilamentMap::is_exact)
        && analysis.filaments.len() == 1
        && analysis.filaments[0].slot == 1
        && snapshot.profile.has_exact_material(role);
    if !external_spool {
        push_issue(
            issues,
            A1MiniRoundTripIssueCode::InvalidExternalSpoolContract,
            Some(role),
            "Project must use one qualified PLA/PETG external spool and no AMS or mixed filament.",
        );
    }
    let single_slot = analysis
        .plates
        .iter()
        .all(|plate| only_slot_one(&plate.effective_slots))
        && analysis.objects.iter().all(|object| {
            only_slot_one(&object.effective_slots)
                && object
                    .parts
                    .iter()
                    .all(|part| only_slot_one(&part.effective_slots))
        });
    if !single_slot {
        push_issue(
            issues,
            A1MiniRoundTripIssueCode::InvalidSingleSlotAssignment,
            Some(role),
            "Geometry retains a logical filament assignment other than external slot 1.",
        );
    }
    let no_embedded_presets = snapshot.embedded_presets.is_empty();
    if !no_embedded_presets {
        push_issue(
            issues,
            A1MiniRoundTripIssueCode::EmbeddedPresetFound,
            Some(role),
            format!(
                "Embedded process, filament, or machine presets are forbidden: {:?}.",
                snapshot.embedded_presets
            ),
        );
    }
    TargetFlags {
        application,
        machine,
        process,
        one_plate,
        external_spool,
        single_slot,
        no_embedded_presets,
    }
}

fn only_slot_one(slots: &[u16]) -> bool {
    slots.iter().all(|slot| *slot == 1)
}

fn internal_ids_are_valid(analysis: &ProjectAnalysis) -> bool {
    let mut object_ids = BTreeSet::new();
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
    let mut plate_ids = BTreeSet::new();
    let mut instance_keys = BTreeSet::new();
    for plate in &analysis.plates {
        if plate.id == 0 || !plate_ids.insert(plate.id) {
            return false;
        }
        for instance in &plate.instances {
            if !object_ids.contains(&instance.object_id)
                || !instance_keys.insert((plate.id, instance.object_id, instance.instance_id))
            {
                return false;
            }
        }
    }
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct A1ProfileIdentity {
    version: String,
    printer_model: String,
    printer_variant: FiniteNumber,
    machine_profile: String,
    process_profile: String,
    nozzle_diameter_mm: FiniteNumber,
    printable_height_mm: FiniteNumber,
    printable_area: Vec<String>,
    has_filament_switcher: bool,
    filament_profile: String,
    default_filament_profile: String,
    filament_setting_id: String,
    filament_color: String,
    filament_multi_color: String,
    filament_material: String,
    filament_map: FiniteNumber,
    filament_nozzle_map: FiniteNumber,
    filament_self_index: FiniteNumber,
    filament_is_mixed: bool,
    filament_mixed_components: String,
    filament_mixed_gradient: FiniteNumber,
    filament_mixed_sublayer_ratios: String,
    flush_volumes_matrix: FiniteNumber,
}

impl A1ProfileIdentity {
    fn load(path: &Path, role: A1MiniRoundTripArtifactRole) -> Result<Self, A1MiniRoundTripError> {
        let bytes = read_named_entry(path, PROJECT_SETTINGS_PATH, MAX_CONFIG_BYTES)?;
        let root: Value =
            serde_json::from_slice(&bytes).map_err(|source| A1MiniRoundTripError::Json {
                path: path.to_path_buf(),
                entry: PROJECT_SETTINGS_PATH,
                source,
            })?;
        let object = root
            .as_object()
            .ok_or_else(|| A1MiniRoundTripError::Semantic {
                path: path.to_path_buf(),
                message: "project settings root is not an object".into(),
            })?;
        Ok(Self {
            version: required_string(path, object, "version")?,
            printer_model: required_string(path, object, "printer_model")?,
            printer_variant: required_number(path, object, "printer_variant")?,
            machine_profile: required_string(path, object, "printer_settings_id")?,
            process_profile: required_string(path, object, "print_settings_id")?,
            nozzle_diameter_mm: required_one_number(path, object, "nozzle_diameter")?,
            printable_height_mm: required_number(path, object, "printable_height")?,
            printable_area: required_string_array(path, object, "printable_area")?,
            has_filament_switcher: required_bool_or_gui_default(
                path,
                object,
                "has_filament_switcher",
                role,
                false,
            )?,
            filament_profile: required_one_string(path, object, "filament_settings_id")?,
            default_filament_profile: required_one_string(
                path,
                object,
                "default_filament_profile",
            )?,
            filament_setting_id: required_one_string(path, object, "filament_ids")?,
            filament_color: normalize_color(&required_one_string(path, object, "filament_colour")?),
            filament_multi_color: normalize_color(&required_one_string(
                path,
                object,
                "filament_multi_colour",
            )?),
            filament_material: required_one_string(path, object, "filament_type")?
                .to_ascii_uppercase(),
            filament_map: required_one_number(path, object, "filament_map")?,
            filament_nozzle_map: required_one_number_or_gui_default(
                path,
                object,
                "filament_nozzle_map",
                role,
                0.0,
            )?,
            filament_self_index: required_one_number(path, object, "filament_self_index")?,
            filament_is_mixed: required_one_bool_or_gui_default(
                path,
                object,
                "filament_is_mixed",
                role,
                false,
            )?,
            filament_mixed_components: required_one_string_or_gui_default(
                path,
                object,
                "filament_mixed_components",
                role,
                "",
            )?,
            filament_mixed_gradient: required_one_number_or_gui_default(
                path,
                object,
                "filament_mixed_gradient",
                role,
                0.0,
            )?,
            filament_mixed_sublayer_ratios: required_one_string_or_gui_default(
                path,
                object,
                "filament_mixed_sublayer_ratios",
                role,
                "",
            )?,
            flush_volumes_matrix: required_one_number(path, object, "flush_volumes_matrix")?,
        })
    }

    fn material(&self) -> Option<A1MiniMaterial> {
        match self.filament_material.as_str() {
            "PLA" => Some(A1MiniMaterial::Pla),
            "PETG" => Some(A1MiniMaterial::Petg),
            _ => None,
        }
    }

    fn has_exact_machine(&self) -> bool {
        self.printer_model == "Bambu Lab A1 mini"
            && self.printer_variant.is(0.4)
            && self.machine_profile == A1MINI_MACHINE_PROFILE
            && self.nozzle_diameter_mm.is(0.4)
            && self.printable_height_mm.is(180.0)
            && self.printable_area == ["0x0", "180x0", "180x180", "0x180"]
    }

    fn has_exact_material(&self, role: A1MiniRoundTripArtifactRole) -> bool {
        self.material().is_some_and(|material| {
            self.filament_profile == material.profile_name()
                && (self.default_filament_profile == material.profile_name()
                    || (role != A1MiniRoundTripArtifactRole::WriterCandidate
                        && self.default_filament_profile == "Bambu PLA Basic @BBL A1M"))
                && !self.filament_setting_id.trim().is_empty()
                && valid_color(&self.filament_color)
                && self.filament_multi_color == self.filament_color
        })
    }

    fn has_exact_external_spool(&self) -> bool {
        !self.has_filament_switcher
            && self.filament_map.is(1.0)
            && self.filament_nozzle_map.is(0.0)
            && self.filament_self_index.is(1.0)
            && !self.filament_is_mixed
            && self.filament_mixed_components.is_empty()
            && self.filament_mixed_gradient.is(0.0)
            && self.filament_mixed_sublayer_ratios.is_empty()
            && self.flush_volumes_matrix.is(0.0)
    }

    fn machine_process_key(&self) -> MachineProcessKey<'_> {
        MachineProcessKey {
            version: &self.version,
            printer_model: &self.printer_model,
            printer_variant: self.printer_variant,
            machine_profile: &self.machine_profile,
            process_profile: &self.process_profile,
            nozzle_diameter_mm: self.nozzle_diameter_mm,
            printable_height_mm: self.printable_height_mm,
            printable_area: &self.printable_area,
        }
    }

    fn material_key(&self) -> MaterialKey<'_> {
        MaterialKey {
            filament_profile: &self.filament_profile,
            filament_color: &self.filament_color,
            filament_material: &self.filament_material,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MachineProcessKey<'a> {
    version: &'a str,
    printer_model: &'a str,
    printer_variant: FiniteNumber,
    machine_profile: &'a str,
    process_profile: &'a str,
    nozzle_diameter_mm: FiniteNumber,
    printable_height_mm: FiniteNumber,
    printable_area: &'a [String],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MaterialKey<'a> {
    filament_profile: &'a str,
    filament_color: &'a str,
    filament_material: &'a str,
}

fn gui_filament_serialization_is_expected(
    candidate: &A1ProfileIdentity,
    first_save: &A1ProfileIdentity,
    reopened_save: &A1ProfileIdentity,
) -> bool {
    if first_save.filament_setting_id != reopened_save.filament_setting_id
        || first_save.default_filament_profile != reopened_save.default_filament_profile
    {
        return false;
    }
    let preserved = first_save.filament_setting_id == candidate.filament_setting_id
        && first_save.default_filament_profile == candidate.default_filament_profile;
    let Some(material) = candidate.material() else {
        return false;
    };
    let normalized_id = match material {
        A1MiniMaterial::Pla => "GFL99",
        A1MiniMaterial::Petg => "GFG99",
    };
    let normalized = first_save.filament_setting_id == normalized_id
        && first_save.default_filament_profile == "Bambu PLA Basic @BBL A1M";
    preserved || normalized
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
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
}

fn required_string(
    path: &Path,
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<String, A1MiniRoundTripError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| A1MiniRoundTripError::Semantic {
            path: path.to_path_buf(),
            message: format!("project setting {key:?} must be a non-empty string"),
        })
}

fn required_number(
    path: &Path,
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<FiniteNumber, A1MiniRoundTripError> {
    object
        .get(key)
        .and_then(json_number)
        .and_then(FiniteNumber::new)
        .ok_or_else(|| A1MiniRoundTripError::Semantic {
            path: path.to_path_buf(),
            message: format!("project setting {key:?} must be a finite number"),
        })
}

fn required_bool(
    path: &Path,
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<bool, A1MiniRoundTripError> {
    object
        .get(key)
        .and_then(json_bool)
        .ok_or_else(|| A1MiniRoundTripError::Semantic {
            path: path.to_path_buf(),
            message: format!("project setting {key:?} must be a boolean or zero/one"),
        })
}

fn required_bool_or_gui_default(
    path: &Path,
    object: &serde_json::Map<String, Value>,
    key: &str,
    role: A1MiniRoundTripArtifactRole,
    gui_default: bool,
) -> Result<bool, A1MiniRoundTripError> {
    if object.contains_key(key) || role == A1MiniRoundTripArtifactRole::WriterCandidate {
        required_bool(path, object, key)
    } else {
        Ok(gui_default)
    }
}

fn required_string_array(
    path: &Path,
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Vec<String>, A1MiniRoundTripError> {
    object
        .get(key)
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
        .and_then(|values| {
            values
                .iter()
                .map(|value| value.as_str().map(str::trim).map(str::to_owned))
                .collect::<Option<Vec<_>>>()
        })
        .ok_or_else(|| A1MiniRoundTripError::Semantic {
            path: path.to_path_buf(),
            message: format!("project setting {key:?} must be a non-empty string array"),
        })
}

fn required_one_string(
    path: &Path,
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<String, A1MiniRoundTripError> {
    let values = required_string_array(path, object, key)?;
    if values.len() == 1 && !values[0].is_empty() {
        Ok(values[0].clone())
    } else {
        Err(A1MiniRoundTripError::Semantic {
            path: path.to_path_buf(),
            message: format!("project setting {key:?} must contain exactly one non-empty string"),
        })
    }
}

fn required_one_string_allow_empty(
    path: &Path,
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<String, A1MiniRoundTripError> {
    let values = object
        .get(key)
        .and_then(Value::as_array)
        .filter(|values| values.len() == 1)
        .ok_or_else(|| A1MiniRoundTripError::Semantic {
            path: path.to_path_buf(),
            message: format!("project setting {key:?} must contain exactly one string"),
        })?;
    values[0]
        .as_str()
        .map(str::trim)
        .map(str::to_owned)
        .ok_or_else(|| A1MiniRoundTripError::Semantic {
            path: path.to_path_buf(),
            message: format!("project setting {key:?} contains no string"),
        })
}

fn required_one_string_or_gui_default(
    path: &Path,
    object: &serde_json::Map<String, Value>,
    key: &str,
    role: A1MiniRoundTripArtifactRole,
    gui_default: &str,
) -> Result<String, A1MiniRoundTripError> {
    if object.contains_key(key) || role == A1MiniRoundTripArtifactRole::WriterCandidate {
        required_one_string_allow_empty(path, object, key)
    } else {
        Ok(gui_default.to_owned())
    }
}

fn required_one_number(
    path: &Path,
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<FiniteNumber, A1MiniRoundTripError> {
    let values = object
        .get(key)
        .and_then(Value::as_array)
        .filter(|values| values.len() == 1)
        .ok_or_else(|| A1MiniRoundTripError::Semantic {
            path: path.to_path_buf(),
            message: format!("project setting {key:?} must contain exactly one number"),
        })?;
    values[0]
        .as_f64()
        .or_else(|| values[0].as_str().and_then(|value| value.parse().ok()))
        .and_then(FiniteNumber::new)
        .ok_or_else(|| A1MiniRoundTripError::Semantic {
            path: path.to_path_buf(),
            message: format!("project setting {key:?} contains no finite number"),
        })
}

fn required_one_number_or_gui_default(
    path: &Path,
    object: &serde_json::Map<String, Value>,
    key: &str,
    role: A1MiniRoundTripArtifactRole,
    gui_default: f64,
) -> Result<FiniteNumber, A1MiniRoundTripError> {
    if object.contains_key(key) || role == A1MiniRoundTripArtifactRole::WriterCandidate {
        required_one_number(path, object, key)
    } else {
        FiniteNumber::new(gui_default).ok_or_else(|| A1MiniRoundTripError::Semantic {
            path: path.to_path_buf(),
            message: format!("internal GUI default for project setting {key:?} is not finite"),
        })
    }
}

fn required_one_bool(
    path: &Path,
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<bool, A1MiniRoundTripError> {
    let values = object
        .get(key)
        .and_then(Value::as_array)
        .filter(|values| values.len() == 1)
        .ok_or_else(|| A1MiniRoundTripError::Semantic {
            path: path.to_path_buf(),
            message: format!("project setting {key:?} must contain exactly one boolean"),
        })?;
    json_bool(&values[0]).ok_or_else(|| A1MiniRoundTripError::Semantic {
        path: path.to_path_buf(),
        message: format!("project setting {key:?} contains no boolean or zero/one"),
    })
}

fn required_one_bool_or_gui_default(
    path: &Path,
    object: &serde_json::Map<String, Value>,
    key: &str,
    role: A1MiniRoundTripArtifactRole,
    gui_default: bool,
) -> Result<bool, A1MiniRoundTripError> {
    if object.contains_key(key) || role == A1MiniRoundTripArtifactRole::WriterCandidate {
        required_one_bool(path, object, key)
    } else {
        Ok(gui_default)
    }
}

fn json_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(value) => value.as_f64(),
        Value::String(value) => value.parse().ok(),
        _ => None,
    }
}

fn json_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(value) => Some(*value),
        Value::Number(value) => match value.as_i64()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        },
        Value::String(value) if value == "0" || value.eq_ignore_ascii_case("false") => Some(false),
        Value::String(value) if value == "1" || value.eq_ignore_ascii_case("true") => Some(true),
        _ => None,
    }
}

fn normalize_color(value: &str) -> String {
    let upper = value.trim().to_ascii_uppercase();
    if upper.len() == 9 && upper.ends_with("FF") {
        upper[..7].to_owned()
    } else {
        upper
    }
}

fn valid_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlateFilamentMap {
    mode: String,
    mapping: String,
}

impl PlateFilamentMap {
    fn is_exact(&self) -> bool {
        self.mode == EXPECTED_FILAMENT_MAP_MODE && self.mapping == EXPECTED_FILAMENT_MAP
    }
}

#[derive(Default)]
struct PendingPlateMap {
    id: Option<u32>,
    mode: Option<String>,
    mapping: Option<String>,
}

fn read_plate_filament_maps(
    path: &Path,
) -> Result<BTreeMap<u32, PlateFilamentMap>, A1MiniRoundTripError> {
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
                .map_err(|error| A1MiniRoundTripError::Xml {
                    path: path.to_path_buf(),
                    entry: MODEL_SETTINGS_PATH.into(),
                    message: error.to_string(),
                })?;
        match event {
            Event::Start(event) => {
                if local_name(event.name().as_ref()) == b"plate" && current.is_none() {
                    plate_depth = Some(depth);
                    current = Some(PendingPlateMap::default());
                }
                depth = depth.saturating_add(1);
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
                            .ok_or_else(|| A1MiniRoundTripError::Semantic {
                                path: path.to_path_buf(),
                                message: "plate has no positive plater_id".into(),
                            })?;
                        if current.id.replace(id).is_some() {
                            return Err(A1MiniRoundTripError::Semantic {
                                path: path.to_path_buf(),
                                message: "plate contains duplicate plater_id metadata".into(),
                            });
                        }
                    }
                    Some("filament_map_mode") => {
                        if current.mode.replace(value.unwrap_or_default()).is_some() {
                            return Err(A1MiniRoundTripError::Semantic {
                                path: path.to_path_buf(),
                                message: "plate contains duplicate filament_map_mode metadata"
                                    .into(),
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
                            return Err(A1MiniRoundTripError::Semantic {
                                path: path.to_path_buf(),
                                message: "plate contains duplicate filament_maps metadata".into(),
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
                    let id = current.id.ok_or_else(|| A1MiniRoundTripError::Semantic {
                        path: path.to_path_buf(),
                        message: "plate is missing plater_id metadata".into(),
                    })?;
                    let map = PlateFilamentMap {
                        mode: current.mode.unwrap_or_default(),
                        mapping: current.mapping.unwrap_or_default(),
                    };
                    if maps.insert(id, map).is_some() {
                        return Err(A1MiniRoundTripError::Semantic {
                            path: path.to_path_buf(),
                            message: format!("model settings contain duplicate plate {id}"),
                        });
                    }
                }
            }
            Event::DocType(_) => {
                return Err(A1MiniRoundTripError::Xml {
                    path: path.to_path_buf(),
                    entry: MODEL_SETTINGS_PATH.into(),
                    message: "DOCTYPE is forbidden".into(),
                });
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(maps)
}

#[derive(Clone, Debug)]
struct SemanticSnapshot {
    profile: A1ProfileIdentity,
    resources: BTreeMap<ResourceKey, RawResource>,
    resource_fingerprints: BTreeMap<ResourceKey, ResourceFingerprint>,
    geometry_resource_multiset: SemanticMultiset,
    object_multiset: SemanticMultiset,
    placements: Vec<PlacementPlate>,
    plate_maps: BTreeMap<u32, PlateFilamentMap>,
    embedded_presets: Vec<String>,
    geometry_resource_count: usize,
    geometry_sha256: String,
    object_part_sha256: String,
    placement_sha256: String,
    semantic_sha256: String,
}

impl SemanticSnapshot {
    fn load(
        path: &Path,
        analysis: &ProjectAnalysis,
        role: A1MiniRoundTripArtifactRole,
    ) -> Result<Self, A1MiniRoundTripError> {
        let profile = A1ProfileIdentity::load(path, role)?;
        let plate_maps = read_plate_filament_maps(path)?;
        let analyzed_plate_ids = analysis
            .plates
            .iter()
            .map(|plate| plate.id)
            .collect::<BTreeSet<_>>();
        let mapped_plate_ids = plate_maps.keys().copied().collect::<BTreeSet<_>>();
        if analyzed_plate_ids != mapped_plate_ids {
            return Err(A1MiniRoundTripError::Semantic {
                path: path.to_path_buf(),
                message: format!(
                    "filament-map plate IDs {mapped_plate_ids:?} do not match analyzed plates {analyzed_plate_ids:?}"
                ),
            });
        }
        let embedded_presets = embedded_preset_entries(path)?;
        let resources = read_resource_graph(path)?;
        let resource_fingerprints = fingerprint_resources(path, &resources)?;
        let geometry_resource_multiset = resource_fingerprints.values().fold(
            SemanticMultiset::new(),
            |mut multiset, fingerprint| {
                *multiset.entry(fingerprint.sha256.clone()).or_insert(0) += 1;
                multiset
            },
        );
        let (object_multiset, object_signatures) =
            semantic_objects(path, analysis, &resource_fingerprints)?;
        let (plate_multiset, placements) = semantic_plates(path, analysis, &object_signatures)?;
        let geometry_sha256 = digest_serializable(&geometry_resource_multiset)?;
        let object_part_sha256 = digest_serializable(&object_multiset)?;
        let placement_sha256 = digest_serializable(&plate_multiset)?;
        let semantic_sha256 = digest_serializable(&(
            &profile,
            &geometry_resource_multiset,
            &object_multiset,
            &plate_multiset,
        ))?;
        Ok(Self {
            profile,
            geometry_resource_multiset,
            object_multiset,
            placements,
            plate_maps,
            embedded_presets,
            geometry_resource_count: resource_fingerprints.len(),
            resources,
            resource_fingerprints,
            geometry_sha256,
            object_part_sha256,
            placement_sha256,
            semantic_sha256,
        })
    }
}

fn embedded_preset_entries(path: &Path) -> Result<Vec<String>, A1MiniRoundTripError> {
    let file = File::open(path).map_err(|source| A1MiniRoundTripError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    let archive = ZipArchive::new(file).map_err(|source| A1MiniRoundTripError::Zip {
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
    object_type: Option<String>,
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
    vertex_count: u64,
    triangle_count: u64,
    #[serde(skip)]
    anchor: [f64; 3],
}

struct PendingResource {
    id: u32,
    object_depth: usize,
    production_uuid: Option<String>,
    object_type: Option<String>,
    has_mesh: bool,
    geometry: Sha256,
    mesh_anchor: Option<[f64; 3]>,
    vertex_count: u64,
    triangle_count: u64,
    components: Vec<RawComponent>,
}

impl PendingResource {
    fn new(
        id: u32,
        object_depth: usize,
        production_uuid: Option<String>,
        object_type: Option<String>,
    ) -> Self {
        let mut geometry = Sha256::new();
        geometry.update(b"u1-a1mini-round-trip-mesh-v1\0");
        Self {
            id,
            object_depth,
            production_uuid,
            object_type,
            has_mesh: false,
            geometry,
            mesh_anchor: None,
            vertex_count: 0,
            triangle_count: 0,
            components: Vec::new(),
        }
    }

    fn finish(self) -> RawResource {
        RawResource {
            production_uuid: self.production_uuid,
            object_type: self.object_type,
            mesh: self.has_mesh.then(|| MeshFingerprint {
                sha256: format!("{:x}", self.geometry.finalize()),
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
) -> Result<BTreeMap<ResourceKey, RawResource>, A1MiniRoundTripError> {
    let file = File::open(package_path).map_err(|source| A1MiniRoundTripError::Open {
        path: package_path.to_path_buf(),
        source,
    })?;
    let mut archive = ZipArchive::new(file).map_err(|source| A1MiniRoundTripError::Zip {
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
            .map_err(|source| A1MiniRoundTripError::Zip {
                path: package_path.to_path_buf(),
                source,
            })?;
        if entry.size() > MAX_MODEL_BYTES {
            return Err(A1MiniRoundTripError::EntryTooLarge {
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
) -> Result<(), A1MiniRoundTripError> {
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
                .map_err(|error| A1MiniRoundTripError::Xml {
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
                            .map_err(|message| A1MiniRoundTripError::Semantic {
                                path: package_path.to_path_buf(),
                                message: format!("{model_path}: {message}"),
                            })?;
                } else if name == b"object" {
                    if current.is_some() {
                        return Err(A1MiniRoundTripError::Semantic {
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
                    let object_type =
                        optional_attribute(&reader, &event, b"type", package_path, model_path)?;
                    current = Some(PendingResource::new(
                        id,
                        depth,
                        production_uuid,
                        object_type,
                    ));
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
                depth = depth.saturating_add(1);
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
                        return Err(A1MiniRoundTripError::Semantic {
                            path: package_path.to_path_buf(),
                            message: format!("duplicate model resource {}/{}", key.path, key.id),
                        });
                    }
                }
            }
            Event::DocType(_) => {
                return Err(A1MiniRoundTripError::Xml {
                    path: package_path.to_path_buf(),
                    entry: model_path.to_owned(),
                    message: "DOCTYPE is forbidden".into(),
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
) -> Result<(), A1MiniRoundTripError> {
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
                current.geometry.update(
                    required_u32_attribute(reader, event, vertex, package_path, model_path)?
                        .to_le_bytes(),
                );
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
                    .map_err(|message| A1MiniRoundTripError::Semantic {
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
) -> Result<BTreeMap<ResourceKey, ResourceFingerprint>, A1MiniRoundTripError> {
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
) -> Result<ResourceFingerprint, A1MiniRoundTripError> {
    if let Some(fingerprint) = memo.get(key) {
        return Ok(fingerprint.clone());
    }
    if !visiting.insert(key.clone()) {
        return Err(A1MiniRoundTripError::Semantic {
            path: package_path.to_path_buf(),
            message: format!(
                "component graph contains a cycle at {}/{}",
                key.path, key.id
            ),
        });
    }
    let resource = resources
        .get(key)
        .ok_or_else(|| A1MiniRoundTripError::Semantic {
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
            .ok_or_else(|| A1MiniRoundTripError::Semantic {
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
    visiting.remove(key);
    let mesh_identity = resource.mesh.as_ref().map(|mesh| {
        if resource.object_type.as_deref() == Some("other")
            && mesh.vertex_count <= MAX_REBASED_TEXT_MESH_VERTICES
            && mesh.triangle_count <= MAX_REBASED_TEXT_MESH_TRIANGLES
        {
            resource.production_uuid.as_ref().map_or_else(
                || format!("mesh-sha256:{}", mesh.sha256),
                |uuid| {
                    format!(
                        "bambu-editable-text:{uuid}:{}:{}",
                        mesh.vertex_count, mesh.triangle_count
                    )
                },
            )
        } else {
            format!("mesh-sha256:{}", mesh.sha256)
        }
    });
    let semantic_identity = digest_serializable(&SemanticResourceFingerprintDocument {
        mesh_identity,
        components: semantic_components,
    })?;
    let fingerprint = ResourceFingerprint {
        sha256,
        semantic_identity,
        anchor,
    };
    memo.insert(key.clone(), fingerprint.clone());
    Ok(fingerprint)
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ComparableResourceComponent {
    target_uuid: String,
    linear_transform: [i64; 9],
    anchor_in_parent: [i64; 3],
}

fn geometry_graphs_equivalent(
    first_path: &Path,
    first: &SemanticSnapshot,
    second_path: &Path,
    second: &SemanticSnapshot,
) -> Result<bool, A1MiniRoundTripError> {
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
                if first_resource.object_type.as_deref() != Some("other")
                    || second_resource.object_type.as_deref() != Some("other")
                    || first_mesh.vertex_count != second_mesh.vertex_count
                    || first_mesh.triangle_count != second_mesh.triangle_count
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
    snapshot: &SemanticSnapshot,
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
    snapshot: &SemanticSnapshot,
    other: &SemanticSnapshot,
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
    snapshot: &SemanticSnapshot,
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
) -> Result<bool, A1MiniRoundTripError> {
    let first = read_bounded_mesh_payload(first_path, first_key, first_mesh)?;
    let second = read_bounded_mesh_payload(second_path, second_key, second_mesh)?;
    if first.triangles != second.triangles
        || first.vertices.len() != second.vertices.len()
        || first.vertices.is_empty()
    {
        return Ok(false);
    }
    let translation = [
        first.vertices[0][0] - second.vertices[0][0],
        first.vertices[0][1] - second.vertices[0][1],
        first.vertices[0][2] - second.vertices[0][2],
    ];
    Ok(first
        .vertices
        .iter()
        .zip(second.vertices)
        .all(|(first, second)| {
            (0..3).all(|axis| {
                ((first[axis] - second[axis]) - translation[axis]).abs()
                    <= TEXT_MESH_REBASE_TOLERANCE_MM
            })
        }))
}

fn read_bounded_mesh_payload(
    package_path: &Path,
    key: &ResourceKey,
    expected: &MeshFingerprint,
) -> Result<BoundedMeshPayload, A1MiniRoundTripError> {
    let file = File::open(package_path).map_err(|source| A1MiniRoundTripError::Open {
        path: package_path.to_path_buf(),
        source,
    })?;
    let mut archive = ZipArchive::new(file).map_err(|source| A1MiniRoundTripError::Zip {
        path: package_path.to_path_buf(),
        source,
    })?;
    let entry = archive
        .by_name(&key.path)
        .map_err(|source| A1MiniRoundTripError::Zip {
            path: package_path.to_path_buf(),
            source,
        })?;
    if entry.size() > MAX_MODEL_BYTES {
        return Err(A1MiniRoundTripError::EntryTooLarge {
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
                .map_err(|error| A1MiniRoundTripError::Xml {
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
                return Err(A1MiniRoundTripError::Xml {
                    path: package_path.to_path_buf(),
                    entry: key.path.clone(),
                    message: "DOCTYPE is forbidden".into(),
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
) -> Result<(SemanticMultiset, ObjectSignatureById), A1MiniRoundTripError> {
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
            .ok_or_else(|| A1MiniRoundTripError::Semantic {
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
                let geometry_fingerprint = resource_hashes
                    .get(&ResourceKey {
                        path: part_path.clone(),
                        id: part.id,
                    })
                    .ok_or_else(|| A1MiniRoundTripError::Semantic {
                        path: package_path.to_path_buf(),
                        message: format!(
                            "analyzed part {} has no geometry resource {part_path}/{}",
                            part.id, part.id
                        ),
                    })?;
                Ok(SemanticPartDescriptor {
                    geometry_sha256: geometry_fingerprint.semantic_identity.clone(),
                    volume_type: volume_type_name(&part.volume_type),
                    printable: part.printable,
                    extruder_slot: part.extruder_slot,
                    inherited_extruder_slot: part.inherited_extruder_slot,
                    painted_slots: sorted_unique(&part.painted_slots),
                    effective_slots: sorted_unique(&part.effective_slots),
                    object_space_bounds: part.object_space_bounds.map(quantized_bounds),
                })
            })
            .collect::<Result<Vec<_>, A1MiniRoundTripError>>()?;
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
            return Err(A1MiniRoundTripError::Semantic {
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
) -> Result<(SemanticMultiset, Vec<PlacementPlate>), A1MiniRoundTripError> {
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
                    .ok_or_else(|| A1MiniRoundTripError::Semantic {
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
            .collect::<Result<Vec<_>, A1MiniRoundTripError>>()?;
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

// Bambu Studio 02.02.00.85 rewrites otherwise unchanged instance transforms with
// bounded decimal serialization. These limits are deliberately far below the
// qualified 0.20 mm process while still rejecting physical placement changes.
const GUI_LINEAR_TRANSFORM_TOLERANCE: f64 = 1.0e-9;
const GUI_PLACEMENT_TOLERANCE_MM: f64 = 1.0e-5;

fn placement_snapshots_equivalent(left: &[PlacementPlate], right: &[PlacementPlate]) -> bool {
    if left.len() != 1 || right.len() != 1 {
        return false;
    }
    let left = &left[0];
    let right = &right[0];
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
        VolumeType::NormalPart => "normal_part".into(),
        VolumeType::NegativePart => "negative_part".into(),
        VolumeType::Modifier => "modifier".into(),
        VolumeType::SupportBlocker => "support_blocker".into(),
        VolumeType::SupportEnforcer => "support_enforcer".into(),
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
    entry_name: &str,
    limit: u64,
) -> Result<Vec<u8>, A1MiniRoundTripError> {
    let file = File::open(package_path).map_err(|source| A1MiniRoundTripError::Open {
        path: package_path.to_path_buf(),
        source,
    })?;
    let mut archive = ZipArchive::new(file).map_err(|source| A1MiniRoundTripError::Zip {
        path: package_path.to_path_buf(),
        source,
    })?;
    let entry = match archive.by_name(entry_name) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => {
            return Err(A1MiniRoundTripError::MissingEntry {
                path: package_path.to_path_buf(),
                entry: entry_name.to_owned(),
            });
        }
        Err(source) => {
            return Err(A1MiniRoundTripError::Zip {
                path: package_path.to_path_buf(),
                source,
            });
        }
    };
    if entry.size() > limit {
        return Err(A1MiniRoundTripError::EntryTooLarge {
            path: package_path.to_path_buf(),
            entry: entry_name.to_owned(),
            limit,
        });
    }
    let mut bytes = Vec::with_capacity(usize::try_from(entry.size()).unwrap_or(0));
    entry
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| A1MiniRoundTripError::Open {
            path: package_path.to_path_buf(),
            source,
        })?;
    if bytes.len() as u64 > limit {
        return Err(A1MiniRoundTripError::EntryTooLarge {
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
) -> Result<Option<String>, A1MiniRoundTripError> {
    let mut value = None;
    for attribute in event.attributes().with_checks(true) {
        let attribute = attribute.map_err(|error| A1MiniRoundTripError::Xml {
            path: package_path.to_path_buf(),
            entry: entry.to_owned(),
            message: error.to_string(),
        })?;
        if local_name(attribute.key.as_ref()) != local {
            continue;
        }
        let decoded = attribute
            .decode_and_unescape_value(reader.decoder())
            .map_err(|error| A1MiniRoundTripError::Xml {
                path: package_path.to_path_buf(),
                entry: entry.to_owned(),
                message: error.to_string(),
            })?
            .into_owned();
        if value.replace(decoded).is_some() {
            return Err(A1MiniRoundTripError::Xml {
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
) -> Result<u32, A1MiniRoundTripError> {
    optional_attribute(reader, event, local, package_path, entry)?
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| A1MiniRoundTripError::Semantic {
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
) -> Result<u32, A1MiniRoundTripError> {
    let value = required_u32_attribute(reader, event, local, package_path, entry)?;
    if value == 0 {
        Err(A1MiniRoundTripError::Semantic {
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
) -> Result<f64, A1MiniRoundTripError> {
    optional_attribute(reader, event, local, package_path, entry)?
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .ok_or_else(|| A1MiniRoundTripError::Semantic {
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

fn digest_serializable(value: &impl Serialize) -> Result<String, A1MiniRoundTripError> {
    let bytes = serde_json::to_vec(value).map_err(|source| A1MiniRoundTripError::Semantic {
        path: PathBuf::from("<semantic-comparison>"),
        message: format!("failed to serialize semantic fingerprint: {source}"),
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use tempfile::TempDir;
    use u1_three_mf::{
        ContentTypesBuilder, MODEL_RELATIONSHIP_TYPE, OpcRelationship, relationships_xml,
    };
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, DateTime, ZipWriter};

    #[derive(Clone, Copy)]
    struct ArchiveOptions {
        object_id: u32,
        vertex_x: f64,
        placement_x: f64,
        material: A1MiniMaterial,
        gui_dialect: bool,
        ams_enabled: bool,
        safe_gui_artifacts: bool,
        stale_slice_artifact: bool,
    }

    impl Default for ArchiveOptions {
        fn default() -> Self {
            Self {
                object_id: 1,
                vertex_x: 10.0,
                placement_x: 20.0,
                material: A1MiniMaterial::Pla,
                gui_dialect: false,
                ams_enabled: false,
                safe_gui_artifacts: false,
                stale_slice_artifact: false,
            }
        }
    }

    fn write_archive(path: &Path, options: ArchiveOptions) {
        let mut content_types = ContentTypesBuilder::project_3mf();
        content_types
            .add_override(PROJECT_SETTINGS_PATH, "application/json")
            .unwrap();
        content_types
            .add_override(MODEL_SETTINGS_PATH, "application/xml")
            .unwrap();
        content_types.add_default("png", "image/png").unwrap();
        if options.stale_slice_artifact {
            content_types
                .add_override("Metadata/slice_data.config", "application/xml")
                .unwrap();
        }
        let relationships = relationships_xml(&[OpcRelationship::internal(
            "rel-1",
            MODEL_RELATIONSHIP_TYPE,
            MAIN_MODEL_PATH,
        )
        .unwrap()])
        .unwrap();
        let model = model_xml(options.object_id, options.vertex_x, options.placement_x);
        let project = project_settings(options.material, options.gui_dialect, options.ams_enabled);
        let model_settings = model_settings_xml(options.object_id);
        let file = File::create(path).unwrap();
        let mut archive = ZipWriter::new(file);
        let zip_options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(6))
            .last_modified_time(DateTime::default())
            .unix_permissions(0o644);
        let mut entries = vec![
            ("[Content_Types].xml", content_types.to_xml().unwrap()),
            ("_rels/.rels", relationships),
            (MAIN_MODEL_PATH, model.into_bytes()),
            (PROJECT_SETTINGS_PATH, project),
            (MODEL_SETTINGS_PATH, model_settings.into_bytes()),
        ];
        if options.safe_gui_artifacts {
            entries.extend([
                ("Metadata/slice_info.config", b"<config/>".to_vec()),
                ("Metadata/plate_1.json", b"{}".to_vec()),
                ("Metadata/plate_1.png", b"regenerated-preview".to_vec()),
            ]);
        }
        if options.stale_slice_artifact {
            entries.push(("Metadata/slice_data.config", b"<config/>".to_vec()));
        }
        for (name, bytes) in entries {
            archive.start_file(name, zip_options).unwrap();
            archive.write_all(&bytes).unwrap();
        }
        archive.finish().unwrap();
    }

    fn model_xml(object_id: u32, vertex_x: f64, placement_x: f64) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:BambuStudio="http://schemas.bambulab.com/package/2021" unit="millimeter">
  <metadata name="Application">BambuStudio-{A1MINI_APPLICATION_VERSION}</metadata>
  <metadata name="BambuStudio:3mfVersion">1</metadata>
  <resources>
    <object id="{object_id}" type="model"><mesh>
      <vertices><vertex x="0" y="0" z="0"/><vertex x="{vertex_x}" y="0" z="0"/><vertex x="0" y="10" z="0"/></vertices>
      <triangles><triangle v1="0" v2="1" v3="2"/></triangles>
    </mesh></object>
  </resources>
  <build><item objectid="{object_id}" printable="1" transform="1 0 0 0 1 0 0 0 1 {placement_x} 30 1"/></build>
</model>"#
        )
    }

    fn model_settings_xml(object_id: u32) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
  <object id="{object_id}"><metadata key="name" value="Qualification Triangle"/><metadata key="extruder" value="1"/></object>
  <plate>
    <metadata key="plater_id" value="1"/><metadata key="plater_name" value="A1 mini qualification"/>
    <metadata key="filament_map_mode" value="Auto For Flush"/><metadata key="filament_maps" value="1"/>
    <model_instance><metadata key="object_id" value="{object_id}"/><metadata key="instance_id" value="0"/><metadata key="identify_id" value="{object_id}"/></model_instance>
  </plate>
</config>"#
        )
    }

    fn project_settings(material: A1MiniMaterial, gui_dialect: bool, ams_enabled: bool) -> Vec<u8> {
        let profile = material.profile_name();
        let material = material.material_name();
        let mut settings = json!({
            "name": "project_settings",
            "from": "project",
            "version": A1MINI_APPLICATION_VERSION,
            "printer_model": "Bambu Lab A1 mini",
            "printer_variant": "0.4",
            "printer_settings_id": A1MINI_MACHINE_PROFILE,
            "print_settings_id": A1MINI_PROCESS_PROFILE,
            "printable_height": "180",
            "printable_area": ["0x0", "180x0", "180x180", "0x180"],
            "has_filament_switcher": if ams_enabled { "1" } else { "0" },
            "nozzle_diameter": ["0.4"],
            "filament_settings_id": [profile],
            "default_filament_profile": [profile],
            "filament_ids": [if material == "PLA" { "GFSL99_02" } else { "GFSG98_04" }],
            "filament_colour": ["#112233"],
            "filament_multi_colour": ["#112233"],
            "filament_type": [material],
            "filament_map": ["1"],
            "filament_nozzle_map": ["0"],
            "filament_self_index": ["1"],
            "filament_is_mixed": ["0"],
            "filament_mixed_components": [""],
            "filament_mixed_gradient": ["0"],
            "filament_mixed_sublayer_ratios": [""],
            "flush_volumes_matrix": ["0"]
        });
        if gui_dialect {
            let object = settings.as_object_mut().unwrap();
            object.insert("printable_height".into(), json!(180));
            object.insert("has_filament_switcher".into(), json!(ams_enabled));
            object.insert("nozzle_diameter".into(), json!([0.4]));
            object.insert("filament_map".into(), json!([1]));
            object.insert("filament_nozzle_map".into(), json!([0]));
            object.insert("filament_self_index".into(), json!([1]));
            object.insert("filament_is_mixed".into(), json!([false]));
            object.insert("filament_mixed_gradient".into(), json!([0]));
            object.insert("flush_volumes_matrix".into(), json!([0]));
        }
        serde_json::to_vec_pretty(&settings).unwrap()
    }

    fn round_trip_paths(directory: &TempDir) -> [PathBuf; 3] {
        [
            directory.path().join("candidate.3mf"),
            directory.path().join("first-save.3mf"),
            directory.path().join("reopened-save.3mf"),
        ]
    }

    fn write_valid_round_trip(paths: &[PathBuf; 3]) {
        write_archive(&paths[0], ArchiveOptions::default());
        write_archive(
            &paths[1],
            ArchiveOptions {
                object_id: 7,
                gui_dialect: true,
                safe_gui_artifacts: true,
                ..ArchiveOptions::default()
            },
        );
        write_archive(
            &paths[2],
            ArchiveOptions {
                object_id: 11,
                gui_dialect: true,
                safe_gui_artifacts: true,
                ..ArchiveOptions::default()
            },
        );
    }

    #[test]
    fn accepts_id_renumbering_and_safe_gui_regeneration() {
        let directory = TempDir::new().unwrap();
        let paths = round_trip_paths(&directory);
        write_valid_round_trip(&paths);

        let report = validate_a1mini_round_trip(&paths[0], &paths[1], &paths[2]).unwrap();

        assert!(report.is_valid, "{:?}", report.issues);
        assert!(report.checks.geometry_stable);
        assert!(report.checks.object_part_membership_stable);
        assert!(report.checks.placement_stable);
        assert!(report.checks.no_forbidden_stale_artifacts);
        assert_eq!(report.artifacts.len(), 3);
        assert!(report.artifacts.iter().all(|artifact| {
            artifact.byte_size > 0
                && artifact.sha256.len() == 64
                && artifact.semantic_sha256.is_some()
        }));
    }

    #[test]
    fn rejects_repaired_or_lost_geometry() {
        let directory = TempDir::new().unwrap();
        let paths = round_trip_paths(&directory);
        write_valid_round_trip(&paths);
        write_archive(
            &paths[2],
            ArchiveOptions {
                object_id: 11,
                vertex_x: 9.5,
                gui_dialect: true,
                safe_gui_artifacts: true,
                ..ArchiveOptions::default()
            },
        );

        let report = validate_a1mini_round_trip(&paths[0], &paths[1], &paths[2]).unwrap();

        assert!(!report.is_valid);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| { issue.code == A1MiniRoundTripIssueCode::GeometryChanged })
        );
    }

    #[test]
    fn rejects_material_identity_changes() {
        let directory = TempDir::new().unwrap();
        let paths = round_trip_paths(&directory);
        write_valid_round_trip(&paths);
        write_archive(
            &paths[2],
            ArchiveOptions {
                object_id: 11,
                material: A1MiniMaterial::Petg,
                gui_dialect: true,
                safe_gui_artifacts: true,
                ..ArchiveOptions::default()
            },
        );

        let report = validate_a1mini_round_trip(&paths[0], &paths[1], &paths[2]).unwrap();

        assert!(!report.is_valid);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| { issue.code == A1MiniRoundTripIssueCode::MaterialIdentityChanged })
        );
    }

    #[test]
    fn rejects_an_ams_enabled_save() {
        let directory = TempDir::new().unwrap();
        let paths = round_trip_paths(&directory);
        write_valid_round_trip(&paths);
        write_archive(
            &paths[2],
            ArchiveOptions {
                object_id: 11,
                gui_dialect: true,
                ams_enabled: true,
                safe_gui_artifacts: true,
                ..ArchiveOptions::default()
            },
        );

        let report = validate_a1mini_round_trip(&paths[0], &paths[1], &paths[2]).unwrap();

        assert!(!report.is_valid);
        assert!(!report.checks.one_external_spool_no_ams_stable);
        assert!(
            report.issues.iter().any(|issue| {
                issue.code == A1MiniRoundTripIssueCode::InvalidExternalSpoolContract
            })
        );
    }

    #[test]
    fn placement_comparison_accepts_only_bounded_gui_numeric_serialization_noise() {
        let instance = |object_sha256: &str, x: f64| PlacementInstance {
            object_sha256: object_sha256.into(),
            printable: true,
            transform: Transform3mf {
                values: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, x, 20.0, 0.0],
            },
            printable_bounds: Some(AxisAlignedBounds {
                min: [x, 20.0, 0.0],
                max: [x + 10.0, 30.0, 5.0],
            }),
        };
        let candidate = vec![PlacementPlate {
            effective_slots: vec![1],
            instances: vec![
                instance("object-a", 20.016_424_960_8),
                instance("object-b", 40.0),
            ],
        }];
        let mut gui_save = candidate.clone();
        gui_save[0].instances.reverse();
        {
            let saved_a = gui_save[0]
                .instances
                .iter_mut()
                .find(|instance| instance.object_sha256 == "object-a")
                .unwrap();
            saved_a.transform.values[9] = 20.016_425;
            saved_a.printable_bounds.as_mut().unwrap().min[0] += 5.0e-7;
        }

        assert!(placement_snapshots_equivalent(&candidate, &gui_save));

        let saved_a = gui_save[0]
            .instances
            .iter_mut()
            .find(|instance| instance.object_sha256 == "object-a")
            .unwrap();
        saved_a.transform.values[9] += GUI_PLACEMENT_TOLERANCE_MM * 2.0;
        assert!(!placement_snapshots_equivalent(&candidate, &gui_save));
    }

    #[test]
    fn rejects_placement_changes() {
        let directory = TempDir::new().unwrap();
        let paths = round_trip_paths(&directory);
        write_valid_round_trip(&paths);
        write_archive(
            &paths[2],
            ArchiveOptions {
                object_id: 11,
                placement_x: 21.0,
                gui_dialect: true,
                safe_gui_artifacts: true,
                ..ArchiveOptions::default()
            },
        );

        let report = validate_a1mini_round_trip(&paths[0], &paths[1], &paths[2]).unwrap();

        assert!(!report.is_valid);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| { issue.code == A1MiniRoundTripIssueCode::PlacementChanged })
        );
    }

    #[test]
    fn rejects_unexpected_slice_payloads() {
        let directory = TempDir::new().unwrap();
        let paths = round_trip_paths(&directory);
        write_valid_round_trip(&paths);
        write_archive(
            &paths[2],
            ArchiveOptions {
                object_id: 11,
                gui_dialect: true,
                safe_gui_artifacts: true,
                stale_slice_artifact: true,
                ..ArchiveOptions::default()
            },
        );

        let report = validate_a1mini_round_trip(&paths[0], &paths[1], &paths[2]).unwrap();

        assert!(!report.is_valid);
        assert!(!report.checks.no_forbidden_stale_artifacts);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| { issue.code == A1MiniRoundTripIssueCode::ForbiddenStaleArtifact })
        );
    }
}
