//! Deterministic Bambu Lab A1 mini single-spool Project 3MF writer.
//!
//! This module consumes only the canonical backend plan. It writes one
//! unsliced Project 3MF per packed A1 mini plate and deliberately refuses any
//! source construct whose geometry, material, or plate semantics cannot be
//! proven after serialization.

use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Seek, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;
use u1_planner::{
    ColorStrategy, Material, PackingStatus, PlannedJob, PlannedPlate, PlanningInput,
    PlanningResult, PrintableUnit, Printer, PrinterLoadout, RgbColor, Spool,
};
use u1_three_mf::{
    AxisAlignedBounds, ContentTypesBuilder, ExpectedSourceIdentity, MAIN_MODEL_PATH,
    MAIN_MODEL_RELATIONSHIPS_PATH, MODEL_RELATIONSHIP_TYPE, OpcPackageWriter, OpcRelationship,
    OpcWriteReport, OutputValidationPolicy, OutputValidationReport, ProjectAnalysis,
    StructuralOutputValidator, Transform3mf, ValidatedStagedPackage, analyze_project,
    decode_paint_annotation, encode_paint_annotation, relationships_xml,
};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

use crate::capability::adapter_context;
use crate::profiles::load_target_profiles;
use crate::schema::{A1MiniMaterial, build_project_settings};
use crate::{
    A1MINI_ADAPTER_ID, A1MINI_APPLICATION_VERSION, A1MINI_BED_DEPTH_MM, A1MINI_BED_WIDTH_MM,
    A1MINI_PRINTABLE_HEIGHT_MM, A1MiniError, A1MiniSpoolSpec, AdapterContext, MODEL_SETTINGS_PATH,
    PROJECT_SETTINGS_PATH,
};

