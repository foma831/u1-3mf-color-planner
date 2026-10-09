//! Version-gated access to the Snapmaker Orca profiles used by the converter.
//!
//! Full Spectrum metadata is not a documented, stable interchange API. This
//! adapter therefore refuses guaranteed export unless it can prove that the
//! expected application and system profiles are present.

use quick_xml::Reader;
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

mod pva_profile;
mod u1_direct;
mod u1_full_spectrum;
mod u1_full_spectrum_gui_round_trip;
mod u1_gui_round_trip;

pub use u1_direct::*;
pub use u1_full_spectrum::*;
pub use u1_full_spectrum_gui_round_trip::*;
pub use u1_gui_round_trip::*;

pub const SUPPORTED_ORCA_VERSION: &str = "2.3.6";
pub const FULL_SPECTRUM_PROFILE_NAME: &str = "Snapmaker PLA Full Spectrum @U1 0.4 nozzle";
pub const U1_MACHINE_PROFILE_NAME: &str = "Snapmaker U1 (0.4 nozzle)";
pub const FULL_SPECTRUM_SETTING_ID: &str = "1195313935011";
pub const FULL_SPECTRUM_PROFILE_SHA256: &str =
    "a69fef730be5386a157ace8e1c50518bb2b308eb5061cdf7f468454e9a4070d6";
pub const U1_MACHINE_SETTING_ID: &str = "SM_U1";
pub const U1_MACHINE_PROFILE_SHA256: &str =
    "6c14f708c0268ec93867a0f090ec2167616cf9cfc5d1b3546454293ae03f58c7";
pub const FULL_SPECTRUM_PROCESS_PROFILE_NAME: &str = "0.08 Extra Fine @Snapmaker U1 (0.4 nozzle)";
pub const FULL_SPECTRUM_PROCESS_SETTING_ID: &str = "GP001";
pub const FULL_SPECTRUM_PROCESS_PROFILE_SHA256: &str =
    "dfb5bf5ce28e4d0b26f98dbb6b9bd8de72c705e84419ea6166851135ff0181c9";
pub const FULL_SPECTRUM_PROCESS_BASE_SHA256: &str =
    "018d613040ac01d46e39914ee5638d2a0a42081de540b7d37a44315a6b187737";
pub const U1_PROCESS_COMMON_SHA256: &str =
    "43fba9fe359864b265ffa8682cf404c2b935d12164ffe3d0048e5dcb0691db74";
pub const U1_PROCESS_BASE_SHA256: &str =
    "66e9e51c4bd04cbbec49fa1a3a043bc3c6e329396c2b38a18db5a66b6e72b4e2";
pub const FULL_SPECTRUM_FILAMENT_BASE_SHA256: &str =
    "2673ec3925d776d2d4228e3d4fd9414a2db40ef36a56d5dd462bf1055a503567";
pub const U1_MACHINE_BASE_SHA256: &str =
    "ff0724000b1436fcc3d77ffa74bb623a9b322474fc20c8a8d5a26ee164077046";
pub const TOOLCHANGER_MACHINE_BASE_SHA256: &str =
    "b2ee2d2c31a26ddee7b0c8bd84c58934d9c01e84e1b8f7ae45e14acfd1f7b52a";
pub const KLIPPER_MACHINE_BASE_SHA256: &str =
    "78dd750211347543368a8a0f136441ef7e23b3a4a09a9e33a370c91dbda273b9";

const FULL_SPECTRUM_RELATIVE_PATH: &str =
    "Snapmaker/filament/Snapmaker PLA Full Spectrum @U1 0.4 nozzle.json";
const MACHINE_RELATIVE_PATH: &str = "Snapmaker/machine/Snapmaker U1 (0.4 nozzle).json";
const PROCESS_PROFILE_RELATIVE_PATH: &str =
    "Snapmaker/process/0.08 Extra Fine @Snapmaker U1 (0.4 nozzle).json";
