//! Clean-room Snapmaker Orca 2.3.6 Full Spectrum metadata adapter.
//!
//! This module owns the versioned virtual-filament contract only. Geometry
//! selection and translation are intentionally supplied by the common 3MF
//! writer. The native writer is enabled only for its exact, hash-bound GUI
//! round-trip evidence. Physical color accuracy remains a separate per-user
//! calibration and explicit-approval concern.

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, Writer};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Seek};
use std::path::{Path, PathBuf};
use thiserror::Error;
use u1_planner::{
    CmyxRecipe, ColorConfidence, ColorStrategy, DedicatedSupportMaterial, FullSpectrumMode,
    FullSpectrumProcessCompatibility, FullSpectrumSubdivisionPolicy, Material, PackingStatus,
    PlannedBatch, PlannedJob, PlannedPlate, PlanningInput, PlanningResult, Printer, PrinterLoadout,
    RgbColor, ScopedUnitRef, SourceToActualMapping, Spool, SupportMaterialUsage, Toolhead,
    ToolheadSlotState, U1Loadout,
};
use u1_three_mf::{
    AnalysisLimits, ExpectedSourceIdentity, OpcPackageWriter, StagedPackageValidationError,
    StaleArtifactPolicy, SupportInformation, ValidatedStagedPackage, analyze_project_with_limits,
    decode_paint_annotation,
};
use zip::ZipArchive;

use crate::pva_profile::{
    RELI3D_PVA_FILAMENT_ID, RELI3D_PVA_PROFILE_NAME, RELI3D_PVA_SETTING_ID,
    apply_reli3d_pva_profile,
};
use crate::{
    Compatibility, FULL_SPECTRUM_FILAMENT_BASE_SHA256, FULL_SPECTRUM_PROCESS_BASE_SHA256,
    FULL_SPECTRUM_PROCESS_PROFILE_NAME, FULL_SPECTRUM_PROCESS_PROFILE_SHA256,
    FULL_SPECTRUM_PROCESS_SETTING_ID, FULL_SPECTRUM_PROFILE_NAME, FULL_SPECTRUM_PROFILE_SHA256,
    FULL_SPECTRUM_SETTING_ID, InstallationReport, KLIPPER_MACHINE_BASE_SHA256,
    SUPPORTED_ORCA_VERSION, TOOLCHANGER_MACHINE_BASE_SHA256, U1_MACHINE_BASE_SHA256,
    U1_MACHINE_PROFILE_NAME, U1_MACHINE_PROFILE_SHA256, U1_MACHINE_SETTING_ID,
    U1_PROCESS_BASE_SHA256, U1_PROCESS_COMMON_SHA256, inspect_macos_application,
};

pub const U1_FULL_SPECTRUM_ADAPTER_ID: &str = "snapmaker-orca/2.3.6/u1-0.4-full-spectrum";
pub const U1_FULL_SPECTRUM_SCHEMA_VERSION: u32 = 1;
pub const U1_FULL_SPECTRUM_PHYSICAL_COUNT: u8 = 4;
pub const U1_FULL_SPECTRUM_FIRST_VIRTUAL_ID: u8 = 5;
pub const U1_FULL_SPECTRUM_MAX_FILAMENT_ID: u8 = 32;
pub const U1_FULL_SPECTRUM_CANCELLATION_ERROR_PREFIX: &str =
    "Snapmaker U1 Full Spectrum conversion was cancelled";

const U1_FULL_SPECTRUM_EXECUTABLE_SHA256: &str =
    "553a02a813e031ef2ea49616dc0edb9d0a4f0cc077b99c70fc30d5060dea3658";
const U1_FULL_SPECTRUM_QUALIFICATION_FIXTURE: &str = "Withered_Foxy.3mf";
const U1_FULL_SPECTRUM_QUALIFICATION_FIXTURE_SHA256: &str =
    "f0ad280964c0f0a3bdae1c3b8cc187d572da2986010f6e2c08e4e803db846b81";
const U1_FULL_SPECTRUM_WRITER_QUALIFICATION_SCOPE: &str =
    "native_project_structure_and_gui_round_trip";
const U1_FULL_SPECTRUM_RECIPE_ACCURACY_POLICY: &str =
    "per_user_measured_calibration_or_explicit_color_approval";
const U1_FULL_SPECTRUM_QUALIFICATION_RECORD: &[u8] =
    include_bytes!("../qualification/u1-full-spectrum-2.3.6.json");
const U1_FULL_SPECTRUM_QUALIFICATION_REPORT: &[u8] =
    include_bytes!("../qualification/u1-full-spectrum-2.3.6-report.json");

const PROJECT_SETTINGS_PATH: &str = "Metadata/project_settings.config";
const MODEL_SETTINGS_PATH: &str = "Metadata/model_settings.config";
const LAYER_HEIGHT_MICRONS: u32 = 80;
const SUBDIVISION_FACTOR: u8 = 4;
const SUBLAYER_HEIGHT_MICRONS: u32 = 20;
const PRIME_TOWER_WIDTH_MM: f64 = 30.0;
const PRIME_TOWER_DEPTH_MM: f64 = 45.0;
const PRIME_TOWER_VOLUME_MM3: f64 = 18.0;
const PRIME_TOWER_BRIM_MM: f64 = 5.0;
const PRIME_TOWER_CONE_ANGLE_DEGREES: f64 = 15.0;
const PRIME_TOWER_RIB_LENGTH_MM: f64 = 8.0;
const PRIME_TOWER_EXTRA_SPACING_PERCENT: f64 = 120.0;
// At the qualified 270 mm print height the 15 degree stabilization cone has a
// 35.55 mm half-width. The planner adds the 5 mm brim, 8 mm rib and 1 mm object
// clearance, rounded upward to the same conservative 49.6 mm half-extent.
const PRIME_TOWER_MAX_HALF_EXTENT_MM: f64 = 49.6;
const GEOMETRY_EPSILON_MM: f64 = 1.0e-6;
const U1_MIN_X_MM: f64 = 0.5;
const U1_MAX_X_MM: f64 = 270.5;
const U1_MIN_Y_MM: f64 = 1.0;
const U1_MAX_Y_MM: f64 = 271.0;
const MAX_CONFIG_BYTES: u64 = 64 * 1024 * 1024;
const COPY_BUFFER_BYTES: usize = 256 * 1024;
const XML_CANCELLATION_INTERVAL_EVENTS: usize = 256;

const FULL_SPECTRUM_PROFILE_PATH: &str = "filament/Snapmaker PLA Full Spectrum @U1 0.4 nozzle.json";
const FULL_SPECTRUM_FILAMENT_BASE_PATH: &str = "filament/Snapmaker PLA Basic @U1 base.json";
const FULL_SPECTRUM_FILAMENT_PROFILE_CHAIN: &[(&str, &str)] = &[
    (
        FULL_SPECTRUM_FILAMENT_BASE_PATH,
        FULL_SPECTRUM_FILAMENT_BASE_SHA256,
    ),
    (FULL_SPECTRUM_PROFILE_PATH, FULL_SPECTRUM_PROFILE_SHA256),
];
const GENERIC_PLA_PROFILE_NAME: &str = "Generic PLA";
const GENERIC_PLA_SETTING_ID: &str = "GFSL991";
const GENERIC_PLA_FILAMENT_ID: &str = "GFL9922";
const POLYMAKER_PLA_PROFILE_NAME: &str = "Polymaker General PLA Family @U1";
const POLYMAKER_PLA_SETTING_ID: &str = "POLY_GENERAL_PLA_U1_001";
const POLYMAKER_PLA_FILAMENT_ID: &str = "OGFL99";
const SNAPMAKER_PVA_PROFILE_NAME: &str = "Snapmaker PVA @U1";
const SNAPMAKER_PVA_SETTING_ID: &str = "41452139080";
const SNAPMAKER_PVA_FILAMENT_ID: &str = "31046369800";
const GENERIC_PLA_PROFILE_CHAIN: &[(&str, &str)] = &[
    (
        "filament/fdm_filament_common_generic.json",
        "f2e3be440a33d2642a84d0b4feb13359c3f27950b28db3288096ea897fa655c4",
    ),
    (
        "filament/fdm_filament_pla_generic.json",
        "d3cb6456d9a15923d732d5ba0ceaaa7ebeeb6b569cbe49bb13383fdbf425f90c",
    ),
    (
        "filament/Generic PLA @base.json",
        "b429b410382b1b9aff795ccb1c9b38c678cb230145a9a524093be678f2ef12e1",
    ),
    (
        "filament/Generic PLA.json",
        "1b67ca20832ec5f72e90d8f5ec76e17bc1c150e45fed1d833412310e8a4378ab",
    ),
];
const POLYMAKER_PLA_PROFILE_CHAIN: &[(&str, &str)] = &[
    (
        "filament/fdm_filament_common_poly.json",
        "e2982eef7fea0fb2656d28d5543e81b9bf8305d5edce22075edb796efb0896de",
    ),
    (
        "filament/fdm_filament_pla_poly.json",
        "082364ea8040b473480fb2563a1ca1648b2dd12d8770e01811551f58a01bda81",
    ),
    (
        "filament/Polymaker PLA @U1 base.json",
        "c8cfe138966bffcbde9bc881e31b13524aa30f0f08a0a58f1760185c3ca32671",
    ),
    (
        "filament/Polymaker General PLA Family @U1.json",
        "32e3cc2b753288acdb5199596e10c0a9e72c5c7aa865c0bfbfcbe59fad8c4ce0",
    ),
];
const SNAPMAKER_PVA_PROFILE_CHAIN: &[(&str, &str)] = &[
    (
        "filament/fdm_filament_common.json",
        "d01d59fe7ae9b999d78c923725aad1d50238c5556c6664a18c3db09555217ece",
    ),
    (
        "filament/fdm_filament_pva.json",
        "ee8b784ceefc21533d9d162ff8d1568097615f4adc6be54562ce8ad2ca6d92a3",
    ),
    (
        "filament/Snapmaker PVA @U1 base.json",
        "3041a49ac415fca9af7d9be452c436b70e6da8218f5a55e3e66db73a04a1341b",
    ),
    (
        "filament/Snapmaker PVA @U1.json",
        "05179ef3dbd0c8ddb722fef39c8c5e4e851a08b2fca86108984dcf50e23ac5e7",
    ),
];
const MACHINE_PROFILE_PATH: &str = "machine/Snapmaker U1 (0.4 nozzle).json";
const MACHINE_U1_PATH: &str = "machine/fdm_U1.json";
const MACHINE_TOOLCHANGER_PATH: &str = "machine/fdm_toolchanger.json";
const MACHINE_KLIPPER_PATH: &str = "machine/fdm_klipper.json";
const PROCESS_PROFILE_PATH: &str = "process/0.08 Extra Fine @Snapmaker U1 (0.4 nozzle).json";
const PROCESS_008_PATH: &str = "process/fdm_process_U1_0.08.json";
const PROCESS_COMMON_PATH: &str = "process/fdm_process_U1_common.json";
const PROCESS_U1_PATH: &str = "process/fdm_process_U1.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum U1FullSpectrumCapabilityStatus {
    Qualified,
    StructurallyReadyNeedsGuiQualification,
    UnsupportedInstallation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumCapabilityReport {
    pub adapter_id: String,
    pub status: U1FullSpectrumCapabilityStatus,
    pub conversion_available: bool,
    pub qualification_candidate_available: bool,
    pub application_version: Option<String>,
    pub executable_sha256: Option<String>,
    pub qualification_evidence_valid: bool,
    pub issues: Vec<String>,
}

impl U1FullSpectrumCapabilityReport {
    #[must_use]
    pub fn installation_supported(&self) -> bool {
        !matches!(
            self.status,
            U1FullSpectrumCapabilityStatus::UnsupportedInstallation
        )
    }
}

