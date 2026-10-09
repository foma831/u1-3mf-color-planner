use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use tauri::Manager;
use u1_application::built_in_spool_inventory;
use u1_planner::{Material, RgbColor, Spool};
use uuid::Uuid;

pub const FILAMENT_LIBRARY_SCHEMA_VERSION: u32 = 3;
const FILAMENT_LIBRARY_FILE_NAME: &str = "filament-library-v3.json";
const LEGACY_V2_FILAMENT_LIBRARY_FILE_NAME: &str = "filament-library-v2.json";
const LEGACY_FILAMENT_LIBRARY_FILE_NAME: &str = "filament-library-v1.json";
const MAX_LIBRARY_BYTES: usize = 1024 * 1024;
const MAX_LIBRARY_SPOOLS: usize = 512;
const MAX_ID_BYTES: usize = 128;
const MAX_NAME_BYTES: usize = 256;
const MAX_METADATA_BYTES: usize = 512;
const MAX_NOTES_BYTES: usize = 4096;
const MAX_NOZZLE_TEMPERATURE_C: u16 = 500;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FilamentLibraryView {
    pub schema_version: u32,
    pub spools: Vec<FilamentSpoolView>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FilamentSpoolView {
    pub id: String,
    #[serde(default)]
    pub calibration_identity: String,
    pub name: String,
    #[serde(default)]
    pub color_name: String,
    pub hex: String,
    pub material: String,
    #[serde(default)]
    pub sku: String,
    #[serde(default)]
    pub profile: String,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub product_line: String,
    #[serde(default)]
    pub optical_descriptor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_nozzle_temperature_c: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_nozzle_temperature_c: Option<u16>,
    #[serde(default)]
    pub batch_lot: String,
    #[serde(default)]
    pub calibration_reference: String,
    #[serde(default)]
    pub notes: String,
    pub color_basis: String,
    pub source: String,
    #[serde(default = "default_available")]
    pub available: bool,
}

fn default_available() -> bool {
    true
}

impl FilamentSpoolView {
    pub fn from_domain(spool: &Spool) -> Option<Self> {
        let material = material_name(&spool.material)?.to_owned();
        let source = if is_built_in_spool(&spool.id) {
            "built-in"
        } else {
            "user"
        };
        Some(Self {
            id: spool.id.clone(),
            calibration_identity: spool
                .calibration_id
                .clone()
                .unwrap_or_else(|| spool.id.clone()),
            name: spool.display_name.clone(),
            color_name: spool.color_name.clone().unwrap_or_else(|| {
                spool
                    .display_name
                    .rsplit_once('—')
                    .map(|(_, color)| color.trim().to_owned())
                    .unwrap_or_else(|| spool.display_name.clone())
            }),
            hex: color_hex(spool.actual_color()),
            material,
            sku: spool.sku.clone().unwrap_or_default(),
            profile: spool.profile_id.clone().unwrap_or_default(),
            vendor: String::new(),
            product_line: String::new(),
            optical_descriptor: String::new(),
            min_nozzle_temperature_c: None,
            max_nozzle_temperature_c: None,
            batch_lot: String::new(),
            calibration_reference: String::new(),
            notes: String::new(),
            color_basis: if spool.measured_color.is_some() {
                "Measured".to_owned()
            } else {
                "Nominal".to_owned()
            },
            source: source.to_owned(),
            available: spool.available,
        })
    }

    pub fn to_domain(&self) -> Result<Spool, String> {
        validate_user_spool(self)?;
        let color = parse_hex_color(&self.hex)?;
        Ok(Spool {
            id: self.id.trim().to_owned(),
            calibration_id: Some(self.calibration_identity.trim().to_owned()),
            display_name: self.name.trim().to_owned(),
            color_name: (!self.color_name.trim().is_empty())
                .then(|| self.color_name.trim().to_owned()),
            material: parse_material(&self.material)?,
            nominal_color: color,
            measured_color: self
                .color_basis
                .eq_ignore_ascii_case("measured")
                .then_some(color),
            sku: (!self.sku.trim().is_empty()).then(|| self.sku.trim().to_owned()),
            profile_id: (!self.profile.trim().is_empty()).then(|| self.profile.trim().to_owned()),
            available: self.available,
        })
    }
}

impl FilamentLibraryView {
    pub fn planning_spools(&self) -> Result<Vec<Spool>, String> {
        // Keep this boundary safe even for recovery/test callers that hold a
        // deserialized v1 document instead of using load_for_app().
        let normalized = normalize_library(self.clone())?;
        let built_ins = built_in_spool_inventory().map_err(|error| error.to_string())?;
        let built_in_ids = built_ins
            .iter()
            .map(|spool| spool.id.clone())
            .collect::<BTreeSet<_>>();
        normalized
            .spools
            .iter()
            .filter(|spool| !built_in_ids.contains(&spool.id))
            .map(FilamentSpoolView::to_domain)
            .chain(built_ins.into_iter().map(Ok))
            .collect::<Result<Vec<_>, _>>()
            .map(|spools| apply_availability_overrides(spools, &normalized.spools))
    }
}

pub fn load_for_app(app: &tauri::AppHandle) -> Result<FilamentLibraryView, String> {
    let path = library_path(app)?;
    if path.exists() {
        return load_from_path(&path);
    }

    let legacy_v2_path = legacy_v2_library_path(app)?;
    let legacy_path = if legacy_v2_path.exists() {
        legacy_v2_path
    } else {
        legacy_library_path(app)?
    };
    if !legacy_path.exists() {
        return load_from_path(&path);
    }

    // Publish the migrated v3 document atomically and leave the legacy file in
    // place as a recovery copy. A failed migration therefore cannot destroy
    // the user's existing catalogue.
    let migrated = load_from_path(&legacy_path)?;
    save_to_path(&path, migrated)
}

pub fn save_for_app(
    app: &tauri::AppHandle,
    library: FilamentLibraryView,
) -> Result<FilamentLibraryView, String> {
    save_to_path(&library_path(app)?, library)
}

fn library_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|directory| directory.join(FILAMENT_LIBRARY_FILE_NAME))
        .map_err(|error| format!("Failed to resolve the application data directory: {error}"))
}