const MAX_SOURCE_ARCHIVE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_METADATA_BYTES: u64 = 64 * 1024 * 1024;
const MAX_MODEL_BYTES: u64 = 1024 * 1024 * 1024;
const COPY_BUFFER_BYTES: usize = 256 * 1024;
const CANCELLATION_XML_EVENT_INTERVAL: usize = 1024;
const BOUNDS_TOLERANCE_MM: f64 = 0.02;
const TARGET_FILAMENT_MAP_MODE: &str = "Auto For Flush";
const SAFE_OBJECT_PROCESS_METADATA_KEYS: &[&str] = &[
    "ironing_type",
    "skeleton_infill_density",
    "skin_infill_density",
    "sparse_infill_anchor_max",
    "sparse_infill_density",
    "wall_loops",
];
const SAFE_PART_IDENTITY_METADATA_KEYS: &[&str] = &[
    "matrix",
    "source_file",
    "source_object_id",
    "source_offset_x",
    "source_offset_y",
    "source_offset_z",
    "source_volume_id",
];
const QUALIFIED_BRIM_TYPES: &[&str] = &[
    "no_brim",
    "outer_only",
    "inner_only",
    "outer_and_inner",
    "auto_brim",
    "brim_ears",
];
const MAX_TEXT_INFO_ATTRIBUTES: usize = 24;
const MAX_TEXT_INFO_TEXT_BYTES: usize = 1024;
const MAX_TEXT_INFO_LABEL_BYTES: usize = 255;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A1MiniMaterialSubstitutionEvidence {
    pub scope_id: String,
    pub requirement_id: String,
    pub approval_id: String,
    pub source_material: Material,
    pub target_material: Material,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A1MiniPreparedSpool {
    pub spool_id: String,
    pub spool_name: String,
    pub material: Material,
    pub color: String,
    pub profile: String,
    pub setting_id: String,
    pub filament_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A1MiniPreparedArtifact {
    pub plate_id: String,
    pub job_id: String,
    pub file_name: String,
    pub source_plate_ids: Vec<u32>,
    pub source_unit_ids: Vec<String>,
    pub spool: A1MiniPreparedSpool,
    pub material_substitutions: Vec<A1MiniMaterialSubstitutionEvidence>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A1MiniPreparation {
    pub adapter_id: String,
    pub source_sha256: String,
    pub plan_fingerprint: String,
    pub artifacts: Vec<A1MiniPreparedArtifact>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A1MiniPublishedArtifact {
    pub plate_id: String,
    pub job_id: String,
    pub file_name: String,
    pub path: PathBuf,
    pub byte_size: u64,
    pub sha256: String,
    pub source_unit_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A1MiniConversionResult {
    pub adapter_id: String,
    pub artifacts: Vec<A1MiniPublishedArtifact>,
    pub warnings: Vec<String>,
}

/// Standalone target-contract validation for an already generated native A1
/// mini candidate. Source-bound geometry and placement proofs remain part of
/// conversion itself; this report verifies the unsliced package and exact
/// one-slot/no-AMS target schema without trusting a file extension.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A1MiniOutputValidationReport {
    pub valid: bool,
    pub adapter_id: String,
    pub material: Option<A1MiniMaterial>,
    pub plate_count: usize,
    pub instance_count: usize,
    pub vertex_count: u64,
    pub triangle_count: u64,
    pub structural: OutputValidationReport,
    pub semantic_issues: Vec<String>,
}

#[derive(Clone, Debug)]
struct ArtifactBuildPlan {
    prepared: A1MiniPreparedArtifact,
    spool: A1MiniSpoolSpec,
    project_settings: Vec<u8>,
    units: Vec<ArtifactUnit>,
    object_slot_maps: BTreeMap<u32, BTreeMap<u8, u8>>,
    resource_slot_maps: BTreeMap<(String, u32), BTreeMap<u8, u8>>,
    selected_root_resource_ids: BTreeSet<u32>,
    external_paths: BTreeSet<String>,
    selected_instances: BTreeMap<(u32, u32), InstancePlacement>,
}

#[derive(Clone, Debug)]
struct ArtifactUnit {
    source: PrintableUnit,
    identify_id: u64,
    target_instance_id: u32,
    target_bounds: AxisAlignedBounds,
    target_transform: Transform3mf,
}

#[derive(Clone, Copy, Debug)]
struct InstancePlacement {
    delta_x: f64,
    delta_y: f64,
    delta_z: f64,
}

#[derive(Clone)]
struct GeometryEvidence {
    vertices: u64,
    triangles: u64,
    fingerprint: Sha256,
}

impl Default for GeometryEvidence {
    fn default() -> Self {
        let mut fingerprint = Sha256::new();
        fingerprint.update(b"u1-a1mini-geometry-v1\0");
        Self {
            vertices: 0,
            triangles: 0,
            fingerprint,
        }
    }
}

impl GeometryEvidence {
    fn sha256(&self) -> String {
        format!("{:x}", self.fingerprint.clone().finalize())
    }
}

#[derive(Default)]
struct PublishedDestinationRollback {
    published: Vec<PublishedDestinationIdentity>,
    committed: bool,
}

struct PublishedDestinationIdentity {
    path: PathBuf,
    byte_size: u64,
    sha256: String,
}

impl PublishedDestinationRollback {
    fn track(&mut self, report: &OpcWriteReport) {
        self.published.push(PublishedDestinationIdentity {
            path: report.destination.clone(),
            byte_size: report.package_bytes,
            sha256: report.package_sha256.clone(),
        });
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for PublishedDestinationRollback {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for published in self.published.iter().rev() {
            let still_owned = hash_file(&published.path).is_ok_and(|(byte_size, sha256)| {
                byte_size == published.byte_size && sha256 == published.sha256
            });
            if still_owned {
                let _ = fs::remove_file(&published.path);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RetainedMetadataScope {
    Object,
    Part,
}

/// Stable fingerprint used to bind a conversion request to the canonical
/// planner input and result retained by the backend.
pub fn canonical_a1mini_plan_fingerprint(
    input: &PlanningInput,
    result: &PlanningResult,
) -> Result<String, A1MiniError> {
    let bytes = serde_json::to_vec(&(input, result)).map_err(|source| A1MiniError::Json {
        path: "canonical A1 mini print plan".into(),
        source,
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// Validates and describes all A1 mini artifacts without writing output.
/// Production preparation remains disabled until the exact adapter has GUI
/// qualification evidence for both PLA and PETG.
pub fn prepare_a1mini_conversion(
    application_path: &Path,
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
) -> Result<A1MiniPreparation, A1MiniError> {
    let context = adapter_context(application_path)?;
    if !context.capability.conversion_available {
        return Err(A1MiniError::Capability(context.capability.issues.join(" ")));
    }
    prepare_with_context(source_path, analysis, input, result, &context).map(|(value, _)| value)
}

/// Publishes one deterministic, validated Project 3MF for every canonical A1
/// mini target plate. Existing files are never overwritten.
pub fn convert_a1mini_plates(
    application_path: &Path,
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    destination_directory: &Path,
) -> Result<A1MiniConversionResult, A1MiniError> {
    convert_a1mini_plates_cancellable(
        application_path,
        source_path,
        analysis,
        input,
        result,
        destination_directory,
        || false,
    )
}

/// Cancellable production conversion for A1 mini target plates.
///
/// `is_cancelled` is polled cooperatively during source snapshotting, per
/// artifact, during streaming XML rewrites, before validation, and around
/// publication. Returning `true` fails closed with [`A1MiniError::Cancelled`]
/// and removes any destination published by this invocation.
pub fn convert_a1mini_plates_cancellable<F>(
    application_path: &Path,
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    destination_directory: &Path,
    mut is_cancelled: F,
) -> Result<A1MiniConversionResult, A1MiniError>
where
    F: FnMut() -> bool,
{
    cancellation_checkpoint(&mut is_cancelled)?;
    let context = adapter_context(application_path)?;
    cancellation_checkpoint(&mut is_cancelled)?;
    if !context.capability.conversion_available {
        return Err(A1MiniError::Capability(context.capability.issues.join(" ")));
    }
    convert_with_context(
        source_path,
        analysis,
        input,
        result,
        destination_directory,
        &context,
        &mut is_cancelled,
    )
}

/// Generates structurally validated candidates for the two mandatory GUI
/// qualification passes. This is the only API allowed to bypass the embedded
/// GUI-evidence gate; the exact application, executable, manifest, and profile
/// hashes must still match.
pub fn build_a1mini_qualification_candidates(
    application_path: &Path,
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    destination_directory: &Path,
) -> Result<A1MiniConversionResult, A1MiniError> {
    let context = adapter_context(application_path)?;
    if !context.capability.installation_supported() {
        return Err(A1MiniError::Capability(context.capability.issues.join(" ")));
    }
    let mut never_cancelled = || false;
    convert_with_context(
        source_path,
        analysis,
        input,
        result,
        destination_directory,
        &context,
        &mut never_cancelled,
    )
}

/// Validates the standalone A1 mini target contract of an unsliced Project
/// 3MF. This does not replace the stronger source/plan-bound validation that
/// runs automatically before publication.
pub fn validate_a1mini_output(path: &Path) -> Result<A1MiniOutputValidationReport, A1MiniError> {
    let structural = StructuralOutputValidator::new(OutputValidationPolicy::strict_unsliced())
        .validate_path(path)
        .map_err(|error| A1MiniError::SemanticValidation(error.to_string()))?;
    let mut issues = Vec::new();
    let mut material = None;
    let mut archive = ZipArchive::new(File::open(path).map_err(|source| A1MiniError::Read {
        path: path.to_path_buf(),
        source,
    })?)?;
    let names = archive.file_names().map(str::to_owned).collect::<Vec<_>>();
    if let Some(unexpected) = names.iter().find(|name| {
        !matches!(
            name.as_str(),
            "[Content_Types].xml"
                | "_rels/.rels"
                | MAIN_MODEL_PATH
                | MAIN_MODEL_RELATIONSHIPS_PATH
                | PROJECT_SETTINGS_PATH
                | MODEL_SETTINGS_PATH
        ) && !(name.starts_with("3D/") && name.ends_with(".model"))
    }) {
        issues.push(format!(
            "entry {unexpected:?} is outside the native A1 mini writer allowlist"
        ));
    }
    match archive.by_name(PROJECT_SETTINGS_PATH) {
        Ok(mut entry) if entry.size() <= MAX_METADATA_BYTES => {
            let mut bytes = Vec::with_capacity(entry.size() as usize);
            if let Err(error) = entry.read_to_end(&mut bytes) {
                issues.push(format!("project settings cannot be read: {error}"));
            } else {
                match serde_json::from_slice::<serde_json::Value>(&bytes) {
                    Ok(settings) => {
                        let declared = settings
                            .get("filament_type")
                            .and_then(serde_json::Value::as_array)
                            .filter(|values| values.len() == 1)
                            .and_then(|values| values[0].as_str());
                        material = match declared {
                            Some("PLA") => Some(A1MiniMaterial::Pla),
                            Some("PETG") => Some(A1MiniMaterial::Petg),
                            Some(other) => {
                                issues.push(format!(
                                    "filament material {other:?} is not qualified for this adapter"
                                ));
                                None
                            }
                            None => {
                                issues.push(
                                    "project settings do not declare exactly one filament material"
                                        .into(),
                                );
                                None
                            }
                        };
                        if let Some(material) = material
                            && let Err(error) =
                                crate::validate_a1mini_project_settings(&bytes, material)
                        {
                            issues.push(error.to_string());
                        }
                    }
                    Err(error) => {
                        issues.push(format!("project settings are invalid JSON: {error}"))
                    }
                }
            }
        }
        Ok(entry) => issues.push(format!(
            "project settings contain {} bytes, above the bounded limit",
            entry.size()
        )),
        Err(error) => issues.push(format!("project settings are missing: {error}")),
    }
    drop(archive);

    let analysis = analyze_project(path).map_err(|error| {
        A1MiniError::SemanticValidation(format!("output cannot be analyzed: {error}"))
    })?;
    if analysis.source.application != u1_three_mf::SourceApplication::BambuStudio
        || analysis.source.application_version.as_deref() != Some(A1MINI_APPLICATION_VERSION)
    {
        issues.push(format!(
            "Application metadata is not BambuStudio-{A1MINI_APPLICATION_VERSION}"
        ));
    }
    if analysis.printer.model.as_deref() != Some("Bambu Lab A1 mini")
        || analysis.printer.variant.as_deref() != Some("0.4")
        || analysis.printer.nozzle_diameters_mm != [0.4]
    {
        issues.push("printer identity is not Bambu Lab A1 mini 0.4 nozzle".into());
    }
    if analysis.plates.len() != 1 || analysis.plates.first().map(|plate| plate.id) != Some(1) {
        issues.push("output must contain exactly target plate 1".into());
    }
    if analysis
        .plates
        .iter()
        .any(|plate| plate.effective_slots != [1])
        || analysis.objects.iter().any(|object| {
            object.effective_slots.iter().any(|slot| *slot != 1)
                || object
                    .parts
                    .iter()
                    .flat_map(|part| part.effective_slots.iter())
                    .any(|slot| *slot != 1)
        })
    {
        issues.push("output retains a logical filament assignment other than slot 1".into());
    }
    if analysis.source.has_sliced_artifacts {
        issues.push("output contains sliced or derived artifacts".into());
    }
    let valid = structural.is_valid && !structural.has_errors() && issues.is_empty();
    Ok(A1MiniOutputValidationReport {
        valid,
        adapter_id: A1MINI_ADAPTER_ID.into(),
        material,
        plate_count: analysis.plates.len(),
        instance_count: analysis.summary.instance_count,
        vertex_count: analysis.summary.vertex_count,
        triangle_count: analysis.summary.triangle_count,
        structural,
        semantic_issues: issues,
    })
}

/// Revalidates an already-published A1 mini artifact against the exact
/// current source, canonical plan, qualified profiles, and prepared artifact.
///
/// Unlike [`validate_a1mini_output`], this function reconstructs the private
/// writer plan and source-derived geometry evidence before accepting the
/// candidate. It is intended for restart recovery, where a self-consistent
/// manifest digest is not proof that the 3MF came from the approved source.
#[allow(clippy::too_many_arguments)]
pub fn validate_a1mini_output_against_plan(
    application_path: &Path,
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    expected_artifact: &A1MiniPreparedArtifact,
    candidate_path: &Path,
) -> Result<A1MiniOutputValidationReport, A1MiniError> {
    validate_a1mini_outputs_against_plan(
        application_path,
        source_path,
        analysis,
        input,
        result,
        &[(expected_artifact, candidate_path)],
    )?
    .into_iter()
    .next()
    .ok_or_else(|| A1MiniError::Plan("no A1 mini recovery candidate was supplied".into()))
}

/// Batch form of [`validate_a1mini_output_against_plan`]. Adapter capability,
/// canonical planning, and the immutable source snapshot are constructed once
/// for the complete recovery bundle.
#[allow(clippy::too_many_arguments)]
pub fn validate_a1mini_outputs_against_plan(
    application_path: &Path,
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    candidates: &[(&A1MiniPreparedArtifact, &Path)],
) -> Result<Vec<A1MiniOutputValidationReport>, A1MiniError> {
    if candidates.is_empty() {
        return Err(A1MiniError::Plan(
            "no A1 mini recovery candidate was supplied".into(),
        ));
    }
    let context = adapter_context(application_path)?;
    if !context.capability.conversion_available {
        return Err(A1MiniError::Capability(context.capability.issues.join(" ")));
    }
    let (preparation, plans) =
        prepare_with_context(source_path, analysis, input, result, &context)?;
    if preparation.source_sha256 != analysis.input.sha256 {
        return Err(A1MiniError::SourceIdentityChanged);
    }
    let scratch = tempfile::tempdir().map_err(|source| A1MiniError::Write {
        path: std::env::temp_dir(),
        source,
    })?;
    let mut never_cancelled = || false;
    let source_snapshot = snapshot_source(
        source_path,
        &analysis.input,
        scratch.path(),
        &mut never_cancelled,
    )?;
    let mut reports = Vec::with_capacity(candidates.len());
    let mut seen = BTreeSet::new();
    for (expected_artifact, candidate_path) in candidates {
        if !seen.insert((
            expected_artifact.job_id.as_str(),
            expected_artifact.file_name.as_str(),
        )) {
            return Err(A1MiniError::Plan(format!(
                "prepared A1 mini artifact {} / {} was supplied more than once",
                expected_artifact.job_id, expected_artifact.file_name
            )));
        }
        let plan = plans
            .iter()
            .find(|plan| {
                plan.prepared.job_id == expected_artifact.job_id
                    && plan.prepared.plate_id == expected_artifact.plate_id
                    && plan.prepared.file_name == expected_artifact.file_name
            })
            .ok_or_else(|| {
                A1MiniError::Plan(format!(
                    "prepared A1 mini artifact {} / {} is not present in the canonical plan",
                    expected_artifact.job_id, expected_artifact.file_name
                ))
            })?;
        if &plan.prepared != *expected_artifact {
            return Err(A1MiniError::Plan(format!(
                "prepared A1 mini artifact {} / {} changed before recovery validation",
                expected_artifact.job_id, expected_artifact.file_name
            )));
        }
        let (_rewritten, _entries, geometry) = rewrite_models_to_archive(
            source_snapshot.path(),
            plan,
            scratch.path(),
            analysis.source.title.as_deref(),
            &mut never_cancelled,
        )?;
        validate_a1mini_candidate(candidate_path, plan, &geometry)?;

        let report = validate_a1mini_output(candidate_path)?;
        if !report.valid {
            return Err(A1MiniError::SemanticValidation(format!(
                "published A1 mini artifact failed standalone target validation: {}",
                report.semantic_issues.join("; ")
            )));
        }
        reports.push(report);
    }
    verify_source_identity_cancellable(source_path, &analysis.input, &mut never_cancelled)?;
    Ok(reports)
}

fn prepare_with_context(
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    context: &AdapterContext,
) -> Result<(A1MiniPreparation, Vec<ArtifactBuildPlan>), A1MiniError> {
    verify_source_identity(source_path, &analysis.input)?;
    let plan_fingerprint = canonical_a1mini_plan_fingerprint(input, result)?;
    let base_name = safe_file_component(
        source_path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("project"),
    );
    let plans = build_artifact_plans(analysis, input, result, context, &base_name)?;
    if plans.is_empty() {
        return Err(A1MiniError::Plan(
            "the canonical plan contains no A1 mini target plates".into(),
        ));
    }
    let warnings = vec![
        "A1 mini outputs are unsliced. Open each file in Bambu Studio, verify the external spool and plate placement, then slice before printing."
            .into(),
        "Every output contains exactly one packed plate, one logical filament slot, and no AMS/filament switcher."
            .into(),
    ];
    Ok((
        A1MiniPreparation {
            adapter_id: A1MINI_ADAPTER_ID.into(),
            source_sha256: analysis.input.sha256.clone(),
            plan_fingerprint,
            artifacts: plans.iter().map(|plan| plan.prepared.clone()).collect(),
            warnings,
        },
        plans,
    ))
}

fn build_artifact_plans(
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    context: &AdapterContext,
    base_name: &str,
) -> Result<Vec<ArtifactBuildPlan>, A1MiniError> {
    if result.has_hard_errors() {
        return Err(A1MiniError::Plan(format!(
            "{} blocking planner error(s) remain",
            result.errors.len()
        )));
    }
    validate_a1mini_config(input)?;
    let scoped_units = input
        .scopes
        .iter()
        .flat_map(|scope| {
            scope
                .units
                .iter()
                .map(move |unit| ((scope.id.as_str(), unit.id.as_str()), (scope, unit)))
        })
        .collect::<BTreeMap<_, _>>();
    let expected_unit_count = input
        .scopes
        .iter()
        .map(|scope| scope.units.len())
        .sum::<usize>();
    if scoped_units.len() != expected_unit_count {
        return Err(A1MiniError::Plan(
            "planning input contains duplicate scope/unit identities".into(),
        ));
    }
    let jobs = unique_by_id(&result.jobs, |job| job.id.as_str(), "job")?;
    let _plates_by_id = unique_by_id(&result.plates, |plate| plate.id.as_str(), "plate")?;
    let analysis_plates = analysis
        .plates
        .iter()
        .map(|plate| (plate.id, plate))
        .collect::<BTreeMap<_, _>>();
    if analysis_plates.len() != analysis.plates.len() {
        return Err(A1MiniError::Plan(
            "source analysis contains duplicate plate IDs".into(),
        ));
    }
    let root_objects = analysis
        .objects
        .iter()
        .filter(|object| object.source_model_path.is_none())
        .map(|object| (object.source_object_id.unwrap_or(object.id), object))
        .collect::<BTreeMap<_, _>>();
    if root_objects.len()
        != analysis
            .objects
            .iter()
            .filter(|object| object.source_model_path.is_none())
            .count()
    {
        return Err(A1MiniError::Plan(
            "source analysis contains duplicate root resource IDs".into(),
        ));
    }
    let inventory = input
        .inventory
        .iter()
        .map(|spool| (spool.id.as_str(), spool))
        .collect::<BTreeMap<_, _>>();
    if inventory.len() != input.inventory.len() {
        return Err(A1MiniError::Plan(
            "physical spool inventory contains duplicate IDs".into(),
        ));
    }

    let a1_plates = result
        .plates
        .iter()
        .filter(|plate| plate.printer == Printer::A1Mini)
        .collect::<Vec<_>>();
    let mut job_plate_units = BTreeMap::<&str, BTreeSet<(&str, &str)>>::new();
    let mut globally_scheduled = BTreeSet::new();
    let mut plans = Vec::with_capacity(a1_plates.len());
    let mut target_profiles = BTreeMap::new();
    for (plate_index, plate) in a1_plates.into_iter().enumerate() {
        ensure_a1mini_plate(plate)?;
        let job = jobs.get(plate.job_id.as_str()).copied().ok_or_else(|| {
            A1MiniError::Plan(format!(
                "A1 mini plate {} references missing job {}",
                plate.id, plate.job_id
            ))
        })?;
        ensure_a1mini_job(job)?;
        let spool_id = match &job.loadout {
            PrinterLoadout::A1Mini { spool_id } => spool_id,
            _ => unreachable!("A1 mini job loadout was validated"),
        };
        let spool = inventory.get(spool_id.as_str()).copied().ok_or_else(|| {
            A1MiniError::Plan(format!(
                "A1 mini job {} references missing spool {spool_id}",
                job.id
            ))
        })?;
        if !spool.available {
            return Err(A1MiniError::Plan(format!(
                "A1 mini spool {} ({}) is out of stock",
                spool.display_name, spool.id
            )));
        }
        if input
            .config
            .a1_mini
            .reserved_spool_ids
            .iter()
            .any(|reserved| reserved == &spool.id)
        {
            return Err(A1MiniError::Plan(format!(
                "A1 mini spool {} is explicitly marked unavailable to the A1 mini",
                spool.id
            )));
        }
        if !input
            .config
            .a1_mini
            .supported_materials
            .contains(&spool.material)
        {
            return Err(A1MiniError::Plan(format!(
                "material {:?} is not enabled for the A1 mini planner target",
                spool.material
            )));
        }
        let printable_materials = job.printable_materials.iter().collect::<BTreeSet<_>>();
        if printable_materials != BTreeSet::from([&spool.material]) {
            return Err(A1MiniError::Plan(format!(
                "A1 mini job {} printable materials do not match its one physical spool",
                job.id
            )));
        }
        let material = A1MiniMaterial::try_from(&spool.material)?;
        if let std::collections::btree_map::Entry::Vacant(entry) = target_profiles.entry(material) {
            entry.insert(load_target_profiles(context, material)?);
        }
        let resolved = target_profiles
            .get(&material)
            .cloned()
            .expect("the target profile was just inserted");
        let spool_spec = A1MiniSpoolSpec {
            spool_id: spool.id.clone(),
            display_name: spool.display_name.clone(),
            material,
            color: spool.actual_color(),
            material_substitution_approval_id: None,
        };
        let project_settings =
            build_project_settings(resolved.clone(), &spool_spec, &analysis.process.support)?;
        let prepared_spool = A1MiniPreparedSpool {
            spool_id: spool.id.clone(),
            spool_name: spool.display_name.clone(),
            material: spool.material.clone(),
            color: rgb_hex(spool.actual_color()),
            profile: resolved.filament.name,
            setting_id: resolved.filament.setting_id,
            filament_id: resolved.filament.filament_id,
        };

        let placements = plate
            .placements
            .iter()
            .map(|placement| {
                (
                    (
                        placement.unit.scope_id.as_str(),
                        placement.unit.unit_id.as_str(),
                    ),
                    placement,
                )
            })
            .collect::<BTreeMap<_, _>>();
        if placements.len() != plate.placements.len() || placements.len() != plate.units.len() {
            return Err(A1MiniError::Plan(format!(
                "A1 mini plate {} does not have an exact placement bijection",
                plate.id
            )));
        }
        let mut units = Vec::with_capacity(plate.units.len());
        let mut substitutions = Vec::new();
        let mut object_slot_maps = BTreeMap::new();
        let mut resource_slot_maps = BTreeMap::new();
        let mut selected_root_resource_ids = BTreeSet::new();
        let mut external_paths = BTreeSet::new();
        let mut selected_instances = BTreeMap::new();
        let mut source_plate_ids = BTreeSet::new();
        let mut source_instance_ids_by_object = BTreeMap::<u32, Vec<u32>>::new();
        for unit_ref in &plate.units {
            let key = (unit_ref.scope_id.as_str(), unit_ref.unit_id.as_str());
            if !globally_scheduled.insert(key) {
                return Err(A1MiniError::Plan(format!(
                    "unit {}/{} is scheduled on more than one A1 mini plate",
                    key.0, key.1
                )));
            }
            job_plate_units
                .entry(job.id.as_str())
                .or_default()
                .insert(key);
            if !job.units.iter().any(|candidate| {
                candidate.scope_id == unit_ref.scope_id && candidate.unit_id == unit_ref.unit_id
            }) {
                return Err(A1MiniError::Plan(format!(
                    "plate {} unit {}/{} is absent from job {}",
                    plate.id, key.0, key.1, job.id
                )));
            }
            if !job
                .scope_ids
                .iter()
                .any(|scope_id| scope_id == &unit_ref.scope_id)
            {
                return Err(A1MiniError::Plan(format!(
                    "job {} does not declare scope {} used by its plate",
                    job.id, unit_ref.scope_id
                )));
            }
            let memberships = result
                .plates
                .iter()
                .filter(|candidate| {
                    candidate.units.iter().any(|candidate_unit| {
                        candidate_unit.scope_id == unit_ref.scope_id
                            && candidate_unit.unit_id == unit_ref.unit_id
                    })
                })
                .collect::<Vec<_>>();
            if memberships.len() != 1 || memberships[0].id != plate.id {
                return Err(A1MiniError::Plan(format!(
                    "unit {}/{} is not assigned to exactly one canonical target plate",
                    unit_ref.scope_id, unit_ref.unit_id
                )));
            }
            let (scope, unit) = scoped_units.get(&key).copied().ok_or_else(|| {
                A1MiniError::Plan(format!(
                    "A1 mini plate references unknown unit {}/{}",
                    key.0, key.1
                ))
            })?;
            if unit.source_model_path.is_some() {
                return Err(A1MiniError::Plan(format!(
                    "source unit {} is a direct external build item; that Production layout is not qualified for the A1 mini writer",
                    unit.source_unit_id
                )));
            }
            let placement = placements.get(&key).copied().ok_or_else(|| {
                A1MiniError::Plan(format!(
                    "A1 mini plate {} has no placement for {}/{}",
                    plate.id, key.0, key.1
                ))
            })?;
            let source_plate_id = parse_source_plate_id(unit.source_plate_id.as_deref())?;
            let source_plate = analysis_plates
                .get(&source_plate_id)
                .copied()
                .ok_or_else(|| {
                    A1MiniError::Plan(format!(
                        "source plate {source_plate_id} is absent from analysis"
                    ))
                })?;
            let source_instance = source_plate
                .instances
                .iter()
                .find(|instance| {
                    instance.object_id == unit.source_object_id
                        && instance.instance_id == unit.source_instance_id
                        && instance.printable
                })
                .ok_or_else(|| {
                    A1MiniError::Plan(format!(
                        "source unit {} has no matching printable source instance",
                        unit.source_unit_id
                    ))
                })?;
            let identify_id = source_instance.identify_id.ok_or_else(|| {
                A1MiniError::Plan(format!(
                    "source unit {} has no preserved Bambu identify_id",
                    unit.source_unit_id
                ))
            })?;
            let source_bounds = source_instance.printable_bounds.ok_or_else(|| {
                A1MiniError::Plan(format!(
                    "source unit {} has unknown printable bounds",
                    unit.source_unit_id
                ))
            })?;
            validate_unit_bounds(unit, source_bounds)?;
            let delta_x = placement.target_min_x_mm - source_bounds.min[0];
            let delta_y = placement.target_min_y_mm - source_bounds.min[1];
            let target_bounds = translate_bounds(source_bounds, delta_x, delta_y)?;
            validate_target_bounds(unit, target_bounds)?;
            let instance_placement = InstancePlacement {
                delta_x,
                delta_y,
                delta_z: 0.0,
            };
            if selected_instances
                .insert(
                    (unit.source_object_id, unit.source_instance_id),
                    instance_placement,
                )
                .is_some()
            {
                return Err(A1MiniError::Plan(format!(
                    "source object {}/instance {} appears twice on A1 mini plate {}",
                    unit.source_object_id, unit.source_instance_id, plate.id
                )));
            }
            source_instance_ids_by_object
                .entry(unit.source_object_id)
                .or_default()
                .push(unit.source_instance_id);
            let slot_map = a1_slot_map_for_unit(scope, unit, job, spool, &mut substitutions)?;
            let source_object = root_objects
                .get(&unit.source_object_id)
                .copied()
                .ok_or_else(|| {
                    A1MiniError::Plan(format!(
                        "source root object {} is absent from analysis",
                        unit.source_object_id
                    ))
                })?;
            ensure_object_slots_are_mapped(source_object, &slot_map, unit)?;
            merge_slot_map(
                &mut object_slot_maps,
                unit.source_object_id,
                &slot_map,
                "source object",
            )?;
            selected_root_resource_ids.insert(unit.source_object_id);
            for part in &source_object.parts {
                let path = normalized_model_path(
                    part.component_path.as_deref().unwrap_or(MAIN_MODEL_PATH),
                )?;
                merge_resource_slot_map(
                    &mut resource_slot_maps,
                    (path.clone(), part.id),
                    &slot_map,
                )?;
                if path == MAIN_MODEL_PATH {
                    selected_root_resource_ids.insert(part.id);
                } else {
                    external_paths.insert(path);
                }
            }
            source_plate_ids.insert(source_plate_id);
            units.push(ArtifactUnit {
                source: unit.clone(),
                identify_id,
                target_instance_id: 0,
                target_bounds,
                target_transform: translated_transform_value(
                    source_instance.transform.unwrap_or(Transform3mf::IDENTITY),
                    delta_x,
                    delta_y,
                )?,
            });
        }
        if units.is_empty() {
            return Err(A1MiniError::Plan(format!(
                "A1 mini plate {} is empty",
                plate.id
            )));
        }
        validate_no_overlaps(&units)?;
        for values in source_instance_ids_by_object.values_mut() {
            values.sort_unstable();
            values.dedup();
        }
        for unit in &mut units {
            unit.target_instance_id = source_instance_ids_by_object
                .get(&unit.source.source_object_id)
                .and_then(|ids| {
                    ids.iter()
                        .position(|id| *id == unit.source.source_instance_id)
                })
                .and_then(|index| u32::try_from(index).ok())
                .ok_or_else(|| {
                    A1MiniError::Plan("failed to derive dense target instance IDs".into())
                })?;
        }
        substitutions.sort_by(|left, right| {
            (&left.scope_id, &left.requirement_id, &left.approval_id).cmp(&(
                &right.scope_id,
                &right.requirement_id,
                &right.approval_id,
            ))
        });
        substitutions.dedup();
        let file_name = format!(
            "{base_name}__A1-mini__plate-{:02}__{}.3mf",
            plate_index + 1,
            safe_file_component(&spool.display_name)
        );
        plans.push(ArtifactBuildPlan {
            prepared: A1MiniPreparedArtifact {
                plate_id: plate.id.clone(),
                job_id: job.id.clone(),
                file_name,
                source_plate_ids: source_plate_ids.into_iter().collect(),
                source_unit_ids: units
                    .iter()
                    .map(|unit| unit.source.source_unit_id.clone())
                    .collect(),
                spool: prepared_spool,
                material_substitutions: substitutions,
            },
            spool: spool_spec,
            project_settings,
            units,
            object_slot_maps,
            resource_slot_maps,
            selected_root_resource_ids,
            external_paths,
            selected_instances,
        });
    }
    validate_job_plate_bijection(result, &job_plate_units)?;
    let mut names = BTreeSet::new();
    for plan in &plans {
        if !names.insert(plan.prepared.file_name.to_ascii_lowercase()) {
            return Err(A1MiniError::Plan(format!(
                "A1 mini output file name {:?} is not unique",
                plan.prepared.file_name
            )));
        }
    }
    Ok(plans)
}

fn unique_by_id<'a, T>(
    values: &'a [T],
    id: impl Fn(&'a T) -> &'a str,
    label: &str,
) -> Result<BTreeMap<&'a str, &'a T>, A1MiniError> {
    let values_by_id = values
        .iter()
        .map(|value| (id(value), value))
        .collect::<BTreeMap<_, _>>();
    if values_by_id.len() != values.len() {
        return Err(A1MiniError::Plan(format!(
            "canonical plan contains duplicate {label} IDs"
        )));
    }
    Ok(values_by_id)
}

fn validate_a1mini_config(input: &PlanningInput) -> Result<(), A1MiniError> {
    let config = &input.config.a1_mini;
    if !config.enabled
        || (config.build_volume.width - A1MINI_BED_WIDTH_MM).abs() > f64::EPSILON
        || (config.build_volume.depth - A1MINI_BED_DEPTH_MM).abs() > f64::EPSILON
        || (config.build_volume.height - A1MINI_PRINTABLE_HEIGHT_MM).abs() > f64::EPSILON
    {
        return Err(A1MiniError::Plan(
            "canonical planner input does not enable the exact 180 x 180 x 180 mm A1 mini target"
                .into(),
        ));
    }
    Ok(())
}

fn ensure_a1mini_job(job: &PlannedJob) -> Result<(), A1MiniError> {
    if job.printer != Printer::A1Mini
        || job.strategy != ColorStrategy::A1Mono
        || !matches!(job.loadout, PrinterLoadout::A1Mini { .. })
        || job.full_spectrum_process.is_some()
    {
        return Err(A1MiniError::Plan(format!(
            "job {} is not a plain A1 mini single-spool job",
            job.id
        )));
    }
    Ok(())
}

fn ensure_a1mini_plate(plate: &PlannedPlate) -> Result<(), A1MiniError> {
    if plate.printer != Printer::A1Mini
        || plate.packing_status != PackingStatus::PackedAabb
        || !plate.individual_bounds_validated
        || plate.prime_tower.is_some()
        || plate.full_spectrum_process.is_some()
    {
        return Err(A1MiniError::Plan(format!(
            "plate {} is not a packed, tower-free A1 mini plate",
            plate.id
        )));
    }
    Ok(())
}

fn validate_job_plate_bijection(
    result: &PlanningResult,
    scheduled: &BTreeMap<&str, BTreeSet<(&str, &str)>>,
) -> Result<(), A1MiniError> {
    for job in result
        .jobs
        .iter()
        .filter(|job| job.printer == Printer::A1Mini)
    {
        ensure_a1mini_job(job)?;
        let expected = job
            .units
            .iter()
            .map(|unit| (unit.scope_id.as_str(), unit.unit_id.as_str()))
            .collect::<BTreeSet<_>>();
        let actual = scheduled.get(job.id.as_str()).cloned().unwrap_or_default();
        if expected != actual || expected.len() != job.units.len() {
            return Err(A1MiniError::Plan(format!(
                "A1 mini job {} units are not placed exactly once across its target plates",
                job.id
            )));
        }
    }
    Ok(())
}

fn a1_slot_map_for_unit(
    scope: &u1_planner::PrintScope,
    unit: &PrintableUnit,
    job: &PlannedJob,
    spool: &Spool,
    substitutions: &mut Vec<A1MiniMaterialSubstitutionEvidence>,
) -> Result<BTreeMap<u8, u8>, A1MiniError> {
    let requirement_ids = unit
        .requirement_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let requirements = scope
        .requirements
        .iter()
        .filter(|requirement| requirement_ids.contains(requirement.id.as_str()))
        .map(|requirement| (requirement.id.as_str(), requirement))
        .collect::<BTreeMap<_, _>>();
    if requirements.len() != requirement_ids.len() {
        return Err(A1MiniError::Plan(format!(
            "unit {} references missing material/color requirements",
            unit.source_unit_id
        )));
    }
    let mut covered = BTreeSet::new();
    let mut slot_map = BTreeMap::new();
    for mapping in &job.color_mappings {
        let relevant = mapping
            .source_requirement_ids
            .iter()
            .filter(|id| requirements.contains_key(id.as_str()))
            .collect::<Vec<_>>();
        if relevant.is_empty() {
            continue;
        }
        if mapping.strategy != ColorStrategy::A1Mono
            || mapping.actual_spool_id.as_deref() != Some(spool.id.as_str())
            || mapping.actual_material.as_ref() != Some(&spool.material)
            || mapping.direct_toolhead.is_some()
            || mapping.actual_color != Some(spool.actual_color())
        {
            return Err(A1MiniError::Plan(format!(
                "A1 mini mapping {} is not bound to the one physical spool {}",
                mapping.scope_id, spool.id
            )));
        }
        for requirement_id in relevant {
            let requirement = requirements[requirement_id.as_str()];
            let mapping_slots = mapping.source_slots.iter().collect::<BTreeSet<_>>();
            let mapping_profiles = mapping.source_profile_ids.iter().collect::<BTreeSet<_>>();
            if mapping.source_material != requirement.material
                || mapping.source_color != requirement.source_color
                || requirement
                    .source_slots
                    .iter()
                    .any(|slot| !mapping_slots.contains(slot))
                || requirement
                    .source_profile_ids
                    .iter()
                    .any(|profile| !mapping_profiles.contains(profile))
            {
                return Err(A1MiniError::Plan(format!(
                    "A1 mini mapping for requirement {} does not preserve its source material/color/profile identity",
                    requirement.id
                )));
            }
            covered.insert(requirement.id.as_str());
            if requirement.material != spool.material {
                substitutions.push(material_substitution_evidence(scope, requirement, spool)?);
            }
        }
        for source_slot in &mapping.source_slots {
            let slot = parse_source_slot(source_slot)?;
            slot_map.insert(slot, 1);
        }
    }
    if covered != requirement_ids || slot_map.is_empty() {
        return Err(A1MiniError::Plan(format!(
            "unit {} does not have complete one-spool A1 color mappings",
            unit.source_unit_id
        )));
    }
    Ok(slot_map)
}

fn material_substitution_evidence(
    scope: &u1_planner::PrintScope,
    requirement: &u1_planner::MaterialColorRequirement,
    spool: &Spool,
) -> Result<A1MiniMaterialSubstitutionEvidence, A1MiniError> {
    if let Some(approval) = scope
        .approved_material_substitutions
        .iter()
        .find(|approval| {
            approval.requirement_id == requirement.id
                && approval.source_material == requirement.material
                && approval.target_material == spool.material
                && approval.acknowledged
                && !approval.candidate_id.trim().is_empty()
        })
    {
        return Ok(A1MiniMaterialSubstitutionEvidence {
            scope_id: scope.id.clone(),
            requirement_id: requirement.id.clone(),
            approval_id: approval.candidate_id.clone(),
            source_material: requirement.material.clone(),
            target_material: spool.material.clone(),
        });
    }
    if scope.direct_assignments.iter().any(|assignment| {
        assignment.requirement_id == requirement.id
            && assignment.spool_id == spool.id
            && assignment.allow_material_substitution
    }) {
        let payload = format!(
            "{}\0{}\0{}\0{:?}\0{:?}",
            scope.id, requirement.id, spool.id, requirement.material, spool.material
        );
        return Ok(A1MiniMaterialSubstitutionEvidence {
            scope_id: scope.id.clone(),
            requirement_id: requirement.id.clone(),
            approval_id: format!("direct-assignment:{:x}", Sha256::digest(payload.as_bytes())),
            source_material: requirement.material.clone(),
            target_material: spool.material.clone(),
        });
    }
    Err(A1MiniError::Plan(format!(
        "requirement {} changes material from {:?} to {:?} without explicit approval",
        requirement.id, requirement.material, spool.material
    )))
}

fn ensure_object_slots_are_mapped(
    object: &u1_three_mf::ObjectAnalysis,
    slot_map: &BTreeMap<u8, u8>,
    unit: &PrintableUnit,
) -> Result<(), A1MiniError> {
    let mut expected = object
        .effective_slots
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    for part in &object.parts {
        expected.extend(part.effective_slots.iter().copied());
    }
    for slot in expected {
        let slot = u8::try_from(slot).map_err(|_| {
            A1MiniError::Plan(format!(
                "source unit {} uses unsupported slot F{slot}",
                unit.source_unit_id
            ))
        })?;
        if slot != 0 && !slot_map.contains_key(&slot) {
            return Err(A1MiniError::Plan(format!(
                "source unit {} uses F{slot}, but its canonical A1 mapping does not cover that slot",
                unit.source_unit_id
            )));
        }
    }
    Ok(())
}

fn parse_source_slot(value: &str) -> Result<u8, A1MiniError> {
    value
        .trim()
        .strip_prefix(['F', 'f'])
        .and_then(|value| value.parse::<u8>().ok())
        .filter(|value| (1..=32).contains(value))
        .ok_or_else(|| A1MiniError::Plan(format!("invalid source filament slot {value:?}")))
}

fn parse_source_plate_id(value: Option<&str>) -> Result<u32, A1MiniError> {
    value
        .and_then(|value| value.strip_prefix("plate-"))
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| A1MiniError::Plan("a printable unit has no stable source plate ID".into()))
}

fn validate_unit_bounds(
    unit: &PrintableUnit,
    source: AxisAlignedBounds,
) -> Result<(), A1MiniError> {
    let size = source.size().ok_or_else(|| {
        A1MiniError::Plan(format!(
            "source unit {} has invalid bounds",
            unit.source_unit_id
        ))
    })?;
    if !unit.bounds.is_valid()
        || !unit.bounds.has_known_size()
        || (size[0] - unit.bounds.width).abs() > BOUNDS_TOLERANCE_MM
        || (size[1] - unit.bounds.depth).abs() > BOUNDS_TOLERANCE_MM
        || (size[2] - unit.bounds.height).abs() > BOUNDS_TOLERANCE_MM
    {
        return Err(A1MiniError::Plan(format!(
            "source geometry bounds for unit {} no longer match the canonical packing bounds",
            unit.source_unit_id
        )));
    }
    Ok(())
}

fn translate_bounds(
    source: AxisAlignedBounds,
    delta_x: f64,
    delta_y: f64,
) -> Result<AxisAlignedBounds, A1MiniError> {
    if !delta_x.is_finite() || !delta_y.is_finite() {
        return Err(A1MiniError::Plan(
            "planned A1 mini translation is not finite".into(),
        ));
    }
    Ok(AxisAlignedBounds {
        min: [
            source.min[0] + delta_x,
            source.min[1] + delta_y,
            source.min[2],
        ],
        max: [
            source.max[0] + delta_x,
            source.max[1] + delta_y,
            source.max[2],
        ],
    })
}

fn translated_transform_value(
    mut transform: Transform3mf,
    delta_x: f64,
    delta_y: f64,
) -> Result<Transform3mf, A1MiniError> {
    transform.values[9] += delta_x;
    transform.values[10] += delta_y;
    if !transform.is_finite() {
        return Err(A1MiniError::Plan(
            "translated A1 mini instance transform is not finite".into(),
        ));
    }
    Ok(transform)
}

fn validate_target_bounds(
    unit: &PrintableUnit,
    bounds: AxisAlignedBounds,
) -> Result<(), A1MiniError> {
    if !bounds.is_valid()
        || bounds.min[0] - unit.bounds.clearance_x < -BOUNDS_TOLERANCE_MM
        || bounds.min[1] - unit.bounds.clearance_y < -BOUNDS_TOLERANCE_MM
        || bounds.max[0] + unit.bounds.clearance_x > A1MINI_BED_WIDTH_MM + BOUNDS_TOLERANCE_MM
        || bounds.max[1] + unit.bounds.clearance_y > A1MINI_BED_DEPTH_MM + BOUNDS_TOLERANCE_MM
        || bounds.max[2] + unit.bounds.clearance_z
            > A1MINI_PRINTABLE_HEIGHT_MM + BOUNDS_TOLERANCE_MM
        || bounds.max[2] <= 0.0
    {
        return Err(A1MiniError::Plan(format!(
            "packed unit {} does not fit the qualified A1 mini build volume with its clearances",
            unit.source_unit_id
        )));
    }
    Ok(())
}

fn validate_no_overlaps(units: &[ArtifactUnit]) -> Result<(), A1MiniError> {
    for (index, left) in units.iter().enumerate() {
        for right in &units[index + 1..] {
            let separated = left.target_bounds.max[0] + left.source.bounds.clearance_x
                <= right.target_bounds.min[0] - right.source.bounds.clearance_x
                    + BOUNDS_TOLERANCE_MM
                || right.target_bounds.max[0] + right.source.bounds.clearance_x
                    <= left.target_bounds.min[0] - left.source.bounds.clearance_x
                        + BOUNDS_TOLERANCE_MM
                || left.target_bounds.max[1] + left.source.bounds.clearance_y
                    <= right.target_bounds.min[1] - right.source.bounds.clearance_y
                        + BOUNDS_TOLERANCE_MM
                || right.target_bounds.max[1] + right.source.bounds.clearance_y
                    <= left.target_bounds.min[1] - left.source.bounds.clearance_y
                        + BOUNDS_TOLERANCE_MM;
            if !separated {
                return Err(A1MiniError::Plan(format!(
                    "packed A1 mini units {} and {} overlap after serialization clearances",
                    left.source.source_unit_id, right.source.source_unit_id
                )));
            }
        }
    }
    Ok(())
}

fn merge_slot_map(
    maps: &mut BTreeMap<u32, BTreeMap<u8, u8>>,
    id: u32,
    candidate: &BTreeMap<u8, u8>,
    label: &str,
) -> Result<(), A1MiniError> {
    if let Some(existing) = maps.get(&id) {
        if existing != candidate {
            return Err(A1MiniError::Plan(format!(
                "{label} {id} has conflicting source slot coverage in one A1 mini output"
            )));
        }
    } else {
        maps.insert(id, candidate.clone());
    }
    Ok(())
}

fn merge_resource_slot_map(
    maps: &mut BTreeMap<(String, u32), BTreeMap<u8, u8>>,
    key: (String, u32),
    candidate: &BTreeMap<u8, u8>,
) -> Result<(), A1MiniError> {
    if let Some(existing) = maps.get(&key) {
        if existing != candidate {
            return Err(A1MiniError::Plan(format!(
                "model resource {}/{} has conflicting source slot coverage",
                key.0, key.1
            )));
        }
    } else {
        maps.insert(key, candidate.clone());
    }
    Ok(())
}

fn normalized_model_path(value: &str) -> Result<String, A1MiniError> {
    let value = value.trim().trim_start_matches('/');
    if value.is_empty()
        || !value.starts_with("3D/")
        || !value.ends_with(".model")
        || value
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
        || value.contains(['\\', '\0'])
    {
        return Err(A1MiniError::Plan(format!(
            "unsafe Production model path {value:?}"
        )));
    }
    Ok(value.into())
}

fn safe_file_component(value: &str) -> String {
    let mut result = String::with_capacity(value.len().min(96));
    for character in value.chars().take(96) {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
            result.push(character);
        } else if character.is_whitespace() && !result.ends_with('_') {
            result.push('_');
        }
    }
    let result = result.trim_matches(['_', '-']);
    if result.is_empty() {
        "project".into()
    } else {
        result.into()
    }
}

fn rgb_hex(color: RgbColor) -> String {
    format!("#{:02X}{:02X}{:02X}", color.red, color.green, color.blue)
}

fn cancellation_checkpoint(is_cancelled: &mut dyn FnMut() -> bool) -> Result<(), A1MiniError> {
    if is_cancelled() {
        Err(A1MiniError::Cancelled)
    } else {
        Ok(())
    }
}

fn xml_cancellation_checkpoint(
    events_until_checkpoint: &mut usize,
    is_cancelled: &mut dyn FnMut() -> bool,
) -> Result<(), A1MiniError> {
    if *events_until_checkpoint == 0 {
        cancellation_checkpoint(is_cancelled)?;
        *events_until_checkpoint = CANCELLATION_XML_EVENT_INTERVAL;
    }
    *events_until_checkpoint -= 1;
    Ok(())
}

fn convert_with_context(
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    destination_directory: &Path,
    context: &AdapterContext,
    is_cancelled: &mut dyn FnMut() -> bool,
) -> Result<A1MiniConversionResult, A1MiniError> {
    cancellation_checkpoint(is_cancelled)?;
    if !destination_directory.is_dir() {
        return Err(A1MiniError::Plan(format!(
            "destination {} is not an existing directory",
            destination_directory.display()
        )));
    }
    cancellation_checkpoint(is_cancelled)?;
    let (preparation, plans) = prepare_with_context(source_path, analysis, input, result, context)?;
    cancellation_checkpoint(is_cancelled)?;
    convert_prepared_plans(
        source_path,
        analysis,
        preparation,
        &plans,
        destination_directory,
        is_cancelled,
    )
}

fn convert_prepared_plans(
    source_path: &Path,
    analysis: &ProjectAnalysis,
    preparation: A1MiniPreparation,
    plans: &[ArtifactBuildPlan],
    destination_directory: &Path,
    is_cancelled: &mut dyn FnMut() -> bool,
) -> Result<A1MiniConversionResult, A1MiniError> {
    let destinations = plans
        .iter()
        .map(|plan| destination_directory.join(&plan.prepared.file_name))
        .collect::<Vec<_>>();
    for destination in &destinations {
        cancellation_checkpoint(is_cancelled)?;
        if destination
            .try_exists()
            .map_err(|source| A1MiniError::Read {
                path: destination.clone(),
                source,
            })?
        {
            return Err(A1MiniError::Plan(format!(
                "output already exists: {}",
                destination.display()
            )));
        }
        if paths_refer_to_same_file(source_path, destination)? {
            return Err(A1MiniError::Plan(
                "the immutable source 3MF cannot be used as an output path".into(),
            ));
        }
    }

    // The writer never extracts the package. It creates one private, bounded
    // byte-for-byte archive snapshot, then opens only selected ZIP entries.
    cancellation_checkpoint(is_cancelled)?;
    let source_snapshot = snapshot_source(
        source_path,
        &analysis.input,
        destination_directory,
        is_cancelled,
    )?;
    cancellation_checkpoint(is_cancelled)?;
    let mut validated = Vec::<(ValidatedStagedPackage, &ArtifactBuildPlan)>::new();
    for (plan, destination) in plans.iter().zip(&destinations) {
        cancellation_checkpoint(is_cancelled)?;
        let (rewrite_archive, rewritten_entries, geometry) = rewrite_models_to_archive(
            source_snapshot.path(),
            plan,
            destination_directory,
            analysis.source.title.as_deref(),
            is_cancelled,
        )?;
        cancellation_checkpoint(is_cancelled)?;
        let rewrite_identity_raw = hash_file_cancellable(rewrite_archive.path(), is_cancelled)?;
        let rewrite_identity =
            ExpectedSourceIdentity::new(rewrite_identity_raw.0, rewrite_identity_raw.1)
                .map_err(|error| A1MiniError::Opc(error.to_string()))?;
        let model_settings = rewrite_model_settings(source_snapshot.path(), plan, is_cancelled)?;
        cancellation_checkpoint(is_cancelled)?;

        let mut content_types = ContentTypesBuilder::project_3mf();
        content_types
            .add_override(PROJECT_SETTINGS_PATH, "application/json")
            .map_err(|error| A1MiniError::Opc(error.to_string()))?;
        content_types
            .add_override(MODEL_SETTINGS_PATH, "application/xml")
            .map_err(|error| A1MiniError::Opc(error.to_string()))?;
        let root_relationship =
            OpcRelationship::internal("rel-1", MODEL_RELATIONSHIP_TYPE, MAIN_MODEL_PATH)
                .map_err(|error| A1MiniError::Opc(error.to_string()))?;
        let model_relationships = plan
            .external_paths
            .iter()
            .enumerate()
            .map(|(index, path)| {
                OpcRelationship::internal(
                    format!("rel-{}", index + 1),
                    MODEL_RELATIONSHIP_TYPE,
                    path,
                )
                .map_err(|error| A1MiniError::Opc(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut package = OpcPackageWriter::new();
        package
            .add_bytes(
                "[Content_Types].xml",
                content_types
                    .to_xml()
                    .map_err(|error| A1MiniError::Opc(error.to_string()))?,
            )
            .map_err(|error| A1MiniError::Opc(error.to_string()))?
            .add_bytes(
                "_rels/.rels",
                relationships_xml(&[root_relationship])
                    .map_err(|error| A1MiniError::Opc(error.to_string()))?,
            )
            .map_err(|error| A1MiniError::Opc(error.to_string()))?
            .add_bytes(PROJECT_SETTINGS_PATH, plan.project_settings.clone())
            .map_err(|error| A1MiniError::Opc(error.to_string()))?
            .add_bytes(MODEL_SETTINGS_PATH, model_settings)
            .map_err(|error| A1MiniError::Opc(error.to_string()))?;
        if !model_relationships.is_empty() {
            package
                .add_bytes(
                    MAIN_MODEL_RELATIONSHIPS_PATH,
                    relationships_xml(&model_relationships)
                        .map_err(|error| A1MiniError::Opc(error.to_string()))?,
                )
                .map_err(|error| A1MiniError::Opc(error.to_string()))?;
        }
        for entry in &rewritten_entries {
            cancellation_checkpoint(is_cancelled)?;
            package
                .copy_zip_entry_raw(rewrite_archive.path(), rewrite_identity.clone(), entry)
                .map_err(|error| A1MiniError::Opc(error.to_string()))?;
        }
        cancellation_checkpoint(is_cancelled)?;
        let staged = package
            .stage_to(destination)
            .map_err(|error| A1MiniError::Opc(error.to_string()))?;
        cancellation_checkpoint(is_cancelled)?;
        validate_a1mini_candidate(staged.path(), plan, &geometry)?;
        cancellation_checkpoint(is_cancelled)?;
        let candidate = staged.validate().map_err(|error| {
            let detail = error
                .report()
                .and_then(|report| serde_json::to_string(&report.issues).ok())
                .unwrap_or_else(|| error.to_string());
            A1MiniError::SemanticValidation(format!(
                "strict unsliced structural validation failed: {detail}"
            ))
        })?;
        cancellation_checkpoint(is_cancelled)?;
        validated.push((candidate, plan));
        cancellation_checkpoint(is_cancelled)?;
    }
    verify_source_identity_cancellable(source_path, &analysis.input, is_cancelled)?;
    cancellation_checkpoint(is_cancelled)?;

    let mut artifacts = Vec::with_capacity(validated.len());
    let mut rollback = PublishedDestinationRollback::default();
    for (candidate, plan) in validated {
        cancellation_checkpoint(is_cancelled)?;
        let report = candidate
            .publish()
            .map_err(|error| A1MiniError::Opc(error.to_string()))?;
        rollback.track(&report);
        cancellation_checkpoint(is_cancelled)?;
        artifacts.push(published_artifact(plan, &report));
    }
    cancellation_checkpoint(is_cancelled)?;
    rollback.commit();
    Ok(A1MiniConversionResult {
        adapter_id: A1MINI_ADAPTER_ID.into(),
        artifacts,
        warnings: preparation.warnings,
    })
}

fn published_artifact(
    plan: &ArtifactBuildPlan,
    report: &OpcWriteReport,
) -> A1MiniPublishedArtifact {
    A1MiniPublishedArtifact {
        plate_id: plan.prepared.plate_id.clone(),
        job_id: plan.prepared.job_id.clone(),
        file_name: plan.prepared.file_name.clone(),
        path: report.destination.clone(),
        byte_size: report.package_bytes,
        sha256: report.package_sha256.clone(),
        source_unit_ids: plan.prepared.source_unit_ids.clone(),
    }
}

fn snapshot_source(
    source_path: &Path,
    identity: &u1_three_mf::InputIdentity,
    destination_directory: &Path,
    is_cancelled: &mut dyn FnMut() -> bool,
) -> Result<NamedTempFile, A1MiniError> {
    cancellation_checkpoint(is_cancelled)?;
    let metadata = fs::metadata(source_path).map_err(|source| A1MiniError::Read {
        path: source_path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file()
        || metadata.len() != identity.byte_size
        || metadata.len() > MAX_SOURCE_ARCHIVE_BYTES
    {
        return Err(A1MiniError::SourceIdentityChanged);
    }
    let mut source = File::open(source_path).map_err(|source| A1MiniError::Read {
        path: source_path.to_path_buf(),
        source,
    })?;
    let mut snapshot =
        NamedTempFile::new_in(destination_directory).map_err(|source| A1MiniError::Write {
            path: destination_directory.to_path_buf(),
            source,
        })?;
    let mut hasher = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        cancellation_checkpoint(is_cancelled)?;
        let count = source
            .read(&mut buffer)
            .map_err(|source| A1MiniError::Read {
                path: source_path.to_path_buf(),
                source,
            })?;
        if count == 0 {
            break;
        }
        copied = copied
            .checked_add(count as u64)
            .filter(|value| *value <= MAX_SOURCE_ARCHIVE_BYTES)
            .ok_or(A1MiniError::SourceIdentityChanged)?;
        hasher.update(&buffer[..count]);
        snapshot
            .write_all(&buffer[..count])
            .map_err(|source| A1MiniError::Write {
                path: snapshot.path().to_path_buf(),
                source,
            })?;
    }
    snapshot
        .as_file_mut()
        .flush()
        .and_then(|()| snapshot.as_file().sync_all())
        .map_err(|source| A1MiniError::Write {
            path: snapshot.path().to_path_buf(),
            source,
        })?;
    if copied != identity.byte_size || format!("{:x}", hasher.finalize()) != identity.sha256 {
        return Err(A1MiniError::SourceIdentityChanged);
    }
    cancellation_checkpoint(is_cancelled)?;
    verify_source_identity_cancellable(source_path, identity, is_cancelled)?;
    cancellation_checkpoint(is_cancelled)?;
    Ok(snapshot)
}

fn verify_source_identity(
    path: &Path,
    identity: &u1_three_mf::InputIdentity,
) -> Result<(), A1MiniError> {
    let (size, sha256) = hash_file(path)?;
    if size != identity.byte_size || sha256 != identity.sha256 {
        return Err(A1MiniError::SourceIdentityChanged);
    }
    Ok(())
}

fn verify_source_identity_cancellable(
    path: &Path,
    identity: &u1_three_mf::InputIdentity,
    is_cancelled: &mut dyn FnMut() -> bool,
) -> Result<(), A1MiniError> {
    let (size, sha256) = hash_file_cancellable(path, is_cancelled)?;
    if size != identity.byte_size || sha256 != identity.sha256 {
        return Err(A1MiniError::SourceIdentityChanged);
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<(u64, String), A1MiniError> {
    let mut never_cancelled = || false;
    hash_file_cancellable(path, &mut never_cancelled)
}

fn hash_file_cancellable(
    path: &Path,
    is_cancelled: &mut dyn FnMut() -> bool,
) -> Result<(u64, String), A1MiniError> {
    cancellation_checkpoint(is_cancelled)?;
    let mut file = File::open(path).map_err(|source| A1MiniError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let metadata = file.metadata().map_err(|source| A1MiniError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() || metadata.len() > MAX_SOURCE_ARCHIVE_BYTES {
        return Err(A1MiniError::SourceIdentityChanged);
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut size = 0_u64;
    loop {
        cancellation_checkpoint(is_cancelled)?;
        let count = file.read(&mut buffer).map_err(|source| A1MiniError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        if count == 0 {
            break;
        }
        size += count as u64;
        hasher.update(&buffer[..count]);
    }
    cancellation_checkpoint(is_cancelled)?;
    Ok((size, format!("{:x}", hasher.finalize())))
}

fn paths_refer_to_same_file(source: &Path, destination: &Path) -> Result<bool, A1MiniError> {
    let source = fs::canonicalize(source).map_err(|source_error| A1MiniError::Read {
        path: source.to_path_buf(),
        source: source_error,
    })?;
    if destination.exists() {
        return fs::canonicalize(destination)
            .map(|candidate| candidate == source)
            .map_err(|source_error| A1MiniError::Read {
                path: destination.to_path_buf(),
                source: source_error,
            });
    }
    let parent = destination
        .parent()
        .ok_or_else(|| A1MiniError::Plan("output path has no parent directory".into()))?;
    let parent = fs::canonicalize(parent).map_err(|source_error| A1MiniError::Read {
        path: parent.to_path_buf(),
        source: source_error,
    })?;
    Ok(destination
        .file_name()
        .map(|name| parent.join(name) == source)
        .unwrap_or(false))
}

fn rewrite_models_to_archive(
    source_snapshot: &Path,
    plan: &ArtifactBuildPlan,
    staging_directory: &Path,
    title: Option<&str>,
    is_cancelled: &mut dyn FnMut() -> bool,
) -> Result<(NamedTempFile, Vec<String>, GeometryEvidence), A1MiniError> {
    cancellation_checkpoint(is_cancelled)?;
    let source_file = File::open(source_snapshot).map_err(|source| A1MiniError::Read {
        path: source_snapshot.to_path_buf(),
        source,
    })?;
    let mut source = ZipArchive::new(source_file)?;
    let temporary =
        NamedTempFile::new_in(staging_directory).map_err(|source| A1MiniError::Write {
            path: staging_directory.to_path_buf(),
            source,
        })?;
    let output_file = temporary.reopen().map_err(|source| A1MiniError::Write {
        path: temporary.path().to_path_buf(),
        source,
    })?;
    let mut output = ZipWriter::new(output_file);
    let timestamp =
        DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0).expect("the ZIP epoch is valid");
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(6))
        .last_modified_time(timestamp)
        .unix_permissions(0o644);
    let mut paths = BTreeSet::from([MAIN_MODEL_PATH.to_owned()]);
    paths.extend(plan.external_paths.iter().cloned());
    let mut geometry = GeometryEvidence::default();
    for path in &paths {
        cancellation_checkpoint(is_cancelled)?;
        let entry = source.by_name(path)?;
        if entry.size() > MAX_MODEL_BYTES {
            return Err(A1MiniError::Plan(format!(
                "selected model part {path} exceeds the {MAX_MODEL_BYTES}-byte bounded rewrite limit"
            )));
        }
        output.start_file(path, options)?;
        rewrite_model_xml(
            entry,
            &mut output,
            path,
            plan,
            title,
            &mut geometry,
            is_cancelled,
        )?;
    }
    cancellation_checkpoint(is_cancelled)?;
    let output_file = output.finish()?;
    output_file
        .sync_all()
        .map_err(|source| A1MiniError::Write {
            path: temporary.path().to_path_buf(),
            source,
        })?;
    cancellation_checkpoint(is_cancelled)?;
    Ok((temporary, paths.into_iter().collect(), geometry))
}

fn rewrite_model_xml<R: Read, W: Write>(
    input: R,
    output: &mut W,
    path: &str,
    plan: &ArtifactBuildPlan,
    title: Option<&str>,
    geometry: &mut GeometryEvidence,
    is_cancelled: &mut dyn FnMut() -> bool,
) -> Result<(), A1MiniError> {
    let resource_maps = if path == MAIN_MODEL_PATH {
        for id in &plan.selected_root_resource_ids {
            if let (Some(object_mapping), Some(resource_mapping)) = (
                plan.object_slot_maps.get(id),
                plan.resource_slot_maps.get(&(path.to_owned(), *id)),
            ) && object_mapping != resource_mapping
            {
                return Err(A1MiniError::Plan(format!(
                    "root resource {id} has conflicting single-spool mappings"
                )));
            }
        }
        plan.selected_root_resource_ids
            .iter()
            .filter_map(|id| {
                plan.resource_slot_maps
                    .get(&(path.to_owned(), *id))
                    .or_else(|| plan.object_slot_maps.get(id))
                    .map(|mapping| (*id, mapping))
            })
            .collect::<BTreeMap<_, _>>()
    } else {
        plan.resource_slot_maps
            .iter()
            .filter(|((resource_path, _), _)| resource_path == path)
            .map(|((_, id), mapping)| (*id, mapping))
            .collect::<BTreeMap<_, _>>()
    };
    if resource_maps.is_empty() {
        return Err(A1MiniError::Plan(format!(
            "no selected resources resolve inside model part {path}"
        )));
    }

    let mut reader = Reader::from_reader(BufReader::new(input));
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(output);
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    let mut skip_depth: Option<usize> = None;
    let mut current_resource: Option<u32> = None;
    let mut found_resources = BTreeSet::new();
    let mut build_occurrences = BTreeMap::<u32, u32>::new();
    let mut found_instances = BTreeSet::new();
    let mut in_build = false;
    let mut saw_root_build = false;
    let mut generated_metadata_written = false;
    let mut seen_external_paths = BTreeSet::new();
    let mut events_until_checkpoint = 0;
    loop {
        xml_cancellation_checkpoint(&mut events_until_checkpoint, is_cancelled)?;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| xml_error(path, error))?;
        match event {
            Event::Start(event) => {
                if skip_depth.is_some() {
                    depth += 1;
                    buffer.clear();
                    continue;
                }
                let event_name = event.name();
                let name = local_xml_name(event_name.as_ref());
                if depth == 1 && name == b"metadata" {
                    skip_depth = Some(depth);
                    depth += 1;
                    buffer.clear();
                    continue;
                }
                if depth == 1 && name == b"resources" && path == MAIN_MODEL_PATH {
                    write_generated_model_metadata(&mut writer, title)?;
                    generated_metadata_written = true;
                }
                if depth == 2 && name == b"object" {
                    let id = required_u32_attribute(&reader, &event, b"id", path)?;
                    if !resource_maps.contains_key(&id) {
                        skip_depth = Some(depth);
                        depth += 1;
                        buffer.clear();
                        continue;
                    }
                    current_resource = Some(id);
                    found_resources.insert(id);
                    begin_geometry_resource(path, id, geometry);
                } else if depth == 1 && name == b"build" {
                    if path != MAIN_MODEL_PATH {
                        return Err(A1MiniError::Plan(format!(
                            "external model part {path} contains a build section"
                        )));
                    }
                    in_build = true;
                    saw_root_build = true;
                }
                if path != MAIN_MODEL_PATH
                    && current_resource.is_some()
                    && (name == b"components" || name == b"component")
                {
                    return Err(A1MiniError::Plan(format!(
                        "selected resource in external model part {path} contains a nested component graph"
                    )));
                }
                if path == MAIN_MODEL_PATH
                    && current_resource.is_some()
                    && name == b"component"
                    && let Some(component_path) = component_external_path(&reader, &event, path)?
                {
                    seen_external_paths.insert(component_path);
                }
                if depth == 2 && in_build && name == b"item" {
                    let Some(rewritten) = rewrite_build_item(
                        &reader,
                        &event,
                        path,
                        plan,
                        &mut build_occurrences,
                        &mut found_instances,
                    )?
                    else {
                        skip_depth = Some(depth);
                        depth += 1;
                        buffer.clear();
                        continue;
                    };
                    writer
                        .write_event(Event::Start(rewritten))
                        .map_err(|error| xml_error(path, error))?;
                    depth += 1;
                    buffer.clear();
                    continue;
                }
                let rewritten = rewrite_model_element(
                    &reader,
                    &event,
                    path,
                    current_resource.and_then(|id| resource_maps.get(&id).copied()),
                )?;
                if current_resource.is_some() {
                    record_geometry_element(&reader, &event, path, geometry)?;
                }
                writer
                    .write_event(Event::Start(rewritten))
                    .map_err(|error| xml_error(path, error))?;
                depth += 1;
            }
            Event::Empty(event) => {
                if skip_depth.is_some() {
                    buffer.clear();
                    continue;
                }
                let event_name = event.name();
                let name = local_xml_name(event_name.as_ref());
                if depth == 1 && name == b"metadata" {
                    buffer.clear();
                    continue;
                }
                if depth == 2 && name == b"object" {
                    let id = required_u32_attribute(&reader, &event, b"id", path)?;
                    if !resource_maps.contains_key(&id) {
                        buffer.clear();
                        continue;
                    }
                    found_resources.insert(id);
                    begin_geometry_resource(path, id, geometry);
                }
                if path != MAIN_MODEL_PATH
                    && current_resource.is_some()
                    && (name == b"components" || name == b"component")
                {
                    return Err(A1MiniError::Plan(format!(
                        "selected resource in external model part {path} contains a nested component graph"
                    )));
                }
                if path == MAIN_MODEL_PATH
                    && current_resource.is_some()
                    && name == b"component"
                    && let Some(component_path) = component_external_path(&reader, &event, path)?
                {
                    seen_external_paths.insert(component_path);
                }
                if depth == 2 && in_build && name == b"item" {
                    if let Some(rewritten) = rewrite_build_item(
                        &reader,
                        &event,
                        path,
                        plan,
                        &mut build_occurrences,
                        &mut found_instances,
                    )? {
                        writer
                            .write_event(Event::Empty(rewritten))
                            .map_err(|error| xml_error(path, error))?;
                    }
                    buffer.clear();
                    continue;
                }
                let resource_map = if depth == 2 && name == b"object" {
                    let id = required_u32_attribute(&reader, &event, b"id", path)?;
                    resource_maps.get(&id).copied()
                } else {
                    current_resource.and_then(|id| resource_maps.get(&id).copied())
                };
                let rewritten = rewrite_model_element(&reader, &event, path, resource_map)?;
                if resource_map.is_some() {
                    record_geometry_element(&reader, &event, path, geometry)?;
                }
                writer
                    .write_event(Event::Empty(rewritten))
                    .map_err(|error| xml_error(path, error))?;
            }
            Event::End(event) => {
                if let Some(start_depth) = skip_depth {
                    depth = depth.saturating_sub(1);
                    if depth == start_depth {
                        skip_depth = None;
                    }
                    buffer.clear();
                    continue;
                }
                let event_name = event.name();
                let name = local_xml_name(event_name.as_ref());
                let closes_object = depth.saturating_sub(1) == 2 && name == b"object";
                let closes_build = depth.saturating_sub(1) == 1 && name == b"build";
                writer
                    .write_event(Event::End(event.into_owned()))
                    .map_err(|error| xml_error(path, error))?;
                depth = depth.saturating_sub(1);
                if closes_object {
                    current_resource = None;
                } else if closes_build {
                    in_build = false;
                }
            }
            Event::DocType(_) => {
                return Err(A1MiniError::Xml {
                    path: path.into(),
                    message: "DOCTYPE is forbidden".into(),
                });
            }
            Event::Eof => break,
            other => {
                if skip_depth.is_none() {
                    writer
                        .write_event(other.into_owned())
                        .map_err(|error| xml_error(path, error))?;
                }
            }
        }
        buffer.clear();
    }
    cancellation_checkpoint(is_cancelled)?;
    let expected_resources = resource_maps.keys().copied().collect::<BTreeSet<_>>();
    if found_resources != expected_resources {
        return Err(A1MiniError::Plan(format!(
            "model part {path} did not contain every selected resource (expected {expected_resources:?}, found {found_resources:?})"
        )));
    }
    if path == MAIN_MODEL_PATH {
        let expected_instances = plan
            .selected_instances
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        if found_instances != expected_instances {
            return Err(A1MiniError::Plan(format!(
                "root build membership differs from the canonical A1 mini plate (expected {expected_instances:?}, found {found_instances:?})"
            )));
        }
        if !generated_metadata_written || !saw_root_build {
            return Err(A1MiniError::Xml {
                path: path.into(),
                message: "root model must contain resources and build elements".into(),
            });
        }
        if seen_external_paths != plan.external_paths {
            return Err(A1MiniError::Plan(format!(
                "rewritten external component paths {seen_external_paths:?} do not match the analyzed closure {:?}",
                plan.external_paths
            )));
        }
    }
    Ok(())
}

fn component_external_path<R: io::BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    path: &str,
) -> Result<Option<String>, A1MiniError> {
    let value = decoded_attributes(reader, event, path)?
        .into_iter()
        .find(|(key, _)| local_xml_name(key.as_bytes()) == b"path")
        .map(|(_, value)| value);
    value.map(|value| normalized_model_path(&value)).transpose()
}

fn write_generated_model_metadata<W: Write>(
    writer: &mut Writer<W>,
    title: Option<&str>,
) -> Result<(), A1MiniError> {
    for (name, value) in [
        (
            "Application",
            format!("BambuStudio-{A1MINI_APPLICATION_VERSION}"),
        ),
        ("BambuStudio:3mfVersion", "1".into()),
        ("U1Planner:Generator", "U1 3MF Color Planner".into()),
        ("U1Planner:Adapter", A1MINI_ADAPTER_ID.into()),
    ] {
        write_metadata_text(writer, name, &value)?;
    }
    if let Some(title) = title.filter(|value| !value.trim().is_empty()) {
        write_metadata_text(writer, "Title", title)?;
    }
    Ok(())
}

fn write_metadata_text<W: Write>(
    writer: &mut Writer<W>,
    name: &str,
    value: &str,
) -> Result<(), A1MiniError> {
    let mut start = BytesStart::new("metadata");
    start.push_attribute(("name", name));
    writer
        .write_event(Event::Start(start))
        .map_err(|error| xml_error(MAIN_MODEL_PATH, error))?;
    writer
        .write_event(Event::Text(BytesText::new(value)))
        .map_err(|error| xml_error(MAIN_MODEL_PATH, error))?;
    writer
        .write_event(Event::End(BytesEnd::new("metadata")))
        .map_err(|error| xml_error(MAIN_MODEL_PATH, error))
}

fn rewrite_build_item<R: io::BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    path: &str,
    plan: &ArtifactBuildPlan,
    occurrences: &mut BTreeMap<u32, u32>,
    found: &mut BTreeSet<(u32, u32)>,
) -> Result<Option<BytesStart<'static>>, A1MiniError> {
    let attributes = decoded_attributes(reader, event, path)?;
    if attributes
        .iter()
        .any(|(key, _)| local_xml_name(key.as_bytes()) == b"path")
    {
        return Err(A1MiniError::Plan(
            "direct external build items are not qualified for the A1 mini writer".into(),
        ));
    }
    let object_id = attributes
        .iter()
        .find(|(key, _)| local_xml_name(key.as_bytes()) == b"objectid")
        .and_then(|(_, value)| value.parse::<u32>().ok())
        .ok_or_else(|| A1MiniError::Xml {
            path: path.into(),
            message: "build item has no valid objectid".into(),
        })?;
    let occurrence = occurrences.entry(object_id).or_default();
    let instance_id = *occurrence;
    *occurrence = occurrence.saturating_add(1);
    let Some(placement) = plan.selected_instances.get(&(object_id, instance_id)) else {
        return Ok(None);
    };
    found.insert((object_id, instance_id));
    let mut replacements = BTreeMap::new();
    let original_transform = attributes
        .iter()
        .find(|(key, _)| local_xml_name(key.as_bytes()) == b"transform")
        .map(|(_, value)| value.as_str());
    replacements.insert(
        "transform".into(),
        translated_transform(original_transform, *placement, path)?,
    );
    Ok(Some(rebuild_start(event, attributes, &replacements)?))
}

fn rewrite_model_element<R: io::BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    path: &str,
    slot_map: Option<&BTreeMap<u8, u8>>,
) -> Result<BytesStart<'static>, A1MiniError> {
    let attributes = decoded_attributes(reader, event, path)?;
    let mut replacements = BTreeMap::new();
    if let Some(slot_map) = slot_map {
        for (key, value) in &attributes {
            let local = local_xml_name(key.as_bytes());
            if local == b"paint_color" || local == b"mmu_segmentation" {
                let mut tree =
                    decode_paint_annotation(value).map_err(|error| A1MiniError::Xml {
                        path: path.into(),
                        message: format!("invalid paint annotation: {error}"),
                    })?;
                tree.remap_states(|state| {
                    slot_map.get(&state).copied().ok_or(
                        u1_three_mf::PaintCodecError::StateOutOfRange(u32::from(state)),
                    )
                })
                .map_err(|error| {
                    A1MiniError::Plan(format!(
                        "paint state has no approved one-spool mapping in {path}: {error}"
                    ))
                })?;
                replacements.insert(
                    key.clone(),
                    encode_paint_annotation(&tree).map_err(|error| A1MiniError::Xml {
                        path: path.into(),
                        message: format!("failed to encode remapped paint annotation: {error}"),
                    })?,
                );
            }
        }
    }
    rebuild_start(event, attributes, &replacements)
}

fn decoded_attributes<R: io::BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    path: &str,
) -> Result<Vec<(String, String)>, A1MiniError> {
    event
        .attributes()
        .with_checks(true)
        .map(|attribute| {
            let attribute = attribute.map_err(|error| A1MiniError::Xml {
                path: path.into(),
                message: error.to_string(),
            })?;
            let key = String::from_utf8(attribute.key.as_ref().to_vec()).map_err(|error| {
                A1MiniError::Xml {
                    path: path.into(),
                    message: error.to_string(),
                }
            })?;
            let value = attribute
                .decode_and_unescape_value(reader.decoder())
                .map_err(|error| A1MiniError::Xml {
                    path: path.into(),
                    message: error.to_string(),
                })?
                .into_owned();
            Ok((key, value))
        })
        .collect()
}

fn rebuild_start(
    event: &BytesStart<'_>,
    mut attributes: Vec<(String, String)>,
    replacements: &BTreeMap<String, String>,
) -> Result<BytesStart<'static>, A1MiniError> {
    for (key, value) in &mut attributes {
        if let Some(replacement) = replacements.get(key).or_else(|| {
            replacements.get(std::str::from_utf8(local_xml_name(key.as_bytes())).unwrap_or(""))
        }) {
            *value = replacement.clone();
        }
    }
    for (key, value) in replacements {
        if !attributes.iter().any(|(candidate, _)| {
            local_xml_name(candidate.as_bytes()) == local_xml_name(key.as_bytes())
        }) {
            attributes.push((key.clone(), value.clone()));
        }
    }
    let name =
        String::from_utf8(event.name().as_ref().to_vec()).map_err(|error| A1MiniError::Xml {
            path: MAIN_MODEL_PATH.into(),
            message: error.to_string(),
        })?;
    let mut rebuilt = BytesStart::new(name);
    for (key, value) in attributes {
        rebuilt.push_attribute((key.as_str(), value.as_str()));
    }
    Ok(rebuilt.into_owned())
}

fn required_u32_attribute<R: io::BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    key: &[u8],
    path: &str,
) -> Result<u32, A1MiniError> {
    decoded_attributes(reader, event, path)?
        .into_iter()
        .find(|(candidate, _)| local_xml_name(candidate.as_bytes()) == key)
        .and_then(|(_, value)| value.parse().ok())
        .ok_or_else(|| A1MiniError::Xml {
            path: path.into(),
            message: format!(
                "element {} has no valid {} attribute",
                String::from_utf8_lossy(event.name().as_ref()),
                String::from_utf8_lossy(key)
            ),
        })
}

fn translated_transform(
    original: Option<&str>,
    placement: InstancePlacement,
    path: &str,
) -> Result<String, A1MiniError> {
    let mut values = original
        .map(|value| {
            value
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            ["1", "0", "0", "0", "1", "0", "0", "0", "1", "0", "0", "0"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        });
    if values.len() != 12 {
        return Err(A1MiniError::Xml {
            path: path.into(),
            message: "build transform must contain exactly 12 numbers".into(),
        });
    }
    for (index, delta) in [placement.delta_x, placement.delta_y, placement.delta_z]
        .into_iter()
        .enumerate()
    {
        let value = values[9 + index]
            .parse::<f64>()
            .map_err(|_| A1MiniError::Xml {
                path: path.into(),
                message: "build transform contains an invalid translation".into(),
            })?;
        let translated = value + delta;
        if !translated.is_finite() {
            return Err(A1MiniError::Xml {
                path: path.into(),
                message: "translated build transform is not finite".into(),
            });
        }
        values[9 + index] = format_f64(translated);
    }
    Ok(values.join(" "))
}

fn format_f64(value: f64) -> String {
    if value == 0.0 {
        "0".into()
    } else {
        format!("{value:.15}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    }
}

fn begin_geometry_resource(path: &str, resource_id: u32, geometry: &mut GeometryEvidence) {
    update_geometry_field(&mut geometry.fingerprint, b"resource");
    update_geometry_field(&mut geometry.fingerprint, path.as_bytes());
    update_geometry_field(&mut geometry.fingerprint, &resource_id.to_be_bytes());
}

fn record_geometry_element<R: io::BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    path: &str,
    geometry: &mut GeometryEvidence,
) -> Result<(), A1MiniError> {
    let event_name = event.name();
    let name = local_xml_name(event_name.as_ref());
    let (kind, keys): (&[u8], &[&[u8]]) = if name == b"vertex" {
        geometry.vertices = geometry.vertices.saturating_add(1);
        (b"vertex", &[b"x", b"y", b"z"])
    } else if name == b"triangle" {
        geometry.triangles = geometry.triangles.saturating_add(1);
        (b"triangle", &[b"v1", b"v2", b"v3"])
    } else {
        return Ok(());
    };
    let attributes = decoded_attributes(reader, event, path)?;
    update_geometry_field(&mut geometry.fingerprint, kind);
    for key in keys {
        let matches = attributes
            .iter()
            .filter(|(candidate, _)| local_xml_name(candidate.as_bytes()) == *key)
            .map(|(_, value)| value)
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(A1MiniError::Xml {
                path: path.into(),
                message: format!(
                    "geometry element {} must have exactly one {} attribute",
                    String::from_utf8_lossy(name),
                    String::from_utf8_lossy(key)
                ),
            });
        }
        let value = matches[0];
        let valid = if kind == b"vertex" {
            value.parse::<f64>().is_ok_and(f64::is_finite)
        } else {
            value.parse::<u32>().is_ok()
        };
        if !valid {
            return Err(A1MiniError::Xml {
                path: path.into(),
                message: format!(
                    "geometry element {} has an invalid {} attribute",
                    String::from_utf8_lossy(name),
                    String::from_utf8_lossy(key)
                ),
            });
        }
        update_geometry_field(&mut geometry.fingerprint, value.as_bytes());
    }
    Ok(())
}

fn update_geometry_field(fingerprint: &mut Sha256, bytes: &[u8]) {
    fingerprint.update((bytes.len() as u64).to_be_bytes());
    fingerprint.update(bytes);
}

fn local_xml_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

fn xml_error(path: &str, error: impl std::fmt::Display) -> A1MiniError {
    A1MiniError::Xml {
        path: path.into(),
        message: error.to_string(),
    }
}

fn rewrite_model_settings(
    source_snapshot: &Path,
    plan: &ArtifactBuildPlan,
    is_cancelled: &mut dyn FnMut() -> bool,
) -> Result<Vec<u8>, A1MiniError> {
    cancellation_checkpoint(is_cancelled)?;
    let file = File::open(source_snapshot).map_err(|source| A1MiniError::Read {
        path: source_snapshot.to_path_buf(),
        source,
    })?;
    let mut archive = ZipArchive::new(file)?;
    let entry = archive.by_name(MODEL_SETTINGS_PATH)?;
    if entry.size() > MAX_METADATA_BYTES {
        return Err(A1MiniError::Plan(
            "source model settings exceed the bounded metadata limit".into(),
        ));
    }
    let mut reader = Reader::from_reader(BufReader::new(entry));
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new_with_indent(Vec::new(), b' ', 2);
    writer
        .write_event(Event::Decl(quick_xml::events::BytesDecl::new(
            "1.0",
            Some("UTF-8"),
            None,
        )))
        .map_err(|error| xml_error(MODEL_SETTINGS_PATH, error))?;
    writer
        .write_event(Event::Start(BytesStart::new("config")))
        .map_err(|error| xml_error(MODEL_SETTINGS_PATH, error))?;
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    let mut skip_depth: Option<usize> = None;
    let mut current_object: Option<u32> = None;
    let mut current_part_depth: Option<usize> = None;
    let mut found_objects = BTreeSet::new();
    let mut events_until_checkpoint = 0;
    loop {
        xml_cancellation_checkpoint(&mut events_until_checkpoint, is_cancelled)?;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| xml_error(MODEL_SETTINGS_PATH, error))?;
        match event {
            Event::Start(event) => {
                if skip_depth.is_some() {
                    depth += 1;
                    buffer.clear();
                    continue;
                }
                let event_name = event.name();
                let name = local_xml_name(event_name.as_ref());
                if depth == 0 && name == b"config" {
                    depth += 1;
                    buffer.clear();
                    continue;
                }
                if depth == 1 && (name == b"plate" || name == b"assemble") {
                    skip_depth = Some(depth);
                    depth += 1;
                    buffer.clear();
                    continue;
                }
                if depth == 1 && name == b"object" {
                    let id = required_u32_attribute(&reader, &event, b"id", MODEL_SETTINGS_PATH)?;
                    if !plan.object_slot_maps.contains_key(&id) {
                        skip_depth = Some(depth);
                        depth += 1;
                        buffer.clear();
                        continue;
                    }
                    current_object = Some(id);
                    found_objects.insert(id);
                } else if depth == 1 {
                    return Err(A1MiniError::Xml {
                        path: MODEL_SETTINGS_PATH.into(),
                        message: format!(
                            "unsupported root-level model settings element {}",
                            String::from_utf8_lossy(event.name().as_ref())
                        ),
                    });
                }
                if name == b"metadata" {
                    return Err(A1MiniError::Xml {
                        path: MODEL_SETTINGS_PATH.into(),
                        message: "retained object metadata must be an empty element".into(),
                    });
                }
                if depth == 2 && name == b"part" {
                    if current_part_depth.replace(depth).is_some() {
                        return Err(A1MiniError::Xml {
                            path: MODEL_SETTINGS_PATH.into(),
                            message: "nested retained part metadata is unsupported".into(),
                        });
                    }
                } else if current_object.is_some() && depth >= 2 {
                    return Err(A1MiniError::Xml {
                        path: MODEL_SETTINGS_PATH.into(),
                        message: format!(
                            "unsupported retained object child element {}",
                            String::from_utf8_lossy(event.name().as_ref())
                        ),
                    });
                }
                let rewritten = rewrite_settings_element(
                    &reader,
                    &event,
                    current_object.and_then(|id| plan.object_slot_maps.get(&id)),
                    None,
                )?;
                writer
                    .write_event(Event::Start(rewritten))
                    .map_err(|error| xml_error(MODEL_SETTINGS_PATH, error))?;
                depth += 1;
            }
            Event::Empty(event) => {
                if skip_depth.is_some() {
                    buffer.clear();
                    continue;
                }
                let event_name = event.name();
                let name = local_xml_name(event_name.as_ref());
                if depth == 1 && (name == b"plate" || name == b"assemble") {
                    buffer.clear();
                    continue;
                }
                if depth == 1 && name == b"object" {
                    let id = required_u32_attribute(&reader, &event, b"id", MODEL_SETTINGS_PATH)?;
                    let Some(mapping) = plan.object_slot_maps.get(&id) else {
                        buffer.clear();
                        continue;
                    };
                    found_objects.insert(id);
                    let rewritten = rewrite_settings_element(&reader, &event, Some(mapping), None)?;
                    writer
                        .write_event(Event::Empty(rewritten))
                        .map_err(|error| xml_error(MODEL_SETTINGS_PATH, error))?;
                    buffer.clear();
                    continue;
                }
                if depth == 1 {
                    return Err(A1MiniError::Xml {
                        path: MODEL_SETTINGS_PATH.into(),
                        message: format!(
                            "unsupported root-level model settings element {}",
                            String::from_utf8_lossy(event.name().as_ref())
                        ),
                    });
                }
                if depth >= 1 && current_object.is_some() {
                    let omittable_text_info = name == b"text_info"
                        && current_part_depth.is_some_and(|part_depth| depth == part_depth + 1);
                    if omittable_text_info {
                        let attributes = decoded_attributes(&reader, &event, MODEL_SETTINGS_PATH)?;
                        validate_omitted_text_info(&attributes)?;
                        // The serialized mesh already contains the negative or
                        // positive text volume. `text_info` is editable-text UI
                        // state, not geometry, and target structural validation
                        // does not permit it. Validate it strictly, then omit it.
                        buffer.clear();
                        continue;
                    }
                    let allowed_structural_element = (depth == 2
                        && current_part_depth.is_none()
                        && name == b"part")
                        || (depth == 3 && current_part_depth == Some(2) && name == b"mesh_stat");
                    if name != b"metadata" && !allowed_structural_element {
                        return Err(A1MiniError::Xml {
                            path: MODEL_SETTINGS_PATH.into(),
                            message: format!(
                                "unsupported retained object child element {}",
                                String::from_utf8_lossy(event.name().as_ref())
                            ),
                        });
                    }
                    let metadata_scope = if name == b"metadata" {
                        match (depth, current_part_depth) {
                            (2, None) => Some(RetainedMetadataScope::Object),
                            (metadata_depth, Some(part_depth))
                                if metadata_depth == part_depth + 1 =>
                            {
                                Some(RetainedMetadataScope::Part)
                            }
                            _ => {
                                return Err(A1MiniError::Xml {
                                    path: MODEL_SETTINGS_PATH.into(),
                                    message:
                                        "retained metadata must be a direct child of object or part"
                                            .into(),
                                });
                            }
                        }
                    } else {
                        None
                    };
                    if let Some(metadata_scope) = metadata_scope {
                        let attributes = decoded_attributes(&reader, &event, MODEL_SETTINGS_PATH)?;
                        let has_key = attributes
                            .iter()
                            .any(|(key, _)| local_xml_name(key.as_bytes()) == b"key");
                        if !has_key {
                            // Bambu sources may cache an unkeyed face_count in
                            // object metadata. It is derived geometry data and
                            // strict unsliced output does not permit unkeyed
                            // metadata, so validate then deliberately omit it.
                            validate_unkeyed_object_metadata(metadata_scope, &attributes)?;
                            buffer.clear();
                            continue;
                        }
                    }
                    let rewritten = rewrite_settings_element(
                        &reader,
                        &event,
                        current_object.and_then(|id| plan.object_slot_maps.get(&id)),
                        metadata_scope,
                    )?;
                    writer
                        .write_event(Event::Empty(rewritten))
                        .map_err(|error| xml_error(MODEL_SETTINGS_PATH, error))?;
                }
            }
            Event::End(event) => {
                if let Some(start_depth) = skip_depth {
                    depth = depth.saturating_sub(1);
                    if depth == start_depth {
                        skip_depth = None;
                    }
                    buffer.clear();
                    continue;
                }
                let event_name = event.name();
                let name = local_xml_name(event_name.as_ref());
                if depth == 1 && name == b"config" {
                    depth = 0;
                    buffer.clear();
                    continue;
                }
                let closes_object = depth.saturating_sub(1) == 1 && name == b"object";
                let closes_part = current_part_depth.is_some_and(|part_depth| {
                    depth.saturating_sub(1) == part_depth && name == b"part"
                });
                if current_object.is_some() {
                    writer
                        .write_event(Event::End(event.into_owned()))
                        .map_err(|error| xml_error(MODEL_SETTINGS_PATH, error))?;
                }
                depth = depth.saturating_sub(1);
                if closes_part {
                    current_part_depth = None;
                }
                if closes_object {
                    current_object = None;
                }
            }
            Event::DocType(_) => {
                return Err(A1MiniError::Xml {
                    path: MODEL_SETTINGS_PATH.into(),
                    message: "DOCTYPE is forbidden".into(),
                });
            }
            Event::Eof => break,
            other => {
                if current_object.is_some() && skip_depth.is_none() {
                    writer
                        .write_event(other.into_owned())
                        .map_err(|error| xml_error(MODEL_SETTINGS_PATH, error))?;
                }
            }
        }
        buffer.clear();
    }
    cancellation_checkpoint(is_cancelled)?;
    let expected_objects = plan
        .object_slot_maps
        .keys()
        .copied()
        .collect::<BTreeSet<_>>();
    if found_objects != expected_objects {
        return Err(A1MiniError::Plan(format!(
            "model settings did not contain every selected root object (expected {expected_objects:?}, found {found_objects:?})"
        )));
    }
    writer
        .write_event(Event::Start(BytesStart::new("plate")))
        .map_err(|error| xml_error(MODEL_SETTINGS_PATH, error))?;
    write_settings_metadata(&mut writer, "plater_id", "1")?;
    write_settings_metadata(&mut writer, "plater_name", &plan.prepared.plate_id)?;
    write_settings_metadata(&mut writer, "locked", "false")?;
    write_settings_metadata(&mut writer, "filament_map_mode", TARGET_FILAMENT_MAP_MODE)?;
    write_settings_metadata(&mut writer, "filament_maps", "1")?;
    write_settings_metadata(&mut writer, "filament_volume_maps", "0")?;
    for unit in &plan.units {
        cancellation_checkpoint(is_cancelled)?;
        writer
            .write_event(Event::Start(BytesStart::new("model_instance")))
            .map_err(|error| xml_error(MODEL_SETTINGS_PATH, error))?;
        write_settings_metadata(
            &mut writer,
            "object_id",
            &unit.source.source_object_id.to_string(),
        )?;
        write_settings_metadata(
            &mut writer,
            "instance_id",
            &unit.target_instance_id.to_string(),
        )?;
        write_settings_metadata(&mut writer, "identify_id", &unit.identify_id.to_string())?;
        writer
            .write_event(Event::End(BytesEnd::new("model_instance")))
            .map_err(|error| xml_error(MODEL_SETTINGS_PATH, error))?;
    }
    writer
        .write_event(Event::End(BytesEnd::new("plate")))
        .map_err(|error| xml_error(MODEL_SETTINGS_PATH, error))?;
    writer
        .write_event(Event::End(BytesEnd::new("config")))
        .map_err(|error| xml_error(MODEL_SETTINGS_PATH, error))?;
    cancellation_checkpoint(is_cancelled)?;
    Ok(writer.into_inner())
}

fn rewrite_settings_element<R: io::BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    slot_map: Option<&BTreeMap<u8, u8>>,
    metadata_scope: Option<RetainedMetadataScope>,
) -> Result<BytesStart<'static>, A1MiniError> {
    let attributes = decoded_attributes(reader, event, MODEL_SETTINGS_PATH)?;
    let mut replacements = BTreeMap::new();
    if local_xml_name(event.name().as_ref()) == b"metadata" {
        let metadata_scope = metadata_scope.ok_or_else(|| A1MiniError::Xml {
            path: MODEL_SETTINGS_PATH.into(),
            message: "retained metadata has no qualified object/part scope".into(),
        })?;
        let key = attributes
            .iter()
            .find(|(key, _)| local_xml_name(key.as_bytes()) == b"key")
            .map(|(_, value)| value.as_str());
        if key.is_some()
            && (attributes.len() != 2
                || !attributes.iter().any(|(name, _)| name == "key")
                || !attributes.iter().any(|(name, _)| name == "value"))
        {
            return Err(A1MiniError::Xml {
                path: MODEL_SETTINGS_PATH.into(),
                message: "keyed retained metadata must contain exactly key and value attributes"
                    .into(),
            });
        }
        if matches!(
            key,
            Some("extruder" | "support_filament" | "support_interface_filament")
        ) {
            let value = attributes
                .iter()
                .find(|(key, _)| local_xml_name(key.as_bytes()) == b"value")
                .and_then(|(_, value)| value.parse::<u8>().ok())
                .ok_or_else(|| A1MiniError::Xml {
                    path: MODEL_SETTINGS_PATH.into(),
                    message: "color metadata has no valid slot value".into(),
                })?;
            if value != 0 {
                let mapped = slot_map
                    .and_then(|map| map.get(&value))
                    .copied()
                    .ok_or_else(|| {
                        A1MiniError::Plan(format!(
                            "model settings slot F{value} has no approved one-spool mapping"
                        ))
                    })?;
                replacements.insert("value".into(), mapped.to_string());
            }
        } else if let Some(key) = key {
            let value = attributes
                .iter()
                .find(|(name, _)| local_xml_name(name.as_bytes()) == b"value")
                .map(|(_, value)| value.as_str())
                .ok_or_else(|| A1MiniError::Xml {
                    path: MODEL_SETTINGS_PATH.into(),
                    message: format!("retained metadata {key:?} has no value"),
                })?;
            validate_retained_object_metadata(metadata_scope, key, value)?;
        } else {
            validate_unkeyed_object_metadata(metadata_scope, &attributes)?;
        }
    }
    rebuild_start(event, attributes, &replacements)
}

fn validate_retained_object_metadata(
    scope: RetainedMetadataScope,
    key: &str,
    value: &str,
) -> Result<(), A1MiniError> {
    if key == "name" {
        return Ok(());
    }
    if scope == RetainedMetadataScope::Part && SAFE_PART_IDENTITY_METADATA_KEYS.contains(&key) {
        let valid = match key {
            "matrix" => {
                let values = value
                    .split_whitespace()
                    .map(str::parse::<f64>)
                    .collect::<Result<Vec<_>, _>>();
                values.is_ok_and(|values| {
                    values.len() == 16
                        && values.iter().all(|value| value.is_finite())
                        && values[12].abs() <= f64::EPSILON
                        && values[13].abs() <= f64::EPSILON
                        && values[14].abs() <= f64::EPSILON
                        && (values[15] - 1.0).abs() <= f64::EPSILON
                })
            }
            "source_object_id" | "source_volume_id" => value.parse::<u32>().is_ok(),
            "source_offset_x" | "source_offset_y" | "source_offset_z" => {
                value.parse::<f64>().is_ok_and(f64::is_finite)
            }
            "source_file" => {
                !value.is_empty()
                    && value.len() <= 255
                    && !value.contains(['/', '\\'])
                    && value != "."
                    && value != ".."
            }
            _ => false,
        };
        if valid {
            return Ok(());
        }
    }
    if scope == RetainedMetadataScope::Object {
        let valid_process_value = match key {
            "ironing_type" => matches!(value, "no ironing" | "top" | "topmost" | "solid"),
            "skeleton_infill_density" | "skin_infill_density" | "sparse_infill_density" => value
                .strip_suffix('%')
                .and_then(|value| value.parse::<f64>().ok())
                .is_some_and(|value| value.is_finite() && (0.0..=100.0).contains(&value)),
            "sparse_infill_anchor_max" => value
                .strip_suffix('%')
                .unwrap_or(value)
                .parse::<f64>()
                .is_ok_and(|value| value.is_finite() && (0.0..=1000.0).contains(&value)),
            "wall_loops" => value.parse::<u32>().is_ok_and(|value| value <= 1000),
            _ => false,
        };
        if SAFE_OBJECT_PROCESS_METADATA_KEYS.contains(&key) && valid_process_value {
            return Ok(());
        }
        if key == "embedding_wall_into_infill" && value == "1" {
            return Ok(());
        }
        if key == "brim_type" && QUALIFIED_BRIM_TYPES.contains(&value) {
            return Ok(());
        }
        if key == "enable_support" && (value == "0" || value.eq_ignore_ascii_case("false")) {
            return Ok(());
        }
        if key == "brim_width"
            && value
                .parse::<f64>()
                .is_ok_and(|value| value.is_finite() && (0.0..=5.0).contains(&value))
        {
            return Ok(());
        }
        if key == "brim_object_gap"
            && value
                .parse::<f64>()
                .is_ok_and(|value| value.is_finite() && (0.0..=1.0).contains(&value))
        {
            return Ok(());
        }
        if matches!(
            key,
            "raft_layers"
                | "raft_first_layer_expansion"
                | "xy_contour_compensation"
                | "xy_hole_compensation"
        ) && value
            .parse::<f64>()
            .is_ok_and(|value| value.is_finite() && value.abs() <= f64::EPSILON)
        {
            return Ok(());
        }
    }
    Err(A1MiniError::Plan(format!(
        "retained {scope:?} metadata {key:?}={value:?} is outside the qualified A1 mini allowlist"
    )))
}

fn validate_unkeyed_object_metadata(
    scope: RetainedMetadataScope,
    attributes: &[(String, String)],
) -> Result<(), A1MiniError> {
    if scope == RetainedMetadataScope::Object
        && attributes.len() == 1
        && attributes[0].0 == "face_count"
        && attributes[0].1.parse::<u64>().is_ok()
    {
        return Ok(());
    }
    Err(A1MiniError::Plan(
        "unkeyed retained metadata must contain only a numeric object face_count".into(),
    ))
}

fn validate_omitted_text_info(attributes: &[(String, String)]) -> Result<(), A1MiniError> {
    if attributes.is_empty() || attributes.len() > MAX_TEXT_INFO_ATTRIBUTES {
        return Err(A1MiniError::Plan(format!(
            "retained text_info must contain between 1 and {MAX_TEXT_INFO_ATTRIBUTES} attributes"
        )));
    }
    let mut seen = BTreeSet::new();
    for (key, value) in attributes {
        if local_xml_name(key.as_bytes()) != key.as_bytes() || !seen.insert(key.as_str()) {
            return Err(A1MiniError::Plan(
                "retained text_info contains a namespaced or duplicate attribute".into(),
            ));
        }
        let valid = match key.as_str() {
            "text" => value.len() <= MAX_TEXT_INFO_TEXT_BYTES,
            "font_name" | "font_version" | "style_name" => value.len() <= MAX_TEXT_INFO_LABEL_BYTES,
            "bold" | "italic" => value.parse::<u8>().is_ok_and(|value| value <= 1),
            "font_index" | "surface_type" | "hit_mesh" => value.parse::<i32>().is_ok(),
            "boldness" | "skew" | "font_size" | "thickness" | "embeded_depth" | "rotate_angle"
            | "text_gap" => value
                .parse::<f64>()
                .is_ok_and(|value| value.is_finite() && value.abs() <= 1_000_000.0),
            "hit_position" | "hit_normal" => {
                let values = value
                    .split_whitespace()
                    .map(str::parse::<f64>)
                    .collect::<Result<Vec<_>, _>>();
                values.is_ok_and(|values| {
                    values.len() == 3 && values.iter().all(|value| value.is_finite())
                })
            }
            _ => false,
        };
        if !valid {
            return Err(A1MiniError::Plan(format!(
                "retained text_info attribute {key:?} is outside the bounded allowlist"
            )));
        }
    }
    if !seen.contains("text") {
        return Err(A1MiniError::Plan(
            "retained text_info does not contain a text attribute".into(),
        ));
    }
    Ok(())
}

fn write_settings_metadata<W: Write>(
    writer: &mut Writer<W>,
    key: &str,
    value: &str,
) -> Result<(), A1MiniError> {
    let mut metadata = BytesStart::new("metadata");
    metadata.push_attribute(("key", key));
    metadata.push_attribute(("value", value));
    writer
        .write_event(Event::Empty(metadata))
        .map_err(|error| xml_error(MODEL_SETTINGS_PATH, error))
}

#[derive(Debug)]
struct ParsedModelInstance {
    object_id: u32,
    instance_id: u32,
    identify_id: u64,
}

fn validate_a1mini_candidate(
    path: &Path,
    plan: &ArtifactBuildPlan,
    expected_geometry: &GeometryEvidence,
) -> Result<(), A1MiniError> {
    let file = File::open(path).map_err(|source| A1MiniError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let mut archive = ZipArchive::new(file)?;
    let mut actual_entries = archive
        .file_names()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    if actual_entries.len() != archive.len() {
        return Err(A1MiniError::SemanticValidation(
            "generated package contains duplicate ZIP entry names".into(),
        ));
    }
    let mut expected_entries = BTreeSet::from([
        "[Content_Types].xml".into(),
        "_rels/.rels".into(),
        MAIN_MODEL_PATH.into(),
        PROJECT_SETTINGS_PATH.into(),
        MODEL_SETTINGS_PATH.into(),
    ]);
    expected_entries.extend(plan.external_paths.iter().cloned());
    if !plan.external_paths.is_empty() {
        expected_entries.insert(MAIN_MODEL_RELATIONSHIPS_PATH.into());
    }
    if actual_entries != expected_entries {
        return Err(A1MiniError::SemanticValidation(format!(
            "generated package allowlist mismatch (expected {expected_entries:?}, found {actual_entries:?})"
        )));
    }
    actual_entries.clear();

    let mut project_entry = archive.by_name(PROJECT_SETTINGS_PATH)?;
    if project_entry.size() > MAX_METADATA_BYTES {
        return Err(A1MiniError::SemanticValidation(
            "generated project settings exceed the bounded metadata limit".into(),
        ));
    }
    let mut project_settings = Vec::with_capacity(project_entry.size() as usize);
    project_entry
        .read_to_end(&mut project_settings)
        .map_err(|source| A1MiniError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    drop(project_entry);
    crate::validate_a1mini_project_settings(&project_settings, plan.spool.material)?;

    let mut settings_entry = archive.by_name(MODEL_SETTINGS_PATH)?;
    if settings_entry.size() > MAX_METADATA_BYTES {
        return Err(A1MiniError::SemanticValidation(
            "generated model settings exceed the bounded metadata limit".into(),
        ));
    }
    let (plate_metadata, instances) =
        parse_generated_model_settings(BufReader::new(&mut settings_entry))?;
    drop(settings_entry);
    for (key, expected) in [
        ("plater_id", "1"),
        ("locked", "false"),
        ("filament_map_mode", TARGET_FILAMENT_MAP_MODE),
        ("filament_maps", "1"),
        ("filament_volume_maps", "0"),
    ] {
        if plate_metadata.get(key).map(String::as_str) != Some(expected) {
            return Err(A1MiniError::SemanticValidation(format!(
                "generated plate metadata {key:?} does not equal {expected:?}"
            )));
        }
    }
    let expected_instances = plan
        .units
        .iter()
        .map(|unit| {
            (
                unit.identify_id,
                (unit.source.source_object_id, unit.target_instance_id),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let actual_instances = instances
        .iter()
        .map(|instance| {
            (
                instance.identify_id,
                (instance.object_id, instance.instance_id),
            )
        })
        .collect::<BTreeMap<_, _>>();
    if expected_instances.len() != plan.units.len()
        || actual_instances.len() != instances.len()
        || actual_instances != expected_instances
    {
        return Err(A1MiniError::SemanticValidation(
            "generated model_settings plate membership or identify_id bijection changed".into(),
        ));
    }

    let actual_geometry = scan_candidate_geometry(&mut archive, &plan.external_paths)?;
    if actual_geometry.vertices != expected_geometry.vertices
        || actual_geometry.triangles != expected_geometry.triangles
        || actual_geometry.sha256() != expected_geometry.sha256()
    {
        return Err(A1MiniError::SemanticValidation(format!(
            "geometry changed during package construction (expected {} vertices, {} triangles, {}; found {} vertices, {} triangles, {})",
            expected_geometry.vertices,
            expected_geometry.triangles,
            expected_geometry.sha256(),
            actual_geometry.vertices,
            actual_geometry.triangles,
            actual_geometry.sha256()
        )));
    }
    drop(archive);

    let analysis = analyze_project(path).map_err(|error| {
        A1MiniError::SemanticValidation(format!("generated project cannot be re-analyzed: {error}"))
    })?;
    if analysis.source.application != u1_three_mf::SourceApplication::BambuStudio
        || analysis.source.application_version.as_deref() != Some(A1MINI_APPLICATION_VERSION)
        || analysis.printer.model.as_deref() != Some("Bambu Lab A1 mini")
        || analysis.printer.variant.as_deref() != Some("0.4")
        || analysis.printer.nozzle_diameters_mm != [0.4]
        || analysis.plates.len() != 1
        || analysis.summary.vertex_count != expected_geometry.vertices
        || analysis.summary.triangle_count != expected_geometry.triangles
    {
        return Err(A1MiniError::SemanticValidation(
            "generated project does not re-analyze as the exact A1 mini 0.4 single-plate target"
                .into(),
        ));
    }
    let plate = &analysis.plates[0];
    if plate.id != 1 || plate.instances.len() != plan.units.len() || plate.effective_slots != [1] {
        return Err(A1MiniError::SemanticValidation(
            "generated plate does not contain exactly the planned instances on logical slot 1"
                .into(),
        ));
    }
    let analyzed_by_identify = plate
        .instances
        .iter()
        .filter_map(|instance| instance.identify_id.map(|id| (id, instance)))
        .collect::<BTreeMap<_, _>>();
    if analyzed_by_identify.len() != plan.units.len() {
        return Err(A1MiniError::SemanticValidation(
            "generated plate has missing or duplicate identify_id values".into(),
        ));
    }
    for unit in &plan.units {
        let instance = analyzed_by_identify
            .get(&unit.identify_id)
            .copied()
            .ok_or_else(|| {
                A1MiniError::SemanticValidation(format!(
                    "generated plate lost identify_id {}",
                    unit.identify_id
                ))
            })?;
        if instance.object_id != unit.source.source_object_id
            || instance.instance_id != unit.target_instance_id
            || !transforms_close(
                instance.transform.unwrap_or(Transform3mf::IDENTITY),
                unit.target_transform,
            )
            || !bounds_close(instance.printable_bounds, Some(unit.target_bounds))
        {
            return Err(A1MiniError::SemanticValidation(format!(
                "generated instance for source unit {} changed identity, scale, orientation, translation, or printable bounds",
                unit.source.source_unit_id
            )));
        }
    }
    for object in &analysis.objects {
        if object.effective_slots.iter().any(|slot| *slot != 1)
            || object
                .parts
                .iter()
                .flat_map(|part| part.effective_slots.iter())
                .any(|slot| *slot != 1)
        {
            return Err(A1MiniError::SemanticValidation(format!(
                "generated object {} retains a logical filament assignment other than slot 1",
                object.id
            )));
        }
    }
    let target_material = plan.spool.material.material_name();
    if analysis
        .effective_material_colors
        .iter()
        .any(|value| value.material.as_deref() != Some(target_material))
    {
        return Err(A1MiniError::SemanticValidation(
            "generated effective material identity differs from the one physical spool".into(),
        ));
    }
    Ok(())
}

fn parse_generated_model_settings<R: io::BufRead>(
    input: R,
) -> Result<(BTreeMap<String, String>, Vec<ParsedModelInstance>), A1MiniError> {
    let mut reader = Reader::from_reader(input);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    let mut plate_count = 0_usize;
    let mut in_plate = false;
    let mut in_instance = false;
    let mut plate_metadata = BTreeMap::new();
    let mut current_instance = BTreeMap::new();
    let mut instances = Vec::new();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| xml_error(MODEL_SETTINGS_PATH, error))?;
        match event {
            Event::Start(event) => {
                let event_name = event.name();
                let name = local_xml_name(event_name.as_ref());
                if depth == 1 && name == b"plate" {
                    plate_count += 1;
                    in_plate = true;
                } else if depth == 2 && in_plate && name == b"model_instance" {
                    if in_instance {
                        return Err(A1MiniError::SemanticValidation(
                            "generated model settings contain nested model instances".into(),
                        ));
                    }
                    in_instance = true;
                    current_instance.clear();
                }
                depth += 1;
            }
            Event::Empty(event)
                if in_plate && local_xml_name(event.name().as_ref()) == b"metadata" =>
            {
                let attributes = decoded_attributes(&reader, &event, MODEL_SETTINGS_PATH)?;
                let key = attributes
                    .iter()
                    .find(|(key, _)| local_xml_name(key.as_bytes()) == b"key")
                    .map(|(_, value)| value.clone())
                    .ok_or_else(|| {
                        A1MiniError::SemanticValidation("generated metadata has no key".into())
                    })?;
                let value = attributes
                    .iter()
                    .find(|(key, _)| local_xml_name(key.as_bytes()) == b"value")
                    .map(|(_, value)| value.clone())
                    .ok_or_else(|| {
                        A1MiniError::SemanticValidation("generated metadata has no value".into())
                    })?;
                let target = if in_instance {
                    &mut current_instance
                } else {
                    &mut plate_metadata
                };
                if target.insert(key.clone(), value).is_some() {
                    return Err(A1MiniError::SemanticValidation(format!(
                        "generated metadata key {key:?} is duplicated"
                    )));
                }
            }
            Event::End(event) => {
                let event_name = event.name();
                let name = local_xml_name(event_name.as_ref());
                let element_depth = depth.saturating_sub(1);
                if element_depth == 2 && name == b"model_instance" && in_instance {
                    instances.push(ParsedModelInstance {
                        object_id: parse_generated_integer(&current_instance, "object_id")?,
                        instance_id: parse_generated_integer(&current_instance, "instance_id")?,
                        identify_id: parse_generated_integer(&current_instance, "identify_id")?,
                    });
                    in_instance = false;
                    current_instance.clear();
                } else if element_depth == 1 && name == b"plate" {
                    in_plate = false;
                }
                depth = element_depth;
            }
            Event::DocType(_) => {
                return Err(A1MiniError::SemanticValidation(
                    "DOCTYPE is forbidden in generated model settings".into(),
                ));
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if plate_count != 1 || in_plate || in_instance {
        return Err(A1MiniError::SemanticValidation(format!(
            "generated model settings contain {plate_count} plates; exactly one is required"
        )));
    }
    Ok((plate_metadata, instances))
}

fn parse_generated_integer<T: std::str::FromStr>(
    values: &BTreeMap<String, String>,
    key: &str,
) -> Result<T, A1MiniError> {
    values
        .get(key)
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| {
            A1MiniError::SemanticValidation(format!("generated model instance has no valid {key}"))
        })
}

fn scan_candidate_geometry<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    external_paths: &BTreeSet<String>,
) -> Result<GeometryEvidence, A1MiniError> {
    let mut paths = BTreeSet::from([MAIN_MODEL_PATH.to_owned()]);
    paths.extend(external_paths.iter().cloned());
    let mut geometry = GeometryEvidence::default();
    for path in paths {
        let entry = archive.by_name(&path)?;
        if entry.size() > MAX_MODEL_BYTES {
            return Err(A1MiniError::SemanticValidation(format!(
                "generated model part {path} exceeds the bounded validation limit"
            )));
        }
        scan_candidate_model_xml(entry, &path, &mut geometry)?;
    }
    Ok(geometry)
}

fn scan_candidate_model_xml<R: Read>(
    input: R,
    path: &str,
    geometry: &mut GeometryEvidence,
) -> Result<(), A1MiniError> {
    let mut reader = Reader::from_reader(BufReader::new(input));
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    let mut current_resource = None;
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| xml_error(path, error))?;
        match event {
            Event::Start(event) => {
                let event_name = event.name();
                let name = local_xml_name(event_name.as_ref());
                if depth == 2 && name == b"object" {
                    let id = required_u32_attribute(&reader, &event, b"id", path)?;
                    current_resource = Some(id);
                    begin_geometry_resource(path, id, geometry);
                }
                if current_resource.is_some() {
                    validate_one_slot_paint(&reader, &event, path)?;
                    record_geometry_element(&reader, &event, path, geometry)?;
                }
                depth += 1;
            }
            Event::Empty(event) => {
                let event_name = event.name();
                let name = local_xml_name(event_name.as_ref());
                if depth == 2 && name == b"object" {
                    let id = required_u32_attribute(&reader, &event, b"id", path)?;
                    begin_geometry_resource(path, id, geometry);
                    validate_one_slot_paint(&reader, &event, path)?;
                    record_geometry_element(&reader, &event, path, geometry)?;
                } else if current_resource.is_some() {
                    validate_one_slot_paint(&reader, &event, path)?;
                    record_geometry_element(&reader, &event, path, geometry)?;
                }
            }
            Event::End(event) => {
                let event_name = event.name();
                let name = local_xml_name(event_name.as_ref());
                if depth.saturating_sub(1) == 2 && name == b"object" {
                    current_resource = None;
                }
                depth = depth.saturating_sub(1);
            }
            Event::DocType(_) => {
                return Err(A1MiniError::SemanticValidation(format!(
                    "DOCTYPE is forbidden in generated model part {path}"
                )));
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(())
}

fn validate_one_slot_paint<R: io::BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    path: &str,
) -> Result<(), A1MiniError> {
    for (key, value) in decoded_attributes(reader, event, path)? {
        let local = local_xml_name(key.as_bytes());
        if local == b"paint_color" || local == b"mmu_segmentation" {
            let states = decode_paint_annotation(&value)
                .map_err(|error| {
                    A1MiniError::SemanticValidation(format!(
                        "generated paint annotation in {path} is invalid: {error}"
                    ))
                })?
                .used_states();
            if states.iter().any(|state| *state > 1) {
                return Err(A1MiniError::SemanticValidation(format!(
                    "generated paint annotation in {path} retains a slot other than 1"
                )));
            }
        }
    }
    Ok(())
}

fn transforms_close(left: Transform3mf, right: Transform3mf) -> bool {
    left.values
        .iter()
        .zip(right.values)
        .all(|(left, right)| (*left - right).abs() <= BOUNDS_TOLERANCE_MM)
}

fn bounds_close(left: Option<AxisAlignedBounds>, right: Option<AxisAlignedBounds>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left
            .min
            .into_iter()
            .chain(left.max)
            .zip(right.min.into_iter().chain(right.max))
            .all(|(left, right)| (left - right).abs() <= BOUNDS_TOLERANCE_MM),
        (None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;
    use u1_planner::{BoundsMm, PrinterPreference};

    const SOURCE_MODEL: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:BambuStudio="http://schemas.bambulab.com/package/2021" unit="millimeter" xml:lang="en-US">
  <metadata name="Application">BambuStudio-02.02.00.85</metadata>
  <metadata name="BambuStudio:3mfVersion">1</metadata>
  <resources>
    <object id="1" type="model"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="1"/></vertices><triangles><triangle v1="0" v2="1" v3="2" paint_color="8"/></triangles></mesh></object>
    <object id="2" type="model"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="2" y="0" z="0"/><vertex x="0" y="2" z="2"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object>
  </resources>
  <build><item objectid="1" transform="1 0 0 0 1 0 0 0 1 2 3 0"/><item objectid="1" transform="1 0 0 0 1 0 0 0 1 30 40 0"/><item objectid="2"/></build>
</model>"#;

    const SOURCE_MODEL_SETTINGS: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<config>
  <object id="1"><metadata key="name" value="Selected"/><metadata key="extruder" value="2"/><metadata face_count="1"/><part id="1" subtype="normal_part"><metadata key="name" value="Text mesh"/><text_info text="A" font_name="Test Font" font_version="1" style_name="Regular" boldness="0" skew="0" font_index="-1" font_size="10" thickness="1" embeded_depth="0" rotate_angle="0" text_gap="0" bold="0" italic="0" surface_type="0" hit_mesh="1" hit_position="0 0 0" hit_normal="0 0 1"/><mesh_stat face_count="1" edges_fixed="0"/></part></object>
  <object id="2"><metadata key="name" value="Excluded"/><metadata key="extruder" value="3"/><metadata face_count="1"/></object>
  <plate><metadata key="plater_id" value="1"/><model_instance><metadata key="object_id" value="1"/><metadata key="instance_id" value="0"/><metadata key="identify_id" value="40"/></model_instance></plate>
  <plate><metadata key="plater_id" value="2"/><model_instance><metadata key="object_id" value="1"/><metadata key="instance_id" value="1"/><metadata key="identify_id" value="41"/></model_instance><model_instance><metadata key="object_id" value="2"/><metadata key="instance_id" value="0"/><metadata key="identify_id" value="50"/></model_instance></plate>
</config>"#;

    #[test]
    fn rewrite_is_deterministic_and_preserves_geometry_while_collapsing_to_slot_one() {
        let directory = tempdir().unwrap();
        let source = write_source_fixture(directory.path());
        let plan = test_plan();
        let first = stage_test_candidate(&source, &plan, &directory.path().join("first.3mf"));
        let second = stage_test_candidate(&source, &plan, &directory.path().join("second.3mf"));
        assert_eq!(first.package_sha256, second.package_sha256);
        assert_eq!(first.package_bytes, second.package_bytes);

        let file = File::open(&first.destination).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        let mut model = String::new();
        archive
            .by_name(MAIN_MODEL_PATH)
            .unwrap()
            .read_to_string(&mut model)
            .unwrap();
        assert!(model.contains("paint_color=\"4\""));
        assert!(model.contains(" 10 20 0\""));
        assert!(!model.contains("objectid=\"2\""));
        assert!(!model.contains("id=\"2\" type=\"model\""));

        let mut settings = String::new();
        archive
            .by_name(MODEL_SETTINGS_PATH)
            .unwrap()
            .read_to_string(&mut settings)
            .unwrap();
        assert!(settings.contains("key=\"extruder\" value=\"1\""));
        assert!(settings.contains("key=\"instance_id\" value=\"0\""));
        assert!(settings.contains("key=\"identify_id\" value=\"41\""));
        assert!(!settings.contains("Excluded"));
        assert!(!settings.contains("text_info"));
        drop(archive);

        let validation = validate_a1mini_output(&first.destination).unwrap();
        assert!(validation.valid, "{:?}", validation.semantic_issues);
        assert_eq!(validation.material, Some(A1MiniMaterial::Pla));
        assert_eq!(validation.plate_count, 1);
        assert_eq!(validation.instance_count, 1);
    }

    #[test]
    fn source_plan_validation_rejects_a_standalone_valid_foreign_placement() {
        let directory = tempdir().unwrap();
        let source = write_source_fixture(directory.path());
        let current = test_plan();
        let mut foreign = current.clone();
        foreign.units[0].target_bounds.min[0] += 5.0;
        foreign.units[0].target_bounds.max[0] += 5.0;
        foreign.units[0].target_transform.values[9] += 5.0;
        foreign.selected_instances.get_mut(&(1, 1)).unwrap().delta_x += 5.0;
        let foreign_candidate = stage_test_candidate(
            &source,
            &foreign,
            &directory.path().join("foreign-placement.3mf"),
        );
        assert!(
            validate_a1mini_output(&foreign_candidate.destination)
                .unwrap()
                .valid
        );

        let mut never_cancelled = || false;
        let (_rewrite, _entries, current_geometry) = rewrite_models_to_archive(
            source.path(),
            &current,
            directory.path(),
            None,
            &mut never_cancelled,
        )
        .unwrap();
        let error =
            validate_a1mini_candidate(&foreign_candidate.destination, &current, &current_geometry)
                .unwrap_err();
        assert!(
            error.to_string().contains("placement")
                || error.to_string().contains("geometry")
                || error.to_string().contains("bounds")
        );
    }

    #[test]
    fn cancellation_before_snapshot_leaves_no_destination_file() {
        let directory = tempdir().unwrap();
        let source = write_source_fixture(directory.path());
        let analysis = analyze_project(source.path()).unwrap();
        let plan = test_plan();
        let destination = directory.path().join(&plan.prepared.file_name);
        let preparation = test_preparation(std::slice::from_ref(&plan));
        let mut is_cancelled = || true;

        let error = convert_prepared_plans(
            source.path(),
            &analysis,
            preparation,
            std::slice::from_ref(&plan),
            directory.path(),
            &mut is_cancelled,
        )
        .unwrap_err();

        assert!(matches!(error, A1MiniError::Cancelled));
        assert!(
            error
                .to_string()
                .starts_with(crate::A1MINI_CANCELLATION_ERROR_PREFIX)
        );
        assert!(!destination.exists());
    }

    #[test]
    fn cancellation_after_first_publication_rolls_back_every_destination() {
        let directory = tempdir().unwrap();
        let source = write_source_fixture(directory.path());
        let analysis = analyze_project(source.path()).unwrap();
        let mut first = test_plan();
        first.prepared.file_name = "cancel-first.3mf".into();
        first.prepared.plate_id = "cancel-plate-1".into();
        first.prepared.job_id = "cancel-job-1".into();
        let mut second = first.clone();
        second.prepared.file_name = "cancel-second.3mf".into();
        second.prepared.plate_id = "cancel-plate-2".into();
        second.prepared.job_id = "cancel-job-2".into();
        let plans = vec![first, second];
        let preparation = test_preparation(&plans);
        let first_destination = directory.path().join(&plans[0].prepared.file_name);
        let second_destination = directory.path().join(&plans[1].prepared.file_name);
        let mut is_cancelled = || first_destination.exists();

        let error = convert_prepared_plans(
            source.path(),
            &analysis,
            preparation,
            &plans,
            directory.path(),
            &mut is_cancelled,
        )
        .unwrap_err();

        assert!(matches!(error, A1MiniError::Cancelled));
        assert!(!first_destination.exists());
        assert!(!second_destination.exists());
    }

    #[test]
    fn streaming_model_rewrite_checks_cancellation_at_bounded_intervals() {
        let mut xml = SOURCE_MODEL.to_vec();
        let insertion = b"<vertex x=\"0\" y=\"0\" z=\"0\"/>".repeat(1_100);
        let marker = b"</vertices>";
        let offset = xml
            .windows(marker.len())
            .position(|window| window == marker)
            .unwrap();
        xml.splice(offset..offset, insertion);
        let plan = test_plan();
        let mut output = Vec::new();
        let mut geometry = GeometryEvidence::default();
        let mut cancellation_checks = 0_usize;
        let mut is_cancelled = || {
            cancellation_checks += 1;
            cancellation_checks >= 2
        };

        let error = rewrite_model_xml(
            xml.as_slice(),
            &mut output,
            MAIN_MODEL_PATH,
            &plan,
            None,
            &mut geometry,
            &mut is_cancelled,
        )
        .unwrap_err();

        assert!(matches!(error, A1MiniError::Cancelled));
        assert_eq!(cancellation_checks, 2);
    }

    #[test]
    fn material_change_requires_a_specific_acknowledgement() {
        let mut scope = test_scope(Material::Petg, false);
        let job = test_job();
        let spool = test_spool();
        let mut evidence = Vec::new();
        let error =
            a1_slot_map_for_unit(&scope, &scope.units[0], &job, &spool, &mut evidence).unwrap_err();
        assert!(error.to_string().contains("without explicit approval"));
        scope.direct_assignments[0].allow_material_substitution = true;
        let mapping =
            a1_slot_map_for_unit(&scope, &scope.units[0], &job, &spool, &mut evidence).unwrap();
        assert_eq!(mapping, BTreeMap::from([(2, 1)]));
        assert_eq!(evidence.len(), 1);
        assert!(evidence[0].approval_id.starts_with("direct-assignment:"));
    }

    #[test]
    fn omitted_text_info_rejects_unknown_or_missing_identity_attributes() {
        let unknown = vec![
            ("text".into(), "A".into()),
            ("unqualified_state".into(), "1".into()),
        ];
        assert!(validate_omitted_text_info(&unknown).is_err());

        let missing_text = vec![("font_name".into(), "Test Font".into())];
        assert!(validate_omitted_text_info(&missing_text).is_err());
    }

    #[test]
    #[ignore = "requires the exact local Bambu Studio installation and the 192 MB forensic sample"]
    fn exact_installation_writes_one_real_forensic_sample_unit() {
        let application = std::env::var_os("A1MINI_BAMBU_STUDIO_APP")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/Applications/BambuStudio.app"));
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../Sample/Withered_Foxy_A1_mini_No_AMS.3mf");
        let analysis = analyze_project(&source).unwrap();
        let (input, result) = forensic_single_unit_plan(&analysis);
        let output = tempdir().unwrap();
        let converted = build_a1mini_qualification_candidates(
            &application,
            &source,
            &analysis,
            &input,
            &result,
            output.path(),
        )
        .unwrap();
        assert_eq!(converted.artifacts.len(), 1);
        assert!(converted.artifacts[0].path.is_file());
        let generated = analyze_project(&converted.artifacts[0].path).unwrap();
        assert_eq!(generated.plates.len(), 1);
        assert_eq!(
            generated.printer.model.as_deref(),
            Some("Bambu Lab A1 mini")
        );
        assert_eq!(generated.plates[0].effective_slots, [1]);
        let validation = validate_a1mini_output(&converted.artifacts[0].path).unwrap();
        assert!(validation.valid, "{:?}", validation.semantic_issues);
    }

    fn stage_test_candidate(
        source: &NamedTempFile,
        plan: &ArtifactBuildPlan,
        destination: &Path,
    ) -> OpcWriteReport {
        let directory = destination.parent().unwrap();
        let mut never_cancelled = || false;
        let (rewrite, entries, geometry) =
            rewrite_models_to_archive(source.path(), plan, directory, None, &mut never_cancelled)
                .unwrap();
        let model_settings =
            rewrite_model_settings(source.path(), plan, &mut never_cancelled).unwrap();
        let source_hash = hash_file(source.path()).unwrap();
        let source_identity = ExpectedSourceIdentity::new(source_hash.0, source_hash.1).unwrap();
        let rewrite_hash = hash_file(rewrite.path()).unwrap();
        let rewrite_identity = ExpectedSourceIdentity::new(rewrite_hash.0, rewrite_hash.1).unwrap();
        let mut types = ContentTypesBuilder::project_3mf();
        types
            .add_override(PROJECT_SETTINGS_PATH, "application/json")
            .unwrap();
        types
            .add_override(MODEL_SETTINGS_PATH, "application/xml")
            .unwrap();
        let relationship =
            OpcRelationship::internal("rel-1", MODEL_RELATIONSHIP_TYPE, MAIN_MODEL_PATH).unwrap();
        let mut package = OpcPackageWriter::new();
        package
            .verify_zip_source(source.path(), source_identity)
            .unwrap()
            .add_bytes("[Content_Types].xml", types.to_xml().unwrap())
            .unwrap()
            .add_bytes("_rels/.rels", relationships_xml(&[relationship]).unwrap())
            .unwrap()
            .add_bytes(PROJECT_SETTINGS_PATH, plan.project_settings.clone())
            .unwrap()
            .add_bytes(MODEL_SETTINGS_PATH, model_settings)
            .unwrap();
        for entry in entries {
            package
                .copy_zip_entry_raw(rewrite.path(), rewrite_identity.clone(), entry)
                .unwrap();
        }
        let staged = package.stage_to(destination).unwrap();
        validate_a1mini_candidate(staged.path(), plan, &geometry).unwrap();
        staged.validate().unwrap().publish().unwrap()
    }

    fn test_preparation(plans: &[ArtifactBuildPlan]) -> A1MiniPreparation {
        A1MiniPreparation {
            adapter_id: A1MINI_ADAPTER_ID.into(),
            source_sha256: "test-source".into(),
            plan_fingerprint: "test-plan".into(),
            artifacts: plans.iter().map(|plan| plan.prepared.clone()).collect(),
            warnings: Vec::new(),
        }
    }

    fn write_source_fixture(directory: &Path) -> NamedTempFile {
        let mut temporary = NamedTempFile::new_in(directory).unwrap();
        {
            let mut zip = ZipWriter::new(temporary.as_file_mut());
            let timestamp = DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0).unwrap();
            let options = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Deflated)
                .last_modified_time(timestamp);
            for (path, bytes) in [
                ("[Content_Types].xml", test_content_types().as_slice()),
                ("_rels/.rels", test_relationships().as_slice()),
                (MAIN_MODEL_PATH, SOURCE_MODEL),
                (PROJECT_SETTINGS_PATH, test_project_settings().as_slice()),
                (MODEL_SETTINGS_PATH, SOURCE_MODEL_SETTINGS),
            ] {
                zip.start_file(path, options).unwrap();
                zip.write_all(bytes).unwrap();
            }
            zip.finish().unwrap();
        }
        temporary.as_file().sync_all().unwrap();
        temporary
    }

    fn test_content_types() -> Vec<u8> {
        let mut types = ContentTypesBuilder::project_3mf();
        types
            .add_override(PROJECT_SETTINGS_PATH, "application/json")
            .unwrap();
        types
            .add_override(MODEL_SETTINGS_PATH, "application/xml")
            .unwrap();
        types.to_xml().unwrap()
    }

    fn test_relationships() -> Vec<u8> {
        relationships_xml(&[OpcRelationship::internal(
            "rel-1",
            MODEL_RELATIONSHIP_TYPE,
            MAIN_MODEL_PATH,
        )
        .unwrap()])
        .unwrap()
    }

    fn test_project_settings() -> Vec<u8> {
        serde_json::to_vec_pretty(&json!({
            "printer_model": "Bambu Lab A1 mini",
            "printer_variant": "0.4",
            "printer_settings_id": crate::A1MINI_MACHINE_PROFILE,
            "print_settings_id": crate::A1MINI_PROCESS_PROFILE,
            "printable_height": "180",
            "printable_area": ["0x0", "180x0", "180x180", "0x180"],
            "has_filament_switcher": "0",
            "version": A1MINI_APPLICATION_VERSION,
            "nozzle_diameter": ["0.4"],
            "filament_settings_id": ["Generic PLA @BBL A1M"],
            "filament_ids": ["GFSL99_02"],
            "filament_colour": ["#010101"],
            "filament_type": ["PLA"],
            "filament_map": ["1"],
            "filament_nozzle_map": ["0"],
            "filament_self_index": ["1"],
            "filament_is_mixed": ["0"],
            "flush_volumes_matrix": ["0"]
        }))
        .unwrap()
    }

    fn test_plan() -> ArtifactBuildPlan {
        let unit = PrintableUnit {
            id: "unit-1".into(),
            source_unit_id: "source-unit-1".into(),
            source_object_id: 1,
            source_instance_id: 1,
            source_model_path: None,
            display_name: "Selected".into(),
            source_plate_id: Some("plate-2".into()),
            requirement_ids: vec!["requirement-1".into()],
            bounds: BoundsMm::from_size(1.0, 1.0, 1.0),
            source_layer_height_mm: Some(0.2),
            printer_preference: PrinterPreference::A1Mini,
        };
        let prepared_spool = A1MiniPreparedSpool {
            spool_id: "spool-1".into(),
            spool_name: "Black".into(),
            material: Material::Pla,
            color: "#010101".into(),
            profile: "Generic PLA @BBL A1M".into(),
            setting_id: "GFSL99_02".into(),
            filament_id: "GFL99".into(),
        };
        ArtifactBuildPlan {
            prepared: A1MiniPreparedArtifact {
                plate_id: "a1-plate-1".into(),
                job_id: "a1-job-1".into(),
                file_name: "test.3mf".into(),
                source_plate_ids: vec![2],
                source_unit_ids: vec![unit.source_unit_id.clone()],
                spool: prepared_spool,
                material_substitutions: Vec::new(),
            },
            spool: A1MiniSpoolSpec {
                spool_id: "spool-1".into(),
                display_name: "Black".into(),
                material: A1MiniMaterial::Pla,
                color: RgbColor::new(1, 1, 1),
                material_substitution_approval_id: None,
            },
            project_settings: test_project_settings(),
            units: vec![ArtifactUnit {
                source: unit,
                identify_id: 41,
                target_instance_id: 0,
                target_bounds: AxisAlignedBounds {
                    min: [10.0, 20.0, 0.0],
                    max: [11.0, 21.0, 1.0],
                },
                target_transform: Transform3mf {
                    values: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 10.0, 20.0, 0.0],
                },
            }],
            object_slot_maps: BTreeMap::from([(1, BTreeMap::from([(2, 1)]))]),
            resource_slot_maps: BTreeMap::new(),
            selected_root_resource_ids: BTreeSet::from([1]),
            external_paths: BTreeSet::new(),
            selected_instances: BTreeMap::from([(
                (1, 1),
                InstancePlacement {
                    delta_x: -20.0,
                    delta_y: -20.0,
                    delta_z: 0.0,
                },
            )]),
        }
    }

    fn test_spool() -> Spool {
        Spool {
            id: "spool-1".into(),
            calibration_id: None,
            display_name: "Black".into(),
            color_name: Some("Black".into()),
            material: Material::Pla,
            nominal_color: RgbColor::new(1, 1, 1),
            measured_color: None,
            sku: None,
            profile_id: None,
            available: true,
        }
    }

    fn test_scope(material: Material, allow_material_substitution: bool) -> u1_planner::PrintScope {
        let unit = test_plan().units.remove(0).source;
        u1_planner::PrintScope {
            id: "scope-1".into(),
            display_name: "Scope".into(),
            requirements: vec![u1_planner::MaterialColorRequirement {
                id: "requirement-1".into(),
                material,
                role: u1_planner::MaterialRole::Functional,
                source_color: RgbColor::new(10, 20, 30),
                source_slots: vec!["F2".into()],
                source_profile_ids: vec!["source-profile".into()],
                cmyx_candidate: u1_planner::CmyxColorCandidate {
                    recipe: u1_planner::CmyxRecipe::Unreachable {
                        reason: "test".into(),
                    },
                    calibration_sample_id: None,
                    process_compatibility: None,
                    required_t4_spool_id: None,
                    predicted_color: None,
                    delta_e00: None,
                    confidence: u1_planner::ColorConfidence::Unknown,
                    warnings: Vec::new(),
                },
                best_effort_cmyx_candidate: None,
                cmyx_palette_candidates: Vec::new(),
                direct_candidates: Vec::new(),
            }],
            units: vec![unit],
            strategy: u1_planner::ScopeStrategy::DirectSpools,
            direct_assignments: vec![u1_planner::DirectAssignmentRequest {
                requirement_id: "requirement-1".into(),
                spool_id: "spool-1".into(),
                toolhead: None,
                allow_material_substitution,
            }],
            dedicated_support: None,
            approved_cmyx_fallbacks: Vec::new(),
            approved_material_substitutions: Vec::new(),
        }
    }

    fn test_job() -> PlannedJob {
        PlannedJob {
            id: "a1-job-1".into(),
            scope_ids: vec!["scope-1".into()],
            units: vec![u1_planner::ScopedUnitRef {
                scope_id: "scope-1".into(),
                unit_id: "unit-1".into(),
            }],
            printer: Printer::A1Mini,
            strategy: ColorStrategy::A1Mono,
            loadout: PrinterLoadout::A1Mini {
                spool_id: "spool-1".into(),
            },
            printable_materials: vec![Material::Pla],
            fast_mono: true,
            full_spectrum_process: None,
            color_mappings: vec![u1_planner::SourceToActualMapping {
                scope_id: "scope-1".into(),
                source_requirement_ids: vec!["requirement-1".into()],
                source_slots: vec!["F2".into()],
                source_profile_ids: vec!["source-profile".into()],
                source_material: Material::Petg,
                source_color: RgbColor::new(10, 20, 30),
                strategy: ColorStrategy::A1Mono,
                cmyx_comparison: u1_planner::CmyxColorCandidate {
                    recipe: u1_planner::CmyxRecipe::Unreachable {
                        reason: "test".into(),
                    },
                    calibration_sample_id: None,
                    process_compatibility: None,
                    required_t4_spool_id: None,
                    predicted_color: None,
                    delta_e00: None,
                    confidence: u1_planner::ColorConfidence::Unknown,
                    warnings: Vec::new(),
                },
                direct_toolhead: None,
                actual_spool_id: Some("spool-1".into()),
                actual_material: Some(Material::Pla),
                actual_color: Some(RgbColor::new(1, 1, 1)),
                delta_e00: None,
                confidence: u1_planner::ColorConfidence::Nominal,
                status: u1_planner::MappingStatus::MaterialMismatch,
            }],
            estimated_tool_changes: u1_planner::Estimate::Estimated(0),
        }
    }

    fn forensic_single_unit_plan(analysis: &ProjectAnalysis) -> (PlanningInput, PlanningResult) {
        let candidate = analysis
            .plates
            .iter()
            .flat_map(|plate| {
                plate.instances.iter().filter_map(move |instance| {
                    let bounds = instance.printable_bounds?;
                    let size = bounds.size()?;
                    let object = analysis.objects.iter().find(|object| {
                        object.id == instance.object_id && object.source_model_path.is_none()
                    })?;
                    let materials = object
                        .effective_material_colors
                        .iter()
                        .filter_map(|value| value.material.as_deref())
                        .map(test_material)
                        .collect::<Option<BTreeSet<_>>>()?;
                    (instance.printable
                        && instance.identify_id.is_some()
                        && size[0] <= 160.0
                        && size[1] <= 160.0
                        && size[2] <= 180.0
                        && materials.len() == 1
                        && !object.effective_material_colors.is_empty())
                    .then_some((
                        plate,
                        instance,
                        object,
                        bounds,
                        materials.into_iter().next()?,
                    ))
                })
            })
            .min_by_key(|(_, _, object, _, _)| object.part_count)
            .expect("the forensic fixture must contain a bounded single-material root unit");
        let (source_plate, instance, object, bounds, material) = candidate;
        let mut requirements = Vec::new();
        let mut mappings = Vec::new();
        let mut requirement_ids = Vec::new();
        for (index, effective) in object.effective_material_colors.iter().enumerate() {
            let requirement_id = format!("requirement-{index}");
            let source_slots = effective
                .source_slots
                .iter()
                .map(|slot| format!("F{slot}"))
                .collect::<Vec<_>>();
            let source_material = effective
                .material
                .as_deref()
                .and_then(test_material)
                .expect("selected object material is supported");
            let source_color = effective
                .color
                .as_deref()
                .and_then(parse_test_color)
                .unwrap_or(RgbColor::new(64, 64, 64));
            let cmyx = u1_planner::CmyxColorCandidate {
                recipe: u1_planner::CmyxRecipe::Unreachable {
                    reason: "A1 forensic writer test".into(),
                },
                calibration_sample_id: None,
                process_compatibility: None,
                required_t4_spool_id: None,
                predicted_color: None,
                delta_e00: None,
                confidence: u1_planner::ColorConfidence::Unknown,
                warnings: Vec::new(),
            };
            requirements.push(u1_planner::MaterialColorRequirement {
                id: requirement_id.clone(),
                material: source_material.clone(),
                role: u1_planner::MaterialRole::Cosmetic,
                source_color,
                source_slots: source_slots.clone(),
                source_profile_ids: effective.source_profile_ids.clone(),
                cmyx_candidate: cmyx.clone(),
                best_effort_cmyx_candidate: None,
                cmyx_palette_candidates: Vec::new(),
                direct_candidates: Vec::new(),
            });
            mappings.push(u1_planner::SourceToActualMapping {
                scope_id: "scope-1".into(),
                source_requirement_ids: vec![requirement_id.clone()],
                source_slots,
                source_profile_ids: effective.source_profile_ids.clone(),
                source_material,
                source_color,
                strategy: ColorStrategy::A1Mono,
                cmyx_comparison: cmyx,
                direct_toolhead: None,
                actual_spool_id: Some("forensic-spool".into()),
                actual_material: Some(material.clone()),
                actual_color: Some(RgbColor::new(32, 32, 32)),
                delta_e00: None,
                confidence: u1_planner::ColorConfidence::Nominal,
                status: u1_planner::MappingStatus::Review,
            });
            requirement_ids.push(requirement_id);
        }
        let size = bounds.size().unwrap();
        let source_object_id = object.source_object_id.unwrap_or(object.id);
        let unit = PrintableUnit {
            id: "unit-1".into(),
            source_unit_id: "forensic-source-unit-1".into(),
            source_object_id,
            source_instance_id: instance.instance_id,
            source_model_path: None,
            display_name: object
                .name
                .clone()
                .unwrap_or_else(|| "Forensic unit".into()),
            source_plate_id: Some(format!("plate-{}", source_plate.id)),
            requirement_ids,
            bounds: BoundsMm::from_size(size[0], size[1], size[2]),
            source_layer_height_mm: analysis.process.layer_height_mm,
            printer_preference: PrinterPreference::A1Mini,
        };
        let scope = u1_planner::PrintScope {
            id: "scope-1".into(),
            display_name: "Forensic unit".into(),
            requirements,
            units: vec![unit.clone()],
            strategy: u1_planner::ScopeStrategy::DirectSpools,
            direct_assignments: unit
                .requirement_ids
                .iter()
                .map(|requirement_id| u1_planner::DirectAssignmentRequest {
                    requirement_id: requirement_id.clone(),
                    spool_id: "forensic-spool".into(),
                    toolhead: None,
                    allow_material_substitution: false,
                })
                .collect(),
            dedicated_support: None,
            approved_cmyx_fallbacks: Vec::new(),
            approved_material_substitutions: Vec::new(),
        };
        let spool = Spool {
            id: "forensic-spool".into(),
            calibration_id: None,
            display_name: "Forensic Black".into(),
            color_name: Some("Black".into()),
            material: material.clone(),
            nominal_color: RgbColor::new(32, 32, 32),
            measured_color: None,
            sku: None,
            profile_id: None,
            available: true,
        };
        let mut config = u1_planner::PlannerConfig::with_cmy_setup(u1_planner::CmySetup {
            cyan_spool_id: "unused-c".into(),
            magenta_spool_id: "unused-m".into(),
            yellow_spool_id: "unused-y".into(),
            default_t4_spool_id: None,
        });
        config.a1_mini.enabled = true;
        let unit_ref = u1_planner::ScopedUnitRef {
            scope_id: "scope-1".into(),
            unit_id: "unit-1".into(),
        };
        let job = PlannedJob {
            id: "a1-job-1".into(),
            scope_ids: vec!["scope-1".into()],
            units: vec![unit_ref.clone()],
            printer: Printer::A1Mini,
            strategy: ColorStrategy::A1Mono,
            loadout: PrinterLoadout::A1Mini {
                spool_id: spool.id.clone(),
            },
            printable_materials: vec![material],
            fast_mono: true,
            full_spectrum_process: None,
            color_mappings: mappings,
            estimated_tool_changes: u1_planner::Estimate::Estimated(0),
        };
        let plate = PlannedPlate {
            id: "a1-plate-1".into(),
            job_id: job.id.clone(),
            printer: Printer::A1Mini,
            units: vec![unit_ref.clone()],
            placements: vec![u1_planner::PlannedPlacement {
                unit: unit_ref,
                target_min_x_mm: 10.0,
                target_min_y_mm: 10.0,
            }],
            prime_tower: None,
            packing_status: PackingStatus::PackedAabb,
            individual_bounds_validated: true,
            full_spectrum_process: None,
            estimated_print_time_seconds: u1_planner::Estimate::RequiresSlicing,
            estimated_material_grams: u1_planner::Estimate::RequiresSlicing,
        };
        (
            PlanningInput {
                scopes: vec![scope],
                inventory: vec![spool],
                current_toolheads: u1_planner::CurrentToolheadState::default(),
                config,
            },
            PlanningResult {
                scope_options: Vec::new(),
                jobs: vec![job],
                plates: vec![plate],
                batches: Vec::new(),
                t4_swap_count: 0,
                a1_spool_change_count: 0,
                final_toolheads: u1_planner::CurrentToolheadState::default(),
                warnings: Vec::new(),
                errors: Vec::new(),
            },
        )
    }

    fn test_material(value: &str) -> Option<Material> {
        match value.trim().to_ascii_uppercase().as_str() {
            "PLA" => Some(Material::Pla),
            "PETG" | "PET" => Some(Material::Petg),
            _ => None,
        }
    }

    fn parse_test_color(value: &str) -> Option<RgbColor> {
        let value = value.trim().strip_prefix('#').unwrap_or(value.trim());
        if value.len() < 6 {
            return None;
        }
        Some(RgbColor::new(
            u8::from_str_radix(&value[0..2], 16).ok()?,
            u8::from_str_radix(&value[2..4], 16).ok()?,
            u8::from_str_radix(&value[4..6], 16).ok()?,
        ))
    }
}
