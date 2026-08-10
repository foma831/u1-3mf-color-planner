use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use tauri::Manager;
use u1_application::{
    CMYX_CALIBRATION_LIBRARY_SCHEMA_VERSION, CmyxCalibrationLoadout, CmyxGeometryContext,
    CmyxMeasurementProvenance, UserCmyxCalibrationLibrary, UserCmyxCalibrationRecord,
};
use u1_color_engine::{CalibrationContext, GeometryClass, MixRecipe, SampleOrientation};

const CALIBRATION_LIBRARY_FILE_NAME: &str = "cmyx-calibration-library-v2.json";
const LEGACY_CALIBRATION_LIBRARY_FILE_NAME: &str = "cmyx-calibration-library-v1.json";
const MAX_CALIBRATION_LIBRARY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LegacyCmyxCalibrationLibraryViewV1 {
    schema_version: u32,
    planning_geometry_context: CmyxGeometryContext,
    records: Vec<LegacyUserCmyxCalibrationRecordV1>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LegacyUserCmyxCalibrationRecordV1 {
    id: String,
    loadout: CmyxCalibrationLoadout,
    context: CalibrationContext,
    recipe: MixRecipe,
    measured_output_hex: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CmyxCalibrationLibraryView {
    pub schema_version: u32,
    pub planning_geometry_context: CmyxGeometryContext,
    pub records: Vec<UserCmyxCalibrationRecord>,
}

impl Default for CmyxCalibrationLibraryView {
    fn default() -> Self {
        Self {
            schema_version: CMYX_CALIBRATION_LIBRARY_SCHEMA_VERSION,
            planning_geometry_context: CmyxGeometryContext::default(),
            records: Vec::new(),
        }
    }
}

impl CmyxCalibrationLibraryView {
    pub fn planning_data(
        &self,
    ) -> Result<(Vec<UserCmyxCalibrationRecord>, CmyxGeometryContext), String> {
        let normalized = normalize_library(self.clone())?;
        Ok((normalized.records, normalized.planning_geometry_context))
    }
}

pub fn load_for_app(app: &tauri::AppHandle) -> Result<CmyxCalibrationLibraryView, String> {
    let (path, legacy_path) = library_paths(app)?;
    load_with_migration(&path, &legacy_path)
}

pub fn upsert_for_app(
    app: &tauri::AppHandle,
    record: UserCmyxCalibrationRecord,
) -> Result<CmyxCalibrationLibraryView, String> {
    let (path, legacy_path) = library_paths(app)?;
    load_with_migration(&path, &legacy_path)?;
    upsert_at_path(&path, record)
}

pub fn delete_for_app(
    app: &tauri::AppHandle,
    record_id: &str,
) -> Result<CmyxCalibrationLibraryView, String> {
    let (path, legacy_path) = library_paths(app)?;
    load_with_migration(&path, &legacy_path)?;
    delete_at_path(&path, record_id)
}

pub fn set_geometry_for_app(
    app: &tauri::AppHandle,
    geometry_context: CmyxGeometryContext,
) -> Result<CmyxCalibrationLibraryView, String> {
    let (path, legacy_path) = library_paths(app)?;
    load_with_migration(&path, &legacy_path)?;
    set_geometry_at_path(&path, geometry_context)
}

fn library_paths(app: &tauri::AppHandle) -> Result<(PathBuf, PathBuf), String> {
    app.path()
        .app_data_dir()
        .map(|directory| {
            (
                directory.join(CALIBRATION_LIBRARY_FILE_NAME),
                directory.join(LEGACY_CALIBRATION_LIBRARY_FILE_NAME),
            )
        })
        .map_err(|error| format!("Failed to resolve the application data directory: {error}"))
}

fn load_with_migration(
    path: &Path,
    legacy_path: &Path,
) -> Result<CmyxCalibrationLibraryView, String> {
    if path.exists() {
        return load_from_path(path);
    }
    if !legacy_path.exists() {
        return Ok(CmyxCalibrationLibraryView::default());
    }
    let bytes = read_limited_library(legacy_path)?;
    let legacy =
        serde_json::from_slice::<LegacyCmyxCalibrationLibraryViewV1>(&bytes).map_err(|error| {
            format!("The CMY+X calibration library is not valid schema v1 JSON: {error}")
        })?;
    if legacy.schema_version != 1 {
        return Err(format!(
            "Unsupported legacy CMY+X calibration library schema version {}; expected 1.",
            legacy.schema_version
        ));
    }
    let migrated = CmyxCalibrationLibraryView {
        schema_version: CMYX_CALIBRATION_LIBRARY_SCHEMA_VERSION,
        planning_geometry_context: legacy.planning_geometry_context,
        records: legacy
            .records
            .into_iter()
            .map(|record| UserCmyxCalibrationRecord {
                id: record.id,
                loadout: record.loadout,
                context: record.context,
                recipe: record.recipe,
                measured_output_hex: record.measured_output_hex,
                provenance: CmyxMeasurementProvenance::legacy_unverified(),
            })
            .collect(),
    };
    save_to_path(path, migrated)
}

fn read_limited_library(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("Failed to inspect the CMY+X calibration library: {error}"))?;
    if metadata.len() > MAX_CALIBRATION_LIBRARY_BYTES as u64 {
        return Err(format!(
            "The CMY+X calibration library exceeds the {MAX_CALIBRATION_LIBRARY_BYTES} byte limit."
        ));
    }
    let mut file = File::open(path)
        .map_err(|error| format!("Failed to open the CMY+X calibration library: {error}"))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    Read::by_ref(&mut file)
        .take((MAX_CALIBRATION_LIBRARY_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Failed to read the CMY+X calibration library: {error}"))?;
    if bytes.len() > MAX_CALIBRATION_LIBRARY_BYTES {
        return Err(format!(
            "The CMY+X calibration library exceeds the {MAX_CALIBRATION_LIBRARY_BYTES} byte limit."
        ));
    }
    Ok(bytes)
}

fn load_from_path(path: &Path) -> Result<CmyxCalibrationLibraryView, String> {
    if !path.exists() {
        return Ok(CmyxCalibrationLibraryView::default());
    }
    let bytes = read_limited_library(path)?;
    let library =
        serde_json::from_slice::<CmyxCalibrationLibraryView>(&bytes).map_err(|error| {
            format!("The CMY+X calibration library is not valid schema v2 JSON: {error}")
        })?;
    normalize_library(library)
}

fn save_to_path(
    path: &Path,
    library: CmyxCalibrationLibraryView,
) -> Result<CmyxCalibrationLibraryView, String> {
    let normalized = normalize_library(library)?;
    let bytes = serde_json::to_vec_pretty(&normalized)
        .map_err(|error| format!("Failed to serialize the CMY+X calibration library: {error}"))?;
    if bytes.len() > MAX_CALIBRATION_LIBRARY_BYTES {
        return Err(format!(
            "The CMY+X calibration library exceeds the {MAX_CALIBRATION_LIBRARY_BYTES} byte limit."
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| "The CMY+X calibration library path has no parent directory.".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Failed to create the application data directory: {error}"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|error| {
        format!("Failed to create a temporary CMY+X calibration library: {error}")
    })?;
    temporary
        .write_all(&bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| format!("Failed to write the CMY+X calibration library: {error}"))?;
    temporary.persist(path).map_err(|error| {
        format!("Failed to publish the CMY+X calibration library atomically: {error}")
    })?;
    sync_parent_directory(parent)?;
    Ok(normalized)
}

fn upsert_at_path(
    path: &Path,
    record: UserCmyxCalibrationRecord,
) -> Result<CmyxCalibrationLibraryView, String> {
    let mut view = load_from_path(path)?;
    let mut library = UserCmyxCalibrationLibrary::from_records(view.records)
        .map_err(|error| error.to_string())?;
    library.upsert(record).map_err(|error| error.to_string())?;
    view.records = library.records;
    save_to_path(path, view)
}

fn delete_at_path(path: &Path, record_id: &str) -> Result<CmyxCalibrationLibraryView, String> {
    let mut view = load_from_path(path)?;
    let mut library = UserCmyxCalibrationLibrary::from_records(view.records)
        .map_err(|error| error.to_string())?;
    let record_id = record_id.trim();
    if record_id.is_empty() {
        return Err("CMY+X calibration record ID must not be empty.".to_owned());
    }
    library
        .remove(record_id)
        .ok_or_else(|| format!("CMY+X calibration record {record_id:?} does not exist."))?;
    view.records = library.records;
    save_to_path(path, view)
}

fn set_geometry_at_path(
    path: &Path,
    geometry_context: CmyxGeometryContext,
) -> Result<CmyxCalibrationLibraryView, String> {
    let mut view = load_from_path(path)?;
    view.planning_geometry_context = normalize_geometry_context(geometry_context)?;
    save_to_path(path, view)
}

fn normalize_library(
    mut library: CmyxCalibrationLibraryView,
) -> Result<CmyxCalibrationLibraryView, String> {
    if library.schema_version != CMYX_CALIBRATION_LIBRARY_SCHEMA_VERSION {
        return Err(format!(
            "Unsupported CMY+X calibration library schema version {}; expected {}.",
            library.schema_version, CMYX_CALIBRATION_LIBRARY_SCHEMA_VERSION
        ));
    }
    let records = UserCmyxCalibrationLibrary::from_records(library.records)
        .map_err(|error| error.to_string())?;
    library.schema_version = CMYX_CALIBRATION_LIBRARY_SCHEMA_VERSION;
    library.records = records.records;
    library.planning_geometry_context =
        normalize_geometry_context(library.planning_geometry_context)?;
    Ok(library)
}

fn normalize_geometry_context(
    mut context: CmyxGeometryContext,
) -> Result<CmyxGeometryContext, String> {
    let unknown_orientation = matches!(context.orientation, SampleOrientation::Unknown);
    let unknown_geometry = matches!(context.geometry_class, GeometryClass::Unknown);
    if unknown_orientation != unknown_geometry {
        return Err(
            "CMY+X planning geometry must either identify both orientation and geometry class or leave both unknown."
                .to_owned(),
        );
    }
    if let SampleOrientation::Custom(value) = &mut context.orientation {
        *value = value.trim().to_owned();
        if value.is_empty() {
            return Err("Custom CMY+X sample orientation must not be empty.".to_owned());
        }
    }
    if let GeometryClass::Custom(value) = &mut context.geometry_class {
        *value = value.trim().to_owned();
        if value.is_empty() {
            return Err("Custom CMY+X geometry class must not be empty.".to_owned());
        }
    }
    Ok(context)
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> Result<(), String> {
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("Failed to finalize the CMY+X calibration update: {error}"))
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use u1_application::{
        CmyxMeasurementMethod, CmyxMeasurementProvenance, full_spectrum_calibration_context,
    };
    use u1_color_engine::{MixRecipe, RecipeComponent, RecipeMode};

    fn geometry() -> CmyxGeometryContext {
        CmyxGeometryContext {
            orientation: SampleOrientation::Upright,
            geometry_class: GeometryClass::CalibrationSwatch,
        }
    }

    fn record(id: &str) -> UserCmyxCalibrationRecord {
        let loadout = CmyxCalibrationLoadout::new(
            "panchroma-translucent-cyan",
            "panchroma-translucent-magenta",
            "panchroma-translucent-yellow",
            "panchroma-basic-black",
        );
        UserCmyxCalibrationRecord {
            id: id.to_owned(),
            context: full_spectrum_calibration_context(&loadout, &geometry()),
            loadout,
            recipe: MixRecipe {
                mode: RecipeMode::Ratio,
                components: vec![
                    RecipeComponent { slot: 1, weight: 3 },
                    RecipeComponent { slot: 4, weight: 1 },
                ],
            },
            measured_output_hex: "#2A6F51".to_owned(),
            provenance: CmyxMeasurementProvenance::verified(
                "2026-08-01T19:15:00Z",
                CmyxMeasurementMethod::InstrumentSrgb,
            ),
        }
    }

    #[test]
    fn persistent_library_round_trips_upserts_and_deletes_atomically() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("calibration.json");
        assert_eq!(
            load_from_path(&path).unwrap(),
            CmyxCalibrationLibraryView::default()
        );

        let saved = upsert_at_path(&path, record("black-r1")).unwrap();
        assert_eq!(saved.records.len(), 1);
        let mut updated = record("black-r1");
        updated.measured_output_hex = "#ABCDEF".to_owned();
        let updated = upsert_at_path(&path, updated).unwrap();
        assert_eq!(updated.records[0].measured_output_hex, "#ABCDEF");
        assert_eq!(load_from_path(&path).unwrap(), updated);

        let deleted = delete_at_path(&path, "black-r1").unwrap();
        assert!(deleted.records.is_empty());
        assert!(
            delete_at_path(&path, "black-r1")
                .unwrap_err()
                .contains("does not exist")
        );
    }

    #[test]
    fn persistence_rejects_unknown_fields_invalid_geometry_and_oversized_input() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("calibration.json");
        fs::write(
            &path,
            br#"{"schemaVersion":1,"planningGeometryContext":{"orientation":"unknown","geometryClass":"unknown"},"records":[],"sourcePath":"/tmp/forbidden"}"#,
        )
        .unwrap();
        assert!(
            load_from_path(&path)
                .unwrap_err()
                .contains("not valid schema")
        );

        assert!(
            normalize_library(CmyxCalibrationLibraryView {
                planning_geometry_context: CmyxGeometryContext {
                    orientation: SampleOrientation::Upright,
                    geometry_class: GeometryClass::Unknown,
                },
                ..CmyxCalibrationLibraryView::default()
            })
            .unwrap_err()
            .contains("either identify both")
        );

        fs::write(&path, vec![b' '; MAX_CALIBRATION_LIBRARY_BYTES + 1]).unwrap();
        assert!(load_from_path(&path).unwrap_err().contains("byte limit"));
    }

    #[test]
    fn active_geometry_is_persisted_separately_from_sample_identity() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("calibration.json");
        upsert_at_path(&path, record("black-r1")).unwrap();
        let selected = CmyxGeometryContext {
            orientation: SampleOrientation::Flat,
            geometry_class: GeometryClass::TopSurface,
        };
        let saved = set_geometry_at_path(&path, selected.clone()).unwrap();
        assert_eq!(saved.planning_geometry_context, selected);
        assert_eq!(
            saved.records[0].context.geometry_class,
            GeometryClass::CalibrationSwatch
        );
    }

    #[test]
    fn schema_v1_is_migrated_without_deleting_or_qualifying_legacy_measurements() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(CALIBRATION_LIBRARY_FILE_NAME);
        let legacy_path = directory.path().join(LEGACY_CALIBRATION_LIBRARY_FILE_NAME);
        let legacy_record = record("legacy-black-r1");
        let legacy = serde_json::json!({
            "schemaVersion": 1,
            "planningGeometryContext": geometry(),
            "records": [{
                "id": legacy_record.id,
                "loadout": legacy_record.loadout,
                "context": legacy_record.context,
                "recipe": legacy_record.recipe,
                "measuredOutputHex": legacy_record.measured_output_hex,
            }],
        });
        let legacy_bytes = serde_json::to_vec_pretty(&legacy).unwrap();
        fs::write(&legacy_path, &legacy_bytes).unwrap();

        let migrated = load_with_migration(&path, &legacy_path).unwrap();
        assert_eq!(migrated.schema_version, 2);
        assert_eq!(
            migrated.records[0].provenance,
            CmyxMeasurementProvenance::legacy_unverified()
        );
        assert!(path.is_file());
        assert_eq!(fs::read(&legacy_path).unwrap(), legacy_bytes);
        assert!(
            UserCmyxCalibrationLibrary::from_records(migrated.records)
                .unwrap()
                .engine_samples()
                .unwrap()
                .is_empty()
        );
    }
}
