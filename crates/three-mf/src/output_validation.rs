//! Structural validation for a staged 3MF output package.
//!
//! This module intentionally does not write or repair OPC data. A package
//! writer can finish a temporary file, call [`validate_staged_output`], require
//! [`OutputValidationReport::ensure_publishable`], and only then atomically
//! publish that file.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use quick_xml::events::{BytesStart, BytesText, Event};
use quick_xml::name::{PrefixDeclaration, ResolveResult};
use quick_xml::{NsReader, Reader};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;
use zip::ZipArchive;

use crate::bounded_xml::XmlTokenLimitedReader;
use crate::stale_artifact_policy::{
    StaleArtifactDisposition, StaleArtifactKind, StaleArtifactPolicy, StaleEntryClassification,
};
use crate::zip_preflight::advertised_zip_entry_count;

const CONTENT_TYPES_PATH: &str = "[Content_Types].xml";
const ROOT_RELATIONSHIPS_PATH: &str = "_rels/.rels";
const PROJECT_SETTINGS_PATH: &str = "Metadata/project_settings.config";
const MODEL_SETTINGS_PATH: &str = "Metadata/model_settings.config";
const BAMBU_CUT_INFORMATION_PATH: &str = "Metadata/cut_information.xml";
const CONTENT_TYPES_NAMESPACE: &str =
    "http://schemas.openxmlformats.org/package/2006/content-types";
const RELATIONSHIPS_NAMESPACE: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships";
const RELATIONSHIPS_CONTENT_TYPE: &str = "application/vnd.openxmlformats-package.relationships+xml";
const MODEL_CONTENT_TYPE: &str = "application/vnd.ms-package.3dmanufacturing-3dmodel+xml";
const MODEL_RELATIONSHIP_TYPE: &str =
    "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel";
const THUMBNAIL_RELATIONSHIP_TYPE: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail";

/// Stable severity used by reports and publication gates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputIssueSeverity {
    Error,
    Warning,
}

/// Machine-readable structural validation issue codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputIssueCode {
    InvalidZip,
    ArchiveTooLarge,
    TooManyEntries,
    NonUtf8EntryName,
    UnsafeEntryPath,
    DuplicateEntry,
    EquivalentEntryName,
    PartNameDerivationConflict,
    EncryptedEntry,
    SymbolicLinkEntry,
    EntryTooLarge,
    CompressionRatioExceeded,
    ZipCrcMismatch,
    EntryReadFailed,
    MissingContentTypes,
    InvalidContentTypes,
    MissingContentTypeDeclaration,
    DanglingContentTypeOverride,
    MissingRootRelationships,
    InvalidRelationships,
    DuplicateRelationshipId,
    ExternalRelationshipForbidden,
    RelationshipSourceMissing,
    UnsafeRelationshipTarget,
    MissingRelationshipTarget,
    MissingRootModelRelationship,
    MultipleRootModelRelationships,
    ProductionRelationshipDepthExceeded,
    InvalidXml,
    ForbiddenXmlDoctype,
    InvalidJson,
    MissingProjectSettings,
    InvalidProjectSettings,
    MissingModelSettings,
    InvalidModelSettings,
    InvalidModel,
    InvalidModelNamespace,
    DuplicateObjectId,
    InvalidProductionUuid,
    DuplicateProductionUuid,
    InvalidModelReference,
    MissingObjectReference,
    InvalidProductionPath,
    MissingProductionPathRelationship,
    MissingProductionPathTarget,
    UnusedProductionModelRelationship,
    ProductionSelfReference,
    ProductionPathOutsideRootModel,
    OrphanModelPart,
    ModelGraphLimitExceeded,
    ForbiddenSlicedArtifact,
    StaleDerivedArtifact,
    UnexpectedGuiSaveArtifact,
    ForbiddenEmbeddedPreset,
    DanglingThumbnail,
}

/// One deterministic, serializable validation finding.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputValidationIssue {
    pub severity: OutputIssueSeverity,
    pub code: OutputIssueCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_entry: Option<String>,
    pub message: String,
}

impl OutputValidationIssue {
    fn error(code: OutputIssueCode, entry: Option<&str>, message: impl Into<String>) -> Self {
        Self {
            severity: OutputIssueSeverity::Error,
            code,
            entry: entry.map(str::to_owned),
            related_entry: None,
            message: message.into(),
        }
    }

    fn with_related_entry(mut self, related_entry: impl Into<String>) -> Self {
        self.related_entry = Some(related_entry.into());
        self
    }
}

/// Bounded ZIP statistics captured while validating the staged package.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputArchiveStatistics {
    pub archive_bytes: u64,
    pub entry_count: usize,
    pub total_compressed_bytes: u64,
    pub total_uncompressed_bytes: u64,
    pub largest_entry_uncompressed_bytes: u64,
    pub maximum_compression_ratio: f64,
    pub crc_checked_entry_count: usize,
}

/// Complete result suitable for logging in an export manifest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputValidationReport {
    pub is_valid: bool,
    pub statistics: OutputArchiveStatistics,
    pub stale_entries: Vec<StaleEntryClassification>,
    pub issues: Vec<OutputValidationIssue>,
}

impl Default for OutputValidationReport {
    fn default() -> Self {
        Self {
            is_valid: true,
            statistics: OutputArchiveStatistics::default(),
            stale_entries: Vec::new(),
            issues: Vec::new(),
        }
    }
}

impl OutputValidationReport {
    pub fn has_errors(&self) -> bool {
        self.issues
            .iter()
            .any(|issue| issue.severity == OutputIssueSeverity::Error)
    }

    pub fn ensure_publishable(&self) -> Result<(), OutputPublicationBlocked> {
        if self.is_valid && !self.has_errors() {
            return Ok(());
        }
        let mut issue_codes = self
            .issues
            .iter()
            .filter(|issue| issue.severity == OutputIssueSeverity::Error)
            .map(|issue| issue.code)
            .collect::<Vec<_>>();
        issue_codes.sort();
        issue_codes.dedup();
        Err(OutputPublicationBlocked {
            error_count: self
                .issues
                .iter()
                .filter(|issue| issue.severity == OutputIssueSeverity::Error)
                .count(),
            issue_codes,
        })
    }

    fn finish(mut self) -> Self {
        self.issues.sort();
        self.issues.dedup();
        self.stale_entries.sort();
        self.stale_entries.dedup();
        self.is_valid = !self.has_errors();
        self
    }

    fn push(&mut self, issue: OutputValidationIssue) {
        self.issues.push(issue);
    }
}

/// Error returned by the final publication gate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Error)]
#[error("staged 3MF output has {error_count} blocking structural issue(s)")]
#[serde(rename_all = "camelCase")]
pub struct OutputPublicationBlocked {
    pub error_count: usize,
    pub issue_codes: Vec<OutputIssueCode>,
}

/// I/O failure that prevents opening the staged path. Package defects are
/// represented inside [`OutputValidationReport`] instead.
#[derive(Debug, Error)]
pub enum StagedOutputValidationError {
    #[error("failed to open staged 3MF output {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Resource limits applied before inflating output entries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OutputValidationLimits {
    pub max_archive_bytes: u64,
    pub max_entries: usize,
    pub max_entry_name_bytes: usize,
    pub max_entry_uncompressed_bytes: u64,
    /// Geometry XML is streamed and can legitimately exceed the generic part
    /// limit (for example, the 619 MB Sailfin root model).
    pub max_model_entry_uncompressed_bytes: u64,
    pub max_relationship_bytes: u64,
    pub max_config_bytes: u64,
    pub max_total_uncompressed_bytes: u64,
    pub max_compression_ratio: f64,
    /// Cap for generic JSON/XML documents that are not `.config`, `.rels`, or
    /// streamed 3MF model parts.
    pub max_xml_json_parse_bytes: u64,
    pub max_xml_depth: usize,
    /// Maximum raw bytes in one XML lexical token before quick-xml can retain
    /// the complete event in memory.
    pub max_xml_token_bytes: usize,
    /// Upper bound for retained object IDs, Production UUIDs, and references.
    pub max_model_graph_records: usize,
}

impl Default for OutputValidationLimits {
    fn default() -> Self {
        Self {
            max_archive_bytes: 4 * 1024 * 1024 * 1024,
            max_entries: 50_000,
            max_entry_name_bytes: 1024,
            max_entry_uncompressed_bytes: 512 * 1024 * 1024,
            max_model_entry_uncompressed_bytes: 1024 * 1024 * 1024,
            max_relationship_bytes: 8 * 1024 * 1024,
            max_config_bytes: 64 * 1024 * 1024,
            max_total_uncompressed_bytes: 8 * 1024 * 1024 * 1024,
            max_compression_ratio: 200.0,
            max_xml_json_parse_bytes: 32 * 1024 * 1024,
            max_xml_depth: 256,
            max_xml_token_bytes: 8 * 1024 * 1024,
            max_model_graph_records: 2_000_000,
        }
    }
}

/// Validation behavior for a staged output package.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "profile", rename_all = "snake_case")]
pub enum GuiSaveArtifactPolicy {
    /// Do not grant any slicer-specific exceptions.
    #[default]
    Disabled,
    /// Accept only the metadata that Snapmaker Orca 2.3.5 is known to
    /// regenerate while saving an otherwise unsliced project.
    SnapmakerOrca2_3_5 { expected_plate_ids: BTreeSet<u32> },
    /// Accept only the metadata that Bambu Studio 02.02.00.85 is known to
    /// regenerate while saving an otherwise unsliced project.
    BambuStudio2_2_0_85 { expected_plate_ids: BTreeSet<u32> },
}

impl GuiSaveArtifactPolicy {
    fn expected_plate_ids(&self) -> Option<&BTreeSet<u32>> {
        match self {
            Self::Disabled => None,
            Self::SnapmakerOrca2_3_5 { expected_plate_ids }
            | Self::BambuStudio2_2_0_85 { expected_plate_ids } => Some(expected_plate_ids),
        }
    }

    fn allows_regenerated_artifact(&self, path: &str, kind: StaleArtifactKind) -> bool {
        let Some(expected_plate_ids) = self.expected_plate_ids() else {
            return false;
        };
        match kind {
            StaleArtifactKind::SliceMetadata => {
                path == "Metadata/slice_info.config"
                    || exact_plate_id(path, "Metadata/plate_", ".json")
                        .is_some_and(|plate_id| expected_plate_ids.contains(&plate_id))
            }
            StaleArtifactKind::SlicePreview => {
                GUI_SAVE_PREVIEW_PATTERNS.iter().any(|(prefix, suffix)| {
                    exact_plate_id(path, prefix, suffix)
                        .is_some_and(|plate_id| expected_plate_ids.contains(&plate_id))
                })
            }
            StaleArtifactKind::Toolpath
            | StaleArtifactKind::ToolpathChecksum
            | StaleArtifactKind::Thumbnail => false,
        }
    }

    fn allows_missing_content_type(&self, path: &str) -> bool {
        if self.expected_plate_ids().is_none() {
            return false;
        }
        if matches!(self, Self::BambuStudio2_2_0_85 { .. }) && path == BAMBU_CUT_INFORMATION_PATH {
            return true;
        }
        matches!(
            path,
            PROJECT_SETTINGS_PATH | MODEL_SETTINGS_PATH | "Metadata/slice_info.config"
        ) || exact_plate_id(path, "Metadata/plate_", ".json").is_some_and(|plate_id| {
            self.expected_plate_ids()
                .is_some_and(|expected| expected.contains(&plate_id))
        })
    }

    fn allows_metadata_entry(&self, path: &str) -> bool {
        if self.expected_plate_ids().is_none() {
            return true;
        }
        if matches!(self, Self::BambuStudio2_2_0_85 { .. }) && path == BAMBU_CUT_INFORMATION_PATH {
            return true;
        }
        if matches!(path, PROJECT_SETTINGS_PATH | MODEL_SETTINGS_PATH) {
            return true;
        }
        crate::stale_artifact_policy::stale_artifact_kind(path)
            .is_some_and(|kind| self.allows_regenerated_artifact(path, kind))
    }

    fn allows_bambu_face_count_metadata(&self) -> bool {
        matches!(self, Self::BambuStudio2_2_0_85 { .. })
    }

    fn allows_unused_empty_production_model_relationship(&self) -> bool {
        matches!(
            self,
            Self::SnapmakerOrca2_3_5 { .. } | Self::BambuStudio2_2_0_85 { .. }
        )
    }

    fn gui_save_label(&self) -> Option<&'static str> {
        match self {
            Self::Disabled => None,
            Self::SnapmakerOrca2_3_5 { .. } => Some("Snapmaker Orca 2.3.5"),
            Self::BambuStudio2_2_0_85 { .. } => Some("Bambu Studio 02.02.00.85"),
        }
    }
}

const GUI_SAVE_PREVIEW_PATTERNS: [(&str, &str); 5] = [
    ("Metadata/plate_", ".png"),
    ("Metadata/plate_", "_small.png"),
    ("Metadata/plate_no_light_", ".png"),
    ("Metadata/top_", ".png"),
    ("Metadata/pick_", ".png"),
];

fn exact_plate_id(path: &str, prefix: &str, suffix: &str) -> Option<u32> {
    let raw = path.strip_prefix(prefix)?.strip_suffix(suffix)?;
    let plate_id = raw.parse::<u32>().ok()?;
    (plate_id > 0 && raw == plate_id.to_string()).then_some(plate_id)
}