const PROCESS_BASE_RELATIVE_PATH: &str = "Snapmaker/process/fdm_process_U1_0.08.json";
const PROCESS_COMMON_RELATIVE_PATH: &str = "Snapmaker/process/fdm_process_U1_common.json";
const PROCESS_U1_RELATIVE_PATH: &str = "Snapmaker/process/fdm_process_U1.json";
const FILAMENT_BASE_RELATIVE_PATH: &str = "Snapmaker/filament/Snapmaker PLA Basic @U1 base.json";
const MACHINE_U1_BASE_RELATIVE_PATH: &str = "Snapmaker/machine/fdm_U1.json";
const MACHINE_TOOLCHANGER_RELATIVE_PATH: &str = "Snapmaker/machine/fdm_toolchanger.json";
const MACHINE_KLIPPER_RELATIVE_PATH: &str = "Snapmaker/machine/fdm_klipper.json";

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("Snapmaker Orca application was not found at {0}")]
    ApplicationNotFound(PathBuf),
    #[error("required Snapmaker Orca file was not found: {0}")]
    RequiredFileMissing(PathBuf),
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse XML property list {path}: {message}")]
    InvalidPropertyList { path: PathBuf, message: String },
    #[error("property {key} is missing from {path}")]
    MissingProperty { path: PathBuf, key: &'static str },
    #[error("failed to parse profile {path}: {source}")]
    InvalidProfile {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Compatibility {
    Guaranteed,
    UnsupportedVersion,
    UnsupportedProfiles,
}

impl Compatibility {
    /// Returns `true` only for the exact application and profile baseline that
    /// this adapter has been verified against.
    #[must_use]
    pub fn allows_guaranteed_export(&self) -> bool {
        matches!(self, Self::Guaranteed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileEvidence {
    pub path: PathBuf,
    pub name: Option<String>,
    pub setting_id: Option<String>,
    pub sha256: String,
    pub valid: bool,
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallationReport {
    pub application_path: PathBuf,
    pub executable_path: PathBuf,
    pub resources_path: PathBuf,
    pub version: String,
    pub supported_version: String,
    pub compatibility: Compatibility,
    pub full_spectrum_profile: ProfileEvidence,
    pub machine_profile: ProfileEvidence,
    pub inherited_profiles: Vec<ProfileEvidence>,
    pub process_profiles: Vec<ProfileEvidence>,
    pub issues: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct OrcaProfile {
    #[serde(rename = "type")]
    profile_type: Option<String>,
    name: Option<String>,
    setting_id: Option<String>,
    compatible_printers: Option<Vec<String>>,
    printer_model: Option<String>,
    printer_variant: Option<String>,
    nozzle_diameter: Option<Vec<String>>,
    inherits: Option<String>,
    layer_height: Option<String>,
}

/// Returns conventional application locations without requiring that they
/// exist. Callers may add user-selected locations before inspection.
pub fn conventional_application_paths() -> Vec<PathBuf> {
    let paths = Vec::new();
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let mut paths = paths;
    #[cfg(target_os = "macos")]
    {
        paths.push(PathBuf::from("/Applications/Snapmaker Orca.app"));
        if let Some(home) = std::env::var_os("HOME") {
            paths.push(PathBuf::from(home).join("Applications/Snapmaker Orca.app"));
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            paths.push(PathBuf::from(program_files).join("Snapmaker Orca"));
        }
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            paths.push(PathBuf::from(local_app_data).join("Snapmaker Orca"));
        }
    }
    paths
}

pub fn discover_installation() -> Option<PathBuf> {
    conventional_application_paths()
        .into_iter()
        .find(|path| path.exists())
}

/// Inspects a macOS `.app` bundle. Other platforms can call
/// [`inspect_resources`] once their resources directory and version are known.
pub fn inspect_macos_application(
    application_path: &Path,
) -> Result<InstallationReport, AdapterError> {
    if !application_path.is_dir() {
        return Err(AdapterError::ApplicationNotFound(
            application_path.to_path_buf(),
        ));
    }
    let contents = application_path.join("Contents");
    let plist_path = contents.join("Info.plist");
    let plist = read_file(&plist_path)?;
    let version = plist_string(&plist_path, &plist, "CFBundleShortVersionString")?;
    let executable_name = plist_string(&plist_path, &plist, "CFBundleExecutable")?;
    inspect_resources(
        application_path,
        contents.join("MacOS").join(executable_name),
        contents.join("Resources"),
        version,
    )
}

pub fn inspect_resources(
    application_path: &Path,
    executable_path: PathBuf,
    resources_path: PathBuf,
    version: String,
) -> Result<InstallationReport, AdapterError> {
    if !executable_path.is_file() {
        return Err(AdapterError::RequiredFileMissing(executable_path));
    }
    if !resources_path.is_dir() {
        return Err(AdapterError::RequiredFileMissing(resources_path));
    }

    let profiles_root = resources_path.join("profiles");
    let full_spectrum_path = profiles_root.join(FULL_SPECTRUM_RELATIVE_PATH);
    let machine_path = profiles_root.join(MACHINE_RELATIVE_PATH);
    let full_spectrum_profile = inspect_profile(&full_spectrum_path, ProfileKind::FullSpectrum)?;
    let machine_profile = inspect_profile(&machine_path, ProfileKind::Machine)?;
    let inherited_profiles = [
        (FILAMENT_BASE_RELATIVE_PATH, ProfileKind::FilamentBase),
        (MACHINE_U1_BASE_RELATIVE_PATH, ProfileKind::MachineU1Base),
        (
            MACHINE_TOOLCHANGER_RELATIVE_PATH,
            ProfileKind::MachineToolchanger,
        ),
        (MACHINE_KLIPPER_RELATIVE_PATH, ProfileKind::MachineKlipper),
    ]
    .into_iter()
    .map(|(relative_path, kind)| inspect_profile(&profiles_root.join(relative_path), kind))
    .collect::<Result<Vec<_>, _>>()?;
    let process_profiles = [
        (PROCESS_PROFILE_RELATIVE_PATH, ProfileKind::ProcessPreset),
        (PROCESS_BASE_RELATIVE_PATH, ProfileKind::ProcessBase08),
        (PROCESS_COMMON_RELATIVE_PATH, ProfileKind::ProcessCommon),
        (PROCESS_U1_RELATIVE_PATH, ProfileKind::ProcessU1),
    ]
    .into_iter()
    .map(|(relative_path, kind)| inspect_profile(&profiles_root.join(relative_path), kind))
    .collect::<Result<Vec<_>, _>>()?;

    let mut issues = Vec::new();
    if version != SUPPORTED_ORCA_VERSION {
        issues.push(format!(
            "Installed Snapmaker Orca {version} is not the supported adapter version {SUPPORTED_ORCA_VERSION}."
        ));
    }
    issues.extend(full_spectrum_profile.issues.iter().cloned());
    issues.extend(machine_profile.issues.iter().cloned());
    issues.extend(
        inherited_profiles
            .iter()
            .flat_map(|profile| profile.issues.iter().cloned()),
    );
    issues.extend(
        process_profiles
            .iter()
            .flat_map(|profile| profile.issues.iter().cloned()),
    );

    let profiles_supported = full_spectrum_profile.valid
        && machine_profile.valid
        && inherited_profiles.iter().all(|profile| profile.valid)
        && process_profiles.iter().all(|profile| profile.valid);
    let compatibility = if version != SUPPORTED_ORCA_VERSION {
        Compatibility::UnsupportedVersion
    } else if !profiles_supported {
        Compatibility::UnsupportedProfiles
    } else {
        Compatibility::Guaranteed
    };

    Ok(InstallationReport {
        application_path: application_path.to_path_buf(),
        executable_path,
        resources_path,
        version,
        supported_version: SUPPORTED_ORCA_VERSION.to_owned(),
        compatibility,
        full_spectrum_profile,
        machine_profile,
        inherited_profiles,
        process_profiles,
        issues,
    })
}

#[derive(Clone, Copy)]
enum ProfileKind {
    FullSpectrum,
    Machine,
    ProcessPreset,
    ProcessBase08,
    ProcessCommon,
    ProcessU1,
    FilamentBase,
    MachineU1Base,
    MachineToolchanger,
    MachineKlipper,
}

fn inspect_profile(path: &Path, kind: ProfileKind) -> Result<ProfileEvidence, AdapterError> {
    if !path.is_file() {
        return Err(AdapterError::RequiredFileMissing(path.to_path_buf()));
    }
    let bytes = read_file(path)?;
    let profile: OrcaProfile =
        serde_json::from_slice(&bytes).map_err(|source| AdapterError::InvalidProfile {
            path: path.to_path_buf(),
            source,
        })?;
    let mut issues = Vec::new();

    match kind {
        ProfileKind::FullSpectrum => {
            if profile.profile_type.as_deref() != Some("filament") {
                issues.push("Full Spectrum profile must have type filament.".to_owned());
            }
            if profile.name.as_deref() != Some(FULL_SPECTRUM_PROFILE_NAME) {
                issues.push(format!(
                    "Expected Full Spectrum profile name {FULL_SPECTRUM_PROFILE_NAME}."
                ));
            }
            if profile.inherits.as_deref() != Some("Snapmaker PLA Basic @U1 base") {
                issues.push(
                    "Full Spectrum profile must inherit from Snapmaker PLA Basic @U1 base."
                        .to_owned(),
                );
            }
            if !profile
                .compatible_printers
                .as_deref()
                .unwrap_or_default()
                .iter()
                .any(|printer| printer == U1_MACHINE_PROFILE_NAME)
            {
                issues.push(format!(
                    "Full Spectrum profile is not compatible with {U1_MACHINE_PROFILE_NAME}."
                ));
            }
        }
        ProfileKind::Machine => {
            if profile.profile_type.as_deref() != Some("machine") {
                issues.push("U1 machine profile must have type machine.".to_owned());
            }
            if profile.name.as_deref() != Some(U1_MACHINE_PROFILE_NAME) {
                issues.push(format!(
                    "Expected machine profile name {U1_MACHINE_PROFILE_NAME}."
                ));
            }
            if profile.printer_model.as_deref() != Some("Snapmaker U1") {
                issues.push("Machine profile does not identify Snapmaker U1.".to_owned());
            }
            if profile.printer_variant.as_deref() != Some("0.4") {
                issues.push("Machine profile is not the 0.4 mm variant.".to_owned());
            }
            if profile.inherits.as_deref() != Some("fdm_U1") {
                issues.push("U1 machine profile must inherit from fdm_U1.".to_owned());
            }
            let has_four_matching_nozzles =
                profile.nozzle_diameter.as_deref().is_some_and(|diameters| {
                    diameters.len() == 4 && diameters.iter().all(|diameter| diameter == "0.4")
                });
            if !has_four_matching_nozzles {
                issues.push("Machine profile must expose four 0.4 mm toolheads.".to_owned());
            }
        }
        ProfileKind::ProcessPreset => {
            validate_process_profile(
                &profile,
                FULL_SPECTRUM_PROCESS_PROFILE_NAME,
                Some("fdm_process_U1_0.08"),
                None,
                &mut issues,
            );
            if !profile
                .compatible_printers
                .as_deref()
                .unwrap_or_default()
                .iter()
                .any(|printer| printer == U1_MACHINE_PROFILE_NAME)
            {
                issues.push(format!(
                    "Full Spectrum process is not compatible with {U1_MACHINE_PROFILE_NAME}."
                ));
            }
        }
        ProfileKind::ProcessBase08 => validate_process_profile(
            &profile,
            "fdm_process_U1_0.08",
            Some("fdm_process_U1_common"),
            Some("0.08"),
            &mut issues,
        ),
        ProfileKind::ProcessCommon => validate_process_profile(
            &profile,
            "fdm_process_U1_common",
            Some("fdm_process_U1"),
            None,
            &mut issues,
        ),
        ProfileKind::ProcessU1 => {
            validate_process_profile(&profile, "fdm_process_U1", None, None, &mut issues)
        }
        ProfileKind::FilamentBase => validate_inherited_profile(
            &profile,
            "filament",
            "Snapmaker PLA Basic @U1 base",
            None,
            &mut issues,
        ),
        ProfileKind::MachineU1Base => validate_inherited_profile(
            &profile,
            "machine",
            "fdm_U1",
            Some("fdm_toolchanger"),
            &mut issues,
        ),
        ProfileKind::MachineToolchanger => validate_inherited_profile(
            &profile,
            "machine",
            "fdm_toolchanger",
            Some("fdm_klipper"),
            &mut issues,
        ),
        ProfileKind::MachineKlipper => {
            validate_inherited_profile(&profile, "machine", "fdm_klipper", None, &mut issues)
        }
    }

    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    match kind {
        ProfileKind::FullSpectrum => validate_profile_fingerprint(
            "Full Spectrum profile",
            profile.setting_id.as_deref(),
            &sha256,
            FULL_SPECTRUM_SETTING_ID,
            FULL_SPECTRUM_PROFILE_SHA256,
            &mut issues,
        ),
        ProfileKind::Machine => validate_profile_fingerprint(
            "U1 machine profile",
            profile.setting_id.as_deref(),
            &sha256,
            U1_MACHINE_SETTING_ID,
            U1_MACHINE_PROFILE_SHA256,
            &mut issues,
        ),
        ProfileKind::ProcessPreset => validate_profile_fingerprint(
            "Full Spectrum process profile",
            profile.setting_id.as_deref(),
            &sha256,
            FULL_SPECTRUM_PROCESS_SETTING_ID,
            FULL_SPECTRUM_PROCESS_PROFILE_SHA256,
            &mut issues,
        ),
        ProfileKind::ProcessBase08 => validate_profile_sha256(
            "Full Spectrum 0.08 process base",
            &sha256,
            FULL_SPECTRUM_PROCESS_BASE_SHA256,
            &mut issues,
        ),
        ProfileKind::ProcessCommon => validate_profile_sha256(
            "U1 common process base",
            &sha256,
            U1_PROCESS_COMMON_SHA256,
            &mut issues,
        ),
        ProfileKind::ProcessU1 => validate_profile_sha256(
            "U1 process base",
            &sha256,
            U1_PROCESS_BASE_SHA256,
            &mut issues,
        ),
        ProfileKind::FilamentBase => validate_profile_sha256(
            "Full Spectrum filament base",
            &sha256,
            FULL_SPECTRUM_FILAMENT_BASE_SHA256,
            &mut issues,
        ),
        ProfileKind::MachineU1Base => validate_profile_sha256(
            "U1 machine base",
            &sha256,
            U1_MACHINE_BASE_SHA256,
            &mut issues,
        ),
        ProfileKind::MachineToolchanger => validate_profile_sha256(
            "Toolchanger machine base",
            &sha256,
            TOOLCHANGER_MACHINE_BASE_SHA256,
            &mut issues,
        ),
        ProfileKind::MachineKlipper => validate_profile_sha256(
            "Klipper machine base",
            &sha256,
            KLIPPER_MACHINE_BASE_SHA256,
            &mut issues,
        ),
    }

    Ok(ProfileEvidence {
        path: path.to_path_buf(),
        name: profile.name,
        setting_id: profile.setting_id,
        sha256,
        valid: issues.is_empty(),
        issues,
    })
}

fn validate_inherited_profile(
    profile: &OrcaProfile,
    expected_type: &str,
    expected_name: &str,
    expected_parent: Option<&str>,
    issues: &mut Vec<String>,
) {
    if profile.profile_type.as_deref() != Some(expected_type) {
        issues.push(format!(
            "Inherited profile {expected_name} must have type {expected_type}."
        ));
    }
    if profile.name.as_deref() != Some(expected_name) {
        issues.push(format!("Expected inherited profile name {expected_name}."));
    }
    if profile.inherits.as_deref() != expected_parent {
        issues.push(format!(
            "Inherited profile {expected_name} must inherit from {}.",
            expected_parent.unwrap_or("no parent")
        ));
    }
}

fn validate_process_profile(
    profile: &OrcaProfile,
    expected_name: &str,
    expected_parent: Option<&str>,
    expected_layer_height: Option<&str>,
    issues: &mut Vec<String>,
) {
    if profile.profile_type.as_deref() != Some("process") {
        issues.push(format!(
            "Process profile {expected_name} must have type process."
        ));
    }
    if profile.name.as_deref() != Some(expected_name) {
        issues.push(format!("Expected process profile name {expected_name}."));
    }
    if profile.inherits.as_deref() != expected_parent {
        issues.push(format!(
            "Process profile {expected_name} must inherit from {}.",
            expected_parent.unwrap_or("no parent")
        ));
    }
    if let Some(expected) = expected_layer_height
        && profile.layer_height.as_deref() != Some(expected)
    {
        issues.push(format!(
            "Process profile {expected_name} must use layer height {expected} mm."
        ));
    }
}

fn validate_profile_sha256(
    profile_label: &str,
    actual_sha256: &str,
    expected_sha256: &str,
    issues: &mut Vec<String>,
) {
    if actual_sha256 != expected_sha256 {
        issues.push(format!(
            "{profile_label} SHA-256 is not the verified Snapmaker Orca {SUPPORTED_ORCA_VERSION} baseline (expected {expected_sha256}, found {actual_sha256})."
        ));
    }
}

fn validate_profile_fingerprint(
    profile_label: &str,
    actual_setting_id: Option<&str>,
    actual_sha256: &str,
    expected_setting_id: &str,
    expected_sha256: &str,
    issues: &mut Vec<String>,
) {
    if actual_setting_id != Some(expected_setting_id) {
        issues.push(format!(
            "{profile_label} setting_id must be {expected_setting_id}; found {}.",
            actual_setting_id.unwrap_or("<missing>")
        ));
    }
    if actual_sha256 != expected_sha256 {
        issues.push(format!(
            "{profile_label} SHA-256 is not the verified Snapmaker Orca {SUPPORTED_ORCA_VERSION} baseline (expected {expected_sha256}, found {actual_sha256})."
        ));
    }
}

fn read_file(path: &Path) -> Result<Vec<u8>, AdapterError> {
    fs::read(path).map_err(|source| AdapterError::Read {
        path: path.to_path_buf(),
        source,
    })
}

fn plist_string(path: &Path, xml: &[u8], wanted_key: &'static str) -> Result<String, AdapterError> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut last_key: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(start)) if start.name().as_ref() == b"key" => {
                let value = reader.read_text(start.name()).map_err(|error| {
                    AdapterError::InvalidPropertyList {
                        path: path.to_path_buf(),
                        message: error.to_string(),
                    }
                })?;
                last_key = Some(value.into_owned());
            }
            Ok(Event::Start(start)) if start.name().as_ref() == b"string" => {
                let value = reader.read_text(start.name()).map_err(|error| {
                    AdapterError::InvalidPropertyList {
                        path: path.to_path_buf(),
                        message: error.to_string(),
                    }
                })?;
                if last_key.as_deref() == Some(wanted_key) {
                    return Ok(value.into_owned());
                }
                last_key = None;
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => {
                return Err(AdapterError::InvalidPropertyList {
                    path: path.to_path_buf(),
                    message: error.to_string(),
                });
            }
        }
        buffer.clear();
    }

    Err(AdapterError::MissingProperty {
        path: path.to_path_buf(),
        key: wanted_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_strings_from_xml_property_list() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
        <plist version="1.0"><dict>
        <key>CFBundleExecutable</key><string>Snapmaker_Orca</string>
        <key>CFBundleShortVersionString</key><string>2.3.5</string>
        </dict></plist>"#;
        let path = Path::new("Info.plist");
        assert_eq!(
            plist_string(path, xml, "CFBundleShortVersionString").unwrap(),
            "2.3.5"
        );
    }

    #[test]
    fn rejects_structurally_valid_but_unverified_profiles() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("Snapmaker Orca");
        fs::write(&executable, b"test executable").unwrap();
        let resources = temp.path().join("Resources");
        let profiles = resources.join("profiles");
        let filament = profiles.join("Snapmaker/filament");
        let machine = profiles.join("Snapmaker/machine");
        let process = profiles.join("Snapmaker/process");
        fs::create_dir_all(&filament).unwrap();
        fs::create_dir_all(&machine).unwrap();
        fs::create_dir_all(&process).unwrap();
        fs::write(
            filament.join("Snapmaker PLA Full Spectrum @U1 0.4 nozzle.json"),
            format!(
                r#"{{"type":"filament","name":"{FULL_SPECTRUM_PROFILE_NAME}","setting_id":"fs","inherits":"Snapmaker PLA Basic @U1 base","compatible_printers":["{U1_MACHINE_PROFILE_NAME}"]}}"#
            ),
        )
        .unwrap();
        fs::write(
            filament.join("Snapmaker PLA Basic @U1 base.json"),
            r#"{"type":"filament","name":"Snapmaker PLA Basic @U1 base"}"#,
        )
        .unwrap();
        fs::write(
            process.join("0.08 Extra Fine @Snapmaker U1 (0.4 nozzle).json"),
            format!(
                r#"{{"type":"process","name":"{FULL_SPECTRUM_PROCESS_PROFILE_NAME}","setting_id":"process","inherits":"fdm_process_U1_0.08","compatible_printers":["{U1_MACHINE_PROFILE_NAME}"]}}"#
            ),
        )
        .unwrap();
        fs::write(
            process.join("fdm_process_U1_0.08.json"),
            r#"{"type":"process","name":"fdm_process_U1_0.08","inherits":"fdm_process_U1_common","layer_height":"0.08"}"#,
        )
        .unwrap();
        fs::write(
            process.join("fdm_process_U1_common.json"),
            r#"{"type":"process","name":"fdm_process_U1_common","inherits":"fdm_process_U1"}"#,
        )
        .unwrap();
        fs::write(
            process.join("fdm_process_U1.json"),
            r#"{"type":"process","name":"fdm_process_U1"}"#,
        )
        .unwrap();
        fs::write(
            machine.join("Snapmaker U1 (0.4 nozzle).json"),
            format!(
                r#"{{"type":"machine","name":"{U1_MACHINE_PROFILE_NAME}","setting_id":"u1","inherits":"fdm_U1","printer_model":"Snapmaker U1","printer_variant":"0.4","nozzle_diameter":["0.4","0.4","0.4","0.4"]}}"#
            ),
        )
        .unwrap();
        fs::write(
            machine.join("fdm_U1.json"),
            r#"{"type":"machine","name":"fdm_U1","inherits":"fdm_toolchanger"}"#,
        )
        .unwrap();
        fs::write(
            machine.join("fdm_toolchanger.json"),
            r#"{"type":"machine","name":"fdm_toolchanger","inherits":"fdm_klipper"}"#,
        )
        .unwrap();
        fs::write(
            machine.join("fdm_klipper.json"),
            r#"{"type":"machine","name":"fdm_klipper"}"#,
        )
        .unwrap();

        let report = inspect_resources(
            Path::new("Snapmaker Orca"),
            executable.clone(),
            resources.clone(),
            SUPPORTED_ORCA_VERSION.to_owned(),
        )
        .unwrap();

        assert_eq!(report.compatibility, Compatibility::UnsupportedProfiles);
        assert!(!report.compatibility.allows_guaranteed_export());
        assert!(report.issues.len() >= 9);
        assert!(!report.full_spectrum_profile.valid);
        assert!(!report.machine_profile.valid);
        assert!(
            report
                .inherited_profiles
                .iter()
                .all(|profile| !profile.valid)
        );
        assert!(report.process_profiles.iter().all(|profile| !profile.valid));
        assert_eq!(
            report.full_spectrum_profile.setting_id.as_deref(),
            Some("fs")
        );

        let unsupported_version = inspect_resources(
            Path::new("Snapmaker Orca"),
            executable,
            resources,
            "2.3.4".to_owned(),
        )
        .unwrap();
        assert_eq!(
            unsupported_version.compatibility,
            Compatibility::UnsupportedVersion
        );
        assert!(!unsupported_version.compatibility.allows_guaranteed_export());
    }

    #[test]
    fn fingerprint_validation_requires_both_exact_values() {
        let mut issues = Vec::new();
        validate_profile_fingerprint(
            "Test profile",
            Some("known-id"),
            "known-hash",
            "known-id",
            "known-hash",
            &mut issues,
        );
        assert!(issues.is_empty());

        validate_profile_fingerprint(
            "Test profile",
            Some("different-id"),
            "different-hash",
            "known-id",
            "known-hash",
            &mut issues,
        );
        assert_eq!(issues.len(), 2);
        assert!(issues[0].contains("setting_id"));
        assert!(issues[1].contains("SHA-256"));
    }

    #[test]
    fn fingerprint_validation_rejects_a_byte_level_modification() {
        let original = br#"{"type":"filament","setting_id":"known-id"}"#;
        let modified = br#"{"type":"filament","setting_id":"known-id"}
"#;
        let expected_sha256 = format!("{:x}", Sha256::digest(original));
        let modified_sha256 = format!("{:x}", Sha256::digest(modified));
        let mut issues = Vec::new();

        validate_profile_fingerprint(
            "Test profile",
            Some("known-id"),
            &modified_sha256,
            "known-id",
            &expected_sha256,
            &mut issues,
        );

        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("SHA-256"));
    }

    #[test]
    fn known_baseline_constants_are_complete() {
        assert_eq!(FULL_SPECTRUM_SETTING_ID, "1195313935011");
        assert_eq!(FULL_SPECTRUM_PROFILE_SHA256.len(), 64);
        assert_eq!(U1_MACHINE_SETTING_ID, "SM_U1");
        assert_eq!(U1_MACHINE_PROFILE_SHA256.len(), 64);
        assert_eq!(FULL_SPECTRUM_PROCESS_SETTING_ID, "GP001");
        assert_eq!(FULL_SPECTRUM_PROCESS_PROFILE_SHA256.len(), 64);
        assert_eq!(FULL_SPECTRUM_PROCESS_BASE_SHA256.len(), 64);
        assert_eq!(U1_PROCESS_COMMON_SHA256.len(), 64);
        assert_eq!(U1_PROCESS_BASE_SHA256.len(), 64);
        assert_eq!(FULL_SPECTRUM_FILAMENT_BASE_SHA256.len(), 64);
        assert_eq!(U1_MACHINE_BASE_SHA256.len(), 64);
        assert_eq!(TOOLCHANGER_MACHINE_BASE_SHA256.len(), 64);
        assert_eq!(KLIPPER_MACHINE_BASE_SHA256.len(), 64);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn installed_known_good_application_matches_the_frozen_baseline_when_present() {
        let application_path = Path::new("/Applications/Snapmaker Orca.app");
        if !application_path.is_dir() {
            return;
        }

        let report = inspect_macos_application(application_path).unwrap();
        assert_eq!(report.version, SUPPORTED_ORCA_VERSION);
        assert_eq!(
            report.full_spectrum_profile.setting_id.as_deref(),
            Some(FULL_SPECTRUM_SETTING_ID)
        );
        assert_eq!(
            report.full_spectrum_profile.sha256,
            FULL_SPECTRUM_PROFILE_SHA256
        );
        assert_eq!(
            report.machine_profile.setting_id.as_deref(),
            Some(U1_MACHINE_SETTING_ID)
        );
        assert_eq!(report.machine_profile.sha256, U1_MACHINE_PROFILE_SHA256);
        assert_eq!(report.inherited_profiles.len(), 4);
        assert_eq!(
            report.inherited_profiles[0].sha256,
            FULL_SPECTRUM_FILAMENT_BASE_SHA256
        );
        assert_eq!(report.inherited_profiles[1].sha256, U1_MACHINE_BASE_SHA256);
        assert_eq!(
            report.inherited_profiles[2].sha256,
            TOOLCHANGER_MACHINE_BASE_SHA256
        );
        assert_eq!(
            report.inherited_profiles[3].sha256,
            KLIPPER_MACHINE_BASE_SHA256
        );
        assert_eq!(report.process_profiles.len(), 4);
        assert_eq!(
            report.process_profiles[0].setting_id.as_deref(),
            Some(FULL_SPECTRUM_PROCESS_SETTING_ID)
        );
        assert_eq!(
            report.process_profiles[0].sha256,
            FULL_SPECTRUM_PROCESS_PROFILE_SHA256
        );
        assert_eq!(
            report.process_profiles[1].sha256,
            FULL_SPECTRUM_PROCESS_BASE_SHA256
        );
        assert_eq!(report.process_profiles[2].sha256, U1_PROCESS_COMMON_SHA256);
        assert_eq!(report.process_profiles[3].sha256, U1_PROCESS_BASE_SHA256);
        assert_eq!(report.compatibility, Compatibility::Guaranteed);
        assert!(report.compatibility.allows_guaranteed_export());
        assert!(report.issues.is_empty());
    }
}
