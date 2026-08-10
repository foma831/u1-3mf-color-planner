//! Deterministic Snapmaker U1 CMY+X calibration-project generation.
//!
//! The implementation is intentionally isolated from desktop state. The
//! public writer added here emits qualification candidates only; it never
//! promotes an unmeasured recipe or an unqualified native writer to production.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use u1_color_engine::{
    ColorBasis, GeometryClass, MixRecipe, PhysicalColor, RecipeComponent, RecipeMode,
    SampleOrientation, SrgbColor, predict_recipe,
};
use u1_orca_adapter::{
    FULL_SPECTRUM_PROFILE_NAME, FULL_SPECTRUM_SETTING_ID, U1_FULL_SPECTRUM_ADAPTER_ID,
    U1FullSpectrumCandidateWriteReport, U1FullSpectrumCompileContext, U1FullSpectrumPhysicalSlot,
    U1FullSpectrumPreparedArtifact, U1FullSpectrumPreparedAssignment, U1FullSpectrumPreparedPlate,
    U1FullSpectrumPreparedUnit, U1FullSpectrumPrimeTower, U1FullSpectrumValidationReport,
    build_u1_full_spectrum_project_settings_with_physical_profiles,
    compile_u1_full_spectrum_calibration_candidate_recipes, inspect_macos_application,
    inspect_u1_full_spectrum_macos_application, qualified_u1_physical_profiles_root,
    u1_full_spectrum_calibration_fingerprint, u1_full_spectrum_process_contract,
    validate_u1_full_spectrum_candidate, write_u1_full_spectrum_qualification_candidate,
};
use u1_planner::{
    CmyxRecipe, ColorConfidence, FullSpectrumMode, Material, RgbColor, ScopedUnitRef, Toolhead,
};
use u1_three_mf::{
    CONTENT_TYPES_PATH, ContentTypesBuilder, MAIN_MODEL_PATH, MODEL_RELATIONSHIP_TYPE,
    OpcPackageWriter, OpcRelationship, ROOT_RELATIONSHIPS_PATH, relationships_xml,
};
use zip::ZipArchive;

use crate::{CmyxCalibrationLoadout, CmyxGeometryContext, full_spectrum_calibration_context};

pub const CMYX_CALIBRATION_PROJECT_SCHEMA_VERSION: u32 = 1;
pub const CMYX_CALIBRATION_MANIFEST_PATH: &str = "Metadata/u1_calibration_manifest.json";
const PROJECT_SETTINGS_PATH: &str = "Metadata/project_settings.config";
const MODEL_SETTINGS_PATH: &str = "Metadata/model_settings.config";
const MAX_SWATCHES: usize = 28;
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_PROJECT_SETTINGS_BYTES: u64 = 64 * 1024 * 1024;
const MAX_MODEL_BYTES: u64 = 16 * 1024 * 1024;

/// A versioned geometry whose dimensions are part of calibration identity.
/// Adding another shape requires a new enum case rather than silently changing
/// the optical context of existing measurements.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CmyxCalibrationGeometryPreset {
    FlatNumberedSwatchV1,
}

/// One unmeasured target on a physical calibration chart.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CmyxCalibrationSwatchSpec {
    pub id: String,
    pub recipe: MixRecipe,
}

/// Complete, immutable request needed to reproduce a chart.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CmyxCalibrationProjectSpec {
    pub schema_version: u32,
    pub project_id: String,
    pub geometry_preset: CmyxCalibrationGeometryPreset,
    /// Exact T1, T2, T3, T4 order, including spool and slicer-profile identity.
    pub loadout: [U1FullSpectrumPhysicalSlot; 4],
    pub swatches: Vec<CmyxCalibrationSwatchSpec>,
}

