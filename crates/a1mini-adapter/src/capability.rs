use quick_xml::Reader;
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::{
    A1MINI_ADAPTER_ID, A1MINI_APPLICATION_VERSION, A1MINI_EXECUTABLE_SHA256,
    A1MINI_PROFILE_BASELINE, A1MINI_PROFILE_MANIFEST_SHA256, A1MINI_PROFILE_PACK_VERSION,
    BBL_SYSTEM_DIRECTORY, BBL_VENDOR_NAME,
};

const QUALIFICATION_RECORD: &[u8] = include_bytes!("../qualification/a1mini-02.02.00.85.json");
const QUALIFICATION_REPORT: &[u8] =
    include_bytes!("../qualification/a1mini-02.02.00.85-report.json");

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum A1MiniCapabilityStatus {
    Qualified,
    StructurallyReadyNeedsGuiQualification,
    UnsupportedInstallation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A1MiniProfileEvidence {
    pub relative_path: String,
    pub sha256: String,
    pub valid: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A1MiniProfilePackEvidence {
    pub source: String,
    pub version: Option<String>,
    pub manifest_sha256: String,
    pub valid: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A1MiniCapabilityReport {
    pub adapter_id: String,
    pub status: A1MiniCapabilityStatus,
    pub conversion_available: bool,
    pub application_path: PathBuf,
    pub application_version: Option<String>,
    pub executable_sha256: Option<String>,
    pub profile_pack: A1MiniProfilePackEvidence,
    pub profiles: Vec<A1MiniProfileEvidence>,
    pub supported_materials: Vec<String>,
    pub single_plate_only: bool,
    pub issues: Vec<String>,
}

impl A1MiniCapabilityReport {
    #[must_use]
    pub fn installation_supported(&self) -> bool {
        !matches!(self.status, A1MiniCapabilityStatus::UnsupportedInstallation)
    }
}

#[derive(Debug, Error)]
pub enum A1MiniError {
    #[error("Bambu Lab A1 mini conversion is unavailable: {0}")]
    Capability(String),
    #[error("the canonical A1 mini plate cannot be converted: {0}")]
    Plan(String),
    #[error("the source 3MF changed after analysis")]
    SourceIdentityChanged,
    #[error("conversion_cancelled: A1 mini conversion was cancelled before publication.")]
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
    #[error("invalid JSON in {path}: {source}")]
    Json {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid XML in {path}: {message}")]
    Xml { path: String, message: String },
    #[error("invalid ZIP package: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("failed to construct the target OPC package: {0}")]
    Opc(String),
    #[error("generated A1 mini project failed semantic validation: {0}")]
    SemanticValidation(String),
}

#[derive(Clone, Debug)]
pub(crate) struct AdapterContext {
    pub(crate) profiles_root: PathBuf,
    pub(crate) manifest_path: PathBuf,
    pub(crate) capability: A1MiniCapabilityReport,
}

#[derive(Debug, Deserialize)]
struct VendorManifest {
    name: String,
    version: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QualificationRecord {
    schema_version: u32,
    adapter_id: String,
    application_version: String,
    executable_sha256: String,
    profile_source: String,
    profile_pack_version: String,
    profile_manifest_sha256: String,
    gui_round_trip_passed: bool,
    status: String,
    evidence: Option<QualificationRecordEvidence>,
    note: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QualificationRecordEvidence {
    qualification_report_sha256: String,
    qualified_at_utc: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QualificationReportDocument {
    schema_version: u32,
    adapter_id: String,
    application_version: String,
    executable_sha256: String,
    profile_source: String,
    profile_pack_version: String,
    profile_manifest_sha256: String,
    status: String,
    qualification: Option<QualificationReportEvidence>,
    note: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QualificationReportEvidence {
    pla: MaterialRoundTripEvidence,
    petg: MaterialRoundTripEvidence,
    profiles: Vec<QualificationProfile>,
    checks: QualificationChecks,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MaterialRoundTripEvidence {
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
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QualificationProfile {
    relative_path: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QualificationChecks {
    machine_profile_loaded: bool,
    process_profile_loaded: bool,
    single_external_spool_verified: bool,
    no_ams_verified: bool,
    one_plate_per_output_verified: bool,
    geometry_and_placements_stable: bool,
    material_identity_stable: bool,
    no_repair_warning: bool,
    no_incompatible_profile_warning: bool,
    no_missing_profile_warning: bool,
    no_custom_profile_warning: bool,
    no_stale_slice_artifacts: bool,
}

impl QualificationChecks {
    fn all_passed(&self) -> bool {
        self.machine_profile_loaded
            && self.process_profile_loaded
            && self.single_external_spool_verified
            && self.no_ams_verified
            && self.one_plate_per_output_verified
            && self.geometry_and_placements_stable
            && self.material_identity_stable
            && self.no_repair_warning
            && self.no_incompatible_profile_warning
            && self.no_missing_profile_warning
            && self.no_custom_profile_warning
            && self.no_stale_slice_artifacts
    }
}

impl MaterialRoundTripEvidence {
    fn valid(&self) -> bool {
        !self.fixture_name.trim().is_empty()
            && valid_nonzero_sha256(&self.source_fixture_sha256)
            && valid_nonzero_sha256(&self.writer_candidate_sha256)
            && valid_nonzero_sha256(&self.first_gui_saved_sha256)
            && valid_nonzero_sha256(&self.reopened_gui_saved_sha256)
            && self.opened
            && self.sliced
            && self.saved
            && self.closed
            && self.reopened
            && self.resliced
            && self.resaved
    }
}

pub fn inspect_a1mini_macos_application(
    application_path: &Path,
) -> Result<A1MiniCapabilityReport, A1MiniError> {
    Ok(adapter_context(application_path)?.capability)
}

pub fn conventional_bambu_studio_paths() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from("/Applications/BambuStudio.app")];
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(home).join("Applications/BambuStudio.app"));
    }
    paths
}

pub fn discover_bambu_studio() -> Option<PathBuf> {
    conventional_bambu_studio_paths()
        .into_iter()
        .find(|path| path.is_dir())
}

pub(crate) fn adapter_context(application_path: &Path) -> Result<AdapterContext, A1MiniError> {
    let contents = application_path.join("Contents");
    if !application_path.is_dir() {
        return Err(A1MiniError::Capability(format!(
            "Bambu Studio application was not found at {}",
            application_path.display()
        )));
    }
    let plist_path = contents.join("Info.plist");
    let plist = fs::read(&plist_path).map_err(|source| A1MiniError::Read {
        path: plist_path.clone(),
        source,
    })?;
    let version = plist_string(&plist_path, &plist, "CFBundleShortVersionString")?;
    let executable_name = plist_string(&plist_path, &plist, "CFBundleExecutable")?;
    let executable_path = contents.join("MacOS").join(executable_name);
    let executable_sha256 = hash_path(&executable_path)?;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    adapter_context_with_home(
        application_path,
        version,
        executable_sha256,
        home.as_deref(),
    )
}

fn adapter_context_with_home(
    application_path: &Path,
    version: String,
    executable_sha256: String,
    home: Option<&Path>,
) -> Result<AdapterContext, A1MiniError> {
    let home = home.filter(|path| path.is_absolute()).ok_or_else(|| {
        A1MiniError::Capability(
            "HOME is unavailable or relative, so Bambu Studio's active system profile directory cannot be resolved safely"
                .into(),
        )
    })?;
    // Bambu Studio 02.02.00.85 uses wxWidgets' per-user data directory on
    // macOS. Unlike Orca's portable mode, this release has no adjacent
    // data_dir override in its GUI initialization path.
    let system_root = home
        .join("Library/Application Support/BambuStudio")
        .join(BBL_SYSTEM_DIRECTORY);
    let manifest_path = system_root.join(format!("{BBL_VENDOR_NAME}.json"));
    let profiles_root = system_root.join(BBL_VENDOR_NAME);
    let mut issues = Vec::new();
    if version != A1MINI_APPLICATION_VERSION {
        issues.push(format!(
            "Bambu Studio {version} is installed; this adapter requires exact version {A1MINI_APPLICATION_VERSION}."
        ));
    }
    if executable_sha256 != A1MINI_EXECUTABLE_SHA256 {
        issues.push(format!(
            "The Bambu Studio executable hash is {executable_sha256}, expected {A1MINI_EXECUTABLE_SHA256}."
        ));
    }
    let profile_pack = inspect_profile_pack(&manifest_path, &mut issues);
    let profiles = inspect_profile_baseline(&profiles_root, &mut issues);
    let installation_supported = issues.is_empty();
    let qualified = installation_supported && qualification_bundle_is_valid();
    if installation_supported && !qualified {
        issues.push(
            "The A1 mini writer is structurally ready for this exact installation, but clean PLA and PETG GUI save/reopen/slice qualification evidence is not installed."
                .into(),
        );
    }
    let status = if !installation_supported {
        A1MiniCapabilityStatus::UnsupportedInstallation
    } else if qualified {
        A1MiniCapabilityStatus::Qualified
    } else {
        A1MiniCapabilityStatus::StructurallyReadyNeedsGuiQualification
    };
    Ok(AdapterContext {
        profiles_root,
        manifest_path,
        capability: A1MiniCapabilityReport {
            adapter_id: A1MINI_ADAPTER_ID.into(),
            status,
            conversion_available: qualified,
            application_path: application_path.to_path_buf(),
            application_version: Some(version),
            executable_sha256: Some(executable_sha256),
            profile_pack,
            profiles,
            supported_materials: vec!["PLA".into(), "PETG".into()],
            single_plate_only: true,
            issues,
        },
    })
}

fn inspect_profile_pack(
    manifest_path: &Path,
    issues: &mut Vec<String>,
) -> A1MiniProfilePackEvidence {
    let bytes = match fs::read(manifest_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            issues.push(format!(
                "The active BBL system profile manifest is missing or unreadable: {error}."
            ));
            return A1MiniProfilePackEvidence {
                source: "installed_system".into(),
                version: None,
                manifest_sha256: String::new(),
                valid: false,
            };
        }
    };
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let manifest = serde_json::from_slice::<VendorManifest>(&bytes);
    let version = manifest.as_ref().ok().map(|value| value.version.clone());
    let mut valid = true;
    match manifest {
        Ok(manifest) => {
            if manifest.name != "Bambulab" {
                valid = false;
                issues.push(format!(
                    "The active BBL manifest declares vendor {:?}, expected \"Bambulab\".",
                    manifest.name
                ));
            }
            if manifest.version != A1MINI_PROFILE_PACK_VERSION {
                valid = false;
                issues.push(format!(
                    "The active BBL profile pack is version {}, expected {}.",
                    manifest.version, A1MINI_PROFILE_PACK_VERSION
                ));
            }
        }
        Err(error) => {
            valid = false;
            issues.push(format!(
                "The active BBL system profile manifest is invalid JSON: {error}."
            ));
        }
    }
    if sha256 != A1MINI_PROFILE_MANIFEST_SHA256 {
        valid = false;
        issues.push(format!(
            "The active BBL profile manifest hash is {sha256}, expected {A1MINI_PROFILE_MANIFEST_SHA256}."
        ));
    }
    A1MiniProfilePackEvidence {
        source: "installed_system".into(),
        version,
        manifest_sha256: sha256,
        valid,
    }
}

fn inspect_profile_baseline(
    profiles_root: &Path,
    issues: &mut Vec<String>,
) -> Vec<A1MiniProfileEvidence> {
    A1MINI_PROFILE_BASELINE
        .iter()
        .map(|(relative_path, expected_hash)| {
            let actual_hash = fs::read(profiles_root.join(relative_path))
                .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
                .unwrap_or_default();
            let valid = actual_hash == *expected_hash;
            if !valid {
                if actual_hash.is_empty() {
                    issues.push(format!(
                        "Required A1 mini profile {relative_path} is missing or unreadable."
                    ));
                } else {
                    issues.push(format!(
                        "Required A1 mini profile {relative_path} has hash {actual_hash}, expected {expected_hash}."
                    ));
                }
            }
            A1MiniProfileEvidence {
                relative_path: (*relative_path).into(),
                sha256: actual_hash,
                valid,
            }
        })
        .collect()
}

pub(crate) fn qualification_bundle_is_valid() -> bool {
    qualification_bundle_bytes_are_valid(QUALIFICATION_RECORD, QUALIFICATION_REPORT)
}

fn qualification_bundle_bytes_are_valid(record_bytes: &[u8], report_bytes: &[u8]) -> bool {
    let Ok(record) = serde_json::from_slice::<QualificationRecord>(record_bytes) else {
        return false;
    };
    let Ok(report) = serde_json::from_slice::<QualificationReportDocument>(report_bytes) else {
        return false;
    };
    let (Some(record_evidence), Some(report_evidence)) = (record.evidence, report.qualification)
    else {
        return false;
    };
    let report_hash = format!("{:x}", Sha256::digest(report_bytes));
    let profiles_match = report_evidence.profiles.len() == A1MINI_PROFILE_BASELINE.len()
        && report_evidence
            .profiles
            .iter()
            .zip(A1MINI_PROFILE_BASELINE)
            .all(|(actual, expected)| {
                actual.relative_path == expected.0 && actual.sha256 == expected.1
            });
    let common = |schema_version: u32,
                  adapter_id: &str,
                  application_version: &str,
                  executable_sha256: &str,
                  profile_source: &str,
                  profile_pack_version: &str,
                  profile_manifest_sha256: &str,
                  status: &str,
                  note: &str| {
        schema_version == 1
            && adapter_id == A1MINI_ADAPTER_ID
            && application_version == A1MINI_APPLICATION_VERSION
            && executable_sha256 == A1MINI_EXECUTABLE_SHA256
            && profile_source == "installed_system"
            && profile_pack_version == A1MINI_PROFILE_PACK_VERSION
            && profile_manifest_sha256 == A1MINI_PROFILE_MANIFEST_SHA256
            && status == "qualified"
            && !note.trim().is_empty()
    };
    common(
        record.schema_version,
        &record.adapter_id,
        &record.application_version,
        &record.executable_sha256,
        &record.profile_source,
        &record.profile_pack_version,
        &record.profile_manifest_sha256,
        &record.status,
        &record.note,
    ) && common(
        report.schema_version,
        &report.adapter_id,
        &report.application_version,
        &report.executable_sha256,
        &report.profile_source,
        &report.profile_pack_version,
        &report.profile_manifest_sha256,
        &report.status,
        &report.note,
    ) && record.gui_round_trip_passed
        && record_evidence.qualification_report_sha256 == report_hash
        && valid_nonzero_sha256(&record_evidence.qualification_report_sha256)
        && !record_evidence.qualified_at_utc.trim().is_empty()
        && report_evidence.pla.valid()
        && report_evidence.petg.valid()
        && profiles_match
        && report_evidence.checks.all_passed()
}

fn valid_nonzero_sha256(value: &str) -> bool {
    value.len() == 64
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        && value.bytes().any(|byte| byte != b'0')
}

fn hash_path(path: &Path) -> Result<String, A1MiniError> {
    let bytes = fs::read(path).map_err(|source| A1MiniError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn plist_string(path: &Path, xml: &[u8], wanted_key: &'static str) -> Result<String, A1MiniError> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut last_key: Option<String> = None;
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(start)) if start.name().as_ref() == b"key" => {
                let value = reader
                    .read_text(start.name())
                    .map_err(|error| A1MiniError::Xml {
                        path: path.to_string_lossy().into_owned(),
                        message: error.to_string(),
                    })?;
                last_key = Some(value.into_owned());
            }
            Ok(Event::Start(start)) if start.name().as_ref() == b"string" => {
                let value = reader
                    .read_text(start.name())
                    .map_err(|error| A1MiniError::Xml {
                        path: path.to_string_lossy().into_owned(),
                        message: error.to_string(),
                    })?;
                if last_key.as_deref() == Some(wanted_key) {
                    return Ok(value.into_owned());
                }
                last_key = None;
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => {
                return Err(A1MiniError::Xml {
                    path: path.to_string_lossy().into_owned(),
                    message: error.to_string(),
                });
            }
        }
        buffer.clear();
    }
    Err(A1MiniError::Capability(format!(
        "property {wanted_key} is missing from {}",
        path.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_qualification_is_hash_bound_and_valid() {
        assert!(qualification_bundle_is_valid());
    }

    #[test]
    fn qualification_fails_closed_when_report_bytes_change() {
        let mut report = QUALIFICATION_REPORT.to_vec();
        report.push(b' ');
        assert!(!qualification_bundle_bytes_are_valid(
            QUALIFICATION_RECORD,
            &report
        ));
    }

    #[test]
    fn qualification_fails_closed_when_the_release_gate_is_downgraded() {
        let record = String::from_utf8(QUALIFICATION_RECORD.to_vec())
            .unwrap()
            .replace(
                "\"guiRoundTripPassed\": true",
                "\"guiRoundTripPassed\": false",
            );
        assert!(!qualification_bundle_bytes_are_valid(
            record.as_bytes(),
            QUALIFICATION_REPORT
        ));
    }

    #[test]
    fn plist_reader_finds_bundle_values() {
        let xml = br#"<plist><dict><key>CFBundleExecutable</key><string>BambuStudio</string><key>CFBundleShortVersionString</key><string>02.02.00.85</string></dict></plist>"#;
        assert_eq!(
            plist_string(Path::new("Info.plist"), xml, "CFBundleExecutable").unwrap(),
            "BambuStudio"
        );
        assert_eq!(
            plist_string(Path::new("Info.plist"), xml, "CFBundleShortVersionString").unwrap(),
            A1MINI_APPLICATION_VERSION
        );
    }

    #[test]
    fn relative_home_is_rejected() {
        let error = adapter_context_with_home(
            Path::new("/Applications/BambuStudio.app"),
            A1MINI_APPLICATION_VERSION.into(),
            A1MINI_EXECUTABLE_SHA256.into(),
            Some(Path::new("relative")),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("HOME is unavailable or relative")
        );
    }
}