fn is_embedded_preset(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let Some(file_name) = lower.strip_prefix("metadata/") else {
        return false;
    };
    file_name.ends_with(".config")
        && [
            "process_settings_",
            "filament_settings_",
            "machine_settings_",
        ]
        .iter()
        .any(|prefix| file_name.starts_with(prefix))
}

fn is_checksum_entry(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".md5", ".sha1", ".sha256", ".checksum"]
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}

/// Validation behavior for a staged output package.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OutputValidationPolicy {
    pub limits: OutputValidationLimits,
    pub stale_artifacts: StaleArtifactPolicy,
    pub require_project_settings: bool,
    pub require_model_settings: bool,
    pub gui_save_artifacts: GuiSaveArtifactPolicy,
}

impl Default for OutputValidationPolicy {
    fn default() -> Self {
        Self::strict_unsliced()
    }
}

impl OutputValidationPolicy {
    pub fn strict_unsliced() -> Self {
        Self {
            limits: OutputValidationLimits::default(),
            stale_artifacts: StaleArtifactPolicy::unsliced(),
            require_project_settings: true,
            require_model_settings: true,
            gui_save_artifacts: GuiSaveArtifactPolicy::Disabled,
        }
    }

    /// Builds the narrowly versioned policy used to inspect a project after a
    /// GUI save in Snapmaker Orca 2.3.5. It does not permit toolpaths, embedded
    /// presets, or metadata for any plate outside `expected_plate_ids`.
    pub fn snapmaker_orca_2_3_5_gui_save(
        expected_plate_ids: impl IntoIterator<Item = u32>,
    ) -> Self {
        let mut policy = Self::strict_unsliced();
        policy.gui_save_artifacts = GuiSaveArtifactPolicy::SnapmakerOrca2_3_5 {
            expected_plate_ids: expected_plate_ids.into_iter().collect(),
        };
        policy
    }

    /// Builds the narrowly versioned policy used to inspect a project after a
    /// GUI save in Bambu Studio 02.02.00.85. Toolpaths, embedded presets, and
    /// metadata for plates outside `expected_plate_ids` remain forbidden.
    pub fn bambu_studio_2_2_0_85_gui_save(
        expected_plate_ids: impl IntoIterator<Item = u32>,
    ) -> Self {
        let mut policy = Self::strict_unsliced();
        policy.gui_save_artifacts = GuiSaveArtifactPolicy::BambuStudio2_2_0_85 {
            expected_plate_ids: expected_plate_ids.into_iter().collect(),
        };
        policy
    }

    fn classify_stale_entry(&self, path: &str) -> Option<StaleEntryClassification> {
        let mut classification = self.stale_artifacts.classify(path)?;
        if self
            .gui_save_artifacts
            .allows_regenerated_artifact(path, classification.kind)
        {
            classification.disposition = StaleArtifactDisposition::Preserve;
        }
        Some(classification)
    }
}

/// Reusable validator object for staged temporary files.
#[derive(Clone, Debug)]
pub struct StructuralOutputValidator {
    policy: OutputValidationPolicy,
}

impl StructuralOutputValidator {
    pub fn new(policy: OutputValidationPolicy) -> Self {
        Self { policy }
    }

    pub fn policy(&self) -> &OutputValidationPolicy {
        &self.policy
    }

    pub fn validate_path(
        &self,
        staged_path: impl AsRef<Path>,
    ) -> Result<OutputValidationReport, StagedOutputValidationError> {
        validate_staged_output(staged_path, &self.policy)
    }

    pub fn validate_reader<R: Read + Seek>(&self, reader: R) -> OutputValidationReport {
        validate_output_reader(reader, &self.policy)
    }
}

/// Validates a staged temporary file without modifying or publishing it.
pub fn validate_staged_output(
    staged_path: impl AsRef<Path>,
    policy: &OutputValidationPolicy,
) -> Result<OutputValidationReport, StagedOutputValidationError> {
    let path = staged_path.as_ref();
    let file = File::open(path).map_err(|source| StagedOutputValidationError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(validate_output_reader(file, policy))
}

/// Validates an in-memory or already-open staged output stream.
pub fn validate_output_reader<R: Read + Seek>(
    mut reader: R,
    policy: &OutputValidationPolicy,
) -> OutputValidationReport {
    let mut report = OutputValidationReport::default();
    let archive_bytes = match reader.seek(SeekFrom::End(0)) {
        Ok(length) => length,
        Err(error) => {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::InvalidZip,
                None,
                format!("Failed to determine staged ZIP length: {error}"),
            ));
            return report.finish();
        }
    };
    report.statistics.archive_bytes = archive_bytes;
    if archive_bytes > policy.limits.max_archive_bytes {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::ArchiveTooLarge,
            None,
            format!(
                "Archive is {archive_bytes} bytes; limit is {} bytes.",
                policy.limits.max_archive_bytes
            ),
        ));
        return report.finish();
    }
    if let Err(error) = reader.seek(SeekFrom::Start(0)) {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::InvalidZip,
            None,
            format!("Failed to rewind staged ZIP: {error}"),
        ));
        return report.finish();
    }
    let advertised_entries = match advertised_zip_entry_count(&mut reader) {
        Ok(count) => count,
        Err(message) => {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::InvalidZip,
                None,
                message,
            ));
            return report.finish();
        }
    };
    let max_entries = u64::try_from(policy.limits.max_entries).unwrap_or(u64::MAX);
    if advertised_entries > max_entries {
        report.statistics.entry_count = usize::try_from(advertised_entries).unwrap_or(usize::MAX);
        report.push(OutputValidationIssue::error(
            OutputIssueCode::TooManyEntries,
            None,
            format!(
                "Archive advertises {advertised_entries} entries; limit is {}.",
                policy.limits.max_entries
            ),
        ));
        return report.finish();
    }

    let mut archive = match ZipArchive::new(reader) {
        Ok(archive) => archive,
        Err(error) => {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::InvalidZip,
                None,
                format!("Staged output is not a readable ZIP archive: {error}"),
            ));
            return report.finish();
        }
    };

    validate_archive(&mut archive, policy, &mut report);
    report.finish()
}

#[derive(Clone, Debug)]
struct EntryMetadata {
    index: usize,
    name: String,
    is_directory: bool,
    uncompressed_size: u64,
    safe_to_inflate: bool,
}

fn validate_archive<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    policy: &OutputValidationPolicy,
    report: &mut OutputValidationReport,
) {
    report.statistics.entry_count = archive.len();
    if archive.len() > policy.limits.max_entries {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::TooManyEntries,
            None,
            format!(
                "Archive has {} entries; limit is {}.",
                archive.len(),
                policy.limits.max_entries
            ),
        ));
        return;
    }

    let scan_count = archive.len().min(policy.limits.max_entries);
    let mut entries = BTreeMap::<String, EntryMetadata>::new();
    let mut equivalent_part_names = BTreeMap::<String, String>::new();
    let mut all_entries = Vec::with_capacity(scan_count);
    let mut total_compressed = 0_u64;
    let mut total_uncompressed = 0_u64;

    for index in 0..scan_count {
        let entry = match archive.by_index(index) {
            Ok(entry) => entry,
            Err(error) => {
                report.push(OutputValidationIssue::error(
                    OutputIssueCode::EntryReadFailed,
                    None,
                    format!("Failed to inspect ZIP entry {index}: {error}"),
                ));
                continue;
            }
        };
        let name = match std::str::from_utf8(entry.name_raw()) {
            Ok(name) => name.to_owned(),
            Err(_) => {
                report.push(OutputValidationIssue::error(
                    OutputIssueCode::NonUtf8EntryName,
                    None,
                    format!("ZIP entry {index} does not have a UTF-8 name."),
                ));
                String::from_utf8_lossy(entry.name_raw()).into_owned()
            }
        };
        let is_directory = entry.is_dir();
        let unsafe_path =
            unsafe_entry_path_reason(&name, is_directory, policy.limits.max_entry_name_bytes);
        if let Some(reason) = unsafe_path.as_deref() {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::UnsafeEntryPath,
                Some(&name),
                reason,
            ));
        }
        let encrypted = entry.encrypted();
        if encrypted {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::EncryptedEntry,
                Some(&name),
                "Encrypted entries are forbidden in converter output.",
            ));
        }
        let symbolic_link = entry.is_symlink();
        if symbolic_link {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::SymbolicLinkEntry,
                Some(&name),
                "Symbolic-link entries are forbidden in converter output.",
            ));
        }

        let uncompressed_size = entry.size();
        let compressed_size = entry.compressed_size();
        total_compressed = total_compressed.saturating_add(compressed_size);
        total_uncompressed = total_uncompressed.saturating_add(uncompressed_size);
        report.statistics.largest_entry_uncompressed_bytes = report
            .statistics
            .largest_entry_uncompressed_bytes
            .max(uncompressed_size);
        let ratio = compression_ratio(uncompressed_size, compressed_size);
        report.statistics.maximum_compression_ratio =
            report.statistics.maximum_compression_ratio.max(ratio);
        let entry_limit = entry_uncompressed_limit(&name, &policy.limits);
        let entry_too_large = uncompressed_size > entry_limit;
        let ratio_exceeded = ratio > policy.limits.max_compression_ratio;
        if entry_too_large {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::EntryTooLarge,
                Some(&name),
                format!(
                    "Entry expands to {uncompressed_size} bytes; per-entry limit is {} bytes.",
                    entry_limit
                ),
            ));
        }
        if ratio_exceeded {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::CompressionRatioExceeded,
                Some(&name),
                format!(
                    "Entry compression ratio {ratio:.2} exceeds limit {:.2}.",
                    policy.limits.max_compression_ratio
                ),
            ));
        }
        let metadata = EntryMetadata {
            index,
            name: name.clone(),
            is_directory,
            uncompressed_size,
            safe_to_inflate: !entry_too_large
                && !ratio_exceeded
                && !encrypted
                && !symbolic_link
                && unsafe_path.is_none(),
        };
        if entries.insert(name.clone(), metadata.clone()).is_some() {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::DuplicateEntry,
                Some(&name),
                "ZIP entry name appears more than once.",
            ));
        }
        if !is_directory {
            let comparison_name = name.to_ascii_lowercase();
            if let Some(existing) = equivalent_part_names
                .insert(comparison_name, name.clone())
                .filter(|existing| existing.as_str() != name)
            {
                report.push(
                    OutputValidationIssue::error(
                        OutputIssueCode::EquivalentEntryName,
                        Some(&name),
                        "OPC part names must be unique under ASCII case-insensitive comparison.",
                    )
                    .with_related_entry(existing),
                );
            }
        }
        all_entries.push(metadata);
    }

    for (comparison_name, original_name) in &equivalent_part_names {
        for (separator, _) in comparison_name.match_indices('/') {
            let ancestor = &comparison_name[..separator];
            if let Some(ancestor_name) = equivalent_part_names.get(ancestor) {
                report.push(
                    OutputValidationIssue::error(
                        OutputIssueCode::PartNameDerivationConflict,
                        Some(original_name),
                        "An OPC part name cannot be derived from another part name by appending path segments.",
                    )
                    .with_related_entry(ancestor_name.clone()),
                );
            }
        }
    }

    report.statistics.total_compressed_bytes = total_compressed;
    report.statistics.total_uncompressed_bytes = total_uncompressed;
    if total_uncompressed > policy.limits.max_total_uncompressed_bytes {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::ArchiveTooLarge,
            None,
            format!(
                "Archive expands to {total_uncompressed} bytes; limit is {} bytes.",
                policy.limits.max_total_uncompressed_bytes
            ),
        ));
        return;
    }

    verify_entry_crc(archive, &all_entries, report);
    classify_stale_entries(&entries, policy, report);
    validate_gui_save_artifact_envelope(&entries, policy, report);

    let content_types = validate_content_types(archive, &entries, policy, report);
    let relationships = validate_relationships(archive, &entries, policy, report);
    if let Some(content_types) = content_types.as_ref() {
        validate_declared_content_types(content_types, &entries, &relationships, policy, report);
    }
    let model_part_paths =
        collect_model_part_paths(&entries, content_types.as_ref(), &relationships);
    validate_model_graph(
        archive,
        &entries,
        policy,
        &relationships,
        &model_part_paths,
        report,
    );
    validate_payload_documents(archive, &entries, policy, &model_part_paths, report);
    validate_thumbnails(&entries, &relationships, report);
}

fn compression_ratio(uncompressed: u64, compressed: u64) -> f64 {
    match (uncompressed, compressed) {
        (0, _) => 1.0,
        (_, 0) => u64::MAX as f64,
        _ => uncompressed as f64 / compressed as f64,
    }
}

fn is_3d_model_entry(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with("3d/") && lower.ends_with(".model")
}

fn entry_uncompressed_limit(path: &str, limits: &OutputValidationLimits) -> u64 {
    let lower = path.to_ascii_lowercase();
    if is_3d_model_entry(path) {
        limits.max_model_entry_uncompressed_bytes
    } else if lower.ends_with(".rels") {
        limits.max_relationship_bytes
    } else if lower.ends_with(".config") {
        limits.max_config_bytes
    } else {
        limits.max_entry_uncompressed_bytes
    }
}

