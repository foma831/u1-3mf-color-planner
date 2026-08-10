//! Strict semantic validation for the Snapmaker Orca 2.3.5 Full Spectrum
//! qualification round trip.
//!
//! Snapmaker Orca may regenerate a narrowly versioned set of thumbnails and
//! slice-metadata entries and may renumber process-local IDs. Those are the
//! only tolerated differences. Physical filament identity, native mixed
//! definitions, assignments, process/tower settings, geometry, parts,
//! transforms, placements, and plate membership remain immutable.

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use thiserror::Error;
use u1_three_mf::{
    AnalysisError, OutputValidationPolicy, OutputValidationReport, ProjectAnalysis,
    StagedOutputValidationError, analyze_project, validate_staged_output,
};
use zip::ZipArchive;

use crate::u1_gui_round_trip::{
    U1GuiRoundTripError, U1SemanticModelSnapshot, embedded_preset_entries, gui_ids_are_valid,
    load_u1_semantic_model_snapshot, u1_semantic_geometry_graphs_equivalent,
    u1_semantic_placements_equivalent, verify_analysis_identity,
};
use crate::{
    FULL_SPECTRUM_PROCESS_PROFILE_NAME, FULL_SPECTRUM_PROFILE_NAME, FULL_SPECTRUM_SETTING_ID,
    SUPPORTED_ORCA_VERSION, U1_FULL_SPECTRUM_ADAPTER_ID, U1_MACHINE_PROFILE_NAME,
    U1FullSpectrumError, U1FullSpectrumNativeDefinition, U1FullSpectrumValidationCode,
    U1FullSpectrumValidationReport, U1FullSpectrumValidationSeverity,
    parse_u1_full_spectrum_definitions, validate_u1_full_spectrum_candidate,
};

const PROJECT_SETTINGS_PATH: &str = "Metadata/project_settings.config";
const MAX_CONFIG_BYTES: u64 = 64 * 1024 * 1024;
const QUALIFIED_CMY_COLORS: [&str; 3] = ["#08ABFB", "#D93B90", "#F9ED3D"];
const GUI_CANONICALIZED_DITHERING_KEYS: &[&str] = &[
    "dithering_local_z_mode",
    "dithering_local_z_whole_objects",
    "dithering_local_z_infill",
];

const PROCESS_GLOBAL_KEYS: &[&str] = &[
    "layer_height",
    "mixed_color_layer_height_a",
    "mixed_color_layer_height_b",
    "mixed_filament_gradient_mode",
    "mixed_filament_height_lower_bound",
    "mixed_filament_height_upper_bound",
    "mixed_filament_advanced_dithering",
    "mixed_filament_pointillism_pixel_size",
    "mixed_filament_pointillism_line_gap",
    "mixed_filament_component_bias_enabled",
    "mixed_filament_surface_indentation",
    "mixed_filament_region_collapse",
    "dithering_z_step_size",
    "dithering_local_z_mode",
    "dithering_local_z_whole_objects",
    "dithering_local_z_infill",
    "dithering_local_z_direct_multicolor",
    "dithering_step_painted_zones_only",
    "curr_bed_type",
    "brim_type",
    "print_sequence",
    "first_layer_print_sequence",
    "other_layers_print_sequence",
    "other_layers_print_sequence_nums",
    "spiral_mode",
    "spiral_mode_smooth",
    "spiral_mode_max_xy_smoothing",
    "flush_volumes_matrix",
    "flush_volumes_vector",
    "flush_multiplier",
];

const PRIME_TOWER_KEYS: &[&str] = &[
    "enable_prime_tower",
    "prime_tower_width",
    "prime_volume",
    "prime_tower_brim_width",
    "wipe_tower_cone_angle",
    "wipe_tower_extra_spacing",
    "wipe_tower_extra_rib_length",
    "wipe_tower_wall_type",
    "wipe_tower_filament",
    "wipe_tower_rotation_angle",
    "wipe_tower_x",
    "wipe_tower_y",
];

/// Role of one file in a Full Spectrum GUI qualification sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum U1FullSpectrumGuiArtifactRole {
    WriterCandidate,
    FirstGuiSave,
    ReopenedGuiSave,
}