/// Integer micrometres keep the geometry contract stable across JSON runtimes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CmyxCalibrationGeometryContract {
    pub preset: CmyxCalibrationGeometryPreset,
    pub orientation: SampleOrientation,
    pub geometry_class: GeometryClass,
    pub coupon_width_microns: u32,
    pub coupon_depth_microns: u32,
    pub base_height_microns: u32,
    pub label_height_microns: u32,
    pub gap_microns: u32,
    pub origin_x_microns: u32,
    pub origin_y_microns: u32,
    pub columns: u8,
    pub prime_tower_x_microns: u32,
    pub prime_tower_y_microns: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CmyxCalibrationEntryChecksum {
    pub path: String,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CmyxCalibrationSwatchPlacement {
    pub min_x_microns: u32,
    pub min_y_microns: u32,
    pub max_x_microns: u32,
    pub max_y_microns: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CmyxCalibrationManifestSwatch {
    pub index: u8,
    pub id: String,
    pub suggested_measurement_id: String,
    pub object_id: u32,
    pub target_filament_id: u8,
    pub recipe_fingerprint: Option<String>,
    pub recipe: MixRecipe,
    /// A linear-light average used only as a visual reference. It is not a
    /// measured output and is never attached to planner calibration evidence.
    pub nominal_reference_color: RgbColor,
    pub placement: CmyxCalibrationSwatchPlacement,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CmyxCalibrationProjectManifest {
    pub schema_version: u32,
    pub purpose: String,
    pub production_qualified: bool,
    pub adapter_id: String,
    pub project_id: String,
    pub full_spectrum_loadout_fingerprint: String,
    pub measurement_context: u1_color_engine::CalibrationContext,
    pub loadout: [U1FullSpectrumPhysicalSlot; 4],
    pub process: u1_planner::FullSpectrumProcessCompatibility,
    pub geometry: CmyxCalibrationGeometryContract,
    pub swatches: Vec<CmyxCalibrationManifestSwatch>,
    pub entry_checksums: Vec<CmyxCalibrationEntryChecksum>,
    pub requires_gui_save_slice_reopen: bool,
    pub requires_physical_measurement: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CmyxCalibrationProjectWriteReport {
    pub project_id: String,
    pub production_qualified: bool,
    pub embedded_manifest_path: String,
    pub embedded_manifest_sha256: String,
    pub swatch_count: usize,
    pub candidate: U1FullSpectrumCandidateWriteReport,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CmyxCalibrationProjectValidationReport {
    pub valid: bool,
    pub project_id: Option<String>,
    pub manifest_sha256: Option<String>,
    pub swatch_count: usize,
    pub full_spectrum: U1FullSpectrumValidationReport,
    pub issues: Vec<String>,
}

#[derive(Debug, Error)]
pub enum CmyxCalibrationProjectError {
    #[error("invalid CMY+X calibration project: {0}")]
    InvalidSpec(String),
    #[error("Snapmaker Orca cannot build this calibration candidate: {0}")]
    Capability(String),
    #[error("Full Spectrum calibration candidate failed: {0}")]
    FullSpectrum(String),
    #[error("failed to build deterministic calibration package: {0}")]
    Package(String),
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid calibration candidate ZIP: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("invalid calibration manifest JSON: {0}")]
    ManifestJson(#[from] serde_json::Error),
}

/// Recommended compact chart: four solids, six 50/50 pairs, two additional
/// T4 ratios per CMY primary, an equal CMY ratio, and one four-way cycle.
#[must_use]
pub fn recommended_cmyx_calibration_swatches() -> Vec<CmyxCalibrationSwatchSpec> {
    let mut recipes = (1..=4).map(MixRecipe::solid).collect::<Vec<_>>();
    for (left, right) in [(1, 2), (1, 3), (2, 3), (1, 4), (2, 4), (3, 4)] {
        recipes.push(ratio(&[(left, 1), (right, 1)]));
    }
    for primary in 1..=3 {
        recipes.push(ratio(&[(primary, 3), (4, 1)]));
        recipes.push(ratio(&[(primary, 1), (4, 3)]));
    }
    recipes.push(ratio(&[(1, 1), (2, 1), (3, 1)]));
    recipes.push(MixRecipe {
        mode: RecipeMode::Cycle,
        components: (1..=4)
            .map(|slot| RecipeComponent { slot, weight: 1 })
            .collect(),
    });
    recipes
        .into_iter()
        .enumerate()
        .map(|(index, recipe)| CmyxCalibrationSwatchSpec {
            id: format!("S{:02}", index + 1),
            recipe,
        })
        .collect()
}

/// Builds the versioned recommended chart around a caller-confirmed physical
/// loadout. No inventory, profile, or color value is inferred here.
#[must_use]
pub fn recommended_cmyx_calibration_project_spec(
    project_id: impl Into<String>,
    loadout: [U1FullSpectrumPhysicalSlot; 4],
) -> CmyxCalibrationProjectSpec {
    CmyxCalibrationProjectSpec {
        schema_version: CMYX_CALIBRATION_PROJECT_SCHEMA_VERSION,
        project_id: project_id.into(),
        geometry_preset: CmyxCalibrationGeometryPreset::FlatNumberedSwatchV1,
        loadout,
        swatches: recommended_cmyx_calibration_swatches(),
    }
}

fn ratio(components: &[(u8, u8)]) -> MixRecipe {
    MixRecipe {
        mode: RecipeMode::Ratio,
        components: components
            .iter()
            .map(|(slot, weight)| RecipeComponent {
                slot: *slot,
                weight: *weight,
            })
            .collect(),
    }
}

/// Writes a deterministic, unsliced **qualification candidate**. The function
/// accepts an exact supported installation but deliberately does not require or
/// imply completed Full Spectrum GUI/physical qualification.
pub fn write_cmyx_calibration_project_candidate(
    application_path: &Path,
    spec: &CmyxCalibrationProjectSpec,
    destination: &Path,
) -> Result<CmyxCalibrationProjectWriteReport, CmyxCalibrationProjectError> {
    if destination.extension().and_then(|value| value.to_str()) != Some("3mf") {
        return Err(CmyxCalibrationProjectError::InvalidSpec(
            "destination must have the .3mf extension".into(),
        ));
    }
    if destination
        .try_exists()
        .map_err(|source| CmyxCalibrationProjectError::Read {
            path: destination.to_owned(),
            source,
        })?
    {
        return Err(CmyxCalibrationProjectError::InvalidSpec(format!(
            "destination already exists: {}",
            destination.display()
        )));
    }
    let parent = destination.parent().ok_or_else(|| {
        CmyxCalibrationProjectError::InvalidSpec("destination has no parent directory".into())
    })?;
    if !parent.is_dir() {
        return Err(CmyxCalibrationProjectError::InvalidSpec(format!(
            "destination directory does not exist: {}",
            parent.display()
        )));
    }

    let capability = inspect_u1_full_spectrum_macos_application(application_path)
        .map_err(|error| CmyxCalibrationProjectError::Capability(error.to_string()))?;
    if !capability.installation_supported() || !capability.qualification_candidate_available {
        return Err(CmyxCalibrationProjectError::Capability(
            capability.issues.join(" "),
        ));
    }
    let installation = inspect_macos_application(application_path)
        .map_err(|error| CmyxCalibrationProjectError::Capability(error.to_string()))?;
    let full_spectrum_profiles_root = installation.resources_path.join("profiles/Snapmaker");
    let physical_profiles_root = qualified_u1_physical_profiles_root(application_path)
        .map_err(|error| CmyxCalibrationProjectError::Capability(error.to_string()))?;

    let mut prepared = prepare_project(spec)?;
    let project_settings = build_u1_full_spectrum_project_settings_with_physical_profiles(
        &full_spectrum_profiles_root,
        &physical_profiles_root,
        &prepared.artifact,
    )
    .map_err(|error| CmyxCalibrationProjectError::FullSpectrum(error.to_string()))?;
    prepared.manifest.entry_checksums = core_entry_checksums(&prepared.model, &project_settings);
    let manifest_bytes = canonical_json(&prepared.manifest)?;
    let manifest_sha256 = sha256(&manifest_bytes);

    let scratch = tempfile::Builder::new()
        .prefix(".u1-cmyx-calibration-")
        .tempdir_in(parent)
        .map_err(|source| CmyxCalibrationProjectError::Read {
            path: parent.to_owned(),
            source,
        })?;
    let substrate = scratch.path().join("calibration-substrate.3mf");
    write_calibration_substrate(
        &substrate,
        &prepared.model,
        &prepared.model_settings,
        &project_settings,
        &manifest_bytes,
    )?;
    let manifest_preflight = validate_cmyx_calibration_project_candidate(&substrate)?;
    if !manifest_preflight.valid {
        return Err(CmyxCalibrationProjectError::Package(format!(
            "calibration substrate validation failed: {}",
            manifest_preflight.issues.join("; ")
        )));
    }

    let candidate = write_u1_full_spectrum_qualification_candidate(
        &substrate,
        destination,
        &project_settings,
        &prepared.artifact,
    )
    .map_err(|error| CmyxCalibrationProjectError::FullSpectrum(error.to_string()))?;
    Ok(CmyxCalibrationProjectWriteReport {
        project_id: prepared.manifest.project_id,
        production_qualified: false,
        embedded_manifest_path: CMYX_CALIBRATION_MANIFEST_PATH.into(),
        embedded_manifest_sha256: manifest_sha256,
        swatch_count: prepared.manifest.swatches.len(),
        candidate,
        warnings: vec![
            "This file is an unsliced calibration qualification candidate, not a production-qualified print artifact.".into(),
            "Open, slice, save, close, reopen, and reslice it in the exact supported Snapmaker Orca GUI before relying on the native metadata.".into(),
            "Every printed swatch remains uncalibrated until its physical output is measured and saved with the exact manifest context.".into(),
        ],
    })
}

/// Reconstructs the expected artifact from the embedded manifest and checks
/// entry hashes plus the complete Full Spectrum semantic contract.
pub fn validate_cmyx_calibration_project_candidate(
    path: &Path,
) -> Result<CmyxCalibrationProjectValidationReport, CmyxCalibrationProjectError> {
    let manifest_bytes =
        read_zip_entry_bounded(path, CMYX_CALIBRATION_MANIFEST_PATH, MAX_MANIFEST_BYTES)?;
    let manifest_sha256 = sha256(&manifest_bytes);
    let manifest = serde_json::from_slice::<CmyxCalibrationProjectManifest>(&manifest_bytes)?;
    let spec = CmyxCalibrationProjectSpec {
        schema_version: manifest.schema_version,
        project_id: manifest.project_id.clone(),
        geometry_preset: manifest.geometry.preset,
        loadout: manifest.loadout.clone(),
        swatches: manifest
            .swatches
            .iter()
            .map(|swatch| CmyxCalibrationSwatchSpec {
                id: swatch.id.clone(),
                recipe: swatch.recipe.clone(),
            })
            .collect(),
    };
    let mut issues = Vec::new();
    let prepared = match prepare_project(&spec) {
        Ok(prepared) => Some(prepared),
        Err(error) => {
            issues.push(error.to_string());
            None
        }
    };
    let project_settings =
        read_zip_entry_bounded(path, PROJECT_SETTINGS_PATH, MAX_PROJECT_SETTINGS_BYTES)?;
    let model = read_zip_entry_bounded(path, MAIN_MODEL_PATH, MAX_MODEL_BYTES)?;
    if let Some(prepared) = &prepared {
        let mut expected_manifest = prepared.manifest.clone();
        expected_manifest.entry_checksums = core_entry_checksums(&model, &project_settings);
        if manifest != expected_manifest {
            issues.push(
                "embedded calibration manifest does not match its reconstructed chart contract"
                    .into(),
            );
        }
        if model != prepared.model {
            issues.push("3D model geometry does not match the manifest geometry preset".into());
        }
    }
    for checksum in &manifest.entry_checksums {
        let actual = match checksum.path.as_str() {
            MAIN_MODEL_PATH => sha256(&model),
            PROJECT_SETTINGS_PATH => sha256(&project_settings),
            other => {
                issues.push(format!(
                    "manifest declares unsupported checksum path {other:?}"
                ));
                continue;
            }
        };
        if actual != checksum.sha256 {
            issues.push(format!(
                "entry {} has SHA-256 {}, expected {}",
                checksum.path, actual, checksum.sha256
            ));
        }
    }
    if manifest.entry_checksums.len() != 2 {
        issues.push("manifest must bind exactly the model and project settings entries".into());
    }
    let full_spectrum = validate_u1_full_spectrum_candidate(
        path,
        prepared.as_ref().map(|prepared| &prepared.artifact),
    )
    .map_err(|error| CmyxCalibrationProjectError::FullSpectrum(error.to_string()))?;
    Ok(CmyxCalibrationProjectValidationReport {
        valid: issues.is_empty() && full_spectrum.valid,
        project_id: Some(manifest.project_id),
        manifest_sha256: Some(manifest_sha256),
        swatch_count: manifest.swatches.len(),
        full_spectrum,
        issues,
    })
}

struct PreparedCalibrationProject {
    artifact: U1FullSpectrumPreparedArtifact,
    manifest: CmyxCalibrationProjectManifest,
    model: Vec<u8>,
    model_settings: Vec<u8>,
}

fn prepare_project(
    spec: &CmyxCalibrationProjectSpec,
) -> Result<PreparedCalibrationProject, CmyxCalibrationProjectError> {
    validate_spec(spec)?;
    let geometry = geometry_contract(spec.geometry_preset);
    let calibration_loadout = CmyxCalibrationLoadout::new(
        &spec.loadout[0].spool_id,
        &spec.loadout[1].spool_id,
        &spec.loadout[2].spool_id,
        &spec.loadout[3].spool_id,
    );
    let geometry_context = CmyxGeometryContext {
        orientation: geometry.orientation.clone(),
        geometry_class: geometry.geometry_class.clone(),
    };
    let measurement_context =
        full_spectrum_calibration_context(&calibration_loadout, &geometry_context);
    let process = u1_full_spectrum_process_contract();
    let full_spectrum_loadout_fingerprint = u1_full_spectrum_calibration_fingerprint(&spec.loadout)
        .map_err(|error| CmyxCalibrationProjectError::FullSpectrum(error.to_string()))?;
    let physical_colors = physical_colors(spec);
    let nominal_colors = spec
        .swatches
        .iter()
        .map(|swatch| {
            predict_recipe(&swatch.recipe, &physical_colors, &measurement_context, &[])
                .map(|(color, _, _)| rgb(color.red, color.green, color.blue))
                .map_err(|error| {
                    CmyxCalibrationProjectError::InvalidSpec(format!(
                        "swatch {} has an invalid recipe: {error}",
                        swatch.id
                    ))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let recipe_inputs = spec
        .swatches
        .iter()
        .zip(&nominal_colors)
        .map(|(swatch, nominal)| {
            Ok(u1_orca_adapter::U1FullSpectrumRecipeInput {
                logical_id: swatch.id.clone(),
                target_material: Material::Pla,
                target_color: *nominal,
                recipe: planner_recipe(&swatch.recipe)?,
                calibration_sample_id: None,
                predicted_color: None,
                confidence: ColorConfidence::Nominal,
            })
        })
        .collect::<Result<Vec<_>, CmyxCalibrationProjectError>>()?;
    let recipe_table = compile_u1_full_spectrum_calibration_candidate_recipes(
        &recipe_inputs,
        &U1FullSpectrumCompileContext {
            calibration_fingerprint: full_spectrum_loadout_fingerprint.clone(),
            process: process.clone(),
            t4_color: Some(spec.loadout[3].color),
        },
    )
    .map_err(|error| CmyxCalibrationProjectError::FullSpectrum(error.to_string()))?;
    let targets = recipe_table
        .targets
        .iter()
        .map(|target| (target.logical_id.as_str(), target))
        .collect::<BTreeMap<_, _>>();

    let mut assignments = Vec::with_capacity(spec.swatches.len());
    let mut units = Vec::with_capacity(spec.swatches.len());
    let mut manifest_swatches = Vec::with_capacity(spec.swatches.len());
    for (index, (swatch, nominal)) in spec.swatches.iter().zip(&nominal_colors).enumerate() {
        let target = targets.get(swatch.id.as_str()).ok_or_else(|| {
            CmyxCalibrationProjectError::Package(format!(
                "compiled recipe table lost swatch {}",
                swatch.id
            ))
        })?;
        let placement = swatch_placement(index, &geometry);
        let object_id = u32::try_from(index + 1).expect("chart is bounded to 28 objects");
        let source_slots = swatch
            .recipe
            .components
            .iter()
            .map(|component| component.slot)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        assignments.push(U1FullSpectrumPreparedAssignment {
            scope_id: "cmyx-calibration-chart".into(),
            source_requirement_ids: vec![swatch.id.clone()],
            source_slots,
            source_material: Material::Pla,
            source_color: *nominal,
            target_filament_id: target.target_filament_id,
            recipe_fingerprint: target.recipe_fingerprint.clone(),
            calibration_sample_id: None,
            predicted_color: None,
        });
        units.push(U1FullSpectrumPreparedUnit {
            unit: ScopedUnitRef {
                scope_id: "cmyx-calibration-chart".into(),
                unit_id: swatch.id.clone(),
            },
            source_unit_id: swatch.id.clone(),
            source_object_id: object_id,
            source_instance_id: 0,
            source_model_path: None,
            source_plate_id: Some("calibration-plate-1".into()),
            target_min_x_mm: f64::from(placement.min_x_microns) / 1_000.0,
            target_min_y_mm: f64::from(placement.min_y_microns) / 1_000.0,
            source_to_target_slots: BTreeMap::from([(1, target.target_filament_id)]),
        });
        manifest_swatches.push(CmyxCalibrationManifestSwatch {
            index: u8::try_from(index + 1).expect("chart is bounded to 28 swatches"),
            id: swatch.id.clone(),
            suggested_measurement_id: format!("{}-{}", spec.project_id, swatch.id),
            object_id,
            target_filament_id: target.target_filament_id,
            recipe_fingerprint: target.recipe_fingerprint.clone(),
            recipe: swatch.recipe.clone(),
            nominal_reference_color: *nominal,
            placement,
        });
    }
    let artifact = U1FullSpectrumPreparedArtifact {
        batch_id: format!("calibration-{}", spec.project_id),
        file_name: format!("{}-cmyx-calibration.3mf", spec.project_id),
        loadout: spec.loadout.clone(),
        calibration_fingerprint: full_spectrum_loadout_fingerprint.clone(),
        process: process.clone(),
        recipe_table,
        recipe_calibration_sample_ids: Vec::new(),
        assignments,
        plates: vec![U1FullSpectrumPreparedPlate {
            plan_plate_id: "calibration-plate-1".into(),
            target_plate_id: 1,
            job_id: format!("calibration-{}", spec.project_id),
            prime_tower: Some(U1FullSpectrumPrimeTower {
                x_mm: f64::from(geometry.prime_tower_x_microns) / 1_000.0,
                y_mm: f64::from(geometry.prime_tower_y_microns) / 1_000.0,
            }),
            units,
        }],
    };
    let model = build_model(&manifest_swatches, &geometry);
    let model_settings = build_model_settings(&manifest_swatches);
    let manifest = CmyxCalibrationProjectManifest {
        schema_version: CMYX_CALIBRATION_PROJECT_SCHEMA_VERSION,
        purpose: "cmyx_calibration_candidate".into(),
        production_qualified: false,
        adapter_id: U1_FULL_SPECTRUM_ADAPTER_ID.into(),
        project_id: spec.project_id.clone(),
        full_spectrum_loadout_fingerprint,
        measurement_context,
        loadout: spec.loadout.clone(),
        process,
        geometry,
        swatches: manifest_swatches,
        entry_checksums: Vec::new(),
        requires_gui_save_slice_reopen: true,
        requires_physical_measurement: true,
    };
    Ok(PreparedCalibrationProject {
        artifact,
        manifest,
        model,
        model_settings,
    })
}

fn validate_spec(spec: &CmyxCalibrationProjectSpec) -> Result<(), CmyxCalibrationProjectError> {
    if spec.schema_version != CMYX_CALIBRATION_PROJECT_SCHEMA_VERSION {
        return Err(CmyxCalibrationProjectError::InvalidSpec(format!(
            "schema version {} is unsupported; expected {}",
            spec.schema_version, CMYX_CALIBRATION_PROJECT_SCHEMA_VERSION
        )));
    }
    validate_portable_id(&spec.project_id, "project ID")?;
    if spec.swatches.is_empty() || spec.swatches.len() > MAX_SWATCHES {
        return Err(CmyxCalibrationProjectError::InvalidSpec(format!(
            "chart must contain 1 through {MAX_SWATCHES} swatches"
        )));
    }
    let mut ids = BTreeSet::new();
    for swatch in &spec.swatches {
        validate_portable_id(&swatch.id, "swatch ID")?;
        if !ids.insert(swatch.id.as_str()) {
            return Err(CmyxCalibrationProjectError::InvalidSpec(format!(
                "swatch ID {:?} is duplicated",
                swatch.id
            )));
        }
    }
    let mut spool_ids = BTreeSet::new();
    for (index, slot) in spec.loadout.iter().enumerate() {
        if slot.toolhead != Toolhead::ALL[index] {
            return Err(CmyxCalibrationProjectError::InvalidSpec(format!(
                "loadout element {} must identify T{}",
                index + 1,
                index + 1
            )));
        }
        validate_portable_id(&slot.spool_id, "spool ID")?;
        if !spool_ids.insert(slot.spool_id.as_str()) {
            return Err(CmyxCalibrationProjectError::InvalidSpec(format!(
                "spool ID {:?} occupies multiple toolheads",
                slot.spool_id
            )));
        }
        if slot.spool_name.trim().is_empty() || slot.spool_name.len() > 128 {
            return Err(CmyxCalibrationProjectError::InvalidSpec(format!(
                "T{} spool name is empty or exceeds 128 bytes",
                index + 1
            )));
        }
        if slot.material != Material::Pla {
            return Err(CmyxCalibrationProjectError::InvalidSpec(format!(
                "T{} uses {:?}; this exact Full Spectrum process is PLA only",
                index + 1,
                slot.material
            )));
        }
        if index < 3
            && (slot.profile != FULL_SPECTRUM_PROFILE_NAME
                || slot.setting_id != FULL_SPECTRUM_SETTING_ID)
        {
            return Err(CmyxCalibrationProjectError::InvalidSpec(format!(
                "T{} must use the exact Full Spectrum profile identity",
                index + 1
            )));
        }
    }
    Ok(())
}

fn validate_portable_id(value: &str, label: &str) -> Result<(), CmyxCalibrationProjectError> {
    if value.is_empty()
        || value.len() > 64
        || !value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphanumeric() || (index > 0 && matches!(byte, b'-' | b'_'))
        })
    {
        return Err(CmyxCalibrationProjectError::InvalidSpec(format!(
            "{label} {value:?} must be 1-64 ASCII letters/digits with optional internal '-' or '_'"
        )));
    }
    Ok(())
}

fn geometry_contract(preset: CmyxCalibrationGeometryPreset) -> CmyxCalibrationGeometryContract {
    match preset {
        CmyxCalibrationGeometryPreset::FlatNumberedSwatchV1 => CmyxCalibrationGeometryContract {
            preset,
            orientation: SampleOrientation::Flat,
            geometry_class: GeometryClass::CalibrationSwatch,
            coupon_width_microns: 22_000,
            coupon_depth_microns: 18_000,
            base_height_microns: 1_200,
            label_height_microns: 400,
            gap_microns: 5_000,
            origin_x_microns: 15_000,
            origin_y_microns: 15_000,
            columns: 4,
            prime_tower_x_microns: 205_900,
            prime_tower_y_microns: 198_900,
        },
    }
}

fn physical_colors(spec: &CmyxCalibrationProjectSpec) -> Vec<PhysicalColor> {
    spec.loadout
        .iter()
        .enumerate()
        .map(|(index, slot)| PhysicalColor {
            slot: u8::try_from(index + 1).expect("exactly four slots"),
            calibration_id: slot.spool_id.clone(),
            name: slot.spool_name.clone(),
            srgb: SrgbColor::new(slot.color.red, slot.color.green, slot.color.blue),
            basis: ColorBasis::Nominal,
        })
        .collect()
}

fn planner_recipe(recipe: &MixRecipe) -> Result<CmyxRecipe, CmyxCalibrationProjectError> {
    if recipe.mode == RecipeMode::Solid {
        return Ok(CmyxRecipe::Solid {
            toolhead: slot_toolhead(recipe.components[0].slot)?,
        });
    }
    let sequence = recipe
        .components
        .iter()
        .map(|component| {
            Ok(std::iter::repeat_n(
                slot_toolhead(component.slot)?,
                usize::from(component.weight),
            ))
        })
        .collect::<Result<Vec<_>, CmyxCalibrationProjectError>>()?
        .into_iter()
        .flatten()
        .collect();
    Ok(CmyxRecipe::FullSpectrum {
        mode: match recipe.mode {
            RecipeMode::Solid => unreachable!("solid handled above"),
            RecipeMode::Cycle => FullSpectrumMode::Cycle,
            RecipeMode::Ratio => FullSpectrumMode::Ratio,
            RecipeMode::Match => FullSpectrumMode::Match,
            RecipeMode::Gradient => FullSpectrumMode::Gradient,
        },
        sequence,
    })
}

fn slot_toolhead(slot: u8) -> Result<Toolhead, CmyxCalibrationProjectError> {
    [Toolhead::T1, Toolhead::T2, Toolhead::T3, Toolhead::T4]
        .get(usize::from(slot.saturating_sub(1)))
        .copied()
        .filter(|_| (1..=4).contains(&slot))
        .ok_or_else(|| {
            CmyxCalibrationProjectError::InvalidSpec(format!(
                "recipe references invalid physical slot T{slot}"
            ))
        })
}

fn swatch_placement(
    index: usize,
    geometry: &CmyxCalibrationGeometryContract,
) -> CmyxCalibrationSwatchPlacement {
    let column = u32::try_from(index % usize::from(geometry.columns)).expect("bounded column");
    let row = u32::try_from(index / usize::from(geometry.columns)).expect("bounded row");
    let min_x =
        geometry.origin_x_microns + column * (geometry.coupon_width_microns + geometry.gap_microns);
    let min_y =
        geometry.origin_y_microns + row * (geometry.coupon_depth_microns + geometry.gap_microns);
    CmyxCalibrationSwatchPlacement {
        min_x_microns: min_x,
        min_y_microns: min_y,
        max_x_microns: min_x + geometry.coupon_width_microns,
        max_y_microns: min_y + geometry.coupon_depth_microns,
    }
}

#[derive(Clone, Copy)]
struct Cuboid {
    min_x: u32,
    min_y: u32,
    min_z: u32,
    max_x: u32,
    max_y: u32,
    max_z: u32,
}

fn build_model(
    swatches: &[CmyxCalibrationManifestSwatch],
    geometry: &CmyxCalibrationGeometryContract,
) -> Vec<u8> {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\" xmlns:BambuStudio=\"http://schemas.bambulab.com/package/2021\" unit=\"millimeter\">\n <metadata name=\"Application\">U1 3MF Color Planner</metadata>\n <metadata name=\"BambuStudio:3mfVersion\">1</metadata>\n <metadata name=\"Title\">CMY+X calibration chart</metadata>\n <resources>\n",
    );
    for swatch in swatches {
        let mut vertices = Vec::<[u32; 3]>::new();
        let mut triangles = Vec::<[usize; 3]>::new();
        push_cuboid(
            &mut vertices,
            &mut triangles,
            Cuboid {
                min_x: swatch.placement.min_x_microns,
                min_y: swatch.placement.min_y_microns,
                min_z: 0,
                max_x: swatch.placement.max_x_microns,
                max_y: swatch.placement.max_y_microns,
                max_z: geometry.base_height_microns,
            },
        );
        push_number_label(
            &mut vertices,
            &mut triangles,
            swatch.index,
            swatch.placement.min_x_microns + 5_000,
            swatch.placement.min_y_microns + 2_000,
            geometry.base_height_microns,
            geometry.base_height_microns + geometry.label_height_microns,
        );
        writeln!(
            xml,
            "  <object id=\"{}\" type=\"model\" name=\"{}\"><mesh>",
            swatch.object_id, swatch.id
        )
        .expect("writing to String cannot fail");
        xml.push_str("   <vertices>\n");
        for vertex in vertices {
            writeln!(
                xml,
                "    <vertex x=\"{}\" y=\"{}\" z=\"{}\"/>",
                mm(vertex[0]),
                mm(vertex[1]),
                mm(vertex[2])
            )
            .expect("writing to String cannot fail");
        }
        xml.push_str("   </vertices>\n   <triangles>\n");
        for triangle in triangles {
            writeln!(
                xml,
                "    <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>",
                triangle[0], triangle[1], triangle[2]
            )
            .expect("writing to String cannot fail");
        }
        xml.push_str("   </triangles>\n  </mesh></object>\n");
    }
    xml.push_str(" </resources>\n <build>\n");
    for swatch in swatches {
        writeln!(
            xml,
            "  <item objectid=\"{}\" printable=\"1\"/>",
            swatch.object_id
        )
        .expect("writing to String cannot fail");
    }
    xml.push_str(" </build>\n</model>\n");
    xml.into_bytes()
}

fn push_cuboid(vertices: &mut Vec<[u32; 3]>, triangles: &mut Vec<[usize; 3]>, cube: Cuboid) {
    let base = vertices.len();
    vertices.extend([
        [cube.min_x, cube.min_y, cube.min_z],
        [cube.max_x, cube.min_y, cube.min_z],
        [cube.max_x, cube.max_y, cube.min_z],
        [cube.min_x, cube.max_y, cube.min_z],
        [cube.min_x, cube.min_y, cube.max_z],
        [cube.max_x, cube.min_y, cube.max_z],
        [cube.max_x, cube.max_y, cube.max_z],
        [cube.min_x, cube.max_y, cube.max_z],
    ]);
    for [a, b, c] in [
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [1, 2, 6],
        [1, 6, 5],
        [2, 3, 7],
        [2, 7, 6],
        [3, 0, 4],
        [3, 4, 7],
    ] {
        triangles.push([base + a, base + b, base + c]);
    }
}

fn push_number_label(
    vertices: &mut Vec<[u32; 3]>,
    triangles: &mut Vec<[usize; 3]>,
    number: u8,
    x: u32,
    y: u32,
    min_z: u32,
    max_z: u32,
) {
    push_digit(vertices, triangles, number / 10, x, y, min_z, max_z);
    push_digit(vertices, triangles, number % 10, x + 5_000, y, min_z, max_z);
}

fn push_digit(
    vertices: &mut Vec<[u32; 3]>,
    triangles: &mut Vec<[usize; 3]>,
    digit: u8,
    x: u32,
    y: u32,
    min_z: u32,
    max_z: u32,
) {
    const SEGMENTS: [[bool; 7]; 10] = [
        [true, true, true, false, true, true, true],
        [false, false, true, false, false, true, false],
        [true, false, true, true, true, false, true],
        [true, false, true, true, false, true, true],
        [false, true, true, true, false, true, false],
        [true, true, false, true, false, true, true],
        [true, true, false, true, true, true, true],
        [true, false, true, false, false, true, false],
        [true, true, true, true, true, true, true],
        [true, true, true, true, false, true, true],
    ];
    let boxes = [
        (x + 700, y + 5_300, x + 2_800, y + 6_000),
        (x, y + 3_000, x + 700, y + 5_300),
        (x + 2_800, y + 3_000, x + 3_500, y + 5_300),
        (x + 700, y + 2_650, x + 2_800, y + 3_350),
        (x, y + 700, x + 700, y + 3_000),
        (x + 2_800, y + 700, x + 3_500, y + 3_000),
        (x + 700, y, x + 2_800, y + 700),
    ];
    for (enabled, (min_x, min_y, max_x, max_y)) in SEGMENTS[usize::from(digit)].iter().zip(boxes) {
        if *enabled {
            push_cuboid(
                vertices,
                triangles,
                Cuboid {
                    min_x,
                    min_y,
                    min_z,
                    max_x,
                    max_y,
                    max_z,
                },
            );
        }
    }
}

fn build_model_settings(swatches: &[CmyxCalibrationManifestSwatch]) -> Vec<u8> {
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<config>\n");
    for swatch in swatches {
        writeln!(xml, " <object id=\"{}\">", swatch.object_id)
            .expect("writing to String cannot fail");
        writeln!(
            xml,
            "  <metadata key=\"name\" value=\"Calibration swatch {}\"/>",
            swatch.id
        )
        .expect("writing to String cannot fail");
        writeln!(
            xml,
            "  <metadata key=\"extruder\" value=\"{}\"/>",
            swatch.target_filament_id
        )
        .expect("writing to String cannot fail");
        xml.push_str(" </object>\n");
    }
    xml.push_str(
        " <plate>\n  <metadata key=\"plater_id\" value=\"1\"/>\n  <metadata key=\"plater_name\" value=\"CMY+X calibration chart\"/>\n  <metadata key=\"filament_map_mode\" value=\"Auto For Flush\"/>\n  <metadata key=\"filament_maps\" value=\"1 1 1 1\"/>\n  <metadata key=\"filament_volume_maps\" value=\"0 0 0 0\"/>\n",
    );
    for swatch in swatches {
        writeln!(
            xml,
            "  <model_instance><metadata key=\"object_id\" value=\"{}\"/><metadata key=\"instance_id\" value=\"0\"/></model_instance>",
            swatch.object_id
        )
        .expect("writing to String cannot fail");
    }
    xml.push_str(" </plate>\n</config>\n");
    xml.into_bytes()
}

fn write_calibration_substrate(
    destination: &Path,
    model: &[u8],
    model_settings: &[u8],
    project_settings: &[u8],
    manifest: &[u8],
) -> Result<(), CmyxCalibrationProjectError> {
    let mut content_types = ContentTypesBuilder::project_3mf();
    content_types
        .add_override(format!("/{PROJECT_SETTINGS_PATH}"), "application/json")
        .map_err(|error| CmyxCalibrationProjectError::Package(error.to_string()))?;
    content_types
        .add_override(format!("/{MODEL_SETTINGS_PATH}"), "application/xml")
        .map_err(|error| CmyxCalibrationProjectError::Package(error.to_string()))?;
    content_types
        .add_override(
            format!("/{CMYX_CALIBRATION_MANIFEST_PATH}"),
            "application/json",
        )
        .map_err(|error| CmyxCalibrationProjectError::Package(error.to_string()))?;
    let relationships = relationships_xml(&[OpcRelationship::internal(
        "rel-1",
        MODEL_RELATIONSHIP_TYPE,
        format!("/{MAIN_MODEL_PATH}"),
    )
    .map_err(|error| CmyxCalibrationProjectError::Package(error.to_string()))?])
    .map_err(|error| CmyxCalibrationProjectError::Package(error.to_string()))?;
    let mut package = OpcPackageWriter::new();
    package
        .add_bytes(
            CONTENT_TYPES_PATH,
            content_types
                .to_xml()
                .map_err(|error| CmyxCalibrationProjectError::Package(error.to_string()))?,
        )
        .map_err(|error| CmyxCalibrationProjectError::Package(error.to_string()))?
        .add_bytes(ROOT_RELATIONSHIPS_PATH, relationships)
        .map_err(|error| CmyxCalibrationProjectError::Package(error.to_string()))?
        .add_bytes(MAIN_MODEL_PATH, model.to_vec())
        .map_err(|error| CmyxCalibrationProjectError::Package(error.to_string()))?
        .add_bytes(MODEL_SETTINGS_PATH, model_settings.to_vec())
        .map_err(|error| CmyxCalibrationProjectError::Package(error.to_string()))?
        .add_bytes(PROJECT_SETTINGS_PATH, project_settings.to_vec())
        .map_err(|error| CmyxCalibrationProjectError::Package(error.to_string()))?
        .add_bytes(CMYX_CALIBRATION_MANIFEST_PATH, manifest.to_vec())
        .map_err(|error| CmyxCalibrationProjectError::Package(error.to_string()))?;
    package
        .stage_to(destination)
        .map_err(|error| CmyxCalibrationProjectError::Package(error.to_string()))?
        .validate()
        .map_err(|error| CmyxCalibrationProjectError::Package(error.to_string()))?
        .publish()
        .map_err(|error| CmyxCalibrationProjectError::Package(error.to_string()))?;
    Ok(())
}

fn read_zip_entry_bounded(
    path: &Path,
    entry_path: &str,
    maximum_bytes: u64,
) -> Result<Vec<u8>, CmyxCalibrationProjectError> {
    let file = File::open(path).map_err(|source| CmyxCalibrationProjectError::Read {
        path: path.to_owned(),
        source,
    })?;
    let mut archive = ZipArchive::new(file)?;
    let entry = archive.by_name(entry_path)?;
    if entry.size() > maximum_bytes {
        return Err(CmyxCalibrationProjectError::Package(format!(
            "entry {entry_path:?} exceeds the {maximum_bytes}-byte limit"
        )));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(entry.size()).unwrap_or(0));
    entry
        .take(maximum_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| CmyxCalibrationProjectError::Read {
            path: path.to_owned(),
            source,
        })?;
    if bytes.len() as u64 > maximum_bytes {
        return Err(CmyxCalibrationProjectError::Package(format!(
            "entry {entry_path:?} expands beyond the {maximum_bytes}-byte limit"
        )));
    }
    Ok(bytes)
}

fn canonical_json(
    manifest: &CmyxCalibrationProjectManifest,
) -> Result<Vec<u8>, CmyxCalibrationProjectError> {
    let mut bytes = serde_json::to_vec_pretty(manifest)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn core_entry_checksums(
    model: &[u8],
    project_settings: &[u8],
) -> Vec<CmyxCalibrationEntryChecksum> {
    vec![
        CmyxCalibrationEntryChecksum {
            path: MAIN_MODEL_PATH.into(),
            sha256: sha256(model),
        },
        CmyxCalibrationEntryChecksum {
            path: PROJECT_SETTINGS_PATH.into(),
            sha256: sha256(project_settings),
        },
    ]
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

const fn rgb(red: u8, green: u8, blue: u8) -> RgbColor {
    RgbColor { red, green, blue }
}

fn mm(microns: u32) -> String {
    let whole = microns / 1_000;
    let fraction = microns % 1_000;
    if fraction == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{fraction:03}")
            .trim_end_matches('0')
            .to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use u1_orca_adapter::{
        FULL_SPECTRUM_PROFILE_NAME, FULL_SPECTRUM_SETTING_ID, apply_u1_full_spectrum_project_patch,
    };

    fn loadout(t4_color: RgbColor) -> [U1FullSpectrumPhysicalSlot; 4] {
        let colors = [
            rgb(0x08, 0xab, 0xfb),
            rgb(0xd9, 0x3b, 0x90),
            rgb(0xf9, 0xed, 0x3d),
            t4_color,
        ];
        std::array::from_fn(|index| U1FullSpectrumPhysicalSlot {
            toolhead: Toolhead::ALL[index],
            spool_id: format!("spool-{}", index + 1),
            spool_name: format!("Calibration spool {}", index + 1),
            material: Material::Pla,
            color: colors[index],
            profile: FULL_SPECTRUM_PROFILE_NAME.into(),
            setting_id: FULL_SPECTRUM_SETTING_ID.into(),
            filament_id: "1417031127011".into(),
            wildcard_resolved: index == 3,
        })
    }

    fn spec() -> CmyxCalibrationProjectSpec {
        CmyxCalibrationProjectSpec {
            schema_version: CMYX_CALIBRATION_PROJECT_SCHEMA_VERSION,
            project_id: "black-t4-chart".into(),
            geometry_preset: CmyxCalibrationGeometryPreset::FlatNumberedSwatchV1,
            loadout: loadout(rgb(0, 0, 0)),
            swatches: recommended_cmyx_calibration_swatches(),
        }
    }

    fn test_project_settings(artifact: &U1FullSpectrumPreparedArtifact) -> Vec<u8> {
        let mut settings = BTreeMap::from([(
            "layer_height".to_owned(),
            serde_json::Value::String("0.08".into()),
        )]);
        apply_u1_full_spectrum_project_patch(&mut settings, artifact).unwrap();
        let mut bytes = serde_json::to_vec_pretty(&settings).unwrap();
        bytes.push(b'\n');
        bytes
    }

    #[test]
    fn recommended_chart_is_bounded_unique_and_includes_all_four_physical_solids() {
        let swatches = recommended_cmyx_calibration_swatches();
        assert_eq!(swatches.len(), 18);
        assert_eq!(
            swatches
                .iter()
                .map(|swatch| swatch.id.as_str())
                .collect::<BTreeSet<_>>()
                .len(),
            swatches.len()
        );
        assert_eq!(
            swatches[..4]
                .iter()
                .map(|swatch| swatch.recipe.clone())
                .collect::<Vec<_>>(),
            (1..=4).map(MixRecipe::solid).collect::<Vec<_>>()
        );
    }

    #[test]
    fn black_t4_chart_preparation_is_explicitly_unmeasured_and_deterministic() {
        let first = prepare_project(&spec()).unwrap();
        let second = prepare_project(&spec()).unwrap();
        assert_eq!(first.model, second.model);
        assert_eq!(first.model_settings, second.model_settings);
        assert_eq!(first.artifact, second.artifact);
        assert!(!first.manifest.production_qualified);
        assert!(first.manifest.requires_physical_measurement);
        assert!(
            first
                .artifact
                .recipe_table
                .definitions
                .iter()
                .all(|definition| definition.calibration_sample_id.is_none())
        );
        assert!(
            first
                .model
                .windows(10)
                .any(|window| window == b"name=\"S01\"")
        );
    }

    #[test]
    fn substrate_manifest_and_full_spectrum_contract_validate_together() {
        let directory = tempdir().unwrap();
        let mut prepared = prepare_project(&spec()).unwrap();
        let settings = test_project_settings(&prepared.artifact);
        prepared.manifest.entry_checksums = core_entry_checksums(&prepared.model, &settings);
        let manifest = canonical_json(&prepared.manifest).unwrap();
        let path = directory.path().join("chart.3mf");
        write_calibration_substrate(
            &path,
            &prepared.model,
            &prepared.model_settings,
            &settings,
            &manifest,
        )
        .unwrap();

        let report = validate_cmyx_calibration_project_candidate(&path).unwrap();

        assert!(
            report.valid,
            "{:?} {:?}",
            report.issues, report.full_spectrum.issues
        );
        assert_eq!(report.project_id.as_deref(), Some("black-t4-chart"));
        assert_eq!(report.swatch_count, 18);
    }

    #[test]
    fn validation_rejects_a_manifest_that_lies_about_recipe_geometry() {
        let directory = tempdir().unwrap();
        let mut prepared = prepare_project(&spec()).unwrap();
        let settings = test_project_settings(&prepared.artifact);
        prepared.manifest.entry_checksums = core_entry_checksums(&prepared.model, &settings);
        prepared.manifest.swatches[0].placement.max_x_microns += 1_000;
        let manifest = canonical_json(&prepared.manifest).unwrap();
        let path = directory.path().join("tampered.3mf");
        write_calibration_substrate(
            &path,
            &prepared.model,
            &prepared.model_settings,
            &settings,
            &manifest,
        )
        .unwrap();

        let report = validate_cmyx_calibration_project_candidate(&path).unwrap();

        assert!(!report.valid);
        assert!(report.issues.iter().any(|issue| issue.contains("manifest")));
    }

    #[test]
    fn chart_rejects_duplicate_spool_identity_and_too_many_swatches() {
        let mut duplicate = spec();
        duplicate.loadout[3].spool_id = duplicate.loadout[0].spool_id.clone();
        assert!(prepare_project(&duplicate).is_err());

        let mut too_many = spec();
        too_many.swatches = (0..=MAX_SWATCHES)
            .map(|index| CmyxCalibrationSwatchSpec {
                id: format!("S{}", index + 1),
                recipe: MixRecipe::solid(1),
            })
            .collect();
        assert!(prepare_project(&too_many).is_err());
    }

    #[test]
    #[ignore = "requires the exact local Snapmaker Orca 2.3.5 installation and profile pack"]
    fn installed_orca_builds_a_reproducible_structurally_valid_candidate() {
        let application = Path::new("/Applications/Snapmaker Orca.app");
        let directory = tempdir().unwrap();
        let first_path = directory.path().join("first.3mf");
        let second_path = directory.path().join("second.3mf");

        let first = write_cmyx_calibration_project_candidate(application, &spec(), &first_path)
            .expect("the exact local installation should build a qualification candidate");
        let second = write_cmyx_calibration_project_candidate(application, &spec(), &second_path)
            .expect("the same request should remain reproducible");

        assert!(!first.production_qualified);
        assert_eq!(first.candidate.sha256, second.candidate.sha256);
        assert_eq!(
            first.embedded_manifest_sha256,
            second.embedded_manifest_sha256
        );
        let validation = validate_cmyx_calibration_project_candidate(&first_path).unwrap();
        assert!(validation.valid, "{:?}", validation.issues);
    }
}