fn unsafe_entry_path_reason(
    name: &str,
    is_directory: bool,
    max_name_bytes: usize,
) -> Option<String> {
    if name.is_empty() {
        return Some("Entry path is empty.".to_owned());
    }
    if name.len() > max_name_bytes {
        return Some(format!(
            "Entry path exceeds the {max_name_bytes}-byte limit."
        ));
    }
    if name.starts_with('/') || name.starts_with('\\') {
        return Some("Absolute entry paths are forbidden.".to_owned());
    }
    if name.contains('\\')
        || name.contains('\0')
        || name.contains('?')
        || name.contains('#')
        || name.contains('%')
        || name.chars().any(char::is_control)
    {
        return Some(
            "Control characters, backslashes, queries, fragments, and percent escapes are forbidden in package paths."
                .to_owned(),
        );
    }
    let segments = name.split('/').collect::<Vec<_>>();
    for (index, segment) in segments.iter().enumerate() {
        if segment.is_empty() && !(is_directory && index + 1 == segments.len()) {
            return Some("Empty package path segments are forbidden.".to_owned());
        }
        if matches!(*segment, "." | "..") {
            return Some("Dot traversal segments are forbidden.".to_owned());
        }
        if segment.contains(':') {
            return Some("Colon characters are forbidden in portable package paths.".to_owned());
        }
    }
    None
}

fn verify_entry_crc<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    entries: &[EntryMetadata],
    report: &mut OutputValidationReport,
) {
    for metadata in entries {
        if metadata.is_directory || !metadata.safe_to_inflate {
            continue;
        }
        let mut entry = match archive.by_index(metadata.index) {
            Ok(entry) => entry,
            Err(error) => {
                report.push(OutputValidationIssue::error(
                    OutputIssueCode::EntryReadFailed,
                    Some(&metadata.name),
                    format!("Failed to open ZIP entry for CRC validation: {error}"),
                ));
                continue;
            }
        };
        match io::copy(&mut entry, &mut io::sink()) {
            Ok(read) if read == metadata.uncompressed_size => {
                report.statistics.crc_checked_entry_count += 1;
            }
            Ok(read) => report.push(OutputValidationIssue::error(
                OutputIssueCode::EntryReadFailed,
                Some(&metadata.name),
                format!(
                    "ZIP entry produced {read} bytes; central directory declares {} bytes.",
                    metadata.uncompressed_size
                ),
            )),
            Err(error) if is_crc_error(&error) => {
                report.push(OutputValidationIssue::error(
                    OutputIssueCode::ZipCrcMismatch,
                    Some(&metadata.name),
                    "ZIP entry CRC does not match its central-directory checksum.",
                ));
            }
            Err(error) => report.push(OutputValidationIssue::error(
                OutputIssueCode::EntryReadFailed,
                Some(&metadata.name),
                format!("Failed while inflating ZIP entry: {error}"),
            )),
        }
    }
}

fn is_crc_error(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::InvalidData
        && error.to_string().to_ascii_lowercase().contains("checksum")
}

fn classify_stale_entries(
    entries: &BTreeMap<String, EntryMetadata>,
    policy: &OutputValidationPolicy,
    report: &mut OutputValidationReport,
) {
    report.stale_entries = entries
        .values()
        .filter(|entry| !entry.is_directory)
        .filter_map(|entry| policy.classify_stale_entry(&entry.name))
        .collect();
    for classification in report.stale_entries.clone() {
        match classification.disposition {
            StaleArtifactDisposition::Reject => {
                let message = match classification.kind {
                    StaleArtifactKind::SliceMetadata => {
                        "Stage A unsliced output contains copied slice metadata; remove it or replace it with adapter-approved generated metadata.".to_owned()
                    }
                    _ => format!(
                        "Unsliced output contains forbidden {:?} data.",
                        classification.kind
                    ),
                };
                report.push(OutputValidationIssue::error(
                    OutputIssueCode::ForbiddenSlicedArtifact,
                    Some(&classification.path),
                    message,
                ));
            }
            StaleArtifactDisposition::RegenerateOrRemove => {
                report.push(OutputValidationIssue::error(
                    OutputIssueCode::StaleDerivedArtifact,
                    Some(&classification.path),
                    "Derived preview must be regenerated by the writer or removed.",
                ));
            }
            StaleArtifactDisposition::Preserve | StaleArtifactDisposition::RequireRelationship => {}
        }
    }
}

fn validate_gui_save_artifact_envelope(
    entries: &BTreeMap<String, EntryMetadata>,
    policy: &OutputValidationPolicy,
    report: &mut OutputValidationReport,
) {
    let Some(gui_save_label) = policy.gui_save_artifacts.gui_save_label() else {
        return;
    };
    for entry in entries.values().filter(|entry| !entry.is_directory) {
        if is_embedded_preset(&entry.name) {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::ForbiddenEmbeddedPreset,
                Some(&entry.name),
                format!(
                    "{gui_save_label} GUI-save qualification forbids embedded process, filament, and machine presets."
                ),
            ));
            continue;
        }
        if is_checksum_entry(&entry.name)
            && crate::stale_artifact_policy::stale_artifact_kind(&entry.name).is_none()
        {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::UnexpectedGuiSaveArtifact,
                Some(&entry.name),
                format!("{gui_save_label} GUI-save qualification forbids checksum artifacts."),
            ));
            continue;
        }
        if entry.name.starts_with("Metadata/")
            && !policy.gui_save_artifacts.allows_metadata_entry(&entry.name)
        {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::UnexpectedGuiSaveArtifact,
                Some(&entry.name),
                format!("Metadata is outside the exact {gui_save_label} GUI-save allowlist."),
            ));
        }
    }
}

#[derive(Clone, Debug, Default)]
struct ContentTypeTable {
    defaults: BTreeMap<String, String>,
    overrides: BTreeMap<String, String>,
}

impl ContentTypeTable {
    fn content_type_for(&self, path: &str) -> Option<&str> {
        self.overrides.get(path).map(String::as_str).or_else(|| {
            let extension = path.rsplit_once('.')?.1.to_ascii_lowercase();
            self.defaults.get(&extension).map(String::as_str)
        })
    }
}

fn validate_content_types<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    entries: &BTreeMap<String, EntryMetadata>,
    policy: &OutputValidationPolicy,
    report: &mut OutputValidationReport,
) -> Option<ContentTypeTable> {
    let Some(metadata) = entries.get(CONTENT_TYPES_PATH) else {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::MissingContentTypes,
            Some(CONTENT_TYPES_PATH),
            "OPC package is missing [Content_Types].xml.",
        ));
        return None;
    };
    if metadata.uncompressed_size > policy.limits.max_relationship_bytes {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::EntryTooLarge,
            Some(CONTENT_TYPES_PATH),
            "Content Types metadata exceeds the configured metadata limit.",
        ));
        return None;
    }
    let entry = match archive.by_index(metadata.index) {
        Ok(entry) => entry,
        Err(error) => {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::InvalidContentTypes,
                Some(CONTENT_TYPES_PATH),
                format!("Failed to open Content Types metadata: {error}"),
            ));
            return None;
        }
    };
    match parse_content_types(
        BufReader::new(entry),
        policy.limits.max_xml_depth,
        policy.limits.max_xml_token_bytes,
    ) {
        Ok((table, overrides)) => {
            for part_name in overrides {
                if !entries.contains_key(&part_name) {
                    report.push(OutputValidationIssue::error(
                        OutputIssueCode::DanglingContentTypeOverride,
                        Some(CONTENT_TYPES_PATH),
                        format!("Content Type override points to missing part '{part_name}'."),
                    ));
                }
            }
            Some(table)
        }
        Err(XmlFailure::Doctype) => {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::ForbiddenXmlDoctype,
                Some(CONTENT_TYPES_PATH),
                "DOCTYPE declarations are forbidden in package XML.",
            ));
            None
        }
        Err(error) => {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::InvalidContentTypes,
                Some(CONTENT_TYPES_PATH),
                error.to_string(),
            ));
            None
        }
    }
}

fn parse_content_types<R: BufRead>(
    source: R,
    max_depth: usize,
    max_token_bytes: usize,
) -> Result<(ContentTypeTable, Vec<String>), XmlFailure> {
    let mut reader = Reader::from_reader(BufReader::new(XmlTokenLimitedReader::new(
        source,
        max_token_bytes,
    )));
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    let mut saw_root = false;
    let mut table = ContentTypeTable::default();
    let mut override_parts = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(event)) => {
                depth = checked_xml_depth(depth, max_depth)?;
                if depth != 1 || saw_root {
                    return Err(XmlFailure::Structure(
                        "Content Types declarations must be empty direct children of Types."
                            .to_owned(),
                    ));
                }
                validate_content_types_root(&reader, &event)?;
                saw_root = true;
            }
            Ok(Event::Empty(event)) => {
                if !saw_root || depth != 1 {
                    return Err(XmlFailure::Structure(
                        "Content Types declarations must be empty direct children of Types."
                            .to_owned(),
                    ));
                }
                collect_content_type(&reader, &event, &mut table, &mut override_parts)?;
            }
            Ok(Event::End(_)) => depth = depth.saturating_sub(1),
            Ok(Event::DocType(_)) => return Err(XmlFailure::Doctype),
            Ok(Event::Text(event)) => {
                validate_xml_text(&event)?;
                if !event.is_empty() {
                    return Err(XmlFailure::Structure(
                        "Content Types XML cannot contain character data.".to_owned(),
                    ));
                }
            }
            Ok(Event::Eof) => {
                if depth != 0 {
                    return Err(XmlFailure::Structure(
                        "Content Types XML ended before all elements were closed.".to_owned(),
                    ));
                }
                break;
            }
            Ok(_) => {}
            Err(error) => return Err(XmlFailure::Parse(error.to_string())),
        }
        buffer.clear();
    }
    if !saw_root {
        return Err(XmlFailure::Structure(
            "Content Types root element must be Types.".to_owned(),
        ));
    }
    if table.defaults.is_empty() && table.overrides.is_empty() {
        return Err(XmlFailure::Structure(
            "Content Types metadata has no Default or Override declarations.".to_owned(),
        ));
    }
    Ok((table, override_parts))
}

fn validate_content_types_root<R: BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
) -> Result<(), XmlFailure> {
    if event.name().as_ref() != b"Types" {
        return Err(XmlFailure::Structure(
            "Content Types root element must be the unprefixed Types element.".to_owned(),
        ));
    }
    if xml_attr(reader, event, b"xmlns")?.as_deref() != Some(CONTENT_TYPES_NAMESPACE) {
        return Err(XmlFailure::Structure(format!(
            "Content Types root must declare namespace '{CONTENT_TYPES_NAMESPACE}'."
        )));
    }
    Ok(())
}