fn legacy_library_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|directory| directory.join(LEGACY_FILAMENT_LIBRARY_FILE_NAME))
        .map_err(|error| format!("Failed to resolve the application data directory: {error}"))
}

fn legacy_v2_library_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|directory| directory.join(LEGACY_V2_FILAMENT_LIBRARY_FILE_NAME))
        .map_err(|error| format!("Failed to resolve the application data directory: {error}"))
}

fn load_from_path(path: &Path) -> Result<FilamentLibraryView, String> {
    if !path.exists() {
        let mut library = normalize_library(FilamentLibraryView {
            schema_version: FILAMENT_LIBRARY_SCHEMA_VERSION,
            spools: Vec::new(),
        })?;
        // The built-in catalogue describes supported products, not the
        // operator's physical shelf. A fresh installation must not claim that
        // any of those spools are available until the operator confirms them.
        for spool in &mut library.spools {
            spool.available = false;
        }
        return Ok(library);
    }
    let metadata = fs::metadata(path)
        .map_err(|error| format!("Failed to inspect the filament library: {error}"))?;
    if metadata.len() > MAX_LIBRARY_BYTES as u64 {
        return Err(format!(
            "The filament library exceeds the {MAX_LIBRARY_BYTES} byte limit."
        ));
    }
    let mut file = File::open(path)
        .map_err(|error| format!("Failed to open the filament library: {error}"))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    Read::by_ref(&mut file)
        .take((MAX_LIBRARY_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Failed to read the filament library: {error}"))?;
    if bytes.len() > MAX_LIBRARY_BYTES {
        return Err(format!(
            "The filament library exceeds the {MAX_LIBRARY_BYTES} byte limit."
        ));
    }
    let library = serde_json::from_slice::<FilamentLibraryView>(&bytes)
        .map_err(|error| format!("The filament library is not valid JSON: {error}"))?;
    normalize_library(library)
}

fn save_to_path(
    path: &Path,
    mut library: FilamentLibraryView,
) -> Result<FilamentLibraryView, String> {
    let previous = path.exists().then(|| load_from_path(path)).transpose()?;
    reconcile_calibration_identities(&mut library, previous.as_ref());
    let normalized = normalize_library(library)?;
    let bytes = serde_json::to_vec_pretty(&normalized)
        .map_err(|error| format!("Failed to serialize the filament library: {error}"))?;
    if bytes.len() > MAX_LIBRARY_BYTES {
        return Err(format!(
            "The filament library exceeds the {MAX_LIBRARY_BYTES} byte limit."
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| "The filament library path has no parent directory.".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Failed to create the application data directory: {error}"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("Failed to create a temporary filament library: {error}"))?;
    temporary
        .write_all(&bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| format!("Failed to write the filament library: {error}"))?;
    temporary
        .persist(path)
        .map_err(|error| format!("Failed to publish the filament library atomically: {error}"))?;
    sync_parent_directory(parent)?;
    Ok(normalized)
}

fn reconcile_calibration_identities(
    library: &mut FilamentLibraryView,
    previous: Option<&FilamentLibraryView>,
) {
    let previous_users = previous
        .into_iter()
        .flat_map(|document| &document.spools)
        .filter(|spool| spool.source == "user")
        .map(|spool| (spool.id.as_str(), spool))
        .collect::<BTreeMap<_, _>>();

    for spool in &mut library.spools {
        if spool.source != "user" {
            continue;
        }
        let previous_spool = previous_users.get(spool.id.trim()).copied();
        match previous_spool {
            Some(previous_spool)
                if previous_spool.batch_lot.trim() == spool.batch_lot.trim()
                    && previous_spool.calibration_reference.trim()
                        == spool.calibration_reference.trim() =>
            {
                // The persisted identity is authoritative. This also prevents
                // a stale client from rolling a batch identity backwards.
                spool.calibration_identity = previous_spool.calibration_identity.clone();
            }
            Some(_) => {
                spool.calibration_identity = new_calibration_identity();
            }
            None if spool.calibration_identity.trim().is_empty() => {
                spool.calibration_identity = new_calibration_identity();
            }
            None => {}
        }
    }
}

fn new_calibration_identity() -> String {
    format!("spool-calibration:{}", Uuid::new_v4().hyphenated())
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> Result<(), String> {
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("Failed to finalize the filament library update: {error}"))
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> Result<(), String> {
    Ok(())
}

fn normalize_library(library: FilamentLibraryView) -> Result<FilamentLibraryView, String> {
    if !matches!(
        library.schema_version,
        1 | 2 | FILAMENT_LIBRARY_SCHEMA_VERSION
    ) {
        return Err(format!(
            "Unsupported filament library schema version {}; expected 1, 2, or {}.",
            library.schema_version, FILAMENT_LIBRARY_SCHEMA_VERSION
        ));
    }
    let source_schema_version = library.schema_version;
    if library.spools.len() > MAX_LIBRARY_SPOOLS {
        return Err(format!(
            "The filament library exceeds the {MAX_LIBRARY_SPOOLS} spool limit."
        ));
    }

    let built_ins = built_in_spool_inventory().map_err(|error| error.to_string())?;
    let built_in_by_id = built_ins
        .iter()
        .map(|spool| (spool.id.as_str(), spool))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    let mut availability = BTreeMap::new();
    let mut users = Vec::new();
    for mut spool in library.spools {
        let id = spool.id.trim();
        validate_id(id)?;
        if !seen.insert(id.to_owned()) {
            return Err(format!("Filament spool ID '{id}' is duplicated."));
        }
        if built_in_by_id.contains_key(id) {
            availability.insert(id.to_owned(), spool.available);
            continue;
        }
        if source_schema_version == 1 {
            // Preserve the identity used by all pre-v2 measured records.
            spool.calibration_identity = id.to_owned();
        }
        if spool.source != "user" {
            return Err(format!(
                "Unknown filament spool '{id}' must use source 'user'."
            ));
        }
        let domain = spool.to_domain()?;
        let mut normalized = FilamentSpoolView::from_domain(&domain)
            .expect("validated desktop library materials have UI labels");
        normalized.vendor = spool.vendor.trim().to_owned();
        normalized.product_line = spool.product_line.trim().to_owned();
        normalized.optical_descriptor = spool.optical_descriptor.trim().to_owned();
        normalized.min_nozzle_temperature_c = spool.min_nozzle_temperature_c;
        normalized.max_nozzle_temperature_c = spool.max_nozzle_temperature_c;
        normalized.batch_lot = spool.batch_lot.trim().to_owned();
        normalized.calibration_reference = spool.calibration_reference.trim().to_owned();
        normalized.notes = spool.notes.trim().to_owned();
        users.push(normalized);
    }

    if built_ins.len() + users.len() > MAX_LIBRARY_SPOOLS {
        return Err(format!(
            "The normalized filament library exceeds the {MAX_LIBRARY_SPOOLS} spool limit."
        ));
    }

    let mut spools = built_ins
        .iter()
        .filter_map(FilamentSpoolView::from_domain)
        .map(|mut spool| {
            if let Some(available) = availability.get(&spool.id) {
                spool.available = *available;
            }
            spool
        })
        .chain(users)
        .collect::<Vec<_>>();
    let mut seen_calibration_identities = BTreeSet::new();
    for spool in &spools {
        if !seen_calibration_identities.insert(spool.calibration_identity.clone()) {
            return Err(format!(
                "Filament calibration identity '{}' is duplicated.",
                spool.calibration_identity
            ));
        }
    }
    spools.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(FilamentLibraryView {
        schema_version: FILAMENT_LIBRARY_SCHEMA_VERSION,
        spools,
    })
}

fn apply_availability_overrides(mut spools: Vec<Spool>, views: &[FilamentSpoolView]) -> Vec<Spool> {
    let availability = views
        .iter()
        .map(|spool| (spool.id.as_str(), spool.available))
        .collect::<BTreeMap<_, _>>();
    for spool in &mut spools {
        if let Some(available) = availability.get(spool.id.as_str()) {
            spool.available = *available;
        }
    }
    spools.sort_by(|left, right| left.id.cmp(&right.id));
    spools
}

fn validate_user_spool(spool: &FilamentSpoolView) -> Result<(), String> {
    validate_id(spool.id.trim())?;
    if spool.id.trim().starts_with("source-") {
        return Err("Filament spool IDs beginning with 'source-' are reserved.".to_owned());
    }
    validate_text("name", &spool.name, 1, MAX_NAME_BYTES)?;
    validate_text("color name", &spool.color_name, 1, MAX_NAME_BYTES)?;
    validate_text("SKU", &spool.sku, 0, MAX_METADATA_BYTES)?;
    validate_text("profile", &spool.profile, 0, MAX_METADATA_BYTES)?;
    validate_text(
        "calibration identity",
        &spool.calibration_identity,
        1,
        MAX_ID_BYTES,
    )?;
    validate_text("vendor", &spool.vendor, 0, MAX_NAME_BYTES)?;
    validate_text("product line", &spool.product_line, 0, MAX_NAME_BYTES)?;
    validate_text(
        "optical descriptor",
        &spool.optical_descriptor,
        0,
        MAX_METADATA_BYTES,
    )?;
    validate_text("batch or lot", &spool.batch_lot, 0, MAX_METADATA_BYTES)?;
    validate_text(
        "calibration reference",
        &spool.calibration_reference,
        0,
        MAX_METADATA_BYTES,
    )?;
    validate_text("notes", &spool.notes, 0, MAX_NOTES_BYTES)?;
    validate_temperature_range(spool)?;
    parse_hex_color(&spool.hex)?;
    parse_material(&spool.material)?;
    if !matches!(spool.color_basis.as_str(), "Measured" | "Nominal") {
        return Err("Filament colorBasis must be 'Measured' or 'Nominal'.".to_owned());
    }
    Ok(())
}

fn validate_temperature_range(spool: &FilamentSpoolView) -> Result<(), String> {
    for (label, value) in [
        ("minimum nozzle temperature", spool.min_nozzle_temperature_c),
        ("maximum nozzle temperature", spool.max_nozzle_temperature_c),
    ] {
        if value.is_some_and(|temperature| temperature > MAX_NOZZLE_TEMPERATURE_C) {
            return Err(format!(
                "Filament {label} must be between 0 and {MAX_NOZZLE_TEMPERATURE_C} °C."
            ));
        }
    }
    if matches!(
        (
            spool.min_nozzle_temperature_c,
            spool.max_nozzle_temperature_c
        ),
        (Some(minimum), Some(maximum)) if minimum > maximum
    ) {
        return Err(
            "Filament minimum nozzle temperature cannot exceed its maximum temperature.".to_owned(),
        );
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > MAX_ID_BYTES {
        return Err(format!(
            "Filament spool IDs must contain 1 to {MAX_ID_BYTES} bytes."
        ));
    }
    if !id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(format!(
            "Filament spool ID '{id}' contains unsupported characters."
        ));
    }
    Ok(())
}

fn validate_text(label: &str, value: &str, minimum: usize, maximum: usize) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.len() < minimum || trimmed.len() > maximum {
        return Err(format!(
            "Filament {label} must contain {minimum} to {maximum} bytes."
        ));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(format!("Filament {label} contains control characters."));
    }
    Ok(())
}

fn parse_hex_color(value: &str) -> Result<RgbColor, String> {
    let digits = value.trim().strip_prefix('#').unwrap_or(value.trim());
    if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("Invalid spool HEX color '{value}'."));
    }
    let channel = |range: std::ops::Range<usize>| {
        u8::from_str_radix(&digits[range], 16)
            .map_err(|_| format!("Invalid spool HEX color '{value}'."))
    };
    Ok(RgbColor::new(
        channel(0..2)?,
        channel(2..4)?,
        channel(4..6)?,
    ))
}

fn parse_material(value: &str) -> Result<Material, String> {
    match value.trim().to_ascii_uppercase().as_str() {
        "PLA" => Ok(Material::Pla),
        "PETG" => Ok(Material::Petg),
        "PVA" => Ok(Material::Pva),
        _ => Err(format!(
            "Unsupported spool material '{value}'; the desktop inventory currently accepts PLA, PETG, or PVA."
        )),
    }
}

fn material_name(material: &Material) -> Option<&'static str> {
    match material {
        Material::Pla => Some("PLA"),
        Material::Petg => Some("PETG"),
        Material::Pva => Some("PVA"),
        Material::Abs | Material::Asa | Material::Tpu | Material::Other(_) => None,
    }
}

fn color_hex(color: RgbColor) -> String {
    format!("#{:02X}{:02X}{:02X}", color.red, color.green, color.blue)
}

fn is_built_in_spool(id: &str) -> bool {
    matches!(
        id,
        "panchroma-translucent-cyan"
            | "panchroma-translucent-magenta"
            | "panchroma-translucent-yellow"
            | "panchroma-translucent-grey"
            | "panchroma-basic-white"
            | "panchroma-basic-black"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_spool(id: &str, available: bool) -> FilamentSpoolView {
        FilamentSpoolView {
            id: id.to_owned(),
            calibration_identity: format!("calibration-{id}"),
            name: "Polymaker Orange".to_owned(),
            color_name: "Orange".to_owned(),
            hex: "#D97813".to_owned(),
            material: "PLA".to_owned(),
            sku: "PM-ORANGE".to_owned(),
            profile: "Generic PLA".to_owned(),
            vendor: "Polymaker".to_owned(),
            product_line: "Panchroma".to_owned(),
            optical_descriptor: "Opaque".to_owned(),
            min_nozzle_temperature_c: Some(190),
            max_nozzle_temperature_c: Some(230),
            batch_lot: "LOT-42".to_owned(),
            calibration_reference: "orange-flat-v1".to_owned(),
            notes: "Workshop shelf A".to_owned(),
            color_basis: "Nominal".to_owned(),
            source: "user".to_owned(),
            available,
        }
    }

    #[test]
    fn missing_file_loads_stable_built_in_library() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missing.json");
        let library = load_from_path(&path).unwrap();
        assert_eq!(library.schema_version, FILAMENT_LIBRARY_SCHEMA_VERSION);
        assert!(!library.spools.is_empty());
        assert!(
            library
                .spools
                .iter()
                .all(|spool| spool.source == "built-in")
        );
        assert!(library.spools.iter().all(|spool| !spool.available));
        assert!(
            library
                .spools
                .windows(2)
                .all(|pair| pair[0].id < pair[1].id)
        );
    }

    #[test]
    fn round_trip_preserves_unavailable_user_spools_and_builtin_overrides() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("library.json");
        let mut initial = load_from_path(&path).unwrap();
        initial
            .spools
            .iter_mut()
            .find(|spool| spool.id == "panchroma-translucent-grey")
            .unwrap()
            .available = false;
        initial.spools.push(user_spool("user-orange", false));

        let saved = save_to_path(&path, initial).unwrap();
        let loaded = load_from_path(&path).unwrap();
        assert_eq!(
            serde_json::to_value(&loaded).unwrap(),
            serde_json::to_value(&saved).unwrap()
        );
        assert!(
            !loaded
                .spools
                .iter()
                .find(|spool| spool.id == "panchroma-translucent-grey")
                .unwrap()
                .available
        );
        assert!(
            !loaded
                .spools
                .iter()
                .find(|spool| spool.id == "user-orange")
                .unwrap()
                .available
        );
        assert!(
            !loaded
                .planning_spools()
                .unwrap()
                .iter()
                .find(|spool| spool.id == "user-orange")
                .unwrap()
                .available
        );
    }

    #[test]
    fn built_in_metadata_cannot_be_redefined() {
        let mut forged = user_spool("panchroma-translucent-cyan", false);
        forged.source = "user".to_owned();
        let normalized = normalize_library(FilamentLibraryView {
            schema_version: FILAMENT_LIBRARY_SCHEMA_VERSION,
            spools: vec![forged],
        })
        .unwrap();
        let cyan = normalized
            .spools
            .iter()
            .find(|spool| spool.id == "panchroma-translucent-cyan")
            .unwrap();
        assert_eq!(cyan.source, "built-in");
        assert_ne!(cyan.name, "Polymaker Orange");
        assert!(!cyan.available);
    }

    #[test]
    fn replacement_save_deletes_omitted_users_but_restores_builtins() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("library.json");
        save_to_path(
            &path,
            FilamentLibraryView {
                schema_version: FILAMENT_LIBRARY_SCHEMA_VERSION,
                spools: vec![user_spool("user-orange", true)],
            },
        )
        .unwrap();
        let saved = save_to_path(
            &path,
            FilamentLibraryView {
                schema_version: FILAMENT_LIBRARY_SCHEMA_VERSION,
                spools: Vec::new(),
            },
        )
        .unwrap();
        assert!(saved.spools.iter().all(|spool| spool.id != "user-orange"));
        assert!(saved.spools.iter().any(|spool| spool.source == "built-in"));
    }

    #[test]
    fn rejects_schema_duplicates_reserved_ids_and_unknown_fields() {
        let invalid_version = normalize_library(FilamentLibraryView {
            schema_version: 4,
            spools: Vec::new(),
        })
        .unwrap_err();
        assert!(invalid_version.contains("schema version"));

        let duplicate = user_spool("user-orange", true);
        let duplicate_error = normalize_library(FilamentLibraryView {
            schema_version: FILAMENT_LIBRARY_SCHEMA_VERSION,
            spools: vec![duplicate.clone(), duplicate],
        })
        .unwrap_err();
        assert!(duplicate_error.contains("duplicated"));

        let reserved_error = normalize_library(FilamentLibraryView {
            schema_version: FILAMENT_LIBRARY_SCHEMA_VERSION,
            spools: vec![user_spool("source-forged", true)],
        })
        .unwrap_err();
        assert!(reserved_error.contains("reserved"));

        let unknown = br#"{"schemaVersion":1,"spools":[],"path":"/tmp/forbidden"}"#;
        assert!(serde_json::from_slice::<FilamentLibraryView>(unknown).is_err());
    }

    #[test]
    fn migrates_v1_atomically_without_losing_user_spools() {
        let directory = tempfile::tempdir().unwrap();
        let legacy_path = directory.path().join(LEGACY_FILAMENT_LIBRARY_FILE_NAME);
        let current_path = directory.path().join(FILAMENT_LIBRARY_FILE_NAME);
        let legacy = br##"{
          "schemaVersion": 1,
          "spools": [{
            "id": "workshop-red",
            "name": "Workshop Red",
            "colorName": "Red",
            "hex": "#C72E2A",
            "material": "PETG",
            "sku": "RED-01",
            "profile": "Generic PETG",
            "colorBasis": "Measured",
            "source": "user",
            "available": false
          }]
        }"##;
        fs::write(&legacy_path, legacy).unwrap();

        let raw_legacy: FilamentLibraryView = serde_json::from_slice(legacy).unwrap();
        assert_eq!(
            raw_legacy
                .planning_spools()
                .unwrap()
                .iter()
                .find(|spool| spool.id == "workshop-red")
                .unwrap()
                .calibration_id
                .as_deref(),
            Some("workshop-red")
        );

        let migrated = load_from_path(&legacy_path).unwrap();
        let user = migrated
            .spools
            .iter()
            .find(|spool| spool.id == "workshop-red")
            .unwrap();
        assert_eq!(migrated.schema_version, 3);
        assert_eq!(user.calibration_identity, "workshop-red");
        assert_eq!(user.sku, "RED-01");
        assert_eq!(user.profile, "Generic PETG");
        assert!(!user.available);

        save_to_path(&current_path, migrated).unwrap();
        assert_eq!(fs::read(&legacy_path).unwrap(), legacy);
        let published = load_from_path(&current_path).unwrap();
        assert_eq!(published.schema_version, 3);
        assert!(published.spools.iter().any(|spool| {
            spool.id == "workshop-red"
                && spool.calibration_identity == "workshop-red"
                && !spool.available
        }));
    }

    #[test]
    fn lot_or_calibration_reference_change_rotates_measured_identity() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(FILAMENT_LIBRARY_FILE_NAME);
        let initial = save_to_path(
            &path,
            FilamentLibraryView {
                schema_version: FILAMENT_LIBRARY_SCHEMA_VERSION,
                spools: vec![user_spool("user-orange", true)],
            },
        )
        .unwrap();
        let original_identity = initial
            .spools
            .iter()
            .find(|spool| spool.id == "user-orange")
            .unwrap()
            .calibration_identity
            .clone();

        let mut display_only_change = initial.clone();
        let user = display_only_change
            .spools
            .iter_mut()
            .find(|spool| spool.id == "user-orange")
            .unwrap();
        user.vendor = "Updated vendor".to_owned();
        user.calibration_identity = "untrusted-client-value".to_owned();
        let display_only_change = save_to_path(&path, display_only_change).unwrap();
        assert_eq!(
            display_only_change
                .spools
                .iter()
                .find(|spool| spool.id == "user-orange")
                .unwrap()
                .calibration_identity,
            original_identity
        );

        let mut lot_change = display_only_change;
        lot_change
            .spools
            .iter_mut()
            .find(|spool| spool.id == "user-orange")
            .unwrap()
            .batch_lot = "LOT-43".to_owned();
        let lot_change = save_to_path(&path, lot_change).unwrap();
        let rotated_identity = lot_change
            .spools
            .iter()
            .find(|spool| spool.id == "user-orange")
            .unwrap()
            .calibration_identity
            .clone();
        assert_ne!(rotated_identity, original_identity);
        assert!(rotated_identity.starts_with("spool-calibration:"));
        assert_eq!(
            lot_change
                .planning_spools()
                .unwrap()
                .iter()
                .find(|spool| spool.id == "user-orange")
                .unwrap()
                .calibration_id
                .as_deref(),
            Some(rotated_identity.as_str())
        );

        let mut reference_change = lot_change;
        reference_change
            .spools
            .iter_mut()
            .find(|spool| spool.id == "user-orange")
            .unwrap()
            .calibration_reference = "orange-flat-v2".to_owned();
        let reference_change = save_to_path(&path, reference_change).unwrap();
        assert_ne!(
            reference_change
                .spools
                .iter()
                .find(|spool| spool.id == "user-orange")
                .unwrap()
                .calibration_identity,
            rotated_identity
        );
    }

    #[test]
    fn rejects_invalid_temperature_range_and_duplicate_calibration_identity() {
        let mut invalid_range = user_spool("user-hot", true);
        invalid_range.min_nozzle_temperature_c = Some(260);
        invalid_range.max_nozzle_temperature_c = Some(230);
        let error = normalize_library(FilamentLibraryView {
            schema_version: FILAMENT_LIBRARY_SCHEMA_VERSION,
            spools: vec![invalid_range],
        })
        .unwrap_err();
        assert!(error.contains("cannot exceed"));

        let first = user_spool("user-first", true);
        let mut second = user_spool("user-second", true);
        second.calibration_identity = first.calibration_identity.clone();
        let error = normalize_library(FilamentLibraryView {
            schema_version: FILAMENT_LIBRARY_SCHEMA_VERSION,
            spools: vec![first, second],
        })
        .unwrap_err();
        assert!(error.contains("calibration identity"));
        assert!(error.contains("duplicated"));
    }

    #[test]
    fn rejects_oversized_library_before_json_parsing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("oversized.json");
        fs::write(&path, vec![b' '; MAX_LIBRARY_BYTES + 1]).unwrap();
        let error = load_from_path(&path).unwrap_err();
        assert!(error.contains("byte limit"));
    }
}