/// Checks the exact structural baseline and the separately hash-bound GUI
/// qualification evidence. Pending or malformed evidence fails closed.
pub fn inspect_u1_full_spectrum_macos_application(
    application_path: &Path,
) -> Result<U1FullSpectrumCapabilityReport, U1FullSpectrumError> {
    let report = inspect_macos_application(application_path)
        .map_err(|error| U1FullSpectrumError::Capability(error.to_string()))?;
    let executable =
        File::open(&report.executable_path).map_err(|source| U1FullSpectrumError::Read {
            path: report.executable_path.clone(),
            source,
        })?;
    let (_, _, executable_sha256) = hash_open_file(executable, &report.executable_path)?;
    let mut issues = report.issues.clone();
    if executable_sha256 != U1_FULL_SPECTRUM_EXECUTABLE_SHA256 {
        issues.push(
            "The Snapmaker Orca executable hash does not match the Full Spectrum structural baseline."
                .to_owned(),
        );
    }
    let supported = report.compatibility == Compatibility::Guaranteed && issues.is_empty();
    let qualified = supported && full_spectrum_qualification_bundle_is_valid(&report);
    if supported && !qualified {
        issues.push(
            "Full Spectrum metadata is structurally implemented, but a dedicated Snapmaker Orca 2.3.6 GUI save/slice/reopen qualification fixture is still required."
                .to_owned(),
        );
    }
    Ok(U1FullSpectrumCapabilityReport {
        adapter_id: U1_FULL_SPECTRUM_ADAPTER_ID.to_owned(),
        status: if !supported {
            U1FullSpectrumCapabilityStatus::UnsupportedInstallation
        } else if qualified {
            U1FullSpectrumCapabilityStatus::Qualified
        } else {
            U1FullSpectrumCapabilityStatus::StructurallyReadyNeedsGuiQualification
        },
        conversion_available: qualified,
        qualification_candidate_available: supported,
        application_version: Some(report.version),
        executable_sha256: Some(executable_sha256),
        qualification_evidence_valid: qualified,
        issues,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct U1FullSpectrumQualificationRecord {
    schema_version: u32,
    adapter_id: String,
    application_version: String,
    executable_sha256: String,
    writer_qualification_scope: String,
    recipe_accuracy_policy: String,
    gui_round_trip_passed: bool,
    status: String,
    evidence: Option<U1FullSpectrumQualificationEvidence>,
    note: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct U1FullSpectrumQualificationEvidence {
    fixture_name: String,
    source_fixture_sha256: String,
    writer_candidate_sha256: String,
    first_gui_saved_sha256: String,
    reopened_gui_saved_sha256: String,
    qualification_report_sha256: String,
    opened: bool,
    sliced: bool,
    saved: bool,
    closed: bool,
    reopened: bool,
    resliced: bool,
    resaved: bool,
    qualified_at_utc: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct U1FullSpectrumQualificationReportDocument {
    schema_version: u32,
    adapter_id: String,
    application_version: String,
    executable_sha256: String,
    writer_qualification_scope: String,
    recipe_accuracy_policy: String,
    status: String,
    qualification: Option<U1FullSpectrumQualificationReportEvidence>,
    note: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct U1FullSpectrumQualificationReportEvidence {
    fixture_name: String,
    source_fixture_sha256: String,
    writer_candidate_sha256: String,
    first_gui_saved_sha256: String,
    reopened_gui_saved_sha256: String,
    opened: bool,
    sliced: bool,
    saved: bool,
    closed: bool,
    reopened: bool,
    resliced: bool,
    resaved: bool,
    qualified_at_utc: String,
    profiles: Vec<U1FullSpectrumQualificationProfile>,
    checks: U1FullSpectrumQualificationChecks,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct U1FullSpectrumQualificationProfile {
    relative_path: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct U1FullSpectrumQualificationChecks {
    full_spectrum_profile_loaded: bool,
    process_profile_loaded: bool,
    four_physical_slots_verified: bool,
    mixed_definitions_stable: bool,
    virtual_assignments_stable: bool,
    solid_t4_region_stable: bool,
    plate_maps_stable: bool,
    subdivision_contract_stable: bool,
    prime_tower_stable: bool,
    slice_completed_without_repair_warning: bool,
    no_incompatible_profile_warning: bool,
    no_missing_profile_warning: bool,
    no_custom_profile_warning: bool,
    first_gui_saved_structurally_valid: bool,
    reopened_gui_saved_structurally_valid: bool,
    no_embedded_presets_after_first_save: bool,
    no_embedded_presets_after_reopened_save: bool,
    geometry_and_placements_stable: bool,
}

impl U1FullSpectrumQualificationChecks {
    fn all_passed(&self) -> bool {
        self.full_spectrum_profile_loaded
            && self.process_profile_loaded
            && self.four_physical_slots_verified
            && self.mixed_definitions_stable
            && self.virtual_assignments_stable
            && self.solid_t4_region_stable
            && self.plate_maps_stable
            && self.subdivision_contract_stable
            && self.prime_tower_stable
            && self.slice_completed_without_repair_warning
            && self.no_incompatible_profile_warning
            && self.no_missing_profile_warning
            && self.no_custom_profile_warning
            && self.first_gui_saved_structurally_valid
            && self.reopened_gui_saved_structurally_valid
            && self.no_embedded_presets_after_first_save
            && self.no_embedded_presets_after_reopened_save
            && self.geometry_and_placements_stable
    }
}

const FULL_SPECTRUM_QUALIFICATION_PROFILES: &[(&str, &str)] = &[
    (FULL_SPECTRUM_PROFILE_PATH, FULL_SPECTRUM_PROFILE_SHA256),
    (
        FULL_SPECTRUM_FILAMENT_BASE_PATH,
        FULL_SPECTRUM_FILAMENT_BASE_SHA256,
    ),
    (MACHINE_PROFILE_PATH, U1_MACHINE_PROFILE_SHA256),
    (MACHINE_U1_PATH, U1_MACHINE_BASE_SHA256),
    (MACHINE_TOOLCHANGER_PATH, TOOLCHANGER_MACHINE_BASE_SHA256),
    (MACHINE_KLIPPER_PATH, KLIPPER_MACHINE_BASE_SHA256),
    (PROCESS_PROFILE_PATH, FULL_SPECTRUM_PROCESS_PROFILE_SHA256),
    (PROCESS_008_PATH, FULL_SPECTRUM_PROCESS_BASE_SHA256),
    (PROCESS_COMMON_PATH, U1_PROCESS_COMMON_SHA256),
    (PROCESS_U1_PATH, U1_PROCESS_BASE_SHA256),
    GENERIC_PLA_PROFILE_CHAIN[0],
    GENERIC_PLA_PROFILE_CHAIN[1],
    GENERIC_PLA_PROFILE_CHAIN[2],
    GENERIC_PLA_PROFILE_CHAIN[3],
    POLYMAKER_PLA_PROFILE_CHAIN[0],
    POLYMAKER_PLA_PROFILE_CHAIN[1],
    POLYMAKER_PLA_PROFILE_CHAIN[2],
    POLYMAKER_PLA_PROFILE_CHAIN[3],
];

fn full_spectrum_qualification_bundle_is_valid(installation: &InstallationReport) -> bool {
    full_spectrum_qualification_bundle_bytes_are_valid(
        U1_FULL_SPECTRUM_QUALIFICATION_RECORD,
        U1_FULL_SPECTRUM_QUALIFICATION_REPORT,
        installation,
    )
}

fn full_spectrum_qualification_bundle_bytes_are_valid(
    record_bytes: &[u8],
    report_bytes: &[u8],
    installation: &InstallationReport,
) -> bool {
    installation.version == SUPPORTED_ORCA_VERSION
        && installation_profile_hashes_match(installation)
        && full_spectrum_qualification_documents_are_valid(record_bytes, report_bytes)
}

fn full_spectrum_qualification_documents_are_valid(
    record_bytes: &[u8],
    report_bytes: &[u8],
) -> bool {
    let Ok(record) = serde_json::from_slice::<U1FullSpectrumQualificationRecord>(record_bytes)
    else {
        return false;
    };
    let Ok(report) =
        serde_json::from_slice::<U1FullSpectrumQualificationReportDocument>(report_bytes)
    else {
        return false;
    };
    let (Some(record_evidence), Some(report_evidence)) = (record.evidence, report.qualification)
    else {
        return false;
    };
    let report_sha256 = format!("{:x}", Sha256::digest(report_bytes));
    let profiles_match = report_evidence.profiles.len()
        == FULL_SPECTRUM_QUALIFICATION_PROFILES.len()
        && report_evidence
            .profiles
            .iter()
            .zip(FULL_SPECTRUM_QUALIFICATION_PROFILES)
            .all(|(actual, expected)| {
                actual.relative_path == expected.0 && actual.sha256 == expected.1
            });
    record.schema_version == 2
        && record.adapter_id == U1_FULL_SPECTRUM_ADAPTER_ID
        && record.application_version == SUPPORTED_ORCA_VERSION
        && record.executable_sha256 == U1_FULL_SPECTRUM_EXECUTABLE_SHA256
        && record.writer_qualification_scope == U1_FULL_SPECTRUM_WRITER_QUALIFICATION_SCOPE
        && record.recipe_accuracy_policy == U1_FULL_SPECTRUM_RECIPE_ACCURACY_POLICY
        && record.gui_round_trip_passed
        && record.status == "qualified"
        && !record.note.trim().is_empty()
        && report.schema_version == 2
        && report.adapter_id == U1_FULL_SPECTRUM_ADAPTER_ID
        && report.application_version == SUPPORTED_ORCA_VERSION
        && report.executable_sha256 == U1_FULL_SPECTRUM_EXECUTABLE_SHA256
        && report.writer_qualification_scope == U1_FULL_SPECTRUM_WRITER_QUALIFICATION_SCOPE
        && report.recipe_accuracy_policy == U1_FULL_SPECTRUM_RECIPE_ACCURACY_POLICY
        && report.status == "qualified"
        && !report.note.trim().is_empty()
        && record_evidence.qualification_report_sha256 == report_sha256
        && record_evidence.fixture_name == U1_FULL_SPECTRUM_QUALIFICATION_FIXTURE
        && record_evidence.source_fixture_sha256 == U1_FULL_SPECTRUM_QUALIFICATION_FIXTURE_SHA256
        && valid_nonzero_sha256(&record_evidence.source_fixture_sha256)
        && record_evidence.fixture_name == report_evidence.fixture_name
        && record_evidence.source_fixture_sha256 == report_evidence.source_fixture_sha256
        && record_evidence.writer_candidate_sha256 == report_evidence.writer_candidate_sha256
        && record_evidence.first_gui_saved_sha256 == report_evidence.first_gui_saved_sha256
        && record_evidence.reopened_gui_saved_sha256 == report_evidence.reopened_gui_saved_sha256
        && valid_nonzero_sha256(&record_evidence.writer_candidate_sha256)
        && valid_nonzero_sha256(&record_evidence.first_gui_saved_sha256)
        && valid_nonzero_sha256(&record_evidence.reopened_gui_saved_sha256)
        && valid_nonzero_sha256(&record_evidence.qualification_report_sha256)
        && record_evidence.opened
        && record_evidence.sliced
        && record_evidence.saved
        && record_evidence.closed
        && record_evidence.reopened
        && record_evidence.resliced
        && record_evidence.resaved
        && record_evidence.opened == report_evidence.opened
        && record_evidence.sliced == report_evidence.sliced
        && record_evidence.saved == report_evidence.saved
        && record_evidence.closed == report_evidence.closed
        && record_evidence.reopened == report_evidence.reopened
        && record_evidence.resliced == report_evidence.resliced
        && record_evidence.resaved == report_evidence.resaved
        && !record_evidence.qualified_at_utc.trim().is_empty()
        && record_evidence.qualified_at_utc == report_evidence.qualified_at_utc
        && profiles_match
        && report_evidence.checks.all_passed()
}

fn installation_profile_hashes_match(installation: &InstallationReport) -> bool {
    let actual = std::iter::once(&installation.full_spectrum_profile)
        .chain(std::iter::once(&installation.machine_profile))
        .chain(installation.inherited_profiles.iter())
        .chain(installation.process_profiles.iter())
        .collect::<Vec<_>>();
    let reported_profiles_match =
        FULL_SPECTRUM_QUALIFICATION_PROFILES[..10]
            .iter()
            .all(|(relative, expected_hash)| {
                actual.iter().any(|profile| {
                    profile.valid
                        && profile.sha256 == *expected_hash
                        && profile
                            .path
                            .to_string_lossy()
                            .replace('\\', "/")
                            .ends_with(&format!("/Snapmaker/{relative}"))
                })
            });
    let Ok(profiles_root) =
        crate::qualified_u1_physical_profiles_root(&installation.application_path)
    else {
        return false;
    };
    let physical_profiles_match =
        FULL_SPECTRUM_QUALIFICATION_PROFILES[10..]
            .iter()
            .all(|(relative, expected_hash)| {
                fs::read(profiles_root.join(relative))
                    .ok()
                    .is_some_and(|bytes| format!("{:x}", Sha256::digest(bytes)) == *expected_hash)
            });
    reported_profiles_match && physical_profiles_match
}

fn valid_nonzero_sha256(value: &str) -> bool {
    value.len() == 64
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        && value.bytes().any(|byte| byte != b'0')
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum U1FullSpectrumNativeDistribution {
    LayerCycle,
    Simple,
}

impl U1FullSpectrumNativeDistribution {
    const fn native_value(self) -> u8 {
        match self {
            Self::LayerCycle => 0,
            Self::Simple => 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumComponent {
    pub toolhead: Toolhead,
    pub weight: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumRecipeDefinition {
    pub virtual_filament_id: u8,
    pub stable_id: u64,
    pub recipe_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration_sample_id: Option<String>,
    pub mode: FullSpectrumMode,
    pub display_name: String,
    pub predicted_color: Option<RgbColor>,
    pub components: Vec<U1FullSpectrumComponent>,
    pub component_a: u8,
    pub component_b: u8,
    pub mix_b_percent: u8,
    pub distribution: U1FullSpectrumNativeDistribution,
    pub gradient_component_ids: Vec<u8>,
    pub gradient_component_weights: Vec<u8>,
    pub manual_pattern: Option<String>,
    pub local_z_max_sublayers: u8,
    pub gradient_start_millionths: Option<u32>,
    pub gradient_end_millionths: Option<u32>,
    pub serialized: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumRecipeInput {
    pub logical_id: String,
    pub target_material: Material,
    pub target_color: RgbColor,
    pub recipe: CmyxRecipe,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration_sample_id: Option<String>,
    pub predicted_color: Option<RgbColor>,
    pub confidence: ColorConfidence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumRecipeTarget {
    pub logical_id: String,
    pub target_filament_id: u8,
    pub recipe_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration_sample_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumRecipeTable {
    pub schema_version: u32,
    pub physical_filament_count: u8,
    pub definitions: Vec<U1FullSpectrumRecipeDefinition>,
    pub serialized_definitions: String,
    pub targets: Vec<U1FullSpectrumRecipeTarget>,
}

/// One active row from Snapmaker Orca 2.3.6's native
/// `mixed_filament_definitions` project setting. Virtual filament IDs are
/// positional in that format and are therefore derived from the active row
/// order rather than serialized in the row itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumNativeDefinition {
    pub virtual_filament_id: u8,
    pub component_a: u8,
    pub component_b: u8,
    pub mix_b_percent: u8,
    pub distribution: U1FullSpectrumNativeDistribution,
    pub gradient_component_ids: Vec<u8>,
    pub gradient_component_weights: Vec<u8>,
    pub local_z_max_sublayers: u8,
    pub stable_id: u64,
    pub mode: FullSpectrumMode,
    pub gradient_start_millionths: Option<u32>,
    pub gradient_end_millionths: Option<u32>,
    pub manual_pattern: Option<String>,
    pub serialized: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumCompileContext {
    pub calibration_fingerprint: String,
    pub process: FullSpectrumProcessCompatibility,
    pub t4_color: Option<RgbColor>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumPhysicalSlot {
    pub toolhead: Toolhead,
    pub spool_id: String,
    pub spool_name: String,
    pub material: Material,
    pub color: RgbColor,
    pub profile: String,
    pub setting_id: String,
    pub filament_id: String,
    pub wildcard_resolved: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumPreparedUnit {
    pub unit: ScopedUnitRef,
    pub source_unit_id: String,
    pub source_object_id: u32,
    pub source_instance_id: u32,
    pub source_model_path: Option<String>,
    pub source_plate_id: Option<String>,
    pub target_min_x_mm: f64,
    pub target_min_y_mm: f64,
    pub source_to_target_slots: BTreeMap<u8, u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumPreparedPlate {
    pub plan_plate_id: String,
    pub target_plate_id: u32,
    pub job_id: String,
    pub prime_tower: Option<U1FullSpectrumPrimeTower>,
    pub units: Vec<U1FullSpectrumPreparedUnit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumPrimeTower {
    pub x_mm: f64,
    pub y_mm: f64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumPreparedAssignment {
    pub scope_id: String,
    pub source_requirement_ids: Vec<String>,
    pub source_slots: Vec<u8>,
    pub source_material: Material,
    pub source_color: RgbColor,
    pub target_filament_id: u8,
    pub recipe_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration_sample_id: Option<String>,
    pub predicted_color: Option<RgbColor>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumPreparedArtifact {
    pub batch_id: String,
    pub file_name: String,
    pub loadout: [U1FullSpectrumPhysicalSlot; 4],
    pub calibration_fingerprint: String,
    pub process: FullSpectrumProcessCompatibility,
    #[serde(default)]
    pub support: SupportInformation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dedicated_support: Option<DedicatedSupportMaterial>,
    pub recipe_table: U1FullSpectrumRecipeTable,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recipe_calibration_sample_ids: Vec<String>,
    pub assignments: Vec<U1FullSpectrumPreparedAssignment>,
    pub plates: Vec<U1FullSpectrumPreparedPlate>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumPreparation {
    pub schema_version: u32,
    pub adapter_id: String,
    pub plan_fingerprint: String,
    pub production_qualified: bool,
    pub artifacts: Vec<U1FullSpectrumPreparedArtifact>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum U1FullSpectrumValidationSeverity {
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum U1FullSpectrumValidationCode {
    StructuralPackage,
    StaleArtifact,
    InvalidProjectSettings,
    WrongTargetProfile,
    UnsynchronizedPhysicalArrays,
    InvalidMixedDefinition,
    NonCanonicalDefinition,
    InvalidMixedProcess,
    InvalidPrimeTower,
    InvalidPlateMap,
    AssignmentOutOfRange,
    MissingVirtualDefinition,
    UnusedVirtualDefinition,
    ArtifactMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumValidationIssue {
    pub severity: U1FullSpectrumValidationSeverity,
    pub code: U1FullSpectrumValidationCode,
    pub path: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumValidationReport {
    pub adapter_id: String,
    pub valid: bool,
    pub physical_filament_count: usize,
    pub virtual_filament_count: usize,
    pub used_filament_ids: Vec<u8>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recipe_calibration_sample_ids: Vec<String>,
    pub issues: Vec<U1FullSpectrumValidationIssue>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumCandidateWriteReport {
    pub path: PathBuf,
    pub byte_size: u64,
    pub sha256: String,
    pub validation: U1FullSpectrumValidationReport,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumPublishedArtifact {
    pub batch_id: String,
    pub file_name: String,
    pub path: PathBuf,
    pub byte_size: u64,
    pub sha256: String,
    pub plate_count: usize,
    pub target_plate_ids: Vec<String>,
    pub source_unit_ids: Vec<String>,
    pub validation: U1FullSpectrumValidationReport,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumConversionResult {
    pub adapter_id: String,
    pub plan_fingerprint: String,
    pub output_directory: PathBuf,
    pub artifacts: Vec<U1FullSpectrumPublishedArtifact>,
    pub warnings: Vec<String>,
}

/// Cooperative cancellation checkpoints exposed by the production Full
/// Spectrum orchestrator. Callbacks should return quickly and must not mutate
/// conversion inputs or output paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum U1FullSpectrumCancellationCheckpoint {
    ConversionStart,
    ArtifactStart,
    BeforeProjectSettingsBuild,
    AfterProjectSettingsBuild,
    BeforeSubstrateBuild,
    AfterSubstrateBuild,
    BeforeModelConfigValidation,
    AfterModelConfigValidation,
    HashChunk,
    XmlEventBatch,
    PackageCopyEntry,
    PackageCopyChunk,
    BeforePackageStaging,
    AfterPackageStaging,
    BeforePublication,
}

impl std::fmt::Display for U1FullSpectrumCancellationCheckpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::ConversionStart => "conversion start",
            Self::ArtifactStart => "artifact start",
            Self::BeforeProjectSettingsBuild => "before project settings build",
            Self::AfterProjectSettingsBuild => "after project settings build",
            Self::BeforeSubstrateBuild => "before substrate build",
            Self::AfterSubstrateBuild => "after substrate build",
            Self::BeforeModelConfigValidation => "before model/config validation",
            Self::AfterModelConfigValidation => "after model/config validation",
            Self::HashChunk => "during file hashing",
            Self::XmlEventBatch => "during XML processing",
            Self::PackageCopyEntry => "during package entry copying",
            Self::PackageCopyChunk => "during package data copying",
            Self::BeforePackageStaging => "before package staging",
            Self::AfterPackageStaging => "after package staging",
            Self::BeforePublication => "before output publication",
        };
        formatter.write_str(label)
    }
}

#[derive(Debug, Error)]
pub enum U1FullSpectrumError {
    #[error("Snapmaker U1 Full Spectrum conversion is unavailable: {0}")]
    Capability(String),
    #[error("the canonical Full Spectrum plan is invalid: {0}")]
    Plan(String),
    #[error("the Full Spectrum recipe is invalid: {0}")]
    Recipe(String),
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid ZIP package: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("invalid JSON in {path}: {source}")]
    Json {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid XML in {path}: {message}")]
    Xml { path: String, message: String },
    #[error("output already exists: {0}")]
    OutputExists(PathBuf),
    #[error("Snapmaker U1 Full Spectrum conversion was cancelled at {checkpoint}")]
    Cancelled {
        checkpoint: U1FullSpectrumCancellationCheckpoint,
    },
    #[error("generated Full Spectrum candidate failed semantic validation: {0}")]
    SemanticValidation(String),
    #[error("failed to build deterministic Full Spectrum OPC package: {0}")]
    Opc(String),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecipeIdentity {
    schema_version: u32,
    target_material: Material,
    mode: FullSpectrumMode,
    sequence: Vec<u8>,
    components: Vec<(u8, u16)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    calibration_sample_id: Option<String>,
    calibration_fingerprint: String,
    printer_profile_fingerprint: String,
    process_fingerprint: String,
    plate_layer_height_microns: u32,
    subdivision_policy: FullSpectrumSubdivisionPolicy,
    subdivision_factor: u8,
    effective_sublayer_height_microns: u32,
}

#[derive(Clone, Debug)]
struct PendingDefinition {
    identity: RecipeIdentity,
    predicted_color: Option<RgbColor>,
}

#[derive(Clone, Debug)]
enum PendingTarget {
    Physical(u8),
    Virtual(String),
}

#[must_use]
pub fn u1_full_spectrum_process_contract() -> FullSpectrumProcessCompatibility {
    FullSpectrumProcessCompatibility {
        printer_profile_fingerprint: format!(
            "machine:{U1_MACHINE_PROFILE_SHA256}:{U1_MACHINE_BASE_SHA256}:{TOOLCHANGER_MACHINE_BASE_SHA256}:{KLIPPER_MACHINE_BASE_SHA256}|filament:{FULL_SPECTRUM_PROFILE_SHA256}:{FULL_SPECTRUM_FILAMENT_BASE_SHA256}"
        ),
        plate_layer_height_microns: LAYER_HEIGHT_MICRONS,
        subdivision_policy: FullSpectrumSubdivisionPolicy::SubdivideMixLayer,
        subdivision_factor: SUBDIVISION_FACTOR,
        effective_sublayer_height_microns: SUBLAYER_HEIGHT_MICRONS,
        process_fingerprint: format!(
            "orca:{SUPPORTED_ORCA_VERSION}|machine:{U1_MACHINE_SETTING_ID}:{U1_MACHINE_PROFILE_SHA256}|machine-u1:{U1_MACHINE_BASE_SHA256}|machine-toolchanger:{TOOLCHANGER_MACHINE_BASE_SHA256}|machine-klipper:{KLIPPER_MACHINE_BASE_SHA256}|filament:{FULL_SPECTRUM_SETTING_ID}:{FULL_SPECTRUM_PROFILE_NAME}:{FULL_SPECTRUM_PROFILE_SHA256}|filament-base:{FULL_SPECTRUM_FILAMENT_BASE_SHA256}|process:{FULL_SPECTRUM_PROCESS_SETTING_ID}:{FULL_SPECTRUM_PROCESS_PROFILE_NAME}:{FULL_SPECTRUM_PROCESS_PROFILE_SHA256}|process-base:{FULL_SPECTRUM_PROCESS_BASE_SHA256}|process-common:{U1_PROCESS_COMMON_SHA256}|process-u1:{U1_PROCESS_BASE_SHA256}|layer:80um|subdivide:4|effective:20um"
        ),
    }
}

pub fn compile_u1_full_spectrum_recipes(
    inputs: &[U1FullSpectrumRecipeInput],
    context: &U1FullSpectrumCompileContext,
) -> Result<U1FullSpectrumRecipeTable, U1FullSpectrumError> {
    compile_u1_full_spectrum_recipes_for_purpose(inputs, context, false)
}

/// Compiles native Full Spectrum rows for a physical calibration chart.
///
/// Unlike [`compile_u1_full_spectrum_recipes`], this explicitly permits a
/// nominal recipe to consume a near-black or near-white T4 spool. That recipe
/// is a measurement target, not a color prediction: callers must keep the
/// resulting package behind the qualification-candidate boundary and must not
/// attach a `calibration_sample_id` until the printed swatch is measured.
pub fn compile_u1_full_spectrum_calibration_candidate_recipes(
    inputs: &[U1FullSpectrumRecipeInput],
    context: &U1FullSpectrumCompileContext,
) -> Result<U1FullSpectrumRecipeTable, U1FullSpectrumError> {
    if inputs
        .iter()
        .any(|input| input.calibration_sample_id.is_some())
    {
        return Err(U1FullSpectrumError::Recipe(
            "calibration candidate recipes must not claim an existing calibration sample".into(),
        ));
    }
    compile_u1_full_spectrum_recipes_for_purpose(inputs, context, true)
}

fn compile_u1_full_spectrum_recipes_for_purpose(
    inputs: &[U1FullSpectrumRecipeInput],
    context: &U1FullSpectrumCompileContext,
    allow_unmeasured_extreme_t4: bool,
) -> Result<U1FullSpectrumRecipeTable, U1FullSpectrumError> {
    if context.calibration_fingerprint.trim().is_empty() {
        return Err(U1FullSpectrumError::Recipe(
            "calibration fingerprint must not be empty".into(),
        ));
    }
    validate_exact_process(&context.process)?;
    let mut logical_ids = BTreeSet::new();
    let mut pending_definitions = BTreeMap::<String, PendingDefinition>::new();
    let mut pending_targets =
        Vec::<(String, PendingTarget, Option<String>)>::with_capacity(inputs.len());

    for input in inputs {
        if input.logical_id.trim().is_empty() || !logical_ids.insert(input.logical_id.clone()) {
            return Err(U1FullSpectrumError::Recipe(format!(
                "logical recipe ID {:?} is empty or duplicated",
                input.logical_id
            )));
        }
        let dedicated_pva = input.target_material == Material::Pva
            && matches!(input.recipe, CmyxRecipe::DedicatedT4);
        if input.target_material != Material::Pla && !dedicated_pva {
            return Err(U1FullSpectrumError::Recipe(format!(
                "recipe {} targets {:?}; only PLA color recipes and solid T4 PVA support are qualified",
                input.logical_id, input.target_material
            )));
        }
        if input
            .calibration_sample_id
            .as_deref()
            .is_some_and(|sample_id| sample_id.trim().is_empty())
        {
            return Err(U1FullSpectrumError::Recipe(format!(
                "recipe {} has an empty calibration sample ID",
                input.logical_id
            )));
        }
        let pending = match &input.recipe {
            CmyxRecipe::Solid { toolhead } => PendingTarget::Physical(toolhead_id(*toolhead)),
            CmyxRecipe::DedicatedT4 => PendingTarget::Physical(4),
            CmyxRecipe::FullSpectrum { mode, sequence } => {
                validate_mixed_sequence(*mode, sequence)?;
                if sequence.contains(&Toolhead::T4)
                    && context.t4_color.is_some_and(is_near_black_or_white)
                    && !allow_unmeasured_extreme_t4
                    && !matches!(
                        input.confidence,
                        ColorConfidence::Calibrated | ColorConfidence::Measured
                    )
                {
                    return Err(U1FullSpectrumError::Recipe(format!(
                        "recipe {} mixes an uncalibrated near-black or near-white T4 filament",
                        input.logical_id
                    )));
                }
                let sequence = canonical_recipe_sequence(*mode, sequence);
                let components = weighted_components(&sequence);
                let identity = RecipeIdentity {
                    schema_version: U1_FULL_SPECTRUM_SCHEMA_VERSION,
                    target_material: input.target_material.clone(),
                    mode: *mode,
                    sequence,
                    components,
                    calibration_sample_id: input.calibration_sample_id.clone(),
                    calibration_fingerprint: context.calibration_fingerprint.clone(),
                    printer_profile_fingerprint: context
                        .process
                        .printer_profile_fingerprint
                        .clone(),
                    process_fingerprint: context.process.process_fingerprint.clone(),
                    plate_layer_height_microns: context.process.plate_layer_height_microns,
                    subdivision_policy: context.process.subdivision_policy.clone(),
                    subdivision_factor: context.process.subdivision_factor,
                    effective_sublayer_height_microns: context
                        .process
                        .effective_sublayer_height_microns,
                };
                let fingerprint = recipe_fingerprint(&identity)?;
                pending_definitions
                    .entry(fingerprint.clone())
                    .and_modify(|definition| {
                        if definition.predicted_color != input.predicted_color {
                            definition.predicted_color = None;
                        }
                    })
                    .or_insert(PendingDefinition {
                        identity,
                        predicted_color: input.predicted_color,
                    });
                PendingTarget::Virtual(fingerprint)
            }
            CmyxRecipe::Fallback { description, .. } => {
                return Err(U1FullSpectrumError::Recipe(format!(
                    "fallback recipe {} has no native Full Spectrum mode: {description}",
                    input.logical_id
                )));
            }
            CmyxRecipe::ManualReview { reason } | CmyxRecipe::Unreachable { reason } => {
                return Err(U1FullSpectrumError::Recipe(format!(
                    "recipe {} is not schedulable: {reason}",
                    input.logical_id
                )));
            }
        };
        pending_targets.push((
            input.logical_id.clone(),
            pending,
            input.calibration_sample_id.clone(),
        ));
    }

    if pending_definitions.len()
        > usize::from(U1_FULL_SPECTRUM_MAX_FILAMENT_ID - U1_FULL_SPECTRUM_PHYSICAL_COUNT)
    {
        return Err(U1FullSpectrumError::Recipe(format!(
            "{} virtual recipes exceed the native paint-state limit of {}",
            pending_definitions.len(),
            U1_FULL_SPECTRUM_MAX_FILAMENT_ID - U1_FULL_SPECTRUM_PHYSICAL_COUNT
        )));
    }

    let mut stable_ids = BTreeSet::new();
    let mut definitions = Vec::with_capacity(pending_definitions.len());
    let mut virtual_by_fingerprint = BTreeMap::new();
    for (index, (fingerprint, pending)) in pending_definitions.into_iter().enumerate() {
        let virtual_filament_id = U1_FULL_SPECTRUM_FIRST_VIRTUAL_ID
            .checked_add(u8::try_from(index).map_err(|_| {
                U1FullSpectrumError::Recipe("virtual recipe count exceeds u8".into())
            })?)
            .ok_or_else(|| U1FullSpectrumError::Recipe("virtual filament ID overflow".into()))?;
        let stable_id = stable_id_from_fingerprint(&fingerprint)?;
        if !stable_ids.insert(stable_id) {
            return Err(U1FullSpectrumError::Recipe(
                "two recipe fingerprints collide in their native stable ID".into(),
            ));
        }
        let definition = materialize_definition(
            virtual_filament_id,
            stable_id,
            fingerprint.clone(),
            pending,
            context.process.subdivision_factor,
        )?;
        virtual_by_fingerprint.insert(fingerprint, virtual_filament_id);
        definitions.push(definition);
    }

    let mut targets = pending_targets
        .into_iter()
        .map(|(logical_id, target, calibration_sample_id)| match target {
            PendingTarget::Physical(target_filament_id) => Ok(U1FullSpectrumRecipeTarget {
                logical_id,
                target_filament_id,
                recipe_fingerprint: None,
                calibration_sample_id,
            }),
            PendingTarget::Virtual(fingerprint) => {
                let target_filament_id = virtual_by_fingerprint
                    .get(&fingerprint)
                    .copied()
                    .ok_or_else(|| {
                        U1FullSpectrumError::Recipe(
                            "compiled recipe lost its virtual definition".into(),
                        )
                    })?;
                Ok(U1FullSpectrumRecipeTarget {
                    logical_id,
                    target_filament_id,
                    recipe_fingerprint: Some(fingerprint),
                    calibration_sample_id,
                })
            }
        })
        .collect::<Result<Vec<_>, U1FullSpectrumError>>()?;
    targets.sort_by(|left, right| left.logical_id.cmp(&right.logical_id));
    let serialized_definitions = definitions
        .iter()
        .map(|definition| definition.serialized.as_str())
        .collect::<Vec<_>>()
        .join(";");
    Ok(U1FullSpectrumRecipeTable {
        schema_version: U1_FULL_SPECTRUM_SCHEMA_VERSION,
        physical_filament_count: U1_FULL_SPECTRUM_PHYSICAL_COUNT,
        definitions,
        serialized_definitions,
        targets,
    })
}

fn validate_exact_process(
    process: &FullSpectrumProcessCompatibility,
) -> Result<(), U1FullSpectrumError> {
    let expected = u1_full_spectrum_process_contract();
    if process != &expected {
        return Err(U1FullSpectrumError::Recipe(format!(
            "process contract does not match the exact {} / {} baseline",
            FULL_SPECTRUM_PROCESS_PROFILE_NAME, FULL_SPECTRUM_PROFILE_NAME
        )));
    }
    Ok(())
}

fn validate_mixed_sequence(
    mode: FullSpectrumMode,
    sequence: &[Toolhead],
) -> Result<(), U1FullSpectrumError> {
    if sequence.is_empty() || sequence.len() > 100 {
        return Err(U1FullSpectrumError::Recipe(
            "mixed sequence must contain 1 through 100 layer tokens".into(),
        ));
    }
    let unique = sequence.iter().copied().collect::<BTreeSet<_>>();
    let valid_count = match mode {
        FullSpectrumMode::Gradient => unique.len() == 2,
        FullSpectrumMode::Ratio | FullSpectrumMode::Match => (2..=3).contains(&unique.len()),
        FullSpectrumMode::Cycle => (2..=4).contains(&unique.len()),
    };
    if !valid_count {
        return Err(U1FullSpectrumError::Recipe(format!(
            "{mode:?} recipe has {} distinct components, outside the native mode limit",
            unique.len()
        )));
    }
    Ok(())
}

fn canonical_recipe_sequence(mode: FullSpectrumMode, sequence: &[Toolhead]) -> Vec<u8> {
    let raw = sequence
        .iter()
        .copied()
        .map(toolhead_id)
        .collect::<Vec<_>>();
    match mode {
        FullSpectrumMode::Cycle => shortest_repeating_pattern(&raw),
        FullSpectrumMode::Ratio | FullSpectrumMode::Match => {
            let components = weighted_components(&raw);
            let divisor = components
                .iter()
                .map(|(_, weight)| *weight)
                .reduce(greatest_common_divisor)
                .unwrap_or(1)
                .max(1);
            components
                .into_iter()
                .flat_map(|(id, weight)| std::iter::repeat_n(id, usize::from(weight / divisor)))
                .collect()
        }
        FullSpectrumMode::Gradient => raw.into_iter().fold(Vec::new(), |mut ids, id| {
            if !ids.contains(&id) {
                ids.push(id);
            }
            ids
        }),
    }
}

fn shortest_repeating_pattern(sequence: &[u8]) -> Vec<u8> {
    for length in 1..=sequence.len() {
        if sequence.len().is_multiple_of(length)
            && sequence
                .iter()
                .enumerate()
                .all(|(index, value)| *value == sequence[index % length])
        {
            return sequence[..length].to_vec();
        }
    }
    sequence.to_vec()
}

fn weighted_components(sequence: &[u8]) -> Vec<(u8, u16)> {
    let mut components = Vec::<(u8, u16)>::new();
    for id in sequence {
        if let Some((_, weight)) = components.iter_mut().find(|(candidate, _)| candidate == id) {
            *weight = weight.saturating_add(1);
        } else {
            components.push((*id, 1));
        }
    }
    components
}

fn greatest_common_divisor(mut left: u16, mut right: u16) -> u16 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left.max(1)
}

fn recipe_fingerprint(identity: &RecipeIdentity) -> Result<String, U1FullSpectrumError> {
    let bytes = serde_json::to_vec(identity).map_err(|source| U1FullSpectrumError::Json {
        path: "Full Spectrum recipe identity".into(),
        source,
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn stable_id_from_fingerprint(fingerprint: &str) -> Result<u64, U1FullSpectrumError> {
    let prefix = fingerprint.get(..16).ok_or_else(|| {
        U1FullSpectrumError::Recipe("recipe fingerprint is shorter than 64 hex digits".into())
    })?;
    let value = u64::from_str_radix(prefix, 16)
        .map_err(|_| U1FullSpectrumError::Recipe("recipe fingerprint is not hexadecimal".into()))?
        & 0x7fff_ffff_ffff_ffff;
    Ok(value.max(1))
}

fn materialize_definition(
    virtual_filament_id: u8,
    stable_id: u64,
    recipe_fingerprint: String,
    pending: PendingDefinition,
    subdivision_factor: u8,
) -> Result<U1FullSpectrumRecipeDefinition, U1FullSpectrumError> {
    let mode = pending.identity.mode;
    let components = pending
        .identity
        .components
        .iter()
        .map(|(id, weight)| U1FullSpectrumComponent {
            toolhead: id_toolhead(*id),
            weight: *weight,
        })
        .collect::<Vec<_>>();
    let component_ids = pending
        .identity
        .components
        .iter()
        .map(|(id, _)| *id)
        .collect::<Vec<_>>();
    let weights = pending
        .identity
        .components
        .iter()
        .map(|(_, weight)| *weight)
        .collect::<Vec<_>>();
    let (
        component_a,
        component_b,
        distribution,
        gradient_component_ids,
        gradient_component_weights,
        manual_pattern,
        mix_b_percent,
        gradient_start_millionths,
        gradient_end_millionths,
    ) = match mode {
        FullSpectrumMode::Ratio | FullSpectrumMode::Match if component_ids.len() == 2 => {
            let pattern = symbolic_pair_pattern(
                &pending.identity.sequence,
                component_ids[0],
                component_ids[1],
            )?;
            (
                component_ids[0],
                component_ids[1],
                U1FullSpectrumNativeDistribution::Simple,
                if mode == FullSpectrumMode::Match {
                    component_ids.clone()
                } else {
                    Vec::new()
                },
                Vec::new(),
                Some(pattern),
                rounded_percent(weights[1], weights.iter().sum()),
                None,
                None,
            )
        }
        FullSpectrumMode::Ratio | FullSpectrumMode::Match => (
            component_ids[0],
            component_ids[1],
            U1FullSpectrumNativeDistribution::LayerCycle,
            component_ids.clone(),
            normalized_percentages(&weights)?,
            None,
            rounded_percent(weights[1], weights.iter().sum()),
            None,
            None,
        ),
        FullSpectrumMode::Cycle => (
            1,
            2,
            U1FullSpectrumNativeDistribution::Simple,
            Vec::new(),
            Vec::new(),
            Some(physical_pattern(&pending.identity.sequence)?),
            rounded_percent(
                pending
                    .identity
                    .sequence
                    .iter()
                    .filter(|id| **id == 2)
                    .count() as u16,
                pending.identity.sequence.len() as u16,
            ),
            None,
            None,
        ),
        FullSpectrumMode::Gradient => (
            component_ids[0],
            component_ids[1],
            U1FullSpectrumNativeDistribution::LayerCycle,
            Vec::new(),
            Vec::new(),
            None,
            50,
            Some(800_000),
            Some(200_000),
        ),
    };

    let mut definition = U1FullSpectrumRecipeDefinition {
        virtual_filament_id,
        stable_id,
        recipe_fingerprint,
        calibration_sample_id: pending.identity.calibration_sample_id.clone(),
        mode,
        display_name: format!(
            "{} {}",
            match mode {
                FullSpectrumMode::Ratio => "Ratio",
                FullSpectrumMode::Match => "Match",
                FullSpectrumMode::Cycle => "Cycle",
                FullSpectrumMode::Gradient => "Gradient",
            },
            components
                .iter()
                .map(|component| format!(
                    "T{}:{}",
                    toolhead_id(component.toolhead),
                    component.weight
                ))
                .collect::<Vec<_>>()
                .join("+")
        ),
        predicted_color: pending.predicted_color,
        components,
        component_a,
        component_b,
        mix_b_percent,
        distribution,
        gradient_component_ids,
        gradient_component_weights,
        manual_pattern,
        local_z_max_sublayers: subdivision_factor,
        gradient_start_millionths,
        gradient_end_millionths,
        serialized: String::new(),
    };
    definition.serialized = serialize_native_definition(&definition)?;
    Ok(definition)
}

fn serialize_native_definition(
    definition: &U1FullSpectrumRecipeDefinition,
) -> Result<String, U1FullSpectrumError> {
    if definition.component_a == 0
        || definition.component_a > U1_FULL_SPECTRUM_PHYSICAL_COUNT
        || definition.component_b == 0
        || definition.component_b > U1_FULL_SPECTRUM_PHYSICAL_COUNT
        || definition.component_a == definition.component_b
        || definition.stable_id == 0
    {
        return Err(U1FullSpectrumError::Recipe(
            "native definition has invalid physical components or stable ID".into(),
        ));
    }
    let gradient_ids = definition
        .gradient_component_ids
        .iter()
        .map(u8::to_string)
        .collect::<String>();
    let gradient_weights = definition
        .gradient_component_weights
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join("/");
    let ui_mode = match definition.mode {
        FullSpectrumMode::Ratio => 0,
        FullSpectrumMode::Cycle => 1,
        FullSpectrumMode::Match => 2,
        FullSpectrumMode::Gradient => 3,
    };
    let mut fields = vec![
        definition.component_a.to_string(),
        definition.component_b.to_string(),
        "1".into(),
        "1".into(),
        definition.mix_b_percent.to_string(),
        "0".into(),
        format!("g{gradient_ids}"),
        format!("w{gradient_weights}"),
        format!("m{}", definition.distribution.native_value()),
        format!("z{}", definition.local_z_max_sublayers),
        "xa0".into(),
        "xb0".into(),
        "d0".into(),
        "o0".into(),
        format!("u{}", definition.stable_id),
        format!("cm{ui_mode}"),
    ];
    if let (Some(start), Some(end)) = (
        definition.gradient_start_millionths,
        definition.gradient_end_millionths,
    ) {
        fields.push(format!(
            "r1/{:.4}/{:.4}",
            f64::from(start) / 1_000_000.0,
            f64::from(end) / 1_000_000.0
        ));
    }
    if let Some(pattern) = &definition.manual_pattern {
        fields.push(pattern.clone());
    }
    Ok(fields.join(","))
}

fn symbolic_pair_pattern(
    sequence: &[u8],
    component_a: u8,
    component_b: u8,
) -> Result<String, U1FullSpectrumError> {
    sequence
        .iter()
        .map(|id| match *id {
            id if id == component_a => Ok('1'),
            id if id == component_b => Ok('2'),
            _ => Err(U1FullSpectrumError::Recipe(
                "two-component ratio sequence contains another component".into(),
            )),
        })
        .collect()
}

fn physical_pattern(sequence: &[u8]) -> Result<String, U1FullSpectrumError> {
    sequence
        .iter()
        .map(|id| match *id {
            1..=4 => Ok(char::from(b'0' + *id)),
            _ => Err(U1FullSpectrumError::Recipe(
                "cycle pattern references a non-physical filament".into(),
            )),
        })
        .collect()
}

fn normalized_percentages(weights: &[u16]) -> Result<Vec<u8>, U1FullSpectrumError> {
    let total = weights.iter().map(|weight| u32::from(*weight)).sum::<u32>();
    if total == 0 || weights.is_empty() {
        return Err(U1FullSpectrumError::Recipe(
            "component weights sum to zero".into(),
        ));
    }
    let mut assigned = 0_u32;
    let mut values = Vec::with_capacity(weights.len());
    let mut remainders = Vec::with_capacity(weights.len());
    for weight in weights {
        let scaled = u32::from(*weight) * 100;
        let whole = scaled / total;
        assigned += whole;
        values.push(whole as u8);
        remainders.push(scaled % total);
    }
    while assigned < 100 {
        let index = remainders
            .iter()
            .enumerate()
            .max_by(|left, right| left.1.cmp(right.1).then_with(|| right.0.cmp(&left.0)))
            .map(|(index, _)| index)
            .ok_or_else(|| U1FullSpectrumError::Recipe("no component remainder".into()))?;
        values[index] = values[index].saturating_add(1);
        remainders[index] = 0;
        assigned += 1;
    }
    Ok(values)
}

fn rounded_percent(part: u16, total: u16) -> u8 {
    if total == 0 {
        0
    } else {
        (((u32::from(part) * 100) + u32::from(total) / 2) / u32::from(total)) as u8
    }
}

const fn toolhead_id(toolhead: Toolhead) -> u8 {
    match toolhead {
        Toolhead::T1 => 1,
        Toolhead::T2 => 2,
        Toolhead::T3 => 3,
        Toolhead::T4 => 4,
    }
}

fn id_toolhead(id: u8) -> Toolhead {
    match id {
        1 => Toolhead::T1,
        2 => Toolhead::T2,
        3 => Toolhead::T3,
        4 => Toolhead::T4,
        _ => unreachable!("validated physical toolhead ID"),
    }
}

fn is_near_black_or_white(color: RgbColor) -> bool {
    let minimum = color.red.min(color.green).min(color.blue);
    let maximum = color.red.max(color.green).max(color.blue);
    maximum <= 24 || minimum >= 232
}

pub fn prepare_u1_full_spectrum_conversion(
    input: &PlanningInput,
    result: &PlanningResult,
) -> Result<U1FullSpectrumPreparation, U1FullSpectrumError> {
    prepare_u1_full_spectrum_conversion_with_support(input, result, &SupportInformation::default())
}

pub fn prepare_u1_full_spectrum_conversion_with_support(
    input: &PlanningInput,
    result: &PlanningResult,
    support: &SupportInformation,
) -> Result<U1FullSpectrumPreparation, U1FullSpectrumError> {
    if result.has_hard_errors() {
        return Err(U1FullSpectrumError::Plan(format!(
            "{} blocking planner error(s) remain",
            result.errors.len()
        )));
    }
    let plan_bytes =
        serde_json::to_vec(&(input, result)).map_err(|source| U1FullSpectrumError::Json {
            path: "canonical print plan".into(),
            source,
        })?;
    let plan_fingerprint = format!("{:x}", Sha256::digest(plan_bytes));
    let units = input
        .scopes
        .iter()
        .flat_map(|scope| {
            scope
                .units
                .iter()
                .map(move |unit| ((scope.id.as_str(), unit.id.as_str()), unit))
        })
        .collect::<BTreeMap<_, _>>();
    let jobs = result
        .jobs
        .iter()
        .map(|job| (job.id.as_str(), job))
        .collect::<BTreeMap<_, _>>();
    let plates = result
        .plates
        .iter()
        .map(|plate| (plate.id.as_str(), plate))
        .collect::<BTreeMap<_, _>>();
    let mut artifacts = Vec::new();

    for batch in result.batches.iter().filter(|batch| {
        batch.printer == Printer::U1 && batch.strategy == ColorStrategy::CmyxFullSpectrum
    }) {
        artifacts.push(prepare_full_spectrum_artifact(
            batch, input, &units, &jobs, &plates, support,
        )?);
    }
    if artifacts.is_empty() {
        return Err(U1FullSpectrumError::Plan(
            "the plan contains no U1 CMY+X Full Spectrum batches".into(),
        ));
    }
    artifacts.sort_by(|left, right| left.batch_id.cmp(&right.batch_id));
    Ok(U1FullSpectrumPreparation {
        schema_version: U1_FULL_SPECTRUM_SCHEMA_VERSION,
        adapter_id: U1_FULL_SPECTRUM_ADAPTER_ID.to_owned(),
        plan_fingerprint,
        production_qualified: false,
        artifacts,
        warnings: vec![
            "Full Spectrum color predictions use nominal spool colors unless a measured calibration exists for the exact physical CMY+X loadout. Verify the generated mapping in Snapmaker Orca before slicing."
                .to_owned(),
        ],
    })
}

#[allow(clippy::too_many_arguments)]
fn prepare_full_spectrum_artifact(
    batch: &PlannedBatch,
    input: &PlanningInput,
    units: &BTreeMap<(&str, &str), &u1_planner::PrintableUnit>,
    jobs: &BTreeMap<&str, &PlannedJob>,
    plates: &BTreeMap<&str, &PlannedPlate>,
    support: &SupportInformation,
) -> Result<U1FullSpectrumPreparedArtifact, U1FullSpectrumError> {
    let PrinterLoadout::U1 { loadout } = &batch.loadout else {
        return Err(U1FullSpectrumError::Plan(format!(
            "Full Spectrum batch {} has a non-U1 loadout",
            batch.id
        )));
    };
    let loadout = resolve_full_spectrum_loadout(loadout, input)?;
    let calibration_fingerprint = calibration_fingerprint(&loadout)?;
    let t4_color = Some(loadout[3].color);
    let batch_jobs = batch
        .job_ids
        .iter()
        .map(|id| {
            jobs.get(id.as_str()).copied().ok_or_else(|| {
                U1FullSpectrumError::Plan(format!("batch {} references missing job {id}", batch.id))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let scopes = input
        .scopes
        .iter()
        .map(|scope| (scope.id.as_str(), scope))
        .collect::<BTreeMap<_, _>>();
    let mut dedicated_support: Option<DedicatedSupportMaterial> = None;
    let mut support_policy_seen = false;
    for scope_id in batch_jobs.iter().flat_map(|job| &job.scope_ids) {
        let scope = scopes.get(scope_id.as_str()).copied().ok_or_else(|| {
            U1FullSpectrumError::Plan(format!(
                "batch {} references missing scope {scope_id}",
                batch.id
            ))
        })?;
        if support_policy_seen && scope.dedicated_support != dedicated_support {
            return Err(U1FullSpectrumError::Plan(format!(
                "batch {} contains different dedicated-support policies",
                batch.id
            )));
        }
        dedicated_support = scope.dedicated_support.clone();
        support_policy_seen = true;
    }
    if let Some(dedicated) = &dedicated_support
        && (dedicated.toolhead != Toolhead::T4
            || loadout[3].spool_id != dedicated.spool_id
            || loadout[3].material != Material::Pva)
    {
        return Err(U1FullSpectrumError::Plan(format!(
            "batch {} does not reserve its dedicated PVA support spool in T4",
            batch.id
        )));
    }
    for job in &batch_jobs {
        if job.printer != Printer::U1 || job.strategy != ColorStrategy::CmyxFullSpectrum {
            return Err(U1FullSpectrumError::Plan(format!(
                "batch {} contains incompatible job {}",
                batch.id, job.id
            )));
        }
        if job.loadout != batch.loadout {
            return Err(U1FullSpectrumError::Plan(format!(
                "job {} loadout differs from batch {}",
                job.id, batch.id
            )));
        }
        let material_contract_valid = job.printable_materials.iter().all(|material| {
            material == &Material::Pla
                || (dedicated_support.is_some() && material == &Material::Pva)
        });
        if !material_contract_valid {
            return Err(U1FullSpectrumError::Plan(format!(
                "job {} contains a non-PLA printable material",
                job.id
            )));
        }
    }
    let mixed_processes = batch_jobs
        .iter()
        .flat_map(|job| job.full_spectrum_process.iter())
        .cloned()
        .collect::<BTreeSet<_>>();
    if mixed_processes.len() > 1 {
        return Err(U1FullSpectrumError::Plan(format!(
            "batch {} contains multiple Full Spectrum process contracts",
            batch.id
        )));
    }
    let process = mixed_processes
        .into_iter()
        .next()
        .unwrap_or_else(u1_full_spectrum_process_contract);
    validate_exact_process(&process)
        .map_err(|error| U1FullSpectrumError::Plan(format!("batch {}: {error}", batch.id)))?;

    let mut recipe_inputs = Vec::new();
    let mut mapping_keys = BTreeSet::new();
    for job in &batch_jobs {
        for mapping in &job.color_mappings {
            let logical_id = mapping_logical_id(job, mapping);
            if !mapping_keys.insert(logical_id.clone()) {
                return Err(U1FullSpectrumError::Plan(format!(
                    "duplicate Full Spectrum mapping identity {logical_id:?}"
                )));
            }
            if mapping.strategy != ColorStrategy::CmyxFullSpectrum {
                return Err(U1FullSpectrumError::Plan(format!(
                    "job {} contains a non-CMY+X mapping",
                    job.id
                )));
            }
            let mixes_t4 = matches!(
                &mapping.cmyx_comparison.recipe,
                CmyxRecipe::FullSpectrum { sequence, .. }
                    if sequence.contains(&Toolhead::T4)
            ) || matches!(
                &mapping.cmyx_comparison.recipe,
                CmyxRecipe::Fallback { physical_slots, .. }
                    if physical_slots.contains(&Toolhead::T4)
            );
            if mixes_t4
                && loadout[3].profile != FULL_SPECTRUM_PROFILE_NAME
                && !matches!(
                    mapping.cmyx_comparison.confidence,
                    ColorConfidence::Calibrated | ColorConfidence::Measured
                )
            {
                return Err(U1FullSpectrumError::Plan(format!(
                    "mapping {logical_id} mixes heterogeneous T4 spool {} without an exact measured calibration. Solid T4 regions are supported, but mixing this spool requires a matching calibration sample",
                    loadout[3].spool_id
                )));
            }
            if let CmyxRecipe::FullSpectrum { .. } = &mapping.cmyx_comparison.recipe
                && mapping.cmyx_comparison.process_compatibility.as_ref() != Some(&process)
            {
                return Err(U1FullSpectrumError::Plan(format!(
                    "mapping {logical_id} does not carry the exact batch process contract"
                )));
            }
            let target_material = mapping.actual_material.clone().ok_or_else(|| {
                U1FullSpectrumError::Plan(format!(
                    "mapping {logical_id} has no resolved target material"
                ))
            })?;
            recipe_inputs.push(U1FullSpectrumRecipeInput {
                logical_id,
                target_material,
                target_color: mapping.source_color,
                recipe: mapping.cmyx_comparison.recipe.clone(),
                calibration_sample_id: mapping.cmyx_comparison.calibration_sample_id.clone(),
                predicted_color: mapping.cmyx_comparison.predicted_color,
                confidence: mapping.cmyx_comparison.confidence,
            });
        }
    }
    recipe_inputs.sort_by(|left, right| left.logical_id.cmp(&right.logical_id));
    let recipe_table = compile_u1_full_spectrum_recipes(
        &recipe_inputs,
        &U1FullSpectrumCompileContext {
            calibration_fingerprint: calibration_fingerprint.clone(),
            process: process.clone(),
            t4_color,
        },
    )?;
    let target_by_logical_id = recipe_table
        .targets
        .iter()
        .map(|target| (target.logical_id.as_str(), target))
        .collect::<BTreeMap<_, _>>();
    let mut assignments_by_logical = BTreeMap::new();
    for job in &batch_jobs {
        for mapping in &job.color_mappings {
            let logical_id = mapping_logical_id(job, mapping);
            let target = target_by_logical_id
                .get(logical_id.as_str())
                .copied()
                .ok_or_else(|| {
                    U1FullSpectrumError::Plan(format!(
                        "compiled target for mapping {logical_id} is missing"
                    ))
                })?;
            let mut source_slots = mapping
                .source_slots
                .iter()
                .map(|slot| parse_source_slot(slot))
                .collect::<Result<Vec<_>, _>>()?;
            source_slots.sort();
            source_slots.dedup();
            let generated_pva_support = mapping.source_material == Material::Pva
                && matches!(mapping.cmyx_comparison.recipe, CmyxRecipe::DedicatedT4)
                && dedicated_support.is_some();
            if source_slots.is_empty() && !generated_pva_support {
                return Err(U1FullSpectrumError::Plan(format!(
                    "mapping {logical_id} has no source slots"
                )));
            }
            let assignment = U1FullSpectrumPreparedAssignment {
                scope_id: mapping.scope_id.clone(),
                source_requirement_ids: mapping.source_requirement_ids.clone(),
                source_slots,
                source_material: mapping.source_material.clone(),
                source_color: mapping.source_color,
                target_filament_id: target.target_filament_id,
                recipe_fingerprint: target.recipe_fingerprint.clone(),
                calibration_sample_id: target.calibration_sample_id.clone(),
                predicted_color: mapping.cmyx_comparison.predicted_color,
            };
            if assignments_by_logical
                .insert(logical_id.clone(), assignment)
                .is_some()
            {
                return Err(U1FullSpectrumError::Plan(format!(
                    "duplicate prepared Full Spectrum assignment {logical_id:?}"
                )));
            }
        }
    }
    let mut assignments = assignments_by_logical.values().cloned().collect::<Vec<_>>();
    assignments.sort_by(|left, right| {
        left.scope_id.cmp(&right.scope_id).then_with(|| {
            left.source_requirement_ids
                .cmp(&right.source_requirement_ids)
        })
    });
    let recipe_calibration_sample_ids = assignments
        .iter()
        .filter_map(|assignment| assignment.calibration_sample_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let prepared_assignment_by_logical = assignments_by_logical
        .iter()
        .map(|(logical_id, assignment)| (logical_id.clone(), assignment))
        .collect::<BTreeMap<_, _>>();

    let mut prepared_plates = Vec::new();
    let mut sorted_plate_ids = batch.plate_ids.clone();
    sorted_plate_ids.sort();
    for (index, plate_id) in sorted_plate_ids.iter().enumerate() {
        let plate = plates.get(plate_id.as_str()).copied().ok_or_else(|| {
            U1FullSpectrumError::Plan(format!(
                "batch {} references missing plate {plate_id}",
                batch.id
            ))
        })?;
        let job = jobs.get(plate.job_id.as_str()).copied().ok_or_else(|| {
            U1FullSpectrumError::Plan(format!(
                "plate {} references missing job {}",
                plate.id, plate.job_id
            ))
        })?;
        if !batch.job_ids.contains(&job.id) {
            return Err(U1FullSpectrumError::Plan(format!(
                "plate {} belongs to a job outside batch {}",
                plate.id, batch.id
            )));
        }
        prepared_plates.push(prepare_full_spectrum_plate(
            plate,
            job,
            u32::try_from(index + 1)
                .map_err(|_| U1FullSpectrumError::Plan("target plate count exceeds u32".into()))?,
            units,
            &prepared_assignment_by_logical,
            &recipe_table,
        )?);
    }
    if prepared_plates.is_empty() {
        return Err(U1FullSpectrumError::Plan(format!(
            "batch {} contains no target plates",
            batch.id
        )));
    }
    Ok(U1FullSpectrumPreparedArtifact {
        batch_id: batch.id.clone(),
        file_name: format!("{}-full-spectrum.3mf", safe_file_stem(&batch.id)),
        loadout,
        calibration_fingerprint,
        process,
        support: SupportInformation {
            enabled: dedicated_support.as_ref().map(|_| true).or(support.enabled),
            ..support.clone()
        },
        dedicated_support,
        recipe_table,
        recipe_calibration_sample_ids,
        assignments,
        plates: prepared_plates,
    })
}

fn resolve_full_spectrum_loadout(
    loadout: &U1Loadout,
    input: &PlanningInput,
) -> Result<[U1FullSpectrumPhysicalSlot; 4], U1FullSpectrumError> {
    let expected_cmy = [
        input.config.cmy_setup.cyan_spool_id.as_str(),
        input.config.cmy_setup.magenta_spool_id.as_str(),
        input.config.cmy_setup.yellow_spool_id.as_str(),
    ];
    for (index, expected) in expected_cmy.iter().enumerate() {
        if loadout.slots[index].as_deref() != Some(*expected) {
            return Err(U1FullSpectrumError::Plan(format!(
                "CMY slot T{} does not match the configured fixed spool",
                index + 1
            )));
        }
    }
    let wildcard_t4 = loadout.slots[3].is_none();
    let resolved_t4 = loadout.slots[3]
        .clone()
        .or_else(|| match &input.current_toolheads.slots[3] {
            ToolheadSlotState::Loaded(spool_id) => Some(spool_id.clone()),
            ToolheadSlotState::Unknown | ToolheadSlotState::Empty => None,
        })
        .or_else(|| input.config.cmy_setup.default_t4_spool_id.clone())
        .ok_or_else(|| {
            U1FullSpectrumError::Plan(
                "T4 is a planner wildcard, but no loaded or default T4 spool can be proven".into(),
            )
        })?;
    let spool_ids = [
        loadout.slots[0].clone().expect("validated T1"),
        loadout.slots[1].clone().expect("validated T2"),
        loadout.slots[2].clone().expect("validated T3"),
        resolved_t4,
    ];
    resolve_u1_full_spectrum_physical_loadout(&spool_ids, input.inventory.as_slice(), wildcard_t4)
}

/// Resolves an explicitly confirmed T1-T4 calibration loadout with the same
/// physical profile rules used by production Full Spectrum planning.
///
/// This helper does not infer CMY roles. Callers must bind T1-T3 to their
/// configured Cyan, Magenta, and Yellow spool identities before calling it.
pub fn resolve_u1_full_spectrum_physical_loadout(
    spool_ids: &[String; 4],
    inventory: &[Spool],
    wildcard_t4: bool,
) -> Result<[U1FullSpectrumPhysicalSlot; 4], U1FullSpectrumError> {
    if spool_ids.iter().collect::<BTreeSet<_>>().len() != 4 {
        return Err(U1FullSpectrumError::Plan(
            "each U1 physical toolhead must reference a distinct spool identity".into(),
        ));
    }
    let inventory = inventory
        .iter()
        .map(|spool| (spool.id.as_str(), spool))
        .collect::<BTreeMap<_, _>>();
    let slots = spool_ids
        .iter()
        .enumerate()
        .map(|(index, spool_id)| {
            let spool = inventory.get(spool_id.as_str()).copied().ok_or_else(|| {
                U1FullSpectrumError::Plan(format!(
                    "physical loadout references missing spool {spool_id}"
                ))
            })?;
            if !spool.available {
                return Err(U1FullSpectrumError::Plan(format!(
                    "physical spool {} is out of stock",
                    spool.display_name
                )));
            }
            if spool.material != Material::Pla
                && !(index == Toolhead::T4.index() && spool.material == Material::Pva)
            {
                return Err(U1FullSpectrumError::Plan(format!(
                    "physical spool {} is {:?}; the exact Full Spectrum profile is PLA only",
                    spool.display_name, spool.material
                )));
            }
            let (profile, setting_id, filament_id) = full_spectrum_physical_profile(index, spool)?;
            Ok(U1FullSpectrumPhysicalSlot {
                toolhead: Toolhead::ALL[index],
                spool_id: spool.id.clone(),
                spool_name: spool.display_name.clone(),
                material: spool.material.clone(),
                color: spool.actual_color(),
                profile: profile.to_owned(),
                setting_id: setting_id.to_owned(),
                filament_id: filament_id.to_owned(),
                wildcard_resolved: index == 3 && wildcard_t4,
            })
        })
        .collect::<Result<Vec<_>, U1FullSpectrumError>>()?;
    slots.try_into().map_err(|_| {
        U1FullSpectrumError::Plan("Full Spectrum loadout must contain four slots".into())
    })
}

fn full_spectrum_physical_profile(
    slot_index: usize,
    spool: &Spool,
) -> Result<(&'static str, &'static str, &'static str), U1FullSpectrumError> {
    let requested = spool
        .profile_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if spool.material == Material::Pva {
        if slot_index != 3 {
            return Err(U1FullSpectrumError::Plan(format!(
                "physical PVA spool {} may only occupy T4 in Full Spectrum mode",
                spool.display_name
            )));
        }
        return match requested {
            Some(SNAPMAKER_PVA_PROFILE_NAME) => Ok((
                SNAPMAKER_PVA_PROFILE_NAME,
                SNAPMAKER_PVA_SETTING_ID,
                SNAPMAKER_PVA_FILAMENT_ID,
            )),
            Some(RELI3D_PVA_PROFILE_NAME) => Ok((
                RELI3D_PVA_PROFILE_NAME,
                RELI3D_PVA_SETTING_ID,
                RELI3D_PVA_FILAMENT_ID,
            )),
            Some(profile) => Err(U1FullSpectrumError::Plan(format!(
                "physical T4 PVA spool {} requests unqualified profile {profile:?}; choose Reli3D PVA @U1 or Snapmaker PVA @U1",
                spool.display_name
            ))),
            None => Err(U1FullSpectrumError::Plan(format!(
                "physical T4 PVA spool {} has no qualified profile; select Reli3D PVA @U1 or Snapmaker PVA @U1 in Filament Library",
                spool.display_name
            ))),
        };
    }
    if slot_index < 3
        || spool.id == "panchroma-translucent-grey"
        || requested == Some(FULL_SPECTRUM_PROFILE_NAME)
    {
        return Ok((
            FULL_SPECTRUM_PROFILE_NAME,
            FULL_SPECTRUM_SETTING_ID,
            "1417031127011",
        ));
    }
    match requested {
        Some(GENERIC_PLA_PROFILE_NAME) => Ok((
            GENERIC_PLA_PROFILE_NAME,
            GENERIC_PLA_SETTING_ID,
            GENERIC_PLA_FILAMENT_ID,
        )),
        Some(POLYMAKER_PLA_PROFILE_NAME) => Ok((
            POLYMAKER_PLA_PROFILE_NAME,
            POLYMAKER_PLA_SETTING_ID,
            POLYMAKER_PLA_FILAMENT_ID,
        )),
        Some(profile) => Err(U1FullSpectrumError::Plan(format!(
            "physical T4 spool {} requests unqualified heterogeneous profile {profile:?}; choose Generic PLA, Polymaker General PLA Family @U1, or the exact Full Spectrum profile",
            spool.display_name
        ))),
        None if spool
            .display_name
            .to_ascii_lowercase()
            .contains("polymaker") =>
        {
            Ok((
                POLYMAKER_PLA_PROFILE_NAME,
                POLYMAKER_PLA_SETTING_ID,
                POLYMAKER_PLA_FILAMENT_ID,
            ))
        }
        None => Ok((
            GENERIC_PLA_PROFILE_NAME,
            GENERIC_PLA_SETTING_ID,
            GENERIC_PLA_FILAMENT_ID,
        )),
    }
}

fn calibration_fingerprint(
    loadout: &[U1FullSpectrumPhysicalSlot; 4],
) -> Result<String, U1FullSpectrumError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct CalibrationIdentity<'a> {
        schema_version: u32,
        adapter_id: &'a str,
        process: FullSpectrumProcessCompatibility,
        slots: &'a [U1FullSpectrumPhysicalSlot; 4],
    }
    let bytes = serde_json::to_vec(&CalibrationIdentity {
        schema_version: U1_FULL_SPECTRUM_SCHEMA_VERSION,
        adapter_id: U1_FULL_SPECTRUM_ADAPTER_ID,
        process: u1_full_spectrum_process_contract(),
        slots: loadout,
    })
    .map_err(|source| U1FullSpectrumError::Json {
        path: "Full Spectrum calibration identity".into(),
        source,
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// Returns the exact Full Spectrum identity bound to the four physical slots
/// and the versioned process contract. Calibration-chart manifests use this
/// value to distinguish two physically different CMY+X loadouts even when
/// their nominal RGB values happen to match.
pub fn u1_full_spectrum_calibration_fingerprint(
    loadout: &[U1FullSpectrumPhysicalSlot; 4],
) -> Result<String, U1FullSpectrumError> {
    calibration_fingerprint(loadout)
}

fn mapping_logical_id(job: &PlannedJob, mapping: &SourceToActualMapping) -> String {
    format!(
        "{}|{}|{}",
        job.id,
        mapping.scope_id,
        mapping.source_requirement_ids.join("+")
    )
}

fn prepare_full_spectrum_plate(
    plate: &PlannedPlate,
    job: &PlannedJob,
    target_plate_id: u32,
    units: &BTreeMap<(&str, &str), &u1_planner::PrintableUnit>,
    assignments: &BTreeMap<String, &U1FullSpectrumPreparedAssignment>,
    recipe_table: &U1FullSpectrumRecipeTable,
) -> Result<U1FullSpectrumPreparedPlate, U1FullSpectrumError> {
    if plate.printer != Printer::U1
        || plate.packing_status != PackingStatus::PackedAabb
        || !plate.individual_bounds_validated
    {
        return Err(U1FullSpectrumError::Plan(format!(
            "plate {} is not a fully packed U1 AABB plate",
            plate.id
        )));
    }
    if plate.full_spectrum_process != job.full_spectrum_process {
        return Err(U1FullSpectrumError::Plan(format!(
            "plate {} process contract differs from job {}",
            plate.id, job.id
        )));
    }
    let planned_units = plate.units.iter().cloned().collect::<BTreeSet<_>>();
    if planned_units.len() != plate.units.len() {
        return Err(U1FullSpectrumError::Plan(format!(
            "plate {} contains duplicate unit references",
            plate.id
        )));
    }
    let mut placements = BTreeMap::new();
    for placement in &plate.placements {
        if !placement.target_min_x_mm.is_finite() || !placement.target_min_y_mm.is_finite() {
            return Err(U1FullSpectrumError::Plan(format!(
                "plate {} contains a non-finite target placement",
                plate.id
            )));
        }
        if placements
            .insert(placement.unit.clone(), placement)
            .is_some()
        {
            return Err(U1FullSpectrumError::Plan(format!(
                "plate {} contains more than one placement for a unit",
                plate.id
            )));
        }
    }
    if placements.keys().cloned().collect::<BTreeSet<_>>() != planned_units {
        return Err(U1FullSpectrumError::Plan(format!(
            "plate {} must contain exactly one placement per unit and no extras",
            plate.id
        )));
    }
    if let Some(tower) = plate.prime_tower {
        validate_prime_tower_anchor(tower.x_mm, tower.y_mm)?;
    }
    let definition_by_id = recipe_table
        .definitions
        .iter()
        .map(|definition| (definition.virtual_filament_id, definition))
        .collect::<BTreeMap<_, _>>();
    let mut prepared_units = Vec::with_capacity(plate.units.len());
    let mut physical_ids = BTreeSet::new();
    for unit_ref in &plate.units {
        let unit = units
            .get(&(unit_ref.scope_id.as_str(), unit_ref.unit_id.as_str()))
            .copied()
            .ok_or_else(|| {
                U1FullSpectrumError::Plan(format!(
                    "plate {} references unknown unit {}/{}",
                    plate.id, unit_ref.scope_id, unit_ref.unit_id
                ))
            })?;
        if !job.units.contains(unit_ref) {
            return Err(U1FullSpectrumError::Plan(format!(
                "unit {}/{} is not part of job {}",
                unit_ref.scope_id, unit_ref.unit_id, job.id
            )));
        }
        let requirements = unit.requirement_ids.iter().collect::<BTreeSet<_>>();
        let mut source_to_target_slots = BTreeMap::new();
        let mut covered_requirements = BTreeSet::new();
        for mapping in &job.color_mappings {
            if mapping.scope_id != unit_ref.scope_id
                || !mapping
                    .source_requirement_ids
                    .iter()
                    .any(|id| requirements.contains(id))
            {
                continue;
            }
            let logical_id = mapping_logical_id(job, mapping);
            let assignment = assignments.get(&logical_id).copied().ok_or_else(|| {
                U1FullSpectrumError::Plan(format!(
                    "unit mapping {logical_id} has no prepared assignment"
                ))
            })?;
            for requirement_id in &mapping.source_requirement_ids {
                if requirements.contains(requirement_id) {
                    covered_requirements.insert(requirement_id);
                }
            }
            for source_slot in &assignment.source_slots {
                if let Some(previous) =
                    source_to_target_slots.insert(*source_slot, assignment.target_filament_id)
                    && previous != assignment.target_filament_id
                {
                    return Err(U1FullSpectrumError::Plan(format!(
                        "unit {} maps source slot F{} to conflicting target filaments",
                        unit.source_unit_id, source_slot
                    )));
                }
            }
            if assignment.target_filament_id <= U1_FULL_SPECTRUM_PHYSICAL_COUNT {
                physical_ids.insert(assignment.target_filament_id);
            } else {
                let definition = definition_by_id
                    .get(&assignment.target_filament_id)
                    .copied()
                    .ok_or_else(|| {
                        U1FullSpectrumError::Plan(format!(
                            "assignment references missing virtual filament {}",
                            assignment.target_filament_id
                        ))
                    })?;
                physical_ids.extend(
                    definition
                        .components
                        .iter()
                        .map(|component| toolhead_id(component.toolhead)),
                );
            }
        }
        if covered_requirements != requirements {
            return Err(U1FullSpectrumError::Plan(format!(
                "unit {} is not covered by every color requirement",
                unit.source_unit_id
            )));
        }
        if source_to_target_slots.is_empty() {
            return Err(U1FullSpectrumError::Plan(format!(
                "unit {} has no source-to-target filament map",
                unit.source_unit_id
            )));
        }
        let placement = placements[unit_ref];
        validate_unit_placement_against_bed_and_tower(
            unit,
            placement.target_min_x_mm,
            placement.target_min_y_mm,
            plate.prime_tower.map(|tower| (tower.x_mm, tower.y_mm)),
        )?;
        prepared_units.push(U1FullSpectrumPreparedUnit {
            unit: unit_ref.clone(),
            source_unit_id: unit.source_unit_id.clone(),
            source_object_id: unit.source_object_id,
            source_instance_id: unit.source_instance_id,
            source_model_path: unit.source_model_path.clone(),
            source_plate_id: unit.source_plate_id.clone(),
            target_min_x_mm: placement.target_min_x_mm,
            target_min_y_mm: placement.target_min_y_mm,
            source_to_target_slots,
        });
    }
    prepared_units.sort_by(|left, right| left.unit.cmp(&right.unit));
    let tower_required = physical_ids.len() > 1;
    if tower_required != plate.prime_tower.is_some() {
        return Err(U1FullSpectrumError::Plan(format!(
            "plate {} prime-tower reservation does not match its resolved physical tools",
            plate.id
        )));
    }
    let prime_tower = plate
        .prime_tower
        .map(|tower| -> Result<_, U1FullSpectrumError> {
            validate_prime_tower_anchor(tower.x_mm, tower.y_mm)?;
            Ok(U1FullSpectrumPrimeTower {
                x_mm: tower.x_mm,
                y_mm: tower.y_mm,
            })
        })
        .transpose()?;
    Ok(U1FullSpectrumPreparedPlate {
        plan_plate_id: plate.id.clone(),
        target_plate_id,
        job_id: plate.job_id.clone(),
        prime_tower,
        units: prepared_units,
    })
}

fn validate_unit_placement_against_bed_and_tower(
    unit: &u1_planner::PrintableUnit,
    target_min_x: f64,
    target_min_y: f64,
    tower: Option<(f64, f64)>,
) -> Result<(), U1FullSpectrumError> {
    if !unit.bounds.is_valid() || !unit.bounds.has_known_size() {
        return Err(U1FullSpectrumError::Plan(format!(
            "unit {} has no production AABB",
            unit.source_unit_id
        )));
    }
    let left = target_min_x - unit.bounds.clearance_x;
    let bottom = target_min_y - unit.bounds.clearance_y;
    let right = target_min_x + unit.bounds.width + unit.bounds.clearance_x;
    let top = target_min_y + unit.bounds.depth + unit.bounds.clearance_y;
    if left < U1_MIN_X_MM - GEOMETRY_EPSILON_MM
        || bottom < U1_MIN_Y_MM - GEOMETRY_EPSILON_MM
        || right > U1_MAX_X_MM + GEOMETRY_EPSILON_MM
        || top > U1_MAX_Y_MM + GEOMETRY_EPSILON_MM
    {
        return Err(U1FullSpectrumError::Plan(format!(
            "unit {} cleared AABB is outside the qualified U1 bed",
            unit.source_unit_id
        )));
    }
    if let Some((tower_x, tower_y)) = tower {
        let tower_center_x = tower_x + PRIME_TOWER_WIDTH_MM / 2.0;
        let tower_center_y = tower_y + PRIME_TOWER_DEPTH_MM / 2.0;
        let tower_left = tower_center_x - PRIME_TOWER_MAX_HALF_EXTENT_MM;
        let tower_right = tower_center_x + PRIME_TOWER_MAX_HALF_EXTENT_MM;
        let tower_bottom = tower_center_y - PRIME_TOWER_MAX_HALF_EXTENT_MM;
        let tower_top = tower_center_y + PRIME_TOWER_MAX_HALF_EXTENT_MM;
        if left < tower_right - GEOMETRY_EPSILON_MM
            && right > tower_left + GEOMETRY_EPSILON_MM
            && bottom < tower_top - GEOMETRY_EPSILON_MM
            && top > tower_bottom + GEOMETRY_EPSILON_MM
        {
            return Err(U1FullSpectrumError::Plan(format!(
                "unit {} overlaps the qualified prime-tower maximum-height cone/brim/rib envelope",
                unit.source_unit_id
            )));
        }
    }
    Ok(())
}

fn validate_prime_tower_anchor(x: f64, y: f64) -> Result<(), U1FullSpectrumError> {
    let center_x = x + PRIME_TOWER_WIDTH_MM / 2.0;
    let center_y = y + PRIME_TOWER_DEPTH_MM / 2.0;
    if !x.is_finite()
        || !y.is_finite()
        || center_x - PRIME_TOWER_MAX_HALF_EXTENT_MM < U1_MIN_X_MM - GEOMETRY_EPSILON_MM
        || center_y - PRIME_TOWER_MAX_HALF_EXTENT_MM < U1_MIN_Y_MM - GEOMETRY_EPSILON_MM
        || center_x + PRIME_TOWER_MAX_HALF_EXTENT_MM > U1_MAX_X_MM + GEOMETRY_EPSILON_MM
        || center_y + PRIME_TOWER_MAX_HALF_EXTENT_MM > U1_MAX_Y_MM + GEOMETRY_EPSILON_MM
    {
        return Err(U1FullSpectrumError::Plan(format!(
            "prime-tower maximum-height cone/brim/rib envelope at body anchor ({x}, {y}) is outside the qualified U1 build area"
        )));
    }
    Ok(())
}

fn parse_source_slot(value: &str) -> Result<u8, U1FullSpectrumError> {
    value
        .trim()
        .strip_prefix(['F', 'f'])
        .and_then(|value| value.parse::<u8>().ok())
        .filter(|value| (1..=U1_FULL_SPECTRUM_MAX_FILAMENT_ID).contains(value))
        .ok_or_else(|| U1FullSpectrumError::Plan(format!("invalid source slot {value:?}")))
}

fn safe_file_stem(value: &str) -> String {
    let value = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let value = value.trim_matches('-');
    if value.is_empty() {
        "u1".to_owned()
    } else {
        value.to_owned()
    }
}

pub fn validate_u1_full_spectrum_recipe_table(
    table: &U1FullSpectrumRecipeTable,
) -> Result<(), U1FullSpectrumError> {
    if table.schema_version != U1_FULL_SPECTRUM_SCHEMA_VERSION
        || table.physical_filament_count != U1_FULL_SPECTRUM_PHYSICAL_COUNT
    {
        return Err(U1FullSpectrumError::Recipe(
            "recipe-table version or physical filament count is unsupported".into(),
        ));
    }
    let mut stable_ids = BTreeSet::new();
    let mut fingerprints = BTreeSet::new();
    for (index, definition) in table.definitions.iter().enumerate() {
        let expected_id = U1_FULL_SPECTRUM_FIRST_VIRTUAL_ID
            + u8::try_from(index).map_err(|_| {
                U1FullSpectrumError::Recipe("virtual definition index exceeds u8".into())
            })?;
        if definition.virtual_filament_id != expected_id {
            return Err(U1FullSpectrumError::Recipe(format!(
                "virtual filament IDs must be contiguous from {}, found {} at row {}",
                U1_FULL_SPECTRUM_FIRST_VIRTUAL_ID,
                definition.virtual_filament_id,
                index + 1
            )));
        }
        if definition.recipe_fingerprint.len() != 64
            || !definition
                .recipe_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || !fingerprints.insert(definition.recipe_fingerprint.as_str())
        {
            return Err(U1FullSpectrumError::Recipe(
                "definition fingerprint is invalid or duplicated".into(),
            ));
        }
        if definition
            .calibration_sample_id
            .as_deref()
            .is_some_and(|sample_id| sample_id.trim().is_empty())
        {
            return Err(U1FullSpectrumError::Recipe(
                "definition calibration sample ID is empty".into(),
            ));
        }
        if definition.stable_id == 0 || !stable_ids.insert(definition.stable_id) {
            return Err(U1FullSpectrumError::Recipe(
                "native stable IDs must be positive and unique".into(),
            ));
        }
        if definition.serialized != serialize_native_definition(definition)? {
            return Err(U1FullSpectrumError::Recipe(format!(
                "virtual filament {} is not in canonical native form",
                definition.virtual_filament_id
            )));
        }
    }
    let serialized = table
        .definitions
        .iter()
        .map(|definition| definition.serialized.as_str())
        .collect::<Vec<_>>()
        .join(";");
    if serialized != table.serialized_definitions {
        return Err(U1FullSpectrumError::Recipe(
            "serialized definition string does not match the ordered recipe rows".into(),
        ));
    }
    let maximum = U1_FULL_SPECTRUM_PHYSICAL_COUNT
        + u8::try_from(table.definitions.len()).map_err(|_| {
            U1FullSpectrumError::Recipe("virtual definition count exceeds u8".into())
        })?;
    let mut target_ids = BTreeSet::new();
    for target in &table.targets {
        if target.logical_id.trim().is_empty() || !target_ids.insert(target.logical_id.as_str()) {
            return Err(U1FullSpectrumError::Recipe(
                "target logical IDs must be non-empty and unique".into(),
            ));
        }
        if target
            .calibration_sample_id
            .as_deref()
            .is_some_and(|sample_id| sample_id.trim().is_empty())
        {
            return Err(U1FullSpectrumError::Recipe(format!(
                "target {} has an empty calibration sample ID",
                target.logical_id
            )));
        }
        if target.target_filament_id == 0 || target.target_filament_id > maximum {
            return Err(U1FullSpectrumError::Recipe(format!(
                "target {} references filament {} outside 1..={maximum}",
                target.logical_id, target.target_filament_id
            )));
        }
        match (
            target.target_filament_id > U1_FULL_SPECTRUM_PHYSICAL_COUNT,
            target.recipe_fingerprint.as_deref(),
        ) {
            (false, None) => {}
            (true, Some(fingerprint))
                if table.definitions.iter().any(|definition| {
                    definition.virtual_filament_id == target.target_filament_id
                        && definition.recipe_fingerprint == fingerprint
                        && definition.calibration_sample_id == target.calibration_sample_id
                }) => {}
            _ => {
                return Err(U1FullSpectrumError::Recipe(format!(
                    "target {} has an inconsistent physical/virtual fingerprint reference",
                    target.logical_id
                )));
            }
        }
    }
    Ok(())
}

/// Parses the exact active-row grammar used by Snapmaker Orca 2.3.6. The
/// parser intentionally rejects disabled/deleted rows, legacy token order, and
/// non-canonical numeric spellings so a candidate cannot silently acquire a
/// different virtual-ID ordering when reopened by the target slicer.
pub fn parse_u1_full_spectrum_definitions(
    serialized: &str,
) -> Result<Vec<U1FullSpectrumNativeDefinition>, U1FullSpectrumError> {
    if serialized.is_empty() {
        return Ok(Vec::new());
    }
    let rows = serialized.split(';').collect::<Vec<_>>();
    let maximum_rows =
        usize::from(U1_FULL_SPECTRUM_MAX_FILAMENT_ID - U1_FULL_SPECTRUM_PHYSICAL_COUNT);
    if rows.len() > maximum_rows || rows.iter().any(|row| row.is_empty()) {
        return Err(U1FullSpectrumError::Recipe(format!(
            "native definition count must be 1 through {maximum_rows} with no empty rows"
        )));
    }
    let mut stable_ids = BTreeSet::new();
    rows.into_iter()
        .enumerate()
        .map(|(index, row)| {
            let virtual_filament_id = U1_FULL_SPECTRUM_FIRST_VIRTUAL_ID
                + u8::try_from(index).map_err(|_| {
                    U1FullSpectrumError::Recipe(
                        "native definition index exceeds the adapter limit".into(),
                    )
                })?;
            let definition = parse_native_definition_row(virtual_filament_id, row)?;
            if !stable_ids.insert(definition.stable_id) {
                return Err(U1FullSpectrumError::Recipe(format!(
                    "native stable ID {} is duplicated",
                    definition.stable_id
                )));
            }
            Ok(definition)
        })
        .collect()
}

fn parse_native_definition_row(
    virtual_filament_id: u8,
    row: &str,
) -> Result<U1FullSpectrumNativeDefinition, U1FullSpectrumError> {
    let fields = row.split(',').collect::<Vec<_>>();
    if !(16..=18).contains(&fields.len()) {
        return Err(U1FullSpectrumError::Recipe(format!(
            "virtual filament {virtual_filament_id} has {} fields; expected 16 through 18",
            fields.len()
        )));
    }
    let component_a = parse_plain_u8(fields[0], "component A")?;
    let component_b = parse_plain_u8(fields[1], "component B")?;
    if !(1..=4).contains(&component_a)
        || !(1..=4).contains(&component_b)
        || component_a == component_b
    {
        return Err(U1FullSpectrumError::Recipe(format!(
            "virtual filament {virtual_filament_id} has invalid physical components"
        )));
    }
    if fields[2] != "1" || fields[3] != "1" || fields[5] != "0" {
        return Err(U1FullSpectrumError::Recipe(format!(
            "virtual filament {virtual_filament_id} must be enabled, custom, and non-pointillist"
        )));
    }
    let mix_b_percent = parse_plain_u8(fields[4], "mix B percent")?;
    if mix_b_percent > 100 {
        return Err(U1FullSpectrumError::Recipe(
            "mix B percent exceeds 100".into(),
        ));
    }
    let gradient_component_ids = parse_compact_physical_ids(
        fields[6]
            .strip_prefix('g')
            .ok_or_else(|| U1FullSpectrumError::Recipe("missing g field".into()))?,
    )?;
    let gradient_component_weights = parse_weight_list(
        fields[7]
            .strip_prefix('w')
            .ok_or_else(|| U1FullSpectrumError::Recipe("missing w field".into()))?,
    )?;
    if gradient_component_ids.is_empty() != gradient_component_weights.is_empty()
        || (!gradient_component_ids.is_empty()
            && (gradient_component_ids.len() != gradient_component_weights.len()
                || gradient_component_weights
                    .iter()
                    .map(|value| u16::from(*value))
                    .sum::<u16>()
                    != 100))
    {
        return Err(U1FullSpectrumError::Recipe(format!(
            "virtual filament {virtual_filament_id} has inconsistent g/w fields"
        )));
    }
    let distribution = match fields[8] {
        "m0" => U1FullSpectrumNativeDistribution::LayerCycle,
        "m2" => U1FullSpectrumNativeDistribution::Simple,
        _ => {
            return Err(U1FullSpectrumError::Recipe(format!(
                "virtual filament {virtual_filament_id} has unsupported distribution {}",
                fields[8]
            )));
        }
    };
    let local_z_max_sublayers = parse_prefixed_u8(fields[9], "z", "local-Z sublayers")?;
    if fields[10] != "xa0" || fields[11] != "xb0" || fields[12] != "d0" || fields[13] != "o0" {
        return Err(U1FullSpectrumError::Recipe(format!(
            "virtual filament {virtual_filament_id} has unsupported offsets, deletion, or origin flags"
        )));
    }
    let stable_id = fields[14]
        .strip_prefix('u')
        .ok_or_else(|| U1FullSpectrumError::Recipe("missing stable-ID field".into()))
        .and_then(|value| parse_plain_u64(value, "stable ID"))?;
    if stable_id == 0 {
        return Err(U1FullSpectrumError::Recipe(
            "native stable ID must be positive".into(),
        ));
    }
    let mode = match fields[15] {
        "cm0" => FullSpectrumMode::Ratio,
        "cm1" => FullSpectrumMode::Cycle,
        "cm2" => FullSpectrumMode::Match,
        "cm3" => FullSpectrumMode::Gradient,
        _ => {
            return Err(U1FullSpectrumError::Recipe(format!(
                "virtual filament {virtual_filament_id} has unsupported UI mode {}",
                fields[15]
            )));
        }
    };
    let mut gradient_start_millionths = None;
    let mut gradient_end_millionths = None;
    let mut manual_pattern = None;
    for field in &fields[16..] {
        if field.starts_with("r1/") {
            if gradient_start_millionths.is_some() {
                return Err(U1FullSpectrumError::Recipe(
                    "native definition contains duplicate gradient ranges".into(),
                ));
            }
            let (start, end) = parse_gradient_range(field)?;
            gradient_start_millionths = Some(start);
            gradient_end_millionths = Some(end);
        } else if manual_pattern.replace((*field).to_owned()).is_some() {
            return Err(U1FullSpectrumError::Recipe(
                "native definition contains duplicate manual patterns".into(),
            ));
        }
    }
    validate_native_mode_fields(
        virtual_filament_id,
        mode,
        component_a,
        component_b,
        distribution,
        &gradient_component_ids,
        &gradient_component_weights,
        manual_pattern.as_deref(),
        gradient_start_millionths,
        gradient_end_millionths,
    )?;
    let definition = U1FullSpectrumNativeDefinition {
        virtual_filament_id,
        component_a,
        component_b,
        mix_b_percent,
        distribution,
        gradient_component_ids,
        gradient_component_weights,
        local_z_max_sublayers,
        stable_id,
        mode,
        gradient_start_millionths,
        gradient_end_millionths,
        manual_pattern,
        serialized: row.to_owned(),
    };
    if serialize_parsed_native_definition(&definition) != row {
        return Err(U1FullSpectrumError::Recipe(format!(
            "virtual filament {virtual_filament_id} is not canonically serialized"
        )));
    }
    Ok(definition)
}

#[allow(clippy::too_many_arguments)]
fn validate_native_mode_fields(
    virtual_filament_id: u8,
    mode: FullSpectrumMode,
    component_a: u8,
    component_b: u8,
    distribution: U1FullSpectrumNativeDistribution,
    ids: &[u8],
    weights: &[u8],
    manual_pattern: Option<&str>,
    gradient_start: Option<u32>,
    gradient_end: Option<u32>,
) -> Result<(), U1FullSpectrumError> {
    let invalid = || {
        U1FullSpectrumError::Recipe(format!(
            "virtual filament {virtual_filament_id} has fields inconsistent with {mode:?} mode"
        ))
    };
    match mode {
        FullSpectrumMode::Cycle => {
            let pattern = manual_pattern.ok_or_else(invalid)?;
            if component_a != 1
                || component_b != 2
                || distribution != U1FullSpectrumNativeDistribution::Simple
                || !ids.is_empty()
                || !weights.is_empty()
                || gradient_start.is_some()
                || gradient_end.is_some()
                || !(2..=100).contains(&pattern.len())
                || !pattern.bytes().all(|byte| (b'1'..=b'4').contains(&byte))
                || pattern.bytes().collect::<BTreeSet<_>>().len() < 2
            {
                return Err(invalid());
            }
        }
        FullSpectrumMode::Gradient => {
            if distribution != U1FullSpectrumNativeDistribution::LayerCycle
                || !ids.is_empty()
                || !weights.is_empty()
                || manual_pattern.is_some()
                || gradient_start.is_none()
                || gradient_end.is_none()
            {
                return Err(invalid());
            }
        }
        FullSpectrumMode::Ratio => {
            validate_ratio_or_match_native_fields(
                false,
                component_a,
                component_b,
                distribution,
                ids,
                weights,
                manual_pattern,
                gradient_start,
                gradient_end,
            )
            .map_err(|_| invalid())?;
        }
        FullSpectrumMode::Match => {
            validate_ratio_or_match_native_fields(
                true,
                component_a,
                component_b,
                distribution,
                ids,
                weights,
                manual_pattern,
                gradient_start,
                gradient_end,
            )
            .map_err(|_| invalid())?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_ratio_or_match_native_fields(
    is_match: bool,
    component_a: u8,
    component_b: u8,
    distribution: U1FullSpectrumNativeDistribution,
    ids: &[u8],
    weights: &[u8],
    manual_pattern: Option<&str>,
    gradient_start: Option<u32>,
    gradient_end: Option<u32>,
) -> Result<(), ()> {
    if gradient_start.is_some() || gradient_end.is_some() {
        return Err(());
    }
    if let Some(pattern) = manual_pattern {
        let expected_ids = if is_match {
            vec![component_a, component_b]
        } else {
            Vec::new()
        };
        if distribution != U1FullSpectrumNativeDistribution::Simple
            || ids != expected_ids
            || !weights.is_empty()
            || !(2..=100).contains(&pattern.len())
            || !pattern.bytes().all(|byte| matches!(byte, b'1' | b'2'))
            || pattern.bytes().collect::<BTreeSet<_>>().len() != 2
        {
            return Err(());
        }
    } else if distribution != U1FullSpectrumNativeDistribution::LayerCycle
        || !(2..=3).contains(&ids.len())
        || ids[0] != component_a
        || ids[1] != component_b
        || ids.len() != weights.len()
    {
        return Err(());
    }
    Ok(())
}

fn serialize_parsed_native_definition(definition: &U1FullSpectrumNativeDefinition) -> String {
    let gradient_ids = definition
        .gradient_component_ids
        .iter()
        .map(u8::to_string)
        .collect::<String>();
    let gradient_weights = definition
        .gradient_component_weights
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join("/");
    let ui_mode = match definition.mode {
        FullSpectrumMode::Ratio => 0,
        FullSpectrumMode::Cycle => 1,
        FullSpectrumMode::Match => 2,
        FullSpectrumMode::Gradient => 3,
    };
    let mut fields = vec![
        definition.component_a.to_string(),
        definition.component_b.to_string(),
        "1".into(),
        "1".into(),
        definition.mix_b_percent.to_string(),
        "0".into(),
        format!("g{gradient_ids}"),
        format!("w{gradient_weights}"),
        format!("m{}", definition.distribution.native_value()),
        format!("z{}", definition.local_z_max_sublayers),
        "xa0".into(),
        "xb0".into(),
        "d0".into(),
        "o0".into(),
        format!("u{}", definition.stable_id),
        format!("cm{ui_mode}"),
    ];
    if let (Some(start), Some(end)) = (
        definition.gradient_start_millionths,
        definition.gradient_end_millionths,
    ) {
        fields.push(format!(
            "r1/{:.4}/{:.4}",
            f64::from(start) / 1_000_000.0,
            f64::from(end) / 1_000_000.0
        ));
    }
    if let Some(pattern) = &definition.manual_pattern {
        fields.push(pattern.clone());
    }
    fields.join(",")
}

fn parse_plain_u8(value: &str, label: &str) -> Result<u8, U1FullSpectrumError> {
    let parsed = value
        .parse::<u8>()
        .map_err(|_| U1FullSpectrumError::Recipe(format!("invalid {label}")))?;
    if parsed.to_string() != value {
        return Err(U1FullSpectrumError::Recipe(format!(
            "{label} is not canonically serialized"
        )));
    }
    Ok(parsed)
}

fn parse_plain_u64(value: &str, label: &str) -> Result<u64, U1FullSpectrumError> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| U1FullSpectrumError::Recipe(format!("invalid {label}")))?;
    if parsed.to_string() != value {
        return Err(U1FullSpectrumError::Recipe(format!(
            "{label} is not canonically serialized"
        )));
    }
    Ok(parsed)
}

fn parse_prefixed_u8(value: &str, prefix: &str, label: &str) -> Result<u8, U1FullSpectrumError> {
    parse_plain_u8(
        value
            .strip_prefix(prefix)
            .ok_or_else(|| U1FullSpectrumError::Recipe(format!("missing {label} field")))?,
        label,
    )
}

fn parse_compact_physical_ids(value: &str) -> Result<Vec<u8>, U1FullSpectrumError> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let values = value
        .bytes()
        .map(|byte| {
            byte.checked_sub(b'0')
                .filter(|id| (1..=4).contains(id))
                .ok_or_else(|| {
                    U1FullSpectrumError::Recipe("g field references a non-physical filament".into())
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if values.iter().copied().collect::<BTreeSet<_>>().len() != values.len() {
        return Err(U1FullSpectrumError::Recipe(
            "g field contains duplicate physical filaments".into(),
        ));
    }
    Ok(values)
}

fn parse_weight_list(value: &str) -> Result<Vec<u8>, U1FullSpectrumError> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    value
        .split('/')
        .map(|value| parse_plain_u8(value, "component weight"))
        .collect()
}

fn parse_gradient_range(value: &str) -> Result<(u32, u32), U1FullSpectrumError> {
    let values = value.split('/').collect::<Vec<_>>();
    if values.len() != 3 || values[0] != "r1" {
        return Err(U1FullSpectrumError::Recipe("invalid gradient range".into()));
    }
    let parse = |value: &str| {
        let numeric = value
            .parse::<f64>()
            .map_err(|_| U1FullSpectrumError::Recipe("invalid gradient endpoint".into()))?;
        if !numeric.is_finite() || !(0.0..=1.0).contains(&numeric) {
            return Err(U1FullSpectrumError::Recipe(
                "gradient endpoint is outside 0 through 1".into(),
            ));
        }
        Ok((numeric * 1_000_000.0).round() as u32)
    };
    Ok((parse(values[1])?, parse(values[2])?))
}

fn validate_project_settings_map(
    settings: &BTreeMap<String, Value>,
    artifact: Option<&U1FullSpectrumPreparedArtifact>,
) -> Result<Vec<U1FullSpectrumNativeDefinition>, U1FullSpectrumError> {
    for (key, expected) in [
        ("name", "project_settings"),
        ("from", "project"),
        ("version", SUPPORTED_ORCA_VERSION),
        ("printer_model", "Snapmaker U1"),
        ("printer_variant", "0.4"),
        ("printer_settings_id", U1_MACHINE_PROFILE_NAME),
        ("print_settings_id", FULL_SPECTRUM_PROCESS_PROFILE_NAME),
        ("layer_height", "0.08"),
        ("enable_prime_tower", "1"),
        ("prime_tower_width", "30"),
        ("prime_volume", "18"),
        ("prime_tower_brim_width", "5"),
        ("wipe_tower_cone_angle", "15"),
        ("wipe_tower_extra_spacing", "120%"),
        ("wipe_tower_extra_rib_length", "8"),
        ("wipe_tower_wall_type", "rib"),
        ("wipe_tower_filament", "0"),
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
        ("print_sequence", "by layer"),
        ("spiral_mode", "0"),
    ] {
        require_setting_string(settings, key, expected)?;
    }
    if settings.contains_key("wipe_tower_rotation_angle") {
        require_setting_string(settings, "wipe_tower_rotation_angle", "0")?;
    }
    for (key, length) in [
        ("nozzle_diameter", 4),
        ("filament_settings_id", 4),
        ("filament_ids", 4),
        ("filament_colour", 4),
        ("default_filament_colour", 4),
        ("filament_type", 4),
        ("flush_volumes_matrix", 16),
        ("flush_volumes_vector", 8),
    ] {
        require_setting_array_len(settings, key, length)?;
    }
    require_setting_string_array(settings, "nozzle_diameter", std::iter::repeat_n("0.4", 4))?;
    validate_full_spectrum_physical_profile_arrays(settings, artifact)?;
    if let Some(artifact) = artifact {
        require_setting_string_array(
            settings,
            "filament_type",
            artifact.loadout.iter().map(|slot| match slot.material {
                Material::Pla => "PLA",
                Material::Pva => "PVA",
                _ => "unsupported",
            }),
        )?;
    } else {
        let profiles = setting_string_array(settings, "filament_settings_id")?;
        let setting_ids = setting_string_array(settings, "filament_ids")?;
        let t4_material = if matches!(
            (profiles.get(3).copied(), setting_ids.get(3).copied()),
            (
                Some(SNAPMAKER_PVA_PROFILE_NAME),
                Some(SNAPMAKER_PVA_SETTING_ID)
            ) | (Some(RELI3D_PVA_PROFILE_NAME), Some(RELI3D_PVA_SETTING_ID))
        ) {
            "PVA"
        } else {
            "PLA"
        };
        require_setting_string_array(
            settings,
            "filament_type",
            ["PLA", "PLA", "PLA", t4_material],
        )?;
    }
    for key in ["filament_colour", "default_filament_colour"] {
        let colors = setting_string_array(settings, key)?;
        if colors.iter().any(|color| !is_canonical_rgb_hex(color)) {
            return Err(U1FullSpectrumError::SemanticValidation(format!(
                "project setting {key} contains a non-canonical RGB color"
            )));
        }
    }
    let serialized = setting_string(settings, "mixed_filament_definitions")?;
    let definitions = parse_u1_full_spectrum_definitions(serialized).map_err(|error| {
        U1FullSpectrumError::SemanticValidation(format!(
            "invalid mixed_filament_definitions: {error}"
        ))
    })?;
    if definitions
        .iter()
        .any(|definition| definition.local_z_max_sublayers != SUBDIVISION_FACTOR)
    {
        return Err(U1FullSpectrumError::SemanticValidation(format!(
            "every mixed definition must use the exact z{SUBDIVISION_FACTOR} subdivision contract"
        )));
    }
    let tower_x = setting_string_array(settings, "wipe_tower_x")?;
    let tower_y = setting_string_array(settings, "wipe_tower_y")?;
    if tower_x.is_empty() || tower_x.len() != tower_y.len() {
        return Err(U1FullSpectrumError::SemanticValidation(
            "wipe-tower coordinate arrays must have equal non-zero cardinality".into(),
        ));
    }
    for (x, y) in tower_x.iter().zip(&tower_y) {
        let x = parse_setting_number(x, "wipe_tower_x")?;
        let y = parse_setting_number(y, "wipe_tower_y")?;
        if (x == 0.0) != (y == 0.0) {
            return Err(U1FullSpectrumError::SemanticValidation(
                "wipe-tower coordinates must both be zero or both describe a planned tower".into(),
            ));
        }
        if x != 0.0 {
            validate_prime_tower_anchor(x, y)
                .map_err(|error| U1FullSpectrumError::SemanticValidation(error.to_string()))?;
        }
    }
    if let Some(artifact) = artifact {
        validate_source_support_intent(
            settings,
            &artifact.support,
            artifact.dedicated_support.as_ref(),
        )?;
        if let Some(dedicated_support) = &artifact.dedicated_support {
            let slot = (dedicated_support.toolhead.index() + 1).to_string();
            let expected_body = match dedicated_support.usage {
                SupportMaterialUsage::BodyAndInterface => slot.as_str(),
                SupportMaterialUsage::InterfaceOnly => "0",
            };
            if setting_string(settings, "enable_support")? != "1"
                || setting_string(settings, "support_filament")? != expected_body
                || setting_string(settings, "support_interface_filament")? != slot
            {
                return Err(U1FullSpectrumError::SemanticValidation(
                    "project does not preserve the dedicated PVA support contract".into(),
                ));
            }
            for (key, expected) in [
                ("support_top_z_distance", "0"),
                ("support_bottom_z_distance", "0"),
                ("support_interface_top_layers", "3"),
                ("support_interface_spacing", "0.2"),
                ("support_interface_speed", "30"),
            ] {
                require_setting_string(settings, key, expected)?;
            }
        }
        if definitions.len() != artifact.recipe_table.definitions.len()
            || definitions
                .iter()
                .zip(&artifact.recipe_table.definitions)
                .any(|(actual, expected)| {
                    actual.virtual_filament_id != expected.virtual_filament_id
                        || actual.stable_id != expected.stable_id
                        || actual.serialized != expected.serialized
                })
        {
            return Err(U1FullSpectrumError::SemanticValidation(
                "native mixed definitions do not match the prepared artifact".into(),
            ));
        }
        if tower_x.len() != artifact.plates.len() {
            return Err(U1FullSpectrumError::SemanticValidation(
                "wipe-tower coordinate count does not match the prepared plate count".into(),
            ));
        }
        require_setting_string_array(
            settings,
            "filament_colour",
            artifact.loadout.iter().map(|slot| rgb_hex(slot.color)),
        )?;
        require_setting_string_array(
            settings,
            "default_filament_colour",
            artifact.loadout.iter().map(|slot| rgb_hex(slot.color)),
        )?;
        require_setting_string_array(
            settings,
            "wipe_tower_x",
            artifact.plates.iter().map(|plate| {
                plate
                    .prime_tower
                    .map_or_else(|| "0".to_owned(), |tower| format_number(tower.x_mm))
            }),
        )?;
        require_setting_string_array(
            settings,
            "wipe_tower_y",
            artifact.plates.iter().map(|plate| {
                plate
                    .prime_tower
                    .map_or_else(|| "0".to_owned(), |tower| format_number(tower.y_mm))
            }),
        )?;
    }
    Ok(definitions)
}

fn validate_source_support_intent(
    settings: &BTreeMap<String, Value>,
    support: &SupportInformation,
    dedicated_support: Option<&DedicatedSupportMaterial>,
) -> Result<(), U1FullSpectrumError> {
    let mut expected_keys = Vec::new();
    let require = |key: &str, expected: String| {
        if settings.get(key).and_then(Value::as_str) == Some(expected.as_str()) {
            Ok(())
        } else {
            Err(U1FullSpectrumError::SemanticValidation(format!(
                "project setting {key} does not preserve source support intent"
            )))
        }
    };
    if let Some(enabled) = support.enabled {
        expected_keys.push("enable_support");
        require("enable_support", if enabled { "1" } else { "0" }.into())?;
    }
    if let Some(support_type) = support.support_type {
        expected_keys.push("support_type");
        require("support_type", support_type.slicer_value().into())?;
    }
    if let Some(angle) = support.threshold_angle_degrees {
        expected_keys.push("support_threshold_angle");
        require("support_threshold_angle", angle.to_string())?;
    }
    if let Some(on_build_plate_only) = support.on_build_plate_only {
        expected_keys.push("support_on_build_plate_only");
        require(
            "support_on_build_plate_only",
            if on_build_plate_only { "1" } else { "0" }.into(),
        )?;
    }
    if dedicated_support.is_some() {
        expected_keys.extend([
            "support_filament",
            "support_interface_filament",
            "support_top_z_distance",
            "support_bottom_z_distance",
            "support_interface_top_layers",
            "support_interface_spacing",
            "support_interface_speed",
        ]);
    }
    if !expected_keys.is_empty() {
        expected_keys.sort_unstable();
        let groups = settings
            .get("different_settings_to_system")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                U1FullSpectrumError::SemanticValidation(
                    "project does not declare its source support overrides".into(),
                )
            })?;
        if groups.len() != 6
            || groups.first().and_then(Value::as_str) != Some(expected_keys.join(";").as_str())
            || groups
                .iter()
                .skip(1)
                .any(|value| value.as_str() != Some(""))
        {
            return Err(U1FullSpectrumError::SemanticValidation(
                "source support override declaration is not canonical".into(),
            ));
        }
    }
    Ok(())
}

fn validate_full_spectrum_physical_profile_arrays(
    settings: &BTreeMap<String, Value>,
    artifact: Option<&U1FullSpectrumPreparedArtifact>,
) -> Result<(), U1FullSpectrumError> {
    let profiles = setting_string_array(settings, "filament_settings_id")?;
    let setting_ids = setting_string_array(settings, "filament_ids")?;
    if let Some(artifact) = artifact {
        require_setting_string_array(
            settings,
            "filament_settings_id",
            artifact.loadout.iter().map(|slot| slot.profile.as_str()),
        )?;
        require_setting_string_array(
            settings,
            "filament_ids",
            artifact.loadout.iter().map(|slot| slot.setting_id.as_str()),
        )?;
        return Ok(());
    }
    let fixed_cmy_is_full_spectrum = (0..3).all(|index| {
        profiles.get(index) == Some(&FULL_SPECTRUM_PROFILE_NAME)
            && setting_ids.get(index) == Some(&FULL_SPECTRUM_SETTING_ID)
    });
    let t4_is_qualified = matches!(
        (profiles.get(3).copied(), setting_ids.get(3).copied()),
        (
            Some(FULL_SPECTRUM_PROFILE_NAME),
            Some(FULL_SPECTRUM_SETTING_ID)
        ) | (Some(GENERIC_PLA_PROFILE_NAME), Some(GENERIC_PLA_SETTING_ID))
            | (
                Some(POLYMAKER_PLA_PROFILE_NAME),
                Some(POLYMAKER_PLA_SETTING_ID)
            )
            | (
                Some(SNAPMAKER_PVA_PROFILE_NAME),
                Some(SNAPMAKER_PVA_SETTING_ID)
            )
            | (Some(RELI3D_PVA_PROFILE_NAME), Some(RELI3D_PVA_SETTING_ID))
    );
    if !fixed_cmy_is_full_spectrum || !t4_is_qualified {
        return Err(U1FullSpectrumError::SemanticValidation(
            "project physical profiles do not match the qualified CMY Full Spectrum plus solid-T4 contract"
                .into(),
        ));
    }
    Ok(())
}

fn setting_string<'a>(
    settings: &'a BTreeMap<String, Value>,
    key: &str,
) -> Result<&'a str, U1FullSpectrumError> {
    settings.get(key).and_then(Value::as_str).ok_or_else(|| {
        U1FullSpectrumError::SemanticValidation(format!(
            "project setting {key} is missing or is not a string"
        ))
    })
}

fn require_setting_string(
    settings: &BTreeMap<String, Value>,
    key: &str,
    expected: &str,
) -> Result<(), U1FullSpectrumError> {
    let actual = setting_string(settings, key)?;
    if actual != expected {
        return Err(U1FullSpectrumError::SemanticValidation(format!(
            "project setting {key} is {actual:?}; expected {expected:?}"
        )));
    }
    Ok(())
}

fn setting_string_array<'a>(
    settings: &'a BTreeMap<String, Value>,
    key: &str,
) -> Result<Vec<&'a str>, U1FullSpectrumError> {
    settings
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| {
            U1FullSpectrumError::SemanticValidation(format!(
                "project setting {key} is missing or is not an array"
            ))
        })?
        .iter()
        .map(|value| {
            value.as_str().ok_or_else(|| {
                U1FullSpectrumError::SemanticValidation(format!(
                    "project setting {key} contains a non-string value"
                ))
            })
        })
        .collect()
}

fn require_setting_array_len(
    settings: &BTreeMap<String, Value>,
    key: &str,
    expected: usize,
) -> Result<(), U1FullSpectrumError> {
    let actual = setting_string_array(settings, key)?.len();
    if actual != expected {
        return Err(U1FullSpectrumError::SemanticValidation(format!(
            "project setting {key} has {actual} entries; expected {expected}"
        )));
    }
    Ok(())
}

fn require_setting_string_array<I, S>(
    settings: &BTreeMap<String, Value>,
    key: &str,
    expected: I,
) -> Result<(), U1FullSpectrumError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let actual = setting_string_array(settings, key)?;
    let expected = expected
        .into_iter()
        .map(|value| value.as_ref().to_owned())
        .collect::<Vec<_>>();
    if actual != expected {
        return Err(U1FullSpectrumError::SemanticValidation(format!(
            "project setting {key} is not synchronized with the exact Full Spectrum contract"
        )));
    }
    Ok(())
}

fn parse_setting_number(value: &str, key: &str) -> Result<f64, U1FullSpectrumError> {
    value
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| {
            U1FullSpectrumError::SemanticValidation(format!(
                "project setting {key} contains an invalid number"
            ))
        })
}

fn is_canonical_rgb_hex(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value[1..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(&byte))
}

#[derive(Clone, Debug, Default)]
struct PlateMapContract {
    plate_id: Option<u32>,
    mode: Option<String>,
    maps: Option<String>,
    volume_maps: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct ModelSettingsContract {
    plates: Vec<PlateMapContract>,
    used_filament_ids: BTreeSet<u8>,
}

fn prepared_artifact_calibration_sample_ids(
    artifact: &U1FullSpectrumPreparedArtifact,
) -> Vec<String> {
    artifact
        .assignments
        .iter()
        .filter_map(|assignment| assignment.calibration_sample_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Validates a structural Full Spectrum qualification candidate without
/// invoking the slicer. This is deliberately a semantic pre-GUI gate: a valid
/// report does not make the adapter production-qualified.
pub fn validate_u1_full_spectrum_candidate(
    path: &Path,
    expected_artifact: Option<&U1FullSpectrumPreparedArtifact>,
) -> Result<U1FullSpectrumValidationReport, U1FullSpectrumError> {
    validate_u1_full_spectrum_candidate_cancellable(path, expected_artifact, &mut |_| false)
}

fn validate_u1_full_spectrum_candidate_cancellable<C>(
    path: &Path,
    expected_artifact: Option<&U1FullSpectrumPreparedArtifact>,
    should_cancel: &mut C,
) -> Result<U1FullSpectrumValidationReport, U1FullSpectrumError>
where
    C: FnMut(U1FullSpectrumCancellationCheckpoint) -> bool,
{
    cancellation_checkpoint(
        should_cancel,
        U1FullSpectrumCancellationCheckpoint::BeforeModelConfigValidation,
    )?;
    let mut report = U1FullSpectrumValidationReport {
        adapter_id: U1_FULL_SPECTRUM_ADAPTER_ID.to_owned(),
        valid: false,
        physical_filament_count: usize::from(U1_FULL_SPECTRUM_PHYSICAL_COUNT),
        virtual_filament_count: 0,
        used_filament_ids: Vec::new(),
        recipe_calibration_sample_ids: expected_artifact
            .map(prepared_artifact_calibration_sample_ids)
            .unwrap_or_default(),
        issues: Vec::new(),
    };
    let analysis = match analyze_project_with_limits(path, AnalysisLimits::default()) {
        Ok(analysis) => analysis,
        Err(error) => {
            push_validation_error(
                &mut report,
                U1FullSpectrumValidationCode::StructuralPackage,
                None,
                format!("3MF structural preflight failed: {error}"),
            );
            return Ok(report);
        }
    };
    let file = File::open(path).map_err(|source| U1FullSpectrumError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let (mut file, byte_size, sha256) = hash_open_file_cancellable(file, path, should_cancel)?;
    if byte_size != analysis.input.byte_size || sha256 != analysis.input.sha256 {
        push_validation_error(
            &mut report,
            U1FullSpectrumValidationCode::StructuralPackage,
            None,
            "candidate changed after structural preflight",
        );
        return Ok(report);
    }
    file.seek(io::SeekFrom::Start(0))
        .map_err(|source| U1FullSpectrumError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    let mut archive = ZipArchive::new(file)?;
    let names = archive.file_names().map(str::to_owned).collect::<Vec<_>>();
    let stale_policy = StaleArtifactPolicy::unsliced();
    for classification in stale_policy.classify_entries(names.iter().map(String::as_str)) {
        push_validation_error(
            &mut report,
            U1FullSpectrumValidationCode::StaleArtifact,
            Some(classification.path),
            format!(
                "qualification candidates must not retain stale-sensitive {:?} content ({:?})",
                classification.kind, classification.disposition
            ),
        );
    }
    for name in &names {
        if is_embedded_profile(name) {
            push_validation_error(
                &mut report,
                U1FullSpectrumValidationCode::WrongTargetProfile,
                Some(name.clone()),
                "candidate embeds a profile instead of using the exact hash-pinned system profile",
            );
        }
    }

    let project_bytes = match read_zip_entry_limited_cancellable(
        &mut archive,
        PROJECT_SETTINGS_PATH,
        MAX_CONFIG_BYTES,
        should_cancel,
    ) {
        Ok(bytes) => bytes,
        Err(error @ U1FullSpectrumError::Cancelled { .. }) => return Err(error),
        Err(error) => {
            push_validation_error(
                &mut report,
                U1FullSpectrumValidationCode::InvalidProjectSettings,
                Some(PROJECT_SETTINGS_PATH.into()),
                error.to_string(),
            );
            return Ok(report);
        }
    };
    let settings = match serde_json::from_slice::<BTreeMap<String, Value>>(&project_bytes) {
        Ok(settings) => settings,
        Err(source) => {
            push_validation_error(
                &mut report,
                U1FullSpectrumValidationCode::InvalidProjectSettings,
                Some(PROJECT_SETTINGS_PATH.into()),
                format!("project settings are not valid JSON: {source}"),
            );
            return Ok(report);
        }
    };
    let definitions = match validate_project_settings_map(&settings, None) {
        Ok(definitions) => definitions,
        Err(error) => {
            push_validation_error(
                &mut report,
                U1FullSpectrumValidationCode::InvalidProjectSettings,
                Some(PROJECT_SETTINGS_PATH.into()),
                error.to_string(),
            );
            Vec::new()
        }
    };
    report.virtual_filament_count = definitions.len();
    if let Some(artifact) = expected_artifact {
        if let Err(error) = validate_exact_process(&artifact.process) {
            push_validation_error(
                &mut report,
                U1FullSpectrumValidationCode::ArtifactMismatch,
                None,
                error.to_string(),
            );
        }
        if let Err(error) = validate_u1_full_spectrum_recipe_table(&artifact.recipe_table) {
            push_validation_error(
                &mut report,
                U1FullSpectrumValidationCode::ArtifactMismatch,
                None,
                error.to_string(),
            );
        }
        let assignment_sample_ids = prepared_artifact_calibration_sample_ids(artifact);
        let table_sample_ids = artifact
            .recipe_table
            .targets
            .iter()
            .filter_map(|target| target.calibration_sample_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if artifact.recipe_calibration_sample_ids != assignment_sample_ids
            || table_sample_ids != assignment_sample_ids
        {
            push_validation_error(
                &mut report,
                U1FullSpectrumValidationCode::ArtifactMismatch,
                None,
                "prepared recipe calibration provenance is inconsistent",
            );
        }
        if let Err(error) = validate_project_settings_map(&settings, Some(artifact)) {
            push_validation_error(
                &mut report,
                U1FullSpectrumValidationCode::ArtifactMismatch,
                Some(PROJECT_SETTINGS_PATH.into()),
                error.to_string(),
            );
        }
    }

    let model_settings_bytes = match read_zip_entry_limited_cancellable(
        &mut archive,
        MODEL_SETTINGS_PATH,
        MAX_CONFIG_BYTES,
        should_cancel,
    ) {
        Ok(bytes) => bytes,
        Err(error @ U1FullSpectrumError::Cancelled { .. }) => return Err(error),
        Err(error) => {
            push_validation_error(
                &mut report,
                U1FullSpectrumValidationCode::InvalidPlateMap,
                Some(MODEL_SETTINGS_PATH.into()),
                error.to_string(),
            );
            return finish_validation_report(report);
        }
    };
    let mut contract = match scan_model_settings_cancellable(&model_settings_bytes, should_cancel) {
        Ok(contract) => contract,
        Err(error @ U1FullSpectrumError::Cancelled { .. }) => return Err(error),
        Err(error) => {
            push_validation_error(
                &mut report,
                U1FullSpectrumValidationCode::InvalidPlateMap,
                Some(MODEL_SETTINGS_PATH.into()),
                error.to_string(),
            );
            return finish_validation_report(report);
        }
    };
    validate_plate_contracts(&contract.plates, expected_artifact, &mut report);

    for name in names
        .iter()
        .filter(|name| name.starts_with("3D/") && name.ends_with(".model"))
    {
        let entry = archive.by_name(name)?;
        if let Err(error) = collect_model_paint_filaments_cancellable(
            BufReader::new(entry),
            name,
            &mut contract.used_filament_ids,
            should_cancel,
        ) {
            if matches!(&error, U1FullSpectrumError::Cancelled { .. }) {
                return Err(error);
            }
            push_validation_error(
                &mut report,
                U1FullSpectrumValidationCode::InvalidMixedDefinition,
                Some(name.clone()),
                error.to_string(),
            );
        }
    }
    report.used_filament_ids = contract.used_filament_ids.iter().copied().collect();
    validate_assignment_closure(
        &contract.used_filament_ids,
        &definitions,
        expected_artifact,
        &mut report,
    );
    cancellation_checkpoint(
        should_cancel,
        U1FullSpectrumCancellationCheckpoint::AfterModelConfigValidation,
    )?;
    finish_validation_report(report)
}

fn finish_validation_report(
    mut report: U1FullSpectrumValidationReport,
) -> Result<U1FullSpectrumValidationReport, U1FullSpectrumError> {
    report.valid = !report
        .issues
        .iter()
        .any(|issue| issue.severity == U1FullSpectrumValidationSeverity::Error);
    Ok(report)
}

fn push_validation_error(
    report: &mut U1FullSpectrumValidationReport,
    code: U1FullSpectrumValidationCode,
    path: Option<String>,
    message: impl Into<String>,
) {
    report.issues.push(U1FullSpectrumValidationIssue {
        severity: U1FullSpectrumValidationSeverity::Error,
        code,
        path,
        message: message.into(),
    });
}

fn validate_plate_contracts(
    plates: &[PlateMapContract],
    expected_artifact: Option<&U1FullSpectrumPreparedArtifact>,
    report: &mut U1FullSpectrumValidationReport,
) {
    if plates.is_empty() {
        push_validation_error(
            report,
            U1FullSpectrumValidationCode::InvalidPlateMap,
            Some(MODEL_SETTINGS_PATH.into()),
            "model settings contain no target plates",
        );
        return;
    }
    let mut ids = BTreeSet::new();
    for plate in plates {
        let Some(plate_id) = plate.plate_id else {
            push_validation_error(
                report,
                U1FullSpectrumValidationCode::InvalidPlateMap,
                Some(MODEL_SETTINGS_PATH.into()),
                "plate is missing a numeric plater_id",
            );
            continue;
        };
        if !ids.insert(plate_id) {
            push_validation_error(
                report,
                U1FullSpectrumValidationCode::InvalidPlateMap,
                Some(MODEL_SETTINGS_PATH.into()),
                format!("duplicate target plate ID {plate_id}"),
            );
        }
        if plate.mode.as_deref() != Some("Auto For Flush")
            || plate.maps.as_deref() != Some("1 1 1 1")
            || plate
                .volume_maps
                .as_deref()
                .is_some_and(|value| value != "0 0 0 0")
        {
            push_validation_error(
                report,
                U1FullSpectrumValidationCode::InvalidPlateMap,
                Some(MODEL_SETTINGS_PATH.into()),
                format!(
                    "plate {plate_id} does not use the exact four-physical-filament Auto For Flush mapping"
                ),
            );
        }
    }
    if let Some(artifact) = expected_artifact {
        let expected = artifact
            .plates
            .iter()
            .map(|plate| plate.target_plate_id)
            .collect::<BTreeSet<_>>();
        if ids != expected || plates.len() != artifact.plates.len() {
            push_validation_error(
                report,
                U1FullSpectrumValidationCode::ArtifactMismatch,
                Some(MODEL_SETTINGS_PATH.into()),
                format!("target plate IDs {ids:?} do not match prepared plate IDs {expected:?}"),
            );
        }
    }
}

fn validate_assignment_closure(
    used_ids: &BTreeSet<u8>,
    definitions: &[U1FullSpectrumNativeDefinition],
    expected_artifact: Option<&U1FullSpectrumPreparedArtifact>,
    report: &mut U1FullSpectrumValidationReport,
) {
    let maximum = U1_FULL_SPECTRUM_PHYSICAL_COUNT
        .saturating_add(u8::try_from(definitions.len()).unwrap_or(u8::MAX));
    for id in used_ids {
        if *id == 0 || *id > maximum || *id > U1_FULL_SPECTRUM_MAX_FILAMENT_ID {
            push_validation_error(
                report,
                U1FullSpectrumValidationCode::AssignmentOutOfRange,
                None,
                format!("geometry references filament {id}, outside 1..={maximum}"),
            );
        } else if *id > U1_FULL_SPECTRUM_PHYSICAL_COUNT
            && !definitions
                .iter()
                .any(|definition| definition.virtual_filament_id == *id)
        {
            push_validation_error(
                report,
                U1FullSpectrumValidationCode::MissingVirtualDefinition,
                None,
                format!("geometry references missing virtual filament {id}"),
            );
        }
    }
    for definition in definitions {
        if !used_ids.contains(&definition.virtual_filament_id) {
            push_validation_error(
                report,
                U1FullSpectrumValidationCode::UnusedVirtualDefinition,
                Some(PROJECT_SETTINGS_PATH.into()),
                format!(
                    "virtual filament {} is defined but not assigned to retained geometry",
                    definition.virtual_filament_id
                ),
            );
        }
    }
    if let Some(artifact) = expected_artifact {
        let expected = prepared_geometry_assignment_ids(artifact);
        if used_ids != &expected {
            push_validation_error(
                report,
                U1FullSpectrumValidationCode::ArtifactMismatch,
                None,
                format!(
                    "geometry filament assignments {used_ids:?} do not match prepared assignments {expected:?}"
                ),
            );
        }
    }
}

fn prepared_geometry_assignment_ids(artifact: &U1FullSpectrumPreparedArtifact) -> BTreeSet<u8> {
    artifact
        .assignments
        .iter()
        .filter(|assignment| !assignment.source_slots.is_empty())
        .map(|assignment| assignment.target_filament_id)
        .collect()
}

fn scan_model_settings_cancellable<C>(
    bytes: &[u8],
    should_cancel: &mut C,
) -> Result<ModelSettingsContract, U1FullSpectrumError>
where
    C: FnMut(U1FullSpectrumCancellationCheckpoint) -> bool,
{
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    let mut event_count = 0_usize;
    let mut current_plate: Option<(usize, PlateMapContract)> = None;
    let mut contract = ModelSettingsContract::default();
    loop {
        if event_count.is_multiple_of(XML_CANCELLATION_INTERVAL_EVENTS) {
            cancellation_checkpoint(
                should_cancel,
                U1FullSpectrumCancellationCheckpoint::XmlEventBatch,
            )?;
        }
        let event =
            reader
                .read_event_into(&mut buffer)
                .map_err(|error| U1FullSpectrumError::Xml {
                    path: MODEL_SETTINGS_PATH.into(),
                    message: error.to_string(),
                })?;
        match event {
            Event::Start(event) => {
                let event_name = event.name();
                let name = local_xml_name(event_name.as_ref());
                if name == b"plate" {
                    if current_plate.is_some() {
                        return Err(U1FullSpectrumError::Xml {
                            path: MODEL_SETTINGS_PATH.into(),
                            message: "nested plate elements are unsupported".into(),
                        });
                    }
                    current_plate = Some((depth, PlateMapContract::default()));
                } else if name == b"metadata" {
                    record_model_settings_metadata(
                        &reader,
                        &event,
                        current_plate.as_mut().map(|(_, plate)| plate),
                        &mut contract.used_filament_ids,
                    )?;
                }
                depth = depth.saturating_add(1);
            }
            Event::Empty(event) => {
                if local_xml_name(event.name().as_ref()) == b"metadata" {
                    record_model_settings_metadata(
                        &reader,
                        &event,
                        current_plate.as_mut().map(|(_, plate)| plate),
                        &mut contract.used_filament_ids,
                    )?;
                }
            }
            Event::End(event) => {
                depth = depth.saturating_sub(1);
                if local_xml_name(event.name().as_ref()) == b"plate"
                    && current_plate
                        .as_ref()
                        .is_some_and(|(plate_depth, _)| *plate_depth == depth)
                {
                    let (_, plate) =
                        current_plate
                            .take()
                            .ok_or_else(|| U1FullSpectrumError::Xml {
                                path: MODEL_SETTINGS_PATH.into(),
                                message: "closed an unopened plate element".into(),
                            })?;
                    contract.plates.push(plate);
                }
            }
            Event::DocType(_) => {
                return Err(U1FullSpectrumError::Xml {
                    path: MODEL_SETTINGS_PATH.into(),
                    message: "DOCTYPE is forbidden".into(),
                });
            }
            Event::Eof => break,
            _ => {}
        }
        event_count = event_count.saturating_add(1);
        buffer.clear();
    }
    if current_plate.is_some() {
        return Err(U1FullSpectrumError::Xml {
            path: MODEL_SETTINGS_PATH.into(),
            message: "unclosed plate element".into(),
        });
    }
    Ok(contract)
}

fn record_model_settings_metadata<R: io::BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    plate: Option<&mut PlateMapContract>,
    used_ids: &mut BTreeSet<u8>,
) -> Result<(), U1FullSpectrumError> {
    let attributes = decoded_xml_attributes(reader, event, MODEL_SETTINGS_PATH)?;
    let key = attribute_value(&attributes, b"key");
    let value = attribute_value(&attributes, b"value");
    if key == Some("extruder") {
        let id = value
            .and_then(|value| value.parse::<u8>().ok())
            .filter(|id| *id <= U1_FULL_SPECTRUM_MAX_FILAMENT_ID)
            .ok_or_else(|| U1FullSpectrumError::Xml {
                path: MODEL_SETTINGS_PATH.into(),
                message: "extruder metadata is not a valid filament ID".into(),
            })?;
        if id != 0 {
            used_ids.insert(id);
        }
    }
    let Some(plate) = plate else {
        return Ok(());
    };
    let Some(value) = value else {
        return Ok(());
    };
    match key {
        Some("plater_id") => set_once(
            &mut plate.plate_id,
            value.parse::<u32>().map_err(|_| U1FullSpectrumError::Xml {
                path: MODEL_SETTINGS_PATH.into(),
                message: "plater_id is not numeric".into(),
            })?,
            "plater_id",
        )?,
        Some("filament_map_mode") => {
            set_once(&mut plate.mode, value.to_owned(), "filament_map_mode")?
        }
        Some("filament_maps") => set_once(&mut plate.maps, value.to_owned(), "filament_maps")?,
        Some("filament_volume_maps") => set_once(
            &mut plate.volume_maps,
            value.to_owned(),
            "filament_volume_maps",
        )?,
        _ => {}
    }
    Ok(())
}

fn set_once<T>(
    destination: &mut Option<T>,
    value: T,
    label: &str,
) -> Result<(), U1FullSpectrumError> {
    if destination.replace(value).is_some() {
        return Err(U1FullSpectrumError::Xml {
            path: MODEL_SETTINGS_PATH.into(),
            message: format!("duplicate {label} metadata"),
        });
    }
    Ok(())
}

fn collect_model_paint_filaments_cancellable<R, C>(
    source: R,
    path: &str,
    used_ids: &mut BTreeSet<u8>,
    should_cancel: &mut C,
) -> Result<(), U1FullSpectrumError>
where
    R: io::BufRead,
    C: FnMut(U1FullSpectrumCancellationCheckpoint) -> bool,
{
    let mut reader = Reader::from_reader(source);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut event_count = 0_usize;
    loop {
        if event_count.is_multiple_of(XML_CANCELLATION_INTERVAL_EVENTS) {
            cancellation_checkpoint(
                should_cancel,
                U1FullSpectrumCancellationCheckpoint::XmlEventBatch,
            )?;
        }
        let event =
            reader
                .read_event_into(&mut buffer)
                .map_err(|error| U1FullSpectrumError::Xml {
                    path: path.into(),
                    message: error.to_string(),
                })?;
        match event {
            Event::Start(event) | Event::Empty(event) => {
                for attribute in event.attributes().with_checks(true) {
                    let attribute = attribute.map_err(|error| U1FullSpectrumError::Xml {
                        path: path.into(),
                        message: error.to_string(),
                    })?;
                    let key = local_xml_name(attribute.key.as_ref());
                    if key != b"paint_color" && key != b"mmu_segmentation" {
                        continue;
                    }
                    let value = attribute
                        .decode_and_unescape_value(reader.decoder())
                        .map_err(|error| U1FullSpectrumError::Xml {
                            path: path.into(),
                            message: error.to_string(),
                        })?;
                    let paint = decode_paint_annotation(&value).map_err(|error| {
                        U1FullSpectrumError::SemanticValidation(format!(
                            "invalid paint annotation in {path}: {error}"
                        ))
                    })?;
                    used_ids.extend(paint.used_states().into_iter().filter(|state| *state != 0));
                }
            }
            Event::DocType(_) => {
                return Err(U1FullSpectrumError::Xml {
                    path: path.into(),
                    message: "DOCTYPE is forbidden".into(),
                });
            }
            Event::Eof => break,
            _ => {}
        }
        event_count = event_count.saturating_add(1);
        buffer.clear();
    }
    Ok(())
}

fn decoded_xml_attributes<R: io::BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    path: &str,
) -> Result<Vec<(Vec<u8>, String)>, U1FullSpectrumError> {
    event
        .attributes()
        .with_checks(true)
        .map(|attribute| {
            let attribute = attribute.map_err(|error| U1FullSpectrumError::Xml {
                path: path.into(),
                message: error.to_string(),
            })?;
            let value = attribute
                .decode_and_unescape_value(reader.decoder())
                .map_err(|error| U1FullSpectrumError::Xml {
                    path: path.into(),
                    message: error.to_string(),
                })?
                .into_owned();
            Ok((attribute.key.as_ref().to_vec(), value))
        })
        .collect()
}

fn attribute_value<'a>(attributes: &'a [(Vec<u8>, String)], key: &[u8]) -> Option<&'a str> {
    attributes
        .iter()
        .find(|(candidate, _)| local_xml_name(candidate) == key)
        .map(|(_, value)| value.as_str())
}

fn local_xml_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

fn read_zip_entry_limited_cancellable<R, C>(
    archive: &mut ZipArchive<R>,
    entry_path: &str,
    limit: u64,
    should_cancel: &mut C,
) -> Result<Vec<u8>, U1FullSpectrumError>
where
    R: Read + Seek,
    C: FnMut(U1FullSpectrumCancellationCheckpoint) -> bool,
{
    let mut entry = archive.by_name(entry_path)?;
    if entry.size() > limit {
        return Err(U1FullSpectrumError::SemanticValidation(format!(
            "{entry_path} expands to {} bytes; limit is {limit}",
            entry.size()
        )));
    }
    let capacity = usize::try_from(entry.size()).map_err(|_| {
        U1FullSpectrumError::SemanticValidation(format!(
            "{entry_path} is too large for this platform"
        ))
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        cancellation_checkpoint(
            should_cancel,
            U1FullSpectrumCancellationCheckpoint::PackageCopyChunk,
        )?;
        let count = entry
            .read(&mut buffer)
            .map_err(|source| U1FullSpectrumError::Read {
                path: PathBuf::from(entry_path),
                source,
            })?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    Ok(bytes)
}

fn hash_open_file(file: File, path: &Path) -> Result<(File, u64, String), U1FullSpectrumError> {
    hash_open_file_cancellable(file, path, &mut |_| false)
}

fn hash_open_file_cancellable<C>(
    mut file: File,
    path: &Path,
    should_cancel: &mut C,
) -> Result<(File, u64, String), U1FullSpectrumError>
where
    C: FnMut(U1FullSpectrumCancellationCheckpoint) -> bool,
{
    file.seek(io::SeekFrom::Start(0))
        .map_err(|source| U1FullSpectrumError::Read {
            path: path.to_owned(),
            source,
        })?;
    let mut digest = Sha256::new();
    let mut byte_size = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        cancellation_checkpoint(
            should_cancel,
            U1FullSpectrumCancellationCheckpoint::HashChunk,
        )?;
        let count = file
            .read(&mut buffer)
            .map_err(|source| U1FullSpectrumError::Read {
                path: path.to_owned(),
                source,
            })?;
        if count == 0 {
            break;
        }
        byte_size = byte_size.saturating_add(count as u64);
        digest.update(&buffer[..count]);
    }
    Ok((file, byte_size, format!("{:x}", digest.finalize())))
}

fn cancellation_checkpoint<C>(
    should_cancel: &mut C,
    checkpoint: U1FullSpectrumCancellationCheckpoint,
) -> Result<(), U1FullSpectrumError>
where
    C: FnMut(U1FullSpectrumCancellationCheckpoint) -> bool,
{
    if should_cancel(checkpoint) {
        Err(U1FullSpectrumError::Cancelled { checkpoint })
    } else {
        Ok(())
    }
}

fn is_embedded_profile(path: &str) -> bool {
    [
        "Metadata/process_settings_",
        "Metadata/filament_settings_",
        "Metadata/machine_settings_",
    ]
    .into_iter()
    .any(|prefix| path.starts_with(prefix))
}

/// Production orchestration boundary for the future common geometry writer.
/// The callback must materialize one normalized unsliced substrate per
/// artifact inside `scratch_directory`, with final geometry selection,
/// placement, object/part extruders, and painted triangle states already
/// remapped to the artifact's physical/virtual filament IDs.
///
/// The callback is never invoked unless the exact installation and embedded
/// Full Spectrum GUI writer-qualification evidence both pass. Recipe accuracy
/// remains governed by measured calibration provenance and explicit color
/// approvals in the canonical plan.
pub fn convert_u1_full_spectrum_with_substrate_builder<F>(
    application_path: &Path,
    input: &PlanningInput,
    result: &PlanningResult,
    destination_directory: &Path,
    build_substrate: F,
) -> Result<U1FullSpectrumConversionResult, U1FullSpectrumError>
where
    F: FnMut(&U1FullSpectrumPreparedArtifact, &Path) -> Result<PathBuf, U1FullSpectrumError>,
{
    convert_u1_full_spectrum_with_substrate_builder_cancellable(
        application_path,
        input,
        result,
        destination_directory,
        build_substrate,
        |_| false,
    )
}

/// Cancellable production orchestration for the common geometry writer.
///
/// `should_cancel` is called at the documented coarse checkpoints and at
/// bounded intervals while this adapter hashes, copies, or parses XML. Returning
/// `true` aborts with [`U1FullSpectrumError::Cancelled`]. The
/// [`U1FullSpectrumCancellationCheckpoint::BeforePublication`] callback is the
/// cancellation/publication boundary: every artifact has been staged and
/// validated at that point, no destination exists yet, and the callback is not
/// queried after it returns `false`.
pub fn convert_u1_full_spectrum_with_substrate_builder_cancellable<F, C>(
    application_path: &Path,
    input: &PlanningInput,
    result: &PlanningResult,
    destination_directory: &Path,
    build_substrate: F,
    should_cancel: C,
) -> Result<U1FullSpectrumConversionResult, U1FullSpectrumError>
where
    F: FnMut(&U1FullSpectrumPreparedArtifact, &Path) -> Result<PathBuf, U1FullSpectrumError>,
    C: FnMut(U1FullSpectrumCancellationCheckpoint) -> bool,
{
    convert_u1_full_spectrum_with_substrate_builder_and_support_cancellable(
        application_path,
        input,
        result,
        &SupportInformation::default(),
        destination_directory,
        build_substrate,
        should_cancel,
    )
}

pub fn convert_u1_full_spectrum_with_substrate_builder_and_support_cancellable<F, C>(
    application_path: &Path,
    input: &PlanningInput,
    result: &PlanningResult,
    support: &SupportInformation,
    destination_directory: &Path,
    mut build_substrate: F,
    mut should_cancel: C,
) -> Result<U1FullSpectrumConversionResult, U1FullSpectrumError>
where
    F: FnMut(&U1FullSpectrumPreparedArtifact, &Path) -> Result<PathBuf, U1FullSpectrumError>,
    C: FnMut(U1FullSpectrumCancellationCheckpoint) -> bool,
{
    cancellation_checkpoint(
        &mut should_cancel,
        U1FullSpectrumCancellationCheckpoint::ConversionStart,
    )?;
    let capability = inspect_u1_full_spectrum_macos_application(application_path)?;
    if !capability.conversion_available || !capability.qualification_evidence_valid {
        return Err(U1FullSpectrumError::Capability(capability.issues.join(" ")));
    }
    if !destination_directory.is_dir() {
        return Err(U1FullSpectrumError::Write {
            path: destination_directory.to_owned(),
            source: io::Error::new(
                io::ErrorKind::NotFound,
                "destination directory does not exist",
            ),
        });
    }
    let installation = inspect_macos_application(application_path)
        .map_err(|error| U1FullSpectrumError::Capability(error.to_string()))?;
    let profiles_root = installation.resources_path.join("profiles/Snapmaker");
    let physical_profiles_root = crate::qualified_u1_physical_profiles_root(application_path)
        .map_err(|error| U1FullSpectrumError::Capability(error.to_string()))?;
    let preparation = prepare_u1_full_spectrum_conversion_with_support(input, result, support)?;
    let scratch = tempfile::Builder::new()
        .prefix(".u1-full-spectrum-substrates-")
        .tempdir_in(destination_directory)
        .map_err(|source| U1FullSpectrumError::Write {
            path: destination_directory.to_owned(),
            source,
        })?;
    let canonical_scratch =
        fs::canonicalize(scratch.path()).map_err(|source| U1FullSpectrumError::Read {
            path: scratch.path().to_owned(),
            source,
        })?;
    let mut staged_artifacts = Vec::with_capacity(preparation.artifacts.len());
    let mut file_names = BTreeSet::new();
    for artifact in &preparation.artifacts {
        cancellation_checkpoint(
            &mut should_cancel,
            U1FullSpectrumCancellationCheckpoint::ArtifactStart,
        )?;
        if !file_names.insert(artifact.file_name.as_str())
            || Path::new(&artifact.file_name).file_name()
                != Some(std::ffi::OsStr::new(&artifact.file_name))
        {
            return Err(U1FullSpectrumError::Plan(format!(
                "artifact file name {:?} is duplicated or unsafe",
                artifact.file_name
            )));
        }
        cancellation_checkpoint(
            &mut should_cancel,
            U1FullSpectrumCancellationCheckpoint::BeforeProjectSettingsBuild,
        )?;
        let settings = build_u1_full_spectrum_project_settings_with_physical_profiles(
            &profiles_root,
            &physical_profiles_root,
            artifact,
        )?;
        cancellation_checkpoint(
            &mut should_cancel,
            U1FullSpectrumCancellationCheckpoint::AfterProjectSettingsBuild,
        )?;
        cancellation_checkpoint(
            &mut should_cancel,
            U1FullSpectrumCancellationCheckpoint::BeforeSubstrateBuild,
        )?;
        let substrate = build_substrate(artifact, scratch.path())?;
        cancellation_checkpoint(
            &mut should_cancel,
            U1FullSpectrumCancellationCheckpoint::AfterSubstrateBuild,
        )?;
        let canonical_substrate =
            fs::canonicalize(&substrate).map_err(|source| U1FullSpectrumError::Read {
                path: substrate.clone(),
                source,
            })?;
        if !canonical_substrate.starts_with(&canonical_scratch) {
            return Err(U1FullSpectrumError::Plan(
                "normalized substrate builder returned a path outside its private scratch directory"
                    .into(),
            ));
        }
        let destination = destination_directory.join(&artifact.file_name);
        let staged = stage_u1_full_spectrum_candidate_internal_cancellable(
            &canonical_substrate,
            &destination,
            &settings,
            artifact,
            &mut should_cancel,
        )?;
        let mut source_unit_ids = artifact
            .plates
            .iter()
            .flat_map(|plate| plate.units.iter().map(|unit| unit.source_unit_id.clone()))
            .collect::<Vec<_>>();
        source_unit_ids.sort();
        source_unit_ids.dedup();
        staged_artifacts.push(PendingFullSpectrumPublication {
            batch_id: artifact.batch_id.clone(),
            file_name: artifact.file_name.clone(),
            plate_count: artifact.plates.len(),
            target_plate_ids: artifact
                .plates
                .iter()
                .map(|plate| plate.plan_plate_id.clone())
                .collect(),
            source_unit_ids,
            staged,
        });
    }
    let published =
        publish_staged_full_spectrum_artifacts_cancellable(staged_artifacts, &mut should_cancel)?;
    Ok(U1FullSpectrumConversionResult {
        adapter_id: U1_FULL_SPECTRUM_ADAPTER_ID.into(),
        plan_fingerprint: preparation.plan_fingerprint,
        output_directory: destination_directory.to_owned(),
        artifacts: published,
        warnings: preparation.warnings,
    })
}

fn publish_staged_full_spectrum_artifacts_cancellable<C>(
    staged_artifacts: Vec<PendingFullSpectrumPublication>,
    should_cancel: &mut C,
) -> Result<Vec<U1FullSpectrumPublishedArtifact>, U1FullSpectrumError>
where
    C: FnMut(U1FullSpectrumCancellationCheckpoint) -> bool,
{
    cancellation_checkpoint(
        should_cancel,
        U1FullSpectrumCancellationCheckpoint::BeforePublication,
    )?;
    let mut published = Vec::with_capacity(staged_artifacts.len());
    for pending in staged_artifacts {
        let report = pending.staged.publish()?;
        published.push(U1FullSpectrumPublishedArtifact {
            batch_id: pending.batch_id,
            file_name: pending.file_name,
            path: report.path,
            byte_size: report.byte_size,
            sha256: report.sha256,
            plate_count: pending.plate_count,
            target_plate_ids: pending.target_plate_ids,
            source_unit_ids: pending.source_unit_ids,
            validation: report.validation,
        });
    }
    Ok(published)
}

struct PendingFullSpectrumPublication {
    batch_id: String,
    file_name: String,
    plate_count: usize,
    target_plate_ids: Vec<String>,
    source_unit_ids: Vec<String>,
    staged: StagedFullSpectrumCandidate,
}

struct StagedFullSpectrumCandidate {
    package: ValidatedStagedPackage,
    validation: U1FullSpectrumValidationReport,
}

impl StagedFullSpectrumCandidate {
    fn publish(self) -> Result<U1FullSpectrumCandidateWriteReport, U1FullSpectrumError> {
        let write_report = self
            .package
            .publish()
            .map_err(|error| U1FullSpectrumError::Opc(error.to_string()))?;
        Ok(U1FullSpectrumCandidateWriteReport {
            path: write_report.destination,
            byte_size: write_report.package_bytes,
            sha256: write_report.package_sha256,
            validation: self.validation,
        })
    }
}

/// Writes an unsliced, deterministic **qualification candidate** from a
/// normalized structural substrate whose geometry selection, placement and
/// filament remap have already been completed by the common 3MF pipeline.
///
/// The function deliberately performs its own bounded substrate analysis and
/// source-identity binding; callers do not supply or override analysis data.
/// It never enables production conversion, even when the returned structural
/// and semantic validation reports are clean.
pub fn write_u1_full_spectrum_qualification_candidate(
    normalized_substrate_path: &Path,
    destination: &Path,
    project_settings: &[u8],
    artifact: &U1FullSpectrumPreparedArtifact,
) -> Result<U1FullSpectrumCandidateWriteReport, U1FullSpectrumError> {
    write_u1_full_spectrum_candidate_internal(
        normalized_substrate_path,
        destination,
        project_settings,
        artifact,
    )
}

/// Revalidates a published Full Spectrum artifact against a freshly rebuilt
/// normalized substrate and the exact prepared metadata contract.
///
/// `validate_u1_full_spectrum_candidate` alone intentionally validates only
/// the target dialect. This recovery gate deterministically stages the exact
/// expected package in a private temporary directory and compares its complete
/// package identity, thereby binding geometry, placements, model settings,
/// project settings, and every carried OPC entry to the current source plan.
pub fn validate_u1_full_spectrum_output_against_substrate(
    candidate_path: &Path,
    normalized_substrate_path: &Path,
    project_settings: &[u8],
    artifact: &U1FullSpectrumPreparedArtifact,
) -> Result<U1FullSpectrumValidationReport, U1FullSpectrumError> {
    let scratch = tempfile::tempdir().map_err(|source| U1FullSpectrumError::Write {
        path: std::env::temp_dir(),
        source,
    })?;
    let expected_path = scratch.path().join("expected-full-spectrum.3mf");
    let expected = write_u1_full_spectrum_candidate_internal(
        normalized_substrate_path,
        &expected_path,
        project_settings,
        artifact,
    )?;

    let metadata =
        fs::symlink_metadata(candidate_path).map_err(|source| U1FullSpectrumError::Read {
            path: candidate_path.to_owned(),
            source,
        })?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.len() != expected.byte_size
    {
        return Err(U1FullSpectrumError::SemanticValidation(
            "published candidate package identity differs from the current normalized substrate"
                .into(),
        ));
    }
    let candidate = File::open(candidate_path).map_err(|source| U1FullSpectrumError::Read {
        path: candidate_path.to_owned(),
        source,
    })?;
    let (_candidate, candidate_size, candidate_sha256) = hash_open_file(candidate, candidate_path)?;
    if candidate_size != expected.byte_size || candidate_sha256 != expected.sha256 {
        return Err(U1FullSpectrumError::SemanticValidation(
            "published candidate contents differ from the current normalized substrate".into(),
        ));
    }

    let validation = validate_u1_full_spectrum_candidate(candidate_path, Some(artifact))?;
    if !validation.valid {
        return Err(U1FullSpectrumError::SemanticValidation(
            validation
                .issues
                .iter()
                .map(|issue| issue.message.as_str())
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    Ok(validation)
}

fn write_u1_full_spectrum_candidate_internal(
    normalized_substrate_path: &Path,
    destination: &Path,
    project_settings: &[u8],
    artifact: &U1FullSpectrumPreparedArtifact,
) -> Result<U1FullSpectrumCandidateWriteReport, U1FullSpectrumError> {
    let staged = stage_u1_full_spectrum_candidate_internal_cancellable(
        normalized_substrate_path,
        destination,
        project_settings,
        artifact,
        &mut |_| false,
    )?;
    staged.publish()
}

fn stage_u1_full_spectrum_candidate_internal_cancellable<C>(
    normalized_substrate_path: &Path,
    destination: &Path,
    project_settings: &[u8],
    artifact: &U1FullSpectrumPreparedArtifact,
    should_cancel: &mut C,
) -> Result<StagedFullSpectrumCandidate, U1FullSpectrumError>
where
    C: FnMut(U1FullSpectrumCancellationCheckpoint) -> bool,
{
    if destination.exists() {
        return Err(U1FullSpectrumError::OutputExists(destination.to_owned()));
    }
    cancellation_checkpoint(
        should_cancel,
        U1FullSpectrumCancellationCheckpoint::BeforeModelConfigValidation,
    )?;
    validate_u1_full_spectrum_recipe_table(&artifact.recipe_table)?;
    validate_exact_process(&artifact.process)?;
    let project_map =
        serde_json::from_slice::<BTreeMap<String, Value>>(project_settings).map_err(|source| {
            U1FullSpectrumError::Json {
                path: PROJECT_SETTINGS_PATH.into(),
                source,
            }
        })?;
    validate_project_settings_map(&project_map, Some(artifact))?;

    let analysis =
        analyze_project_with_limits(normalized_substrate_path, AnalysisLimits::default()).map_err(
            |error| {
                U1FullSpectrumError::SemanticValidation(format!(
                    "normalized substrate failed structural preflight: {error}"
                ))
            },
        )?;
    let file =
        File::open(normalized_substrate_path).map_err(|source| U1FullSpectrumError::Read {
            path: normalized_substrate_path.to_owned(),
            source,
        })?;
    let (mut file, byte_size, sha256) =
        hash_open_file_cancellable(file, normalized_substrate_path, should_cancel)?;
    if byte_size != analysis.input.byte_size || sha256 != analysis.input.sha256 {
        return Err(U1FullSpectrumError::SemanticValidation(
            "normalized substrate changed after structural preflight".into(),
        ));
    }
    file.seek(io::SeekFrom::Start(0))
        .map_err(|source| U1FullSpectrumError::Read {
            path: normalized_substrate_path.to_owned(),
            source,
        })?;
    let mut archive = ZipArchive::new(file)?;
    let names = archive.file_names().map(str::to_owned).collect::<Vec<_>>();
    let stale = StaleArtifactPolicy::unsliced().classify_entries(names.iter().map(String::as_str));
    if !stale.is_empty() {
        return Err(U1FullSpectrumError::SemanticValidation(format!(
            "normalized substrate retains stale-sensitive entries: {}",
            stale
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    if let Some(path) = names.iter().find(|path| is_embedded_profile(path)) {
        return Err(U1FullSpectrumError::SemanticValidation(format!(
            "normalized substrate embeds profile {path:?}"
        )));
    }
    let model_settings = read_zip_entry_limited_cancellable(
        &mut archive,
        MODEL_SETTINGS_PATH,
        MAX_CONFIG_BYTES,
        should_cancel,
    )?;
    let mut model_contract = scan_model_settings_cancellable(&model_settings, should_cancel)?;
    validate_substrate_plate_ids(&model_contract.plates, artifact)?;
    let model_paths = names
        .iter()
        .filter(|name| name.starts_with("3D/") && name.ends_with(".model"))
        .cloned()
        .collect::<Vec<_>>();
    if model_paths.is_empty() {
        return Err(U1FullSpectrumError::SemanticValidation(
            "normalized substrate contains no 3D model parts".into(),
        ));
    }
    for name in &model_paths {
        let entry = archive.by_name(name)?;
        collect_model_paint_filaments_cancellable(
            BufReader::new(entry),
            name,
            &mut model_contract.used_filament_ids,
            should_cancel,
        )?;
    }
    let expected_assignments = prepared_geometry_assignment_ids(artifact);
    if model_contract.used_filament_ids != expected_assignments {
        return Err(U1FullSpectrumError::SemanticValidation(format!(
            "normalized substrate assignments {:?} do not match prepared assignments {expected_assignments:?}",
            model_contract.used_filament_ids
        )));
    }
    drop(archive);

    let rewritten_model_settings =
        rewrite_model_settings_plate_maps_cancellable(&model_settings, should_cancel)?;
    cancellation_checkpoint(
        should_cancel,
        U1FullSpectrumCancellationCheckpoint::AfterModelConfigValidation,
    )?;
    let expected_identity =
        ExpectedSourceIdentity::new(analysis.input.byte_size, analysis.input.sha256.clone())
            .map_err(|error| U1FullSpectrumError::Opc(error.to_string()))?;
    let mut package = OpcPackageWriter::new();
    package
        .verify_zip_source(normalized_substrate_path, expected_identity.clone())
        .map_err(|error| U1FullSpectrumError::Opc(error.to_string()))?
        .add_bytes(PROJECT_SETTINGS_PATH, project_settings.to_vec())
        .map_err(|error| U1FullSpectrumError::Opc(error.to_string()))?
        .add_bytes(MODEL_SETTINGS_PATH, rewritten_model_settings)
        .map_err(|error| U1FullSpectrumError::Opc(error.to_string()))?;
    for name in names.iter().filter(|name| {
        !name.ends_with('/')
            && name.as_str() != PROJECT_SETTINGS_PATH
            && name.as_str() != MODEL_SETTINGS_PATH
    }) {
        cancellation_checkpoint(
            should_cancel,
            U1FullSpectrumCancellationCheckpoint::PackageCopyEntry,
        )?;
        package
            .copy_zip_entry_raw(
                normalized_substrate_path,
                expected_identity.clone(),
                name.clone(),
            )
            .map_err(|error| U1FullSpectrumError::Opc(error.to_string()))?;
    }
    cancellation_checkpoint(
        should_cancel,
        U1FullSpectrumCancellationCheckpoint::BeforePackageStaging,
    )?;
    let staged = package
        .stage_to(destination)
        .map_err(|error| U1FullSpectrumError::Opc(error.to_string()))?;
    cancellation_checkpoint(
        should_cancel,
        U1FullSpectrumCancellationCheckpoint::AfterPackageStaging,
    )?;
    let semantic = validate_u1_full_spectrum_candidate_cancellable(
        staged.path(),
        Some(artifact),
        should_cancel,
    )?;
    if !semantic.valid {
        return Err(U1FullSpectrumError::SemanticValidation(
            semantic
                .issues
                .iter()
                .map(|issue| issue.message.as_str())
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    let validated = staged.validate().map_err(|error| match error {
        StagedPackageValidationError::Blocked { report } => {
            U1FullSpectrumError::SemanticValidation(format!(
                "strict unsliced structural validation failed: {}",
                serde_json::to_string(&report.issues)
                    .unwrap_or_else(|_| "validation report unavailable".into())
            ))
        }
        other => U1FullSpectrumError::Opc(other.to_string()),
    })?;
    Ok(StagedFullSpectrumCandidate {
        package: validated,
        validation: semantic,
    })
}

fn validate_substrate_plate_ids(
    plates: &[PlateMapContract],
    artifact: &U1FullSpectrumPreparedArtifact,
) -> Result<(), U1FullSpectrumError> {
    let actual = plates
        .iter()
        .map(|plate| {
            if plate.mode.is_none() || plate.maps.is_none() {
                return Err(U1FullSpectrumError::SemanticValidation(
                    "normalized substrate plate is missing filament mapping metadata".into(),
                ));
            }
            plate.plate_id.ok_or_else(|| {
                U1FullSpectrumError::SemanticValidation(
                    "normalized substrate plate is missing plater_id".into(),
                )
            })
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let expected = artifact
        .plates
        .iter()
        .map(|plate| plate.target_plate_id)
        .collect::<BTreeSet<_>>();
    if plates.len() != artifact.plates.len() || actual.len() != plates.len() || actual != expected {
        return Err(U1FullSpectrumError::SemanticValidation(format!(
            "normalized substrate plate IDs {actual:?} do not match prepared plate IDs {expected:?}"
        )));
    }
    Ok(())
}

fn rewrite_model_settings_plate_maps_cancellable<C>(
    bytes: &[u8],
    should_cancel: &mut C,
) -> Result<Vec<u8>, U1FullSpectrumError>
where
    C: FnMut(U1FullSpectrumCancellationCheckpoint) -> bool,
{
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::with_capacity(bytes.len()));
    let mut buffer = Vec::new();
    let mut plate_depths = Vec::<usize>::new();
    let mut depth = 0_usize;
    let mut event_count = 0_usize;
    loop {
        if event_count.is_multiple_of(XML_CANCELLATION_INTERVAL_EVENTS) {
            cancellation_checkpoint(
                should_cancel,
                U1FullSpectrumCancellationCheckpoint::XmlEventBatch,
            )?;
        }
        let event =
            reader
                .read_event_into(&mut buffer)
                .map_err(|error| U1FullSpectrumError::Xml {
                    path: MODEL_SETTINGS_PATH.into(),
                    message: error.to_string(),
                })?;
        match event {
            Event::Start(event) => {
                let event_name = event.name();
                let name = local_xml_name(event_name.as_ref());
                if name == b"plate" {
                    plate_depths.push(depth);
                }
                let event = if name == b"metadata" && !plate_depths.is_empty() {
                    rewrite_plate_metadata(&reader, &event)?
                } else {
                    event.into_owned()
                };
                writer
                    .write_event(Event::Start(event))
                    .map_err(model_settings_write_error)?;
                depth = depth.saturating_add(1);
            }
            Event::Empty(event) => {
                let event_name = event.name();
                let event = if local_xml_name(event_name.as_ref()) == b"metadata"
                    && !plate_depths.is_empty()
                {
                    rewrite_plate_metadata(&reader, &event)?
                } else {
                    event.into_owned()
                };
                writer
                    .write_event(Event::Empty(event))
                    .map_err(model_settings_write_error)?;
            }
            Event::End(event) => {
                depth = depth.saturating_sub(1);
                let event_name = event.name();
                if local_xml_name(event_name.as_ref()) == b"plate"
                    && plate_depths.last().copied() == Some(depth)
                {
                    plate_depths.pop();
                }
                writer
                    .write_event(Event::End(event.into_owned()))
                    .map_err(model_settings_write_error)?;
            }
            Event::DocType(_) => {
                return Err(U1FullSpectrumError::Xml {
                    path: MODEL_SETTINGS_PATH.into(),
                    message: "DOCTYPE is forbidden".into(),
                });
            }
            Event::Eof => break,
            other => writer
                .write_event(other.into_owned())
                .map_err(model_settings_write_error)?,
        }
        event_count = event_count.saturating_add(1);
        buffer.clear();
    }
    Ok(writer.into_inner())
}

fn rewrite_plate_metadata<R: io::BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
) -> Result<BytesStart<'static>, U1FullSpectrumError> {
    let mut attributes = decoded_xml_attributes(reader, event, MODEL_SETTINGS_PATH)?;
    let replacement = match attribute_value(&attributes, b"key") {
        Some("filament_map_mode") => Some("Auto For Flush"),
        Some("filament_maps") => Some("1 1 1 1"),
        Some("filament_volume_maps") => Some("0 0 0 0"),
        _ => None,
    };
    if let Some(replacement) = replacement {
        let value = attributes
            .iter_mut()
            .find(|(key, _)| local_xml_name(key) == b"value")
            .ok_or_else(|| U1FullSpectrumError::Xml {
                path: MODEL_SETTINGS_PATH.into(),
                message: "plate filament mapping metadata has no value attribute".into(),
            })?;
        value.1 = replacement.into();
    }
    let name = String::from_utf8(event.name().as_ref().to_vec()).map_err(|error| {
        U1FullSpectrumError::Xml {
            path: MODEL_SETTINGS_PATH.into(),
            message: error.to_string(),
        }
    })?;
    let mut rebuilt = BytesStart::new(name);
    for (key, value) in attributes {
        let key = String::from_utf8(key).map_err(|error| U1FullSpectrumError::Xml {
            path: MODEL_SETTINGS_PATH.into(),
            message: error.to_string(),
        })?;
        rebuilt.push_attribute((key.as_str(), value.as_str()));
    }
    Ok(rebuilt.into_owned())
}

fn model_settings_write_error(error: io::Error) -> U1FullSpectrumError {
    U1FullSpectrumError::Xml {
        path: MODEL_SETTINGS_PATH.into(),
        message: error.to_string(),
    }
}

pub fn build_u1_full_spectrum_project_settings(
    snapmaker_profiles_root: &Path,
    artifact: &U1FullSpectrumPreparedArtifact,
) -> Result<Vec<u8>, U1FullSpectrumError> {
    build_u1_full_spectrum_project_settings_with_physical_profiles(
        snapmaker_profiles_root,
        snapmaker_profiles_root,
        artifact,
    )
}

pub fn build_u1_full_spectrum_project_settings_with_physical_profiles(
    full_spectrum_profiles_root: &Path,
    physical_profiles_root: &Path,
    artifact: &U1FullSpectrumPreparedArtifact,
) -> Result<Vec<u8>, U1FullSpectrumError> {
    validate_u1_full_spectrum_recipe_table(&artifact.recipe_table)?;
    validate_exact_process(&artifact.process)?;
    let mut settings = BTreeMap::<String, Value>::new();
    for (relative_path, expected_hash) in [
        (MACHINE_KLIPPER_PATH, KLIPPER_MACHINE_BASE_SHA256),
        (MACHINE_TOOLCHANGER_PATH, TOOLCHANGER_MACHINE_BASE_SHA256),
        (MACHINE_U1_PATH, U1_MACHINE_BASE_SHA256),
        (MACHINE_PROFILE_PATH, U1_MACHINE_PROFILE_SHA256),
    ] {
        settings.extend(read_exact_profile(
            full_spectrum_profiles_root,
            relative_path,
            expected_hash,
        )?);
    }
    for (relative_path, expected_hash) in [
        (PROCESS_U1_PATH, U1_PROCESS_BASE_SHA256),
        (PROCESS_COMMON_PATH, U1_PROCESS_COMMON_SHA256),
        (PROCESS_008_PATH, FULL_SPECTRUM_PROCESS_BASE_SHA256),
        (PROCESS_PROFILE_PATH, FULL_SPECTRUM_PROCESS_PROFILE_SHA256),
    ] {
        settings.extend(read_exact_profile(
            full_spectrum_profiles_root,
            relative_path,
            expected_hash,
        )?);
    }
    let filament_profiles = artifact
        .loadout
        .iter()
        .map(|slot| {
            exact_full_spectrum_slot_profile(
                full_spectrum_profiles_root,
                physical_profiles_root,
                slot,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    merge_full_spectrum_physical_profile_arrays(&mut settings, &filament_profiles)?;
    apply_u1_full_spectrum_project_patch(&mut settings, artifact)?;
    let mut bytes =
        serde_json::to_vec_pretty(&settings).map_err(|source| U1FullSpectrumError::Json {
            path: PROJECT_SETTINGS_PATH.into(),
            source,
        })?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn merge_full_spectrum_physical_profile_arrays(
    settings: &mut BTreeMap<String, Value>,
    filament_profiles: &[BTreeMap<String, Value>],
) -> Result<(), U1FullSpectrumError> {
    // Snapmaker Orca expands every selected filament preset from its typed
    // defaults before it combines the four physical slots.  The checked-in
    // Full Spectrum profile is our qualified U1 baseline, so only its vector
    // key set is serialized here.  A heterogeneous T4 profile is overlaid on
    // that baseline below.  Taking the union of raw JSON keys is unsafe: a
    // profile may omit a typed default or carry a legacy-only key, and an
    // empty-string placeholder is then parsed as an invalid numeric sentinel.
    let baseline = filament_profiles.first().ok_or_else(|| {
        U1FullSpectrumError::Capability(
            "Full Spectrum project has no physical filament profiles".into(),
        )
    })?;
    let filament_keys = baseline
        .iter()
        .filter_map(|(key, value)| value.is_array().then_some(key.clone()))
        .collect::<BTreeSet<_>>();
    for key in filament_keys {
        if matches!(
            key.as_str(),
            "compatible_printers" | "compatible_prints" | "default_filament_profile"
        ) {
            continue;
        }
        let values = filament_profiles
            .iter()
            .enumerate()
            .map(|(slot_index, profile)| {
                profile
                    .get(&key)
                    .and_then(Value::as_array)
                    .and_then(|values| values.first())
                    .cloned()
                    .ok_or_else(|| {
                        U1FullSpectrumError::Capability(format!(
                            "physical profile T{} does not provide required Full Spectrum setting {key}",
                            slot_index + 1
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        settings.insert(key, Value::Array(values));
    }
    Ok(())
}

fn exact_full_spectrum_slot_profile(
    full_spectrum_profiles_root: &Path,
    physical_profiles_root: &Path,
    slot: &U1FullSpectrumPhysicalSlot,
) -> Result<BTreeMap<String, Value>, U1FullSpectrumError> {
    let mut resolved = BTreeMap::new();
    for (relative_path, expected_sha256) in FULL_SPECTRUM_FILAMENT_PROFILE_CHAIN {
        resolved.extend(read_exact_profile(
            full_spectrum_profiles_root,
            relative_path,
            expected_sha256,
        )?);
    }
    let (overlay_chain, reli3d_pva) = match (slot.profile.as_str(), slot.setting_id.as_str()) {
        (FULL_SPECTRUM_PROFILE_NAME, FULL_SPECTRUM_SETTING_ID) => (None, false),
        (GENERIC_PLA_PROFILE_NAME, GENERIC_PLA_SETTING_ID) => {
            (Some(GENERIC_PLA_PROFILE_CHAIN), false)
        }
        (POLYMAKER_PLA_PROFILE_NAME, POLYMAKER_PLA_SETTING_ID) => {
            (Some(POLYMAKER_PLA_PROFILE_CHAIN), false)
        }
        (SNAPMAKER_PVA_PROFILE_NAME, SNAPMAKER_PVA_SETTING_ID) => {
            (Some(SNAPMAKER_PVA_PROFILE_CHAIN), false)
        }
        (RELI3D_PVA_PROFILE_NAME, RELI3D_PVA_SETTING_ID) => {
            (Some(SNAPMAKER_PVA_PROFILE_CHAIN), true)
        }
        _ => {
            return Err(U1FullSpectrumError::Plan(format!(
                "physical slot T{} uses unsupported profile identity {}/{}",
                slot.toolhead.index() + 1,
                slot.profile,
                slot.setting_id
            )));
        }
    };
    if let Some(chain) = overlay_chain {
        let mut overlay = BTreeMap::new();
        for (relative_path, expected_sha256) in chain {
            overlay.extend(read_exact_profile(
                physical_profiles_root,
                relative_path,
                expected_sha256,
            )?);
        }
        normalize_heterogeneous_filament_overlay(&mut overlay)?;
        if reli3d_pva {
            apply_reli3d_pva_profile(&mut overlay);
        }
        resolved.extend(overlay);
    }
    Ok(resolved)
}

fn normalize_heterogeneous_filament_overlay(
    overlay: &mut BTreeMap<String, Value>,
) -> Result<(), U1FullSpectrumError> {
    let Some(legacy_chamber_temperature) = overlay.remove("chamber_temperatures") else {
        return Ok(());
    };
    if let Some(chamber_temperature) = overlay.get("chamber_temperature") {
        if chamber_temperature != &legacy_chamber_temperature {
            return Err(U1FullSpectrumError::Capability(
                "physical profile defines conflicting chamber_temperature and legacy chamber_temperatures values"
                    .into(),
            ));
        }
    } else {
        overlay.insert("chamber_temperature".into(), legacy_chamber_temperature);
    }
    Ok(())
}

pub fn apply_u1_full_spectrum_project_patch(
    settings: &mut BTreeMap<String, Value>,
    artifact: &U1FullSpectrumPreparedArtifact,
) -> Result<(), U1FullSpectrumError> {
    validate_u1_full_spectrum_recipe_table(&artifact.recipe_table)?;
    validate_exact_process(&artifact.process)?;
    if artifact.plates.is_empty() {
        return Err(U1FullSpectrumError::Plan(
            "Full Spectrum artifact contains no plates".into(),
        ));
    }
    for (index, slot) in artifact.loadout.iter().enumerate() {
        if slot.toolhead != Toolhead::ALL[index]
            || !full_spectrum_slot_profile_contract_is_valid(index, slot)
        {
            return Err(U1FullSpectrumError::Plan(format!(
                "physical slot T{} does not match the qualified Full Spectrum/solid-T4 profile contract",
                index + 1
            )));
        }
    }
    for key in [
        "type",
        "setting_id",
        "inherits",
        "compatible_printers",
        "compatible_printers_condition",
        "instantiation",
        "description",
    ] {
        settings.remove(key);
    }
    settings.insert("name".into(), Value::String("project_settings".into()));
    settings.insert("from".into(), Value::String("project".into()));
    settings.insert(
        "version".into(),
        Value::String(SUPPORTED_ORCA_VERSION.into()),
    );
    settings.insert("printer_model".into(), Value::String("Snapmaker U1".into()));
    settings.insert("printer_variant".into(), Value::String("0.4".into()));
    settings.insert(
        "printer_settings_id".into(),
        Value::String(U1_MACHINE_PROFILE_NAME.into()),
    );
    settings.insert(
        "print_settings_id".into(),
        Value::String(FULL_SPECTRUM_PROCESS_PROFILE_NAME.into()),
    );
    settings.insert(
        "nozzle_diameter".into(),
        string_array(std::iter::repeat_n("0.4", 4)),
    );
    settings.insert(
        "filament_settings_id".into(),
        string_array(artifact.loadout.iter().map(|slot| slot.profile.as_str())),
    );
    settings.insert(
        "filament_ids".into(),
        string_array(artifact.loadout.iter().map(|slot| slot.setting_id.as_str())),
    );
    settings.insert(
        "filament_colour".into(),
        string_array(artifact.loadout.iter().map(|slot| rgb_hex(slot.color))),
    );
    settings.insert(
        "default_filament_colour".into(),
        string_array(artifact.loadout.iter().map(|slot| rgb_hex(slot.color))),
    );
    settings.insert(
        "filament_type".into(),
        string_array(artifact.loadout.iter().map(|slot| match slot.material {
            Material::Pla => "PLA",
            Material::Pva => "PVA",
            _ => "unsupported",
        })),
    );
    let flush_matrix = (0..4)
        .flat_map(|source| (0..4).map(move |target| if source == target { "0" } else { "140" }))
        .collect::<Vec<_>>();
    settings.insert("flush_volumes_matrix".into(), string_array(flush_matrix));
    settings.insert(
        "flush_volumes_vector".into(),
        string_array(std::iter::repeat_n("140", 8)),
    );
    settings.insert("flush_multiplier".into(), Value::String("1".into()));

    settings.insert(
        "mixed_filament_definitions".into(),
        Value::String(artifact.recipe_table.serialized_definitions.clone()),
    );
    for (key, value) in [
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
    ] {
        settings.insert(key.into(), Value::String(value.into()));
    }
    settings.insert(
        "curr_bed_type".into(),
        Value::String("Textured PEI Plate".into()),
    );
    settings.insert("brim_type".into(), Value::String("auto_brim".into()));
    settings.insert("print_sequence".into(), Value::String("by layer".into()));
    settings.insert("first_layer_print_sequence".into(), string_array(["0"]));
    settings.insert("other_layers_print_sequence".into(), string_array(["0"]));
    settings.insert(
        "other_layers_print_sequence_nums".into(),
        Value::String("0".into()),
    );
    settings.insert("spiral_mode".into(), Value::String("0".into()));
    settings.insert("spiral_mode_smooth".into(), Value::String("0".into()));
    settings.insert(
        "spiral_mode_max_xy_smoothing".into(),
        Value::String("200%".into()),
    );
    apply_source_support_intent(
        settings,
        &artifact.support,
        artifact.dedicated_support.as_ref(),
        artifact.loadout.len(),
    );
    settings.insert("enable_prime_tower".into(), Value::String("1".into()));
    settings.insert(
        "prime_tower_width".into(),
        Value::String(format_number(PRIME_TOWER_WIDTH_MM)),
    );
    settings.insert(
        "prime_volume".into(),
        Value::String(format_number(PRIME_TOWER_VOLUME_MM3)),
    );
    settings.insert(
        "prime_tower_brim_width".into(),
        Value::String(format_number(PRIME_TOWER_BRIM_MM)),
    );
    settings.insert(
        "wipe_tower_cone_angle".into(),
        Value::String(format_number(PRIME_TOWER_CONE_ANGLE_DEGREES)),
    );
    settings.insert(
        "wipe_tower_extra_spacing".into(),
        Value::String(format!(
            "{}%",
            format_number(PRIME_TOWER_EXTRA_SPACING_PERCENT)
        )),
    );
    settings.insert(
        "wipe_tower_extra_rib_length".into(),
        Value::String(format_number(PRIME_TOWER_RIB_LENGTH_MM)),
    );
    settings.insert("wipe_tower_wall_type".into(), Value::String("rib".into()));
    settings.insert("wipe_tower_filament".into(), Value::String("0".into()));
    settings.insert(
        "wipe_tower_rotation_angle".into(),
        Value::String("0".into()),
    );
    settings.insert(
        "wipe_tower_x".into(),
        string_array(artifact.plates.iter().map(|plate| {
            plate
                .prime_tower
                .map_or_else(|| "0".to_owned(), |tower| format_number(tower.x_mm))
        })),
    );
    settings.insert(
        "wipe_tower_y".into(),
        string_array(artifact.plates.iter().map(|plate| {
            plate
                .prime_tower
                .map_or_else(|| "0".to_owned(), |tower| format_number(tower.y_mm))
        })),
    );
    validate_project_settings_map(settings, Some(artifact))?;
    Ok(())
}

fn apply_source_support_intent(
    settings: &mut BTreeMap<String, Value>,
    support: &SupportInformation,
    dedicated_support: Option<&DedicatedSupportMaterial>,
    physical_filament_count: usize,
) {
    let mut override_keys = Vec::new();
    if let Some(enabled) = support.enabled {
        override_keys.push("enable_support");
        settings.insert(
            "enable_support".into(),
            Value::String(if enabled { "1" } else { "0" }.into()),
        );
    }
    if let Some(support_type) = support.support_type {
        override_keys.push("support_type");
        settings.insert(
            "support_type".into(),
            Value::String(support_type.slicer_value().into()),
        );
    }
    if let Some(angle) = support.threshold_angle_degrees {
        override_keys.push("support_threshold_angle");
        settings.insert(
            "support_threshold_angle".into(),
            Value::String(angle.to_string()),
        );
    }
    if let Some(on_build_plate_only) = support.on_build_plate_only {
        override_keys.push("support_on_build_plate_only");
        settings.insert(
            "support_on_build_plate_only".into(),
            Value::String(if on_build_plate_only { "1" } else { "0" }.into()),
        );
    }
    if let Some(dedicated_support) = dedicated_support {
        let slot = (dedicated_support.toolhead.index() + 1).to_string();
        override_keys.push("support_filament");
        settings.insert(
            "support_filament".into(),
            Value::String(match dedicated_support.usage {
                SupportMaterialUsage::BodyAndInterface => slot.clone(),
                SupportMaterialUsage::InterfaceOnly => "0".into(),
            }),
        );
        override_keys.push("support_interface_filament");
        settings.insert("support_interface_filament".into(), Value::String(slot));
        for (key, value) in [
            ("support_top_z_distance", "0"),
            ("support_bottom_z_distance", "0"),
            ("support_interface_top_layers", "3"),
            ("support_interface_spacing", "0.2"),
            ("support_interface_speed", "30"),
        ] {
            override_keys.push(key);
            settings.insert(key.into(), Value::String(value.into()));
        }
    }
    if !override_keys.is_empty() {
        override_keys.sort_unstable();
        let mut groups = vec![Value::String(String::new()); physical_filament_count + 2];
        groups[0] = Value::String(override_keys.join(";"));
        settings.insert("different_settings_to_system".into(), Value::Array(groups));
    }
}

fn full_spectrum_slot_profile_contract_is_valid(
    index: usize,
    slot: &U1FullSpectrumPhysicalSlot,
) -> bool {
    let identity = (
        slot.profile.as_str(),
        slot.setting_id.as_str(),
        slot.filament_id.as_str(),
    );
    let full_spectrum = (
        FULL_SPECTRUM_PROFILE_NAME,
        FULL_SPECTRUM_SETTING_ID,
        "1417031127011",
    );
    if index < 3 {
        return slot.material == Material::Pla && identity == full_spectrum;
    }
    if index != 3 {
        return false;
    }
    match identity {
        (FULL_SPECTRUM_PROFILE_NAME, FULL_SPECTRUM_SETTING_ID, "1417031127011")
        | (GENERIC_PLA_PROFILE_NAME, GENERIC_PLA_SETTING_ID, GENERIC_PLA_FILAMENT_ID)
        | (POLYMAKER_PLA_PROFILE_NAME, POLYMAKER_PLA_SETTING_ID, POLYMAKER_PLA_FILAMENT_ID) => {
            slot.material == Material::Pla
        }
        (SNAPMAKER_PVA_PROFILE_NAME, SNAPMAKER_PVA_SETTING_ID, SNAPMAKER_PVA_FILAMENT_ID)
        | (RELI3D_PVA_PROFILE_NAME, RELI3D_PVA_SETTING_ID, RELI3D_PVA_FILAMENT_ID) => {
            slot.material == Material::Pva
        }
        _ => false,
    }
}

fn read_exact_profile(
    root: &Path,
    relative_path: &str,
    expected_sha256: &str,
) -> Result<BTreeMap<String, Value>, U1FullSpectrumError> {
    let path = root.join(relative_path);
    let bytes = fs::read(&path).map_err(|source| U1FullSpectrumError::Read {
        path: path.clone(),
        source,
    })?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != expected_sha256 {
        return Err(U1FullSpectrumError::Capability(format!(
            "profile {relative_path} has SHA-256 {actual}, expected {expected_sha256}"
        )));
    }
    serde_json::from_slice(&bytes).map_err(|source| U1FullSpectrumError::Json {
        path: path.to_string_lossy().into_owned(),
        source,
    })
}

fn string_array<I, S>(values: I) -> Value
where
    I: IntoIterator<Item = S>,
    S: ToString,
{
    Value::Array(
        values
            .into_iter()
            .map(|value| Value::String(value.to_string()))
            .collect(),
    )
}

fn rgb_hex(color: RgbColor) -> String {
    format!("#{:02X}{:02X}{:02X}", color.red, color.green, color.blue)
}

fn format_number(value: f64) -> String {
    if value == 0.0 {
        "0".into()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
#[path = "u1_full_spectrum_tests.rs"]
mod tests;