fn collect_content_type<R: BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    table: &mut ContentTypeTable,
    override_parts: &mut Vec<String>,
) -> Result<(), XmlFailure> {
    match event.name().as_ref() {
        b"Default" => {
            let extension = required_xml_attr(reader, event, b"Extension")?;
            let content_type = required_xml_attr(reader, event, b"ContentType")?;
            if extension.is_empty()
                || extension.starts_with('.')
                || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
                || content_type.trim().is_empty()
            {
                return Err(XmlFailure::Structure(format!(
                    "Invalid default Content Type extension '{extension}'."
                )));
            }
            let extension = extension.to_ascii_lowercase();
            if table
                .defaults
                .insert(extension.clone(), content_type)
                .is_some()
            {
                return Err(XmlFailure::Structure(format!(
                    "Duplicate default Content Type extension '{extension}'."
                )));
            }
        }
        b"Override" => {
            let part_name = required_xml_attr(reader, event, b"PartName")?;
            let content_type = required_xml_attr(reader, event, b"ContentType")?;
            if !part_name.starts_with('/') {
                return Err(XmlFailure::Structure(format!(
                    "Content Type override '{part_name}' must start with '/'."
                )));
            }
            let part_name =
                resolve_internal_target("", &part_name).map_err(XmlFailure::Structure)?;
            if content_type.trim().is_empty() {
                return Err(XmlFailure::Structure(
                    "Content Type override must not be empty.".to_owned(),
                ));
            }
            if table
                .overrides
                .insert(part_name.clone(), content_type)
                .is_some()
            {
                return Err(XmlFailure::Structure(format!(
                    "Duplicate Content Type override for '{part_name}'."
                )));
            }
            override_parts.push(part_name);
        }
        _ => {
            return Err(XmlFailure::Structure(format!(
                "Unexpected Content Types child element '{}'.",
                String::from_utf8_lossy(event.name().as_ref())
            )));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Default)]
struct RelationshipGraph {
    root_model_targets: BTreeSet<String>,
    thumbnail_targets: BTreeSet<String>,
    model_targets_by_source: BTreeMap<String, BTreeSet<String>>,
    truncated: bool,
}

impl RelationshipGraph {
    fn all_model_targets(&self) -> BTreeSet<String> {
        let mut targets = self.root_model_targets.clone();
        for related in self.model_targets_by_source.values() {
            targets.extend(related.iter().cloned());
        }
        targets
    }
}

#[derive(Clone, Debug)]
struct RelationshipRecord {
    relationship_type: String,
    target: String,
    external: bool,
}

fn validate_relationships<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    entries: &BTreeMap<String, EntryMetadata>,
    policy: &OutputValidationPolicy,
    report: &mut OutputValidationReport,
) -> RelationshipGraph {
    let mut graph = RelationshipGraph::default();
    let mut retained_graph_records = 0_usize;
    if !entries.contains_key(ROOT_RELATIONSHIPS_PATH) {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::MissingRootRelationships,
            Some(ROOT_RELATIONSHIPS_PATH),
            "OPC package is missing root relationships.",
        ));
    }

    for metadata in entries
        .values()
        .filter(|entry| !entry.is_directory && entry.name.to_ascii_lowercase().ends_with(".rels"))
    {
        let source_part = match source_part_for_relationship(&metadata.name) {
            Ok(source_part) => source_part,
            Err(message) => {
                report.push(OutputValidationIssue::error(
                    OutputIssueCode::InvalidRelationships,
                    Some(&metadata.name),
                    message,
                ));
                continue;
            }
        };
        if !source_part.is_empty() && !entries.contains_key(&source_part) {
            report.push(
                OutputValidationIssue::error(
                    OutputIssueCode::RelationshipSourceMissing,
                    Some(&metadata.name),
                    format!("Relationship source part '{source_part}' is missing."),
                )
                .with_related_entry(source_part.clone()),
            );
        }
        if metadata.uncompressed_size > policy.limits.max_relationship_bytes {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::EntryTooLarge,
                Some(&metadata.name),
                "Relationship metadata exceeds the configured metadata limit.",
            ));
            continue;
        }
        let entry = match archive.by_index(metadata.index) {
            Ok(entry) => entry,
            Err(error) => {
                report.push(OutputValidationIssue::error(
                    OutputIssueCode::InvalidRelationships,
                    Some(&metadata.name),
                    format!("Failed to open relationship metadata: {error}"),
                ));
                continue;
            }
        };
        let records = match parse_relationship_records(
            BufReader::new(entry),
            policy.limits.max_xml_depth,
            policy.limits.max_xml_token_bytes,
        ) {
            Ok(records) => records,
            Err(RelationshipFailure::Doctype) => {
                report.push(OutputValidationIssue::error(
                    OutputIssueCode::ForbiddenXmlDoctype,
                    Some(&metadata.name),
                    "DOCTYPE declarations are forbidden in relationship XML.",
                ));
                continue;
            }
            Err(RelationshipFailure::DuplicateId(id)) => {
                report.push(OutputValidationIssue::error(
                    OutputIssueCode::DuplicateRelationshipId,
                    Some(&metadata.name),
                    format!("Relationship ID '{id}' appears more than once."),
                ));
                continue;
            }
            Err(error) => {
                report.push(OutputValidationIssue::error(
                    OutputIssueCode::InvalidRelationships,
                    Some(&metadata.name),
                    error.to_string(),
                ));
                continue;
            }
        };

        for record in records {
            if record.external {
                report.push(OutputValidationIssue::error(
                    OutputIssueCode::ExternalRelationshipForbidden,
                    Some(&metadata.name),
                    format!(
                        "External relationship target '{}' is forbidden in converter output.",
                        record.target
                    ),
                ));
                continue;
            }
            let target = match resolve_internal_target(&source_part, &record.target) {
                Ok(target) => target,
                Err(message) => {
                    report.push(OutputValidationIssue::error(
                        OutputIssueCode::UnsafeRelationshipTarget,
                        Some(&metadata.name),
                        message,
                    ));
                    continue;
                }
            };
            if !entries.contains_key(&target) {
                report.push(
                    OutputValidationIssue::error(
                        OutputIssueCode::MissingRelationshipTarget,
                        Some(&metadata.name),
                        format!("Internal relationship target '{target}' is missing."),
                    )
                    .with_related_entry(target.clone()),
                );
            }
            let is_model_relationship = record.relationship_type == MODEL_RELATIONSHIP_TYPE;
            let is_thumbnail_relationship = record.relationship_type == THUMBNAIL_RELATIONSHIP_TYPE;
            if (is_model_relationship || is_thumbnail_relationship)
                && !reserve_relationship_graph_record(
                    &metadata.name,
                    &mut graph,
                    &mut retained_graph_records,
                    policy.limits.max_model_graph_records,
                    report,
                )
            {
                continue;
            }
            if metadata.name == ROOT_RELATIONSHIPS_PATH && is_model_relationship {
                graph.root_model_targets.insert(target.clone());
            }
            if is_model_relationship {
                graph
                    .model_targets_by_source
                    .entry(source_part.clone())
                    .or_default()
                    .insert(target.clone());
            }
            if is_thumbnail_relationship {
                graph.thumbnail_targets.insert(target);
            }
        }
    }

    if graph.root_model_targets.is_empty() {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::MissingRootModelRelationship,
            Some(ROOT_RELATIONSHIPS_PATH),
            "Root relationships do not select a main 3MF model part.",
        ));
    } else if graph.root_model_targets.len() != 1 {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::MultipleRootModelRelationships,
            Some(ROOT_RELATIONSHIPS_PATH),
            format!(
                "Root relationships select {} model parts; a 3MF package must have exactly one root model part.",
                graph.root_model_targets.len()
            ),
        ));
    }
    if let Some(root_model) = graph.root_model_targets.iter().next() {
        for (source, targets) in &graph.model_targets_by_source {
            if source.is_empty() || source == root_model || targets.is_empty() {
                continue;
            }
            report.push(OutputValidationIssue::error(
                OutputIssueCode::ProductionRelationshipDepthExceeded,
                Some(&relationship_part_for_source(source)),
                format!(
                    "Non-root model part '{source}' relates to another model part; Production Extension model relationships are limited to one level."
                ),
            ));
        }
    }
    graph
}

fn relationship_part_for_source(source_part: &str) -> String {
    let (directory, file_name) = source_part
        .rsplit_once('/')
        .map_or(("", source_part), |(directory, file_name)| {
            (directory, file_name)
        });
    if directory.is_empty() {
        format!("_rels/{file_name}.rels")
    } else {
        format!("{directory}/_rels/{file_name}.rels")
    }
}

fn reserve_relationship_graph_record(
    path: &str,
    graph: &mut RelationshipGraph,
    retained_graph_records: &mut usize,
    max_graph_records: usize,
    report: &mut OutputValidationReport,
) -> bool {
    if *retained_graph_records >= max_graph_records {
        if !graph.truncated {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::ModelGraphLimitExceeded,
                Some(path),
                format!(
                    "Relationship graph exceeds the {max_graph_records}-record validation limit."
                ),
            ));
        }
        graph.truncated = true;
        return false;
    }
    *retained_graph_records += 1;
    true
}

fn source_part_for_relationship(path: &str) -> Result<String, String> {
    if path == ROOT_RELATIONSHIPS_PATH {
        return Ok(String::new());
    }
    if let Some(relationship_name) = path.strip_prefix("_rels/") {
        if relationship_name.contains('/') {
            return Err(format!(
                "Relationship part '{path}' is not directly inside the package-root _rels directory."
            ));
        }
        let source_name = relationship_name.strip_suffix(".rels").ok_or_else(|| {
            format!("Relationship part '{path}' does not end with the required .rels suffix.")
        })?;
        if source_name.is_empty() {
            return Err(format!("Relationship part '{path}' has no source part."));
        }
        return Ok(source_name.to_owned());
    }
    let (directory, relationship_name) = path
        .rsplit_once("/_rels/")
        .ok_or_else(|| format!("Relationship part '{path}' is not inside an _rels directory."))?;
    let source_name = relationship_name.strip_suffix(".rels").ok_or_else(|| {
        format!("Relationship part '{path}' does not end with the required .rels suffix.")
    })?;
    if directory.is_empty() || source_name.is_empty() {
        return Err(format!("Relationship part '{path}' has no source part."));
    }
    Ok(format!("{directory}/{source_name}"))
}

fn resolve_internal_target(source_part: &str, target: &str) -> Result<String, String> {
    if target.is_empty()
        || target.contains('\\')
        || target.contains('\0')
        || target.contains('?')
        || target.contains('#')
        || target.contains('%')
        || target.chars().any(char::is_control)
    {
        return Err(format!("Unsafe internal relationship target '{target}'."));
    }
    if target.contains("://") {
        return Err(format!(
            "External URI '{target}' is marked as an internal relationship target."
        ));
    }
    let mut segments = Vec::<&str>::new();
    if !target.starts_with('/') && !source_part.is_empty() {
        let parent = source_part
            .rsplit_once('/')
            .map_or("", |(parent, _)| parent);
        segments.extend(parent.split('/').filter(|segment| !segment.is_empty()));
    }
    let target_path = target.strip_prefix('/').unwrap_or(target);
    for segment in target_path.split('/') {
        match segment {
            "" => {
                return Err(format!(
                    "Relationship target '{target}' contains an empty path segment."
                ));
            }
            "." | ".." => {
                return Err(format!(
                    "Relationship target '{target}' contains a forbidden dot segment."
                ));
            }
            segment if segment.contains(':') => {
                return Err(format!(
                    "Relationship target '{target}' contains an unsafe path segment."
                ));
            }
            segment => segments.push(segment),
        }
    }
    if segments.is_empty() {
        return Err(format!(
            "Relationship target '{target}' resolves to an empty package path."
        ));
    }
    Ok(segments.join("/"))
}

#[derive(Debug)]
enum RelationshipFailure {
    Doctype,
    DuplicateId(String),
    Parse(String),
    Structure(String),
}

impl std::fmt::Display for RelationshipFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Doctype => formatter.write_str("DOCTYPE is forbidden."),
            Self::DuplicateId(id) => write!(formatter, "Duplicate relationship ID '{id}'."),
            Self::Parse(message) | Self::Structure(message) => formatter.write_str(message),
        }
    }
}

fn parse_relationship_records<R: BufRead>(
    source: R,
    max_depth: usize,
    max_token_bytes: usize,
) -> Result<Vec<RelationshipRecord>, RelationshipFailure> {
    let mut reader = Reader::from_reader(BufReader::new(XmlTokenLimitedReader::new(
        source,
        max_token_bytes,
    )));
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut saw_root = false;
    let mut depth = 0_usize;
    let mut seen_ids = BTreeSet::new();
    let mut seen_endpoints = BTreeSet::new();
    let mut records = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(event)) => {
                depth = checked_xml_depth(depth, max_depth)
                    .map_err(|error| RelationshipFailure::Structure(error.to_string()))?;
                if depth != 1 || saw_root {
                    return Err(RelationshipFailure::Structure(
                        "Relationship records must be empty direct children of Relationships."
                            .to_owned(),
                    ));
                }
                validate_relationships_root(&reader, &event)?;
                saw_root = true;
            }
            Ok(Event::Empty(event)) => {
                if !saw_root || depth != 1 || event.name().as_ref() != b"Relationship" {
                    return Err(RelationshipFailure::Structure(format!(
                        "Unexpected relationship child element '{}'; only empty Relationship records are allowed.",
                        String::from_utf8_lossy(event.name().as_ref())
                    )));
                }
                let record = parse_relationship_event(&reader, &event, &mut seen_ids)?;
                if !seen_endpoints.insert((record.relationship_type.clone(), record.target.clone()))
                {
                    return Err(RelationshipFailure::Structure(format!(
                        "Relationship endpoint '{}:{}' is duplicated.",
                        record.relationship_type, record.target
                    )));
                }
                records.push(record);
            }
            Ok(Event::End(_)) => depth = depth.saturating_sub(1),
            Ok(Event::DocType(_)) => return Err(RelationshipFailure::Doctype),
            Ok(Event::Text(event)) => {
                validate_xml_text(&event)
                    .map_err(|error| RelationshipFailure::Parse(error.to_string()))?;
                if !event.is_empty() {
                    return Err(RelationshipFailure::Structure(
                        "Relationship XML cannot contain character data.".to_owned(),
                    ));
                }
            }
            Ok(Event::Eof) => {
                if depth != 0 {
                    return Err(RelationshipFailure::Structure(
                        "Relationship XML ended before all elements were closed.".to_owned(),
                    ));
                }
                break;
            }
            Ok(_) => {}
            Err(error) => return Err(RelationshipFailure::Parse(error.to_string())),
        }
        buffer.clear();
    }
    if !saw_root {
        return Err(RelationshipFailure::Structure(
            "Relationship XML root element must be Relationships.".to_owned(),
        ));
    }
    Ok(records)
}

fn validate_relationships_root<R: BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
) -> Result<(), RelationshipFailure> {
    if event.name().as_ref() != b"Relationships" {
        return Err(RelationshipFailure::Structure(
            "Relationship XML root must be the unprefixed Relationships element.".to_owned(),
        ));
    }
    let namespace = xml_attr(reader, event, b"xmlns")
        .map_err(|error| RelationshipFailure::Structure(error.to_string()))?;
    if namespace.as_deref() != Some(RELATIONSHIPS_NAMESPACE) {
        return Err(RelationshipFailure::Structure(format!(
            "Relationship root must declare namespace '{RELATIONSHIPS_NAMESPACE}'."
        )));
    }
    Ok(())
}