/// Stable issue categories emitted by the Full Spectrum round-trip validator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum U1FullSpectrumGuiRoundTripIssueCode {
    StructuralArtifactPolicyFailed,
    FullSpectrumContractRejected,
    InvalidPhysicalCmyxIdentity,
    PhysicalT1T4IdentityChanged,
    MissingMixedDefinition,
    MixedDefinitionsChanged,
    MissingVirtualAssignment,
    VirtualAssignmentsChanged,
    MissingSolidT4Assignment,
    SolidT4AssignmentChanged,
    SubdivisionOrProcessGlobalsChanged,
    InvalidPrimeTowerContract,
    PrimeTowerContractChanged,
    EmbeddedPresetFound,
    InvalidProcessLocalIds,
    GeometryOrPartChanged,
    PlacementOrPlateMembershipChanged,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumGuiRoundTripIssue {
    pub code: U1FullSpectrumGuiRoundTripIssueCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<U1FullSpectrumGuiArtifactRole>,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumGuiArtifactSummary {
    pub role: U1FullSpectrumGuiArtifactRole,
    pub byte_size: u64,
    pub sha256: String,
    pub plate_count: usize,
    pub object_count: usize,
    pub instance_count: usize,
    pub part_count: usize,
    pub geometry_resource_count: usize,
    pub used_filament_ids: Vec<u8>,
    pub virtual_filament_count: usize,
    pub full_spectrum_contract_valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_sha256: Option<String>,
    pub structural_validation: OutputValidationReport,
    pub full_spectrum_validation: U1FullSpectrumValidationReport,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumGuiRoundTripChecks {
    pub candidate_structurally_valid: bool,
    pub first_gui_save_structurally_valid: bool,
    pub reopened_gui_save_structurally_valid: bool,
    pub every_artifact_has_exact_full_spectrum_contract: bool,
    pub candidate_has_exact_physical_cmyx_identity: bool,
    pub physical_t1_t4_identity_stable: bool,
    pub candidate_has_mixed_definition: bool,
    pub mixed_definitions_stable: bool,
    pub candidate_has_virtual_assignment: bool,
    pub virtual_assignments_stable: bool,
    pub candidate_has_solid_t4_assignment: bool,
    pub solid_t4_assignment_stable: bool,
    pub subdivision_and_process_globals_stable: bool,
    pub candidate_prime_tower_contract_active: bool,
    pub prime_tower_contract_stable: bool,
    pub no_embedded_presets: bool,
    pub process_local_ids_valid: bool,
    pub geometry_and_parts_stable: bool,
    pub placements_and_plate_membership_stable: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumGuiRoundTripReport {
    pub schema_version: u32,
    pub adapter_id: &'static str,
    pub application_version: &'static str,
    pub is_valid: bool,
    pub artifacts: Vec<U1FullSpectrumGuiArtifactSummary>,
    pub checks: U1FullSpectrumGuiRoundTripChecks,
    pub issues: Vec<U1FullSpectrumGuiRoundTripIssue>,
}

#[derive(Debug, Error)]
pub enum U1FullSpectrumGuiRoundTripError {
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
    #[error("failed to validate the Full Spectrum contract in {path}: {source}")]
    FullSpectrum {
        path: PathBuf,
        #[source]
        source: U1FullSpectrumError,
    },
    #[error("failed to inspect semantic model data in {path}: {source}")]
    SemanticModel {
        path: PathBuf,
        #[source]
        source: U1GuiRoundTripError,
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
        entry: &'static str,
        limit: u64,
    },
    #[error("invalid JSON in {entry} from {path}: {source}")]
    Json {
        path: PathBuf,
        entry: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid Full Spectrum round-trip data in {path}: {message}")]
    Semantic { path: PathBuf, message: String },
}

/// Validate an unsliced writer candidate, its first Snapmaker Orca save, and
/// the save produced after closing and reopening the first save.
///
/// The writer candidate must satisfy the strict unsliced policy. GUI saves use
/// the version-scoped 2.3.5 allowlist for regenerated thumbnails and slice
/// metadata; G-code, embedded presets, unknown derived entries, and stale
/// references remain forbidden.
pub fn validate_u1_full_spectrum_gui_round_trip(
    writer_candidate: impl AsRef<Path>,
    first_gui_save: impl AsRef<Path>,
    reopened_gui_save: impl AsRef<Path>,
) -> Result<U1FullSpectrumGuiRoundTripReport, U1FullSpectrumGuiRoundTripError> {
    let paths = [
        writer_candidate.as_ref(),
        first_gui_save.as_ref(),
        reopened_gui_save.as_ref(),
    ];
    let roles = [
        U1FullSpectrumGuiArtifactRole::WriterCandidate,
        U1FullSpectrumGuiArtifactRole::FirstGuiSave,
        U1FullSpectrumGuiArtifactRole::ReopenedGuiSave,
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
    let validations = paths
        .iter()
        .map(|path| {
            validate_u1_full_spectrum_candidate(path, None).map_err(|source| {
                U1FullSpectrumGuiRoundTripError::FullSpectrum {
                    path: (*path).to_path_buf(),
                    source,
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    for (path, analysis) in paths.iter().zip(&analyses) {
        verify_analysis_identity(path, analysis).map_err(|source| {
            U1FullSpectrumGuiRoundTripError::SemanticModel {
                path: (*path).to_path_buf(),
                source,
            }
        })?;
    }

    let embedded_presets = paths
        .iter()
        .map(|path| {
            embedded_preset_entries(path).map_err(|source| {
                U1FullSpectrumGuiRoundTripError::SemanticModel {
                    path: (*path).to_path_buf(),
                    source,
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let contract_valid = roles
        .iter()
        .zip(&structural)
        .zip(&validations)
        .map(|((role, structural), validation)| {
            structural.is_valid && full_spectrum_contract_is_acceptable(*role, validation)
        })
        .collect::<Vec<_>>();

    let mut issues = Vec::new();
    for (((role, path), structural), validation) in
        roles.iter().zip(paths).zip(&structural).zip(&validations)
    {
        if !structural.is_valid {
            issues.push(U1FullSpectrumGuiRoundTripIssue {
                code: U1FullSpectrumGuiRoundTripIssueCode::StructuralArtifactPolicyFailed,
                artifact: Some(*role),
                message: format!(
                    "{} failed the version-scoped artifact policy with {} issue(s); repair output, toolpaths, embedded presets, and unknown stale parts are forbidden.",
                    path.display(),
                    structural.issues.len()
                ),
            });
        }
        for issue in disallowed_full_spectrum_issues(*role, validation) {
            issues.push(U1FullSpectrumGuiRoundTripIssue {
                code: U1FullSpectrumGuiRoundTripIssueCode::FullSpectrumContractRejected,
                artifact: Some(*role),
                message: format!(
                    "Full Spectrum {:?} validation failed{}: {}",
                    issue.code,
                    issue
                        .path
                        .as_deref()
                        .map(|path| format!(" at {path}"))
                        .unwrap_or_default(),
                    issue.message
                ),
            });
        }
    }

    let no_embedded_presets = embedded_presets.iter().all(Vec::is_empty);
    for ((role, path), presets) in roles.iter().zip(paths).zip(&embedded_presets) {
        if !presets.is_empty() {
            issues.push(U1FullSpectrumGuiRoundTripIssue {
                code: U1FullSpectrumGuiRoundTripIssueCode::EmbeddedPresetFound,
                artifact: Some(*role),
                message: format!(
                    "{} contains forbidden embedded preset entries: {presets:?}.",
                    path.display()
                ),
            });
        }
    }

    let can_compare_semantics = contract_valid.iter().all(|valid| *valid)
        && structural.iter().all(|report| report.is_valid)
        && no_embedded_presets;
    let contracts = can_compare_semantics
        .then(|| {
            paths
                .iter()
                .map(|path| read_project_contract(path))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    let models = can_compare_semantics
        .then(|| {
            paths
                .iter()
                .zip(&analyses)
                .map(|(path, analysis)| {
                    load_u1_semantic_model_snapshot(path, analysis).map_err(|source| {
                        U1FullSpectrumGuiRoundTripError::SemanticModel {
                            path: (*path).to_path_buf(),
                            source,
                        }
                    })
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();

    for (path, analysis) in paths.iter().zip(&analyses) {
        verify_analysis_identity(path, analysis).map_err(|source| {
            U1FullSpectrumGuiRoundTripError::SemanticModel {
                path: (*path).to_path_buf(),
                source,
            }
        })?;
    }

    let process_local_ids_valid = analyses.iter().all(gui_ids_are_valid);
    if !process_local_ids_valid {
        issues.push(U1FullSpectrumGuiRoundTripIssue {
            code: U1FullSpectrumGuiRoundTripIssueCode::InvalidProcessLocalIds,
            artifact: None,
            message: "Object, part, plate, instance, and identify IDs must remain positive, unique, and internally consistent; numeric equality between GUI saves is not required."
                .to_owned(),
        });
    }

    let mut checks = empty_checks(&structural, &contract_valid, no_embedded_presets);
    checks.process_local_ids_valid = process_local_ids_valid;

    if can_compare_semantics {
        checks.candidate_has_exact_physical_cmyx_identity = contracts[0].physical.is_exact_cmyx();
        if !checks.candidate_has_exact_physical_cmyx_identity {
            issues.push(U1FullSpectrumGuiRoundTripIssue {
                code: U1FullSpectrumGuiRoundTripIssueCode::InvalidPhysicalCmyxIdentity,
                artifact: Some(U1FullSpectrumGuiArtifactRole::WriterCandidate),
                message: "Writer candidate does not preserve the exact CMY physical identities in T1-T3 and a canonical solid X identity in T4."
                    .to_owned(),
            });
        }

        checks.physical_t1_t4_identity_stable = contracts.iter().skip(1).all(|contract| {
            contract
                .physical
                .has_same_physical_loadout(&contracts[0].physical)
                && contract
                    .physical
                    .has_expected_gui_default_color_cache(&contracts[0].physical)
        });
        if !checks.physical_t1_t4_identity_stable {
            issues.push(U1FullSpectrumGuiRoundTripIssue {
                code: U1FullSpectrumGuiRoundTripIssueCode::PhysicalT1T4IdentityChanged,
                artifact: None,
                message: "Machine identity or a T1-T4 profile, setting ID, material, nozzle, or color changed during the GUI round trip."
                    .to_owned(),
            });
        }

        checks.candidate_has_mixed_definition = !contracts[0].definitions.is_empty();
        if !checks.candidate_has_mixed_definition {
            issues.push(U1FullSpectrumGuiRoundTripIssue {
                code: U1FullSpectrumGuiRoundTripIssueCode::MissingMixedDefinition,
                artifact: Some(U1FullSpectrumGuiArtifactRole::WriterCandidate),
                message: "Qualification requires at least one canonical native mixed-filament definition."
                    .to_owned(),
            });
        }
        checks.mixed_definitions_stable = all_equal_by(&contracts, |value| &value.definitions);
        if !checks.mixed_definitions_stable {
            issues.push(U1FullSpectrumGuiRoundTripIssue {
                code: U1FullSpectrumGuiRoundTripIssueCode::MixedDefinitionsChanged,
                artifact: None,
                message: "Native mixed-filament definitions, stable IDs, component order, ratios, or z-subdivision changed during a GUI save."
                    .to_owned(),
            });
        }

        let printable_slots = analyses
            .iter()
            .map(printable_filament_ids)
            .collect::<Vec<_>>();
        let virtual_ids = printable_slots
            .iter()
            .map(|ids| ids.iter().copied().filter(|id| *id > 4).collect::<Vec<_>>())
            .collect::<Vec<_>>();
        checks.candidate_has_virtual_assignment = !virtual_ids[0].is_empty();
        if !checks.candidate_has_virtual_assignment {
            issues.push(U1FullSpectrumGuiRoundTripIssue {
                code: U1FullSpectrumGuiRoundTripIssueCode::MissingVirtualAssignment,
                artifact: Some(U1FullSpectrumGuiArtifactRole::WriterCandidate),
                message: "Qualification requires at least one virtual filament assigned to retained printable geometry."
                    .to_owned(),
            });
        }

        let objects_stable = all_equal_by(&models, |value| &value.object_multiset);
        checks.virtual_assignments_stable = all_equal(&virtual_ids) && objects_stable;
        if !checks.virtual_assignments_stable {
            issues.push(U1FullSpectrumGuiRoundTripIssue {
                code: U1FullSpectrumGuiRoundTripIssueCode::VirtualAssignmentsChanged,
                artifact: None,
                message: "Object, part, or decoded per-facet virtual-filament assignments changed during the GUI round trip."
                    .to_owned(),
            });
        }

        checks.candidate_has_solid_t4_assignment = printable_slots[0].contains(&4);
        if !checks.candidate_has_solid_t4_assignment {
            issues.push(U1FullSpectrumGuiRoundTripIssue {
                code: U1FullSpectrumGuiRoundTripIssueCode::MissingSolidT4Assignment,
                artifact: Some(U1FullSpectrumGuiArtifactRole::WriterCandidate),
                message: "Qualification requires a retained printable region assigned directly to physical T4."
                    .to_owned(),
            });
        }
        let solid_t4_presence = printable_slots
            .iter()
            .map(|ids| ids.contains(&4))
            .collect::<Vec<_>>();
        checks.solid_t4_assignment_stable = all_equal(&solid_t4_presence) && objects_stable;
        if !checks.solid_t4_assignment_stable {
            issues.push(U1FullSpectrumGuiRoundTripIssue {
                code: U1FullSpectrumGuiRoundTripIssueCode::SolidT4AssignmentChanged,
                artifact: None,
                message: "The direct physical-T4 region was lost or reassigned during the GUI round trip."
                    .to_owned(),
            });
        }

        checks.subdivision_and_process_globals_stable =
            process_globals_are_round_trip_stable(&contracts);
        if !checks.subdivision_and_process_globals_stable {
            issues.push(U1FullSpectrumGuiRoundTripIssue {
                code: U1FullSpectrumGuiRoundTripIssueCode::SubdivisionOrProcessGlobalsChanged,
                artifact: None,
                message: "Full Spectrum subdivision, dithering, flush, layer, bed, or sequencing globals changed during a GUI save."
                    .to_owned(),
            });
        }

        checks.candidate_prime_tower_contract_active = contracts[0].has_active_prime_tower();
        if !checks.candidate_prime_tower_contract_active {
            issues.push(U1FullSpectrumGuiRoundTripIssue {
                code: U1FullSpectrumGuiRoundTripIssueCode::InvalidPrimeTowerContract,
                artifact: Some(U1FullSpectrumGuiArtifactRole::WriterCandidate),
                message: "Qualification requires at least one active, non-zero prime-tower placement for the mixed/solid multi-filament plate."
                    .to_owned(),
            });
        }
        checks.prime_tower_contract_stable =
            prime_tower_contracts_are_round_trip_stable(&contracts);
        if !checks.prime_tower_contract_stable {
            issues.push(U1FullSpectrumGuiRoundTripIssue {
                code: U1FullSpectrumGuiRoundTripIssueCode::PrimeTowerContractChanged,
                artifact: None,
                message: "Prime-tower dimensions, material selection, or per-plate placement changed during a GUI save."
                    .to_owned(),
            });
        }

        checks.geometry_and_parts_stable = objects_stable
            && semantic_geometry_equivalent(paths[0], &models[0], paths[1], &models[1])?
            && semantic_geometry_equivalent(paths[0], &models[0], paths[2], &models[2])?;
        if !checks.geometry_and_parts_stable {
            issues.push(U1FullSpectrumGuiRoundTripIssue {
                code: U1FullSpectrumGuiRoundTripIssueCode::GeometryOrPartChanged,
                artifact: None,
                message: "Mesh vertices/triangles, decoded paint trees, object/part identity, printable bounds, or part semantics changed."
                    .to_owned(),
            });
        }

        checks.placements_and_plate_membership_stable =
            u1_semantic_placements_equivalent(&models[0], &models[1])
                && u1_semantic_placements_equivalent(&models[0], &models[2]);
        if !checks.placements_and_plate_membership_stable {
            issues.push(U1FullSpectrumGuiRoundTripIssue {
                code: U1FullSpectrumGuiRoundTripIssueCode::PlacementOrPlateMembershipChanged,
                artifact: None,
                message: "Instance transforms, printable bounds, or plate membership changed during the GUI round trip."
                    .to_owned(),
            });
        }
    }

    issues.sort();
    issues.dedup();
    let artifacts = build_artifact_summaries(
        roles,
        analyses,
        structural,
        validations,
        contract_valid,
        &contracts,
        &models,
    )?;

    Ok(U1FullSpectrumGuiRoundTripReport {
        schema_version: 1,
        adapter_id: U1_FULL_SPECTRUM_ADAPTER_ID,
        application_version: SUPPORTED_ORCA_VERSION,
        is_valid: issues.is_empty(),
        artifacts,
        checks,
        issues,
    })
}

fn analyze(path: &Path) -> Result<ProjectAnalysis, U1FullSpectrumGuiRoundTripError> {
    analyze_project(path).map_err(|source| U1FullSpectrumGuiRoundTripError::Analyze {
        path: path.to_path_buf(),
        source,
    })
}

fn semantic_geometry_equivalent(
    first_path: &Path,
    first: &U1SemanticModelSnapshot,
    second_path: &Path,
    second: &U1SemanticModelSnapshot,
) -> Result<bool, U1FullSpectrumGuiRoundTripError> {
    u1_semantic_geometry_graphs_equivalent(first_path, first, second_path, second).map_err(
        |source| U1FullSpectrumGuiRoundTripError::SemanticModel {
            path: second_path.to_path_buf(),
            source,
        },
    )
}

fn structurally_validate(
    path: &Path,
    policy: &OutputValidationPolicy,
) -> Result<OutputValidationReport, U1FullSpectrumGuiRoundTripError> {
    validate_staged_output(path, policy).map_err(|source| {
        U1FullSpectrumGuiRoundTripError::Structural {
            path: path.to_path_buf(),
            source,
        }
    })
}

fn disallowed_full_spectrum_issues(
    role: U1FullSpectrumGuiArtifactRole,
    report: &U1FullSpectrumValidationReport,
) -> impl Iterator<Item = &crate::U1FullSpectrumValidationIssue> {
    report.issues.iter().filter(move |issue| {
        issue.severity == U1FullSpectrumValidationSeverity::Error
            && !is_expected_gui_save_validation_issue(role, issue)
    })
}

fn is_expected_gui_save_validation_issue(
    role: U1FullSpectrumGuiArtifactRole,
    issue: &crate::U1FullSpectrumValidationIssue,
) -> bool {
    if role == U1FullSpectrumGuiArtifactRole::WriterCandidate {
        return false;
    }
    match issue.code {
        // Snapmaker Orca 2.3.5 regenerates these entries after slicing. The
        // structural GUI-save policy still rejects G-code, repair output,
        // embedded presets, and unknown derived entries.
        U1FullSpectrumValidationCode::StaleArtifact => true,
        // The pristine-candidate validator only knows about four physical
        // tools, so it reports retained virtual assignments (T5+) as out of
        // range. The round-trip validator independently requires definitions
        // and every decoded assignment to remain identical to the valid
        // candidate before accepting this exception.
        U1FullSpectrumValidationCode::AssignmentOutOfRange => true,
        // A GUI save canonicalizes the editor-mode switch to zero after the
        // already-materialized mixed assignments have been sliced. No other
        // project-setting failure is tolerated.
        U1FullSpectrumValidationCode::InvalidProjectSettings => {
            issue.path.as_deref() == Some(PROJECT_SETTINGS_PATH)
                && issue.message
                    == "generated Full Spectrum candidate failed semantic validation: project setting dithering_local_z_mode is \"0\"; expected \"1\""
        }
        _ => false,
    }
}

fn full_spectrum_contract_is_acceptable(
    role: U1FullSpectrumGuiArtifactRole,
    report: &U1FullSpectrumValidationReport,
) -> bool {
    disallowed_full_spectrum_issues(role, report)
        .next()
        .is_none()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhysicalT1T4Identity {
    version: String,
    printer_model: String,
    printer_variant: String,
    machine_profile: String,
    process_profile: String,
    nozzle_diameters: [String; 4],
    filament_profiles: [String; 4],
    filament_setting_ids: [String; 4],
    filament_colors: [String; 4],
    default_filament_colors: [String; 4],
    filament_materials: [String; 4],
}

impl PhysicalT1T4Identity {
    fn is_exact_cmyx(&self) -> bool {
        self.version == SUPPORTED_ORCA_VERSION
            && self.printer_model == "Snapmaker U1"
            && self.printer_variant == "0.4"
            && self.machine_profile == U1_MACHINE_PROFILE_NAME
            && self.process_profile == FULL_SPECTRUM_PROCESS_PROFILE_NAME
            && self.nozzle_diameters.iter().all(|value| value == "0.4")
            && self
                .filament_profiles
                .iter()
                .take(3)
                .all(|value| value == FULL_SPECTRUM_PROFILE_NAME)
            && self
                .filament_setting_ids
                .iter()
                .take(3)
                .all(|value| value == FULL_SPECTRUM_SETTING_ID)
            && self.filament_materials.iter().all(|value| value == "PLA")
            && self.filament_colors[..3] == QUALIFIED_CMY_COLORS
            && self.default_filament_colors == self.filament_colors
            && !self.filament_profiles[3].is_empty()
            && !self.filament_setting_ids[3].is_empty()
            && is_canonical_rgb_hex(&self.filament_colors[3])
    }

    fn has_same_physical_loadout(&self, other: &Self) -> bool {
        self.version == other.version
            && self.printer_model == other.printer_model
            && self.printer_variant == other.printer_variant
            && self.machine_profile == other.machine_profile
            && self.process_profile == other.process_profile
            && self.nozzle_diameters == other.nozzle_diameters
            && self.filament_profiles == other.filament_profiles
            && self.filament_setting_ids == other.filament_setting_ids
            && self.filament_colors == other.filament_colors
            && self.filament_materials == other.filament_materials
    }

    fn has_expected_gui_default_color_cache(&self, candidate: &Self) -> bool {
        self.default_filament_colors == candidate.filament_colors
            || self.default_filament_colors.iter().all(String::is_empty)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct FullSpectrumProjectContract {
    physical: PhysicalT1T4Identity,
    definitions: Vec<U1FullSpectrumNativeDefinition>,
    process_globals: BTreeMap<String, Value>,
    prime_tower: BTreeMap<String, Value>,
}

impl FullSpectrumProjectContract {
    fn has_active_prime_tower(&self) -> bool {
        if self
            .prime_tower
            .get("enable_prime_tower")
            .and_then(Value::as_str)
            != Some("1")
        {
            return false;
        }
        let coordinates = |key: &str| {
            self.prime_tower
                .get(key)
                .and_then(Value::as_array)
                .and_then(|values| {
                    values
                        .iter()
                        .map(Value::as_str)
                        .map(|value| value?.parse::<f64>().ok())
                        .collect::<Option<Vec<_>>>()
                })
        };
        let (Some(x), Some(y)) = (coordinates("wipe_tower_x"), coordinates("wipe_tower_y")) else {
            return false;
        };
        !x.is_empty()
            && x.len() == y.len()
            && x.iter()
                .zip(y)
                .any(|(x, y)| x.is_finite() && y.is_finite() && *x != 0.0 && y != 0.0)
    }
}

fn read_project_contract(
    path: &Path,
) -> Result<FullSpectrumProjectContract, U1FullSpectrumGuiRoundTripError> {
    let bytes = read_named_entry(path, PROJECT_SETTINGS_PATH, MAX_CONFIG_BYTES)?;
    let settings = serde_json::from_slice::<BTreeMap<String, Value>>(&bytes).map_err(|source| {
        U1FullSpectrumGuiRoundTripError::Json {
            path: path.to_path_buf(),
            entry: PROJECT_SETTINGS_PATH,
            source,
        }
    })?;
    let definitions = settings
        .get("mixed_filament_definitions")
        .and_then(Value::as_str)
        .ok_or_else(|| U1FullSpectrumGuiRoundTripError::Semantic {
            path: path.to_path_buf(),
            message: "mixed_filament_definitions is missing or is not a string".to_owned(),
        })
        .and_then(|serialized| {
            parse_u1_full_spectrum_definitions(serialized).map_err(|error| {
                U1FullSpectrumGuiRoundTripError::Semantic {
                    path: path.to_path_buf(),
                    message: format!("invalid mixed_filament_definitions: {error}"),
                }
            })
        })?;
    Ok(FullSpectrumProjectContract {
        physical: PhysicalT1T4Identity {
            version: required_string(path, &settings, "version")?,
            printer_model: required_string(path, &settings, "printer_model")?,
            printer_variant: required_string(path, &settings, "printer_variant")?,
            machine_profile: required_string(path, &settings, "printer_settings_id")?,
            process_profile: required_string(path, &settings, "print_settings_id")?,
            nozzle_diameters: required_string_array4(path, &settings, "nozzle_diameter")?,
            filament_profiles: required_string_array4(path, &settings, "filament_settings_id")?,
            filament_setting_ids: required_string_array4(path, &settings, "filament_ids")?,
            filament_colors: required_string_array4(path, &settings, "filament_colour")?,
            default_filament_colors: required_string_array4(
                path,
                &settings,
                "default_filament_colour",
            )?,
            filament_materials: required_string_array4(path, &settings, "filament_type")?,
        },
        definitions,
        process_globals: selected_settings(&settings, PROCESS_GLOBAL_KEYS),
        prime_tower: selected_settings(&settings, PRIME_TOWER_KEYS),
    })
}

fn required_string(
    path: &Path,
    settings: &BTreeMap<String, Value>,
    key: &str,
) -> Result<String, U1FullSpectrumGuiRoundTripError> {
    settings
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| U1FullSpectrumGuiRoundTripError::Semantic {
            path: path.to_path_buf(),
            message: format!("project setting {key:?} is missing or is not a string"),
        })
}

fn required_string_array4(
    path: &Path,
    settings: &BTreeMap<String, Value>,
    key: &str,
) -> Result<[String; 4], U1FullSpectrumGuiRoundTripError> {
    let values = settings.get(key).and_then(Value::as_array).ok_or_else(|| {
        U1FullSpectrumGuiRoundTripError::Semantic {
            path: path.to_path_buf(),
            message: format!("project setting {key:?} is missing or is not an array"),
        }
    })?;
    if values.len() != 4 {
        return Err(U1FullSpectrumGuiRoundTripError::Semantic {
            path: path.to_path_buf(),
            message: format!("project setting {key:?} must contain exactly four values"),
        });
    }
    values
        .iter()
        .map(|value| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                U1FullSpectrumGuiRoundTripError::Semantic {
                    path: path.to_path_buf(),
                    message: format!("project setting {key:?} contains a non-string value"),
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| U1FullSpectrumGuiRoundTripError::Semantic {
            path: path.to_path_buf(),
            message: format!("project setting {key:?} could not be normalized"),
        })
}

fn selected_settings(settings: &BTreeMap<String, Value>, keys: &[&str]) -> BTreeMap<String, Value> {
    keys.iter()
        .map(|key| {
            (
                (*key).to_owned(),
                settings.get(*key).cloned().unwrap_or(Value::Null),
            )
        })
        .collect()
}

fn read_named_entry(
    package_path: &Path,
    entry_name: &'static str,
    limit: u64,
) -> Result<Vec<u8>, U1FullSpectrumGuiRoundTripError> {
    let file =
        File::open(package_path).map_err(|source| U1FullSpectrumGuiRoundTripError::Open {
            path: package_path.to_path_buf(),
            source,
        })?;
    let mut archive =
        ZipArchive::new(file).map_err(|source| U1FullSpectrumGuiRoundTripError::Zip {
            path: package_path.to_path_buf(),
            source,
        })?;
    let entry = match archive.by_name(entry_name) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => {
            return Err(U1FullSpectrumGuiRoundTripError::MissingEntry {
                path: package_path.to_path_buf(),
                entry: entry_name,
            });
        }
        Err(source) => {
            return Err(U1FullSpectrumGuiRoundTripError::Zip {
                path: package_path.to_path_buf(),
                source,
            });
        }
    };
    if entry.size() > limit {
        return Err(U1FullSpectrumGuiRoundTripError::EntryTooLarge {
            path: package_path.to_path_buf(),
            entry: entry_name,
            limit,
        });
    }
    let mut bytes = Vec::with_capacity(usize::try_from(entry.size()).unwrap_or(0));
    entry
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| U1FullSpectrumGuiRoundTripError::Open {
            path: package_path.to_path_buf(),
            source,
        })?;
    if bytes.len() as u64 > limit {
        return Err(U1FullSpectrumGuiRoundTripError::EntryTooLarge {
            path: package_path.to_path_buf(),
            entry: entry_name,
            limit,
        });
    }
    Ok(bytes)
}

fn printable_filament_ids(analysis: &ProjectAnalysis) -> Vec<u8> {
    let printable_object_ids = analysis
        .plates
        .iter()
        .flat_map(|plate| &plate.instances)
        .filter(|instance| instance.printable)
        .map(|instance| instance.object_id)
        .collect::<BTreeSet<_>>();
    analysis
        .objects
        .iter()
        .filter(|object| printable_object_ids.contains(&object.id))
        .flat_map(|object| {
            let part_slots = object
                .parts
                .iter()
                .filter(|part| part.printable)
                .flat_map(|part| part.effective_slots.iter().copied())
                .collect::<Vec<_>>();
            if object.parts.is_empty() {
                object.effective_slots.clone()
            } else {
                part_slots
            }
        })
        .filter_map(|id| u8::try_from(id).ok())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn is_canonical_rgb_hex(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value[1..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(&byte))
}

fn all_equal<T: PartialEq>(values: &[T]) -> bool {
    values
        .first()
        .is_some_and(|first| values.iter().skip(1).all(|value| value == first))
}

fn all_equal_by<T, U: PartialEq + ?Sized>(values: &[T], select: impl Fn(&T) -> &U) -> bool {
    values.first().is_some_and(|first| {
        let first = select(first);
        values.iter().skip(1).all(|value| select(value) == first)
    })
}

fn process_globals_are_round_trip_stable(contracts: &[FullSpectrumProjectContract]) -> bool {
    let Some(candidate) = contracts.first() else {
        return false;
    };
    if contracts.len() != 3 {
        return false;
    }

    let ordinary_keys_stable = PROCESS_GLOBAL_KEYS
        .iter()
        .filter(|key| !GUI_CANONICALIZED_DITHERING_KEYS.contains(key))
        .all(|key| {
            let expected = candidate.process_globals.get(*key);
            contracts
                .iter()
                .skip(1)
                .all(|contract| contract.process_globals.get(*key) == expected)
        });
    if !ordinary_keys_stable {
        return false;
    }

    GUI_CANONICALIZED_DITHERING_KEYS.iter().all(|key| {
        let values = contracts
            .iter()
            .map(|contract| contract.process_globals.get(*key).and_then(Value::as_str))
            .collect::<Vec<_>>();
        values.iter().all(|value| *value == values[0])
            || (values[0] == Some("1") && values[1..].iter().all(|value| *value == Some("0")))
    })
}

fn prime_tower_contracts_are_round_trip_stable(contracts: &[FullSpectrumProjectContract]) -> bool {
    let Some(candidate) = contracts.first() else {
        return false;
    };
    if contracts.len() != 3 {
        return false;
    }
    PRIME_TOWER_KEYS.iter().all(|key| {
        let normalize = |value: Option<&Value>| {
            if *key == "wipe_tower_rotation_angle"
                && (value.is_none() || value == Some(&Value::Null))
            {
                Some(Value::String("0".to_owned()))
            } else {
                value.cloned()
            }
        };
        let expected = normalize(candidate.prime_tower.get(*key));
        contracts
            .iter()
            .skip(1)
            .all(|contract| normalize(contract.prime_tower.get(*key)) == expected)
    })
}

fn empty_checks(
    structural: &[OutputValidationReport],
    contract_valid: &[bool],
    no_embedded_presets: bool,
) -> U1FullSpectrumGuiRoundTripChecks {
    U1FullSpectrumGuiRoundTripChecks {
        candidate_structurally_valid: structural[0].is_valid,
        first_gui_save_structurally_valid: structural[1].is_valid,
        reopened_gui_save_structurally_valid: structural[2].is_valid,
        every_artifact_has_exact_full_spectrum_contract: contract_valid.iter().all(|valid| *valid),
        candidate_has_exact_physical_cmyx_identity: false,
        physical_t1_t4_identity_stable: false,
        candidate_has_mixed_definition: false,
        mixed_definitions_stable: false,
        candidate_has_virtual_assignment: false,
        virtual_assignments_stable: false,
        candidate_has_solid_t4_assignment: false,
        solid_t4_assignment_stable: false,
        subdivision_and_process_globals_stable: false,
        candidate_prime_tower_contract_active: false,
        prime_tower_contract_stable: false,
        no_embedded_presets,
        process_local_ids_valid: false,
        geometry_and_parts_stable: false,
        placements_and_plate_membership_stable: false,
    }
}

fn build_artifact_summaries(
    roles: [U1FullSpectrumGuiArtifactRole; 3],
    analyses: Vec<ProjectAnalysis>,
    structural: Vec<OutputValidationReport>,
    validations: Vec<U1FullSpectrumValidationReport>,
    contract_valid: Vec<bool>,
    contracts: &[FullSpectrumProjectContract],
    models: &[U1SemanticModelSnapshot],
) -> Result<Vec<U1FullSpectrumGuiArtifactSummary>, U1FullSpectrumGuiRoundTripError> {
    roles
        .into_iter()
        .zip(analyses)
        .zip(structural)
        .zip(validations)
        .zip(contract_valid)
        .enumerate()
        .map(
            |(
                index,
                ((((role, analysis), structural_validation), full_spectrum_validation), valid),
            )| {
                let semantic_sha256 = contracts
                    .get(index)
                    .zip(models.get(index))
                    .map(|(contract, model)| {
                        digest_serializable(&(contract, &model.semantic_sha256))
                    })
                    .transpose()?;
                Ok(U1FullSpectrumGuiArtifactSummary {
                    role,
                    byte_size: analysis.input.byte_size,
                    sha256: analysis.input.sha256.clone(),
                    plate_count: analysis.summary.plate_count,
                    object_count: analysis.summary.object_count,
                    instance_count: analysis.summary.instance_count,
                    part_count: analysis.summary.part_count,
                    geometry_resource_count: models
                        .get(index)
                        .map_or(0, |model| model.geometry_resource_count),
                    used_filament_ids: printable_filament_ids(&analysis),
                    virtual_filament_count: contracts.get(index).map_or(
                        full_spectrum_validation.virtual_filament_count,
                        |contract| contract.definitions.len(),
                    ),
                    full_spectrum_contract_valid: valid,
                    semantic_sha256,
                    structural_validation,
                    full_spectrum_validation,
                })
            },
        )
        .collect()
}

fn digest_serializable(value: &impl Serialize) -> Result<String, U1FullSpectrumGuiRoundTripError> {
    let bytes =
        serde_json::to_vec(value).map_err(|source| U1FullSpectrumGuiRoundTripError::Semantic {
            path: PathBuf::from("<semantic-comparison>"),
            message: format!("failed to serialize semantic fingerprint: {source}"),
        })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::U1FullSpectrumValidationIssue;
    use serde_json::json;
    use std::io::Write as _;
    use tempfile::tempdir;
    use u1_three_mf::{PaintNode, encode_paint_annotation};
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

    fn qualification_settings(definition: &str) -> Vec<u8> {
        let flush_matrix = (0..4)
            .flat_map(|source| (0..4).map(move |target| if source == target { "0" } else { "140" }))
            .collect::<Vec<_>>();
        let mut settings = BTreeMap::new();
        for (key, value) in [
            ("name", "project_settings"),
            ("from", "project"),
            ("version", "2.3.5"),
            ("printer_model", "Snapmaker U1"),
            ("printer_variant", "0.4"),
            ("printer_settings_id", "Snapmaker U1 (0.4 nozzle)"),
            (
                "print_settings_id",
                "0.08 Extra Fine @Snapmaker U1 (0.4 nozzle)",
            ),
            ("layer_height", "0.08"),
            ("flush_multiplier", "1"),
            ("mixed_filament_definitions", definition),
            ("mixed_color_layer_height_a", "0"),
            ("mixed_color_layer_height_b", "0"),
            ("mixed_filament_gradient_mode", "0"),
            ("mixed_filament_height_lower_bound", "0.02"),
            ("mixed_filament_height_upper_bound", "0.02"),
            ("mixed_filament_advanced_dithering", "0"),
            ("mixed_filament_pointillism_pixel_size", "0"),
            ("mixed_filament_pointillism_line_gap", "0"),
            ("mixed_filament_component_bias_enabled", "0"),
            ("mixed_filament_surface_indentation", "0"),
            ("mixed_filament_region_collapse", "1"),
            ("dithering_z_step_size", "0"),
            ("dithering_local_z_mode", "1"),
            ("dithering_local_z_whole_objects", "1"),
            ("dithering_local_z_infill", "1"),
            ("dithering_local_z_direct_multicolor", "0"),
            ("dithering_step_painted_zones_only", "1"),
            ("curr_bed_type", "Textured PEI Plate"),
            ("brim_type", "auto_brim"),
            ("print_sequence", "by layer"),
            ("other_layers_print_sequence_nums", "0"),
            ("spiral_mode", "0"),
            ("spiral_mode_smooth", "0"),
            ("spiral_mode_max_xy_smoothing", "200%"),
            ("enable_prime_tower", "1"),
            ("prime_tower_width", "30"),
            ("prime_volume", "18"),
            ("prime_tower_brim_width", "5"),
            ("wipe_tower_cone_angle", "15"),
            ("wipe_tower_extra_spacing", "120%"),
            ("wipe_tower_extra_rib_length", "8"),
            ("wipe_tower_wall_type", "rib"),
            ("wipe_tower_filament", "0"),
        ] {
            settings.insert(key, Value::String(value.to_owned()));
        }
        settings.insert("nozzle_diameter", json!(["0.4", "0.4", "0.4", "0.4"]));
        settings.insert(
            "filament_settings_id",
            json!([
                "Snapmaker PLA Full Spectrum @U1 0.4 nozzle",
                "Snapmaker PLA Full Spectrum @U1 0.4 nozzle",
                "Snapmaker PLA Full Spectrum @U1 0.4 nozzle",
                "Snapmaker PLA Full Spectrum @U1 0.4 nozzle"
            ]),
        );
        settings.insert(
            "filament_ids",
            json!([
                "1195313935011",
                "1195313935011",
                "1195313935011",
                "1195313935011"
            ]),
        );
        settings.insert(
            "filament_colour",
            json!(["#08ABFB", "#D93B90", "#F9ED3D", "#9199A4"]),
        );
        settings.insert(
            "default_filament_colour",
            json!(["#08ABFB", "#D93B90", "#F9ED3D", "#9199A4"]),
        );
        settings.insert("filament_type", json!(["PLA", "PLA", "PLA", "PLA"]));
        settings.insert("flush_volumes_matrix", json!(flush_matrix));
        settings.insert(
            "flush_volumes_vector",
            json!(["140", "140", "140", "140", "140", "140", "140", "140"]),
        );
        settings.insert("first_layer_print_sequence", json!(["0"]));
        settings.insert("other_layers_print_sequence", json!(["0"]));
        settings.insert("wipe_tower_x", json!(["205.9"]));
        settings.insert("wipe_tower_y", json!(["198.9"]));
        serde_json::to_vec_pretty(&settings).unwrap()
    }

    fn qualification_model(vertex_x: f64, paint_state: Option<u8>) -> String {
        let paint_attribute = paint_state.map_or_else(String::new, |state| {
            let encoded = encode_paint_annotation(&PaintNode::Leaf { state }).unwrap();
            format!(r#" paint_color="{encoded}""#)
        });
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" unit="millimeter">
 <metadata name="Application">Snapmaker Orca 2.3.5</metadata>
 <resources>
  <object id="1" type="model"><mesh><vertices>
   <vertex x="0" y="0" z="0"/><vertex x="{vertex_x}" y="0" z="0"/><vertex x="0" y="10" z="0"/>
  </vertices><triangles><triangle v1="0" v2="1" v3="2"{paint_attribute}/></triangles></mesh></object>
  <object id="2" type="model"><mesh><vertices>
   <vertex x="0" y="0" z="0"/><vertex x="8" y="0" z="0"/><vertex x="0" y="8" z="0"/>
  </vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object>
 </resources>
 <build>
  <item objectid="1" transform="1 0 0 0 1 0 0 0 1 20 30 1" printable="1"/>
  <item objectid="2" transform="1 0 0 0 1 0 0 0 1 60 30 1" printable="1"/>
 </build>
</model>"#
        )
    }

    fn qualification_model_settings(first_id: u64, second_id: u64) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
 <object id="1"><metadata key="name" value="Mixed region"/><metadata key="extruder" value="5"/></object>
 <object id="2"><metadata key="name" value="Solid T4 region"/><metadata key="extruder" value="4"/></object>
 <plate>
  <metadata key="plater_id" value="1"/><metadata key="plater_name" value="Qualification plate"/>
  <metadata key="filament_map_mode" value="Auto For Flush"/><metadata key="filament_maps" value="1 1 1 1"/>
  <metadata key="filament_volume_maps" value="0 0 0 0"/>
  <model_instance><metadata key="object_id" value="1"/><metadata key="instance_id" value="0"/><metadata key="identify_id" value="{first_id}"/></model_instance>
  <model_instance><metadata key="object_id" value="2"/><metadata key="instance_id" value="0"/><metadata key="identify_id" value="{second_id}"/></model_instance>
 </plate>
</config>"#
        )
    }

    fn write_qualification_archive(
        path: &Path,
        definition: &str,
        vertex_x: f64,
        ids: (u64, u64),
        paint_state: Option<u8>,
    ) {
        let file = File::create(path).unwrap();
        let mut writer = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let entries = [
            ("[Content_Types].xml", CONTENT_TYPES.as_bytes().to_vec()),
            ("_rels/.rels", ROOT_RELATIONSHIPS.as_bytes().to_vec()),
            (
                "3D/3dmodel.model",
                qualification_model(vertex_x, paint_state).into_bytes(),
            ),
            (PROJECT_SETTINGS_PATH, qualification_settings(definition)),
            (
                "Metadata/model_settings.config",
                qualification_model_settings(ids.0, ids.1).into_bytes(),
            ),
        ];
        for (name, bytes) in entries {
            writer.start_file(name, options).unwrap();
            writer.write_all(&bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    fn validation_with(code: U1FullSpectrumValidationCode) -> U1FullSpectrumValidationReport {
        U1FullSpectrumValidationReport {
            adapter_id: U1_FULL_SPECTRUM_ADAPTER_ID.to_owned(),
            valid: false,
            physical_filament_count: 4,
            virtual_filament_count: 1,
            used_filament_ids: vec![4, 5],
            recipe_calibration_sample_ids: Vec::new(),
            issues: vec![U1FullSpectrumValidationIssue {
                severity: U1FullSpectrumValidationSeverity::Error,
                code,
                path: Some("Metadata/slice_info.config".to_owned()),
                message: "fixture issue".to_owned(),
            }],
        }
    }

    #[test]
    fn only_gui_roles_may_ignore_candidate_stale_markers() {
        let report = validation_with(U1FullSpectrumValidationCode::StaleArtifact);
        assert!(!full_spectrum_contract_is_acceptable(
            U1FullSpectrumGuiArtifactRole::WriterCandidate,
            &report
        ));
        assert!(full_spectrum_contract_is_acceptable(
            U1FullSpectrumGuiArtifactRole::FirstGuiSave,
            &report
        ));
        assert!(full_spectrum_contract_is_acceptable(
            U1FullSpectrumGuiArtifactRole::ReopenedGuiSave,
            &report
        ));
    }

    #[test]
    fn gui_roles_never_ignore_non_stale_contract_failures() {
        let report = validation_with(U1FullSpectrumValidationCode::InvalidMixedDefinition);
        assert!(!full_spectrum_contract_is_acceptable(
            U1FullSpectrumGuiArtifactRole::FirstGuiSave,
            &report
        ));
    }

    #[test]
    fn gui_roles_only_allow_the_observed_candidate_validator_false_positives() {
        let mut assignment = validation_with(U1FullSpectrumValidationCode::AssignmentOutOfRange);
        assignment.issues[0].path = None;
        assignment.issues[0].message = "geometry references filament 5, outside 1..=4".to_owned();
        assert!(full_spectrum_contract_is_acceptable(
            U1FullSpectrumGuiArtifactRole::FirstGuiSave,
            &assignment
        ));

        let mut canonicalized =
            validation_with(U1FullSpectrumValidationCode::InvalidProjectSettings);
        canonicalized.issues[0].path = Some(PROJECT_SETTINGS_PATH.to_owned());
        canonicalized.issues[0].message = "generated Full Spectrum candidate failed semantic validation: project setting dithering_local_z_mode is \"0\"; expected \"1\"".to_owned();
        assert!(full_spectrum_contract_is_acceptable(
            U1FullSpectrumGuiArtifactRole::ReopenedGuiSave,
            &canonicalized
        ));

        canonicalized.issues[0].message = "some other project setting changed".to_owned();
        assert!(!full_spectrum_contract_is_acceptable(
            U1FullSpectrumGuiArtifactRole::ReopenedGuiSave,
            &canonicalized
        ));
    }

    #[test]
    fn physical_identity_requires_exact_cmy_and_canonical_t4() {
        let identity = PhysicalT1T4Identity {
            version: SUPPORTED_ORCA_VERSION.to_owned(),
            printer_model: "Snapmaker U1".to_owned(),
            printer_variant: "0.4".to_owned(),
            machine_profile: U1_MACHINE_PROFILE_NAME.to_owned(),
            process_profile: FULL_SPECTRUM_PROCESS_PROFILE_NAME.to_owned(),
            nozzle_diameters: std::array::from_fn(|_| "0.4".to_owned()),
            filament_profiles: std::array::from_fn(|_| FULL_SPECTRUM_PROFILE_NAME.to_owned()),
            filament_setting_ids: std::array::from_fn(|_| FULL_SPECTRUM_SETTING_ID.to_owned()),
            filament_colors: [
                "#08ABFB".to_owned(),
                "#D93B90".to_owned(),
                "#F9ED3D".to_owned(),
                "#9199A4".to_owned(),
            ],
            default_filament_colors: [
                "#08ABFB".to_owned(),
                "#D93B90".to_owned(),
                "#F9ED3D".to_owned(),
                "#9199A4".to_owned(),
            ],
            filament_materials: std::array::from_fn(|_| "PLA".to_owned()),
        };
        assert!(identity.is_exact_cmyx());

        let mut heterogeneous_t4 = identity.clone();
        heterogeneous_t4.filament_profiles[3] = "Polymaker General PLA Family @U1".to_owned();
        heterogeneous_t4.filament_setting_ids[3] = "POLY_GENERAL_PLA_U1_001".to_owned();
        heterogeneous_t4.filament_colors[3] = "#080A0D".to_owned();
        heterogeneous_t4.default_filament_colors[3] = "#080A0D".to_owned();
        assert!(heterogeneous_t4.is_exact_cmyx());

        let mut gui_saved = heterogeneous_t4.clone();
        gui_saved.default_filament_colors = std::array::from_fn(|_| String::new());
        assert!(gui_saved.has_same_physical_loadout(&heterogeneous_t4));
        assert!(gui_saved.has_expected_gui_default_color_cache(&heterogeneous_t4));
        assert!(!gui_saved.is_exact_cmyx());

        let mut changed = identity.clone();
        changed.filament_colors[1] = "#000000".to_owned();
        assert!(!changed.is_exact_cmyx());

        let mut invalid_t4 = identity;
        invalid_t4.filament_colors[3] = "#9199a4".to_owned();
        invalid_t4.default_filament_colors[3] = "#9199a4".to_owned();
        assert!(!invalid_t4.is_exact_cmyx());
    }

    #[test]
    fn process_globals_allow_only_the_observed_gui_dithering_reset() {
        let physical = PhysicalT1T4Identity {
            version: SUPPORTED_ORCA_VERSION.to_owned(),
            printer_model: "Snapmaker U1".to_owned(),
            printer_variant: "0.4".to_owned(),
            machine_profile: U1_MACHINE_PROFILE_NAME.to_owned(),
            process_profile: FULL_SPECTRUM_PROCESS_PROFILE_NAME.to_owned(),
            nozzle_diameters: std::array::from_fn(|_| "0.4".to_owned()),
            filament_profiles: std::array::from_fn(|_| FULL_SPECTRUM_PROFILE_NAME.to_owned()),
            filament_setting_ids: std::array::from_fn(|_| FULL_SPECTRUM_SETTING_ID.to_owned()),
            filament_colors: std::array::from_fn(|_| "#080A0D".to_owned()),
            default_filament_colors: std::array::from_fn(|_| "#080A0D".to_owned()),
            filament_materials: std::array::from_fn(|_| "PLA".to_owned()),
        };
        let mut candidate_globals = PROCESS_GLOBAL_KEYS
            .iter()
            .map(|key| ((*key).to_owned(), Value::String("stable".to_owned())))
            .collect::<BTreeMap<_, _>>();
        for key in GUI_CANONICALIZED_DITHERING_KEYS {
            candidate_globals.insert((*key).to_owned(), Value::String("1".to_owned()));
        }
        let mut gui_globals = candidate_globals.clone();
        for key in GUI_CANONICALIZED_DITHERING_KEYS {
            gui_globals.insert((*key).to_owned(), Value::String("0".to_owned()));
        }
        let contract = |process_globals| FullSpectrumProjectContract {
            physical: physical.clone(),
            definitions: Vec::new(),
            process_globals,
            prime_tower: BTreeMap::new(),
        };
        let mut contracts = vec![
            contract(candidate_globals),
            contract(gui_globals.clone()),
            contract(gui_globals),
        ];
        assert!(process_globals_are_round_trip_stable(&contracts));

        contracts[2]
            .process_globals
            .insert("layer_height".to_owned(), Value::String("0.20".to_owned()));
        assert!(!process_globals_are_round_trip_stable(&contracts));
    }

    #[test]
    fn accepts_semantic_round_trip_with_process_local_id_renumbering() {
        let directory = tempdir().unwrap();
        let candidate = directory.path().join("candidate.3mf");
        let first = directory.path().join("first.3mf");
        let reopened = directory.path().join("reopened.3mf");
        let definition = "1,2,1,1,50,0,g,w,m2,z4,xa0,xb0,d0,o0,u1,cm0,12";
        write_qualification_archive(&candidate, definition, 10.0, (101, 102), None);
        write_qualification_archive(&first, definition, 10.0, (201, 202), None);
        write_qualification_archive(&reopened, definition, 10.0, (301, 302), None);

        let report =
            validate_u1_full_spectrum_gui_round_trip(&candidate, &first, &reopened).unwrap();

        assert!(report.is_valid, "{:#?}", report.issues);
        assert!(report.checks.candidate_has_virtual_assignment);
        assert!(report.checks.candidate_has_solid_t4_assignment);
        assert!(report.checks.process_local_ids_valid);
        assert!(report.checks.geometry_and_parts_stable);
        assert!(
            report
                .artifacts
                .iter()
                .all(|artifact| artifact.semantic_sha256.is_some())
        );
    }

    #[test]
    fn rejects_definition_and_geometry_drift_even_when_each_file_is_valid() {
        let directory = tempdir().unwrap();
        let candidate = directory.path().join("candidate.3mf");
        let first = directory.path().join("first.3mf");
        let reopened = directory.path().join("reopened.3mf");
        let definition = "1,2,1,1,50,0,g,w,m2,z4,xa0,xb0,d0,o0,u1,cm0,12";
        let changed_definition = "1,2,1,1,51,0,g,w,m2,z4,xa0,xb0,d0,o0,u1,cm0,12";
        write_qualification_archive(&candidate, definition, 10.0, (101, 102), None);
        write_qualification_archive(&first, changed_definition, 10.0, (201, 202), None);
        write_qualification_archive(&reopened, changed_definition, 11.0, (301, 302), None);

        let report =
            validate_u1_full_spectrum_gui_round_trip(&candidate, &first, &reopened).unwrap();

        assert!(!report.is_valid);
        assert!(!report.checks.mixed_definitions_stable);
        assert!(!report.checks.geometry_and_parts_stable);
        assert!(report.issues.iter().any(|issue| {
            issue.code == U1FullSpectrumGuiRoundTripIssueCode::MixedDefinitionsChanged
        }));
        assert!(report.issues.iter().any(|issue| {
            issue.code == U1FullSpectrumGuiRoundTripIssueCode::GeometryOrPartChanged
        }));
    }

    #[test]
    fn rejects_decoded_per_facet_virtual_assignment_drift() {
        let directory = tempdir().unwrap();
        let candidate = directory.path().join("candidate.3mf");
        let first = directory.path().join("first.3mf");
        let reopened = directory.path().join("reopened.3mf");
        let definition = "1,2,1,1,50,0,g,w,m2,z4,xa0,xb0,d0,o0,u1,cm0,12";
        write_qualification_archive(&candidate, definition, 10.0, (101, 102), Some(5));
        write_qualification_archive(&first, definition, 10.0, (201, 202), Some(4));
        write_qualification_archive(&reopened, definition, 10.0, (301, 302), Some(4));

        let report =
            validate_u1_full_spectrum_gui_round_trip(&candidate, &first, &reopened).unwrap();

        assert!(!report.is_valid);
        assert!(!report.checks.virtual_assignments_stable);
        assert!(report.issues.iter().any(|issue| {
            issue.code == U1FullSpectrumGuiRoundTripIssueCode::VirtualAssignmentsChanged
        }));
    }
}
