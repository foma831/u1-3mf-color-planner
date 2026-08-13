//! Clean-room Snapmaker U1 Direct Spools Project 3MF adapter.
//!
//! The adapter is intentionally narrower than the planner. It accepts only a
//! complete, backend-owned U1/Direct plan, preserves immutable source geometry
//! and material identity, applies only validated target placements, rewrites
//! color assignments to physical T1–T4 IDs, and publishes an unsliced bundle
//! through the generic validated OPC writer.

use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};
use tempfile::NamedTempFile;
use thiserror::Error;
use u1_planner::{
    ColorStrategy, Material, PackingStatus, PlannedBatch, PlannedJob, PlannedPlate, PlanningInput,
    PlanningResult, PrintableUnit, Printer, PrinterLoadout, RgbColor, Spool, Toolhead,
};
use u1_three_mf::{
    ContentTypesBuilder, ExpectedSourceIdentity, InputIdentity, MAIN_MODEL_PATH,
    MAIN_MODEL_RELATIONSHIPS_PATH, MODEL_RELATIONSHIP_TYPE, OpcPackageWriter, OpcRelationship,
    OpcWriteReport, ProcessInformation, ProjectAnalysis, SupportInformation,
    ValidatedStagedPackage, analyze_project, decode_paint_annotation, encode_paint_annotation,
    relationships_xml,
};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

use crate::{SUPPORTED_ORCA_VERSION, U1FullSpectrumPreparedArtifact, inspect_macos_application};

pub const U1_DIRECT_ADAPTER_ID: &str = "snapmaker-orca/2.3.5/u1-0.4-direct";
pub const U1_DIRECT_MACHINE_PROFILE: &str = "Snapmaker U1 (0.4 nozzle)";
pub const U1_DIRECT_PROCESS_PROFILE: &str = "0.20 Standard @Snapmaker U1 (0.4 nozzle)";
pub const U1_DIRECT_PROCESS_SETTING_ID: &str = "GP004";
pub const U1_DIRECT_EXECUTABLE_SHA256: &str =
    "4c30e59cf582dcc4f12e43741fcab2f97045e972065d465481ed3760678d0fbe";
pub const U1_DIRECT_PROFILE_PACK_VERSION: &str = "02.02.53.02";
pub const U1_DIRECT_PROFILE_MANIFEST_SHA256: &str =
    "08d2e3a4450f07fa75f123c691495b3cd3c354c3c29697ceb8a35d6407186cb9";

const PROJECT_SETTINGS_PATH: &str = "Metadata/project_settings.config";
const MODEL_SETTINGS_PATH: &str = "Metadata/model_settings.config";
const BUNDLED_SNAPMAKER_PROFILE_ROOT: &str = "profiles/Snapmaker";
const BUNDLED_SNAPMAKER_PROFILE_MANIFEST: &str = "profiles/Snapmaker.json";
const SNAPMAKER_SYSTEM_DIR: &str = "system";
const SNAPMAKER_VENDOR_NAME: &str = "Snapmaker";
const DIRECT_PROCESS_PATH: &str = "process/0.20 Standard @Snapmaker U1 (0.4 nozzle).json";
const MACHINE_PATH: &str = "machine/Snapmaker U1 (0.4 nozzle).json";
const GENERIC_PLA_PATH: &str = "filament/Generic PLA.json";
const POLYMAKER_PLA_PATH: &str = "filament/Polymaker General PLA Family @U1.json";
const GENERIC_PETG_PATH: &str = "filament/Generic PETG.json";
const TARGET_BED_SIZE_MM: f64 = 270.0;
const TARGET_MIN_X_MM: f64 = 0.5;
const TARGET_MAX_X_MM: f64 = 270.5;
const TARGET_MIN_Y_MM: f64 = 1.0;
const TARGET_MAX_Y_MM: f64 = 271.0;
const TARGET_PRINTABLE_HEIGHT_MM: f64 = 270.05;
const TARGET_BOUNDS_TOLERANCE_MM: f64 = 0.02;
const PRIME_TOWER_WIDTH_MM: f64 = 30.0;
const PRIME_TOWER_DEPTH_MM: f64 = 45.0;
const PRIME_TOWER_VOLUME_MM3: f64 = 45.0;
const PRIME_TOWER_BRIM_MM: f64 = 5.0;
const PRIME_TOWER_CLEARANCE_MM: f64 = 1.0;
const PRIME_TOWER_EXTRA_RIB_LENGTH_MM: f64 = 8.0;
const PRIME_TOWER_EXTRA_SPACING_PERCENT: f64 = 120.0;
const PRIME_TOWER_CONE_ANGLE_DEGREES: f64 = 15.0;
const FULL_SPECTRUM_PRIME_TOWER_MAX_HALF_EXTENT_MM: f64 = 49.6;
const MIN_QUALIFIED_LAYER_HEIGHT_MM: f64 = 0.08;
const MAX_QUALIFIED_LAYER_HEIGHT_MM: f64 = 0.32;
// Snapmaker Orca's inherited `auto_brim` may grow an outer brim to 18 mm.
// Add the largest qualified object gap so the tower collision proof remains
// valid without changing that source/target print intent.
const AUTO_BRIM_MAX_WIDTH_MM: f64 = 18.0;
const OBJECT_FOOTPRINT_CLEARANCE_MM: f64 = AUTO_BRIM_MAX_WIDTH_MM + MAX_OBJECT_BRIM_GAP_MM;
const MAX_OBJECT_BRIM_WIDTH_MM: f64 = 5.0;
const MAX_OBJECT_BRIM_GAP_MM: f64 = 1.0;
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
const VIRTUAL_PLATE_GAP_RATIO: f64 = 0.2;
const MAX_METADATA_BYTES: u64 = 64 * 1024 * 1024;
const COPY_BUFFER_BYTES: usize = 256 * 1024;
const CONVERSION_STATE_ACTIVE: u8 = 0;
const CONVERSION_STATE_CANCELLED: u8 = 1;
const CONVERSION_STATE_PUBLISHING: u8 = 2;
const CONVERSION_STATE_PUBLISHED: u8 = 3;
const STAGING_RECORD_SCHEMA_VERSION: u8 = 1;
const STAGING_DIRECTORY_PREFIX: &str = ".u1-3mf-conversion-";
const STAGING_OWNER_SUFFIX: &str = ".owner";
const MAX_STAGING_OWNER_BYTES: u64 = 4 * 1024;
const U1_DIRECT_QUALIFICATION_RECORD: &[u8] =
    include_bytes!("../qualification/u1-direct-2.3.5.json");
const U1_DIRECT_QUALIFICATION_REPORT: &[u8] =
    include_bytes!("../qualification/u1-direct-2.3.5-report.json");
const U1_DIRECT_QUALIFICATION_FIXTURE: &str = "Sailfin Dragon - Articulated Lizard by Raki-Box.3mf";
const U1_DIRECT_QUALIFICATION_FIXTURE_SHA256: &str =
    "1b20d6124353d3bc31c4ea554e482ba6f8ef281dd23e2df6bbc0d561b4f6e0d7";

const DIRECT_PROFILE_BASELINE: &[(&str, &str)] = &[
    (
        MACHINE_PATH,
        "545e4456b78cc58e701ed257e4024b0ed9e2461cccc1046e6f901ce3c020ee0d",
    ),
    (
        "machine/fdm_U1.json",
        "ff0724000b1436fcc3d77ffa74bb623a9b322474fc20c8a8d5a26ee164077046",
    ),
    (
        "machine/fdm_toolchanger.json",
        "b2ee2d2c31a26ddee7b0c8bd84c58934d9c01e84e1b8f7ae45e14acfd1f7b52a",
    ),
    (
        "machine/fdm_klipper.json",
        "78dd750211347543368a8a0f136441ef7e23b3a4a09a9e33a370c91dbda273b9",
    ),
    (
        DIRECT_PROCESS_PATH,
        "3342dbe3e995b66ab5700245c676f446a88d35f56498b074ab6fe14039d67763",
    ),
    (
        "process/fdm_process_U1_0.20.json",
        "2df28dc2474b6aeb77d5416043b3a67e54ff7ea1cdf5a5daa9e97db7e5b034a6",
    ),
    (
        "process/fdm_process_U1_common.json",
        "43fba9fe359864b265ffa8682cf404c2b935d12164ffe3d0048e5dcb0691db74",
    ),
    (
        "process/fdm_process_U1.json",
        "66e9e51c4bd04cbbec49fa1a3a043bc3c6e329396c2b38a18db5a66b6e72b4e2",
    ),
    (
        GENERIC_PLA_PATH,
        "1b67ca20832ec5f72e90d8f5ec76e17bc1c150e45fed1d833412310e8a4378ab",
    ),
    (
        "filament/Generic PLA @base.json",
        "bfee7e83f1d76c862e36680ae3bccba55d45c88bc84bfdc489272f881fad9564",
    ),
    (
        "filament/fdm_filament_pla_generic.json",
        "b815ff418638712ac57f740b5d735e231147128fcd13dc16a16c69011d3bd75a",
    ),
    (
        "filament/fdm_filament_common_generic.json",
        "24e4920907489bfdeb250ba749b005c97571d7a547416989af10df502896e99d",
    ),
    (
        POLYMAKER_PLA_PATH,
        "37f61ec15169713050f6ace92da102fc5ad39b05556965f364db941b3a8f9729",
    ),
    (
        "filament/Polymaker PLA @U1 base.json",
        "f9770b13c948e0249c09741babda89f8a6b16a0302f1a9f5e86cc2d3e57e3f96",
    ),
    (
        "filament/fdm_filament_pla_poly.json",
        "b3aa7fda86cec0cb9c6bee3bb600aa44a54f962fc7c8350bfd6e82d1295bc533",
    ),
    (
        "filament/fdm_filament_common_poly.json",
        "ea111ac929ad6d131237c25eba3af23f802a61afee3bea099eea6c93212ecbbc",
    ),
    (
        GENERIC_PETG_PATH,
        "9887427bf889b4364b49b401aa69a1f874ff484e530a09a54be5e77a0bd5ad9b",
    ),
    (
        "filament/Generic PETG @base.json",
        "e46b5aac9d983eff69a18f6f2453fc2e5a28f1d581370195ef01c9d0d7032e57",
    ),
    (
        "filament/fdm_filament_pet_generic.json",
        "33f78fc9a87c83976a635d23c0303797bc9468c8a1c27ab2d1ef769ea66ac2d1",
    ),
];