fn parse_relationship_event<R: BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    seen_ids: &mut BTreeSet<String>,
) -> Result<RelationshipRecord, RelationshipFailure> {
    let id = required_xml_attr(reader, event, b"Id")
        .map_err(|error| RelationshipFailure::Structure(error.to_string()))?;
    if !is_portable_relationship_id(&id) {
        return Err(RelationshipFailure::Structure(format!(
            "Relationship ID '{id}' is not a portable XML ID."
        )));
    }
    if !seen_ids.insert(id.clone()) {
        return Err(RelationshipFailure::DuplicateId(id));
    }
    let target = required_xml_attr(reader, event, b"Target")
        .map_err(|error| RelationshipFailure::Structure(error.to_string()))?;
    let relationship_type = required_xml_attr(reader, event, b"Type")
        .map_err(|error| RelationshipFailure::Structure(error.to_string()))?;
    let target_mode = xml_attr(reader, event, b"TargetMode")
        .map_err(|error| RelationshipFailure::Structure(error.to_string()))?;
    let external = match target_mode.as_deref() {
        None | Some("Internal") | Some("internal") => false,
        Some("External") | Some("external") => true,
        Some(mode) => {
            return Err(RelationshipFailure::Structure(format!(
                "Relationship TargetMode '{mode}' is invalid."
            )));
        }
    };
    Ok(RelationshipRecord {
        relationship_type,
        target,
        external,
    })
}

fn validate_declared_content_types(
    content_types: &ContentTypeTable,
    entries: &BTreeMap<String, EntryMetadata>,
    relationships: &RelationshipGraph,
    policy: &OutputValidationPolicy,
    report: &mut OutputValidationReport,
) {
    for entry in entries.values().filter(|entry| !entry.is_directory) {
        if entry.name == CONTENT_TYPES_PATH {
            continue;
        }
        let expected = if entry.name.to_ascii_lowercase().ends_with(".rels") {
            Some(RELATIONSHIPS_CONTENT_TYPE)
        } else if is_3d_model_entry(&entry.name) {
            Some(MODEL_CONTENT_TYPE)
        } else {
            None
        };
        let Some(declared) = content_types.content_type_for(&entry.name) else {
            if policy
                .gui_save_artifacts
                .allows_missing_content_type(&entry.name)
            {
                continue;
            }
            report.push(OutputValidationIssue::error(
                OutputIssueCode::MissingContentTypeDeclaration,
                Some(&entry.name),
                "Package part has no Content Type declaration.",
            ));
            continue;
        };
        if let Some(expected) = expected
            && !declared.eq_ignore_ascii_case(expected)
        {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::MissingContentTypeDeclaration,
                Some(&entry.name),
                format!("Part declares Content Type '{declared}', but '{expected}' is required."),
            ));
        }
    }
    for model_part in relationships.all_model_targets() {
        if !content_types
            .content_type_for(&model_part)
            .is_some_and(|content_type| content_type.eq_ignore_ascii_case(MODEL_CONTENT_TYPE))
        {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::MissingContentTypeDeclaration,
                Some(&model_part),
                "Model relationship target is not declared as a 3MF model Content Type.",
            ));
        }
    }
}

fn collect_model_part_paths(
    entries: &BTreeMap<String, EntryMetadata>,
    content_types: Option<&ContentTypeTable>,
    relationships: &RelationshipGraph,
) -> BTreeSet<String> {
    let mut model_parts = relationships.all_model_targets();
    if let Some(content_types) = content_types {
        for entry in entries.values().filter(|entry| !entry.is_directory) {
            if content_types
                .content_type_for(&entry.name)
                .is_some_and(|content_type| content_type.eq_ignore_ascii_case(MODEL_CONTENT_TYPE))
            {
                model_parts.insert(entry.name.clone());
            }
        }
    }
    model_parts
}

const CORE_MODEL_NAMESPACE: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";
const PRODUCTION_NAMESPACE: &str =
    "http://schemas.microsoft.com/3dmanufacturing/production/2015/06";

#[derive(Clone, Copy, Debug)]
enum ModelReferenceKind {
    Component,
    BuildItem,
}

#[derive(Clone, Debug)]
struct ModelObjectReference {
    kind: ModelReferenceKind,
    object_id: u32,
    production_path: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct ModelPartGraph {
    objects: BTreeSet<u32>,
    references: Vec<ModelObjectReference>,
    root_is_model: bool,
    has_resources: bool,
    has_build: bool,
    has_build_items: bool,
    core_namespace: Option<String>,
    production_namespace: Option<String>,
    required_extensions: BTreeSet<String>,
    production_is_required: bool,
    uses_production_extension: bool,
    uses_production_path: bool,
    missing_production_uuids: BTreeMap<String, usize>,
    truncated: bool,
}

fn validate_model_graph<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    entries: &BTreeMap<String, EntryMetadata>,
    policy: &OutputValidationPolicy,
    relationships: &RelationshipGraph,
    model_part_paths: &BTreeSet<String>,
    report: &mut OutputValidationReport,
) {
    let mut model_parts = BTreeMap::<String, ModelPartGraph>::new();
    let mut package_uuids = BTreeMap::<Uuid, String>::new();
    let mut retained_graph_records = 0_usize;

    for metadata in entries
        .values()
        .filter(|entry| !entry.is_directory && model_part_paths.contains(&entry.name))
    {
        if !metadata.safe_to_inflate {
            continue;
        }
        let entry = match archive.by_index(metadata.index) {
            Ok(entry) => entry,
            Err(error) => {
                report.push(OutputValidationIssue::error(
                    OutputIssueCode::EntryReadFailed,
                    Some(&metadata.name),
                    format!("Failed to open 3MF model part: {error}"),
                ));
                continue;
            }
        };
        let parsed = parse_model_part_graph(
            BufReader::new(entry),
            &metadata.name,
            policy,
            &mut package_uuids,
            &mut retained_graph_records,
            report,
        );
        let graph = match parsed {
            Ok(graph) => graph,
            Err(XmlFailure::Doctype) => {
                report.push(OutputValidationIssue::error(
                    OutputIssueCode::ForbiddenXmlDoctype,
                    Some(&metadata.name),
                    "DOCTYPE declarations are forbidden in 3MF model XML.",
                ));
                continue;
            }
            Err(error) => {
                report.push(OutputValidationIssue::error(
                    OutputIssueCode::InvalidXml,
                    Some(&metadata.name),
                    error.to_string(),
                ));
                continue;
            }
        };
        model_parts.insert(metadata.name.clone(), graph);
    }

    let package_uses_production = model_parts
        .values()
        .any(|graph| graph.uses_production_extension);
    for (path, graph) in &model_parts {
        validate_model_part_shape(
            path,
            graph,
            relationships.root_model_targets.contains(path),
            package_uses_production,
            report,
        );
    }

    validate_model_reachability(&model_parts, relationships, report);
    for (source_path, source) in &model_parts {
        if source.truncated {
            continue;
        }
        for reference in &source.references {
            validate_model_reference(
                source_path,
                source,
                reference,
                &model_parts,
                entries,
                relationships,
                report,
            );
        }
    }
    validate_production_relationship_closure(&model_parts, relationships, policy, report);
}

fn validate_model_reachability(
    model_parts: &BTreeMap<String, ModelPartGraph>,
    relationships: &RelationshipGraph,
    report: &mut OutputValidationReport,
) {
    if relationships.truncated {
        return;
    }
    let mut reachable = relationships.root_model_targets.clone();
    for root_model in &relationships.root_model_targets {
        if let Some(targets) = relationships.model_targets_by_source.get(root_model) {
            reachable.extend(targets.iter().cloned());
        }
    }
    for path in model_parts.keys() {
        if !reachable.contains(path) {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::OrphanModelPart,
                Some(path),
                "Model part is not selected by the root relationship graph.",
            ));
        }
    }
}

fn validate_production_relationship_closure(
    model_parts: &BTreeMap<String, ModelPartGraph>,
    relationships: &RelationshipGraph,
    policy: &OutputValidationPolicy,
    report: &mut OutputValidationReport,
) {
    if relationships.truncated {
        return;
    }
    for root_model in &relationships.root_model_targets {
        let Some(root_graph) = model_parts.get(root_model) else {
            continue;
        };
        if root_graph.truncated {
            continue;
        }
        let mut referenced_targets = BTreeSet::new();
        for reference in &root_graph.references {
            let Some(raw_path) = reference.production_path.as_deref() else {
                continue;
            };
            let Ok(target) = resolve_internal_target(root_model, raw_path) else {
                continue;
            };
            if target == *root_model {
                report.push(
                    OutputValidationIssue::error(
                        OutputIssueCode::ProductionSelfReference,
                        Some(root_model),
                        "A Production p:path cannot target its own root model part.",
                    )
                    .with_related_entry(target.clone()),
                );
            }
            referenced_targets.insert(target);
        }
        if let Some(declared_targets) = relationships.model_targets_by_source.get(root_model) {
            for target in declared_targets {
                if target == root_model {
                    report.push(
                        OutputValidationIssue::error(
                            OutputIssueCode::ProductionSelfReference,
                            Some(&relationship_part_for_source(root_model)),
                            "A Production model relationship cannot target its own source part.",
                        )
                        .with_related_entry(target.clone()),
                    );
                } else if !referenced_targets.contains(target)
                    && !(policy
                        .gui_save_artifacts
                        .allows_unused_empty_production_model_relationship()
                        && model_parts.get(target).is_some_and(|graph| {
                            is_gui_save_empty_placeholder_model(target, graph)
                        }))
                {
                    report.push(
                        OutputValidationIssue::error(
                            OutputIssueCode::UnusedProductionModelRelationship,
                            Some(&relationship_part_for_source(root_model)),
                            format!(
                                "Production model relationship target '{target}' is not used by any root-model p:path."
                            ),
                        )
                        .with_related_entry(target.clone()),
                    );
                }
            }
        }
    }
}

fn is_gui_save_empty_placeholder_model(path: &str, graph: &ModelPartGraph) -> bool {
    let Some(filename) = path.strip_prefix("3D/Objects/") else {
        return false;
    };
    let Some(stem) = filename.strip_suffix(".model") else {
        return false;
    };
    if stem.is_empty() || filename.contains('/') {
        return false;
    }
    !graph.truncated
        && graph.root_is_model
        && graph.has_resources
        && graph.has_build
        && !graph.has_build_items
        && graph.objects.is_empty()
        && graph.references.is_empty()
        && graph.core_namespace.as_deref() == Some(CORE_MODEL_NAMESPACE)
        && graph.production_namespace.as_deref() == Some(PRODUCTION_NAMESPACE)
        && graph.production_is_required
        && graph.required_extensions.len() == 1
}

fn parse_model_part_graph<R: BufRead>(
    source: R,
    path: &str,
    policy: &OutputValidationPolicy,
    package_uuids: &mut BTreeMap<Uuid, String>,
    retained_graph_records: &mut usize,
    report: &mut OutputValidationReport,
) -> Result<ModelPartGraph, XmlFailure> {
    let mut reader = NsReader::from_reader(BufReader::new(XmlTokenLimitedReader::new(
        source,
        policy.limits.max_xml_token_bytes,
    )));
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    let mut graph = ModelPartGraph::default();
    let mut saw_root = false;
    let mut ancestors = Vec::<String>::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(event)) => {
                depth = checked_xml_depth(depth, policy.limits.max_xml_depth)?;
                collect_model_event(
                    &reader,
                    &event,
                    path,
                    !saw_root,
                    &ancestors,
                    &mut graph,
                    package_uuids,
                    retained_graph_records,
                    policy.limits.max_model_graph_records,
                    report,
                )?;
                saw_root = true;
                ancestors.push(local_name(event.name().as_ref()));
            }
            Ok(Event::Empty(event)) => {
                collect_model_event(
                    &reader,
                    &event,
                    path,
                    !saw_root,
                    &ancestors,
                    &mut graph,
                    package_uuids,
                    retained_graph_records,
                    policy.limits.max_model_graph_records,
                    report,
                )?;
                saw_root = true;
            }
            Ok(Event::End(_)) => {
                depth = depth.saturating_sub(1);
                ancestors.pop();
            }
            Ok(Event::DocType(_)) => return Err(XmlFailure::Doctype),
            Ok(Event::Text(event)) => validate_xml_text(&event)?,
            Ok(Event::Eof) => {
                if depth != 0 {
                    return Err(XmlFailure::Structure(
                        "3MF model XML ended before all elements were closed.".to_owned(),
                    ));
                }
                break;
            }
            Ok(_) => {}
            Err(error) => return Err(XmlFailure::Parse(error.to_string())),
        }
        buffer.clear();
    }
    Ok(graph)
}

