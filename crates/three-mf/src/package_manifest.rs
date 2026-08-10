use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    InputIdentity, OpcEntryOrigin, OpcWriteReport, OutputIssueSeverity, OutputValidationReport,
    ValidatedStagedPackage,
};

pub const PACKAGE_BUILD_MANIFEST_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageFileIdentity {
    pub byte_size: u64,
    pub sha256: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageEntryOrigin {
    Copied,
    Generated,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageManifestEntry {
    pub path: String,
    pub origin: PackageEntryOrigin,
    pub uncompressed_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovedPackageEntry {
    pub path: String,
    pub reason_code: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageValidationSummary {
    pub valid: bool,
    pub error_count: usize,
    pub warning_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageBuildManifest {
    schema_version: u32,
    source: PackageFileIdentity,
    output: PackageFileIdentity,
    entries: Vec<PackageManifestEntry>,
    removed_entries: Vec<RemovedPackageEntry>,
    validation: PackageValidationSummary,
}

#[derive(Debug, Error)]
pub enum PackageManifestError {
    #[error("manifest entry path {0:?} is duplicated")]
    DuplicateEntry(String),
    #[error("removed manifest entry path {0:?} is duplicated")]
    DuplicateRemovedEntry(String),
    #[error("manifest package path {path:?} is invalid: {reason}")]
    InvalidPackagePath { path: String, reason: &'static str },
    #[error("manifest entry {0:?} is both written and removed")]
    EntryAlsoRemoved(String),
    #[error("removed entry reason code {0:?} is invalid")]
    InvalidReasonCode(String),
    #[error("manifest SHA-256 value must be 64 lowercase hexadecimal characters: {0:?}")]
    InvalidSha256(String),
    #[error("manifest validation counts are inconsistent with valid={valid}")]
    InconsistentValidation { valid: bool },
    #[error("manifest source identity was not verified by the package writer")]
    UnverifiedSourceIdentity,
    #[error("unsupported package manifest schema version {0}")]
    UnsupportedSchemaVersion(u32),
    #[error("failed to serialize package manifest: {0}")]
    Serialize(#[from] serde_json::Error),
}

impl PackageBuildManifest {
    /// Builds a manifest from the exact staged bytes and validation report held
    /// by the publication capability. An arbitrary write report or caller-made
    /// validation summary cannot enter this public API.
    pub fn from_validated_package(
        source: &InputIdentity,
        package: &ValidatedStagedPackage,
        removed_entries: Vec<RemovedPackageEntry>,
    ) -> Result<Self, PackageManifestError> {
        Self::from_reports(
            source,
            package.report(),
            removed_entries,
            package.validation_report(),
        )
    }

    fn from_reports(
        source: &InputIdentity,
        report: &OpcWriteReport,
        removed_entries: Vec<RemovedPackageEntry>,
        validation_report: &OutputValidationReport,
    ) -> Result<Self, PackageManifestError> {
        if !report.verified_sources.iter().any(|verified| {
            verified.byte_size == source.byte_size && verified.sha256 == source.sha256
        }) {
            return Err(PackageManifestError::UnverifiedSourceIdentity);
        }
        let entries = report
            .entries
            .iter()
            .map(|entry| PackageManifestEntry {
                path: entry.path.clone(),
                origin: match &entry.origin {
                    OpcEntryOrigin::Generated => PackageEntryOrigin::Generated,
                    OpcEntryOrigin::Copied { .. } => PackageEntryOrigin::Copied,
                },
                uncompressed_bytes: entry.uncompressed_bytes,
            })
            .collect();
        Self::new(
            PackageFileIdentity::from(source),
            PackageFileIdentity::from(report),
            entries,
            removed_entries,
            PackageValidationSummary::from(validation_report),
        )
    }

    fn new(
        source: PackageFileIdentity,
        output: PackageFileIdentity,
        mut entries: Vec<PackageManifestEntry>,
        mut removed_entries: Vec<RemovedPackageEntry>,
        validation: PackageValidationSummary,
    ) -> Result<Self, PackageManifestError> {
        validate_sha256(&source.sha256)?;
        validate_sha256(&output.sha256)?;
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        removed_entries.sort_by(|left, right| left.path.cmp(&right.path));
        for entry in &entries {
            validate_package_path(&entry.path)?;
        }
        for entry in &removed_entries {
            validate_package_path(&entry.path)?;
            validate_reason_code(&entry.reason_code)?;
        }
        ensure_unique_entries(&entries)?;
        ensure_unique_removed_entries(&removed_entries)?;
        ensure_written_and_removed_are_disjoint(&entries, &removed_entries)?;
        if validation.valid != (validation.error_count == 0) {
            return Err(PackageManifestError::InconsistentValidation {
                valid: validation.valid,
            });
        }
        Ok(Self {
            schema_version: PACKAGE_BUILD_MANIFEST_SCHEMA_VERSION,
            source,
            output,
            entries,
            removed_entries,
            validation,
        })
    }

    pub fn to_canonical_json(&self) -> Result<Vec<u8>, PackageManifestError> {
        self.validate()?;
        let mut json = serde_json::to_vec_pretty(self)?;
        json.push(b'\n');
        Ok(json)
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn source(&self) -> &PackageFileIdentity {
        &self.source
    }

    pub fn output(&self) -> &PackageFileIdentity {
        &self.output
    }

    pub fn entries(&self) -> &[PackageManifestEntry] {
        &self.entries
    }

    pub fn removed_entries(&self) -> &[RemovedPackageEntry] {
        &self.removed_entries
    }

    pub fn validation(&self) -> &PackageValidationSummary {
        &self.validation
    }

    fn validate(&self) -> Result<(), PackageManifestError> {
        if self.schema_version != PACKAGE_BUILD_MANIFEST_SCHEMA_VERSION {
            return Err(PackageManifestError::UnsupportedSchemaVersion(
                self.schema_version,
            ));
        }
        validate_sha256(&self.source.sha256)?;
        validate_sha256(&self.output.sha256)?;
        for entry in &self.entries {
            validate_package_path(&entry.path)?;
        }
        for entry in &self.removed_entries {
            validate_package_path(&entry.path)?;
            validate_reason_code(&entry.reason_code)?;
        }
        ensure_unique_entries(&self.entries)?;
        ensure_unique_removed_entries(&self.removed_entries)?;
        ensure_written_and_removed_are_disjoint(&self.entries, &self.removed_entries)?;
        if self.validation.valid != (self.validation.error_count == 0) {
            return Err(PackageManifestError::InconsistentValidation {
                valid: self.validation.valid,
            });
        }
        Ok(())
    }
}

impl From<&OutputValidationReport> for PackageValidationSummary {
    fn from(report: &OutputValidationReport) -> Self {
        let error_count = report
            .issues
            .iter()
            .filter(|issue| issue.severity == OutputIssueSeverity::Error)
            .count();
        let warning_count = report
            .issues
            .iter()
            .filter(|issue| issue.severity == OutputIssueSeverity::Warning)
            .count();
        Self {
            valid: report.is_valid && error_count == 0,
            error_count,
            warning_count,
        }
    }
}

impl From<&InputIdentity> for PackageFileIdentity {
    fn from(identity: &InputIdentity) -> Self {
        Self {
            byte_size: identity.byte_size,
            sha256: identity.sha256.clone(),
        }
    }
}

impl From<&OpcWriteReport> for PackageFileIdentity {
    fn from(report: &OpcWriteReport) -> Self {
        Self {
            byte_size: report.package_bytes,
            sha256: report.package_sha256.clone(),
        }
    }
}

fn validate_sha256(value: &str) -> Result<(), PackageManifestError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(PackageManifestError::InvalidSha256(value.to_owned()));
    }
    Ok(())
}

fn ensure_unique_entries(entries: &[PackageManifestEntry]) -> Result<(), PackageManifestError> {
    let mut paths = BTreeSet::new();
    for entry in entries {
        if !paths.insert(entry.path.as_str()) {
            return Err(PackageManifestError::DuplicateEntry(entry.path.clone()));
        }
    }
    Ok(())
}

fn ensure_unique_removed_entries(
    entries: &[RemovedPackageEntry],
) -> Result<(), PackageManifestError> {
    let mut paths = BTreeSet::new();
    for entry in entries {
        if !paths.insert(entry.path.as_str()) {
            return Err(PackageManifestError::DuplicateRemovedEntry(
                entry.path.clone(),
            ));
        }
    }
    Ok(())
}

fn ensure_written_and_removed_are_disjoint(
    entries: &[PackageManifestEntry],
    removed_entries: &[RemovedPackageEntry],
) -> Result<(), PackageManifestError> {
    let written = entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<BTreeSet<_>>();
    if let Some(entry) = removed_entries
        .iter()
        .find(|entry| written.contains(entry.path.as_str()))
    {
        return Err(PackageManifestError::EntryAlsoRemoved(entry.path.clone()));
    }
    Ok(())
}

fn validate_package_path(path: &str) -> Result<(), PackageManifestError> {
    let invalid = |reason| PackageManifestError::InvalidPackagePath {
        path: path.to_owned(),
        reason,
    };
    if path.is_empty() {
        return Err(invalid("path is empty"));
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return Err(invalid("absolute paths are forbidden"));
    }
    if path.contains('\\')
        || path.contains('\0')
        || path.contains('?')
        || path.contains('#')
        || path.contains('%')
        || path.chars().any(char::is_control)
    {
        return Err(invalid("path contains a forbidden character"));
    }
    if path.split('/').any(|segment| {
        segment.is_empty() || segment == "." || segment == ".." || segment.contains(':')
    }) {
        return Err(invalid("path contains an unsafe segment"));
    }
    Ok(())
}

fn validate_reason_code(code: &str) -> Result<(), PackageManifestError> {
    let mut bytes = code.bytes();
    let valid = matches!(bytes.next(), Some(first) if first.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    if !valid {
        return Err(PackageManifestError::InvalidReasonCode(code.to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn identity(fill: char) -> PackageFileIdentity {
        PackageFileIdentity {
            byte_size: 42,
            sha256: fill.to_string().repeat(64),
        }
    }

    #[test]
    fn canonical_manifest_sorts_entries_and_is_byte_stable() {
        let manifest = PackageBuildManifest::new(
            identity('a'),
            identity('b'),
            vec![
                PackageManifestEntry {
                    path: "Metadata/project_settings.config".to_owned(),
                    origin: PackageEntryOrigin::Generated,
                    uncompressed_bytes: 20,
                },
                PackageManifestEntry {
                    path: "3D/3dmodel.model".to_owned(),
                    origin: PackageEntryOrigin::Copied,
                    uncompressed_bytes: 100,
                },
            ],
            vec![RemovedPackageEntry {
                path: "Metadata/plate_1.gcode".to_owned(),
                reason_code: "sliced_artifact".to_owned(),
            }],
            PackageValidationSummary {
                valid: true,
                error_count: 0,
                warning_count: 1,
            },
        )
        .expect("valid manifest");

        assert_eq!(manifest.entries[0].path, "3D/3dmodel.model");
        assert_eq!(
            manifest.to_canonical_json().expect("serialize manifest"),
            manifest.to_canonical_json().expect("serialize manifest")
        );
    }

    #[test]
    fn manifest_rejects_duplicates_invalid_hashes_and_inconsistent_validation() {
        let duplicate = PackageManifestEntry {
            path: "3D/3dmodel.model".to_owned(),
            origin: PackageEntryOrigin::Copied,
            uncompressed_bytes: 1,
        };
        let error = PackageBuildManifest::new(
            identity('a'),
            identity('b'),
            vec![duplicate.clone(), duplicate],
            Vec::new(),
            PackageValidationSummary {
                valid: true,
                error_count: 0,
                warning_count: 0,
            },
        )
        .expect_err("duplicate entries must fail");
        assert!(matches!(error, PackageManifestError::DuplicateEntry(_)));

        let error = PackageBuildManifest::new(
            PackageFileIdentity {
                byte_size: 1,
                sha256: "ABC".to_owned(),
            },
            identity('b'),
            Vec::new(),
            Vec::new(),
            PackageValidationSummary {
                valid: true,
                error_count: 0,
                warning_count: 0,
            },
        )
        .expect_err("invalid hashes must fail");
        assert!(matches!(error, PackageManifestError::InvalidSha256(_)));

        let error = PackageBuildManifest::new(
            identity('a'),
            identity('b'),
            Vec::new(),
            Vec::new(),
            PackageValidationSummary {
                valid: true,
                error_count: 1,
                warning_count: 0,
            },
        )
        .expect_err("valid with errors must fail");
        assert!(matches!(
            error,
            PackageManifestError::InconsistentValidation { .. }
        ));
    }

    #[test]
    fn manifest_from_write_report_does_not_serialize_host_paths() {
        let source = InputIdentity {
            byte_size: 10,
            sha256: "a".repeat(64),
        };
        let report = OpcWriteReport {
            destination: PathBuf::from("/private/output/project.3mf"),
            package_bytes: 20,
            package_sha256: "b".repeat(64),
            total_uncompressed_bytes: 5,
            entries: vec![crate::OpcEntryWriteReport {
                path: "3D/Objects/part.model".to_owned(),
                origin: OpcEntryOrigin::Copied {
                    source_archive: PathBuf::from("/private/source/project.3mf"),
                    source_entry: "3D/Objects/part.model".to_owned(),
                    raw: true,
                },
                uncompressed_bytes: 5,
            }],
            verified_sources: vec![crate::VerifiedSourceIdentity {
                byte_size: source.byte_size,
                sha256: source.sha256.clone(),
            }],
        };
        let validation = OutputValidationReport::default();
        let manifest =
            PackageBuildManifest::from_reports(&source, &report, Vec::new(), &validation)
                .expect("valid manifest");
        let json = String::from_utf8(manifest.to_canonical_json().unwrap()).unwrap();
        assert!(!json.contains("/private/"));
        assert!(json.contains("3D/Objects/part.model"));

        let forged_source = InputIdentity {
            byte_size: source.byte_size,
            sha256: "c".repeat(64),
        };
        assert!(matches!(
            PackageBuildManifest::from_reports(&forged_source, &report, Vec::new(), &validation,),
            Err(PackageManifestError::UnverifiedSourceIdentity)
        ));
    }

    #[test]
    fn manifest_rejects_unsafe_overlap_and_invalid_reason_codes() {
        let validation = PackageValidationSummary {
            valid: true,
            error_count: 0,
            warning_count: 0,
        };
        let unsafe_path = PackageBuildManifest::new(
            identity('a'),
            identity('b'),
            vec![PackageManifestEntry {
                path: "../escape".to_owned(),
                origin: PackageEntryOrigin::Generated,
                uncompressed_bytes: 1,
            }],
            Vec::new(),
            validation.clone(),
        )
        .expect_err("unsafe path must fail");
        assert!(matches!(
            unsafe_path,
            PackageManifestError::InvalidPackagePath { .. }
        ));

        let overlap = PackageBuildManifest::new(
            identity('a'),
            identity('b'),
            vec![PackageManifestEntry {
                path: "entry".to_owned(),
                origin: PackageEntryOrigin::Generated,
                uncompressed_bytes: 1,
            }],
            vec![RemovedPackageEntry {
                path: "entry".to_owned(),
                reason_code: "stale_artifact".to_owned(),
            }],
            validation.clone(),
        )
        .expect_err("entry cannot be written and removed");
        assert!(matches!(overlap, PackageManifestError::EntryAlsoRemoved(_)));

        let invalid_reason = PackageBuildManifest::new(
            identity('a'),
            identity('b'),
            Vec::new(),
            vec![RemovedPackageEntry {
                path: "entry".to_owned(),
                reason_code: "Human readable".to_owned(),
            }],
            validation,
        )
        .expect_err("reason code must be stable machine text");
        assert!(matches!(
            invalid_reason,
            PackageManifestError::InvalidReasonCode(_)
        ));
    }
}