// These files are not ordinary preset documents and therefore are absent
// from Snapmaker.json's profile lists. Snapmaker Orca nevertheless reads the
// installed system copies at runtime before falling back to bundled resources.
// They affect material/nozzle and mixed-material compatibility and are part of
// the same fail-closed Direct Spools qualification boundary.
const DIRECT_AUXILIARY_PROFILE_BASELINE: &[(&str, &str)] = &[
    (
        "filament/filament_hot_bed_nozzles.json",
        "a824576f8d9fc26f0d9c32ad386ecd86f182ed907025aec0aa6df74b0d2c08e7",
    ),
    (
        "filament/filament_compatibility.json",
        "daf9fa227f2aaa118e021a51b942180baf8a322002b4294cc4ec0b6000ebc472",
    ),
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum U1DirectCapabilityStatus {
    Qualified,
    StructurallyReadyNeedsGuiQualification,
    UnsupportedInstallation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum U1DirectProfileSource {
    /// The vendor pack Snapmaker Orca actually loads from its GUI data dir.
    InstalledSystem,
    /// The application resource copied into an empty GUI data dir on startup.
    BundledSeed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1DirectProfilePackEvidence {
    pub source: U1DirectProfileSource,
    pub version: Option<String>,
    pub manifest_sha256: String,
    pub valid: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1DirectProfileEvidence {
    pub relative_path: String,
    pub sha256: String,
    pub valid: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1DirectCapabilityReport {
    pub adapter_id: String,
    pub status: U1DirectCapabilityStatus,
    pub conversion_available: bool,
    pub application_version: Option<String>,
    pub executable_sha256: Option<String>,
    pub profile_pack: U1DirectProfilePackEvidence,
    pub profiles: Vec<U1DirectProfileEvidence>,
    pub auxiliary_profiles: Vec<U1DirectProfileEvidence>,
    pub issues: Vec<String>,
}

impl U1DirectCapabilityReport {
    #[must_use]
    pub fn installation_supported(&self) -> bool {
        !matches!(
            self.status,
            U1DirectCapabilityStatus::UnsupportedInstallation
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedPhysicalSlot {
    pub toolhead: String,
    pub spool_id: Option<String>,
    pub spool_name: Option<String>,
    pub material: Option<String>,
    pub color: Option<String>,
    pub profile: String,
    pub setting_id: String,
    pub filament_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1DirectPreparedArtifact {
    pub batch_id: String,
    pub file_name: String,
    pub target_plate_ids: Vec<String>,
    pub source_plate_ids: Vec<u32>,
    pub source_unit_ids: Vec<String>,
    pub loadout: Vec<PreparedPhysicalSlot>,
    pub setup_actions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1DirectPreparation {
    pub adapter_id: String,
    pub source_sha256: String,
    pub plan_fingerprint: String,
    pub bundle_directory_name: String,
    pub artifacts: Vec<U1DirectPreparedArtifact>,
    pub warnings: Vec<String>,
}

/// Canonical source-to-target evidence reconstructed while validating a
/// published Direct Spools artifact during restart recovery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct U1DirectSourceTargetValidationEntry {
    pub source_unit_id: String,
    pub source_plate_id: u32,
    pub output_file: String,
    pub batch_id: String,
    pub target_plate_id: u32,
}

/// Fresh adapter evidence for one source-and-plan-bound Direct Spools output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct U1DirectOutputValidationReport {
    pub adapter_capability: U1DirectCapabilityReport,
    pub source_to_target: Vec<U1DirectSourceTargetValidationEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishedArtifact {
    pub batch_id: String,
    pub file_name: String,
    pub path: PathBuf,
    pub byte_size: u64,
    pub sha256: String,
    pub plate_count: usize,
    pub target_plate_ids: Vec<String>,
    pub source_unit_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1DirectConversionResult {
    pub adapter_id: String,
    pub output_directory: PathBuf,
    pub manifest_path: PathBuf,
    pub artifacts: Vec<PublishedArtifact>,
    pub warnings: Vec<String>,
}

/// Evidence produced while building the geometry-and-paint substrate consumed
/// by the versioned Full Spectrum metadata writer. The substrate itself is an
/// unsliced Project 3MF and is never a user-facing production artifact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct U1FullSpectrumNormalizedSubstrateReport {
    pub path: PathBuf,
    pub byte_size: u64,
    pub sha256: String,
    pub plate_count: usize,
    pub source_unit_ids: Vec<String>,
}

/// Backend-owned control shared between the Tauri command and the blocking
/// writer. Cancellation has a linearization point with final publication:
/// once publication begins, `cancel` returns `false` and the no-clobber atomic
/// rename is allowed to complete.
#[derive(Clone, Debug)]
pub struct U1DirectConversionControl {
    conversion_id: String,
    recovery_key: String,
    state: Arc<AtomicU8>,
}

impl Default for U1DirectConversionControl {
    fn default() -> Self {
        Self::new()
    }
}

impl U1DirectConversionControl {
    #[must_use]
    pub fn new() -> Self {
        Self {
            conversion_id: uuid::Uuid::new_v4().hyphenated().to_string(),
            recovery_key: uuid::Uuid::new_v4().hyphenated().to_string(),
            state: Arc::new(AtomicU8::new(CONVERSION_STATE_ACTIVE)),
        }
    }

    #[must_use]
    pub fn conversion_id(&self) -> &str {
        &self.conversion_id
    }

    /// Requests cancellation while publication can still be prevented.
    /// Returns `true` only when this call changed the state to cancelled.
    #[must_use]
    pub fn cancel(&self) -> bool {
        self.state
            .compare_exchange(
                CONVERSION_STATE_ACTIVE,
                CONVERSION_STATE_CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    #[must_use]
    pub fn state(&self) -> U1DirectConversionState {
        match self.state.load(Ordering::Acquire) {
            CONVERSION_STATE_ACTIVE => U1DirectConversionState::Active,
            CONVERSION_STATE_CANCELLED => U1DirectConversionState::Cancelled,
            CONVERSION_STATE_PUBLISHING => U1DirectConversionState::Publishing,
            CONVERSION_STATE_PUBLISHED => U1DirectConversionState::Published,
            _ => unreachable!("conversion control contains an invalid state"),
        }
    }

    pub fn recovery_record(
        &self,
        destination_parent: &Path,
    ) -> Result<U1DirectStagingRecoveryRecord, U1DirectError> {
        let destination_parent =
            destination_parent
                .canonicalize()
                .map_err(|source| U1DirectError::Read {
                    path: destination_parent.to_path_buf(),
                    source,
                })?;
        if !destination_parent.is_dir() {
            return Err(U1DirectError::Publish(
                "the staging destination is not a directory".into(),
            ));
        }
        Ok(U1DirectStagingRecoveryRecord {
            schema_version: STAGING_RECORD_SCHEMA_VERSION,
            conversion_id: self.conversion_id.clone(),
            recovery_key: self.recovery_key.clone(),
            destination_parent,
            owner_process_id: std::process::id(),
        })
    }

    fn checkpoint(&self) -> Result<(), U1DirectError> {
        if self.state.load(Ordering::Acquire) == CONVERSION_STATE_CANCELLED {
            Err(U1DirectError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn begin_publication(&self) -> Result<(), U1DirectError> {
        self.state
            .compare_exchange(
                CONVERSION_STATE_ACTIVE,
                CONVERSION_STATE_PUBLISHING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map(|_| ())
            .map_err(|state| {
                if state == CONVERSION_STATE_CANCELLED {
                    U1DirectError::Cancelled
                } else {
                    U1DirectError::Publish("conversion publication was already started".into())
                }
            })
    }

    fn mark_published(&self) {
        self.state
            .store(CONVERSION_STATE_PUBLISHED, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum U1DirectConversionState {
    Active,
    Cancelled,
    Publishing,
    Published,
}

fn optional_checkpoint(control: Option<&U1DirectConversionControl>) -> Result<(), U1DirectError> {
    if let Some(control) = control {
        control.checkpoint()?;
    }
    Ok(())
}

/// Opaque recovery capability persisted by the desktop before the writer
/// creates its hidden staging directory. Its secret owner marker prevents a
/// startup scan from treating an arbitrary similarly named user directory as
/// application-owned staging.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct U1DirectStagingRecoveryRecord {
    schema_version: u8,
    conversion_id: String,
    recovery_key: String,
    destination_parent: PathBuf,
    owner_process_id: u32,
}

impl U1DirectStagingRecoveryRecord {
    #[must_use]
    pub fn conversion_id(&self) -> &str {
        &self.conversion_id
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum U1DirectStagingCleanup {
    Removed,
    Missing,
    OwnerStillRunning,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct U1DirectStagingOwner {
    schema_version: u8,
    conversion_id: String,
    recovery_key: String,
}

#[derive(Debug, Error)]
pub enum U1DirectError {
    #[error("Snapmaker U1 Direct conversion is unavailable: {0}")]
    Capability(String),
    #[error("the canonical print plan cannot be converted: {0}")]
    Plan(String),
    #[error("the source 3MF changed after analysis")]
    SourceIdentityChanged,
    #[error("conversion was cancelled before publication")]
    Cancelled,
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
    #[error("invalid source ZIP: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("invalid source XML in {path}: {message}")]
    Xml { path: String, message: String },
    #[error("invalid source JSON in {path}: {source}")]
    Json {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid paint annotation in {path}: {message}")]
    Paint { path: String, message: String },
    #[error("failed to construct the target OPC package: {0}")]
    Opc(String),
    #[error("generated U1 project failed semantic validation: {0}")]
    SemanticValidation(String),
    #[error("output already exists: {0}")]
    OutputExists(PathBuf),
    #[error("failed to publish output bundle: {0}")]
    Publish(String),
}

#[derive(Clone, Debug)]
struct AdapterContext {
    profiles_root: PathBuf,
    profile_manifest_path: PathBuf,
    capability: U1DirectCapabilityReport,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ResolvedProfileSource {
    kind: U1DirectProfileSource,
    profiles_root: PathBuf,
    manifest_path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct SnapmakerVendorManifest {
    name: String,
    version: String,
}

/// Profile documents parsed from the exact bytes whose SHA-256 values match
/// `DIRECT_PROFILE_BASELINE`. Conversion reads this store once during
/// preparation and never re-opens the application profile files afterwards.
#[derive(Clone, Debug)]
struct VerifiedProfileStore {
    documents: BTreeMap<String, BTreeMap<String, Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct U1DirectQualificationRecord {
    schema_version: u32,
    adapter_id: String,
    application_version: String,
    executable_sha256: String,
    profile_source: U1DirectProfileSource,
    profile_pack_version: String,
    profile_manifest_sha256: String,
    gui_round_trip_passed: bool,
    status: String,
    evidence: Option<U1DirectQualificationEvidence>,
    note: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct U1DirectQualificationEvidence {
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
struct U1DirectQualificationReportDocument {
    schema_version: u32,
    adapter_id: String,
    application_version: String,
    executable_sha256: String,
    profile_source: U1DirectProfileSource,
    profile_pack_version: String,
    profile_manifest_sha256: String,
    status: String,
    qualification: Option<U1DirectQualificationReportEvidence>,
    note: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct U1DirectQualificationReportEvidence {
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
    profiles: Vec<U1DirectQualificationProfile>,
    auxiliary_profiles: Vec<U1DirectQualificationProfile>,
    checks: U1DirectQualificationChecks,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct U1DirectQualificationProfile {
    relative_path: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct U1DirectQualificationChecks {
    machine_profile_loaded: bool,
    process_profile_loaded: bool,
    t1_t4_mapping_verified: bool,
    prime_tower_verified: bool,
    plate_count_stable: bool,
    object_count_stable: bool,
    slice_completed_without_repair_warning: bool,
    no_incompatible_profile_warning: bool,
    no_missing_profile_warning: bool,
    no_custom_profile_warning: bool,
    first_gui_saved_structurally_valid: bool,
    reopened_gui_saved_structurally_valid: bool,
    no_embedded_presets_after_first_save: bool,
    no_embedded_presets_after_reopened_save: bool,
    geometry_and_placements_stable: bool,
    target_globals_stable: bool,
    writer_candidate_identify_ids_positive_and_unique: bool,
    writer_candidate_source_identify_ids_preserved: bool,
    gui_identify_ids_unique: bool,
    instance_identity_bijection_stable: bool,
    derived_profile_identity_stable: bool,
}

impl U1DirectQualificationChecks {
    fn all_passed(&self) -> bool {
        self.machine_profile_loaded
            && self.process_profile_loaded
            && self.t1_t4_mapping_verified
            && self.prime_tower_verified
            && self.plate_count_stable
            && self.object_count_stable
            && self.slice_completed_without_repair_warning
            && self.no_incompatible_profile_warning
            && self.no_missing_profile_warning
            && self.no_custom_profile_warning
            && self.first_gui_saved_structurally_valid
            && self.reopened_gui_saved_structurally_valid
            && self.no_embedded_presets_after_first_save
            && self.no_embedded_presets_after_reopened_save
            && self.geometry_and_placements_stable
            && self.target_globals_stable
            && self.writer_candidate_identify_ids_positive_and_unique
            && self.writer_candidate_source_identify_ids_preserved
            && self.gui_identify_ids_unique
            && self.instance_identity_bijection_stable
            && self.derived_profile_identity_stable
    }
}

#[derive(Clone, Debug)]
struct ArtifactBuildPlan {
    prepared: U1DirectPreparedArtifact,
    plates: Vec<ArtifactPlate>,
    object_slot_maps: BTreeMap<u32, BTreeMap<u8, u8>>,
    resource_slot_maps: BTreeMap<(String, u32), BTreeMap<u8, u8>>,
    selected_root_resource_ids: BTreeSet<u32>,
    external_paths: BTreeSet<String>,
    selected_instances: BTreeMap<(u32, u32), InstancePlacement>,
    physical_slots: [PhysicalProfile; 4],
    project_settings: Vec<u8>,
}

#[derive(Clone, Debug)]
struct ArtifactPlate {
    source_plate_ids: Vec<u32>,
    target_plate_id: u32,
    name: String,
    units: Vec<PrintableUnit>,
    source_identify_ids: BTreeMap<(u32, u32), u64>,
    wipe_tower_x: f64,
    wipe_tower_y: f64,
}

#[derive(Clone, Copy, Debug)]
struct InstancePlacement {
    delta_x: f64,
    delta_y: f64,
    delta_z: f64,
}

#[derive(Clone, Debug)]
struct PhysicalProfile {
    name: String,
    setting_id: String,
    filament_id: String,
    color: String,
    material: String,
    resolved: BTreeMap<String, Value>,
}

#[derive(Clone)]
struct GeometryCounts {
    vertices: u64,
    triangles: u64,
    fingerprint: Sha256,
}

impl Default for GeometryCounts {
    fn default() -> Self {
        let mut fingerprint = Sha256::new();
        fingerprint.update(b"u1-planner-geometry-v1\0");
        Self {
            vertices: 0,
            triangles: 0,
            fingerprint,
        }
    }
}

impl GeometryCounts {
    fn sha256(&self) -> String {
        format!("{:x}", self.fingerprint.clone().finalize())
    }
}

#[derive(Clone, Debug)]
struct VerifiedPreparationSource {
    base_name: String,
    bed_size: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BundleManifest<'a> {
    schema_version: u32,
    adapter_id: &'a str,
    adapter_evidence: &'a U1DirectCapabilityReport,
    source: &'a InputIdentity,
    plan_fingerprint: &'a str,
    artifacts: Vec<ManifestArtifact<'a>>,
    source_to_target: Vec<SourceTargetManifestEntry<'a>>,
    warnings: &'a [String],
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManifestArtifact<'a> {
    batch_id: &'a str,
    file_name: &'a str,
    byte_size: u64,
    sha256: &'a str,
    plate_count: usize,
    target_plate_ids: &'a [String],
    source_plate_ids: &'a [u32],
    source_unit_ids: &'a [String],
    loadout: &'a [PreparedPhysicalSlot],
    setup_actions: &'a [String],
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceTargetManifestEntry<'a> {
    source_unit_id: &'a str,
    source_plate_id: u32,
    output_file: &'a str,
    batch_id: &'a str,
    target_plate_id: u32,
}

pub fn inspect_u1_direct_macos_application(
    application_path: &Path,
) -> Result<U1DirectCapabilityReport, U1DirectError> {
    Ok(adapter_context(application_path)?.capability)
}

/// Resolves and re-verifies the effective Snapmaker profile pack used by the
/// qualified GUI. Full Spectrum uses this root for a heterogeneous solid T4
/// profile while retaining its dedicated bundled Full Spectrum baseline.
pub fn qualified_u1_physical_profiles_root(
    application_path: &Path,
) -> Result<PathBuf, U1DirectError> {
    let context = adapter_context(application_path)?;
    if !context.capability.conversion_available {
        return Err(U1DirectError::Capability(
            "the effective U1 physical profile pack is not GUI-qualified".into(),
        ));
    }
    let _verified = load_verified_profile_store(&context)?;
    Ok(context.profiles_root)
}

fn adapter_context(application_path: &Path) -> Result<AdapterContext, U1DirectError> {
    adapter_context_with_control(application_path, None)
}

fn adapter_context_with_control(
    application_path: &Path,
    control: Option<&U1DirectConversionControl>,
) -> Result<AdapterContext, U1DirectError> {
    optional_checkpoint(control)?;
    let base = inspect_macos_application(application_path)
        .map_err(|error| U1DirectError::Capability(error.to_string()))?;
    optional_checkpoint(control)?;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let profile_source =
        resolve_macos_profile_source(application_path, &base.resources_path, home.as_deref())?;
    let mut issues = Vec::new();
    if base.version != SUPPORTED_ORCA_VERSION {
        issues.push(format!(
            "Snapmaker Orca {} is installed; this writer requires exact version {}.",
            base.version, SUPPORTED_ORCA_VERSION
        ));
    }
    let executable_sha256 = hash_path_with_control(&base.executable_path, control)?.1;
    if executable_sha256 != U1_DIRECT_EXECUTABLE_SHA256 {
        issues.push(
            "The Snapmaker Orca executable hash does not match the qualified baseline.".into(),
        );
    }
    let profile_pack = inspect_profile_pack(
        &profile_source,
        U1_DIRECT_PROFILE_PACK_VERSION,
        U1_DIRECT_PROFILE_MANIFEST_SHA256,
        &mut issues,
    );
    optional_checkpoint(control)?;
    let profiles = inspect_profile_baseline(
        &profile_source.profiles_root,
        DIRECT_PROFILE_BASELINE,
        "U1 profile",
        &mut issues,
    );
    optional_checkpoint(control)?;
    let auxiliary_profiles = inspect_profile_baseline(
        &profile_source.profiles_root,
        DIRECT_AUXILIARY_PROFILE_BASELINE,
        "U1 auxiliary runtime profile",
        &mut issues,
    );
    optional_checkpoint(control)?;
    let installation_supported = issues.is_empty();
    // The checked-in GUI evidence is bound to the effective profile source as
    // well as its exact bytes. A byte-identical bundled seed therefore remains
    // structurally usable for qualification work, but it cannot inherit an
    // installed-system GUI attestation.
    let qualified =
        installation_supported && gui_qualification_record_is_valid(profile_source.kind);
    if installation_supported && !qualified {
        issues.push(
            "U1 Direct writer is structurally ready, but the Snapmaker Orca 2.3.5 GUI qualification does not match the effective profile source."
                .into(),
        );
    }
    let status = if !installation_supported {
        U1DirectCapabilityStatus::UnsupportedInstallation
    } else if qualified {
        U1DirectCapabilityStatus::Qualified
    } else {
        U1DirectCapabilityStatus::StructurallyReadyNeedsGuiQualification
    };
    Ok(AdapterContext {
        profiles_root: profile_source.profiles_root,
        profile_manifest_path: profile_source.manifest_path,
        capability: U1DirectCapabilityReport {
            adapter_id: U1_DIRECT_ADAPTER_ID.into(),
            status,
            conversion_available: qualified,
            application_version: Some(base.version),
            executable_sha256: Some(executable_sha256),
            profile_pack,
            profiles,
            auxiliary_profiles,
            issues,
        },
    })
}

/// Resolves the same default macOS data directory used by Snapmaker Orca
/// 2.3.5. A portable `data_dir` beside the application bundle takes
/// precedence. Otherwise the GUI uses wxWidgets' per-user Application Support
/// directory. Command-line `--datadir` launches are intentionally outside this
/// API: callers cannot prove their runtime directory from an application path
/// alone and must not silently qualify another profile tree.
fn resolve_macos_profile_source(
    application_path: &Path,
    resources_path: &Path,
    home: Option<&Path>,
) -> Result<ResolvedProfileSource, U1DirectError> {
    let application_parent = application_path.parent().ok_or_else(|| {
        U1DirectError::Capability(
            "the Snapmaker Orca application path has no parent directory".into(),
        )
    })?;
    let portable_data_dir = application_parent.join("data_dir");
    let data_dir = if try_path_exists(&portable_data_dir)? {
        portable_data_dir
    } else {
        let home = home.filter(|path| path.is_absolute()).ok_or_else(|| {
            U1DirectError::Capability(
                "HOME is unavailable or relative, so the Snapmaker Orca GUI profile data directory cannot be resolved safely"
                    .into(),
            )
        })?;
        home.join("Library/Application Support/Snapmaker_Orca")
    };
    let installed_manifest = data_dir
        .join(SNAPMAKER_SYSTEM_DIR)
        .join(format!("{SNAPMAKER_VENDOR_NAME}.json"));
    let installed_root = data_dir
        .join(SNAPMAKER_SYSTEM_DIR)
        .join(SNAPMAKER_VENDOR_NAME);

    // Any installed manifest/root is authoritative, including a partial or
    // corrupt update. PresetUpdater operates on this system location before
    // PresetBundle loads it; falling back to Resources here would validate a
    // different pack than the GUI will attempt to use.
    if try_path_exists(&installed_manifest)? || try_path_exists(&installed_root)? {
        return Ok(ResolvedProfileSource {
            kind: U1DirectProfileSource::InstalledSystem,
            profiles_root: installed_root,
            manifest_path: installed_manifest,
        });
    }

    // With no installed vendor state at all, PresetUpdater seeds the system
    // directory from the application resources. Qualifying those exact bytes
    // is therefore a deliberate bootstrap policy, not a stale-pack fallback.
    Ok(ResolvedProfileSource {
        kind: U1DirectProfileSource::BundledSeed,
        profiles_root: resources_path.join(BUNDLED_SNAPMAKER_PROFILE_ROOT),
        manifest_path: resources_path.join(BUNDLED_SNAPMAKER_PROFILE_MANIFEST),
    })
}

fn try_path_exists(path: &Path) -> Result<bool, U1DirectError> {
    path.try_exists().map_err(|source| U1DirectError::Read {
        path: path.to_path_buf(),
        source,
    })
}

fn inspect_profile_pack(
    source: &ResolvedProfileSource,
    expected_version: &str,
    expected_hash: &str,
    issues: &mut Vec<String>,
) -> U1DirectProfilePackEvidence {
    let source_label = match source.kind {
        U1DirectProfileSource::InstalledSystem => "installed system",
        U1DirectProfileSource::BundledSeed => "bundled seed",
    };
    let bytes = match fs::read(&source.manifest_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            issues.push(format!(
                "The Snapmaker {source_label} profile manifest could not be read: {error}."
            ));
            return U1DirectProfilePackEvidence {
                source: source.kind,
                version: None,
                manifest_sha256: String::new(),
                valid: false,
            };
        }
    };
    let actual_hash = format!("{:x}", Sha256::digest(&bytes));
    let manifest = serde_json::from_slice::<SnapmakerVendorManifest>(&bytes);
    let version = manifest.as_ref().ok().map(|value| value.version.clone());
    let mut valid = true;
    match manifest {
        Ok(manifest) => {
            if manifest.name != SNAPMAKER_VENDOR_NAME {
                valid = false;
                issues.push(format!(
                    "The Snapmaker {source_label} profile manifest declares vendor {:?}, expected {SNAPMAKER_VENDOR_NAME:?}.",
                    manifest.name
                ));
            }
            if manifest.version != expected_version {
                valid = false;
                issues.push(format!(
                    "The Snapmaker {source_label} profile pack is version {}, expected {}.",
                    manifest.version, expected_version
                ));
            }
        }
        Err(error) => {
            valid = false;
            issues.push(format!(
                "The Snapmaker {source_label} profile manifest is invalid JSON: {error}."
            ));
        }
    }
    if actual_hash != expected_hash {
        valid = false;
        issues.push(format!(
            "The Snapmaker {source_label} profile manifest hash is {actual_hash}, expected {expected_hash}."
        ));
    }
    U1DirectProfilePackEvidence {
        source: source.kind,
        version,
        manifest_sha256: actual_hash,
        valid,
    }
}

fn inspect_profile_baseline(
    profiles_root: &Path,
    baseline: &[(&str, &str)],
    description: &str,
    issues: &mut Vec<String>,
) -> Vec<U1DirectProfileEvidence> {
    baseline
        .iter()
        .map(|(relative_path, expected_hash)| {
            let path = profiles_root.join(relative_path);
            let actual_hash = fs::read(&path)
                .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
                .unwrap_or_default();
            let valid = actual_hash == *expected_hash;
            if !valid {
                if actual_hash.is_empty() {
                    issues.push(format!(
                        "Required {description} {relative_path} is missing or unreadable."
                    ));
                } else {
                    issues.push(format!(
                        "Required {description} {relative_path} has hash {actual_hash}, expected {expected_hash}."
                    ));
                }
            }
            U1DirectProfileEvidence {
                relative_path: (*relative_path).to_owned(),
                sha256: actual_hash,
                valid,
            }
        })
        .collect()
}

fn gui_qualification_record_is_valid(profile_source: U1DirectProfileSource) -> bool {
    qualification_bundle_is_valid(
        U1_DIRECT_QUALIFICATION_RECORD,
        U1_DIRECT_QUALIFICATION_REPORT,
        profile_source,
    )
}

fn qualification_bundle_is_valid(
    record_bytes: &[u8],
    report_bytes: &[u8],
    profile_source: U1DirectProfileSource,
) -> bool {
    let Ok(record) = serde_json::from_slice::<U1DirectQualificationRecord>(record_bytes) else {
        return false;
    };
    let Ok(report) = serde_json::from_slice::<U1DirectQualificationReportDocument>(report_bytes)
    else {
        return false;
    };
    let Some(record_evidence) = record.evidence else {
        return false;
    };
    let Some(report_evidence) = report.qualification else {
        return false;
    };
    let report_sha256 = format!("{:x}", Sha256::digest(report_bytes));
    let profiles_match = report_evidence.profiles.len() == DIRECT_PROFILE_BASELINE.len()
        && report_evidence
            .profiles
            .iter()
            .zip(DIRECT_PROFILE_BASELINE)
            .all(|(actual, expected)| {
                actual.relative_path == expected.0 && actual.sha256 == expected.1
            });
    let auxiliary_profiles_match = report_evidence.auxiliary_profiles.len()
        == DIRECT_AUXILIARY_PROFILE_BASELINE.len()
        && report_evidence
            .auxiliary_profiles
            .iter()
            .zip(DIRECT_AUXILIARY_PROFILE_BASELINE)
            .all(|(actual, expected)| {
                actual.relative_path == expected.0 && actual.sha256 == expected.1
            });
    record.schema_version == 2
        && record.adapter_id == U1_DIRECT_ADAPTER_ID
        && record.application_version == SUPPORTED_ORCA_VERSION
        && record.executable_sha256 == U1_DIRECT_EXECUTABLE_SHA256
        && record.profile_source == profile_source
        && record.profile_pack_version == U1_DIRECT_PROFILE_PACK_VERSION
        && record.profile_manifest_sha256 == U1_DIRECT_PROFILE_MANIFEST_SHA256
        && record.gui_round_trip_passed
        && record.status == "qualified"
        && !record.note.trim().is_empty()
        && report.schema_version == 2
        && report.adapter_id == U1_DIRECT_ADAPTER_ID
        && report.application_version == SUPPORTED_ORCA_VERSION
        && report.executable_sha256 == U1_DIRECT_EXECUTABLE_SHA256
        && report.profile_source == profile_source
        && report.profile_pack_version == U1_DIRECT_PROFILE_PACK_VERSION
        && report.profile_manifest_sha256 == U1_DIRECT_PROFILE_MANIFEST_SHA256
        && report.status == "qualified"
        && !report.note.trim().is_empty()
        && record_evidence.qualification_report_sha256 == report_sha256
        && record_evidence.fixture_name == U1_DIRECT_QUALIFICATION_FIXTURE
        && record_evidence.source_fixture_sha256 == U1_DIRECT_QUALIFICATION_FIXTURE_SHA256
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
        && auxiliary_profiles_match
        && report_evidence.checks.all_passed()
}

fn valid_nonzero_sha256(value: &str) -> bool {
    value.len() == 64
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        && value.bytes().any(|byte| byte != b'0')
}

pub fn canonical_plan_fingerprint(
    input: &PlanningInput,
    result: &PlanningResult,
) -> Result<String, U1DirectError> {
    let bytes = serde_json::to_vec(&(input, result)).map_err(|source| U1DirectError::Json {
        path: "canonical print plan".into(),
        source,
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub fn prepare_u1_direct_conversion(
    application_path: &Path,
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
) -> Result<U1DirectPreparation, U1DirectError> {
    let context = adapter_context(application_path)?;
    if !context.capability.conversion_available {
        return Err(U1DirectError::Capability(
            context.capability.issues.join(" "),
        ));
    }
    prepare_with_context(source_path, analysis, input, result, &context).map(|(value, _)| value)
}

/// Validates and describes U1 Direct artifacts while cooperatively observing
/// the same cancellation capability used by conversion publication.
pub fn prepare_u1_direct_conversion_cancellable(
    application_path: &Path,
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    control: &U1DirectConversionControl,
) -> Result<U1DirectPreparation, U1DirectError> {
    control.checkpoint()?;
    let context = adapter_context_with_control(application_path, Some(control))?;
    control.checkpoint()?;
    if !context.capability.conversion_available {
        return Err(U1DirectError::Capability(
            context.capability.issues.join(" "),
        ));
    }
    prepare_with_context_with_control(
        source_path,
        analysis,
        input,
        result,
        &context,
        Some(control),
    )
    .map(|(value, _)| value)
}

/// Revalidates an already-published Direct Spools artifact against the exact
/// current source, canonical plan, qualified profile pack, and prepared
/// artifact.
///
/// This intentionally rebuilds private source-derived geometry evidence. A
/// standalone-valid U1 project, or a digest rewritten in a mutable bundle
/// manifest, therefore cannot substitute for the approved artifact.
#[allow(clippy::too_many_arguments)]
pub fn validate_u1_direct_output_against_plan(
    application_path: &Path,
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    expected_artifact: &U1DirectPreparedArtifact,
    candidate_path: &Path,
) -> Result<U1DirectOutputValidationReport, U1DirectError> {
    validate_u1_direct_outputs_against_plan(
        application_path,
        source_path,
        analysis,
        input,
        result,
        &[(expected_artifact, candidate_path)],
    )?
    .into_iter()
    .next()
    .ok_or_else(|| U1DirectError::Plan("no Direct Spools recovery candidate was supplied".into()))
}

/// Batch form of [`validate_u1_direct_output_against_plan`]. Profile
/// capability, canonical planning, and the immutable source snapshot are
/// established once for the complete recovery bundle.
#[allow(clippy::too_many_arguments)]
pub fn validate_u1_direct_outputs_against_plan(
    application_path: &Path,
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    candidates: &[(&U1DirectPreparedArtifact, &Path)],
) -> Result<Vec<U1DirectOutputValidationReport>, U1DirectError> {
    if candidates.is_empty() {
        return Err(U1DirectError::Plan(
            "no Direct Spools recovery candidate was supplied".into(),
        ));
    }
    let control = U1DirectConversionControl::new();
    let context = adapter_context_with_control(application_path, Some(&control))?;
    if !context.capability.conversion_available {
        return Err(U1DirectError::Capability(
            context.capability.issues.join(" "),
        ));
    }
    let (preparation, plans) = prepare_with_context_with_control(
        source_path,
        analysis,
        input,
        result,
        &context,
        Some(&control),
    )?;
    if preparation.source_sha256 != analysis.input.sha256 {
        return Err(U1DirectError::SourceIdentityChanged);
    }
    let scratch = tempfile::tempdir().map_err(|source| U1DirectError::Write {
        path: std::env::temp_dir(),
        source,
    })?;
    let source_snapshot = snapshot_source(source_path, &analysis.input, scratch.path(), &control)?;
    let mut reports = Vec::with_capacity(candidates.len());
    let mut seen = BTreeSet::new();
    for (expected_artifact, candidate_path) in candidates {
        if !seen.insert((
            expected_artifact.batch_id.as_str(),
            expected_artifact.file_name.as_str(),
        )) {
            return Err(U1DirectError::Plan(format!(
                "prepared Direct Spools artifact {} / {} was supplied more than once",
                expected_artifact.batch_id, expected_artifact.file_name
            )));
        }
        let plan = plans
            .iter()
            .find(|plan| {
                plan.prepared.batch_id == expected_artifact.batch_id
                    && plan.prepared.file_name == expected_artifact.file_name
            })
            .ok_or_else(|| {
                U1DirectError::Plan(format!(
                    "prepared Direct Spools artifact {} / {} is not present in the canonical plan",
                    expected_artifact.batch_id, expected_artifact.file_name
                ))
            })?;
        if &plan.prepared != *expected_artifact {
            return Err(U1DirectError::Plan(format!(
                "prepared Direct Spools artifact {} / {} changed before recovery validation",
                expected_artifact.batch_id, expected_artifact.file_name
            )));
        }
        let (_rewritten, _entries, geometry) = rewrite_models_to_archive(
            source_snapshot.path(),
            plan,
            scratch.path(),
            analysis.source.title.as_deref(),
            &control,
        )?;
        validate_u1_direct_candidate(candidate_path, plan, &geometry, analysis)?;

        let source_to_target = plan
            .plates
            .iter()
            .flat_map(|plate| {
                plate
                    .units
                    .iter()
                    .map(move |unit| -> Result<_, U1DirectError> {
                        Ok(U1DirectSourceTargetValidationEntry {
                            source_unit_id: unit.source_unit_id.clone(),
                            source_plate_id: parse_source_plate_id(
                                unit.source_plate_id.as_deref(),
                            )?,
                            output_file: plan.prepared.file_name.clone(),
                            batch_id: plan.prepared.batch_id.clone(),
                            target_plate_id: plate.target_plate_id,
                        })
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        reports.push(U1DirectOutputValidationReport {
            adapter_capability: context.capability.clone(),
            source_to_target,
        });
    }
    reverify_open_snapshot_and_source(
        source_path,
        source_snapshot.as_file(),
        &analysis.input,
        &control,
    )?;
    Ok(reports)
}

fn prepare_with_context(
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    context: &AdapterContext,
) -> Result<(U1DirectPreparation, Vec<ArtifactBuildPlan>), U1DirectError> {
    prepare_with_context_with_control(source_path, analysis, input, result, context, None)
}

fn prepare_with_context_with_control(
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    context: &AdapterContext,
    control: Option<&U1DirectConversionControl>,
) -> Result<(U1DirectPreparation, Vec<ArtifactBuildPlan>), U1DirectError> {
    prepare_with_context_from_verified_source(
        source_path,
        source_path,
        analysis,
        input,
        result,
        context,
        control,
    )
}

fn prepare_with_context_from_verified_source(
    verified_source_path: &Path,
    source_name_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    context: &AdapterContext,
    control: Option<&U1DirectConversionControl>,
) -> Result<(U1DirectPreparation, Vec<ArtifactBuildPlan>), U1DirectError> {
    optional_checkpoint(control)?;
    if result.has_hard_errors() {
        return Err(U1DirectError::Plan(format!(
            "{} blocking planner error(s) remain",
            result.errors.len()
        )));
    }
    let source = inspect_verified_preparation_source_with_control(
        verified_source_path,
        source_name_path,
        &analysis.input,
        control,
    )?;
    optional_checkpoint(control)?;
    let normalized_plate_override_count =
        inspect_source_plate_process_overrides_with_control(verified_source_path, control)?;
    optional_checkpoint(control)?;
    let plan_fingerprint = canonical_plan_fingerprint(input, result)?;
    // Re-read every qualified profile exactly once, validate that same byte
    // buffer, and retain its parsed document for the complete preparation.
    // This makes a profile replacement after capability inspection fail
    // closed instead of mixing verified hashes with subsequently read data.
    let verified_profiles = load_verified_profile_store(context)?;
    optional_checkpoint(control)?;
    let artifact_plans = build_artifact_plans(
        analysis,
        input,
        result,
        &verified_profiles,
        &source.base_name,
        source.bed_size,
    )?;
    optional_checkpoint(control)?;
    let artifacts = artifact_plans
        .iter()
        .map(|artifact| artifact.prepared.clone())
        .collect::<Vec<_>>();
    let mut warnings = vec![
        "Output projects are unsliced. Open every file in Snapmaker Orca, verify T1–T4, then slice before printing."
            .to_owned(),
        "Stage B preserves source geometry and material identity. Validated packed plans may translate individual instances from multiple source plates into fresh target plates."
            .to_owned(),
        "Stage B keeps target-owned U1 safety settings while preserving qualified source quality intent: finer layer height, wall generator, wall-speed ceilings, shell minimums, and support gaps."
            .to_owned(),
    ];
    if normalized_plate_override_count > 0 {
        warnings.push(format!(
            "{normalized_plate_override_count} equivalent source per-plate process override(s) were normalized to the explicit U1 target defaults."
        ));
    }
    Ok((
        U1DirectPreparation {
            adapter_id: U1_DIRECT_ADAPTER_ID.into(),
            source_sha256: analysis.input.sha256.clone(),
            plan_fingerprint,
            bundle_directory_name: format!("{}__converted", source.base_name),
            artifacts,
            warnings,
        },
        artifact_plans,
    ))
}

fn inspect_source_plate_process_overrides_with_control(
    source_path: &Path,
    control: Option<&U1DirectConversionControl>,
) -> Result<usize, U1DirectError> {
    optional_checkpoint(control)?;
    let file = File::open(source_path).map_err(|source| U1DirectError::Read {
        path: source_path.to_path_buf(),
        source,
    })?;
    let mut archive = ZipArchive::new(file)?;
    let entry = archive.by_name(MODEL_SETTINGS_PATH)?;
    if entry.size() > MAX_METADATA_BYTES {
        return Err(U1DirectError::Plan(
            "source model settings exceed the safe metadata limit".into(),
        ));
    }
    validate_source_plate_process_overrides_xml_with_control(BufReader::new(entry), control)
}

#[cfg(test)]
fn validate_source_plate_process_overrides_xml<R: io::BufRead>(
    source: R,
) -> Result<usize, U1DirectError> {
    validate_source_plate_process_overrides_xml_with_control(source, None)
}

fn validate_source_plate_process_overrides_xml_with_control<R: io::BufRead>(
    source: R,
    control: Option<&U1DirectConversionControl>,
) -> Result<usize, U1DirectError> {
    const TARGET_OVERRIDE_KEYS: &[&str] = &[
        "bed_type",
        "print_sequence",
        "first_layer_print_sequence",
        "other_layers_print_sequence",
        "other_layers_print_sequence_nums",
        "spiral_mode",
        "timelapse_type",
    ];

    let mut reader = Reader::from_reader(source);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    let mut plate_ordinal = 0_usize;
    let mut current_plate: Option<String> = None;
    let mut seen_overrides = BTreeSet::new();
    let mut normalized_count = 0_usize;
    let mut events_until_checkpoint = 0_usize;
    loop {
        if events_until_checkpoint == 0 {
            optional_checkpoint(control)?;
            events_until_checkpoint = 1_024;
        }
        events_until_checkpoint -= 1;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| U1DirectError::Xml {
                path: MODEL_SETTINGS_PATH.into(),
                message: error.to_string(),
            })?;
        match event {
            Event::Start(event) => {
                let event_name = event.name();
                let name = local_xml_name(event_name.as_ref());
                if depth == 1 && name == b"plate" {
                    plate_ordinal += 1;
                    current_plate = Some(format!("source plate {plate_ordinal}"));
                } else if current_plate.is_some() && name == b"metadata" {
                    return Err(U1DirectError::Plan(
                        "source plate contains non-empty process metadata; this dialect is not qualified for Stage B"
                            .into(),
                    ));
                }
                depth += 1;
            }
            Event::Empty(event) => {
                let event_name = event.name();
                if depth != 2
                    || current_plate.is_none()
                    || local_xml_name(event_name.as_ref()) != b"metadata"
                {
                    buffer.clear();
                    continue;
                }
                let attributes = decoded_attributes(&reader, &event, MODEL_SETTINGS_PATH)?;
                let key = attributes
                    .iter()
                    .find(|(key, _)| local_xml_name(key.as_bytes()) == b"key")
                    .map(|(_, value)| value.as_str());
                let value = attributes
                    .iter()
                    .find(|(key, _)| local_xml_name(key.as_bytes()) == b"value")
                    .map(|(_, value)| value.as_str());
                if key == Some("plater_id") {
                    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
                        current_plate = Some(format!("source plate {value}"));
                    }
                    buffer.clear();
                    continue;
                }
                let Some(key) = key.filter(|key| TARGET_OVERRIDE_KEYS.contains(key)) else {
                    buffer.clear();
                    continue;
                };
                let plate = current_plate.as_deref().unwrap_or("source plate");
                if !seen_overrides.insert((plate_ordinal, key.to_owned())) {
                    return Err(U1DirectError::Plan(format!(
                        "{plate} contains duplicate per-plate override {key:?}"
                    )));
                }
                let value = value.ok_or_else(|| {
                    U1DirectError::Plan(format!("{plate} per-plate override {key:?} has no value"))
                })?;
                if !source_plate_override_matches_target(key, value) {
                    return Err(U1DirectError::Plan(format!(
                        "{plate} overrides {key:?} with {value:?}; Stage B intentionally targets Textured PEI, by-layer, non-spiral U1 printing and cannot silently discard this source behavior"
                    )));
                }
                normalized_count += 1;
            }
            Event::End(event) => {
                let event_name = event.name();
                let closes_plate =
                    depth.saturating_sub(1) == 1 && local_xml_name(event_name.as_ref()) == b"plate";
                depth = depth.saturating_sub(1);
                if closes_plate {
                    current_plate = None;
                }
            }
            Event::DocType(_) => {
                return Err(U1DirectError::Xml {
                    path: MODEL_SETTINGS_PATH.into(),
                    message: "DOCTYPE is forbidden".into(),
                });
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(normalized_count)
}

fn source_plate_override_matches_target(key: &str, value: &str) -> bool {
    let value = value.trim();
    match key {
        "bed_type" => value == "Textured PEI Plate",
        "print_sequence" => value == "by layer",
        "first_layer_print_sequence" | "other_layers_print_sequence" => {
            value.split_whitespace().eq(["0"])
        }
        "other_layers_print_sequence_nums" => value.parse::<i64>() == Ok(0),
        "spiral_mode" => value == "0" || value.eq_ignore_ascii_case("false"),
        "timelapse_type" => value == "0",
        _ => false,
    }
}

fn build_artifact_plans(
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    verified_profiles: &VerifiedProfileStore,
    base_name: &str,
    source_bed_size: f64,
) -> Result<Vec<ArtifactBuildPlan>, U1DirectError> {
    if result.batches.is_empty() {
        return Err(U1DirectError::Plan(
            "the plan contains no print batches".into(),
        ));
    }
    let scoped_units = input
        .scopes
        .iter()
        .flat_map(|scope| {
            scope
                .units
                .iter()
                .map(move |unit| ((scope.id.as_str(), unit.id.as_str()), unit))
        })
        .collect::<BTreeMap<_, _>>();
    let scoped_unit_count = input
        .scopes
        .iter()
        .map(|scope| scope.units.len())
        .sum::<usize>();
    if scoped_units.len() != scoped_unit_count {
        return Err(U1DirectError::Plan(
            "planning input contains duplicate scope/unit identities".into(),
        ));
    }
    let all_units_by_source = input
        .scopes
        .iter()
        .flat_map(|scope| scope.units.iter())
        .map(|unit| (unit.source_unit_id.as_str(), unit))
        .collect::<BTreeMap<_, _>>();
    if all_units_by_source.len() != scoped_unit_count {
        return Err(U1DirectError::Plan(
            "planning input contains duplicate source-unit identities".into(),
        ));
    }
    let jobs = result
        .jobs
        .iter()
        .map(|job| (job.id.as_str(), job))
        .collect::<BTreeMap<_, _>>();
    if jobs.len() != result.jobs.len() {
        return Err(U1DirectError::Plan(
            "canonical plan contains duplicate job IDs".into(),
        ));
    }
    let plates = result
        .plates
        .iter()
        .map(|plate| (plate.id.as_str(), plate))
        .collect::<BTreeMap<_, _>>();
    if plates.len() != result.plates.len() {
        return Err(U1DirectError::Plan(
            "canonical plan contains duplicate target-plate IDs".into(),
        ));
    }
    let analysis_plates = analysis
        .plates
        .iter()
        .enumerate()
        .map(|(index, plate)| (plate.id, (index, plate)))
        .collect::<BTreeMap<_, _>>();
    if analysis_plates.len() != analysis.plates.len() {
        return Err(U1DirectError::Plan(
            "source analysis contains duplicate plate IDs".into(),
        ));
    }
    let analysis_objects = analysis
        .objects
        .iter()
        .filter(|object| object.source_model_path.is_none())
        .map(|object| (object.source_object_id.unwrap_or(object.id), object))
        .collect::<BTreeMap<_, _>>();
    let analysis_root_object_count = analysis
        .objects
        .iter()
        .filter(|object| object.source_model_path.is_none())
        .count();
    if analysis_objects.len() != analysis_root_object_count {
        return Err(U1DirectError::Plan(
            "source analysis contains duplicate root object IDs".into(),
        ));
    }

    let mut scheduled_source_units = BTreeSet::new();
    let mut artifacts = Vec::with_capacity(result.batches.len());
    for (batch_index, batch) in result.batches.iter().enumerate() {
        ensure_direct_u1_batch(batch)?;
        let batch_jobs = batch
            .job_ids
            .iter()
            .map(|job_id| {
                jobs.get(job_id.as_str()).copied().ok_or_else(|| {
                    U1DirectError::Plan(format!("batch references missing job {job_id}"))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        for job in &batch_jobs {
            ensure_direct_u1_job(job)?;
        }
        let loadout = match &batch.loadout {
            PrinterLoadout::U1 { loadout } => loadout,
            _ => unreachable!("ensure_direct_u1_batch already checked the loadout"),
        };
        let mut units_by_source_plate = BTreeMap::<u32, Vec<PrintableUnit>>::new();
        let mut planned_target_plates = Vec::<&PlannedPlate>::new();
        let mut target_plate_ids = Vec::new();
        let mut job_by_ref = BTreeMap::new();
        let mut job_by_source_unit = BTreeMap::new();
        let mut consumed_job_refs = BTreeSet::new();
        for job in &batch_jobs {
            for unit_ref in &job.units {
                if !scoped_units
                    .contains_key(&(unit_ref.scope_id.as_str(), unit_ref.unit_id.as_str()))
                {
                    return Err(U1DirectError::Plan(format!(
                        "job {} references unknown unit {}/{}",
                        job.id, unit_ref.scope_id, unit_ref.unit_id
                    )));
                }
                if job_by_ref
                    .insert(
                        (unit_ref.scope_id.as_str(), unit_ref.unit_id.as_str()),
                        *job,
                    )
                    .is_some()
                {
                    return Err(U1DirectError::Plan(format!(
                        "unit {}/{} is scheduled more than once in batch {}",
                        unit_ref.scope_id, unit_ref.unit_id, batch.id
                    )));
                }
            }
        }
        for target_plate_id in &batch.plate_ids {
            let planned_plate = plates
                .get(target_plate_id.as_str())
                .copied()
                .ok_or_else(|| {
                    U1DirectError::Plan(format!(
                        "batch references missing target plate {target_plate_id}"
                    ))
                })?;
            ensure_direct_u1_plate(planned_plate)?;
            target_plate_ids.push(target_plate_id.clone());
            for unit_ref in &planned_plate.units {
                let unit = scoped_units
                    .get(&(unit_ref.scope_id.as_str(), unit_ref.unit_id.as_str()))
                    .copied()
                    .ok_or_else(|| {
                        U1DirectError::Plan(format!(
                            "target plate references unknown unit {}/{}",
                            unit_ref.scope_id, unit_ref.unit_id
                        ))
                    })?;
                let exact_job = job_by_ref
                    .get(&(unit_ref.scope_id.as_str(), unit_ref.unit_id.as_str()))
                    .copied()
                    .ok_or_else(|| {
                        U1DirectError::Plan(format!(
                            "target plate unit {}/{} is not attached to a batch job",
                            unit_ref.scope_id, unit_ref.unit_id
                        ))
                    })?;
                if !consumed_job_refs
                    .insert((unit_ref.scope_id.as_str(), unit_ref.unit_id.as_str()))
                {
                    return Err(U1DirectError::Plan(format!(
                        "unit {}/{} is placed on more than one target plate in batch {}",
                        unit_ref.scope_id, unit_ref.unit_id, batch.id
                    )));
                }
                if let Some(previous) =
                    job_by_source_unit.insert(unit.source_unit_id.as_str(), exact_job)
                    && previous.id != exact_job.id
                {
                    return Err(U1DirectError::Plan(format!(
                        "source unit {} is attached to multiple batch jobs",
                        unit.source_unit_id
                    )));
                }
                let source_plate_id = parse_source_plate_id(unit.source_plate_id.as_deref())?;
                if !scheduled_source_units.insert(unit.source_unit_id.clone()) {
                    return Err(U1DirectError::Plan(format!(
                        "source unit {} is scheduled more than once",
                        unit.source_unit_id
                    )));
                }
                units_by_source_plate
                    .entry(source_plate_id)
                    .or_default()
                    .push(unit.clone());
            }
            planned_target_plates.push(planned_plate);
        }
        if consumed_job_refs.len() != job_by_ref.len()
            || consumed_job_refs
                .iter()
                .any(|unit_ref| !job_by_ref.contains_key(unit_ref))
        {
            return Err(U1DirectError::Plan(format!(
                "batch {} has job units that are not placed on exactly one target plate",
                batch.id
            )));
        }
        if units_by_source_plate.is_empty() {
            return Err(U1DirectError::Plan(format!(
                "batch {} contains no printable units",
                batch.id
            )));
        }
        let mut preserve_source_layout = planned_target_plates.len() == units_by_source_plate.len();
        let mut target_source_plates = BTreeSet::new();
        for planned_plate in &planned_target_plates {
            let source_plate_ids = planned_plate
                .units
                .iter()
                .map(|unit_ref| {
                    scoped_units
                        .get(&(unit_ref.scope_id.as_str(), unit_ref.unit_id.as_str()))
                        .copied()
                        .ok_or_else(|| {
                            U1DirectError::Plan(format!(
                                "target plate references unknown unit {}/{}",
                                unit_ref.scope_id, unit_ref.unit_id
                            ))
                        })
                        .and_then(|unit| parse_source_plate_id(unit.source_plate_id.as_deref()))
                })
                .collect::<Result<BTreeSet<_>, _>>()?;
            if source_plate_ids.len() != 1
                || !target_source_plates.insert(*source_plate_ids.first().expect("one source"))
            {
                preserve_source_layout = false;
            }
        }
        for (source_plate_id, selected) in &units_by_source_plate {
            let (_, source_plate) =
                analysis_plates
                    .get(source_plate_id)
                    .copied()
                    .ok_or_else(|| {
                        U1DirectError::Plan(format!(
                            "source plate {source_plate_id} is missing from analysis"
                        ))
                    })?;
            let expected_instances = source_plate
                .instances
                .iter()
                .filter(|instance| instance.printable)
                .map(|instance| (instance.object_id, instance.instance_id))
                .collect::<BTreeSet<_>>();
            let actual_instances = selected
                .iter()
                .map(|unit| (unit.source_object_id, unit.source_instance_id))
                .collect::<BTreeSet<_>>();
            if actual_instances.len() != selected.len() || expected_instances != actual_instances {
                preserve_source_layout = false;
            }
        }

        let target_plate_count = if preserve_source_layout {
            units_by_source_plate.len()
        } else {
            planned_target_plates.len()
        };
        let mut artifact_plates = Vec::with_capacity(target_plate_count);
        let mut selected_instances = BTreeMap::new();
        let mut used_source_identify_ids = BTreeSet::new();
        let mut object_slot_maps = BTreeMap::new();
        let mut resource_slot_maps = BTreeMap::new();
        let mut selected_root_resource_ids = BTreeSet::new();
        let mut external_paths = BTreeSet::new();
        if preserve_source_layout {
            for (target_index, (source_plate_id, units)) in
                units_by_source_plate.into_iter().enumerate()
            {
                let (source_index, source_plate) = analysis_plates
                    .get(&source_plate_id)
                    .copied()
                    .ok_or_else(|| {
                    U1DirectError::Plan(format!(
                        "source plate {source_plate_id} is missing from analysis"
                    ))
                })?;
                validate_preserved_plate_bounds(
                    source_plate,
                    source_index,
                    analysis.plates.len(),
                    source_bed_size,
                )?;
                let source_origin =
                    virtual_plate_origin(source_index, analysis.plates.len(), source_bed_size);
                let target_origin =
                    virtual_plate_origin(target_index, target_plate_count, TARGET_BED_SIZE_MM);
                let placement = InstancePlacement {
                    delta_x: target_origin.0 - source_origin.0,
                    delta_y: target_origin.1 - source_origin.1,
                    delta_z: 0.0,
                };
                let mut used_target_slots = BTreeSet::new();
                for unit in &units {
                    let slot_map = register_direct_unit_resources(
                        unit,
                        &job_by_source_unit,
                        loadout,
                        &analysis_objects,
                        &mut object_slot_maps,
                        &mut resource_slot_maps,
                        &mut selected_root_resource_ids,
                        &mut external_paths,
                    )?;
                    used_target_slots.extend(slot_map.values().copied());
                    selected_instances
                        .insert((unit.source_object_id, unit.source_instance_id), placement);
                }
                let (wipe_tower_x, wipe_tower_y) = choose_wipe_tower_position(
                    source_plate,
                    source_origin,
                    used_target_slots.len(),
                )?;
                let source_identify_ids = units
                    .iter()
                    .map(|unit| {
                        let identify_id = source_plate
                            .instances
                            .iter()
                            .find(|instance| {
                                instance.object_id == unit.source_object_id
                                    && instance.instance_id == unit.source_instance_id
                                    && instance.printable
                            })
                            .and_then(|instance| instance.identify_id)
                            .ok_or_else(|| {
                                U1DirectError::Plan(format!(
                                    "source unit {} has no preserved Orca identify_id",
                                    unit.source_unit_id
                                ))
                            })?;
                        Ok((
                            (unit.source_object_id, unit.source_instance_id),
                            identify_id,
                        ))
                    })
                    .collect::<Result<BTreeMap<_, _>, U1DirectError>>()?;
                if source_identify_ids.len() != units.len() {
                    return Err(U1DirectError::Plan(format!(
                        "source plate {source_plate_id} contains duplicate object/instance identities"
                    )));
                }
                for identify_id in source_identify_ids.values().copied() {
                    if identify_id == 0 || !used_source_identify_ids.insert(identify_id) {
                        return Err(U1DirectError::Plan(format!(
                            "source identify_id {identify_id} is missing or duplicated across the target artifact"
                        )));
                    }
                }
                artifact_plates.push(ArtifactPlate {
                    source_plate_ids: vec![source_plate_id],
                    target_plate_id: (target_index + 1) as u32,
                    name: source_plate
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("Plate {source_plate_id:02}")),
                    units,
                    source_identify_ids,
                    wipe_tower_x,
                    wipe_tower_y,
                });
            }
        } else {
            for (target_index, planned_plate) in planned_target_plates.iter().enumerate() {
                if planned_plate.packing_status != PackingStatus::PackedAabb
                    || !planned_plate.individual_bounds_validated
                {
                    return Err(U1DirectError::Plan(format!(
                        "target plate {} is not backed by a fully validated packed layout",
                        planned_plate.id
                    )));
                }
                let placement_by_ref = planned_plate
                    .placements
                    .iter()
                    .map(|placement| (placement.unit.clone(), placement))
                    .collect::<BTreeMap<_, _>>();
                if placement_by_ref.len() != planned_plate.placements.len()
                    || placement_by_ref.len() != planned_plate.units.len()
                {
                    return Err(U1DirectError::Plan(format!(
                        "target plate {} does not contain exactly one placement per unit",
                        planned_plate.id
                    )));
                }
                let target_origin =
                    virtual_plate_origin(target_index, target_plate_count, TARGET_BED_SIZE_MM);
                let mut units = Vec::with_capacity(planned_plate.units.len());
                let mut source_plate_ids = BTreeSet::new();
                let mut source_identify_ids = BTreeMap::new();
                let mut used_target_slots = BTreeSet::new();
                let mut target_object_bounds = Vec::<[f64; 4]>::new();
                let mut maximum_printable_z = 0.0_f64;
                for unit_ref in &planned_plate.units {
                    let unit = scoped_units
                        .get(&(unit_ref.scope_id.as_str(), unit_ref.unit_id.as_str()))
                        .copied()
                        .ok_or_else(|| {
                            U1DirectError::Plan(format!(
                                "target plate references unknown unit {}/{}",
                                unit_ref.scope_id, unit_ref.unit_id
                            ))
                        })?;
                    let placement = placement_by_ref.get(unit_ref).copied().ok_or_else(|| {
                        U1DirectError::Plan(format!(
                            "target plate {} has no placement for {}/{}",
                            planned_plate.id, unit_ref.scope_id, unit_ref.unit_id
                        ))
                    })?;
                    let source_plate_id = parse_source_plate_id(unit.source_plate_id.as_deref())?;
                    source_plate_ids.insert(source_plate_id);
                    let (_, source_plate) = analysis_plates
                        .get(&source_plate_id)
                        .copied()
                        .ok_or_else(|| {
                            U1DirectError::Plan(format!(
                                "source plate {source_plate_id} is missing from analysis"
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
                            U1DirectError::Plan(format!(
                                "source unit {} has no matching printable source instance",
                                unit.source_unit_id
                            ))
                        })?;
                    let source_bounds = source_instance.printable_bounds.ok_or_else(|| {
                        U1DirectError::Plan(format!(
                            "source unit {} has unknown printable bounds",
                            unit.source_unit_id
                        ))
                    })?;
                    let source_width = source_bounds.max[0] - source_bounds.min[0];
                    let source_depth = source_bounds.max[1] - source_bounds.min[1];
                    let source_height = source_bounds.max[2] - source_bounds.min[2];
                    if !unit.bounds.is_valid()
                        || !unit.bounds.has_known_size()
                        || (source_width - unit.bounds.width).abs() > TARGET_BOUNDS_TOLERANCE_MM
                        || (source_depth - unit.bounds.depth).abs() > TARGET_BOUNDS_TOLERANCE_MM
                        || (source_height - unit.bounds.height).abs() > TARGET_BOUNDS_TOLERANCE_MM
                    {
                        return Err(U1DirectError::Plan(format!(
                            "source bounds for unit {} no longer match the canonical packing input",
                            unit.source_unit_id
                        )));
                    }
                    let local_bounds = [
                        placement.target_min_x_mm,
                        placement.target_min_y_mm,
                        placement.target_min_x_mm + source_width,
                        placement.target_min_y_mm + source_depth,
                    ];
                    if !local_bounds.into_iter().all(f64::is_finite)
                        || local_bounds[0] - unit.bounds.clearance_x
                            < TARGET_MIN_X_MM - TARGET_BOUNDS_TOLERANCE_MM
                        || local_bounds[1] - unit.bounds.clearance_y
                            < TARGET_MIN_Y_MM - TARGET_BOUNDS_TOLERANCE_MM
                        || local_bounds[2] + unit.bounds.clearance_x
                            > TARGET_MAX_X_MM + TARGET_BOUNDS_TOLERANCE_MM
                        || local_bounds[3] + unit.bounds.clearance_y
                            > TARGET_MAX_Y_MM + TARGET_BOUNDS_TOLERANCE_MM
                        || source_bounds.max[2] <= 0.0
                        || source_bounds.max[2] + unit.bounds.clearance_z
                            > TARGET_PRINTABLE_HEIGHT_MM + TARGET_BOUNDS_TOLERANCE_MM
                    {
                        return Err(U1DirectError::Plan(format!(
                            "packed unit {} is outside the qualified U1 build volume",
                            unit.source_unit_id
                        )));
                    }
                    let instance_placement = InstancePlacement {
                        delta_x: target_origin.0 + placement.target_min_x_mm - source_bounds.min[0],
                        delta_y: target_origin.1 + placement.target_min_y_mm - source_bounds.min[1],
                        delta_z: 0.0,
                    };
                    if selected_instances
                        .insert(
                            (unit.source_object_id, unit.source_instance_id),
                            instance_placement,
                        )
                        .is_some()
                    {
                        return Err(U1DirectError::Plan(format!(
                            "source unit {} is placed more than once in one artifact",
                            unit.source_unit_id
                        )));
                    }
                    let slot_map = register_direct_unit_resources(
                        unit,
                        &job_by_source_unit,
                        loadout,
                        &analysis_objects,
                        &mut object_slot_maps,
                        &mut resource_slot_maps,
                        &mut selected_root_resource_ids,
                        &mut external_paths,
                    )?;
                    used_target_slots.extend(slot_map.values().copied());
                    let identify_id = source_instance.identify_id.ok_or_else(|| {
                        U1DirectError::Plan(format!(
                            "source unit {} has no preserved Orca identify_id",
                            unit.source_unit_id
                        ))
                    })?;
                    if source_identify_ids
                        .insert(
                            (unit.source_object_id, unit.source_instance_id),
                            identify_id,
                        )
                        .is_some()
                    {
                        return Err(U1DirectError::Plan(format!(
                            "target plate {} contains duplicate source instance identities",
                            planned_plate.id
                        )));
                    }
                    maximum_printable_z = maximum_printable_z.max(source_bounds.max[2]);
                    target_object_bounds.push(local_bounds);
                    units.push(unit.clone());
                }
                for identify_id in source_identify_ids.values().copied() {
                    if identify_id == 0 || !used_source_identify_ids.insert(identify_id) {
                        return Err(U1DirectError::Plan(format!(
                            "source identify_id {identify_id} is missing or duplicated across the target artifact"
                        )));
                    }
                }
                let tower_required = used_target_slots.len() > 1;
                if tower_required != planned_plate.prime_tower.is_some() {
                    return Err(U1DirectError::Plan(format!(
                        "target plate {} prime-tower reservation does not match its physical tool usage",
                        planned_plate.id
                    )));
                }
                let (wipe_tower_x, wipe_tower_y) = if let Some(tower) = planned_plate.prime_tower {
                    let envelope = conservative_prime_tower_envelope(
                        tower.x_mm,
                        tower.y_mm,
                        maximum_printable_z,
                    );
                    if envelope[0] < TARGET_MIN_X_MM
                        || envelope[1] < TARGET_MIN_Y_MM
                        || envelope[2] > TARGET_MAX_X_MM
                        || envelope[3] > TARGET_MAX_Y_MM
                        || target_object_bounds.iter().any(|bounds| {
                            rectangles_overlap(
                                envelope,
                                [
                                    bounds[0] - OBJECT_FOOTPRINT_CLEARANCE_MM,
                                    bounds[1] - OBJECT_FOOTPRINT_CLEARANCE_MM,
                                    bounds[2] + OBJECT_FOOTPRINT_CLEARANCE_MM,
                                    bounds[3] + OBJECT_FOOTPRINT_CLEARANCE_MM,
                                ],
                            )
                        })
                    {
                        return Err(U1DirectError::Plan(format!(
                            "target plate {} does not preserve the qualified U1 prime-tower envelope",
                            planned_plate.id
                        )));
                    }
                    (tower.x_mm, tower.y_mm)
                } else {
                    (40.0, 200.0)
                };
                let canonical_source_plate_ids = canonical_source_plate_ids(&units)?;
                if canonical_source_plate_ids
                    .iter()
                    .copied()
                    .collect::<BTreeSet<_>>()
                    != source_plate_ids
                {
                    return Err(U1DirectError::Plan(format!(
                        "target plate {} has inconsistent source provenance",
                        planned_plate.id
                    )));
                }
                artifact_plates.push(ArtifactPlate {
                    source_plate_ids: canonical_source_plate_ids,
                    target_plate_id: (target_index + 1) as u32,
                    // Packed plates are freshly authored target metadata. A
                    // source-derived name would be ambiguous once one target
                    // contains units from more than one immutable source plate.
                    name: format!(
                        "Packed U1 plate {:02} — {}",
                        target_index + 1,
                        planned_plate.id
                    ),
                    units,
                    source_identify_ids,
                    wipe_tower_x,
                    wipe_tower_y,
                });
            }
        }

        let physical_slots = resolve_physical_slots_verified(verified_profiles, input, loadout)?;
        let project_settings = build_project_settings_verified(
            verified_profiles,
            &physical_slots,
            &artifact_plates,
            &analysis.process,
        )?;
        let prepared_slots = prepared_physical_slots(input, loadout, &physical_slots)?;
        let source_plate_ids = artifact_plates
            .iter()
            .flat_map(|plate| plate.source_plate_ids.iter().copied())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let source_unit_ids = artifact_plates
            .iter()
            .flat_map(|plate| plate.units.iter().map(|unit| unit.source_unit_id.clone()))
            .collect::<Vec<_>>();
        artifacts.push(ArtifactBuildPlan {
            prepared: U1DirectPreparedArtifact {
                batch_id: batch.id.clone(),
                file_name: format!(
                    "{base_name}__U1__batch-{:02}__{}.3mf",
                    batch_index + 1,
                    match batch.strategy {
                        ColorStrategy::DirectSpools => "Direct-Spools",
                        ColorStrategy::CmyxSolid => "CMY-X-Solid",
                        ColorStrategy::CmyxFullSpectrum | ColorStrategy::A1Mono => {
                            unreachable!("plain U1 writer strategy validated above")
                        }
                    }
                ),
                target_plate_ids,
                source_plate_ids,
                source_unit_ids,
                loadout: prepared_slots,
                setup_actions: batch
                    .setup_actions
                    .iter()
                    .map(|action| format_setup_action(action, &input.inventory))
                    .collect(),
            },
            plates: artifact_plates,
            object_slot_maps,
            resource_slot_maps,
            selected_root_resource_ids,
            external_paths,
            selected_instances,
            physical_slots,
            project_settings,
        });
    }

    let expected_source_units = all_units_by_source.keys().copied().collect::<BTreeSet<_>>();
    let actual_source_units = scheduled_source_units
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if expected_source_units != actual_source_units {
        let omitted = expected_source_units
            .difference(&actual_source_units)
            .copied()
            .collect::<Vec<_>>();
        return Err(U1DirectError::Plan(format!(
            "{} source unit(s) are omitted from the canonical plan: {}",
            omitted.len(),
            omitted.join(", ")
        )));
    }
    Ok(artifacts)
}

fn ensure_direct_u1_batch(batch: &PlannedBatch) -> Result<(), U1DirectError> {
    if batch.printer != Printer::U1
        || !matches!(
            batch.strategy,
            ColorStrategy::DirectSpools | ColorStrategy::CmyxSolid
        )
    {
        return Err(U1DirectError::Plan(format!(
            "batch {} uses {:?}/{:?}; the plain U1 writer supports only Direct Spools or physically solid CMY+X batches",
            batch.id, batch.printer, batch.strategy
        )));
    }
    if !matches!(batch.loadout, PrinterLoadout::U1 { .. }) {
        return Err(U1DirectError::Plan(format!(
            "batch {} does not have a U1 T1–T4 loadout",
            batch.id
        )));
    }
    Ok(())
}

fn ensure_direct_u1_job(job: &PlannedJob) -> Result<(), U1DirectError> {
    if job.printer != Printer::U1
        || !matches!(
            job.strategy,
            ColorStrategy::DirectSpools | ColorStrategy::CmyxSolid
        )
    {
        return Err(U1DirectError::Plan(format!(
            "job {} is not a plain physical-spool Snapmaker U1 job",
            job.id
        )));
    }
    if job.full_spectrum_process.is_some() {
        return Err(U1DirectError::Plan(format!(
            "job {} contains Full Spectrum process data, which belongs to Stage D",
            job.id
        )));
    }
    Ok(())
}

fn ensure_direct_u1_plate(plate: &PlannedPlate) -> Result<(), U1DirectError> {
    if plate.printer != Printer::U1 || plate.full_spectrum_process.is_some() {
        return Err(U1DirectError::Plan(format!(
            "target plate {} is not a plain U1 Direct plate",
            plate.id
        )));
    }
    Ok(())
}

fn parse_source_plate_id(value: Option<&str>) -> Result<u32, U1DirectError> {
    value
        .and_then(|value| value.strip_prefix("plate-"))
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| U1DirectError::Plan("a printable unit has no stable source plate ID".into()))
}

fn canonical_source_plate_ids(units: &[PrintableUnit]) -> Result<Vec<u32>, U1DirectError> {
    if units.is_empty() {
        return Err(U1DirectError::Plan(
            "a target plate contains no printable units".into(),
        ));
    }
    units
        .iter()
        .map(|unit| parse_source_plate_id(unit.source_plate_id.as_deref()))
        .collect::<Result<BTreeSet<_>, _>>()
        .map(|ids| ids.into_iter().collect())
}

fn find_job_for_unit<'a>(
    unit: &PrintableUnit,
    job_by_source_unit: &BTreeMap<&str, &'a PlannedJob>,
) -> Result<&'a PlannedJob, U1DirectError> {
    job_by_source_unit
        .get(unit.source_unit_id.as_str())
        .copied()
        .ok_or_else(|| {
            U1DirectError::Plan(format!(
                "source unit {} is not attached to a batch job",
                unit.source_unit_id
            ))
        })
}

#[allow(clippy::too_many_arguments)]
fn register_direct_unit_resources(
    unit: &PrintableUnit,
    job_by_source_unit: &BTreeMap<&str, &PlannedJob>,
    loadout: &u1_planner::U1Loadout,
    analysis_objects: &BTreeMap<u32, &u1_three_mf::ObjectAnalysis>,
    object_slot_maps: &mut BTreeMap<u32, BTreeMap<u8, u8>>,
    resource_slot_maps: &mut BTreeMap<(String, u32), BTreeMap<u8, u8>>,
    selected_root_resource_ids: &mut BTreeSet<u32>,
    external_paths: &mut BTreeSet<String>,
) -> Result<BTreeMap<u8, u8>, U1DirectError> {
    if unit.source_model_path.is_some() {
        return Err(U1DirectError::Plan(format!(
            "source unit {} builds an external model directly; this layout is not qualified for the U1 writer",
            unit.source_unit_id
        )));
    }
    let job = find_job_for_unit(unit, job_by_source_unit)?;
    let slot_map = direct_slot_map_for_unit(unit, job, loadout)?;
    merge_slot_map(
        object_slot_maps,
        unit.source_object_id,
        &slot_map,
        "source object",
    )?;
    selected_root_resource_ids.insert(unit.source_object_id);
    let source_object = analysis_objects
        .get(&unit.source_object_id)
        .copied()
        .ok_or_else(|| {
            U1DirectError::Plan(format!(
                "source object {} for unit {} is unavailable",
                unit.source_object_id, unit.source_unit_id
            ))
        })?;
    for part in &source_object.parts {
        let path = part
            .component_path
            .as_deref()
            .unwrap_or(MAIN_MODEL_PATH)
            .trim_start_matches('/')
            .to_owned();
        merge_resource_slot_map(resource_slot_maps, (path.clone(), part.id), &slot_map)?;
        if path == MAIN_MODEL_PATH {
            selected_root_resource_ids.insert(part.id);
        } else {
            external_paths.insert(path);
        }
    }
    Ok(slot_map)
}

fn direct_slot_map_for_unit(
    unit: &PrintableUnit,
    job: &PlannedJob,
    loadout: &u1_planner::U1Loadout,
) -> Result<BTreeMap<u8, u8>, U1DirectError> {
    let requirements = unit.requirement_ids.iter().collect::<BTreeSet<_>>();
    let mut mapping = BTreeMap::new();
    for color_mapping in &job.color_mappings {
        if !color_mapping
            .source_requirement_ids
            .iter()
            .any(|id| requirements.contains(id))
        {
            continue;
        }
        if color_mapping.strategy != job.strategy {
            return Err(U1DirectError::Plan(format!(
                "physical mapping in job {} does not preserve the canonical job strategy",
                job.id
            )));
        }
        let toolhead = match job.strategy {
            ColorStrategy::DirectSpools => color_mapping.direct_toolhead.ok_or_else(|| {
                U1DirectError::Plan(format!(
                    "Direct mapping in job {} has no physical toolhead",
                    job.id
                ))
            })?,
            ColorStrategy::CmyxSolid => {
                if color_mapping.direct_toolhead.is_some() {
                    return Err(U1DirectError::Plan(format!(
                        "CMY+X solid mapping in job {} unexpectedly contains a Direct toolhead override",
                        job.id
                    )));
                }
                match color_mapping.cmyx_comparison.recipe {
                    u1_planner::CmyxRecipe::Solid { toolhead } => toolhead,
                    u1_planner::CmyxRecipe::DedicatedT4 => Toolhead::T4,
                    _ => {
                        return Err(U1DirectError::Plan(format!(
                            "CMY+X solid mapping in job {} contains a non-solid recipe",
                            job.id
                        )));
                    }
                }
            }
            ColorStrategy::CmyxFullSpectrum | ColorStrategy::A1Mono => {
                return Err(U1DirectError::Plan(format!(
                    "job {} is not compatible with the plain physical-spool writer",
                    job.id
                )));
            }
        };
        let expected_spool = loadout.spool(toolhead).ok_or_else(|| {
            U1DirectError::Plan(format!(
                "{} is empty in job {}",
                toolhead_name(toolhead),
                job.id
            ))
        })?;
        if color_mapping.actual_spool_id.as_deref() != Some(expected_spool) {
            return Err(U1DirectError::Plan(format!(
                "mapping spool for {} does not match the physical batch loadout",
                toolhead_name(toolhead)
            )));
        }
        let target = (toolhead.index() + 1) as u8;
        for source_slot in &color_mapping.source_slots {
            let source = parse_source_slot(source_slot)?;
            if let Some(previous) = mapping.insert(source, target)
                && previous != target
            {
                return Err(U1DirectError::Plan(format!(
                    "source slot F{source} maps to multiple U1 toolheads in job {}",
                    job.id
                )));
            }
        }
    }
    if mapping.is_empty() {
        return Err(U1DirectError::Plan(format!(
            "source unit {} has no Direct color mapping",
            unit.source_unit_id
        )));
    }
    Ok(mapping)
}

fn parse_source_slot(value: &str) -> Result<u8, U1DirectError> {
    value
        .trim()
        .strip_prefix(['F', 'f'])
        .and_then(|value| value.parse::<u8>().ok())
        .filter(|value| (1..=32).contains(value))
        .ok_or_else(|| U1DirectError::Plan(format!("invalid source filament slot {value:?}")))
}

fn merge_slot_map(
    maps: &mut BTreeMap<u32, BTreeMap<u8, u8>>,
    id: u32,
    candidate: &BTreeMap<u8, u8>,
    label: &str,
) -> Result<(), U1DirectError> {
    if let Some(existing) = maps.get(&id) {
        if existing != candidate {
            return Err(U1DirectError::Plan(format!(
                "{label} {id} would require conflicting physical mappings in one output file"
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
) -> Result<(), U1DirectError> {
    if let Some(existing) = maps.get(&key) {
        if existing != candidate {
            return Err(U1DirectError::Plan(format!(
                "model resource {}/{} is shared by incompatible Direct mappings",
                key.0, key.1
            )));
        }
    } else {
        maps.insert(key, candidate.clone());
    }
    Ok(())
}

fn build_full_spectrum_substrate_plan(
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    artifact: &U1FullSpectrumPreparedArtifact,
    project_settings: Vec<u8>,
) -> Result<ArtifactBuildPlan, U1DirectError> {
    if artifact.plates.is_empty() {
        return Err(U1DirectError::Plan(
            "the Full Spectrum artifact contains no target plates".into(),
        ));
    }
    let scoped_units = input
        .scopes
        .iter()
        .flat_map(|scope| {
            scope
                .units
                .iter()
                .map(move |unit| ((scope.id.as_str(), unit.id.as_str()), unit))
        })
        .collect::<BTreeMap<_, _>>();
    let expected_unit_count = input
        .scopes
        .iter()
        .map(|scope| scope.units.len())
        .sum::<usize>();
    if scoped_units.len() != expected_unit_count {
        return Err(U1DirectError::Plan(
            "planning input contains duplicate scope/unit identities".into(),
        ));
    }
    let analysis_plates = analysis
        .plates
        .iter()
        .map(|plate| (plate.id, plate))
        .collect::<BTreeMap<_, _>>();
    if analysis_plates.len() != analysis.plates.len() {
        return Err(U1DirectError::Plan(
            "source analysis contains duplicate plate IDs".into(),
        ));
    }
    let analysis_objects = analysis
        .objects
        .iter()
        .filter(|object| object.source_model_path.is_none())
        .map(|object| (object.source_object_id.unwrap_or(object.id), object))
        .collect::<BTreeMap<_, _>>();
    if analysis_objects.len()
        != analysis
            .objects
            .iter()
            .filter(|object| object.source_model_path.is_none())
            .count()
    {
        return Err(U1DirectError::Plan(
            "source analysis contains duplicate root object IDs".into(),
        ));
    }

    let mut selected_instances = BTreeMap::new();
    let mut object_slot_maps = BTreeMap::new();
    let mut resource_slot_maps = BTreeMap::new();
    let mut selected_root_resource_ids = BTreeSet::new();
    let mut external_paths = BTreeSet::new();
    let mut artifact_plates = Vec::with_capacity(artifact.plates.len());
    let mut all_source_unit_ids = BTreeSet::new();
    let mut all_source_plate_ids = BTreeSet::new();
    let mut used_source_identify_ids = BTreeSet::new();

    for (plate_index, prepared_plate) in artifact.plates.iter().enumerate() {
        let expected_target_id = u32::try_from(plate_index + 1)
            .map_err(|_| U1DirectError::Plan("target plate count exceeds u32".into()))?;
        if prepared_plate.target_plate_id != expected_target_id {
            return Err(U1DirectError::Plan(format!(
                "Full Spectrum target plate IDs must be dense and ordered from 1; found {} at index {}",
                prepared_plate.target_plate_id, plate_index
            )));
        }
        if prepared_plate.units.is_empty() {
            return Err(U1DirectError::Plan(format!(
                "Full Spectrum target plate {} is empty",
                prepared_plate.plan_plate_id
            )));
        }
        let target_origin =
            virtual_plate_origin(plate_index, artifact.plates.len(), TARGET_BED_SIZE_MM);
        let mut plate_units = Vec::with_capacity(prepared_plate.units.len());
        let mut source_plate_ids = BTreeSet::new();
        let mut source_identify_ids = BTreeMap::new();
        let mut cleared_footprints = Vec::<(String, [f64; 4])>::new();

        for prepared_unit in &prepared_plate.units {
            let unit = scoped_units
                .get(&(
                    prepared_unit.unit.scope_id.as_str(),
                    prepared_unit.unit.unit_id.as_str(),
                ))
                .copied()
                .ok_or_else(|| {
                    U1DirectError::Plan(format!(
                        "Full Spectrum target plate references unknown unit {}/{}",
                        prepared_unit.unit.scope_id, prepared_unit.unit.unit_id
                    ))
                })?;
            if unit.source_unit_id != prepared_unit.source_unit_id
                || unit.source_object_id != prepared_unit.source_object_id
                || unit.source_instance_id != prepared_unit.source_instance_id
                || unit.source_model_path != prepared_unit.source_model_path
                || unit.source_plate_id != prepared_unit.source_plate_id
            {
                return Err(U1DirectError::Plan(format!(
                    "prepared Full Spectrum unit {} no longer matches the canonical source identity",
                    prepared_unit.source_unit_id
                )));
            }
            if unit.source_model_path.is_some() {
                return Err(U1DirectError::Plan(format!(
                    "source unit {} builds an external model directly; this layout is not qualified for the U1 writer",
                    unit.source_unit_id
                )));
            }
            if !all_source_unit_ids.insert(unit.source_unit_id.clone()) {
                return Err(U1DirectError::Plan(format!(
                    "source unit {} is scheduled more than once in the Full Spectrum artifact",
                    unit.source_unit_id
                )));
            }
            let source_plate_id = parse_source_plate_id(unit.source_plate_id.as_deref())?;
            source_plate_ids.insert(source_plate_id);
            let source_plate = analysis_plates
                .get(&source_plate_id)
                .copied()
                .ok_or_else(|| {
                    U1DirectError::Plan(format!(
                        "source plate {source_plate_id} is missing from analysis"
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
                    U1DirectError::Plan(format!(
                        "source unit {} has no matching printable source instance",
                        unit.source_unit_id
                    ))
                })?;
            let source_bounds = source_instance.printable_bounds.ok_or_else(|| {
                U1DirectError::Plan(format!(
                    "source unit {} has unknown printable bounds",
                    unit.source_unit_id
                ))
            })?;
            let source_size = source_bounds.size().ok_or_else(|| {
                U1DirectError::Plan(format!(
                    "source unit {} has invalid printable bounds",
                    unit.source_unit_id
                ))
            })?;
            if !unit.bounds.is_valid()
                || !unit.bounds.has_known_size()
                || (source_size[0] - unit.bounds.width).abs() > TARGET_BOUNDS_TOLERANCE_MM
                || (source_size[1] - unit.bounds.depth).abs() > TARGET_BOUNDS_TOLERANCE_MM
                || (source_size[2] - unit.bounds.height).abs() > TARGET_BOUNDS_TOLERANCE_MM
            {
                return Err(U1DirectError::Plan(format!(
                    "source bounds for unit {} no longer match the canonical Full Spectrum packing input",
                    unit.source_unit_id
                )));
            }
            let local_bounds = [
                prepared_unit.target_min_x_mm,
                prepared_unit.target_min_y_mm,
                prepared_unit.target_min_x_mm + source_size[0],
                prepared_unit.target_min_y_mm + source_size[1],
            ];
            let cleared = [
                local_bounds[0] - unit.bounds.clearance_x,
                local_bounds[1] - unit.bounds.clearance_y,
                local_bounds[2] + unit.bounds.clearance_x,
                local_bounds[3] + unit.bounds.clearance_y,
            ];
            if !cleared.into_iter().all(f64::is_finite)
                || cleared[0] < TARGET_MIN_X_MM - TARGET_BOUNDS_TOLERANCE_MM
                || cleared[1] < TARGET_MIN_Y_MM - TARGET_BOUNDS_TOLERANCE_MM
                || cleared[2] > TARGET_MAX_X_MM + TARGET_BOUNDS_TOLERANCE_MM
                || cleared[3] > TARGET_MAX_Y_MM + TARGET_BOUNDS_TOLERANCE_MM
                || source_bounds.max[2] <= 0.0
                || source_bounds.max[2] + unit.bounds.clearance_z
                    > TARGET_PRINTABLE_HEIGHT_MM + TARGET_BOUNDS_TOLERANCE_MM
            {
                return Err(U1DirectError::Plan(format!(
                    "packed Full Spectrum unit {} is outside the qualified U1 build volume",
                    unit.source_unit_id
                )));
            }
            let instance_placement = InstancePlacement {
                delta_x: target_origin.0 + prepared_unit.target_min_x_mm - source_bounds.min[0],
                delta_y: target_origin.1 + prepared_unit.target_min_y_mm - source_bounds.min[1],
                delta_z: 0.0,
            };
            if selected_instances
                .insert(
                    (unit.source_object_id, unit.source_instance_id),
                    instance_placement,
                )
                .is_some()
            {
                return Err(U1DirectError::Plan(format!(
                    "source unit {} is placed more than once in the Full Spectrum artifact",
                    unit.source_unit_id
                )));
            }
            register_full_spectrum_unit_resources(
                unit,
                &prepared_unit.source_to_target_slots,
                &analysis_objects,
                &mut object_slot_maps,
                &mut resource_slot_maps,
                &mut selected_root_resource_ids,
                &mut external_paths,
            )?;
            let identify_id = source_instance.identify_id.ok_or_else(|| {
                U1DirectError::Plan(format!(
                    "source unit {} has no preserved Orca identify_id",
                    unit.source_unit_id
                ))
            })?;
            if source_identify_ids
                .insert(
                    (unit.source_object_id, unit.source_instance_id),
                    identify_id,
                )
                .is_some()
            {
                return Err(U1DirectError::Plan(format!(
                    "Full Spectrum target plate {} contains duplicate source instance identities",
                    prepared_plate.plan_plate_id
                )));
            }
            if identify_id == 0 || !used_source_identify_ids.insert(identify_id) {
                return Err(U1DirectError::Plan(format!(
                    "source identify_id {identify_id} is missing or duplicated across the Full Spectrum target artifact"
                )));
            }
            cleared_footprints.push((unit.source_unit_id.clone(), cleared));
            plate_units.push(unit.clone());
        }
        let canonical_source_plate_ids = canonical_source_plate_ids(&plate_units)?;
        if canonical_source_plate_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            != source_plate_ids
        {
            return Err(U1DirectError::Plan(format!(
                "Full Spectrum target plate {} has inconsistent source provenance",
                prepared_plate.plan_plate_id
            )));
        }
        validate_full_spectrum_footprints(
            &prepared_plate.plan_plate_id,
            &cleared_footprints,
            prepared_plate
                .prime_tower
                .map(|tower| (tower.x_mm, tower.y_mm)),
        )?;
        all_source_plate_ids.extend(canonical_source_plate_ids.iter().copied());
        let (wipe_tower_x, wipe_tower_y) = prepared_plate
            .prime_tower
            .map(|tower| (tower.x_mm, tower.y_mm))
            .unwrap_or((0.0, 0.0));
        artifact_plates.push(ArtifactPlate {
            source_plate_ids: canonical_source_plate_ids,
            target_plate_id: prepared_plate.target_plate_id,
            name: format!(
                "Packed Full Spectrum plate {:02} — {}",
                prepared_plate.target_plate_id, prepared_plate.plan_plate_id
            ),
            units: plate_units,
            source_identify_ids,
            wipe_tower_x,
            wipe_tower_y,
        });
    }

    let physical_slots = artifact.loadout.clone().map(|slot| PhysicalProfile {
        name: slot.profile,
        setting_id: slot.setting_id,
        filament_id: slot.filament_id,
        color: rgb_hex(slot.color),
        material: material_name(&slot.material).to_owned(),
        resolved: BTreeMap::new(),
    });
    let prepared_loadout = artifact
        .loadout
        .iter()
        .map(|slot| PreparedPhysicalSlot {
            toolhead: toolhead_name(slot.toolhead).to_owned(),
            spool_id: Some(slot.spool_id.clone()),
            spool_name: Some(slot.spool_name.clone()),
            material: Some(material_name(&slot.material).to_owned()),
            color: Some(rgb_hex(slot.color)),
            profile: slot.profile.clone(),
            setting_id: slot.setting_id.clone(),
            filament_id: slot.filament_id.clone(),
        })
        .collect::<Vec<_>>();
    Ok(ArtifactBuildPlan {
        prepared: U1DirectPreparedArtifact {
            batch_id: artifact.batch_id.clone(),
            file_name: artifact.file_name.clone(),
            target_plate_ids: artifact
                .plates
                .iter()
                .map(|plate| plate.plan_plate_id.clone())
                .collect(),
            source_plate_ids: all_source_plate_ids.into_iter().collect(),
            source_unit_ids: all_source_unit_ids.into_iter().collect(),
            loadout: prepared_loadout,
            setup_actions: Vec::new(),
        },
        plates: artifact_plates,
        object_slot_maps,
        resource_slot_maps,
        selected_root_resource_ids,
        external_paths,
        selected_instances,
        physical_slots,
        project_settings,
    })
}

#[allow(clippy::too_many_arguments)]
fn register_full_spectrum_unit_resources(
    unit: &PrintableUnit,
    slot_map: &BTreeMap<u8, u8>,
    analysis_objects: &BTreeMap<u32, &u1_three_mf::ObjectAnalysis>,
    object_slot_maps: &mut BTreeMap<u32, BTreeMap<u8, u8>>,
    resource_slot_maps: &mut BTreeMap<(String, u32), BTreeMap<u8, u8>>,
    selected_root_resource_ids: &mut BTreeSet<u32>,
    external_paths: &mut BTreeSet<String>,
) -> Result<(), U1DirectError> {
    if slot_map.is_empty()
        || slot_map
            .iter()
            .any(|(source, target)| *source == 0 || *target == 0 || *target > 32)
    {
        return Err(U1DirectError::Plan(format!(
            "source unit {} has an invalid Full Spectrum filament map",
            unit.source_unit_id
        )));
    }
    let source_object = analysis_objects
        .get(&unit.source_object_id)
        .copied()
        .ok_or_else(|| {
            U1DirectError::Plan(format!(
                "source object {} for unit {} is unavailable",
                unit.source_object_id, unit.source_unit_id
            ))
        })?;
    let expected_slots = source_object
        .effective_slots
        .iter()
        .copied()
        .chain(
            source_object
                .parts
                .iter()
                .flat_map(|part| part.effective_slots.iter().copied()),
        )
        .filter(|slot| *slot != 0)
        .collect::<BTreeSet<_>>();
    for source_slot in expected_slots {
        let source_slot = u8::try_from(source_slot).map_err(|_| {
            U1DirectError::Plan(format!(
                "source unit {} uses unsupported filament F{source_slot}",
                unit.source_unit_id
            ))
        })?;
        if !slot_map.contains_key(&source_slot) {
            return Err(U1DirectError::Plan(format!(
                "source unit {} uses F{source_slot}, but its Full Spectrum map does not cover it",
                unit.source_unit_id
            )));
        }
    }
    merge_slot_map(
        object_slot_maps,
        unit.source_object_id,
        slot_map,
        "source object",
    )?;
    selected_root_resource_ids.insert(unit.source_object_id);
    for part in &source_object.parts {
        let path = normalize_model_resource_path(
            part.component_path.as_deref().unwrap_or(MAIN_MODEL_PATH),
        )?;
        merge_resource_slot_map(resource_slot_maps, (path.clone(), part.id), slot_map)?;
        if path == MAIN_MODEL_PATH {
            selected_root_resource_ids.insert(part.id);
        } else {
            external_paths.insert(path);
        }
    }
    Ok(())
}

fn normalize_model_resource_path(path: &str) -> Result<String, U1DirectError> {
    let normalized = path.trim().trim_start_matches('/');
    if !normalized.starts_with("3D/")
        || !normalized.ends_with(".model")
        || normalized.contains(['\\', '\0'])
        || normalized
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(U1DirectError::Plan(format!(
            "unsafe Production model resource path {path:?}"
        )));
    }
    Ok(normalized.to_owned())
}

fn validate_full_spectrum_footprints(
    plate_id: &str,
    footprints: &[(String, [f64; 4])],
    tower: Option<(f64, f64)>,
) -> Result<(), U1DirectError> {
    for (index, (left_id, left)) in footprints.iter().enumerate() {
        for (right_id, right) in &footprints[index + 1..] {
            if rectangles_overlap(*left, *right) {
                return Err(U1DirectError::Plan(format!(
                    "Full Spectrum units {left_id} and {right_id} overlap on target plate {plate_id}"
                )));
            }
        }
    }
    if let Some((x, y)) = tower {
        if !x.is_finite() || !y.is_finite() {
            return Err(U1DirectError::Plan(format!(
                "Full Spectrum target plate {plate_id} has a non-finite prime-tower anchor"
            )));
        }
        let center_x = x + PRIME_TOWER_WIDTH_MM / 2.0;
        let center_y = y + PRIME_TOWER_DEPTH_MM / 2.0;
        let envelope = [
            center_x - FULL_SPECTRUM_PRIME_TOWER_MAX_HALF_EXTENT_MM,
            center_y - FULL_SPECTRUM_PRIME_TOWER_MAX_HALF_EXTENT_MM,
            center_x + FULL_SPECTRUM_PRIME_TOWER_MAX_HALF_EXTENT_MM,
            center_y + FULL_SPECTRUM_PRIME_TOWER_MAX_HALF_EXTENT_MM,
        ];
        if envelope[0] < TARGET_MIN_X_MM - TARGET_BOUNDS_TOLERANCE_MM
            || envelope[1] < TARGET_MIN_Y_MM - TARGET_BOUNDS_TOLERANCE_MM
            || envelope[2] > TARGET_MAX_X_MM + TARGET_BOUNDS_TOLERANCE_MM
            || envelope[3] > TARGET_MAX_Y_MM + TARGET_BOUNDS_TOLERANCE_MM
            || footprints
                .iter()
                .any(|(_, footprint)| rectangles_overlap(envelope, *footprint))
        {
            return Err(U1DirectError::Plan(format!(
                "Full Spectrum target plate {plate_id} does not preserve the qualified prime-tower envelope"
            )));
        }
    }
    Ok(())
}

fn validate_preserved_plate_bounds(
    plate: &u1_three_mf::PlateAnalysis,
    source_index: usize,
    source_plate_count: usize,
    source_bed_size: f64,
) -> Result<(), U1DirectError> {
    let bounds = plate.printable_bounds.ok_or_else(|| {
        U1DirectError::Plan(format!(
            "source plate {} has unknown printable bounds; a production U1 layout cannot be proven",
            plate.id
        ))
    })?;
    let origin = virtual_plate_origin(source_index, source_plate_count, source_bed_size);
    let local_min_x = bounds.min[0] - origin.0;
    let local_max_x = bounds.max[0] - origin.0;
    let local_min_y = bounds.min[1] - origin.1;
    let local_max_y = bounds.max[1] - origin.1;
    if ![
        local_min_x,
        local_max_x,
        local_min_y,
        local_max_y,
        bounds.min[2],
        bounds.max[2],
    ]
    .into_iter()
    .all(f64::is_finite)
        || local_min_x < TARGET_MIN_X_MM - TARGET_BOUNDS_TOLERANCE_MM
        || local_min_y < TARGET_MIN_Y_MM - TARGET_BOUNDS_TOLERANCE_MM
        // Snapmaker Orca 2.3.5 deliberately supports "sinking" objects: its
        // build-volume test clips geometry at Z=0 and ignores the part below
        // the bed. Preserve that source intent exactly, but reject geometry
        // that is completely below the printable plane.
        || bounds.max[2] <= 0.0
        || local_max_x > TARGET_MAX_X_MM + TARGET_BOUNDS_TOLERANCE_MM
        || local_max_y > TARGET_MAX_Y_MM + TARGET_BOUNDS_TOLERANCE_MM
        || bounds.max[2] > TARGET_PRINTABLE_HEIGHT_MM + TARGET_BOUNDS_TOLERANCE_MM
    {
        return Err(U1DirectError::Plan(format!(
            "source plate {} does not fit the preserved Snapmaker U1 build volume (local bounds {:.2}..{:.2} × {:.2}..{:.2} × Z {:.2}..{:.2} mm)",
            plate.id,
            local_min_x,
            local_max_x,
            local_min_y,
            local_max_y,
            bounds.min[2],
            bounds.max[2]
        )));
    }
    Ok(())
}

fn choose_wipe_tower_position(
    plate: &u1_three_mf::PlateAnalysis,
    source_origin: (f64, f64),
    used_slot_count: usize,
) -> Result<(f64, f64), U1DirectError> {
    if used_slot_count <= 1 {
        return Ok((40.0, 200.0));
    }
    let tower_height = plate
        .printable_bounds
        .ok_or_else(|| {
            U1DirectError::Plan(format!(
                "source plate {} has unknown printable bounds; prime-tower height cannot be proven",
                plate.id
            ))
        })?
        .max[2];
    if !tower_height.is_finite() || tower_height <= 0.0 || tower_height > TARGET_PRINTABLE_HEIGHT_MM
    {
        return Err(U1DirectError::Plan(format!(
            "source plate {} has an invalid prime-tower height bound",
            plate.id
        )));
    }
    // Snapmaker Orca 2.3.5 interprets wipe_tower_x/y as the lower-left
    // unrotated tower-body corner (Print.cpp::first_layer_wipe_tower_corners).
    // The exact U1 Standard profile is unrotated. Reserve a deliberately
    // conservative axis-aligned envelope around that anchor: the profile's
    // worst-case 45 mm depth, full printed-height stabilization cone, brim,
    // and complete configured rib extension. This exceeds the generated
    // first-layer footprint rather than depending on the GUI's lighter
    // preview box.
    let tower_envelope = |position: (f64, f64)| {
        conservative_prime_tower_envelope(position.0, position.1, tower_height)
    };
    let [
        relative_min_x,
        relative_min_y,
        relative_max_x,
        relative_max_y,
    ] = tower_envelope((0.0, 0.0));
    let center_x = (relative_min_x + relative_max_x) / 2.0;
    let center_y = (relative_min_y + relative_max_y) / 2.0;
    let half_width = (relative_max_x - relative_min_x) / 2.0;
    let half_depth = (relative_max_y - relative_min_y) / 2.0;
    let minimum_x = TARGET_MIN_X_MM + half_width - center_x;
    let maximum_x = TARGET_MAX_X_MM - half_width - center_x;
    let minimum_y = TARGET_MIN_Y_MM + half_depth - center_y;
    let maximum_y = TARGET_MAX_Y_MM - half_depth - center_y;
    let candidates = [
        (40.0, 200.0),
        (minimum_x, minimum_y),
        (maximum_x, minimum_y),
        (minimum_x, maximum_y),
        (maximum_x, maximum_y),
    ];
    for candidate in candidates {
        let tower = tower_envelope(candidate);
        if tower[0] < TARGET_MIN_X_MM
            || tower[1] < TARGET_MIN_Y_MM
            || tower[2] > TARGET_MAX_X_MM
            || tower[3] > TARGET_MAX_Y_MM
        {
            continue;
        }
        let collides = plate
            .instances
            .iter()
            .filter(|instance| instance.printable)
            .try_fold(false, |collision, instance| {
                let bounds = instance.printable_bounds.ok_or_else(|| {
                    U1DirectError::Plan(format!(
                        "source plate {} has an instance with unknown bounds; prime-tower clearance cannot be proven",
                        plate.id
                    ))
                })?;
                let object = [
                    bounds.min[0] - source_origin.0 - OBJECT_FOOTPRINT_CLEARANCE_MM,
                    bounds.min[1] - source_origin.1 - OBJECT_FOOTPRINT_CLEARANCE_MM,
                    bounds.max[0] - source_origin.0 + OBJECT_FOOTPRINT_CLEARANCE_MM,
                    bounds.max[1] - source_origin.1 + OBJECT_FOOTPRINT_CLEARANCE_MM,
                ];
                Ok::<_, U1DirectError>(collision || rectangles_overlap(tower, object))
            })?;
        if !collides {
            return Ok(candidate);
        }
    }
    Err(U1DirectError::Plan(format!(
        "source plate {} has no qualified corner for the conservative U1 prime-tower envelope, including its printed-height cone, rib extension, brim, and object clearance; rearrange the source plate in Snapmaker Orca",
        plate.id
    )))
}

fn conservative_prime_tower_envelope(x: f64, y: f64, tower_height: f64) -> [f64; 4] {
    let cone_radius = (PRIME_TOWER_CONE_ANGLE_DEGREES.to_radians() / 2.0).tan() * tower_height;
    let half_width = (PRIME_TOWER_WIDTH_MM / 2.0).max(cone_radius)
        + PRIME_TOWER_BRIM_MM
        + PRIME_TOWER_EXTRA_RIB_LENGTH_MM
        + PRIME_TOWER_CLEARANCE_MM;
    let half_depth = (PRIME_TOWER_DEPTH_MM / 2.0).max(cone_radius)
        + PRIME_TOWER_BRIM_MM
        + PRIME_TOWER_EXTRA_RIB_LENGTH_MM
        + PRIME_TOWER_CLEARANCE_MM;
    let center_x = x + PRIME_TOWER_WIDTH_MM / 2.0;
    let center_y = y + PRIME_TOWER_DEPTH_MM / 2.0;
    [
        center_x - half_width,
        center_y - half_depth,
        center_x + half_width,
        center_y + half_depth,
    ]
}

fn rectangles_overlap(left: [f64; 4], right: [f64; 4]) -> bool {
    left[0] < right[2] && left[2] > right[0] && left[1] < right[3] && left[3] > right[1]
}

fn virtual_plate_origin(index: usize, plate_count: usize, bed_size: f64) -> (f64, f64) {
    // Snapmaker Orca 2.3.5 PartPlateList uses ceil(sqrt(plate_count)) columns
    // and a 20% logical gap in both axes. The exact adapter/version gate and
    // square-bed precondition above bind this clean-room implementation to
    // that layout contract; GUI round-trip qualification remains mandatory.
    let columns = (plate_count as f64).sqrt().ceil().max(1.0) as usize;
    let stride = bed_size * (1.0 + VIRTUAL_PLATE_GAP_RATIO);
    (
        (index % columns) as f64 * stride,
        -((index / columns) as f64) * stride,
    )
}

#[cfg(test)]
fn inspect_verified_preparation_source(
    verified_source_path: &Path,
    source_name_path: &Path,
    expected: &InputIdentity,
) -> Result<VerifiedPreparationSource, U1DirectError> {
    inspect_verified_preparation_source_with_control(
        verified_source_path,
        source_name_path,
        expected,
        None,
    )
}

fn inspect_verified_preparation_source_with_control(
    verified_source_path: &Path,
    source_name_path: &Path,
    expected: &InputIdentity,
    control: Option<&U1DirectConversionControl>,
) -> Result<VerifiedPreparationSource, U1DirectError> {
    let (source_bytes, source_hash) = hash_path_with_control(verified_source_path, control)?;
    if source_bytes != expected.byte_size || source_hash != expected.sha256 {
        return Err(U1DirectError::SourceIdentityChanged);
    }
    optional_checkpoint(control)?;
    let base_name = safe_file_component(
        source_name_path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("project"),
    );
    let bed_size = read_source_bed_size(verified_source_path)?;
    optional_checkpoint(control)?;
    ensure_source_model_uses_millimeters(verified_source_path)?;
    optional_checkpoint(control)?;
    Ok(VerifiedPreparationSource {
        base_name,
        bed_size,
    })
}

fn read_source_bed_size(source_path: &Path) -> Result<f64, U1DirectError> {
    let file = File::open(source_path).map_err(|source| U1DirectError::Read {
        path: source_path.to_path_buf(),
        source,
    })?;
    let mut archive = ZipArchive::new(file)?;
    let mut entry = archive.by_name(PROJECT_SETTINGS_PATH)?;
    if entry.size() > MAX_METADATA_BYTES {
        return Err(U1DirectError::Plan(
            "source project settings exceed the safe metadata limit".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry
        .read_to_end(&mut bytes)
        .map_err(|source| U1DirectError::Read {
            path: source_path.to_path_buf(),
            source,
        })?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|source| U1DirectError::Json {
        path: PROJECT_SETTINGS_PATH.into(),
        source,
    })?;
    let points = value
        .get("printable_area")
        .and_then(Value::as_array)
        .ok_or_else(|| U1DirectError::Plan("source project has no printable_area".into()))?;
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for point in points {
        let Some(point) = point.as_str() else {
            continue;
        };
        let Some((x, y)) = point.split_once('x') else {
            continue;
        };
        let (Ok(x), Ok(y)) = (x.parse::<f64>(), y.parse::<f64>()) else {
            continue;
        };
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    let width = max_x - min_x;
    let depth = max_y - min_y;
    if !width.is_finite()
        || !depth.is_finite()
        || !(100.0..=500.0).contains(&width)
        || !(100.0..=500.0).contains(&depth)
        || (width - depth).abs() > TARGET_BOUNDS_TOLERANCE_MM
    {
        return Err(U1DirectError::Plan(
            "source printable_area must define a supported finite square bed before its virtual plate grid can be preserved".into(),
        ));
    }
    Ok(width)
}

fn ensure_source_model_uses_millimeters(source_path: &Path) -> Result<(), U1DirectError> {
    const ROOT_PREFIX_LIMIT: u64 = 1024 * 1024;
    let file = File::open(source_path).map_err(|source| U1DirectError::Read {
        path: source_path.to_path_buf(),
        source,
    })?;
    let mut archive = ZipArchive::new(file)?;
    let entry = archive.by_name(MAIN_MODEL_PATH)?;
    let mut reader = Reader::from_reader(BufReader::new(entry.take(ROOT_PREFIX_LIMIT)));
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| U1DirectError::Xml {
                path: MAIN_MODEL_PATH.into(),
                message: error.to_string(),
            })? {
            Event::Start(event) | Event::Empty(event)
                if local_xml_name(event.name().as_ref()) == b"model" =>
            {
                let unit = decoded_attributes(&reader, &event, MAIN_MODEL_PATH)?
                    .into_iter()
                    .find(|(key, _)| local_xml_name(key.as_bytes()) == b"unit")
                    .map(|(_, value)| value)
                    .unwrap_or_else(|| "millimeter".into());
                if unit != "millimeter" {
                    return Err(U1DirectError::Plan(format!(
                        "source root model uses unit {unit:?}; Stage B currently accepts only millimeter 3MF transforms"
                    )));
                }
                return Ok(());
            }
            Event::DocType(_) => {
                return Err(U1DirectError::Xml {
                    path: MAIN_MODEL_PATH.into(),
                    message: "DOCTYPE is forbidden".into(),
                });
            }
            Event::Eof => {
                return Err(U1DirectError::Xml {
                    path: MAIN_MODEL_PATH.into(),
                    message: "root model element was not found within the bounded prefix".into(),
                });
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn load_verified_profile_store(
    context: &AdapterContext,
) -> Result<VerifiedProfileStore, U1DirectError> {
    let manifest_before =
        fs::read(&context.profile_manifest_path).map_err(|source| U1DirectError::Read {
            path: context.profile_manifest_path.clone(),
            source,
        })?;
    verify_profile_manifest_bytes(&manifest_before)?;

    for (relative_path, expected_hash) in DIRECT_AUXILIARY_PROFILE_BASELINE {
        read_verified_profile_file(&context.profiles_root, relative_path, expected_hash)?;
    }

    let mut documents = BTreeMap::new();
    for (relative_path, expected_hash) in DIRECT_PROFILE_BASELINE {
        let path = context.profiles_root.join(relative_path);
        let bytes =
            read_verified_profile_file(&context.profiles_root, relative_path, expected_hash)?;
        let document =
            serde_json::from_slice::<BTreeMap<String, Value>>(&bytes).map_err(|source| {
                U1DirectError::Json {
                    path: path.to_string_lossy().into_owned(),
                    source,
                }
            })?;
        documents.insert((*relative_path).to_owned(), document);
    }

    let manifest_after =
        fs::read(&context.profile_manifest_path).map_err(|source| U1DirectError::Read {
            path: context.profile_manifest_path.clone(),
            source,
        })?;
    verify_profile_manifest_bytes(&manifest_after)?;
    if manifest_after != manifest_before {
        return Err(U1DirectError::Capability(
            "the Snapmaker profile manifest changed while the qualified profiles were being loaded"
                .into(),
        ));
    }
    Ok(VerifiedProfileStore { documents })
}

fn verify_profile_manifest_bytes(bytes: &[u8]) -> Result<(), U1DirectError> {
    let actual_hash = format!("{:x}", Sha256::digest(bytes));
    if actual_hash != U1_DIRECT_PROFILE_MANIFEST_SHA256 {
        return Err(U1DirectError::Capability(format!(
            "the Snapmaker profile manifest changed after capability inspection (found {actual_hash}, expected {U1_DIRECT_PROFILE_MANIFEST_SHA256})"
        )));
    }
    let manifest = serde_json::from_slice::<SnapmakerVendorManifest>(bytes).map_err(|source| {
        U1DirectError::Json {
            path: "effective Snapmaker.json".into(),
            source,
        }
    })?;
    if manifest.name != SNAPMAKER_VENDOR_NAME || manifest.version != U1_DIRECT_PROFILE_PACK_VERSION
    {
        return Err(U1DirectError::Capability(format!(
            "the Snapmaker profile manifest changed identity after capability inspection (vendor {:?}, version {:?})",
            manifest.name, manifest.version
        )));
    }
    Ok(())
}

fn read_verified_profile_file(
    profiles_root: &Path,
    relative_path: &str,
    expected_hash: &str,
) -> Result<Vec<u8>, U1DirectError> {
    let path = profiles_root.join(relative_path);
    let bytes = fs::read(&path).map_err(|source| U1DirectError::Read { path, source })?;
    let actual_hash = format!("{:x}", Sha256::digest(&bytes));
    if actual_hash != expected_hash {
        return Err(U1DirectError::Capability(format!(
            "required U1 profile {relative_path} changed after capability inspection (found {actual_hash}, expected {expected_hash})"
        )));
    }
    Ok(bytes)
}

fn resolve_verified_profile_chain(
    store: &VerifiedProfileStore,
    relative_path: &str,
) -> Result<BTreeMap<String, Value>, U1DirectError> {
    let category = Path::new(relative_path).parent().ok_or_else(|| {
        U1DirectError::Capability(format!("profile path {relative_path} has no category"))
    })?;
    let mut chain = Vec::<BTreeMap<String, Value>>::new();
    let mut current = relative_path.to_owned();
    let mut seen = BTreeSet::new();
    loop {
        let document = store.documents.get(&current).ok_or_else(|| {
            U1DirectError::Capability(format!(
                "profile {current} is absent from the verified Snapmaker baseline"
            ))
        })?;
        let name = document
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&current)
            .to_owned();
        if !seen.insert(name.clone()) {
            return Err(U1DirectError::Capability(format!(
                "profile inheritance cycle at {name}"
            )));
        }
        let inherits = document
            .get("inherits")
            .and_then(Value::as_str)
            .map(str::to_owned);
        chain.push(document.clone());
        let Some(parent_name) = inherits else { break };
        let matches = store
            .documents
            .iter()
            .filter(|(candidate_path, candidate)| {
                Path::new(candidate_path.as_str()).parent() == Some(category)
                    && candidate.get("name").and_then(Value::as_str) == Some(parent_name.as_str())
            })
            .map(|(candidate_path, _)| candidate_path.clone())
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(U1DirectError::Capability(format!(
                "verified profile {name} inherits {parent_name:?}, which resolves to {} baseline files inside the Snapmaker {:?} namespace",
                matches.len(),
                category.file_name().unwrap_or_default()
            )));
        }
        current = matches[0].clone();
    }
    let mut resolved = BTreeMap::new();
    for profile in chain.into_iter().rev() {
        resolved.extend(profile);
    }
    Ok(resolved)
}

fn resolve_physical_slots_verified(
    profiles: &VerifiedProfileStore,
    input: &PlanningInput,
    loadout: &u1_planner::U1Loadout,
) -> Result<[PhysicalProfile; 4], U1DirectError> {
    let inventory = input
        .inventory
        .iter()
        .map(|spool| (spool.id.as_str(), spool))
        .collect::<BTreeMap<_, _>>();
    let mut slots = Vec::with_capacity(4);
    for spool_id in &loadout.slots {
        let spool = spool_id
            .as_deref()
            .map(|id| {
                inventory.get(id).copied().ok_or_else(|| {
                    U1DirectError::Plan(format!("loadout references missing spool {id}"))
                })
            })
            .transpose()?;
        slots.push(resolve_physical_profile_with(spool, |relative_path| {
            resolve_verified_profile_chain(profiles, relative_path)
        })?);
    }
    slots.try_into().map_err(|_| {
        U1DirectError::Plan("U1 Direct loadout must contain exactly four physical slots".into())
    })
}

#[cfg(test)]
fn resolve_physical_profile(
    context: &AdapterContext,
    spool: Option<&Spool>,
) -> Result<PhysicalProfile, U1DirectError> {
    resolve_physical_profile_with(spool, |relative_path| {
        resolve_profile_chain(&context.profiles_root, relative_path)
    })
}

fn resolve_physical_profile_with(
    spool: Option<&Spool>,
    resolve: impl FnOnce(&str) -> Result<BTreeMap<String, Value>, U1DirectError>,
) -> Result<PhysicalProfile, U1DirectError> {
    let (relative_path, expected_name) = match spool {
        None => (GENERIC_PLA_PATH, "Generic PLA"),
        Some(spool) => {
            if !spool.available {
                return Err(U1DirectError::Plan(format!(
                    "spool {} is out of stock",
                    spool.display_name
                )));
            }
            let requested = spool
                .profile_id
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty());
            match (&spool.material, requested) {
                (Material::Pla, Some("Generic PLA")) => (GENERIC_PLA_PATH, "Generic PLA"),
                (Material::Pla, Some("Polymaker General PLA Family @U1")) => {
                    (POLYMAKER_PLA_PATH, "Polymaker General PLA Family @U1")
                }
                (Material::Petg, Some("Generic PETG")) => (GENERIC_PETG_PATH, "Generic PETG"),
                (_, Some(profile)) => {
                    return Err(U1DirectError::Plan(format!(
                        "physical spool {} requests unqualified U1 profile {profile:?}; choose Generic PLA, Polymaker General PLA Family @U1, or Generic PETG",
                        spool.display_name
                    )));
                }
                (Material::Pla, None)
                    if spool
                        .display_name
                        .to_ascii_lowercase()
                        .contains("polymaker") =>
                {
                    (POLYMAKER_PLA_PATH, "Polymaker General PLA Family @U1")
                }
                (Material::Pla, None) => (GENERIC_PLA_PATH, "Generic PLA"),
                (Material::Petg, None) => (GENERIC_PETG_PATH, "Generic PETG"),
                (material, None) => {
                    return Err(U1DirectError::Plan(format!(
                        "material {material:?} has no qualified Snapmaker U1 Direct profile"
                    )));
                }
            }
        }
    };
    let resolved = resolve(relative_path)?;
    let name = resolved
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(expected_name)
        .to_owned();
    if name != expected_name {
        return Err(U1DirectError::Capability(format!(
            "profile {relative_path} resolved to unexpected name {name:?}"
        )));
    }
    let setting_id = resolved
        .get("setting_id")
        .and_then(Value::as_str)
        .ok_or_else(|| U1DirectError::Capability(format!("profile {name} has no setting_id")))?
        .to_owned();
    let filament_id = resolved
        .get("filament_id")
        .and_then(Value::as_str)
        .ok_or_else(|| U1DirectError::Capability(format!("profile {name} has no filament_id")))?
        .to_owned();
    Ok(PhysicalProfile {
        name,
        setting_id,
        filament_id,
        color: spool
            .map(|spool| rgb_hex(spool.actual_color()))
            .unwrap_or_else(|| "#FFFFFF".into()),
        material: spool
            .map(|spool| material_name(&spool.material).to_owned())
            .unwrap_or_else(|| "PLA".into()),
        resolved,
    })
}

#[cfg(test)]
fn resolve_profile_chain(
    profiles_root: &Path,
    relative_path: &str,
) -> Result<BTreeMap<String, Value>, U1DirectError> {
    let path = profiles_root.join(relative_path);
    let category = path
        .parent()
        .ok_or_else(|| {
            U1DirectError::Capability(format!("profile path {relative_path} has no category"))
        })?
        .to_path_buf();
    let mut chain = Vec::<BTreeMap<String, Value>>::new();
    let mut current = path;
    let mut seen = BTreeSet::new();
    loop {
        let bytes = fs::read(&current).map_err(|source| U1DirectError::Read {
            path: current.clone(),
            source,
        })?;
        let map = serde_json::from_slice::<BTreeMap<String, Value>>(&bytes).map_err(|source| {
            U1DirectError::Json {
                path: current.to_string_lossy().into_owned(),
                source,
            }
        })?;
        let name = map
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| current.to_string_lossy().into_owned());
        if !seen.insert(name.clone()) {
            return Err(U1DirectError::Capability(format!(
                "profile inheritance cycle at {name}"
            )));
        }
        let inherits = map
            .get("inherits")
            .and_then(Value::as_str)
            .map(str::to_owned);
        chain.push(map);
        let Some(parent_name) = inherits else { break };
        let mut matches = Vec::new();
        for candidate in fs::read_dir(&category).map_err(|source| U1DirectError::Read {
            path: category.clone(),
            source,
        })? {
            let candidate = candidate.map_err(|source| U1DirectError::Read {
                path: category.clone(),
                source,
            })?;
            if candidate
                .path()
                .extension()
                .and_then(|value| value.to_str())
                != Some("json")
            {
                continue;
            }
            let Ok(bytes) = fs::read(candidate.path()) else {
                continue;
            };
            let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            if value.get("name").and_then(Value::as_str) == Some(parent_name.as_str()) {
                matches.push(candidate.path());
            }
        }
        if matches.len() != 1 {
            return Err(U1DirectError::Capability(format!(
                "profile {name} inherits {parent_name:?}, which resolves to {} files inside the Snapmaker {:?} namespace",
                matches.len(),
                category.file_name().unwrap_or_default()
            )));
        }
        current = matches.remove(0);
    }
    let mut resolved = BTreeMap::new();
    for profile in chain.into_iter().rev() {
        resolved.extend(profile);
    }
    Ok(resolved)
}

#[cfg(test)]
fn build_project_settings(
    context: &AdapterContext,
    slots: &[PhysicalProfile; 4],
    plates: &[ArtifactPlate],
    source_process: &ProcessInformation,
) -> Result<Vec<u8>, U1DirectError> {
    let machine = resolve_profile_chain(&context.profiles_root, MACHINE_PATH)?;
    let process = resolve_profile_chain(&context.profiles_root, DIRECT_PROCESS_PATH)?;
    build_project_settings_from_profiles(machine, process, slots, plates, source_process)
}

fn build_project_settings_verified(
    profiles: &VerifiedProfileStore,
    slots: &[PhysicalProfile; 4],
    plates: &[ArtifactPlate],
    source_process: &ProcessInformation,
) -> Result<Vec<u8>, U1DirectError> {
    let machine = resolve_verified_profile_chain(profiles, MACHINE_PATH)?;
    let process = resolve_verified_profile_chain(profiles, DIRECT_PROCESS_PATH)?;
    build_project_settings_from_profiles(machine, process, slots, plates, source_process)
}

fn build_project_settings_from_profiles(
    mut settings: BTreeMap<String, Value>,
    process: BTreeMap<String, Value>,
    slots: &[PhysicalProfile; 4],
    plates: &[ArtifactPlate],
    source_process: &ProcessInformation,
) -> Result<Vec<u8>, U1DirectError> {
    settings.extend(process);
    apply_target_project_defaults(&mut settings);
    let mut override_keys = BTreeSet::new();
    apply_source_quality_intent(&mut settings, source_process, &mut override_keys)?;
    apply_source_support_intent(&mut settings, &source_process.support, &mut override_keys);
    declare_process_overrides(&mut settings, &override_keys, slots.len());
    validate_source_quality_intent(&settings, source_process)?;
    validate_source_support_intent(&settings, &source_process.support)?;
    validate_process_override_groups(&settings, &override_keys, slots.len() + 2)?;
    validate_prime_tower_profile_contract(&settings)?;
    validate_target_project_contract(&settings)?;
    let identity_keys = [
        "type",
        "name",
        "setting_id",
        "inherits",
        "compatible_printers",
        "compatible_printers_condition",
        "instantiation",
        "description",
    ];
    for key in identity_keys {
        settings.remove(key);
    }
    let filament_keys = slots
        .iter()
        .flat_map(|slot| slot.resolved.iter())
        .filter_map(|(key, value)| value.is_array().then_some(key.clone()))
        .collect::<BTreeSet<_>>();
    for key in filament_keys {
        if matches!(
            key.as_str(),
            "compatible_printers" | "compatible_prints" | "default_filament_profile"
        ) {
            continue;
        }
        let values = slots
            .iter()
            .map(|slot| {
                slot.resolved
                    .get(&key)
                    .and_then(Value::as_array)
                    .and_then(|values| values.first())
                    .cloned()
                    .unwrap_or(Value::String(String::new()))
            })
            .collect::<Vec<_>>();
        settings.insert(key, Value::Array(values));
    }
    settings.insert("printer_model".into(), Value::String("Snapmaker U1".into()));
    settings.insert("printer_variant".into(), Value::String("0.4".into()));
    settings.insert(
        "printer_settings_id".into(),
        Value::String(U1_DIRECT_MACHINE_PROFILE.into()),
    );
    settings.insert(
        "print_settings_id".into(),
        Value::String(U1_DIRECT_PROCESS_PROFILE.into()),
    );
    settings.insert(
        "nozzle_diameter".into(),
        Value::Array((0..4).map(|_| Value::String("0.4".into())).collect()),
    );
    settings.insert(
        "filament_settings_id".into(),
        Value::Array(
            slots
                .iter()
                .map(|slot| Value::String(slot.name.clone()))
                .collect(),
        ),
    );
    settings.insert(
        "filament_ids".into(),
        Value::Array(
            slots
                .iter()
                // Snapmaker Orca serializes the leaf filament preset's
                // `setting_id` in project `filament_ids`. The inherited
                // material-family `filament_id` remains useful inventory
                // metadata, but writing it here makes a normal GUI Save As
                // silently canonicalize the project identity.
                .map(|slot| Value::String(slot.setting_id.clone()))
                .collect(),
        ),
    );
    settings.insert(
        "filament_colour".into(),
        Value::Array(
            slots
                .iter()
                .map(|slot| Value::String(slot.color.clone()))
                .collect(),
        ),
    );
    settings.insert(
        "filament_type".into(),
        Value::Array(
            slots
                .iter()
                .map(|slot| Value::String(slot.material.clone()))
                .collect(),
        ),
    );
    let flush_matrix = build_orca_flush_matrix(&settings, slots)?
        .into_iter()
        .map(|volume| Value::String(volume.to_string()))
        .collect();
    settings.insert("flush_volumes_matrix".into(), Value::Array(flush_matrix));
    settings.insert(
        "flush_volumes_vector".into(),
        Value::Array((0..8).map(|_| Value::String("140".into())).collect()),
    );
    settings.insert("flush_multiplier".into(), Value::String("1".into()));
    settings.insert(
        "wipe_tower_x".into(),
        Value::Array(
            plates
                .iter()
                .map(|plate| Value::String(format_f64(plate.wipe_tower_x)))
                .collect(),
        ),
    );
    settings.insert(
        "wipe_tower_y".into(),
        Value::Array(
            plates
                .iter()
                .map(|plate| Value::String(format_f64(plate.wipe_tower_y)))
                .collect(),
        ),
    );
    settings.insert("from".into(), Value::String("project".into()));
    settings.insert(
        "version".into(),
        Value::String(SUPPORTED_ORCA_VERSION.into()),
    );
    let mut bytes = serde_json::to_vec_pretty(&settings).map_err(|source| U1DirectError::Json {
        path: PROJECT_SETTINGS_PATH.into(),
        source,
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn apply_target_project_defaults(settings: &mut BTreeMap<String, Value>) {
    // These are Stage-B target decisions, not inherited Bambu project
    // settings. Making them explicit keeps command-line slicing deterministic
    // and avoids relying on Snapmaker Orca's GUI-only U1 bed normalization.
    settings.insert(
        "curr_bed_type".into(),
        Value::String("Textured PEI Plate".into()),
    );
    // Orca's built-in default is auto_brim, but the Snapmaker profile chain
    // does not serialize it. Pin it explicitly so the 18 mm cap used by the
    // collision envelope cannot drift with an application default.
    settings.insert("brim_type".into(), Value::String("auto_brim".into()));
    settings.insert("print_sequence".into(), Value::String("by layer".into()));
    settings.insert(
        "first_layer_print_sequence".into(),
        Value::Array(vec![Value::String("0".into())]),
    );
    settings.insert(
        "other_layers_print_sequence".into(),
        Value::Array(vec![Value::String("0".into())]),
    );
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
    settings.insert("timelapse_type".into(), Value::String("0".into()));
    settings.insert(
        "wipe_tower_rotation_angle".into(),
        Value::String("0".into()),
    );
    settings.insert("purge_in_prime_tower".into(), Value::String("0".into()));
    settings.insert(
        "single_extruder_multi_material".into(),
        Value::String("0".into()),
    );
}

fn apply_source_support_intent(
    settings: &mut BTreeMap<String, Value>,
    support: &SupportInformation,
    override_keys: &mut BTreeSet<&'static str>,
) {
    if let Some(enabled) = support.enabled {
        override_keys.insert("enable_support");
        settings.insert(
            "enable_support".into(),
            Value::String(if enabled { "1" } else { "0" }.into()),
        );
    }
    if let Some(support_type) = support.support_type {
        override_keys.insert("support_type");
        settings.insert(
            "support_type".into(),
            Value::String(support_type.slicer_value().into()),
        );
    }
    if let Some(angle) = support.threshold_angle_degrees {
        override_keys.insert("support_threshold_angle");
        settings.insert(
            "support_threshold_angle".into(),
            Value::String(angle.to_string()),
        );
    }
    if let Some(on_build_plate_only) = support.on_build_plate_only {
        override_keys.insert("support_on_build_plate_only");
        settings.insert(
            "support_on_build_plate_only".into(),
            Value::String(if on_build_plate_only { "1" } else { "0" }.into()),
        );
    }
    if support.enabled == Some(true)
        && let Some(layer_height) = config_scalar_f64(settings, "layer_height")
    {
        let distance = format_config_number(layer_height);
        for key in ["support_top_z_distance", "support_bottom_z_distance"] {
            settings.insert(key.into(), Value::String(distance.clone()));
            override_keys.insert(key);
        }
    }
}

fn apply_source_quality_intent(
    settings: &mut BTreeMap<String, Value>,
    process: &ProcessInformation,
    override_keys: &mut BTreeSet<&'static str>,
) -> Result<(), U1DirectError> {
    if let Some(source_layer_height) = process.layer_height_mm {
        if source_layer_height < MIN_QUALIFIED_LAYER_HEIGHT_MM {
            return Err(U1DirectError::Capability(format!(
                "source layer height {source_layer_height:.3} mm is finer than the qualified U1 minimum {MIN_QUALIFIED_LAYER_HEIGHT_MM:.2} mm"
            )));
        }
        let target_layer_height = config_scalar_f64(settings, "layer_height").ok_or_else(|| {
            U1DirectError::Capability("qualified U1 process has no layer_height".into())
        })?;
        let selected = source_layer_height
            .min(target_layer_height)
            .min(MAX_QUALIFIED_LAYER_HEIGHT_MM);
        settings.insert(
            "layer_height".into(),
            Value::String(format_config_number(selected)),
        );
        override_keys.insert("layer_height");
    }
    if let Some(wall_generator) = process.quality.wall_generator {
        settings.insert(
            "wall_generator".into(),
            Value::String(wall_generator.slicer_value().into()),
        );
        override_keys.insert("wall_generator");
    }
    for (key, source_limit) in [
        ("outer_wall_speed", process.quality.outer_wall_speed_mm_s),
        ("inner_wall_speed", process.quality.inner_wall_speed_mm_s),
        ("top_surface_speed", process.quality.top_surface_speed_mm_s),
        (
            "outer_wall_acceleration",
            process.quality.outer_wall_acceleration_mm_s2,
        ),
    ] {
        let Some(source_limit) = source_limit else {
            continue;
        };
        let target_limit = config_scalar_f64(settings, key).ok_or_else(|| {
            U1DirectError::Capability(format!("qualified U1 process has no numeric {key}"))
        })?;
        settings.insert(
            key.into(),
            Value::String(format_config_number(source_limit.min(target_limit))),
        );
        override_keys.insert(key);
    }
    for (key, source_minimum) in [
        ("wall_loops", process.quality.wall_loops),
        ("top_shell_layers", process.quality.top_shell_layers),
        ("bottom_shell_layers", process.quality.bottom_shell_layers),
    ] {
        let Some(source_minimum) = source_minimum else {
            continue;
        };
        let target = config_scalar_f64(settings, key).ok_or_else(|| {
            U1DirectError::Capability(format!("qualified U1 process has no numeric {key}"))
        })?;
        let selected = u16::try_from(target.round() as i64)
            .unwrap_or(source_minimum)
            .max(source_minimum);
        settings.insert(key.into(), Value::String(selected.to_string()));
        override_keys.insert(key);
    }
    Ok(())
}

fn declare_process_overrides(
    settings: &mut BTreeMap<String, Value>,
    override_keys: &BTreeSet<&'static str>,
    physical_filament_count: usize,
) {
    if override_keys.is_empty() {
        return;
    }
    let mut groups = vec![Value::String(String::new()); physical_filament_count + 2];
    groups[0] = Value::String(override_keys.iter().copied().collect::<Vec<_>>().join(";"));
    settings.insert("different_settings_to_system".into(), Value::Array(groups));
}

fn validate_source_support_intent(
    settings: &BTreeMap<String, Value>,
    support: &SupportInformation,
) -> Result<(), U1DirectError> {
    let require = |key: &'static str, expected: String| {
        if settings.get(key).and_then(Value::as_str) == Some(expected.as_str()) {
            Ok(())
        } else {
            Err(U1DirectError::Capability(format!(
                "generated U1 setting {key} does not preserve source support intent"
            )))
        }
    };
    if let Some(enabled) = support.enabled {
        require("enable_support", if enabled { "1" } else { "0" }.into())?;
    }
    if let Some(support_type) = support.support_type {
        require("support_type", support_type.slicer_value().into())?;
    }
    if let Some(angle) = support.threshold_angle_degrees {
        require("support_threshold_angle", angle.to_string())?;
    }
    if let Some(on_build_plate_only) = support.on_build_plate_only {
        require(
            "support_on_build_plate_only",
            if on_build_plate_only { "1" } else { "0" }.into(),
        )?;
    }
    if support.enabled == Some(true) {
        let layer_height = config_scalar_f64(settings, "layer_height");
        if config_scalar_f64(settings, "support_top_z_distance") != layer_height
            || config_scalar_f64(settings, "support_bottom_z_distance") != layer_height
        {
            return Err(U1DirectError::Capability(
                "generated U1 support gaps do not track the selected layer height".into(),
            ));
        }
    }
    Ok(())
}

fn validate_source_quality_intent(
    settings: &BTreeMap<String, Value>,
    process: &ProcessInformation,
) -> Result<(), U1DirectError> {
    if let Some(source_layer_height) = process.layer_height_mm {
        let selected = config_scalar_f64(settings, "layer_height").ok_or_else(|| {
            U1DirectError::Capability("generated U1 process has no layer_height".into())
        })?;
        if selected > source_layer_height
            || !(MIN_QUALIFIED_LAYER_HEIGHT_MM..=MAX_QUALIFIED_LAYER_HEIGHT_MM).contains(&selected)
        {
            return Err(U1DirectError::Capability(
                "generated U1 layer height weakens source quality intent".into(),
            ));
        }
    }
    if let Some(wall_generator) = process.quality.wall_generator
        && config_scalar_string(settings, "wall_generator") != Some(wall_generator.slicer_value())
    {
        return Err(U1DirectError::Capability(
            "generated U1 wall generator does not preserve source quality intent".into(),
        ));
    }
    for (key, source_limit) in [
        ("outer_wall_speed", process.quality.outer_wall_speed_mm_s),
        ("inner_wall_speed", process.quality.inner_wall_speed_mm_s),
        ("top_surface_speed", process.quality.top_surface_speed_mm_s),
        (
            "outer_wall_acceleration",
            process.quality.outer_wall_acceleration_mm_s2,
        ),
    ] {
        if let Some(source_limit) = source_limit
            && config_scalar_f64(settings, key).is_none_or(|value| value > source_limit)
        {
            return Err(U1DirectError::Capability(format!(
                "generated U1 {key} exceeds the source quality ceiling"
            )));
        }
    }
    for (key, source_minimum) in [
        ("wall_loops", process.quality.wall_loops),
        ("top_shell_layers", process.quality.top_shell_layers),
        ("bottom_shell_layers", process.quality.bottom_shell_layers),
    ] {
        if let Some(source_minimum) = source_minimum
            && config_scalar_f64(settings, key)
                .is_none_or(|value| value + f64::EPSILON < f64::from(source_minimum))
        {
            return Err(U1DirectError::Capability(format!(
                "generated U1 {key} weakens the source shell intent"
            )));
        }
    }
    Ok(())
}

fn validate_process_override_groups(
    settings: &BTreeMap<String, Value>,
    expected_keys: &BTreeSet<&'static str>,
    expected_group_count: usize,
) -> Result<(), U1DirectError> {
    if expected_keys.is_empty() {
        return Ok(());
    }
    let groups = settings
        .get("different_settings_to_system")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            U1DirectError::Capability(
                "generated U1 project does not declare its process overrides".into(),
            )
        })?;
    if groups.len() != expected_group_count
        || groups.first().and_then(Value::as_str)
            != Some(
                expected_keys
                    .iter()
                    .copied()
                    .collect::<Vec<_>>()
                    .join(";")
                    .as_str(),
            )
        || groups
            .iter()
            .skip(1)
            .any(|value| value.as_str() != Some(""))
    {
        return Err(U1DirectError::Capability(
            "generated U1 process override declaration is not canonical".into(),
        ));
    }
    Ok(())
}

fn format_config_number(value: f64) -> String {
    let formatted = format!("{value:.6}");
    formatted
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}

fn validate_prime_tower_profile_contract(
    settings: &BTreeMap<String, Value>,
) -> Result<(), U1DirectError> {
    let enabled = settings
        .get("enable_prime_tower")
        .and_then(|value| match value {
            Value::Bool(value) => Some(*value),
            Value::Number(value) => value.as_i64().map(|value| value != 0),
            Value::String(value) if value == "1" || value.eq_ignore_ascii_case("true") => {
                Some(true)
            }
            Value::String(value) if value == "0" || value.eq_ignore_ascii_case("false") => {
                Some(false)
            }
            _ => None,
        })
        .unwrap_or(false);
    let width = config_scalar_f64(settings, "prime_tower_width");
    let brim = config_scalar_f64(settings, "prime_tower_brim_width");
    let brim_type = config_scalar_string(settings, "brim_type");
    let object_brim = config_scalar_f64(settings, "brim_width");
    let object_brim_gap = config_scalar_f64(settings, "brim_object_gap");
    let volume = config_scalar_f64(settings, "prime_volume");
    let cone_angle = config_scalar_f64(settings, "wipe_tower_cone_angle");
    let rib_length = config_scalar_f64(settings, "wipe_tower_extra_rib_length");
    let extra_spacing = config_scalar_percent(settings, "wipe_tower_extra_spacing");
    let layer_height = config_scalar_f64(settings, "layer_height");
    if !enabled
        || width.is_none_or(|value| (value - PRIME_TOWER_WIDTH_MM).abs() > f64::EPSILON)
        || brim.is_none_or(|value| (value - PRIME_TOWER_BRIM_MM).abs() > f64::EPSILON)
        || volume.is_none_or(|value| (value - PRIME_TOWER_VOLUME_MM3).abs() > f64::EPSILON)
        || cone_angle
            .is_none_or(|value| (value - PRIME_TOWER_CONE_ANGLE_DEGREES).abs() > f64::EPSILON)
        || rib_length
            .is_none_or(|value| (value - PRIME_TOWER_EXTRA_RIB_LENGTH_MM).abs() > f64::EPSILON)
        || extra_spacing
            .is_none_or(|value| (value - PRIME_TOWER_EXTRA_SPACING_PERCENT).abs() > f64::EPSILON)
        || layer_height.is_none_or(|value| {
            !(MIN_QUALIFIED_LAYER_HEIGHT_MM..=MAX_QUALIFIED_LAYER_HEIGHT_MM).contains(&value)
        })
        || config_scalar_string(settings, "wipe_tower_wall_type") != Some("rib")
        || config_scalar_f64(settings, "wipe_tower_rotation_angle") != Some(0.0)
        || config_scalar_f64(settings, "timelapse_type") != Some(0.0)
        || config_scalar_bool(settings, "purge_in_prime_tower") != Some(false)
        || config_scalar_bool(settings, "single_extruder_multi_material") != Some(false)
        || brim_type.is_none_or(|value| !QUALIFIED_BRIM_TYPES.contains(&value))
        || object_brim.is_none_or(|value| !(0.0..=MAX_OBJECT_BRIM_WIDTH_MM).contains(&value))
        || object_brim_gap.is_none_or(|value| !(0.0..=MAX_OBJECT_BRIM_GAP_MM).contains(&value))
    {
        return Err(U1DirectError::Capability(format!(
            "qualified U1 process must match the ribbed prime-tower contract ({PRIME_TOWER_WIDTH_MM:.0} mm width, {PRIME_TOWER_VOLUME_MM3:.0} mm³ prime volume, {PRIME_TOWER_BRIM_MM:.0} mm brim, {PRIME_TOWER_CONE_ANGLE_DEGREES:.0}° cone), the {MIN_QUALIFIED_LAYER_HEIGHT_MM:.2}–{MAX_QUALIFIED_LAYER_HEIGHT_MM:.2} mm layer range, and the {OBJECT_FOOTPRINT_CLEARANCE_MM:.0} mm object-clearance contract"
        )));
    }
    Ok(())
}

fn validate_target_project_contract(
    settings: &BTreeMap<String, Value>,
) -> Result<(), U1DirectError> {
    let zero_array = |key| {
        settings
            .get(key)
            .and_then(Value::as_array)
            .is_some_and(|values| {
                values.len() == 1 && values.first().and_then(Value::as_str) == Some("0")
            })
    };
    if config_scalar_string(settings, "curr_bed_type") != Some("Textured PEI Plate")
        || config_scalar_string(settings, "print_sequence") != Some("by layer")
        || !zero_array("first_layer_print_sequence")
        || !zero_array("other_layers_print_sequence")
        || config_scalar_f64(settings, "other_layers_print_sequence_nums") != Some(0.0)
        || config_scalar_bool(settings, "spiral_mode") != Some(false)
        || config_scalar_bool(settings, "spiral_mode_smooth") != Some(false)
        || config_scalar_percent(settings, "spiral_mode_max_xy_smoothing") != Some(200.0)
    {
        return Err(U1DirectError::Capability(
            "generated U1 project does not match the explicit Textured PEI, by-layer, non-spiral Stage-B target defaults"
                .into(),
        ));
    }
    Ok(())
}

fn build_orca_flush_matrix(
    settings: &BTreeMap<String, Value>,
    slots: &[PhysicalProfile; 4],
) -> Result<Vec<i32>, U1DirectError> {
    let minimums = orca_minimum_flush_volumes(settings, slots.len())?;
    let support_flags = (0..slots.len())
        .map(|index| config_array_bool(settings, "filament_is_support", index).unwrap_or(false))
        .collect::<Vec<_>>();
    let mut matrix = Vec::with_capacity(slots.len() * slots.len());
    for (source_index, source) in slots.iter().enumerate() {
        for (target_index, target) in slots.iter().enumerate() {
            let volume = if source_index == target_index {
                0
            } else if support_flags[target_index] {
                230
            } else {
                let calculated = orca_flush_volume(
                    minimums[source_index],
                    parse_profile_color(&source.color)?,
                    parse_profile_color(&target.color)?,
                );
                if support_flags[source_index] {
                    calculated.max(420)
                } else {
                    calculated
                }
            };
            matrix.push(volume);
        }
    }
    Ok(matrix)
}

fn orca_minimum_flush_volumes(
    settings: &BTreeMap<String, Value>,
    filament_count: usize,
) -> Result<Vec<i32>, U1DirectError> {
    let nozzle_volume = config_scalar_f64(settings, "nozzle_volume")
        .unwrap_or(0.0)
        .trunc() as i32;
    let machine_level = config_scalar_f64(settings, "enable_long_retraction_when_cut")
        .unwrap_or(0.0)
        .trunc() as i32;
    let machine_activated =
        config_array_bool(settings, "long_retractions_when_cut", 0).unwrap_or(false);
    let printer_retraction =
        config_array_f64(settings, "retraction_distances_when_cut", 0).unwrap_or(18.0);
    let mut result = Vec::with_capacity(filament_count);
    for index in 0..filament_count {
        let filament_activated =
            config_array_bool(settings, "filament_long_retractions_when_cut", index)
                .unwrap_or(false);
        let filament_retraction =
            config_array_f64(settings, "filament_retraction_distances_when_cut", index);
        let retract_length = if !machine_activated || !filament_activated {
            0.0
        } else if machine_level == 2 {
            filament_retraction.unwrap_or(printer_retraction)
        } else {
            printer_retraction
        };
        let displaced = std::f64::consts::PI * 1.75_f64 * 1.75_f64 / 4.0 * retract_length;
        let minimum = f64::from(nozzle_volume) - displaced;
        if !minimum.is_finite() || minimum < f64::from(i32::MIN) || minimum > f64::from(i32::MAX) {
            return Err(U1DirectError::Capability(
                "qualified U1 profiles produce an invalid minimum flush volume".into(),
            ));
        }
        result.push(minimum.trunc() as i32);
    }
    Ok(result)
}

fn config_scalar_f64(settings: &BTreeMap<String, Value>, key: &str) -> Option<f64> {
    settings.get(key).and_then(value_as_f64)
}

fn config_scalar_percent(settings: &BTreeMap<String, Value>, key: &str) -> Option<f64> {
    settings.get(key).and_then(|value| match value {
        Value::String(value) => value.strip_suffix('%').unwrap_or(value).parse::<f64>().ok(),
        other => value_as_f64(other),
    })
}

fn config_scalar_string<'a>(settings: &'a BTreeMap<String, Value>, key: &str) -> Option<&'a str> {
    settings.get(key).and_then(Value::as_str)
}

fn config_scalar_bool(settings: &BTreeMap<String, Value>, key: &str) -> Option<bool> {
    settings.get(key).and_then(|value| match value {
        Value::Bool(value) => Some(*value),
        Value::Number(value) => value.as_i64().map(|value| value != 0),
        Value::String(value) if value.eq_ignore_ascii_case("true") => Some(true),
        Value::String(value) if value.eq_ignore_ascii_case("false") => Some(false),
        Value::String(value) => value.parse::<i64>().ok().map(|value| value != 0),
        _ => None,
    })
}

fn config_array_f64(settings: &BTreeMap<String, Value>, key: &str, index: usize) -> Option<f64> {
    settings
        .get(key)
        .and_then(Value::as_array)
        .and_then(|values| values.get(index).or_else(|| values.first()))
        .and_then(value_as_f64)
}

fn config_array_bool(settings: &BTreeMap<String, Value>, key: &str, index: usize) -> Option<bool> {
    settings
        .get(key)
        .and_then(Value::as_array)
        .and_then(|values| values.get(index).or_else(|| values.first()))
        .and_then(|value| match value {
            Value::Bool(value) => Some(*value),
            Value::Number(value) => value.as_i64().map(|value| value != 0),
            Value::String(value) if value.eq_ignore_ascii_case("true") => Some(true),
            Value::String(value) if value.eq_ignore_ascii_case("false") => Some(false),
            Value::String(value) => value.parse::<i64>().ok().map(|value| value != 0),
            _ => None,
        })
}

fn value_as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(value) => value.as_f64(),
        Value::String(value) => value.parse::<f64>().ok(),
        _ => None,
    }
}

fn parse_profile_color(value: &str) -> Result<[u8; 4], U1DirectError> {
    let value = value.strip_prefix('#').unwrap_or(value);
    if value.len() != 6 && value.len() != 8 {
        return Err(U1DirectError::Plan(format!(
            "physical filament color #{value} is not a six- or eight-digit HEX color"
        )));
    }
    let parse = |range: std::ops::Range<usize>| {
        u8::from_str_radix(&value[range], 16).map_err(|_| {
            U1DirectError::Plan(format!("physical filament color #{value} is invalid"))
        })
    };
    Ok([
        parse(0..2)?,
        parse(2..4)?,
        parse(4..6)?,
        if value.len() == 8 { parse(6..8)? } else { 255 },
    ])
}

fn orca_flush_volume(minimum: i32, source: [u8; 4], target: [u8; 4]) -> i32 {
    let source = if source[3] == 0 {
        [255, 255, 255, 255]
    } else {
        source
    };
    let target = if target[3] == 0 {
        [255, 255, 255, 255]
    } else {
        target
    };
    let source_rgb = [
        f32::from(source[0]) / 255.0,
        f32::from(source[1]) / 255.0,
        f32::from(source[2]) / 255.0,
    ];
    let target_rgb = [
        f32::from(target[0]) / 255.0,
        f32::from(target[1]) / 255.0,
        f32::from(target[2]) / 255.0,
    ];
    let source_hsv = rgb_to_orca_hsv(source_rgb);
    let target_hsv = rgb_to_orca_hsv(target_rgb);
    let source_hue = source_hsv[0].to_radians();
    let target_hue = target_hsv[0].to_radians();
    let delta_x = source_hue.cos() * source_hsv[1] * source_hsv[2]
        - target_hue.cos() * target_hsv[1] * target_hsv[2];
    let delta_y = source_hue.sin() * source_hsv[1] * source_hsv[2]
        - target_hue.sin() * target_hsv[1] * target_hsv[2];
    let mut hue_saturation_distance = (delta_x * delta_x + delta_y * delta_y).sqrt().min(1.2);
    let source_luminance = source_rgb[0] * 0.3 + source_rgb[1] * 0.59 + source_rgb[2] * 0.11;
    let target_luminance = target_rgb[0] * 0.3 + target_rgb[1] * 0.59 + target_rgb[2] * 0.11;
    let luminance_flush = if target_luminance >= source_luminance {
        (target_luminance - source_luminance).powf(0.7) * 560.0
    } else {
        let intermediate_value = 0.67 * target_hsv[2] + 0.33 * source_hsv[2];
        hue_saturation_distance = hue_saturation_distance.min(intermediate_value);
        (source_luminance - target_luminance) * 80.0
    };
    let hue_saturation_flush = 230.0 * hue_saturation_distance;
    let angle = 120.0_f32.to_radians();
    let color_flush = (hue_saturation_flush * hue_saturation_flush
        + luminance_flush * luminance_flush
        - 2.0 * hue_saturation_flush * luminance_flush * angle.cos())
    .sqrt()
    .max(60.0);
    ((color_flush + minimum as f32) as i32).min(800)
}

fn rgb_to_orca_hsv(rgb: [f32; 3]) -> [f32; 3] {
    let maximum = rgb.into_iter().fold(f32::NEG_INFINITY, f32::max);
    let minimum = rgb.into_iter().fold(f32::INFINITY, f32::min);
    let delta = maximum - minimum;
    let hue = if delta.abs() < 0.001 {
        0.0
    } else if maximum == rgb[0] {
        60.0 * (((rgb[1] - rgb[2]) / delta) % 6.0)
    } else if maximum == rgb[1] {
        60.0 * ((rgb[2] - rgb[0]) / delta + 2.0)
    } else {
        60.0 * ((rgb[0] - rgb[1]) / delta + 4.0)
    };
    let saturation = if maximum.abs() < 0.001 {
        0.0
    } else {
        delta / maximum
    };
    [hue, saturation, maximum]
}

fn prepared_physical_slots(
    input: &PlanningInput,
    loadout: &u1_planner::U1Loadout,
    profiles: &[PhysicalProfile; 4],
) -> Result<Vec<PreparedPhysicalSlot>, U1DirectError> {
    let inventory = input
        .inventory
        .iter()
        .map(|spool| (spool.id.as_str(), spool))
        .collect::<BTreeMap<_, _>>();
    Toolhead::ALL
        .into_iter()
        .map(|toolhead| {
            let spool_id = loadout.spool(toolhead);
            let spool = spool_id.and_then(|id| inventory.get(id).copied());
            Ok(PreparedPhysicalSlot {
                toolhead: toolhead_name(toolhead).into(),
                spool_id: spool_id.map(str::to_owned),
                spool_name: spool.map(|spool| spool.display_name.clone()),
                material: spool.map(|spool| material_name(&spool.material).to_owned()),
                color: spool.map(|spool| rgb_hex(spool.actual_color())),
                profile: profiles[toolhead.index()].name.clone(),
                setting_id: profiles[toolhead.index()].setting_id.clone(),
                filament_id: profiles[toolhead.index()].filament_id.clone(),
            })
        })
        .collect()
}

fn toolhead_name(toolhead: Toolhead) -> &'static str {
    match toolhead {
        Toolhead::T1 => "T1",
        Toolhead::T2 => "T2",
        Toolhead::T3 => "T3",
        Toolhead::T4 => "T4",
    }
}

fn format_setup_action(action: &u1_planner::SetupAction, inventory: &[Spool]) -> String {
    let phase = match action.phase {
        u1_planner::SetupPhase::BeforeBatch => "Before batch",
        u1_planner::SetupPhase::AfterBatch => "After batch",
    };
    let kind = match action.kind {
        u1_planner::SetupActionKind::Keep => "Keep",
        u1_planner::SetupActionKind::Unload => "Unload",
        u1_planner::SetupActionKind::Load => "Load",
        u1_planner::SetupActionKind::Restore => "Restore",
    };
    let target = action.toolhead.map(toolhead_name).unwrap_or("Printer");
    format!(
        "{phase} — {target} — {kind}: {} → {}",
        format_slot_state(&action.from, inventory),
        format_slot_state(&action.to, inventory)
    )
}

fn format_slot_state(state: &u1_planner::ToolheadSlotState, inventory: &[Spool]) -> String {
    match state {
        u1_planner::ToolheadSlotState::Unknown => "unknown".into(),
        u1_planner::ToolheadSlotState::Empty => "empty".into(),
        u1_planner::ToolheadSlotState::Loaded(spool_id) => inventory
            .iter()
            .find(|spool| spool.id == *spool_id)
            .map(|spool| format!("{} ({spool_id})", spool.display_name))
            .unwrap_or_else(|| spool_id.clone()),
    }
}

fn material_name(material: &Material) -> &str {
    match material {
        Material::Pla => "PLA",
        Material::Petg => "PETG",
        Material::Abs => "ABS",
        Material::Asa => "ASA",
        Material::Tpu => "TPU",
        Material::Other(value) => value,
    }
}

fn rgb_hex(color: RgbColor) -> String {
    format!("#{:02X}{:02X}{:02X}", color.red, color.green, color.blue)
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

fn staging_paths(
    record: &U1DirectStagingRecoveryRecord,
) -> Result<(PathBuf, PathBuf), U1DirectError> {
    if record.schema_version != STAGING_RECORD_SCHEMA_VERSION {
        return Err(U1DirectError::Publish(
            "staging recovery record has an unsupported schema version".into(),
        ));
    }
    let conversion_id = uuid::Uuid::parse_str(&record.conversion_id).map_err(|_| {
        U1DirectError::Publish("staging recovery record has an invalid conversion ID".into())
    })?;
    if conversion_id.hyphenated().to_string() != record.conversion_id {
        return Err(U1DirectError::Publish(
            "staging recovery record has a non-canonical conversion ID".into(),
        ));
    }
    let recovery_key = uuid::Uuid::parse_str(&record.recovery_key).map_err(|_| {
        U1DirectError::Publish("staging recovery record has an invalid recovery key".into())
    })?;
    if recovery_key.hyphenated().to_string() != record.recovery_key {
        return Err(U1DirectError::Publish(
            "staging recovery record has a non-canonical recovery key".into(),
        ));
    }
    let canonical_parent =
        record
            .destination_parent
            .canonicalize()
            .map_err(|source| U1DirectError::Read {
                path: record.destination_parent.clone(),
                source,
            })?;
    if canonical_parent != record.destination_parent || !canonical_parent.is_dir() {
        return Err(U1DirectError::Publish(
            "staging recovery destination is no longer the canonical directory that was registered"
                .into(),
        ));
    }
    let base_name = format!("{STAGING_DIRECTORY_PREFIX}{}", record.conversion_id);
    Ok((
        canonical_parent.join(&base_name),
        canonical_parent.join(format!("{base_name}{STAGING_OWNER_SUFFIX}")),
    ))
}

fn process_is_running(process_id: u32) -> bool {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let Ok(process_id) = i32::try_from(process_id) else {
            return false;
        };
        if process_id <= 0 {
            return false;
        }
        let result = unsafe {
            // SAFETY: signal zero performs an existence/permission check and
            // does not deliver a signal to the target process.
            libc::kill(process_id, 0)
        };
        result == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = process_id;
        false
    }
}

fn validate_staging_owner(
    marker_path: &Path,
    record: &U1DirectStagingRecoveryRecord,
) -> Result<(), U1DirectError> {
    let metadata = fs::symlink_metadata(marker_path).map_err(|source| U1DirectError::Read {
        path: marker_path.to_path_buf(),
        source,
    })?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_STAGING_OWNER_BYTES
    {
        return Err(U1DirectError::Publish(
            "staging owner marker is not a bounded regular file".into(),
        ));
    }
    let bytes = fs::read(marker_path).map_err(|source| U1DirectError::Read {
        path: marker_path.to_path_buf(),
        source,
    })?;
    let owner = serde_json::from_slice::<U1DirectStagingOwner>(&bytes).map_err(|source| {
        U1DirectError::Json {
            path: "staging owner marker".into(),
            source,
        }
    })?;
    if owner.schema_version != STAGING_RECORD_SCHEMA_VERSION
        || owner.conversion_id != record.conversion_id
        || owner.recovery_key != record.recovery_key
    {
        return Err(U1DirectError::Publish(
            "staging owner marker does not match its recovery capability".into(),
        ));
    }
    Ok(())
}

fn cleanup_registered_staging(
    record: &U1DirectStagingRecoveryRecord,
    require_abandoned_owner: bool,
) -> Result<U1DirectStagingCleanup, U1DirectError> {
    let (staging_path, marker_path) = staging_paths(record)?;
    let staging_metadata = match fs::symlink_metadata(&staging_path) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(U1DirectError::Read {
                path: staging_path,
                source,
            });
        }
    };
    let marker_exists = match fs::symlink_metadata(&marker_path) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(source) => {
            return Err(U1DirectError::Read {
                path: marker_path,
                source,
            });
        }
    };
    if staging_metadata.is_none() && !marker_exists {
        if require_abandoned_owner && process_is_running(record.owner_process_id) {
            return Ok(U1DirectStagingCleanup::OwnerStillRunning);
        }
        return Ok(U1DirectStagingCleanup::Missing);
    }
    if require_abandoned_owner && process_is_running(record.owner_process_id) {
        return Ok(U1DirectStagingCleanup::OwnerStillRunning);
    }
    if !marker_exists {
        return Err(U1DirectError::Publish(
            "refusing to remove a staging directory without its owner marker".into(),
        ));
    }
    validate_staging_owner(&marker_path, record)?;
    let staging_was_present = staging_metadata.is_some();
    if let Some(metadata) = staging_metadata {
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return Err(U1DirectError::Publish(
                "refusing to remove a staging path that is not a real directory".into(),
            ));
        }
        fs::remove_dir_all(&staging_path).map_err(|source| U1DirectError::Write {
            path: staging_path,
            source,
        })?;
    }
    fs::remove_file(&marker_path).map_err(|source| U1DirectError::Write {
        path: marker_path,
        source,
    })?;
    sync_directory(&record.destination_parent)?;
    Ok(if staging_was_present {
        U1DirectStagingCleanup::Removed
    } else {
        U1DirectStagingCleanup::Missing
    })
}

/// Removes registered staging only when its recorded owner process no longer
/// exists and the secret owner marker matches. Unregistered lookalike paths are
/// never scanned or removed.
pub fn cleanup_abandoned_u1_direct_staging(
    record: &U1DirectStagingRecoveryRecord,
) -> Result<U1DirectStagingCleanup, U1DirectError> {
    cleanup_registered_staging(record, true)
}

/// Final cleanup used after the corresponding blocking writer has returned.
/// The caller owns that worker lifecycle, so the current process ID is not an
/// abandonment blocker.
pub fn finalize_u1_direct_staging(
    record: &U1DirectStagingRecoveryRecord,
) -> Result<U1DirectStagingCleanup, U1DirectError> {
    cleanup_registered_staging(record, false)
}

struct OwnedConversionStaging {
    path: PathBuf,
    marker_path: PathBuf,
    published: bool,
}

impl OwnedConversionStaging {
    fn create(
        destination_parent: &Path,
        control: &U1DirectConversionControl,
    ) -> Result<Self, U1DirectError> {
        let record = control.recovery_record(destination_parent)?;
        let (path, marker_path) = staging_paths(&record)?;
        let owner = U1DirectStagingOwner {
            schema_version: STAGING_RECORD_SCHEMA_VERSION,
            conversion_id: record.conversion_id,
            recovery_key: record.recovery_key,
        };
        let mut owner_bytes = serde_json::to_vec(&owner).map_err(|source| U1DirectError::Json {
            path: "staging owner marker".into(),
            source,
        })?;
        owner_bytes.push(b'\n');
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut marker = options
            .open(&marker_path)
            .map_err(|source| U1DirectError::Write {
                path: marker_path.clone(),
                source,
            })?;
        if let Err(source) = marker
            .write_all(&owner_bytes)
            .and_then(|()| marker.sync_all())
        {
            let _ = fs::remove_file(&marker_path);
            return Err(U1DirectError::Write {
                path: marker_path,
                source,
            });
        }
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        if let Err(source) = builder.create(&path) {
            let _ = fs::remove_file(&marker_path);
            return Err(U1DirectError::Write { path, source });
        }
        let staging = Self {
            path,
            marker_path,
            published: false,
        };
        sync_directory(destination_parent)?;
        Ok(staging)
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn mark_published(&mut self) {
        self.published = true;
    }
}

impl Drop for OwnedConversionStaging {
    fn drop(&mut self) {
        let staging_removed = self.published
            || match fs::remove_dir_all(&self.path) {
                Ok(()) => true,
                Err(error) if error.kind() == io::ErrorKind::NotFound => true,
                Err(_) => false,
            };
        if staging_removed {
            let _ = fs::remove_file(&self.marker_path);
        }
    }
}

/// Recoverable private staging owned by one backend conversion control. This
/// is shared by the mixed-adapter orchestrator so Direct, Full Spectrum and A1
/// outputs become visible through one final no-clobber atomic publication.
pub struct U1NativeConversionStaging {
    inner: OwnedConversionStaging,
}

impl U1NativeConversionStaging {
    pub fn create(
        destination_parent: &Path,
        control: &U1DirectConversionControl,
    ) -> Result<Self, U1DirectError> {
        OwnedConversionStaging::create(destination_parent, control).map(|inner| Self { inner })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        self.inner.path()
    }

    /// Atomically publishes the complete staged directory without replacing
    /// an existing path. This is the cancellation/publication linearization
    /// point for the outer mixed-adapter bundle.
    pub fn publish(
        mut self,
        final_directory: &Path,
        control: &U1DirectConversionControl,
    ) -> Result<(), U1DirectError> {
        publish_owned_staging(&mut self.inner, final_directory, control)
    }
}

fn publish_owned_staging(
    staging: &mut OwnedConversionStaging,
    final_directory: &Path,
    control: &U1DirectConversionControl,
) -> Result<(), U1DirectError> {
    // This compare-exchange is the cancellation/publication linearization
    // point. Cancellation that wins first leaves no final artifact; once this
    // succeeds, cancellation reports that publication is already too late.
    control.begin_publication()?;
    if let Err(error) = publish_directory_no_clobber(staging.path(), final_directory) {
        if error.kind() == io::ErrorKind::AlreadyExists
            || matches!(
                error.raw_os_error(),
                Some(code) if code == libc::EEXIST || code == libc::ENOTEMPTY
            )
        {
            return Err(U1DirectError::OutputExists(final_directory.to_path_buf()));
        }
        return Err(U1DirectError::Publish(format!(
            "failed to atomically publish {} without replacing an existing entry: {error}",
            final_directory.display()
        )));
    }
    staging.mark_published();
    control.mark_published();
    Ok(())
}

pub fn convert_u1_direct_bundle(
    application_path: &Path,
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    destination_parent: &Path,
) -> Result<U1DirectConversionResult, U1DirectError> {
    let control = U1DirectConversionControl::new();
    convert_u1_direct_bundle_cancellable(
        application_path,
        source_path,
        analysis,
        input,
        result,
        destination_parent,
        &control,
    )
}

pub fn convert_u1_direct_bundle_cancellable(
    application_path: &Path,
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    destination_parent: &Path,
    control: &U1DirectConversionControl,
) -> Result<U1DirectConversionResult, U1DirectError> {
    control.checkpoint()?;
    let context = adapter_context(application_path)?;
    control.checkpoint()?;
    if !context.capability.conversion_available {
        return Err(U1DirectError::Capability(
            context.capability.issues.join(" "),
        ));
    }
    convert_with_context(
        source_path,
        analysis,
        input,
        result,
        destination_parent,
        &context,
        control,
    )
}

/// Produces a structurally validated candidate for the explicit adapter
/// qualification workflow. Desktop conversion never calls this bypass: it is
/// intentionally named and separated so an unqualified GUI baseline cannot be
/// mistaken for production availability.
pub fn build_u1_direct_qualification_candidate(
    application_path: &Path,
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    destination_parent: &Path,
) -> Result<U1DirectConversionResult, U1DirectError> {
    let control = U1DirectConversionControl::new();
    let context = adapter_context(application_path)?;
    if !context.capability.installation_supported() {
        return Err(U1DirectError::Capability(
            context.capability.issues.join(" "),
        ));
    }
    convert_with_context(
        source_path,
        analysis,
        input,
        result,
        destination_parent,
        &context,
        &control,
    )
}

/// Builds the bounded geometry/placement/paint substrate required by the Full
/// Spectrum metadata adapter. This deliberately does not expose a printable
/// production file: callers must pass the result through the Full Spectrum
/// semantic writer and its versioned qualification gate before publication.
#[allow(clippy::too_many_arguments)]
pub fn write_u1_full_spectrum_normalized_substrate(
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    artifact: &U1FullSpectrumPreparedArtifact,
    project_settings: &[u8],
    destination: &Path,
    control: &U1DirectConversionControl,
) -> Result<U1FullSpectrumNormalizedSubstrateReport, U1DirectError> {
    write_u1_full_spectrum_normalized_substrates(
        source_path,
        analysis,
        input,
        &[(artifact, project_settings, destination)],
        control,
    )?
    .into_iter()
    .next()
    .ok_or_else(|| U1DirectError::Plan("no Full Spectrum substrate was requested".into()))
}

/// Batch form of [`write_u1_full_spectrum_normalized_substrate`]. The source
/// is authenticated and snapshotted once, while independent artifact geometry
/// rewrites run with a bounded amount of parallelism. Results and errors retain
/// request order, and no destination is published until every staged package
/// passes validation and the live source identity is reverified.
#[allow(clippy::too_many_arguments)]
pub fn write_u1_full_spectrum_normalized_substrates(
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    requests: &[(&U1FullSpectrumPreparedArtifact, &[u8], &Path)],
    control: &U1DirectConversionControl,
) -> Result<Vec<U1FullSpectrumNormalizedSubstrateReport>, U1DirectError> {
    if requests.is_empty() {
        return Err(U1DirectError::Plan(
            "no Full Spectrum substrate was requested".into(),
        ));
    }
    control.checkpoint()?;
    let mut destinations = BTreeSet::new();
    for (_, _, destination) in requests {
        let parent = destination.parent().ok_or_else(|| {
            U1DirectError::Publish("the Full Spectrum substrate has no parent directory".into())
        })?;
        if !parent.is_dir() {
            return Err(U1DirectError::Publish(
                "the Full Spectrum substrate parent directory does not exist".into(),
            ));
        }
        if destination.exists() {
            return Err(U1DirectError::OutputExists((*destination).to_owned()));
        }
        if source_path == *destination {
            return Err(U1DirectError::Plan(
                "the immutable source 3MF cannot be used as a Full Spectrum substrate".into(),
            ));
        }
        if !destinations.insert((*destination).to_owned()) {
            return Err(U1DirectError::Plan(format!(
                "Full Spectrum substrate destination {:?} was requested more than once",
                destination
            )));
        }
    }

    let work = tempfile::tempdir().map_err(|source| U1DirectError::Write {
        path: std::env::temp_dir(),
        source,
    })?;
    let source_snapshot = snapshot_source(source_path, &analysis.input, work.path(), control)?;
    let scratch = (0..requests.len())
        .map(|_| {
            tempfile::tempdir_in(work.path()).map_err(|source| U1DirectError::Write {
                path: work.path().to_owned(),
                source,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let worker_limit = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .clamp(1, 4);
    let source_snapshot_path = source_snapshot.path();
    let mut staged = Vec::with_capacity(requests.len());
    for (request_chunk, scratch_chunk) in requests
        .chunks(worker_limit)
        .zip(scratch.chunks(worker_limit))
    {
        let chunk_results = std::thread::scope(|scope| {
            let handles = request_chunk
                .iter()
                .zip(scratch_chunk)
                .map(|(request, scratch)| {
                    let &(artifact, project_settings, destination) = request;
                    scope.spawn(move || {
                        stage_u1_full_spectrum_normalized_substrate_from_snapshot(
                            source_snapshot_path,
                            analysis,
                            input,
                            artifact,
                            project_settings,
                            destination,
                            scratch.path(),
                            control,
                        )
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| {
                    handle.join().map_err(|_| {
                        U1DirectError::Publish(
                            "a Full Spectrum substrate worker terminated unexpectedly".into(),
                        )
                    })?
                })
                .collect::<Result<Vec<_>, _>>()
        })?;
        staged.extend(chunk_results);
    }
    reverify_open_snapshot_and_source(
        source_path,
        source_snapshot.as_file(),
        &analysis.input,
        control,
    )?;
    control.checkpoint()?;

    let mut reports = Vec::with_capacity(staged.len());
    for staged in staged {
        let report = match staged.package.publish() {
            Ok(report) => report,
            Err(error) => {
                for destination in &destinations {
                    let _ = fs::remove_file(destination);
                }
                return Err(U1DirectError::Opc(error.to_string()));
            }
        };
        reports.push(U1FullSpectrumNormalizedSubstrateReport {
            path: report.destination,
            byte_size: report.package_bytes,
            sha256: report.package_sha256,
            plate_count: staged.plate_count,
            source_unit_ids: staged.source_unit_ids,
        });
    }
    Ok(reports)
}

struct StagedU1FullSpectrumNormalizedSubstrate {
    package: ValidatedStagedPackage,
    plate_count: usize,
    source_unit_ids: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
fn stage_u1_full_spectrum_normalized_substrate_from_snapshot(
    source_snapshot_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    artifact: &U1FullSpectrumPreparedArtifact,
    project_settings: &[u8],
    destination: &Path,
    work_directory: &Path,
    control: &U1DirectConversionControl,
) -> Result<StagedU1FullSpectrumNormalizedSubstrate, U1DirectError> {
    let build_plan =
        build_full_spectrum_substrate_plan(analysis, input, artifact, project_settings.to_vec())?;
    let (rewrite_archive, rewritten_entries, _) = rewrite_models_to_archive(
        source_snapshot_path,
        &build_plan,
        work_directory,
        analysis.source.title.as_deref(),
        control,
    )?;
    let rewrite_identity_raw = hash_path_cancellable(rewrite_archive.path(), control)?;
    let rewrite_identity =
        ExpectedSourceIdentity::new(rewrite_identity_raw.0, rewrite_identity_raw.1)
            .map_err(|error| U1DirectError::Opc(error.to_string()))?;
    let source_identity =
        ExpectedSourceIdentity::new(analysis.input.byte_size, analysis.input.sha256.clone())
            .map_err(|error| U1DirectError::Opc(error.to_string()))?;
    let model_settings = rewrite_model_settings(source_snapshot_path, &build_plan)?;

    let mut content_types = ContentTypesBuilder::project_3mf();
    content_types
        .add_override(PROJECT_SETTINGS_PATH, "application/json")
        .map_err(|error| U1DirectError::Opc(error.to_string()))?;
    content_types
        .add_override(MODEL_SETTINGS_PATH, "application/xml")
        .map_err(|error| U1DirectError::Opc(error.to_string()))?;
    content_types
        .add_default("png", "image/png")
        .map_err(|error| U1DirectError::Opc(error.to_string()))?;
    content_types
        .add_default("gcode", "text/x.gcode")
        .map_err(|error| U1DirectError::Opc(error.to_string()))?;
    let root_relationship =
        OpcRelationship::internal("rel-1", MODEL_RELATIONSHIP_TYPE, MAIN_MODEL_PATH)
            .map_err(|error| U1DirectError::Opc(error.to_string()))?;
    let model_relationships = build_plan
        .external_paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            OpcRelationship::internal(format!("rel-{}", index + 1), MODEL_RELATIONSHIP_TYPE, path)
                .map_err(|error| U1DirectError::Opc(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut package = OpcPackageWriter::new();
    package
        .verify_zip_source(source_snapshot_path, source_identity)
        .map_err(|error| U1DirectError::Opc(error.to_string()))?
        .add_bytes(
            "[Content_Types].xml",
            content_types
                .to_xml()
                .map_err(|error| U1DirectError::Opc(error.to_string()))?,
        )
        .map_err(|error| U1DirectError::Opc(error.to_string()))?
        .add_bytes(
            "_rels/.rels",
            relationships_xml(&[root_relationship])
                .map_err(|error| U1DirectError::Opc(error.to_string()))?,
        )
        .map_err(|error| U1DirectError::Opc(error.to_string()))?
        .add_bytes(PROJECT_SETTINGS_PATH, build_plan.project_settings.clone())
        .map_err(|error| U1DirectError::Opc(error.to_string()))?
        .add_bytes(MODEL_SETTINGS_PATH, model_settings)
        .map_err(|error| U1DirectError::Opc(error.to_string()))?;
    if !model_relationships.is_empty() {
        package
            .add_bytes(
                MAIN_MODEL_RELATIONSHIPS_PATH,
                relationships_xml(&model_relationships)
                    .map_err(|error| U1DirectError::Opc(error.to_string()))?,
            )
            .map_err(|error| U1DirectError::Opc(error.to_string()))?;
    }
    for entry in &rewritten_entries {
        package
            .copy_zip_entry_raw(rewrite_archive.path(), rewrite_identity.clone(), entry)
            .map_err(|error| U1DirectError::Opc(error.to_string()))?;
    }
    control.checkpoint()?;
    let staged = package
        .stage_to(destination)
        .map_err(|error| U1DirectError::Opc(error.to_string()))?;
    let validated = staged.validate().map_err(|error| match error {
        u1_three_mf::StagedPackageValidationError::Blocked { report } => {
            U1DirectError::Opc(format!(
                "Full Spectrum substrate failed structural validation: {}",
                serde_json::to_string(&report.issues)
                    .unwrap_or_else(|_| "validation report unavailable".into())
            ))
        }
        other => U1DirectError::Opc(other.to_string()),
    })?;
    Ok(StagedU1FullSpectrumNormalizedSubstrate {
        package: validated,
        plate_count: build_plan.plates.len(),
        source_unit_ids: build_plan.prepared.source_unit_ids,
    })
}

fn convert_with_context(
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    destination_parent: &Path,
    context: &AdapterContext,
    control: &U1DirectConversionControl,
) -> Result<U1DirectConversionResult, U1DirectError> {
    control.checkpoint()?;
    ensure_atomic_publication_supported()?;
    if !destination_parent.is_dir() {
        return Err(U1DirectError::Publish(
            "the selected destination directory does not exist".into(),
        ));
    }
    // Every temporary byte, including the authenticated source snapshot, lives
    // below the one recovery-registered staging directory. A crash therefore
    // cannot strand an untracked source copy beside the user's destination.
    let mut staging_directory = OwnedConversionStaging::create(destination_parent, control)?;
    let source_snapshot = snapshot_source(
        source_path,
        &analysis.input,
        staging_directory.path(),
        control,
    )?;
    control.checkpoint()?;
    let (preparation, build_plans) = prepare_with_context_from_verified_source(
        source_snapshot.path(),
        source_path,
        analysis,
        input,
        result,
        context,
        Some(control),
    )?;
    control.checkpoint()?;
    let final_directory = destination_parent.join(&preparation.bundle_directory_name);
    if final_directory.exists() {
        return Err(U1DirectError::OutputExists(final_directory));
    }
    let source_identity =
        ExpectedSourceIdentity::new(analysis.input.byte_size, analysis.input.sha256.clone())
            .map_err(|error| U1DirectError::Opc(error.to_string()))?;
    let mut published = Vec::with_capacity(build_plans.len());
    for build_plan in &build_plans {
        control.checkpoint()?;
        let (rewrite_archive, rewritten_entries, geometry_counts) = rewrite_models_to_archive(
            source_snapshot.path(),
            build_plan,
            staging_directory.path(),
            analysis.source.title.as_deref(),
            control,
        )?;
        control.checkpoint()?;
        let rewrite_identity_raw = hash_path_cancellable(rewrite_archive.path(), control)?;
        let rewrite_identity =
            ExpectedSourceIdentity::new(rewrite_identity_raw.0, rewrite_identity_raw.1)
                .map_err(|error| U1DirectError::Opc(error.to_string()))?;
        let model_settings = rewrite_model_settings(source_snapshot.path(), build_plan)?;
        control.checkpoint()?;
        let mut content_types = ContentTypesBuilder::project_3mf();
        content_types
            .add_override(PROJECT_SETTINGS_PATH, "application/json")
            .map_err(|error| U1DirectError::Opc(error.to_string()))?;
        content_types
            .add_override(MODEL_SETTINGS_PATH, "application/xml")
            .map_err(|error| U1DirectError::Opc(error.to_string()))?;
        content_types
            .add_default("png", "image/png")
            .map_err(|error| U1DirectError::Opc(error.to_string()))?;
        content_types
            .add_default("gcode", "text/x.gcode")
            .map_err(|error| U1DirectError::Opc(error.to_string()))?;
        let root_relationship =
            OpcRelationship::internal("rel-1", MODEL_RELATIONSHIP_TYPE, MAIN_MODEL_PATH)
                .map_err(|error| U1DirectError::Opc(error.to_string()))?;
        let model_relationships = build_plan
            .external_paths
            .iter()
            .enumerate()
            .map(|(index, path)| {
                OpcRelationship::internal(
                    format!("rel-{}", index + 1),
                    MODEL_RELATIONSHIP_TYPE,
                    path,
                )
                .map_err(|error| U1DirectError::Opc(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut package = OpcPackageWriter::new();
        package
            .verify_zip_source(source_snapshot.path(), source_identity.clone())
            .map_err(|error| U1DirectError::Opc(error.to_string()))?
            .add_bytes(
                "[Content_Types].xml",
                content_types
                    .to_xml()
                    .map_err(|error| U1DirectError::Opc(error.to_string()))?,
            )
            .map_err(|error| U1DirectError::Opc(error.to_string()))?
            .add_bytes(
                "_rels/.rels",
                relationships_xml(&[root_relationship])
                    .map_err(|error| U1DirectError::Opc(error.to_string()))?,
            )
            .map_err(|error| U1DirectError::Opc(error.to_string()))?
            .add_bytes(PROJECT_SETTINGS_PATH, build_plan.project_settings.clone())
            .map_err(|error| U1DirectError::Opc(error.to_string()))?
            .add_bytes(MODEL_SETTINGS_PATH, model_settings)
            .map_err(|error| U1DirectError::Opc(error.to_string()))?;
        if !model_relationships.is_empty() {
            package
                .add_bytes(
                    MAIN_MODEL_RELATIONSHIPS_PATH,
                    relationships_xml(&model_relationships)
                        .map_err(|error| U1DirectError::Opc(error.to_string()))?,
                )
                .map_err(|error| U1DirectError::Opc(error.to_string()))?;
        }
        for entry in &rewritten_entries {
            package
                .copy_zip_entry_raw(rewrite_archive.path(), rewrite_identity.clone(), entry)
                .map_err(|error| U1DirectError::Opc(error.to_string()))?;
        }
        let destination = staging_directory
            .path()
            .join(&build_plan.prepared.file_name);
        control.checkpoint()?;
        let staged = package
            .stage_to(&destination)
            .map_err(|error| U1DirectError::Opc(error.to_string()))?;
        control.checkpoint()?;
        validate_u1_direct_candidate(staged.path(), build_plan, &geometry_counts, analysis)?;
        control.checkpoint()?;
        let validated = staged.validate().map_err(|error| match error {
            u1_three_mf::StagedPackageValidationError::Blocked { report } => {
                U1DirectError::Opc(format!(
                    "staged 3MF package failed structural validation: {}",
                    serde_json::to_string(&report.issues)
                        .unwrap_or_else(|_| "validation report unavailable".into())
                ))
            }
            other => U1DirectError::Opc(other.to_string()),
        })?;
        control.checkpoint()?;
        let report = validated
            .publish()
            .map_err(|error| U1DirectError::Opc(error.to_string()))?;
        published.push(published_artifact(build_plan, &report));
    }
    control.checkpoint()?;
    reverify_open_snapshot_and_source(
        source_path,
        source_snapshot.as_file(),
        &analysis.input,
        control,
    )?;
    source_snapshot
        .close()
        .map_err(|source| U1DirectError::Write {
            path: staging_directory.path().to_path_buf(),
            source,
        })?;
    control.checkpoint()?;

    let source_to_target = published
        .iter()
        .zip(build_plans.iter())
        .flat_map(|(artifact, build_plan)| {
            build_plan.plates.iter().flat_map(move |plate| {
                plate
                    .units
                    .iter()
                    .map(move |unit| -> Result<_, U1DirectError> {
                        Ok(SourceTargetManifestEntry {
                            source_unit_id: unit.source_unit_id.as_str(),
                            source_plate_id: parse_source_plate_id(
                                unit.source_plate_id.as_deref(),
                            )?,
                            output_file: artifact.file_name.as_str(),
                            batch_id: artifact.batch_id.as_str(),
                            target_plate_id: plate.target_plate_id,
                        })
                    })
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let manifest_artifacts = published
        .iter()
        .zip(build_plans.iter())
        .map(|(artifact, build_plan)| ManifestArtifact {
            batch_id: &artifact.batch_id,
            file_name: &artifact.file_name,
            byte_size: artifact.byte_size,
            sha256: &artifact.sha256,
            plate_count: artifact.plate_count,
            target_plate_ids: &build_plan.prepared.target_plate_ids,
            source_plate_ids: &build_plan.prepared.source_plate_ids,
            source_unit_ids: &artifact.source_unit_ids,
            loadout: &build_plan.prepared.loadout,
            setup_actions: &build_plan.prepared.setup_actions,
        })
        .collect::<Vec<_>>();
    let manifest = BundleManifest {
        schema_version: 1,
        adapter_id: U1_DIRECT_ADAPTER_ID,
        adapter_evidence: &context.capability,
        source: &analysis.input,
        plan_fingerprint: &preparation.plan_fingerprint,
        artifacts: manifest_artifacts,
        source_to_target,
        warnings: &preparation.warnings,
    };
    let mut manifest_bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|source| U1DirectError::Json {
            path: "manifest.json".into(),
            source,
        })?;
    manifest_bytes.push(b'\n');
    write_new_file_synced(
        &staging_directory.path().join("manifest.json"),
        &manifest_bytes,
    )?;
    control.checkpoint()?;
    let mut plan_bytes = serde_json::to_vec_pretty(&serde_json::json!({
        "schemaVersion": 1,
        "source": analysis.input,
        "planFingerprint": preparation.plan_fingerprint,
        "adapterId": U1_DIRECT_ADAPTER_ID,
        "planningInput": input,
        "planningResult": result,
    }))
    .map_err(|source| U1DirectError::Json {
        path: "print-plan.json".into(),
        source,
    })?;
    plan_bytes.push(b'\n');
    write_new_file_synced(
        &staging_directory.path().join("print-plan.json"),
        &plan_bytes,
    )?;
    control.checkpoint()?;
    let mut checksum_lines = published
        .iter()
        .map(|artifact| format!("{}  {}", artifact.sha256, artifact.file_name))
        .collect::<Vec<_>>();
    checksum_lines.push(format!(
        "{}  manifest.json",
        hash_path(&staging_directory.path().join("manifest.json"))?.1
    ));
    checksum_lines.push(format!(
        "{}  print-plan.json",
        hash_path(&staging_directory.path().join("print-plan.json"))?.1
    ));
    checksum_lines.sort();
    let checksums = format!("{}\n", checksum_lines.join("\n"));
    write_new_file_synced(
        &staging_directory.path().join("checksums.sha256"),
        checksums.as_bytes(),
    )?;
    control.checkpoint()?;
    validate_bundle_entries(staging_directory.path(), &published)?;
    sync_directory(staging_directory.path())?;
    publish_owned_staging(&mut staging_directory, &final_directory, control)?;
    let mut result_warnings = preparation.warnings.clone();
    if let Err(error) = sync_directory(destination_parent) {
        result_warnings.push(format!(
            "The bundle was published successfully at {}, but the destination directory durability sync failed: {error}",
            final_directory.display()
        ));
    }
    for artifact in &mut published {
        artifact.path = final_directory.join(&artifact.file_name);
    }
    Ok(U1DirectConversionResult {
        adapter_id: U1_DIRECT_ADAPTER_ID.into(),
        manifest_path: final_directory.join("manifest.json"),
        output_directory: final_directory,
        artifacts: published,
        warnings: result_warnings,
    })
}

fn ensure_atomic_publication_supported() -> Result<(), U1DirectError> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err(U1DirectError::Capability(
            "this platform has no qualified atomic no-replace directory publication primitive"
                .into(),
        ))
    }
}

fn published_artifact(plan: &ArtifactBuildPlan, report: &OpcWriteReport) -> PublishedArtifact {
    PublishedArtifact {
        batch_id: plan.prepared.batch_id.clone(),
        file_name: plan.prepared.file_name.clone(),
        path: report.destination.clone(),
        byte_size: report.package_bytes,
        sha256: report.package_sha256.clone(),
        plate_count: plan.plates.len(),
        target_plate_ids: plan.prepared.target_plate_ids.clone(),
        source_unit_ids: plan.prepared.source_unit_ids.clone(),
    }
}

fn snapshot_source(
    source_path: &Path,
    expected: &InputIdentity,
    staging_directory: &Path,
    control: &U1DirectConversionControl,
) -> Result<NamedTempFile, U1DirectError> {
    control.checkpoint()?;
    let mut source_file = File::open(source_path).map_err(|source| U1DirectError::Read {
        path: source_path.to_path_buf(),
        source,
    })?;
    let metadata = source_file
        .metadata()
        .map_err(|source| U1DirectError::Read {
            path: source_path.to_path_buf(),
            source,
        })?;
    if !metadata.is_file() || metadata.len() != expected.byte_size {
        return Err(U1DirectError::SourceIdentityChanged);
    }
    let mut snapshot =
        NamedTempFile::new_in(staging_directory).map_err(|source| U1DirectError::Write {
            path: staging_directory.to_path_buf(),
            source,
        })?;
    let mut hasher = Sha256::new();
    let mut byte_size = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        control.checkpoint()?;
        let read = source_file
            .read(&mut buffer)
            .map_err(|source| U1DirectError::Read {
                path: source_path.to_path_buf(),
                source,
            })?;
        if read == 0 {
            break;
        }
        snapshot
            .write_all(&buffer[..read])
            .map_err(|source| U1DirectError::Write {
                path: snapshot.path().to_path_buf(),
                source,
            })?;
        hasher.update(&buffer[..read]);
        byte_size = byte_size.saturating_add(read as u64);
    }
    snapshot
        .as_file_mut()
        .sync_all()
        .map_err(|source| U1DirectError::Write {
            path: snapshot.path().to_path_buf(),
            source,
        })?;
    control.checkpoint()?;
    let hash = format!("{:x}", hasher.finalize());
    if byte_size != expected.byte_size || hash != expected.sha256 {
        return Err(U1DirectError::SourceIdentityChanged);
    }
    Ok(snapshot)
}

fn reverify_open_snapshot_and_source(
    source_path: &Path,
    snapshot_file: &File,
    expected: &InputIdentity,
    control: &U1DirectConversionControl,
) -> Result<(), U1DirectError> {
    let mut snapshot_file = snapshot_file
        .try_clone()
        .map_err(|source| U1DirectError::Read {
            path: source_path.to_path_buf(),
            source,
        })?;
    let snapshot = hash_reader_cancellable(&mut snapshot_file, control)?;
    let source = hash_path_cancellable(source_path, control)?;
    if snapshot.0 != expected.byte_size
        || snapshot.1 != expected.sha256
        || source.0 != expected.byte_size
        || source.1 != expected.sha256
    {
        return Err(U1DirectError::SourceIdentityChanged);
    }
    Ok(())
}

fn write_new_file_synced(path: &Path, bytes: &[u8]) -> Result<(), U1DirectError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options.open(path).map_err(|source| U1DirectError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|source| U1DirectError::Write {
            path: path.to_path_buf(),
            source,
        })
}

fn validate_bundle_entries(
    directory: &Path,
    artifacts: &[PublishedArtifact],
) -> Result<(), U1DirectError> {
    let mut expected = artifacts
        .iter()
        .map(|artifact| artifact.file_name.clone())
        .collect::<BTreeSet<_>>();
    expected.extend(
        ["manifest.json", "print-plan.json", "checksums.sha256"]
            .into_iter()
            .map(str::to_owned),
    );
    let mut actual = BTreeSet::new();
    for entry in fs::read_dir(directory).map_err(|source| U1DirectError::Read {
        path: directory.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| U1DirectError::Read {
            path: directory.to_path_buf(),
            source,
        })?;
        let file_type = entry.file_type().map_err(|source| U1DirectError::Read {
            path: entry.path(),
            source,
        })?;
        if !file_type.is_file() || file_type.is_symlink() {
            return Err(U1DirectError::Publish(format!(
                "unexpected non-file entry remained in the staged bundle: {}",
                entry.path().display()
            )));
        }
        let name = entry.file_name().into_string().map_err(|_| {
            U1DirectError::Publish("staged bundle contains a non-UTF-8 entry name".into())
        })?;
        actual.insert(name);
    }
    if actual != expected {
        return Err(U1DirectError::Publish(format!(
            "staged bundle does not match the strict allowlist (expected {expected:?}, found {actual:?})"
        )));
    }
    Ok(())
}

/// Atomically publishes a fully prepared directory while refusing to replace
/// any destination entry created by another process after our advisory
/// existence check. Unsupported platforms fail closed.
fn publish_directory_no_clobber(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let source = CString::new(source.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "staging path contains an interior NUL byte",
            )
        })?;
        let destination = CString::new(destination.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination path contains an interior NUL byte",
            )
        })?;

        #[cfg(target_os = "macos")]
        let result = unsafe {
            // SAFETY: both C strings live through the call, contain no interior
            // NUL, and the flags request a no-replace atomic rename.
            libc::renameatx_np(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        #[cfg(target_os = "linux")]
        let result = unsafe {
            // SAFETY: both C strings live through the call, contain no interior
            // NUL, and the flags request a no-replace atomic rename.
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };

        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (source, destination);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this platform has no qualified atomic no-replace directory publication primitive",
        ))
    }
}

fn sync_directory(path: &Path) -> Result<(), U1DirectError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| U1DirectError::Write {
            path: path.to_path_buf(),
            source,
        })
}

fn rewrite_models_to_archive(
    source_snapshot: &Path,
    plan: &ArtifactBuildPlan,
    staging_directory: &Path,
    title: Option<&str>,
    control: &U1DirectConversionControl,
) -> Result<(NamedTempFile, Vec<String>, GeometryCounts), U1DirectError> {
    control.checkpoint()?;
    let source_file = File::open(source_snapshot).map_err(|source| U1DirectError::Read {
        path: source_snapshot.to_path_buf(),
        source,
    })?;
    let mut source = ZipArchive::new(source_file)?;
    let temporary =
        NamedTempFile::new_in(staging_directory).map_err(|source| U1DirectError::Write {
            path: staging_directory.to_path_buf(),
            source,
        })?;
    let output_file = temporary.reopen().map_err(|source| U1DirectError::Write {
        path: temporary.path().to_path_buf(),
        source,
    })?;
    let mut output = ZipWriter::new(output_file);
    let timestamp = DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0).expect("ZIP epoch is valid");
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(6))
        .last_modified_time(timestamp)
        .unix_permissions(0o644);
    let mut paths = BTreeSet::from([MAIN_MODEL_PATH.to_owned()]);
    paths.extend(plan.external_paths.iter().cloned());
    let mut geometry = GeometryCounts::default();
    for path in &paths {
        control.checkpoint()?;
        let entry = source.by_name(path)?;
        output.start_file(path, options)?;
        rewrite_model_xml(
            entry,
            &mut output,
            path,
            plan,
            title,
            &mut geometry,
            control,
        )?;
    }
    control.checkpoint()?;
    let output_file = output.finish()?;
    output_file
        .sync_all()
        .map_err(|source| U1DirectError::Write {
            path: temporary.path().to_path_buf(),
            source,
        })?;
    Ok((temporary, paths.into_iter().collect(), geometry))
}

fn rewrite_model_xml<R: Read, W: Write>(
    input: R,
    output: &mut W,
    path: &str,
    plan: &ArtifactBuildPlan,
    title: Option<&str>,
    geometry: &mut GeometryCounts,
    control: &U1DirectConversionControl,
) -> Result<(), U1DirectError> {
    control.checkpoint()?;
    let resource_maps = if path == MAIN_MODEL_PATH {
        for id in &plan.selected_root_resource_ids {
            if let (Some(object_mapping), Some(resource_mapping)) = (
                plan.object_slot_maps.get(id),
                plan.resource_slot_maps.get(&(path.to_owned(), *id)),
            ) && object_mapping != resource_mapping
            {
                return Err(U1DirectError::Plan(format!(
                    "root model resource {id} has conflicting object and path-qualified Direct mappings"
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
        return Err(U1DirectError::Plan(format!(
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
    let mut generated_metadata_written = false;
    let mut seen_external_paths = BTreeSet::new();
    loop {
        control.checkpoint()?;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| U1DirectError::Xml {
                path: path.into(),
                message: error.to_string(),
            })?;
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
                    in_build = true;
                }
                if path != MAIN_MODEL_PATH
                    && current_resource.is_some()
                    && (name == b"components" || name == b"component")
                {
                    return Err(U1DirectError::Plan(format!(
                        "selected resource in external model part {path} contains a nested component graph; Stage B supports external mesh resources only"
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
                        .map_err(|error| U1DirectError::Xml {
                            path: path.into(),
                            message: error.to_string(),
                        })?;
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
                    .map_err(|error| U1DirectError::Xml {
                        path: path.into(),
                        message: error.to_string(),
                    })?;
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
                    return Err(U1DirectError::Plan(format!(
                        "selected resource in external model part {path} contains a nested component graph; Stage B supports external mesh resources only"
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
                            .map_err(|error| U1DirectError::Xml {
                                path: path.into(),
                                message: error.to_string(),
                            })?;
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
                    .map_err(|error| U1DirectError::Xml {
                        path: path.into(),
                        message: error.to_string(),
                    })?;
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
                    .map_err(|error| U1DirectError::Xml {
                        path: path.into(),
                        message: error.to_string(),
                    })?;
                depth = depth.saturating_sub(1);
                if closes_object {
                    current_resource = None;
                } else if closes_build {
                    in_build = false;
                }
            }
            Event::DocType(_) => {
                return Err(U1DirectError::Xml {
                    path: path.into(),
                    message: "DOCTYPE is forbidden".into(),
                });
            }
            Event::Eof => break,
            other => {
                if skip_depth.is_none() {
                    writer
                        .write_event(other.into_owned())
                        .map_err(|error| U1DirectError::Xml {
                            path: path.into(),
                            message: error.to_string(),
                        })?;
                }
            }
        }
        buffer.clear();
    }
    control.checkpoint()?;
    let expected_resources = resource_maps.keys().copied().collect::<BTreeSet<_>>();
    if found_resources != expected_resources {
        return Err(U1DirectError::Plan(format!(
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
            return Err(U1DirectError::Plan(format!(
                "root build membership differs from the canonical plan (expected {expected_instances:?}, found {found_instances:?})"
            )));
        }
        if !generated_metadata_written {
            return Err(U1DirectError::Xml {
                path: path.into(),
                message: "root model has no resources element".into(),
            });
        }
        if seen_external_paths != plan.external_paths {
            return Err(U1DirectError::Plan(format!(
                "rewritten root model external component paths {seen_external_paths:?} do not match the analyzed relationship closure {:?}",
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
) -> Result<Option<String>, U1DirectError> {
    let value = decoded_attributes(reader, event, path)?
        .into_iter()
        .find(|(key, _)| local_xml_name(key.as_bytes()) == b"path")
        .map(|(_, value)| value);
    let Some(value) = value else { return Ok(None) };
    let normalized = value.trim().trim_start_matches('/');
    if normalized.is_empty() {
        return Err(U1DirectError::Xml {
            path: path.into(),
            message: "external component path is empty".into(),
        });
    }
    Ok(Some(normalized.to_owned()))
}

fn write_generated_model_metadata<W: Write>(
    writer: &mut Writer<W>,
    title: Option<&str>,
) -> Result<(), U1DirectError> {
    // Snapmaker Orca 2.3.5 deliberately serializes the upstream-compatible
    // `BambuStudio-<version>` Application value in its own 3MF exporter. Keep
    // that exact dialect marker; the Snapmaker-specific adapter identity is
    // recorded separately below.
    let metadata = [
        ("Application", "BambuStudio-2.3.5"),
        ("BambuStudio:3mfVersion", "1"),
        ("U1Planner:Generator", "U1 3MF Color Planner"),
        ("U1Planner:Adapter", U1_DIRECT_ADAPTER_ID),
    ];
    for (name, value) in metadata {
        write_metadata_text(writer, name, value)?;
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
) -> Result<(), U1DirectError> {
    let mut start = BytesStart::new("metadata");
    start.push_attribute(("name", name));
    writer
        .write_event(Event::Start(start))
        .map_err(|error| U1DirectError::Xml {
            path: MAIN_MODEL_PATH.into(),
            message: error.to_string(),
        })?;
    writer
        .write_event(Event::Text(BytesText::new(value)))
        .map_err(|error| U1DirectError::Xml {
            path: MAIN_MODEL_PATH.into(),
            message: error.to_string(),
        })?;
    writer
        .write_event(Event::End(BytesEnd::new("metadata")))
        .map_err(|error| U1DirectError::Xml {
            path: MAIN_MODEL_PATH.into(),
            message: error.to_string(),
        })
}

fn rewrite_build_item<R: io::BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    path: &str,
    plan: &ArtifactBuildPlan,
    occurrences: &mut BTreeMap<u32, u32>,
    found: &mut BTreeSet<(u32, u32)>,
) -> Result<Option<BytesStart<'static>>, U1DirectError> {
    let attributes = decoded_attributes(reader, event, path)?;
    if attributes
        .iter()
        .any(|(key, _)| local_xml_name(key.as_bytes()) == b"path")
    {
        return Err(U1DirectError::Plan(
            "direct external build items are not qualified for Stage B".into(),
        ));
    }
    let object_id = attributes
        .iter()
        .find(|(key, _)| local_xml_name(key.as_bytes()) == b"objectid")
        .and_then(|(_, value)| value.parse::<u32>().ok())
        .ok_or_else(|| U1DirectError::Xml {
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
        "transform".to_owned(),
        translated_transform(original_transform, *placement, path)?,
    );
    Ok(Some(rebuild_start(event, attributes, &replacements)?))
}

fn rewrite_model_element<R: io::BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    path: &str,
    slot_map: Option<&BTreeMap<u8, u8>>,
) -> Result<BytesStart<'static>, U1DirectError> {
    let attributes = decoded_attributes(reader, event, path)?;
    let mut replacements = BTreeMap::new();
    if let Some(slot_map) = slot_map {
        for (key, value) in &attributes {
            let local = local_xml_name(key.as_bytes());
            if local == b"paint_color" || local == b"mmu_segmentation" {
                let mut tree =
                    decode_paint_annotation(value).map_err(|error| U1DirectError::Paint {
                        path: path.into(),
                        message: error.to_string(),
                    })?;
                tree.remap_states(|state| {
                    slot_map.get(&state).copied().ok_or_else(|| {
                        u1_three_mf::PaintCodecError::StateOutOfRange(u32::from(state))
                    })
                })
                .map_err(|error| U1DirectError::Paint {
                    path: path.into(),
                    message: format!("{error}; source state has no Direct mapping"),
                })?;
                replacements.insert(
                    key.clone(),
                    encode_paint_annotation(&tree).map_err(|error| U1DirectError::Paint {
                        path: path.into(),
                        message: error.to_string(),
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
) -> Result<Vec<(String, String)>, U1DirectError> {
    event
        .attributes()
        .with_checks(true)
        .map(|attribute| {
            let attribute = attribute.map_err(|error| U1DirectError::Xml {
                path: path.into(),
                message: error.to_string(),
            })?;
            let key = String::from_utf8(attribute.key.as_ref().to_vec()).map_err(|error| {
                U1DirectError::Xml {
                    path: path.into(),
                    message: error.to_string(),
                }
            })?;
            let value = attribute
                .decode_and_unescape_value(reader.decoder())
                .map_err(|error| U1DirectError::Xml {
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
) -> Result<BytesStart<'static>, U1DirectError> {
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
        String::from_utf8(event.name().as_ref().to_vec()).map_err(|error| U1DirectError::Xml {
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
) -> Result<u32, U1DirectError> {
    decoded_attributes(reader, event, path)?
        .into_iter()
        .find(|(candidate, _)| local_xml_name(candidate.as_bytes()) == key)
        .and_then(|(_, value)| value.parse().ok())
        .ok_or_else(|| U1DirectError::Xml {
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
) -> Result<String, U1DirectError> {
    let mut values = if let Some(original) = original {
        original
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    } else {
        ["1", "0", "0", "0", "1", "0", "0", "0", "1", "0", "0", "0"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    };
    if values.len() != 12 {
        return Err(U1DirectError::Xml {
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
            .map_err(|_| U1DirectError::Xml {
                path: path.into(),
                message: "build transform contains an invalid translation".into(),
            })?;
        let translated = value + delta;
        if !translated.is_finite() {
            return Err(U1DirectError::Xml {
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

fn begin_geometry_resource(path: &str, resource_id: u32, geometry: &mut GeometryCounts) {
    update_geometry_field(&mut geometry.fingerprint, b"resource");
    update_geometry_field(&mut geometry.fingerprint, path.as_bytes());
    update_geometry_field(&mut geometry.fingerprint, &resource_id.to_be_bytes());
}

fn record_geometry_element<R: io::BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    path: &str,
    geometry: &mut GeometryCounts,
) -> Result<(), U1DirectError> {
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
            return Err(U1DirectError::Xml {
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
            value
                .parse::<f64>()
                .is_ok_and(|coordinate| coordinate.is_finite())
        } else {
            value.parse::<u32>().is_ok()
        };
        if !valid {
            return Err(U1DirectError::Xml {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RetainedMetadataScope {
    Object,
    Part,
}

fn rewrite_model_settings(
    source_snapshot: &Path,
    plan: &ArtifactBuildPlan,
) -> Result<Vec<u8>, U1DirectError> {
    let file = File::open(source_snapshot).map_err(|source| U1DirectError::Read {
        path: source_snapshot.to_path_buf(),
        source,
    })?;
    let mut archive = ZipArchive::new(file)?;
    let entry = archive.by_name(MODEL_SETTINGS_PATH)?;
    if entry.size() > MAX_METADATA_BYTES {
        return Err(U1DirectError::Plan(
            "source model settings exceed the safe metadata limit".into(),
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
        .map_err(|error| U1DirectError::Xml {
            path: MODEL_SETTINGS_PATH.into(),
            message: error.to_string(),
        })?;
    writer
        .write_event(Event::Start(BytesStart::new("config")))
        .map_err(|error| U1DirectError::Xml {
            path: MODEL_SETTINGS_PATH.into(),
            message: error.to_string(),
        })?;
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    let mut skip_depth: Option<usize> = None;
    let mut current_object: Option<u32> = None;
    let mut current_part_depth: Option<usize> = None;
    let mut found_objects = BTreeSet::new();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| U1DirectError::Xml {
                path: MODEL_SETTINGS_PATH.into(),
                message: error.to_string(),
            })?;
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
                    return Err(U1DirectError::Xml {
                        path: MODEL_SETTINGS_PATH.into(),
                        message: format!(
                            "unsupported root-level model settings element {}",
                            String::from_utf8_lossy(event.name().as_ref())
                        ),
                    });
                }
                if name == b"metadata" {
                    return Err(U1DirectError::Xml {
                        path: MODEL_SETTINGS_PATH.into(),
                        message: "retained object metadata must be an empty XML element".into(),
                    });
                }
                if depth == 2 && name == b"part" {
                    if current_part_depth.replace(depth).is_some() {
                        return Err(U1DirectError::Xml {
                            path: MODEL_SETTINGS_PATH.into(),
                            message: "nested retained part metadata is unsupported".into(),
                        });
                    }
                } else if current_object.is_some() && depth >= 2 {
                    return Err(U1DirectError::Xml {
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
                    .map_err(|error| U1DirectError::Xml {
                        path: MODEL_SETTINGS_PATH.into(),
                        message: error.to_string(),
                    })?;
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
                        .map_err(|error| U1DirectError::Xml {
                            path: MODEL_SETTINGS_PATH.into(),
                            message: error.to_string(),
                        })?;
                    buffer.clear();
                    continue;
                }
                if depth == 1 {
                    return Err(U1DirectError::Xml {
                        path: MODEL_SETTINGS_PATH.into(),
                        message: format!(
                            "unsupported root-level model settings element {}",
                            String::from_utf8_lossy(event.name().as_ref())
                        ),
                    });
                }
                if depth >= 1 && current_object.is_some() {
                    let allowed_structural_element =
                        (depth == 2 && current_part_depth.is_none() && name == b"part")
                            || (depth == 3
                                && current_part_depth == Some(2)
                                && (name == b"mesh_stat" || name == b"text_info"));
                    if name != b"metadata" && !allowed_structural_element {
                        return Err(U1DirectError::Xml {
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
                                return Err(U1DirectError::Xml {
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
                    if name == b"text_info" {
                        // The selected part already carries the generated text
                        // mesh. text_info is editable-font state, not geometry;
                        // omit it so the normalized target does not depend on a
                        // source font installation or retain unqualified UI data.
                        buffer.clear();
                        continue;
                    }
                    let rewritten = rewrite_settings_element(
                        &reader,
                        &event,
                        current_object.and_then(|id| plan.object_slot_maps.get(&id)),
                        metadata_scope,
                    )?;
                    writer
                        .write_event(Event::Empty(rewritten))
                        .map_err(|error| U1DirectError::Xml {
                            path: MODEL_SETTINGS_PATH.into(),
                            message: error.to_string(),
                        })?;
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
                        .map_err(|error| U1DirectError::Xml {
                            path: MODEL_SETTINGS_PATH.into(),
                            message: error.to_string(),
                        })?;
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
                return Err(U1DirectError::Xml {
                    path: MODEL_SETTINGS_PATH.into(),
                    message: "DOCTYPE is forbidden".into(),
                });
            }
            Event::Eof => break,
            other => {
                if current_object.is_some() && skip_depth.is_none() {
                    writer
                        .write_event(other.into_owned())
                        .map_err(|error| U1DirectError::Xml {
                            path: MODEL_SETTINGS_PATH.into(),
                            message: error.to_string(),
                        })?;
                }
            }
        }
        buffer.clear();
    }
    let expected_objects = plan
        .object_slot_maps
        .keys()
        .copied()
        .collect::<BTreeSet<_>>();
    if found_objects != expected_objects {
        return Err(U1DirectError::Plan(format!(
            "model settings did not contain every selected object (expected {expected_objects:?}, found {found_objects:?})"
        )));
    }
    for plate in &plan.plates {
        writer
            .write_event(Event::Start(BytesStart::new("plate")))
            .map_err(settings_xml_error)?;
        write_settings_metadata(&mut writer, "plater_id", &plate.target_plate_id.to_string())?;
        write_settings_metadata(&mut writer, "plater_name", &plate.name)?;
        write_settings_metadata(&mut writer, "locked", "false")?;
        // Snapmaker Orca 2.3.5's own bbs_3mf exporter emits Auto For Flush
        // and one literal `1` per project filament here (the upstream code
        // labels this as an Orca compatibility hack). Derive the cardinality
        // from the exact T1–T4 target instead of carrying source metadata.
        write_settings_metadata(&mut writer, "filament_map_mode", TARGET_FILAMENT_MAP_MODE)?;
        write_settings_metadata(
            &mut writer,
            "filament_maps",
            &target_filament_maps(plan.physical_slots.len()),
        )?;
        for unit in &plate.units {
            writer
                .write_event(Event::Start(BytesStart::new("model_instance")))
                .map_err(settings_xml_error)?;
            write_settings_metadata(&mut writer, "object_id", &unit.source_object_id.to_string())?;
            write_settings_metadata(
                &mut writer,
                "instance_id",
                &unit.source_instance_id.to_string(),
            )?;
            let identify_id = plate
                .source_identify_ids
                .get(&(unit.source_object_id, unit.source_instance_id))
                .ok_or_else(|| {
                    U1DirectError::Plan(format!(
                        "source identify_id for object {}/instance {} is missing",
                        unit.source_object_id, unit.source_instance_id
                    ))
                })?;
            write_settings_metadata(&mut writer, "identify_id", &identify_id.to_string())?;
            writer
                .write_event(Event::End(BytesEnd::new("model_instance")))
                .map_err(settings_xml_error)?;
        }
        writer
            .write_event(Event::End(BytesEnd::new("plate")))
            .map_err(settings_xml_error)?;
    }
    writer
        .write_event(Event::End(BytesEnd::new("config")))
        .map_err(settings_xml_error)?;
    Ok(writer.into_inner())
}

fn rewrite_settings_element<R: io::BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    slot_map: Option<&BTreeMap<u8, u8>>,
    metadata_scope: Option<RetainedMetadataScope>,
) -> Result<BytesStart<'static>, U1DirectError> {
    let attributes = decoded_attributes(reader, event, MODEL_SETTINGS_PATH)?;
    let mut replacements = BTreeMap::new();
    if local_xml_name(event.name().as_ref()) == b"metadata" {
        let metadata_scope = metadata_scope.ok_or_else(|| U1DirectError::Xml {
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
            return Err(U1DirectError::Xml {
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
                .ok_or_else(|| U1DirectError::Xml {
                    path: MODEL_SETTINGS_PATH.into(),
                    message: "color metadata has no valid slot value".into(),
                })?;
            if value != 0 {
                let mapped = slot_map
                    .and_then(|map| map.get(&value))
                    .copied()
                    .ok_or_else(|| {
                        U1DirectError::Plan(format!(
                            "model settings slot F{value} has no Direct mapping"
                        ))
                    })?;
                replacements.insert("value".into(), mapped.to_string());
            }
        } else if let Some(key @ ("brim_width" | "brim_object_gap")) = key {
            if metadata_scope != RetainedMetadataScope::Object {
                return Err(U1DirectError::Plan(format!(
                    "object footprint override {key:?} is not qualified inside a part"
                )));
            }
            let value = attributes
                .iter()
                .find(|(key, _)| local_xml_name(key.as_bytes()) == b"value")
                .map(|(_, value)| value.as_str())
                .ok_or_else(|| U1DirectError::Xml {
                    path: MODEL_SETTINGS_PATH.into(),
                    message: format!("object footprint override {key:?} has no value"),
                })?;
            validate_object_footprint_override(key, value)?;
        } else if let Some(key) = key {
            let value = attributes
                .iter()
                .find(|(key, _)| local_xml_name(key.as_bytes()) == b"value")
                .map(|(_, value)| value.as_str())
                .ok_or_else(|| U1DirectError::Xml {
                    path: MODEL_SETTINGS_PATH.into(),
                    message: format!("retained object metadata {key:?} has no value"),
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
) -> Result<(), U1DirectError> {
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
                value.parse::<f64>().is_ok_and(|value| value.is_finite())
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
    Err(U1DirectError::Plan(format!(
        "retained {scope:?} metadata {key:?}={value:?} is outside the qualified Stage B allowlist; remove the per-object process override or qualify its footprint effect"
    )))
}

fn validate_unkeyed_object_metadata(
    scope: RetainedMetadataScope,
    attributes: &[(String, String)],
) -> Result<(), U1DirectError> {
    if scope == RetainedMetadataScope::Object
        && attributes.len() == 1
        && attributes[0].0 == "face_count"
        && attributes[0].1.parse::<u64>().is_ok()
    {
        return Ok(());
    }
    Err(U1DirectError::Plan(
        "unkeyed retained object metadata must contain only a numeric face_count".into(),
    ))
}

fn validate_object_footprint_override(key: &str, value: &str) -> Result<(), U1DirectError> {
    let value = value.parse::<f64>().map_err(|_| {
        U1DirectError::Plan(format!(
            "object footprint override {key:?} has non-numeric value {value:?}"
        ))
    })?;
    let maximum = match key {
        "brim_width" => MAX_OBJECT_BRIM_WIDTH_MM,
        "brim_object_gap" => MAX_OBJECT_BRIM_GAP_MM,
        _ => {
            return Err(U1DirectError::Plan(format!(
                "unsupported object footprint override {key:?}"
            )));
        }
    };
    if !value.is_finite() || value < 0.0 || value > maximum {
        return Err(U1DirectError::Plan(format!(
            "object footprint override {key:?}={value} mm exceeds the qualified {maximum} mm limit covered by the {OBJECT_FOOTPRINT_CLEARANCE_MM} mm prime-tower object clearance"
        )));
    }
    Ok(())
}

fn target_filament_maps(filament_count: usize) -> String {
    std::iter::repeat_n("1", filament_count)
        .collect::<Vec<_>>()
        .join(" ")
}

fn write_settings_metadata<W: Write>(
    writer: &mut Writer<W>,
    key: &str,
    value: &str,
) -> Result<(), U1DirectError> {
    let mut metadata = BytesStart::new("metadata");
    metadata.push_attribute(("key", key));
    metadata.push_attribute(("value", value));
    writer
        .write_event(Event::Empty(metadata))
        .map_err(settings_xml_error)
}

fn settings_xml_error(error: io::Error) -> U1DirectError {
    U1DirectError::Xml {
        path: MODEL_SETTINGS_PATH.into(),
        message: error.to_string(),
    }
}

fn validate_u1_direct_candidate(
    path: &Path,
    plan: &ArtifactBuildPlan,
    expected_geometry: &GeometryCounts,
    source_analysis: &ProjectAnalysis,
) -> Result<(), U1DirectError> {
    let file = File::open(path).map_err(|source| U1DirectError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let mut archive = ZipArchive::new(file)?;
    if let Some(entry) = archive
        .file_names()
        .find(|name| is_embedded_preset_entry(name))
        .map(str::to_owned)
    {
        return Err(U1DirectError::SemanticValidation(format!(
            "generated project unexpectedly embeds preset {entry:?}; Stage B uses only exact hash-pinned Snapmaker Orca 2.3.5 system profiles"
        )));
    }
    let mut project_entry = archive.by_name(PROJECT_SETTINGS_PATH)?;
    if project_entry.size() > MAX_METADATA_BYTES {
        return Err(U1DirectError::SemanticValidation(
            "generated project settings exceed the adapter limit".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(project_entry.size() as usize);
    project_entry
        .read_to_end(&mut bytes)
        .map_err(|source| U1DirectError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    drop(project_entry);
    let settings_map: BTreeMap<String, Value> =
        serde_json::from_slice(&bytes).map_err(|source| U1DirectError::Json {
            path: PROJECT_SETTINGS_PATH.into(),
            source,
        })?;
    validate_prime_tower_profile_contract(&settings_map).map_err(|error| {
        U1DirectError::SemanticValidation(format!(
            "generated prime-tower settings failed validation: {error}"
        ))
    })?;
    validate_target_project_contract(&settings_map).map_err(|error| {
        U1DirectError::SemanticValidation(format!(
            "generated target defaults failed validation: {error}"
        ))
    })?;
    let settings = Value::Object(settings_map.into_iter().collect());
    for key in [
        "nozzle_diameter",
        "filament_settings_id",
        "filament_ids",
        "filament_colour",
        "filament_type",
    ] {
        require_array_len(&settings, key, 4)?;
    }
    require_array_len(&settings, "flush_volumes_matrix", 16)?;
    require_array_len(&settings, "flush_volumes_vector", 8)?;
    require_array_len(&settings, "wipe_tower_x", plan.plates.len())?;
    require_array_len(&settings, "wipe_tower_y", plan.plates.len())?;
    require_string_array(
        &settings,
        "filament_settings_id",
        plan.physical_slots.iter().map(|slot| slot.name.as_str()),
    )?;
    require_string_array(
        &settings,
        "filament_ids",
        plan.physical_slots
            .iter()
            .map(|slot| slot.setting_id.as_str()),
    )?;
    require_string_array(
        &settings,
        "filament_colour",
        plan.physical_slots.iter().map(|slot| slot.color.as_str()),
    )?;
    require_string_array(
        &settings,
        "filament_type",
        plan.physical_slots
            .iter()
            .map(|slot| slot.material.as_str()),
    )?;
    require_string_array(
        &settings,
        "wipe_tower_x",
        plan.plates
            .iter()
            .map(|plate| format_f64(plate.wipe_tower_x)),
    )?;
    require_string_array(
        &settings,
        "wipe_tower_y",
        plan.plates
            .iter()
            .map(|plate| format_f64(plate.wipe_tower_y)),
    )?;
    if settings.get("printer_model").and_then(Value::as_str) != Some("Snapmaker U1")
        || settings.get("printer_variant").and_then(Value::as_str) != Some("0.4")
        || settings.get("printer_settings_id").and_then(Value::as_str)
            != Some(U1_DIRECT_MACHINE_PROFILE)
        || settings.get("print_settings_id").and_then(Value::as_str)
            != Some(U1_DIRECT_PROCESS_PROFILE)
    {
        return Err(U1DirectError::SemanticValidation(
            "generated project settings do not identify the exact U1 Direct adapter".into(),
        ));
    }
    drop(archive);
    validate_generated_plate_filament_maps(path, plan)?;
    let mut model_paths = BTreeSet::from([MAIN_MODEL_PATH.to_owned()]);
    model_paths.extend(plan.external_paths.iter().cloned());
    let output_geometry = fingerprint_generated_geometry(path, &model_paths)?;
    let expected_geometry_sha256 = expected_geometry.sha256();
    let output_geometry_sha256 = output_geometry.sha256();
    if output_geometry.vertices != expected_geometry.vertices
        || output_geometry.triangles != expected_geometry.triangles
        || output_geometry_sha256 != expected_geometry_sha256
    {
        return Err(U1DirectError::SemanticValidation(format!(
            "canonical geometry changed during serialization: expected {}/{} vertices/triangles with SHA-256 {}, found {}/{} with SHA-256 {}",
            expected_geometry.vertices,
            expected_geometry.triangles,
            expected_geometry_sha256,
            output_geometry.vertices,
            output_geometry.triangles,
            output_geometry_sha256
        )));
    }
    let output = analyze_project(path).map_err(|error| {
        U1DirectError::SemanticValidation(format!(
            "generated output cannot be re-analyzed: {error}"
        ))
    })?;
    if output.printer.model.as_deref() != Some("Snapmaker U1")
        || output.printer.variant.as_deref() != Some("0.4")
        || output.printer.nozzle_diameters_mm.len() != 4
        || output
            .printer
            .nozzle_diameters_mm
            .iter()
            .any(|diameter| (*diameter - 0.4).abs() > f64::EPSILON)
    {
        return Err(U1DirectError::SemanticValidation(
            "re-analyzed output does not expose Snapmaker U1 with four 0.4 mm nozzles".into(),
        ));
    }
    if output.summary.vertex_count != expected_geometry.vertices
        || output.summary.triangle_count != expected_geometry.triangles
    {
        return Err(U1DirectError::SemanticValidation(format!(
            "geometry count changed during serialization: expected {}/{} vertices/triangles, found {}/{}",
            expected_geometry.vertices,
            expected_geometry.triangles,
            output.summary.vertex_count,
            output.summary.triangle_count
        )));
    }
    let expected_instances = plan
        .plates
        .iter()
        .map(|plate| plate.units.len())
        .sum::<usize>();
    if output.summary.instance_count != expected_instances
        || output.summary.plate_count != plan.plates.len()
    {
        return Err(U1DirectError::SemanticValidation(format!(
            "plate membership changed during serialization: expected {} plate(s)/{} instance(s), found {}/{}",
            plan.plates.len(),
            expected_instances,
            output.summary.plate_count,
            output.summary.instance_count
        )));
    }
    validate_exact_plate_membership_and_transforms(source_analysis, &output, plan)?;
    for plate in &output.plates {
        let bounds = plate.printable_bounds.ok_or_else(|| {
            U1DirectError::SemanticValidation(format!(
                "generated plate {} has unknown printable bounds",
                plate.id
            ))
        })?;
        let planned_plate = plan
            .plates
            .iter()
            .find(|planned| planned.target_plate_id == plate.id)
            .ok_or_else(|| {
                U1DirectError::SemanticValidation(format!(
                    "generated plate {} has no planned target plate",
                    plate.id
                ))
            })?;
        let target_index = usize::try_from(planned_plate.target_plate_id.saturating_sub(1))
            .map_err(|_| {
                U1DirectError::SemanticValidation(
                    "generated target plate index exceeds this platform".into(),
                )
            })?;
        let origin = virtual_plate_origin(target_index, output.plates.len(), TARGET_BED_SIZE_MM);
        let local_min_x = bounds.min[0] - origin.0;
        let local_max_x = bounds.max[0] - origin.0;
        let local_min_y = bounds.min[1] - origin.1;
        let local_max_y = bounds.max[1] - origin.1;
        if ![
            local_min_x,
            local_max_x,
            local_min_y,
            local_max_y,
            bounds.min[2],
            bounds.max[2],
        ]
        .into_iter()
        .all(f64::is_finite)
            || local_min_x < TARGET_MIN_X_MM - TARGET_BOUNDS_TOLERANCE_MM
            || local_min_y < TARGET_MIN_Y_MM - TARGET_BOUNDS_TOLERANCE_MM
            || bounds.max[2] <= 0.0
            || local_max_x > TARGET_MAX_X_MM + TARGET_BOUNDS_TOLERANCE_MM
            || local_max_y > TARGET_MAX_Y_MM + TARGET_BOUNDS_TOLERANCE_MM
            || bounds.max[2] > TARGET_PRINTABLE_HEIGHT_MM + TARGET_BOUNDS_TOLERANCE_MM
        {
            return Err(U1DirectError::SemanticValidation(format!(
                "generated plate {} is outside the U1 build volume",
                plate.id
            )));
        }
        if plate
            .effective_slots
            .iter()
            .any(|slot| !(1..=4).contains(slot))
        {
            return Err(U1DirectError::SemanticValidation(format!(
                "generated plate {} references a logical filament outside T1–T4",
                plate.id
            )));
        }
    }
    if output
        .objects
        .iter()
        .flat_map(|object| object.effective_slots.iter())
        .any(|slot| !(1..=4).contains(slot))
    {
        return Err(U1DirectError::SemanticValidation(
            "generated object or paint data references a logical filament outside T1–T4".into(),
        ));
    }
    validate_exact_slot_remap(source_analysis, &output, plan)?;
    Ok(())
}

#[derive(Default)]
struct GeneratedPlateFilamentMapContract {
    plate_id: Option<u32>,
    mode: Option<String>,
    maps: Option<String>,
}

fn validate_generated_plate_filament_maps(
    package_path: &Path,
    plan: &ArtifactBuildPlan,
) -> Result<(), U1DirectError> {
    let file = File::open(package_path).map_err(|source| U1DirectError::Read {
        path: package_path.to_path_buf(),
        source,
    })?;
    let mut archive = ZipArchive::new(file)?;
    let entry = archive.by_name(MODEL_SETTINGS_PATH)?;
    if entry.size() > MAX_METADATA_BYTES {
        return Err(U1DirectError::SemanticValidation(
            "generated model settings exceed the adapter limit".into(),
        ));
    }
    let mut reader = Reader::from_reader(BufReader::new(entry));
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    let mut current: Option<GeneratedPlateFilamentMapContract> = None;
    let mut plate_ids = BTreeSet::new();
    let expected_maps = target_filament_maps(plan.physical_slots.len());
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| U1DirectError::Xml {
                path: MODEL_SETTINGS_PATH.into(),
                message: error.to_string(),
            })?;
        match event {
            Event::Start(event) => {
                let event_name = event.name();
                let name = local_xml_name(event_name.as_ref());
                if depth == 1
                    && name == b"plate"
                    && current
                        .replace(GeneratedPlateFilamentMapContract::default())
                        .is_some()
                {
                    return Err(U1DirectError::SemanticValidation(
                        "generated model settings contain nested plate blocks".into(),
                    ));
                }
                depth += 1;
            }
            Event::Empty(event) => {
                let event_name = event.name();
                if depth != 2
                    || current.is_none()
                    || local_xml_name(event_name.as_ref()) != b"metadata"
                {
                    buffer.clear();
                    continue;
                }
                let attributes = decoded_attributes(&reader, &event, MODEL_SETTINGS_PATH)?;
                let key = attributes
                    .iter()
                    .find(|(key, _)| local_xml_name(key.as_bytes()) == b"key")
                    .map(|(_, value)| value.as_str());
                let value = attributes
                    .iter()
                    .find(|(key, _)| local_xml_name(key.as_bytes()) == b"value")
                    .map(|(_, value)| value.as_str());
                let Some(contract) = current.as_mut() else {
                    unreachable!("current plate was checked above")
                };
                match key {
                    Some("plater_id") => {
                        let plate_id = value
                            .and_then(|value| value.parse::<u32>().ok())
                            .ok_or_else(|| {
                                U1DirectError::SemanticValidation(
                                    "generated plate has no valid plater_id".into(),
                                )
                            })?;
                        if contract.plate_id.replace(plate_id).is_some() {
                            return Err(U1DirectError::SemanticValidation(
                                "generated plate contains duplicate plater_id metadata".into(),
                            ));
                        }
                    }
                    Some("filament_map_mode") => {
                        let value = value.ok_or_else(|| {
                            U1DirectError::SemanticValidation(
                                "generated filament_map_mode has no value".into(),
                            )
                        })?;
                        if contract.mode.replace(value.to_owned()).is_some() {
                            return Err(U1DirectError::SemanticValidation(
                                "generated plate contains duplicate filament_map_mode metadata"
                                    .into(),
                            ));
                        }
                    }
                    Some("filament_maps") => {
                        let value = value.ok_or_else(|| {
                            U1DirectError::SemanticValidation(
                                "generated filament_maps has no value".into(),
                            )
                        })?;
                        if contract.maps.replace(value.to_owned()).is_some() {
                            return Err(U1DirectError::SemanticValidation(
                                "generated plate contains duplicate filament_maps metadata".into(),
                            ));
                        }
                    }
                    _ => {}
                }
            }
            Event::End(event) => {
                let event_name = event.name();
                let closes_plate =
                    depth.saturating_sub(1) == 1 && local_xml_name(event_name.as_ref()) == b"plate";
                depth = depth.saturating_sub(1);
                if closes_plate {
                    let contract = current.take().ok_or_else(|| {
                        U1DirectError::SemanticValidation(
                            "generated model settings closed an unopened plate".into(),
                        )
                    })?;
                    let plate_id = contract.plate_id.ok_or_else(|| {
                        U1DirectError::SemanticValidation(
                            "generated plate is missing plater_id metadata".into(),
                        )
                    })?;
                    if !plate_ids.insert(plate_id) {
                        return Err(U1DirectError::SemanticValidation(format!(
                            "generated model settings contain duplicate plate {plate_id}"
                        )));
                    }
                    if contract.mode.as_deref() != Some(TARGET_FILAMENT_MAP_MODE)
                        || contract.maps.as_deref() != Some(expected_maps.as_str())
                    {
                        return Err(U1DirectError::SemanticValidation(format!(
                            "generated plate {plate_id} does not use the exact four-filament Orca Auto For Flush mapping"
                        )));
                    }
                }
            }
            Event::DocType(_) => {
                return Err(U1DirectError::Xml {
                    path: MODEL_SETTINGS_PATH.into(),
                    message: "DOCTYPE is forbidden".into(),
                });
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if current.is_some() {
        return Err(U1DirectError::SemanticValidation(
            "generated model settings contain an unclosed plate block".into(),
        ));
    }
    let expected_plate_ids = plan
        .plates
        .iter()
        .map(|plate| plate.target_plate_id)
        .collect::<BTreeSet<_>>();
    if plate_ids != expected_plate_ids {
        return Err(U1DirectError::SemanticValidation(format!(
            "generated filament-map plate IDs {plate_ids:?} do not match planned plate IDs {expected_plate_ids:?}"
        )));
    }
    Ok(())
}

fn is_embedded_preset_entry(path: &str) -> bool {
    [
        "Metadata/process_settings_",
        "Metadata/filament_settings_",
        "Metadata/machine_settings_",
    ]
    .into_iter()
    .any(|prefix| path.starts_with(prefix))
}

fn fingerprint_generated_geometry(
    package_path: &Path,
    model_paths: &BTreeSet<String>,
) -> Result<GeometryCounts, U1DirectError> {
    let file = File::open(package_path).map_err(|source| U1DirectError::Read {
        path: package_path.to_path_buf(),
        source,
    })?;
    let mut archive = ZipArchive::new(file)?;
    let mut geometry = GeometryCounts::default();
    for path in model_paths {
        let entry = archive.by_name(path)?;
        let mut reader = Reader::from_reader(BufReader::new(entry));
        reader.config_mut().trim_text(false);
        let mut buffer = Vec::new();
        let mut depth = 0_usize;
        let mut current_resource = None;
        let mut found_resources = BTreeSet::new();
        loop {
            let event =
                reader
                    .read_event_into(&mut buffer)
                    .map_err(|error| U1DirectError::Xml {
                        path: path.clone(),
                        message: error.to_string(),
                    })?;
            match event {
                Event::Start(event) => {
                    let event_name = event.name();
                    let name = local_xml_name(event_name.as_ref());
                    if depth == 2 && name == b"object" {
                        let id = required_u32_attribute(&reader, &event, b"id", path)?;
                        if !found_resources.insert(id) {
                            return Err(U1DirectError::SemanticValidation(format!(
                                "generated model part {path} contains duplicate resource {id}"
                            )));
                        }
                        current_resource = Some(id);
                        begin_geometry_resource(path, id, &mut geometry);
                    }
                    if (name == b"vertex" || name == b"triangle") && current_resource.is_none() {
                        return Err(U1DirectError::SemanticValidation(format!(
                            "generated model part {path} contains geometry outside an object resource"
                        )));
                    }
                    if current_resource.is_some() {
                        record_geometry_element(&reader, &event, path, &mut geometry)?;
                    }
                    depth += 1;
                }
                Event::Empty(event) => {
                    let event_name = event.name();
                    let name = local_xml_name(event_name.as_ref());
                    if depth == 2 && name == b"object" {
                        let id = required_u32_attribute(&reader, &event, b"id", path)?;
                        if !found_resources.insert(id) {
                            return Err(U1DirectError::SemanticValidation(format!(
                                "generated model part {path} contains duplicate resource {id}"
                            )));
                        }
                        begin_geometry_resource(path, id, &mut geometry);
                    } else if name == b"vertex" || name == b"triangle" {
                        if current_resource.is_none() {
                            return Err(U1DirectError::SemanticValidation(format!(
                                "generated model part {path} contains geometry outside an object resource"
                            )));
                        }
                        record_geometry_element(&reader, &event, path, &mut geometry)?;
                    }
                }
                Event::End(event) => {
                    let event_name = event.name();
                    let closes_object = depth.saturating_sub(1) == 2
                        && local_xml_name(event_name.as_ref()) == b"object";
                    depth = depth.saturating_sub(1);
                    if closes_object {
                        current_resource = None;
                    }
                }
                Event::DocType(_) => {
                    return Err(U1DirectError::Xml {
                        path: path.clone(),
                        message: "DOCTYPE is forbidden".into(),
                    });
                }
                Event::Eof => break,
                _ => {}
            }
            buffer.clear();
        }
    }
    Ok(geometry)
}

fn validate_exact_plate_membership_and_transforms(
    source: &ProjectAnalysis,
    output: &ProjectAnalysis,
    plan: &ArtifactBuildPlan,
) -> Result<(), U1DirectError> {
    const TRANSFORM_TOLERANCE: f64 = 1.0e-9;
    const BOUNDS_TOLERANCE: f64 = 1.0e-8;

    let output_plate_ids = output
        .plates
        .iter()
        .map(|plate| plate.id)
        .collect::<BTreeSet<_>>();
    let planned_plate_ids = plan
        .plates
        .iter()
        .map(|plate| plate.target_plate_id)
        .collect::<BTreeSet<_>>();
    if output_plate_ids.len() != output.plates.len()
        || planned_plate_ids.len() != plan.plates.len()
        || output_plate_ids != planned_plate_ids
    {
        return Err(U1DirectError::SemanticValidation(format!(
            "generated plate IDs {output_plate_ids:?} do not match planned plate IDs {planned_plate_ids:?}"
        )));
    }

    for planned_plate in &plan.plates {
        let declared_source_plate_ids = planned_plate
            .source_plate_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let unit_source_plate_ids = canonical_source_plate_ids(&planned_plate.units)?
            .into_iter()
            .collect::<BTreeSet<_>>();
        if declared_source_plate_ids.len() != planned_plate.source_plate_ids.len()
            || declared_source_plate_ids != unit_source_plate_ids
        {
            return Err(U1DirectError::SemanticValidation(format!(
                "target plate {} source provenance does not exactly match its units",
                planned_plate.target_plate_id
            )));
        }
        let output_plate = output
            .plates
            .iter()
            .find(|plate| plate.id == planned_plate.target_plate_id)
            .ok_or_else(|| {
                U1DirectError::SemanticValidation(format!(
                    "generated target plate {} is missing",
                    planned_plate.target_plate_id
                ))
            })?;
        if output_plate.instances.len() != planned_plate.units.len()
            || output_plate
                .instances
                .iter()
                .any(|instance| !instance.printable)
        {
            return Err(U1DirectError::SemanticValidation(format!(
                "generated target plate {} does not contain exactly {} printable instances",
                planned_plate.target_plate_id,
                planned_plate.units.len()
            )));
        }

        let mut matched = vec![false; output_plate.instances.len()];
        for unit in &planned_plate.units {
            let source_plate_id = parse_source_plate_id(unit.source_plate_id.as_deref())?;
            let source_plate = source
                .plates
                .iter()
                .find(|plate| plate.id == source_plate_id)
                .ok_or_else(|| {
                    U1DirectError::SemanticValidation(format!(
                        "source plate {source_plate_id} is unavailable during exact membership validation"
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
                    U1DirectError::SemanticValidation(format!(
                        "source instance {}/{} for {} is unavailable",
                        unit.source_object_id, unit.source_instance_id, unit.source_unit_id
                    ))
                })?;
            let placement = plan
                .selected_instances
                .get(&(unit.source_object_id, unit.source_instance_id))
                .copied()
                .ok_or_else(|| {
                    U1DirectError::SemanticValidation(format!(
                        "planned placement for source instance {}/{} is missing",
                        unit.source_object_id, unit.source_instance_id
                    ))
                })?;
            let mut expected_transform = source_instance
                .transform
                .unwrap_or(u1_three_mf::Transform3mf::IDENTITY)
                .values;
            expected_transform[9] += placement.delta_x;
            expected_transform[10] += placement.delta_y;
            expected_transform[11] += placement.delta_z;
            let expected_bounds = source_instance.printable_bounds.map(|mut bounds| {
                for point in [&mut bounds.min, &mut bounds.max] {
                    point[0] += placement.delta_x;
                    point[1] += placement.delta_y;
                    point[2] += placement.delta_z;
                }
                bounds
            });

            let candidate = output_plate
                .instances
                .iter()
                .enumerate()
                .find(|(index, instance)| {
                    !matched[*index]
                        && instance.object_id == unit.source_object_id
                        && instance.identify_id == source_instance.identify_id
                        && transform_values_close(
                            instance
                                .transform
                                .unwrap_or(u1_three_mf::Transform3mf::IDENTITY)
                                .values,
                            expected_transform,
                            TRANSFORM_TOLERANCE,
                        )
                        && optional_bounds_close(
                            instance.printable_bounds,
                            expected_bounds,
                            BOUNDS_TOLERANCE,
                        )
                })
                .map(|(index, _)| index)
                .ok_or_else(|| {
                    U1DirectError::SemanticValidation(format!(
                        "generated target plate {} changed the plate membership, identify_id, transform, or bounds of source instance {}/{}",
                        planned_plate.target_plate_id,
                        unit.source_object_id,
                        unit.source_instance_id
                    ))
                })?;
            matched[candidate] = true;
        }
        if matched.iter().any(|value| !value) {
            return Err(U1DirectError::SemanticValidation(format!(
                "generated target plate {} contains an unplanned instance",
                planned_plate.target_plate_id
            )));
        }
    }
    Ok(())
}

fn transform_values_close(first: [f64; 12], second: [f64; 12], tolerance: f64) -> bool {
    first
        .into_iter()
        .zip(second)
        .all(|(first, second)| (first - second).abs() <= tolerance)
}

fn optional_bounds_close(
    first: Option<u1_three_mf::AxisAlignedBounds>,
    second: Option<u1_three_mf::AxisAlignedBounds>,
    tolerance: f64,
) -> bool {
    match (first, second) {
        (Some(first), Some(second)) => [first.min, first.max]
            .into_iter()
            .flatten()
            .zip([second.min, second.max].into_iter().flatten())
            .all(|(first, second)| (first - second).abs() <= tolerance),
        (None, None) => true,
        _ => false,
    }
}

fn validate_exact_slot_remap(
    source: &ProjectAnalysis,
    output: &ProjectAnalysis,
    plan: &ArtifactBuildPlan,
) -> Result<(), U1DirectError> {
    let source_objects = source
        .objects
        .iter()
        .filter(|object| object.source_model_path.is_none())
        .map(|object| (object.source_object_id.unwrap_or(object.id), object))
        .collect::<BTreeMap<_, _>>();
    let output_objects = output
        .objects
        .iter()
        .filter(|object| object.source_model_path.is_none())
        .map(|object| (object.source_object_id.unwrap_or(object.id), object))
        .collect::<BTreeMap<_, _>>();
    for (object_id, mapping) in &plan.object_slot_maps {
        let source_object = source_objects.get(object_id).copied().ok_or_else(|| {
            U1DirectError::SemanticValidation(format!(
                "source object {object_id} disappeared before remap validation"
            ))
        })?;
        let output_object = output_objects.get(object_id).copied().ok_or_else(|| {
            U1DirectError::SemanticValidation(format!("generated object {object_id} is missing"))
        })?;
        let expected_slots = mapped_slot_set(&source_object.effective_slots, mapping)?;
        let actual_slots = output_object
            .effective_slots
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        if actual_slots != expected_slots {
            return Err(U1DirectError::SemanticValidation(format!(
                "generated object {object_id} uses slots {actual_slots:?}; expected exact Direct remap {expected_slots:?}"
            )));
        }
        let expected_object_extruder =
            mapped_optional_slot(source_object.object_extruder_slot, mapping)?;
        if output_object.object_extruder_slot != expected_object_extruder {
            return Err(U1DirectError::SemanticValidation(format!(
                "generated object {object_id} has object extruder {:?}; expected {:?}",
                output_object.object_extruder_slot, expected_object_extruder
            )));
        }
        let output_parts = output_object
            .parts
            .iter()
            .map(|part| {
                (
                    (
                        part.component_path.as_deref().unwrap_or(MAIN_MODEL_PATH),
                        part.id,
                    ),
                    part,
                )
            })
            .collect::<BTreeMap<_, _>>();
        for source_part in &source_object.parts {
            let key = (
                source_part
                    .component_path
                    .as_deref()
                    .unwrap_or(MAIN_MODEL_PATH),
                source_part.id,
            );
            let output_part = output_parts.get(&key).copied().ok_or_else(|| {
                U1DirectError::SemanticValidation(format!(
                    "generated object {object_id} is missing part {}/{}",
                    key.0, key.1
                ))
            })?;
            let expected_part_slots = mapped_slot_set(&source_part.effective_slots, mapping)?;
            let actual_part_slots = output_part
                .effective_slots
                .iter()
                .copied()
                .collect::<BTreeSet<_>>();
            if actual_part_slots != expected_part_slots {
                return Err(U1DirectError::SemanticValidation(format!(
                    "generated part {}/{} uses slots {actual_part_slots:?}; expected {expected_part_slots:?}",
                    key.0, key.1
                )));
            }
            let expected_extruder = mapped_optional_slot(source_part.extruder_slot, mapping)?;
            let expected_inherited =
                mapped_optional_slot(source_part.inherited_extruder_slot, mapping)?;
            if output_part.extruder_slot != expected_extruder
                || output_part.inherited_extruder_slot != expected_inherited
            {
                return Err(U1DirectError::SemanticValidation(format!(
                    "generated part {}/{} changed its exact extruder mapping",
                    key.0, key.1
                )));
            }
        }
    }
    Ok(())
}

fn mapped_optional_slot(
    source: Option<u16>,
    mapping: &BTreeMap<u8, u8>,
) -> Result<Option<u16>, U1DirectError> {
    source.map(|slot| mapped_slot(slot, mapping)).transpose()
}

fn mapped_slot_set(
    source: &[u16],
    mapping: &BTreeMap<u8, u8>,
) -> Result<BTreeSet<u16>, U1DirectError> {
    source
        .iter()
        .copied()
        .map(|slot| mapped_slot(slot, mapping))
        .collect()
}

fn mapped_slot(slot: u16, mapping: &BTreeMap<u8, u8>) -> Result<u16, U1DirectError> {
    if slot == 0 {
        return Ok(0);
    }
    let source = u8::try_from(slot).map_err(|_| {
        U1DirectError::SemanticValidation(format!(
            "source slot F{slot} is outside the supported paint range"
        ))
    })?;
    mapping.get(&source).copied().map(u16::from).ok_or_else(|| {
        U1DirectError::SemanticValidation(format!(
            "source slot F{slot} has no expected Direct mapping"
        ))
    })
}

fn require_array_len(settings: &Value, key: &str, expected: usize) -> Result<(), U1DirectError> {
    let actual = settings
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::len)
        .ok_or_else(|| {
            U1DirectError::SemanticValidation(format!(
                "project setting {key:?} is missing or is not an array"
            ))
        })?;
    if actual != expected {
        return Err(U1DirectError::SemanticValidation(format!(
            "project setting {key:?} has {actual} values; the U1 adapter requires {expected}"
        )));
    }
    Ok(())
}

fn require_string_array(
    settings: &Value,
    key: &str,
    expected: impl IntoIterator<Item = impl AsRef<str>>,
) -> Result<(), U1DirectError> {
    let expected = expected
        .into_iter()
        .map(|value| value.as_ref().to_owned())
        .collect::<Vec<_>>();
    let actual = settings
        .get(key)
        .and_then(Value::as_array)
        .and_then(|values| values.iter().map(Value::as_str).collect::<Option<Vec<_>>>())
        .ok_or_else(|| {
            U1DirectError::SemanticValidation(format!(
                "project setting {key:?} is missing or contains non-string values"
            ))
        })?;
    if actual != expected.iter().map(String::as_str).collect::<Vec<_>>() {
        return Err(U1DirectError::SemanticValidation(format!(
            "project setting {key:?} differs from the planned physical T1–T4 order"
        )));
    }
    Ok(())
}

fn hash_path(path: &Path) -> Result<(u64, String), U1DirectError> {
    hash_path_with_control(path, None)
}

fn hash_path_cancellable(
    path: &Path,
    control: &U1DirectConversionControl,
) -> Result<(u64, String), U1DirectError> {
    hash_path_with_control(path, Some(control))
}

fn hash_path_with_control(
    path: &Path,
    control: Option<&U1DirectConversionControl>,
) -> Result<(u64, String), U1DirectError> {
    let mut file = File::open(path).map_err(|source| U1DirectError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    hash_reader_with_control(&mut file, control).map_err(|error| match error {
        U1DirectError::Read { source, .. } => U1DirectError::Read {
            path: path.to_path_buf(),
            source,
        },
        other => other,
    })
}

fn hash_reader_cancellable(
    file: &mut File,
    control: &U1DirectConversionControl,
) -> Result<(u64, String), U1DirectError> {
    hash_reader_with_control(file, Some(control))
}

fn hash_reader_with_control<R>(
    file: &mut R,
    control: Option<&U1DirectConversionControl>,
) -> Result<(u64, String), U1DirectError>
where
    R: Read + Seek,
{
    if let Some(control) = control {
        control.checkpoint()?;
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|source| U1DirectError::Read {
            path: PathBuf::from("open file handle"),
            source,
        })?;
    let mut hasher = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        if let Some(control) = control {
            control.checkpoint()?;
        }
        let read = file
            .read(&mut buffer)
            .map_err(|source| U1DirectError::Read {
                path: PathBuf::from("open file handle"),
                source,
            })?;
        if read == 0 {
            break;
        }
        bytes = bytes.saturating_add(read as u64);
        hasher.update(&buffer[..read]);
    }
    if let Some(control) = control {
        control.checkpoint()?;
    }
    Ok((bytes, format!("{:x}", hasher.finalize())))
}

#[cfg(test)]
mod source_snapshot_toctou_tests {
    use super::*;
    use std::io::Cursor;

    struct CancellingReader {
        cursor: Cursor<Vec<u8>>,
        control: U1DirectConversionControl,
        cancelled: bool,
    }

    impl Read for CancellingReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let read = self.cursor.read(buffer)?;
            if read > 0 && !self.cancelled {
                self.cancelled = self.control.cancel();
            }
            Ok(read)
        }
    }

    impl Seek for CancellingReader {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.cursor.seek(position)
        }
    }

    fn write_minimal_source(path: &Path, bed_width: u32, unit: &str) {
        let file = File::create(path).unwrap();
        let mut archive = ZipWriter::new(file);
        archive
            .start_file(PROJECT_SETTINGS_PATH, SimpleFileOptions::default())
            .unwrap();
        archive
            .write_all(
                serde_json::to_string(&serde_json::json!({
                    "printable_area": [
                        "0x0",
                        format!("{bed_width}x0"),
                        format!("{bed_width}x{bed_width}"),
                        format!("0x{bed_width}")
                    ]
                }))
                .unwrap()
                .as_bytes(),
            )
            .unwrap();
        archive
            .start_file(MAIN_MODEL_PATH, SimpleFileOptions::default())
            .unwrap();
        archive
            .write_all(
                format!(
                    r#"<?xml version="1.0"?><model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" unit="{unit}"><resources/><build/></model>"#
                )
                .as_bytes(),
            )
            .unwrap();
        archive.finish().unwrap();
    }

    #[test]
    fn preparation_reads_the_verified_snapshot_but_keeps_the_original_basename() {
        let temporary = tempfile::TempDir::new().unwrap();
        let live_source = temporary.path().join("Original Project.3mf");
        write_minimal_source(&live_source, 256, "millimeter");
        let (byte_size, sha256) = hash_path(&live_source).unwrap();
        let expected = InputIdentity { byte_size, sha256 };
        let snapshot_directory = tempfile::TempDir::new().unwrap();
        let control = U1DirectConversionControl::new();
        let snapshot =
            snapshot_source(&live_source, &expected, snapshot_directory.path(), &control).unwrap();

        write_minimal_source(&live_source, 180, "inch");

        let inspected =
            inspect_verified_preparation_source(snapshot.path(), &live_source, &expected).unwrap();
        assert_eq!(inspected.base_name, "Original_Project");
        assert_eq!(inspected.bed_size, 256.0);
        assert!(matches!(
            inspect_verified_preparation_source(&live_source, &live_source, &expected),
            Err(U1DirectError::SourceIdentityChanged)
        ));
    }

    #[test]
    fn preparation_source_verification_observes_conversion_cancellation() {
        let temporary = tempfile::TempDir::new().unwrap();
        let source = temporary.path().join("Cancelled Project.3mf");
        write_minimal_source(&source, 256, "millimeter");
        let (byte_size, sha256) = hash_path(&source).unwrap();
        let expected = InputIdentity { byte_size, sha256 };
        let control = U1DirectConversionControl::new();
        assert!(control.cancel());

        let error = inspect_verified_preparation_source_with_control(
            &source,
            &source,
            &expected,
            Some(&control),
        )
        .unwrap_err();

        assert!(matches!(error, U1DirectError::Cancelled));
    }

    #[test]
    fn preparation_hash_stops_at_the_next_checkpoint_after_cancellation() {
        let control = U1DirectConversionControl::new();
        let mut reader = CancellingReader {
            cursor: Cursor::new(vec![42_u8; COPY_BUFFER_BYTES * 2]),
            control: control.clone(),
            cancelled: false,
        };

        let error = hash_reader_with_control(&mut reader, Some(&control)).unwrap_err();

        assert!(matches!(error, U1DirectError::Cancelled));
        assert!(reader.cancelled);
        assert_eq!(control.state(), U1DirectConversionState::Cancelled);
        assert!(reader.cursor.position() <= COPY_BUFFER_BYTES as u64);
    }
}

#[cfg(test)]
#[path = "u1_direct_tests.rs"]
mod tests;