#[allow(clippy::too_many_arguments)]
fn collect_model_event<R: BufRead>(
    reader: &NsReader<R>,
    event: &BytesStart<'_>,
    path: &str,
    is_root: bool,
    ancestors: &[String],
    graph: &mut ModelPartGraph,
    package_uuids: &mut BTreeMap<Uuid, String>,
    retained_graph_records: &mut usize,
    max_graph_records: usize,
    report: &mut OutputValidationReport,
) -> Result<(), XmlFailure> {
    let element = local_name(event.name().as_ref());
    if is_root {
        graph.root_is_model = event.name().as_ref() == b"model";
        graph.core_namespace = xml_attr(reader, event, b"xmlns")?;
        graph.required_extensions = xml_attr(reader, event, b"requiredextensions")?
            .unwrap_or_default()
            .split_ascii_whitespace()
            .map(str::to_owned)
            .collect();
        let production_prefixes = reader
            .prefixes()
            .filter_map(|(prefix, namespace)| {
                if namespace.as_ref() != PRODUCTION_NAMESPACE.as_bytes() {
                    return None;
                }
                match prefix {
                    PrefixDeclaration::Named(prefix) => {
                        Some(String::from_utf8_lossy(prefix).into_owned())
                    }
                    PrefixDeclaration::Default => None,
                }
            })
            .collect::<BTreeSet<_>>();
        if !production_prefixes.is_empty() {
            graph.production_namespace = Some(PRODUCTION_NAMESPACE.to_owned());
        }
        graph.production_is_required = production_prefixes
            .iter()
            .any(|prefix| graph.required_extensions.contains(prefix));
    } else if ancestors.is_empty() {
        return Err(XmlFailure::Structure(
            "3MF model XML contains more than one top-level element.".to_owned(),
        ));
    }
    validate_model_element_position(reader, event, &element, ancestors, is_root)?;
    match element.as_str() {
        "resources" => {
            if std::mem::replace(&mut graph.has_resources, true) {
                return Err(XmlFailure::Structure(
                    "3MF model contains more than one resources element.".to_owned(),
                ));
            }
        }
        "build" => {
            if std::mem::replace(&mut graph.has_build, true) {
                return Err(XmlFailure::Structure(
                    "3MF model contains more than one build element.".to_owned(),
                ));
            }
        }
        "object" => {
            if reserve_graph_record(
                path,
                graph,
                retained_graph_records,
                max_graph_records,
                report,
            ) {
                match parse_positive_object_id(reader, event, b"id") {
                    Ok(object_id) if graph.objects.insert(object_id) => {}
                    Ok(object_id) => report.push(OutputValidationIssue::error(
                        OutputIssueCode::DuplicateObjectId,
                        Some(path),
                        format!("Object ID {object_id} appears more than once in this model part."),
                    )),
                    Err(message) => report.push(OutputValidationIssue::error(
                        OutputIssueCode::InvalidModelReference,
                        Some(path),
                        message,
                    )),
                }
            }
        }
        "component" | "item" => {
            graph.has_build_items |= element == "item";
            collect_model_reference(
                reader,
                event,
                path,
                element == "component",
                graph,
                retained_graph_records,
                max_graph_records,
                report,
            )?;
        }
        _ => {}
    }

    let production_uuid_element =
        matches!(element.as_str(), "build" | "item" | "object" | "component");
    let mut production_uuid = None;
    for attribute in event.attributes() {
        let attribute = attribute.map_err(|error| XmlFailure::Parse(error.to_string()))?;
        let (resolution, local_name) = reader.resolve_attribute(attribute.key);
        if local_name.as_ref() != b"UUID"
            || !matches!(
                resolution,
                ResolveResult::Bound(namespace)
                    if namespace.as_ref() == PRODUCTION_NAMESPACE.as_bytes()
            )
        {
            continue;
        }
        graph.uses_production_extension = true;
        if production_uuid.is_some() {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::InvalidProductionUuid,
                Some(path),
                format!(
                    "Element '{element}' carries the Production UUID attribute more than once through namespace aliases."
                ),
            ));
            continue;
        }
        if !production_uuid_element {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::InvalidProductionUuid,
                Some(path),
                format!("Element '{element}' cannot carry a Production UUID."),
            ));
        }
        if !reserve_graph_record(
            path,
            graph,
            retained_graph_records,
            max_graph_records,
            report,
        ) {
            continue;
        }
        let raw = attribute
            .decode_and_unescape_value(reader.decoder())
            .map_err(|error| XmlFailure::Parse(error.to_string()))?
            .into_owned();
        production_uuid = Some(raw.clone());
        match Uuid::parse_str(&raw) {
            Ok(uuid) => {
                if uuid.hyphenated().to_string() != raw {
                    report.push(OutputValidationIssue::error(
                        OutputIssueCode::InvalidProductionUuid,
                        Some(path),
                        format!(
                            "Production UUID '{raw}' must use canonical lowercase hyphenated form."
                        ),
                    ));
                }
                if let Some(first_path) = package_uuids.get(&uuid).cloned() {
                    report.push(
                        OutputValidationIssue::error(
                            OutputIssueCode::DuplicateProductionUuid,
                            Some(path),
                            format!("Production UUID '{raw}' is not unique in the package."),
                        )
                        .with_related_entry(first_path),
                    );
                } else {
                    package_uuids.insert(uuid, path.to_owned());
                }
            }
            Err(_) => report.push(OutputValidationIssue::error(
                OutputIssueCode::InvalidProductionUuid,
                Some(path),
                format!("Production UUID '{raw}' is malformed."),
            )),
        }
    }
    if production_uuid_element && production_uuid.is_none() {
        *graph.missing_production_uuids.entry(element).or_default() += 1;
    }
    Ok(())
}

fn validate_model_element_position<R: BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    element: &str,
    ancestors: &[String],
    is_root: bool,
) -> Result<(), XmlFailure> {
    let expected_parent = match element {
        "model" if is_root => None,
        "resources" | "build" => Some("model"),
        "object" => Some("resources"),
        "components" => Some("object"),
        "component" => Some("components"),
        "item" => Some("build"),
        _ => return Ok(()),
    };
    if event.name().as_ref() != element.as_bytes() {
        return Err(XmlFailure::Structure(format!(
            "Core 3MF element '{element}' must use the inherited core namespace without a prefix."
        )));
    }
    if let Some(namespace) = xml_attr(reader, event, b"xmlns")?
        && namespace != CORE_MODEL_NAMESPACE
    {
        return Err(XmlFailure::Structure(format!(
            "Core 3MF element '{element}' redeclares an invalid namespace '{namespace}'."
        )));
    }
    if let Some(expected_parent) = expected_parent
        && ancestors.last().map(String::as_str) != Some(expected_parent)
    {
        return Err(XmlFailure::Structure(format!(
            "Core 3MF element '{element}' must be a direct child of '{expected_parent}'."
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn collect_model_reference<R: BufRead>(
    reader: &NsReader<R>,
    event: &BytesStart<'_>,
    path: &str,
    is_component: bool,
    graph: &mut ModelPartGraph,
    retained_graph_records: &mut usize,
    max_graph_records: usize,
    report: &mut OutputValidationReport,
) -> Result<(), XmlFailure> {
    if !reserve_graph_record(
        path,
        graph,
        retained_graph_records,
        max_graph_records,
        report,
    ) {
        return Ok(());
    }
    let production_path = namespaced_xml_attr(reader, event, PRODUCTION_NAMESPACE, b"path")?;
    if let Some(path_value) = production_path.as_deref()
        && !path_value.starts_with('/')
    {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::InvalidProductionPath,
            Some(path),
            format!(
                "Production p:path '{path_value}' must be an absolute package path beginning with '/'."
            ),
        ));
    }
    graph.uses_production_extension |= production_path.is_some();
    graph.uses_production_path |= production_path.is_some();
    match parse_positive_object_id(reader, event, b"objectid") {
        Ok(object_id) => graph.references.push(ModelObjectReference {
            kind: if is_component {
                ModelReferenceKind::Component
            } else {
                ModelReferenceKind::BuildItem
            },
            object_id,
            production_path,
        }),
        Err(message) => report.push(OutputValidationIssue::error(
            OutputIssueCode::InvalidModelReference,
            Some(path),
            message,
        )),
    }
    Ok(())
}

fn reserve_graph_record(
    path: &str,
    graph: &mut ModelPartGraph,
    retained_graph_records: &mut usize,
    max_graph_records: usize,
    report: &mut OutputValidationReport,
) -> bool {
    if *retained_graph_records >= max_graph_records {
        if !graph.truncated {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::ModelGraphLimitExceeded,
                Some(path),
                format!(
                    "Production graph exceeds the {max_graph_records}-record validation limit."
                ),
            ));
        }
        graph.truncated = true;
        return false;
    }
    *retained_graph_records += 1;
    true
}

fn parse_positive_object_id<R: BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    attribute: &[u8],
) -> Result<u32, String> {
    let raw = required_xml_attr(reader, event, attribute).map_err(|error| error.to_string())?;
    let object_id = raw.parse::<u32>().map_err(|_| {
        format!(
            "Element '{}' has invalid {} value '{raw}'.",
            local_name(event.name().as_ref()),
            String::from_utf8_lossy(attribute)
        )
    })?;
    if object_id == 0 || object_id >= 2_147_483_648 {
        return Err(format!(
            "Element '{}' must use a resource ID in the range 1..2147483648.",
            local_name(event.name().as_ref())
        ));
    }
    Ok(object_id)
}

fn validate_model_part_shape(
    path: &str,
    graph: &ModelPartGraph,
    is_root_model: bool,
    package_uses_production: bool,
    report: &mut OutputValidationReport,
) {
    if !graph.root_is_model || !graph.has_resources || !graph.has_build {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::InvalidModel,
            Some(path),
            "3MF model part must have model, resources, and build elements.",
        ));
    }
    if graph.core_namespace.as_deref() != Some(CORE_MODEL_NAMESPACE) {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::InvalidModelNamespace,
            Some(path),
            format!("3MF model root must declare core namespace '{CORE_MODEL_NAMESPACE}'."),
        ));
    }
    if !is_root_model && graph.has_build_items {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::InvalidModel,
            Some(path),
            "A Production Extension child model must have an empty build element.",
        ));
    }
    if graph.uses_production_extension
        && graph.production_namespace.as_deref() != Some(PRODUCTION_NAMESPACE)
    {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::InvalidModelNamespace,
            Some(path),
            "Production attributes require a prefix bound to the official Production Extension namespace on the model root.",
        ));
    }
    if graph.uses_production_path && !graph.production_is_required {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::InvalidModelNamespace,
            Some(path),
            "A model that references objects through a Production path must list its Production namespace prefix in requiredextensions.",
        ));
    }
    if package_uses_production {
        for (element, count) in &graph.missing_production_uuids {
            if element == "build" && !is_root_model {
                continue;
            }
            report.push(OutputValidationIssue::error(
                OutputIssueCode::InvalidProductionUuid,
                Some(path),
                format!(
                    "Production model has {count} '{element}' element(s) without required p:UUID."
                ),
            ));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_model_reference(
    source_path: &str,
    source: &ModelPartGraph,
    reference: &ModelObjectReference,
    model_parts: &BTreeMap<String, ModelPartGraph>,
    entries: &BTreeMap<String, EntryMetadata>,
    relationships: &RelationshipGraph,
    report: &mut OutputValidationReport,
) {
    let kind = match reference.kind {
        ModelReferenceKind::Component => "Component",
        ModelReferenceKind::BuildItem => "Build item",
    };
    let Some(raw_target_path) = reference.production_path.as_deref() else {
        if !source.objects.contains(&reference.object_id) {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::MissingObjectReference,
                Some(source_path),
                format!(
                    "{kind} references missing local object ID {}.",
                    reference.object_id
                ),
            ));
        }
        return;
    };
    if !relationships.root_model_targets.contains(source_path) {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::ProductionPathOutsideRootModel,
            Some(source_path),
            "Production p:path is only permitted in a root-selected model part.",
        ));
    }
    let target_path = match resolve_internal_target(source_path, raw_target_path) {
        Ok(path) => path,
        Err(message) => {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::MissingProductionPathTarget,
                Some(source_path),
                message,
            ));
            return;
        }
    };
    if !entries.contains_key(&target_path) || !model_parts.contains_key(&target_path) {
        report.push(
            OutputValidationIssue::error(
                OutputIssueCode::MissingProductionPathTarget,
                Some(source_path),
                format!("Production path target '{target_path}' is not a model part."),
            )
            .with_related_entry(target_path.clone()),
        );
    }
    if !relationships.truncated
        && !relationships
            .model_targets_by_source
            .get(source_path)
            .is_some_and(|targets| targets.contains(&target_path))
    {
        report.push(
            OutputValidationIssue::error(
                OutputIssueCode::MissingProductionPathRelationship,
                Some(source_path),
                format!(
                    "Production path target '{target_path}' is not declared by the source model relationship part."
                ),
            )
            .with_related_entry(target_path.clone()),
        );
    }
    let Some(target) = model_parts.get(&target_path) else {
        return;
    };
    if !target.truncated && !target.objects.contains(&reference.object_id) {
        report.push(
            OutputValidationIssue::error(
                OutputIssueCode::MissingObjectReference,
                Some(source_path),
                format!(
                    "{kind} references missing object ID {} in '{target_path}'.",
                    reference.object_id
                ),
            )
            .with_related_entry(target_path),
        );
    }
}

fn validate_payload_documents<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    entries: &BTreeMap<String, EntryMetadata>,
    policy: &OutputValidationPolicy,
    model_part_paths: &BTreeSet<String>,
    report: &mut OutputValidationReport,
) {
    if policy.require_project_settings && !entries.contains_key(PROJECT_SETTINGS_PATH) {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::MissingProjectSettings,
            Some(PROJECT_SETTINGS_PATH),
            "Output package is missing project settings.",
        ));
    }
    if policy.require_model_settings && !entries.contains_key(MODEL_SETTINGS_PATH) {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::MissingModelSettings,
            Some(MODEL_SETTINGS_PATH),
            "Output package is missing model settings.",
        ));
    }

    for metadata in entries.values().filter(|entry| !entry.is_directory) {
        if !metadata.safe_to_inflate {
            continue;
        }
        if metadata.name == CONTENT_TYPES_PATH
            || metadata.name.to_ascii_lowercase().ends_with(".rels")
            || model_part_paths.contains(&metadata.name)
        {
            continue;
        }
        if metadata.name == PROJECT_SETTINGS_PATH || metadata.name.ends_with(".json") {
            validate_json_payload(archive, metadata, policy, report);
            continue;
        }
        if metadata.name == BAMBU_CUT_INFORMATION_PATH
            && matches!(
                policy.gui_save_artifacts,
                GuiSaveArtifactPolicy::BambuStudio2_2_0_85 { .. }
            )
        {
            validate_bambu_cut_information(archive, metadata, policy, report);
            continue;
        }
        let lower_name = metadata.name.to_ascii_lowercase();
        let is_xml = lower_name.ends_with(".xml")
            || metadata.name == MODEL_SETTINGS_PATH
            || (metadata.name == "Metadata/slice_info.config"
                && policy
                    .gui_save_artifacts
                    .allows_metadata_entry(&metadata.name));
        if is_xml {
            validate_xml_payload(archive, metadata, policy, report);
        }
    }
}

fn validate_json_payload<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    metadata: &EntryMetadata,
    policy: &OutputValidationPolicy,
    report: &mut OutputValidationReport,
) {
    let parse_limit = if metadata.name == PROJECT_SETTINGS_PATH {
        policy.limits.max_config_bytes
    } else {
        policy.limits.max_xml_json_parse_bytes
    };
    if metadata.uncompressed_size > parse_limit {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::EntryTooLarge,
            Some(&metadata.name),
            "JSON metadata exceeds the configured metadata limit.",
        ));
        return;
    }
    let mut entry = match archive.by_index(metadata.index) {
        Ok(entry) => entry,
        Err(error) => {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::EntryReadFailed,
                Some(&metadata.name),
                format!("Failed to open JSON metadata: {error}"),
            ));
            return;
        }
    };
    let mut data = Vec::with_capacity(metadata.uncompressed_size as usize);
    if let Err(error) = entry.read_to_end(&mut data) {
        let code = if is_crc_error(&error) {
            OutputIssueCode::ZipCrcMismatch
        } else {
            OutputIssueCode::EntryReadFailed
        };
        report.push(OutputValidationIssue::error(
            code,
            Some(&metadata.name),
            format!("Failed to read JSON metadata: {error}"),
        ));
        return;
    }
    let value = match serde_json::from_slice::<serde_json::Value>(&data) {
        Ok(value) => value,
        Err(error) => {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::InvalidJson,
                Some(&metadata.name),
                format!("JSON is not well formed: {error}"),
            ));
            return;
        }
    };
    if metadata.name == PROJECT_SETTINGS_PATH {
        validate_project_settings(&value, report);
    }
    validate_embedded_stale_json_references(&value, metadata, policy, report);
}

fn validate_bambu_cut_information<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    metadata: &EntryMetadata,
    policy: &OutputValidationPolicy,
    report: &mut OutputValidationReport,
) {
    if metadata.uncompressed_size > policy.limits.max_xml_json_parse_bytes {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::EntryTooLarge,
            Some(BAMBU_CUT_INFORMATION_PATH),
            "Bambu Studio cut information exceeds the configured XML metadata limit.",
        ));
        return;
    }
    let entry = match archive.by_index(metadata.index) {
        Ok(entry) => entry,
        Err(error) => {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::EntryReadFailed,
                Some(BAMBU_CUT_INFORMATION_PATH),
                format!("Failed to open Bambu Studio cut information: {error}"),
            ));
            return;
        }
    };
    match parse_bambu_cut_information(
        BufReader::new(entry),
        policy.limits.max_xml_depth,
        policy.limits.max_xml_token_bytes,
    ) {
        Ok(()) => {}
        Err(XmlFailure::Doctype) => report.push(OutputValidationIssue::error(
            OutputIssueCode::ForbiddenXmlDoctype,
            Some(BAMBU_CUT_INFORMATION_PATH),
            "DOCTYPE declarations are forbidden in Bambu Studio cut information.",
        )),
        Err(error) => report.push(OutputValidationIssue::error(
            OutputIssueCode::UnexpectedGuiSaveArtifact,
            Some(BAMBU_CUT_INFORMATION_PATH),
            format!(
                "Bambu Studio 02.02.00.85 cut information is outside the observed safe schema: {error}"
            ),
        )),
    }
}

fn parse_bambu_cut_information<R: BufRead>(
    source: R,
    max_depth: usize,
    max_token_bytes: usize,
) -> Result<(), XmlFailure> {
    let mut reader = Reader::from_reader(BufReader::new(XmlTokenLimitedReader::new(
        source,
        max_token_bytes,
    )));
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    let mut saw_root = false;
    let mut object_ids = BTreeSet::new();
    let mut current_object_has_cut_id = None::<bool>;

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(event)) => {
                depth = checked_xml_depth(depth, max_depth)?;
                match (depth, event.name().as_ref()) {
                    (1, b"objects") if !saw_root => {
                        if !xml_attributes(&reader, &event)?.is_empty() {
                            return Err(XmlFailure::Structure(
                                "The objects root must not contain attributes.".to_owned(),
                            ));
                        }
                        saw_root = true;
                    }
                    (2, b"object") if current_object_has_cut_id.is_none() => {
                        let attributes = xml_attributes(&reader, &event)?;
                        let Some(raw_id) = attributes.get("id") else {
                            return Err(XmlFailure::Structure(
                                "Each cut-information object requires an id.".to_owned(),
                            ));
                        };
                        if attributes.len() != 1 {
                            return Err(XmlFailure::Structure(
                                "A cut-information object may contain only the id attribute."
                                    .to_owned(),
                            ));
                        }
                        let id = raw_id
                            .parse::<u32>()
                            .ok()
                            .filter(|id| *id > 0)
                            .filter(|id| raw_id == &id.to_string())
                            .ok_or_else(|| {
                                XmlFailure::Structure(format!(
                                    "Cut-information object id '{raw_id}' is not a canonical positive integer."
                                ))
                            })?;
                        if !object_ids.insert(id) {
                            return Err(XmlFailure::Structure(format!(
                                "Cut-information object id '{id}' appears more than once."
                            )));
                        }
                        current_object_has_cut_id = Some(false);
                    }
                    _ => {
                        return Err(XmlFailure::Structure(format!(
                            "Unexpected non-empty element '{}' in Bambu Studio cut information.",
                            local_name(event.name().as_ref())
                        )));
                    }
                }
            }
            Ok(Event::Empty(event)) => {
                if depth != 2 || event.name().as_ref() != b"cut_id" {
                    return Err(XmlFailure::Structure(format!(
                        "Unexpected empty element '{}' in Bambu Studio cut information.",
                        local_name(event.name().as_ref())
                    )));
                }
                let Some(has_cut_id) = current_object_has_cut_id.as_mut() else {
                    return Err(XmlFailure::Structure(
                        "A cut_id element must be inside an object.".to_owned(),
                    ));
                };
                if *has_cut_id {
                    return Err(XmlFailure::Structure(
                        "Each cut-information object may contain only one cut_id.".to_owned(),
                    ));
                }
                let attributes = xml_attributes(&reader, &event)?;
                let expected = BTreeMap::from([
                    ("check_sum".to_owned(), "1".to_owned()),
                    ("connectors_cnt".to_owned(), "0".to_owned()),
                    ("id".to_owned(), "0".to_owned()),
                ]);
                if attributes != expected {
                    return Err(XmlFailure::Structure(
                        "cut_id must contain exactly id=0, check_sum=1, and connectors_cnt=0."
                            .to_owned(),
                    ));
                }
                *has_cut_id = true;
            }
            Ok(Event::End(event)) => {
                match (depth, event.name().as_ref()) {
                    (2, b"object") => {
                        if current_object_has_cut_id != Some(true) {
                            return Err(XmlFailure::Structure(
                                "Each cut-information object requires exactly one cut_id."
                                    .to_owned(),
                            ));
                        }
                        current_object_has_cut_id = None;
                    }
                    (1, b"objects") if current_object_has_cut_id.is_none() => {}
                    _ => {
                        return Err(XmlFailure::Structure(format!(
                            "Unexpected closing element '{}' in Bambu Studio cut information.",
                            local_name(event.name().as_ref())
                        )));
                    }
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Text(event)) => {
                validate_xml_text(&event)?;
                let decoded = event
                    .decode()
                    .map_err(|error| XmlFailure::Parse(error.to_string()))?;
                if !decoded.trim().is_empty() {
                    return Err(XmlFailure::Structure(
                        "Bambu Studio cut information may not contain text payloads.".to_owned(),
                    ));
                }
            }
            Ok(Event::Decl(_)) => {
                if saw_root || depth != 0 {
                    return Err(XmlFailure::Structure(
                        "The XML declaration must precede Bambu Studio cut information.".to_owned(),
                    ));
                }
            }
            Ok(Event::DocType(_)) => return Err(XmlFailure::Doctype),
            Ok(Event::Eof) => break,
            Ok(_) => {
                return Err(XmlFailure::Structure(
                    "Bambu Studio cut information contains an unsupported XML event.".to_owned(),
                ));
            }
            Err(error) => return Err(XmlFailure::Parse(error.to_string())),
        }
        buffer.clear();
    }

    if depth != 0 || !saw_root || current_object_has_cut_id.is_some() || object_ids.is_empty() {
        return Err(XmlFailure::Structure(
            "Bambu Studio cut information must contain one complete objects document.".to_owned(),
        ));
    }
    let expected_ids = (1..=u32::try_from(object_ids.len()).map_err(|_| {
        XmlFailure::Structure("Cut-information object count exceeds u32.".to_owned())
    })?)
        .collect::<BTreeSet<_>>();
    if object_ids != expected_ids {
        return Err(XmlFailure::Structure(
            "Cut-information object ids must be consecutive starting at 1.".to_owned(),
        ));
    }
    Ok(())
}

fn xml_attributes<R: BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
) -> Result<BTreeMap<String, String>, XmlFailure> {
    let mut attributes = BTreeMap::new();
    for attribute in event.attributes() {
        let attribute = attribute.map_err(|error| XmlFailure::Parse(error.to_string()))?;
        let name = String::from_utf8(attribute.key.as_ref().to_vec())
            .map_err(|error| XmlFailure::Parse(error.to_string()))?;
        let value = attribute
            .decode_and_unescape_value(reader.decoder())
            .map_err(|error| XmlFailure::Parse(error.to_string()))?
            .into_owned();
        if attributes.insert(name.clone(), value).is_some() {
            return Err(XmlFailure::Structure(format!(
                "Attribute '{name}' appears more than once."
            )));
        }
    }
    Ok(attributes)
}

fn validate_embedded_stale_json_references(
    value: &serde_json::Value,
    metadata: &EntryMetadata,
    policy: &OutputValidationPolicy,
    report: &mut OutputValidationReport,
) {
    let mut pending = vec![value];
    let mut references = BTreeSet::new();
    while let Some(value) = pending.pop() {
        match value {
            serde_json::Value::String(value) => {
                // Orca profile values such as
                // `{input_filename_base}_{filament_type[0]}_{print_time}.gcode`
                // are output-name templates, not references to a sliced part
                // already embedded in this unsliced 3MF package.
                if !is_orca_output_name_template(value) {
                    references.insert(value.as_str());
                }
            }
            serde_json::Value::Array(values) => pending.extend(values),
            serde_json::Value::Object(values) => pending.extend(values.values()),
            _ => {}
        }
    }
    for reference in references {
        let Some(classification) = policy.classify_stale_entry(reference) else {
            continue;
        };
        push_embedded_stale_reference(&metadata.name, &classification, report);
    }
}

fn is_orca_output_name_template(value: &str) -> bool {
    let Some(open) = value.find('{') else {
        return false;
    };
    value[open + 1..].contains('}')
}

fn push_embedded_stale_reference(
    container: &str,
    classification: &StaleEntryClassification,
    report: &mut OutputValidationReport,
) {
    let (code, message) = match classification.disposition {
        StaleArtifactDisposition::Reject => (
            OutputIssueCode::ForbiddenSlicedArtifact,
            format!(
                "Unsliced metadata references forbidden derived artifact '{}'.",
                classification.path
            ),
        ),
        StaleArtifactDisposition::RegenerateOrRemove => (
            OutputIssueCode::StaleDerivedArtifact,
            format!(
                "Metadata references preview '{}' that must be regenerated or removed.",
                classification.path
            ),
        ),
        StaleArtifactDisposition::Preserve | StaleArtifactDisposition::RequireRelationship => {
            return;
        }
    };
    report.push(
        OutputValidationIssue::error(code, Some(container), message)
            .with_related_entry(classification.path.clone()),
    );
}

fn validate_project_settings(value: &serde_json::Value, report: &mut OutputValidationReport) {
    let Some(settings) = value.as_object() else {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::InvalidProjectSettings,
            Some(PROJECT_SETTINGS_PATH),
            "Project settings root must be a JSON object.",
        ));
        return;
    };
    if !["printer_model", "printer_settings_id"]
        .iter()
        .any(|key| json_nonempty_scalar(settings.get(*key)))
    {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::InvalidProjectSettings,
            Some(PROJECT_SETTINGS_PATH),
            "Project settings must identify the target printer profile.",
        ));
    }
    if !json_nonempty_scalar(settings.get("print_settings_id")) {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::InvalidProjectSettings,
            Some(PROJECT_SETTINGS_PATH),
            "Project settings must identify the print process profile.",
        ));
    }
    if !json_positive_number_or_array(settings.get("nozzle_diameter")) {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::InvalidProjectSettings,
            Some(PROJECT_SETTINGS_PATH),
            "Project settings must declare at least one nozzle diameter.",
        ));
    }
    let filament_settings = json_nonempty_array_len(settings.get("filament_settings_id"));
    let filament_colours = json_nonempty_array_len(
        settings
            .get("filament_colour")
            .or_else(|| settings.get("filament_color")),
    );
    match (filament_settings, filament_colours) {
        (Some(settings_len), Some(colours_len)) if settings_len == colours_len => {}
        (Some(_), Some(_)) => report.push(OutputValidationIssue::error(
            OutputIssueCode::InvalidProjectSettings,
            Some(PROJECT_SETTINGS_PATH),
            "Filament profile and color arrays must have the same non-zero length.",
        )),
        _ => report.push(OutputValidationIssue::error(
            OutputIssueCode::InvalidProjectSettings,
            Some(PROJECT_SETTINGS_PATH),
            "Project settings must contain non-empty filament profile and color arrays.",
        )),
    }
    if !json_nonempty_string_array(settings.get("filament_settings_id")) {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::InvalidProjectSettings,
            Some(PROJECT_SETTINGS_PATH),
            "Every filament profile ID must be a non-empty string.",
        ));
    }
    let colour_value = settings
        .get("filament_colour")
        .or_else(|| settings.get("filament_color"));
    if !json_hex_colour_array(colour_value) {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::InvalidProjectSettings,
            Some(PROJECT_SETTINGS_PATH),
            "Every filament color must be a six-digit #RRGGBB value.",
        ));
    }
    if let (Some(types), Some(expected)) = (
        json_nonempty_array_len(settings.get("filament_type")),
        filament_settings,
    ) && types != expected
    {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::InvalidProjectSettings,
            Some(PROJECT_SETTINGS_PATH),
            "Filament type and profile arrays must have the same length.",
        ));
    }
    if settings.contains_key("filament_type")
        && !json_nonempty_string_array(settings.get("filament_type"))
    {
        report.push(OutputValidationIssue::error(
            OutputIssueCode::InvalidProjectSettings,
            Some(PROJECT_SETTINGS_PATH),
            "Every filament material type must be a non-empty string.",
        ));
    }
}

fn json_nonempty_scalar(value: Option<&serde_json::Value>) -> bool {
    match value {
        Some(serde_json::Value::String(value)) => !value.trim().is_empty(),
        Some(serde_json::Value::Number(_)) => true,
        _ => false,
    }
}

fn json_positive_number_or_array(value: Option<&serde_json::Value>) -> bool {
    fn positive(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Number(value) => value
                .as_f64()
                .is_some_and(|value| value.is_finite() && value > 0.0),
            serde_json::Value::String(value) => value
                .parse::<f64>()
                .is_ok_and(|value| value.is_finite() && value > 0.0),
            _ => false,
        }
    }
    match value {
        Some(serde_json::Value::Array(values)) => !values.is_empty() && values.iter().all(positive),
        Some(value) => positive(value),
        None => false,
    }
}

fn json_nonempty_string_array(value: Option<&serde_json::Value>) -> bool {
    value
        .and_then(serde_json::Value::as_array)
        .is_some_and(|values| {
            !values.is_empty()
                && values
                    .iter()
                    .all(|value| value.as_str().is_some_and(|value| !value.trim().is_empty()))
        })
}

fn json_hex_colour_array(value: Option<&serde_json::Value>) -> bool {
    value
        .and_then(serde_json::Value::as_array)
        .is_some_and(|values| {
            !values.is_empty()
                && values.iter().all(|value| {
                    value.as_str().is_some_and(|value| {
                        value.len() == 7
                            && value.starts_with('#')
                            && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
                })
        })
}

fn json_nonempty_array_len(value: Option<&serde_json::Value>) -> Option<usize> {
    value?
        .as_array()
        .filter(|values| !values.is_empty())
        .map(Vec::len)
}

fn validate_xml_payload<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    metadata: &EntryMetadata,
    policy: &OutputValidationPolicy,
    report: &mut OutputValidationReport,
) {
    let entry = match archive.by_index(metadata.index) {
        Ok(entry) => entry,
        Err(error) => {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::EntryReadFailed,
                Some(&metadata.name),
                format!("Failed to open XML part: {error}"),
            ));
            return;
        }
    };
    match parse_xml_summary(
        BufReader::new(entry),
        policy.limits.max_xml_depth,
        policy.limits.max_xml_token_bytes,
    ) {
        Ok(summary) if is_3d_model_entry(&metadata.name) => {
            if summary.root.as_deref() != Some("model")
                || !summary.elements.contains("resources")
                || !summary.elements.contains("build")
            {
                report.push(OutputValidationIssue::error(
                    OutputIssueCode::InvalidModel,
                    Some(&metadata.name),
                    "3MF model part must have model, resources, and build elements.",
                ));
            }
        }
        Ok(summary) if metadata.name == MODEL_SETTINGS_PATH => {
            let invalid_unkeyed_metadata = summary.metadata_without_key
                && (!policy.gui_save_artifacts.allows_bambu_face_count_metadata()
                    || summary.metadata_without_key_not_bambu_face_count);
            if summary.root.as_deref() != Some("config")
                || !summary.elements.contains("object")
                || !summary.elements.contains("plate")
                || invalid_unkeyed_metadata
            {
                report.push(OutputValidationIssue::error(
                    OutputIssueCode::InvalidModelSettings,
                    Some(MODEL_SETTINGS_PATH),
                    "Model settings must contain config, object, and plate elements; metadata requires a key.",
                ));
            }
            for reference in &summary.stale_references {
                if let Some(classification) = policy.classify_stale_entry(reference) {
                    push_embedded_stale_reference(MODEL_SETTINGS_PATH, &classification, report);
                }
            }
        }
        Ok(_) => {}
        Err(XmlFailure::Doctype) => report.push(OutputValidationIssue::error(
            OutputIssueCode::ForbiddenXmlDoctype,
            Some(&metadata.name),
            "DOCTYPE declarations are forbidden in package XML.",
        )),
        Err(error) => report.push(OutputValidationIssue::error(
            OutputIssueCode::InvalidXml,
            Some(&metadata.name),
            error.to_string(),
        )),
    }
}

#[derive(Clone, Debug, Default)]
struct XmlSummary {
    root: Option<String>,
    elements: BTreeSet<String>,
    metadata_without_key: bool,
    metadata_without_key_not_bambu_face_count: bool,
    stale_references: BTreeSet<String>,
}

#[derive(Debug)]
enum XmlFailure {
    Doctype,
    Parse(String),
    Structure(String),
}

fn validate_xml_text(event: &BytesText<'_>) -> Result<(), XmlFailure> {
    if !event.as_ref().contains(&b'&') {
        return Ok(());
    }
    let decoded = event
        .decode()
        .map_err(|error| XmlFailure::Parse(error.to_string()))?;
    quick_xml::escape::unescape(&decoded)
        .map(|_| ())
        .map_err(|error| XmlFailure::Parse(error.to_string()))
}

impl std::fmt::Display for XmlFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Doctype => formatter.write_str("DOCTYPE is forbidden."),
            Self::Parse(message) | Self::Structure(message) => formatter.write_str(message),
        }
    }
}

fn parse_xml_summary<R: BufRead>(
    source: R,
    max_depth: usize,
    max_token_bytes: usize,
) -> Result<XmlSummary, XmlFailure> {
    let mut reader = Reader::from_reader(BufReader::new(XmlTokenLimitedReader::new(
        source,
        max_token_bytes,
    )));
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    let mut summary = XmlSummary::default();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(event)) => {
                depth = checked_xml_depth(depth, max_depth)?;
                collect_xml_summary(&reader, &event, &mut summary)?;
            }
            Ok(Event::Empty(event)) => collect_xml_summary(&reader, &event, &mut summary)?,
            Ok(Event::End(_)) => depth = depth.saturating_sub(1),
            Ok(Event::DocType(_)) => return Err(XmlFailure::Doctype),
            Ok(Event::Text(event)) => validate_xml_text(&event)?,
            Ok(Event::Eof) => {
                if depth != 0 {
                    return Err(XmlFailure::Structure(
                        "XML part ended before all elements were closed.".to_owned(),
                    ));
                }
                break;
            }
            Ok(_) => {}
            Err(error) => return Err(XmlFailure::Parse(error.to_string())),
        }
        buffer.clear();
    }
    if summary.root.is_none() {
        return Err(XmlFailure::Structure(
            "XML part has no root element.".to_owned(),
        ));
    }
    Ok(summary)
}

fn collect_xml_summary<R: BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    summary: &mut XmlSummary,
) -> Result<(), XmlFailure> {
    let name = local_name(event.name().as_ref());
    summary.root.get_or_insert_with(|| name.clone());
    summary.elements.insert(name.clone());
    if name == "metadata" && xml_attr(reader, event, b"key")?.is_none() {
        summary.metadata_without_key = true;
        let attributes = event
            .attributes()
            .map(|attribute| {
                let attribute = attribute.map_err(|error| XmlFailure::Parse(error.to_string()))?;
                let name = attribute.key.as_ref().to_vec();
                let value = attribute
                    .decode_and_unescape_value(reader.decoder())
                    .map_err(|error| XmlFailure::Parse(error.to_string()))?
                    .into_owned();
                Ok((name, value))
            })
            .collect::<Result<Vec<_>, XmlFailure>>()?;
        let valid_face_count = matches!(
            attributes.as_slice(),
            [(name, value)]
                if name.as_slice() == b"face_count"
                    && value
                        .parse::<u64>()
                        .ok()
                        .filter(|count| *count > 0)
                        .is_some_and(|count| value == &count.to_string())
        );
        summary.metadata_without_key_not_bambu_face_count |= !valid_face_count;
    }
    for attribute in event.attributes() {
        let attribute = attribute.map_err(|error| XmlFailure::Parse(error.to_string()))?;
        let value = attribute
            .decode_and_unescape_value(reader.decoder())
            .map_err(|error| XmlFailure::Parse(error.to_string()))?;
        if crate::stale_artifact_policy::stale_artifact_kind(&value).is_some() {
            summary.stale_references.insert(value.into_owned());
        }
    }
    Ok(())
}

fn checked_xml_depth(depth: usize, max_depth: usize) -> Result<usize, XmlFailure> {
    let depth = depth
        .checked_add(1)
        .ok_or_else(|| XmlFailure::Structure("XML nesting depth overflowed.".to_owned()))?;
    if depth > max_depth {
        return Err(XmlFailure::Structure(format!(
            "XML nesting depth exceeds configured limit {max_depth}."
        )));
    }
    Ok(depth)
}

fn local_name(name: &[u8]) -> String {
    let local = name.rsplit(|byte| *byte == b':').next().unwrap_or(name);
    String::from_utf8_lossy(local).into_owned()
}

fn is_portable_relationship_id(value: &str) -> bool {
    let mut characters = value.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
}

fn required_xml_attr<R: BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    name: &[u8],
) -> Result<String, XmlFailure> {
    xml_attr(reader, event, name)?.ok_or_else(|| {
        XmlFailure::Structure(format!(
            "Element '{}' is missing required attribute '{}'.",
            local_name(event.name().as_ref()),
            String::from_utf8_lossy(name)
        ))
    })
}

fn xml_attr<R: BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    name: &[u8],
) -> Result<Option<String>, XmlFailure> {
    for attribute in event.attributes() {
        let attribute = attribute.map_err(|error| XmlFailure::Parse(error.to_string()))?;
        if attribute.key.as_ref() == name {
            return attribute
                .decode_and_unescape_value(reader.decoder())
                .map(|value| Some(value.into_owned()))
                .map_err(|error| XmlFailure::Parse(error.to_string()));
        }
    }
    Ok(None)
}

fn namespaced_xml_attr<R: BufRead>(
    reader: &NsReader<R>,
    event: &BytesStart<'_>,
    namespace: &str,
    attribute_local_name: &[u8],
) -> Result<Option<String>, XmlFailure> {
    let mut value = None;
    for attribute in event.attributes() {
        let attribute = attribute.map_err(|error| XmlFailure::Parse(error.to_string()))?;
        let (resolution, resolved_local_name) = reader.resolve_attribute(attribute.key);
        let is_match = resolved_local_name.as_ref() == attribute_local_name
            && matches!(
                resolution,
                ResolveResult::Bound(resolved_namespace)
                    if resolved_namespace.as_ref() == namespace.as_bytes()
            );
        if !is_match {
            continue;
        }
        if value.is_some() {
            return Err(XmlFailure::Structure(format!(
                "Element '{}' contains the namespaced attribute '{}' more than once.",
                local_name(event.name().as_ref()),
                String::from_utf8_lossy(attribute_local_name)
            )));
        }
        value = Some(
            attribute
                .decode_and_unescape_value(reader.decoder())
                .map_err(|error| XmlFailure::Parse(error.to_string()))?
                .into_owned(),
        );
    }
    Ok(value)
}

fn validate_thumbnails(
    entries: &BTreeMap<String, EntryMetadata>,
    relationships: &RelationshipGraph,
    report: &mut OutputValidationReport,
) {
    let thumbnails = report
        .stale_entries
        .iter()
        .filter(|entry| entry.kind == StaleArtifactKind::Thumbnail)
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    for thumbnail in thumbnails {
        if entries.contains_key(&thumbnail) && !relationships.thumbnail_targets.contains(&thumbnail)
        {
            report.push(OutputValidationIssue::error(
                OutputIssueCode::DanglingThumbnail,
                Some(&thumbnail),
                "Thumbnail part is not the target of a thumbnail relationship.",
            ));
        }
    }
}
