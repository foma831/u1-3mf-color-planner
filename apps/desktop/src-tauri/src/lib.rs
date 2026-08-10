mod calibration_library;
mod conversion_report;
mod filament_library;
#[cfg(test)]
mod native_e2e;
mod view;

use calibration_library::{
    CmyxCalibrationLibraryView, delete_for_app as delete_calibration_for_app,
    load_for_app as load_calibration_for_app,
    set_geometry_for_app as set_calibration_geometry_for_app,
    upsert_for_app as upsert_calibration_for_app,
};
use conversion_report::render_conversion_report;
use filament_library::{FilamentLibraryView, load_for_app, save_for_app};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager};
use u1_a1mini_adapter::{
    A1MiniPreparation, convert_a1mini_plates_cancellable, discover_bambu_studio,
    inspect_a1mini_macos_application, prepare_a1mini_conversion, validate_a1mini_output,
    validate_a1mini_output_against_plan, validate_a1mini_outputs_against_plan,
};
use u1_application::{
    CmyxCalibrationLoadout, CmyxCalibrationProjectValidationReport, CmyxGeometryContext,
    CmyxMeasurementMethod, CmyxMeasurementProvenance, ConversionPlanSlice, ConversionTarget,
    ExcludedSourceUnit, PartialConversionApproval, UserCmyxCalibrationRecord,
    canonical_partial_conversion_exclusions, full_spectrum_calibration_context,
    recommended_cmyx_calibration_project_spec, slice_all_conversion_targets_with_partial_approval,
    source_dialect_approval_fingerprint,
    validate_cmyx_calibration_project_candidate as validate_cmyx_calibration_artifact,
    validate_source_dialect_for_conversion, write_cmyx_calibration_project_candidate,
};
use u1_color_engine::MixRecipe;
use u1_orca_adapter::{
    U1DirectConversionControl, U1DirectConversionState, U1DirectError, U1DirectPreparation,
    U1DirectStagingCleanup, U1DirectStagingRecoveryRecord, U1FullSpectrumError,
    U1FullSpectrumPreparation, U1NativeConversionStaging,
    build_u1_full_spectrum_project_settings_with_physical_profiles, canonical_plan_fingerprint,
    cleanup_abandoned_u1_direct_staging, convert_u1_direct_bundle_cancellable,
    convert_u1_full_spectrum_with_substrate_builder_cancellable, discover_installation,
    finalize_u1_direct_staging, inspect_macos_application, inspect_u1_direct_macos_application,
    inspect_u1_full_spectrum_macos_application, prepare_u1_direct_conversion,
    prepare_u1_direct_conversion_cancellable, prepare_u1_full_spectrum_conversion,
    qualified_u1_physical_profiles_root, resolve_u1_full_spectrum_physical_loadout,
    validate_u1_direct_output_against_plan, validate_u1_direct_outputs_against_plan,
    validate_u1_full_spectrum_output_against_substrate,
    write_u1_full_spectrum_normalized_substrate, write_u1_full_spectrum_normalized_substrates,
};
use u1_planner::{ColorStrategy, PlanningInput, PlanningResult};
use u1_three_mf::{
    DialectSupport, InputIdentity, ProjectAnalysis, ProjectDialect, SourceApplication,
};
use view::{PlanningRequestView, ProjectPlanView};

#[derive(Default)]
struct ProjectCache {
    operation_generation: Arc<Mutex<u64>>,
    active_analysis: Mutex<Option<u64>>,
    calibration_mutations: Mutex<()>,
    analyzed: Mutex<Option<CachedProject>>,
    prepared: Mutex<Option<CachedPreparation>>,
    active_conversions: Mutex<HashMap<String, MixedConversionControl>>,
    published_outputs: Arc<Mutex<HashMap<PathBuf, RegisteredPublishedOutput>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CancelAnalysisResult {
    accepted: bool,
}

struct CachedProject {
    canonical_path: PathBuf,
    analysis: ProjectAnalysis,
    latest_plan: CachedPlan,
    planning_input: PlanningInput,
    planning_result: PlanningResult,
    plan_fingerprint: String,
}

#[derive(Clone)]
struct CachedPreparation {
    token: String,
    cache_generation: u64,
    canonical_path: PathBuf,
    source_sha256: String,
    plan_fingerprint: String,
    created_at: Instant,
    preparation: NativeConversionPreparation,
    targets: PreparedTargetSlices,
    partial_conversion_approval: Option<PartialConversionApproval>,
    experimental_dialect_approval: Option<ExperimentalDialectApproval>,
    control: MixedConversionControl,
}

#[derive(Clone, PartialEq)]
struct PreparedTargetSlices {
    direct: Option<ConversionPlanSlice>,
    full_spectrum: Option<ConversionPlanSlice>,
    a1_mini: Option<ConversionPlanSlice>,
    excluded_source_units: Vec<ExcludedSourceUnit>,
}

struct NativeAdapterPreparations<'a> {
    direct: Option<&'a U1DirectPreparation>,
    full_spectrum: Option<&'a U1FullSpectrumPreparation>,
    a1_mini: Option<&'a A1MiniPreparation>,
}

struct OwnedNativeAdapterPreparations {
    direct: Option<U1DirectPreparation>,
    full_spectrum: Option<U1FullSpectrumPreparation>,
    a1_mini: Option<A1MiniPreparation>,
}

impl OwnedNativeAdapterPreparations {
    fn as_refs(&self) -> NativeAdapterPreparations<'_> {
        NativeAdapterPreparations {
            direct: self.direct.as_ref(),
            full_spectrum: self.full_spectrum.as_ref(),
            a1_mini: self.a1_mini.as_ref(),
        }
    }
}

#[derive(Clone)]
struct MixedConversionControl {
    outer: U1DirectConversionControl,
    children: Arc<Mutex<HashMap<String, U1DirectConversionControl>>>,
}

impl MixedConversionControl {
    fn new() -> Self {
        Self {
            outer: U1DirectConversionControl::new(),
            children: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn conversion_id(&self) -> &str {
        self.outer.conversion_id()
    }

    fn state(&self) -> U1DirectConversionState {
        self.outer.state()
    }

    fn recovery_record(
        &self,
        destination_parent: &Path,
    ) -> Result<U1DirectStagingRecoveryRecord, U1DirectError> {
        self.outer.recovery_record(destination_parent)
    }

    fn cancel(&self) -> bool {
        let accepted = self.outer.cancel();
        if let Ok(children) = self.children.lock() {
            for child in children.values() {
                let _ = child.cancel();
            }
        }
        accepted
    }

    fn register_child(&self, child: U1DirectConversionControl) -> Result<(), String> {
        let mut children = self
            .children
            .lock()
            .map_err(|_| "conversion child registry is unavailable".to_owned())?;
        // The child lock is the linearization point shared with `cancel`.
        // Either cancellation observes the registered child, or registration
        // observes the cancelled outer control and stops the child itself.
        if self.state() == U1DirectConversionState::Cancelled {
            let _ = child.cancel();
            return Err(
                "conversion_cancelled: Conversion was cancelled before publication.".into(),
            );
        }
        let child_id = child.conversion_id().to_owned();
        if children.contains_key(&child_id) {
            return Err("conversion child ID is already registered".into());
        }
        children.insert(child_id, child);
        Ok(())
    }

    fn remove_child(&self, conversion_id: &str) {
        if let Ok(mut children) = self.children.lock() {
            children.remove(conversion_id);
        }
    }
}

const CONVERSION_PREPARATION_TTL: Duration = Duration::from_secs(15 * 60);
const CONVERSION_RECOVERY_DIRECTORY: &str = "conversion-staging-v1";
const PUBLICATION_RECEIPT_DIRECTORY: &str = "publication-receipts-v1";
const UNRECEIPTED_QUARANTINE_PREFIX: &str = ".u1-planner-unreceipted-";
const UNTRUSTED_BUNDLE_RETRY_GUIDANCE: &str = "Select a different destination, or move/remove the untrusted existing bundle before converting again; the converter never overwrites an existing output path.";
const MAX_CONVERSION_RECOVERY_RECORD_BYTES: u64 = 64 * 1024;
const MAX_PUBLICATION_RECEIPT_BYTES: u64 = 16 * 1024 * 1024;
const PUBLICATION_RECEIPT_SCHEMA_VERSION: u32 = 1;
const MAX_PRIVATE_ADAPTER_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PUBLISHED_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PUBLISHED_CONVERSION_PLAN_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PUBLISHED_REPORT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PUBLISHED_CHECKSUM_BYTES: u64 = 1024 * 1024;
#[allow(dead_code)]
const MAX_PUBLISHED_ARTIFACT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const PUBLISHED_BUNDLE_SCHEMA_VERSION: u32 = 2;
const PUBLISHED_MANIFEST_FILE_NAME: &str = "manifest.json";
const PUBLISHED_CONVERSION_PLAN_FILE_NAME: &str = "conversion-plan.json";
const PUBLISHED_CONVERSION_REPORT_FILE_NAME: &str = "conversion-report.html";
const PUBLISHED_CHECKSUMS_FILE_NAME: &str = "checksums.sha256";

#[derive(Clone)]
struct CachedPlan {
    value: serde_json::Value,
    contents: String,
    invalidated: bool,
}

impl CachedPlan {
    fn from_view(view: &ProjectPlanView) -> Result<Self, String> {
        let value = serde_json::to_value(view).map_err(|error| {
            format!("Failed to serialize the authoritative print plan: {error}")
        })?;
        let contents = serde_json::to_string_pretty(&value).map_err(|error| {
            format!("Failed to serialize the authoritative print plan: {error}")
        })?;
        Ok(Self {
            value,
            contents,
            invalidated: false,
        })
    }

    fn is_ready(&self) -> bool {
        self.value
            .get("planReady")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    }

    fn invalidate_inventory(&mut self) {
        self.invalidated = true;
        if !self.value.is_object() {
            // CachedPlan values originate from a typed ProjectPlanView, so
            // this is only a fail-closed guard against internal corruption.
            self.value = serde_json::json!({});
        }
        let object = self
            .value
            .as_object_mut()
            .expect("the cached plan was normalized to a JSON object");
        object.insert("planReady".into(), serde_json::Value::Bool(false));
        self.contents = serde_json::to_string_pretty(&self.value)
            .unwrap_or_else(|_| "{\n  \"planReady\": false\n}".into());
    }
}

fn begin_cache_operation(cache: &ProjectCache) -> Result<u64, String> {
    begin_cache_operation_kind(cache, false)
}

fn begin_analysis_operation(cache: &ProjectCache) -> Result<u64, String> {
    begin_cache_operation_kind(cache, true)
}

fn begin_cache_operation_kind(
    cache: &ProjectCache,
    register_analysis: bool,
) -> Result<u64, String> {
    // The generation lock is the transaction boundary shared with inventory
    // invalidation and successful-output registration. A preparation cannot
    // be installed between advancing the generation and evicting its
    // predecessor.
    let mut generation = cache
        .operation_generation
        .lock()
        .map_err(|_| "project operation generation is unavailable".to_owned())?;
    let mut prepared = cache
        .prepared
        .lock()
        .map_err(|_| "conversion preparation cache is unavailable".to_owned())?;
    let mut active_analysis = cache
        .active_analysis
        .lock()
        .map_err(|_| "active analysis registry is unavailable".to_owned())?;
    let active_controls = cache
        .active_conversions
        .lock()
        .map_err(|_| "active conversion registry is unavailable".to_owned())?
        .values()
        .cloned()
        .collect::<Vec<_>>();
    *generation = generation
        .checked_add(1)
        .ok_or_else(|| "project operation generation overflowed".to_owned())?;
    let current_generation = *generation;
    if register_analysis {
        *active_analysis = Some(current_generation);
    } else {
        active_analysis.take();
    }
    let previous = prepared.take();
    drop(prepared);
    drop(active_analysis);
    drop(generation);
    if let Some(previous) = previous {
        let _ = previous.control.cancel();
    }
    for control in active_controls {
        let _ = control.cancel();
    }
    Ok(current_generation)
}

fn invalidate_inventory_dependent_state(cache: &ProjectCache) -> Result<(), String> {
    // Acquire every planning-dependent cache behind the generation lock
    // before changing any of them. Readers/registrars that participate in the
    // same generation protocol therefore observe either the old complete
    // state or the invalidated complete state.
    let mut generation = cache
        .operation_generation
        .lock()
        .map_err(|_| "project operation generation is unavailable".to_owned())?;
    let mut prepared = cache
        .prepared
        .lock()
        .map_err(|_| "conversion preparation cache is unavailable".to_owned())?;
    let mut active_analysis = cache
        .active_analysis
        .lock()
        .map_err(|_| "active analysis registry is unavailable".to_owned())?;
    let active_controls = cache
        .active_conversions
        .lock()
        .map_err(|_| "active conversion registry is unavailable".to_owned())?
        .values()
        .cloned()
        .collect::<Vec<_>>();
    let mut analyzed = cache
        .analyzed
        .lock()
        .map_err(|_| "project cache is unavailable".to_owned())?;
    let mut published_outputs = cache
        .published_outputs
        .lock()
        .map_err(|_| "published output action registry is unavailable".to_owned())?;

    let invalidated_plan = analyzed.as_ref().map(|cached| {
        let mut plan = cached.latest_plan.clone();
        plan.invalidate_inventory();
        plan
    });
    let next_generation = generation
        .checked_add(1)
        .ok_or_else(|| "project operation generation overflowed".to_owned())?;

    *generation = next_generation;
    active_analysis.take();
    let previous = prepared.take();
    if let (Some(cached), Some(plan)) = (analyzed.as_mut(), invalidated_plan) {
        cached.latest_plan = plan;
    }
    published_outputs.clear();
    drop(published_outputs);
    drop(analyzed);
    drop(prepared);
    drop(active_analysis);
    drop(generation);
    if let Some(previous) = previous {
        let _ = previous.control.cancel();
    }
    for control in active_controls {
        let _ = control.cancel();
    }
    Ok(())
}

fn ensure_current_cache_operation<'a>(
    cache: &'a ProjectCache,
    expected_generation: u64,
    operation: &str,
) -> Result<std::sync::MutexGuard<'a, u64>, String> {
    let generation = cache
        .operation_generation
        .lock()
        .map_err(|_| "project operation generation is unavailable".to_owned())?;
    if *generation != expected_generation {
        return Err(format!(
            "The {operation} request was superseded by a newer project operation."
        ));
    }
    Ok(generation)
}

fn clear_analysis_registration(cache: &ProjectCache, generation: u64) -> Result<(), String> {
    let _generation = cache
        .operation_generation
        .lock()
        .map_err(|_| "project operation generation is unavailable".to_owned())?;
    let mut active_analysis = cache
        .active_analysis
        .lock()
        .map_err(|_| "active analysis registry is unavailable".to_owned())?;
    if *active_analysis == Some(generation) {
        active_analysis.take();
    }
    Ok(())
}

fn cancel_active_analysis(cache: &ProjectCache) -> Result<CancelAnalysisResult, String> {
    let mut generation = cache
        .operation_generation
        .lock()
        .map_err(|_| "project operation generation is unavailable".to_owned())?;
    let mut active_analysis = cache
        .active_analysis
        .lock()
        .map_err(|_| "active analysis registry is unavailable".to_owned())?;
    let Some(analysis_generation) = *active_analysis else {
        return Ok(CancelAnalysisResult { accepted: false });
    };
    if analysis_generation != *generation {
        active_analysis.take();
        return Ok(CancelAnalysisResult { accepted: false });
    }
    *generation = generation
        .checked_add(1)
        .ok_or_else(|| "project operation generation overflowed".to_owned())?;
    active_analysis.take();
    // Analysis reads an immutable snapshot and has a bounded runtime. The
    // parser may finish in the background after this linearization point, but
    // its stale generation can no longer update the authoritative cache.
    Ok(CancelAnalysisResult { accepted: true })
}

#[tauri::command]
fn cancel_analysis(cache: tauri::State<'_, ProjectCache>) -> Result<CancelAnalysisResult, String> {
    cancel_active_analysis(&cache)
}

#[tauri::command]
async fn analyze_project(
    source_path: String,
    app: tauri::AppHandle,
    cache: tauri::State<'_, ProjectCache>,
) -> Result<ProjectPlanView, String> {
    let canonical_path = canonical_project_path(&source_path)?;
    let generation = begin_analysis_operation(&cache)?;
    let planning_data = (|| {
        let library_spools = load_for_app(&app)?.planning_spools()?;
        let (calibration_samples, calibration_geometry) =
            load_calibration_for_app(&app)?.planning_data()?;
        Ok::<_, String>((library_spools, calibration_samples, calibration_geometry))
    })();
    let (library_spools, calibration_samples, calibration_geometry) = match planning_data {
        Ok(planning_data) => planning_data,
        Err(error) => {
            clear_analysis_registration(&cache, generation)?;
            return Err(error);
        }
    };
    let worker_path = canonical_path.clone();
    let worker_result = tauri::async_runtime::spawn_blocking(move || {
        view::analyze_native_project_data_with_backend_state(
            worker_path.to_string_lossy().as_ref(),
            library_spools,
            calibration_samples,
            calibration_geometry,
        )
    })
    .await;
    let (analysis, outcome) = match worker_result {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => {
            clear_analysis_registration(&cache, generation)?;
            return Err(error.to_string());
        }
        Err(error) => {
            clear_analysis_registration(&cache, generation)?;
            return Err(format!("analysis worker failed: {error}"));
        }
    };
    let prepared_result = (|| {
        let latest_plan = CachedPlan::from_view(&outcome.view)?;
        let plan_fingerprint = canonical_plan_fingerprint(&outcome.input, &outcome.result)
            .map_err(|error| error.to_string())?;
        Ok::<_, String>((latest_plan, plan_fingerprint))
    })();
    let (latest_plan, plan_fingerprint) = match prepared_result {
        Ok(prepared) => prepared,
        Err(error) => {
            clear_analysis_registration(&cache, generation)?;
            return Err(error);
        }
    };
    let project_view = outcome.view;
    let _generation = match ensure_current_cache_operation(&cache, generation, "analysis") {
        Ok(generation) => generation,
        Err(error) => {
            clear_analysis_registration(&cache, generation)?;
            return Err(error);
        }
    };
    let mut active_analysis = cache
        .active_analysis
        .lock()
        .map_err(|_| "active analysis registry is unavailable".to_owned())?;
    if *active_analysis != Some(generation) {
        return Err("The analysis registration is no longer active.".to_owned());
    }
    active_analysis.take();
    let mut cached = cache
        .analyzed
        .lock()
        .map_err(|_| "project cache is unavailable".to_owned())?;
    *cached = Some(CachedProject {
        canonical_path,
        analysis,
        latest_plan,
        planning_input: outcome.input,
        planning_result: outcome.result,
        plan_fingerprint,
    });
    Ok(project_view)
}

#[tauri::command]
async fn replan_project(
    source_path: String,
    request: PlanningRequestView,
    app: tauri::AppHandle,
    cache: tauri::State<'_, ProjectCache>,
) -> Result<ProjectPlanView, String> {
    let canonical_path = canonical_project_path(&source_path)?;
    let generation = begin_cache_operation(&cache)?;
    let library_spools = load_for_app(&app)?.planning_spools()?;
    let (calibration_samples, calibration_geometry) =
        load_calibration_for_app(&app)?.planning_data()?;
    let analysis = {
        let cached = cache
            .analyzed
            .lock()
            .map_err(|_| "project cache is unavailable".to_owned())?;
        let cached = cached.as_ref().ok_or_else(|| {
            "Analyze the selected 3MF before applying planning choices.".to_owned()
        })?;
        if cached.canonical_path != canonical_path {
            return Err(
                "The requested 3MF does not match the project in the analysis cache.".to_owned(),
            );
        }
        cached.analysis.clone()
    };
    let analyzed_byte_size = analysis.input.byte_size;
    let analyzed_sha256 = analysis.input.sha256.clone();
    let worker_path = canonical_path.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        validate_cached_identity(&worker_path, &analysis)?;
        view::replan_native_project_view_with_backend_state(
            worker_path.to_string_lossy().as_ref(),
            &analysis,
            request,
            library_spools,
            calibration_samples,
            calibration_geometry,
        )
    })
    .await
    .map_err(|error| format!("planning worker failed: {error}"))??;
    let latest_plan = CachedPlan::from_view(&outcome.view)?;
    let plan_fingerprint = canonical_plan_fingerprint(&outcome.input, &outcome.result)
        .map_err(|error| error.to_string())?;
    let project_view = outcome.view;
    let _generation = ensure_current_cache_operation(&cache, generation, "planning")?;
    let mut cached = cache
        .analyzed
        .lock()
        .map_err(|_| "project cache is unavailable".to_owned())?;
    let cached = cached.as_mut().ok_or_else(|| {
        "The analyzed project was cleared before planning completed. Analyze it again.".to_owned()
    })?;
    if cached.canonical_path != canonical_path
        || cached.analysis.input.byte_size != analyzed_byte_size
        || cached.analysis.input.sha256 != analyzed_sha256
    {
        return Err(
            "The analyzed project changed before planning completed. Analyze it again.".to_owned(),
        );
    }
    cached.latest_plan = latest_plan;
    cached.planning_input = outcome.input;
    cached.planning_result = outcome.result;
    cached.plan_fingerprint = plan_fingerprint;
    Ok(project_view)
}

#[tauri::command]
fn load_filament_library(app: tauri::AppHandle) -> Result<FilamentLibraryView, String> {
    load_for_app(&app)
}

#[tauri::command]
fn save_filament_library(
    app: tauri::AppHandle,
    library: FilamentLibraryView,
    cache: tauri::State<'_, ProjectCache>,
) -> Result<FilamentLibraryView, String> {
    let saved = save_for_app(&app, library)?;
    invalidate_inventory_dependent_state(&cache)?;
    Ok(saved)
}

fn commit_calibration_library_change<T>(
    cache: &ProjectCache,
    persist: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let _mutation = cache
        .calibration_mutations
        .lock()
        .map_err(|_| "CMY+X calibration library transaction lock is unavailable".to_owned())?;
    let saved = persist()?;
    invalidate_inventory_dependent_state(cache)?;
    Ok(saved)
}

#[tauri::command]
fn load_cmyx_calibration_library(
    app: tauri::AppHandle,
) -> Result<CmyxCalibrationLibraryView, String> {
    load_calibration_for_app(&app)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CmyxCalibrationMeasurementInput {
    id: String,
    loadout: CmyxCalibrationLoadout,
    recipe: MixRecipe,
    measured_output_hex: String,
    geometry_context: CmyxGeometryContext,
    provenance: CmyxMeasurementProvenance,
    physical_measurement_confirmed: bool,
}

impl CmyxCalibrationMeasurementInput {
    fn into_record(self) -> Result<UserCmyxCalibrationRecord, String> {
        if !self.physical_measurement_confirmed {
            return Err(
                "Confirm that the color came from a physical printed swatch, not a nominal preview."
                    .to_owned(),
            );
        }
        if self.provenance.method == CmyxMeasurementMethod::LegacyUnverified {
            return Err("New measurements cannot use legacy unverified provenance.".to_owned());
        }
        let context = full_spectrum_calibration_context(&self.loadout, &self.geometry_context);
        let record = UserCmyxCalibrationRecord {
            id: self.id,
            loadout: self.loadout,
            context,
            recipe: self.recipe,
            measured_output_hex: self.measured_output_hex,
            provenance: self.provenance,
        };
        record.validate().map_err(|error| error.to_string())?;
        Ok(record)
    }
}

#[tauri::command]
fn upsert_cmyx_calibration_measurement(
    app: tauri::AppHandle,
    measurement: CmyxCalibrationMeasurementInput,
    cache: tauri::State<'_, ProjectCache>,
) -> Result<CmyxCalibrationLibraryView, String> {
    let record = measurement.into_record()?;
    commit_calibration_library_change(&cache, || upsert_calibration_for_app(&app, record))
}

#[tauri::command]
fn delete_cmyx_calibration_record(
    app: tauri::AppHandle,
    record_id: String,
    cache: tauri::State<'_, ProjectCache>,
) -> Result<CmyxCalibrationLibraryView, String> {
    commit_calibration_library_change(&cache, || delete_calibration_for_app(&app, &record_id))
}

#[tauri::command]
fn set_cmyx_calibration_geometry_context(
    app: tauri::AppHandle,
    geometry_context: CmyxGeometryContext,
    cache: tauri::State<'_, ProjectCache>,
) -> Result<CmyxCalibrationLibraryView, String> {
    commit_calibration_library_change(&cache, || {
        set_calibration_geometry_for_app(&app, geometry_context)
    })
}

const CALIBRATION_FIXED_CMY_SPOOL_IDS: [&str; 3] = [
    "panchroma-translucent-cyan",
    "panchroma-translucent-magenta",
    "panchroma-translucent-yellow",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CmyxCalibrationProjectRequest {
    project_id: String,
    /// Exact physical spool identities in T1, T2, T3, T4 order.
    spool_ids: [String; 4],
    destination_path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeCmyxCalibrationProjectResult {
    project_id: String,
    path: String,
    file_name: String,
    byte_size: u64,
    artifact_sha256: String,
    manifest_path: String,
    manifest_sha256: String,
    swatch_count: usize,
    production_qualified: bool,
    validation: CmyxCalibrationProjectValidationReport,
    warnings: Vec<String>,
    next_steps: Vec<String>,
}

fn validate_calibration_project_id(project_id: &str) -> Result<String, String> {
    let project_id = project_id.trim();
    if project_id.is_empty()
        || project_id.len() > 64
        || !project_id.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphanumeric() || (index > 0 && matches!(byte, b'-' | b'_'))
        })
    {
        return Err(
            "Calibration project ID must contain 1-64 ASCII letters or digits with optional internal '-' or '_'."
                .to_owned(),
        );
    }
    Ok(project_id.to_owned())
}

fn new_calibration_destination(destination_path: &str) -> Result<PathBuf, String> {
    let requested = PathBuf::from(destination_path.trim());
    if !requested.is_absolute() {
        return Err("Choose an absolute destination for the calibration 3MF.".to_owned());
    }
    if requested
        .extension()
        .and_then(|extension| extension.to_str())
        != Some("3mf")
    {
        return Err("Calibration project destination must use the .3mf extension.".to_owned());
    }
    let file_name = requested
        .file_name()
        .ok_or_else(|| "Calibration project destination has no file name.".to_owned())?;
    let parent = requested
        .parent()
        .ok_or_else(|| "Calibration project destination has no parent directory.".to_owned())?
        .canonicalize()
        .map_err(|error| {
            format!("Failed to resolve the calibration destination directory: {error}")
        })?;
    if !parent.is_dir() {
        return Err("Calibration project destination parent is not a directory.".to_owned());
    }
    let destination = parent.join(file_name);
    match fs::symlink_metadata(&destination) {
        Ok(_) => {
            return Err(format!(
                "Calibration project destination already exists and will not be overwritten: {}",
                destination.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "Failed to inspect calibration project destination {}: {error}",
                destination.display()
            ));
        }
    }
    Ok(destination)
}

fn build_calibration_project_spec(
    project_id: String,
    spool_ids: [String; 4],
    inventory: &[u1_planner::Spool],
) -> Result<u1_application::CmyxCalibrationProjectSpec, String> {
    for (index, expected) in CALIBRATION_FIXED_CMY_SPOOL_IDS.iter().enumerate() {
        if spool_ids[index] != *expected {
            return Err(format!(
                "Calibration T{} must use the fixed {} spool {expected:?}.",
                index + 1,
                ["Cyan", "Magenta", "Yellow"][index]
            ));
        }
    }
    let loadout = resolve_u1_full_spectrum_physical_loadout(&spool_ids, inventory, false)
        .map_err(|error| error.to_string())?;
    let spec = recommended_cmyx_calibration_project_spec(project_id, loadout);
    if spec.swatches.len() != 18 {
        return Err(
            "The recommended calibration chart contract no longer contains exactly 18 swatches."
                .to_owned(),
        );
    }
    Ok(spec)
}

#[tauri::command]
async fn build_cmyx_calibration_project(
    app: tauri::AppHandle,
    request: CmyxCalibrationProjectRequest,
) -> Result<NativeCmyxCalibrationProjectResult, String> {
    let project_id = validate_calibration_project_id(&request.project_id)?;
    let destination = new_calibration_destination(&request.destination_path)?;
    let inventory = load_for_app(&app)?.planning_spools()?;
    let spec = build_calibration_project_spec(project_id, request.spool_ids, &inventory)?;
    let application_path = discover_installation().ok_or_else(|| {
        "Snapmaker Orca was not found in a supported macOS application location.".to_owned()
    })?;
    tauri::async_runtime::spawn_blocking(move || {
        let written = write_cmyx_calibration_project_candidate(
            &application_path,
            &spec,
            &destination,
        )
        .map_err(|error| error.to_string())?;
        let validation = validate_cmyx_calibration_artifact(&destination)
            .map_err(|error| error.to_string())?;
        if !validation.valid
            || validation.project_id.as_deref() != Some(written.project_id.as_str())
            || validation.manifest_sha256.as_deref()
                != Some(written.embedded_manifest_sha256.as_str())
            || validation.swatch_count != written.swatch_count
        {
            return Err(format!(
                "Generated calibration candidate failed final validation: {}",
                validation.issues.join("; ")
            ));
        }
        if written.production_qualified || written.swatch_count != 18 {
            return Err(
                "Generated calibration candidate crossed its qualification boundary unexpectedly."
                    .to_owned(),
            );
        }
        Ok(NativeCmyxCalibrationProjectResult {
            project_id: written.project_id,
            path: destination.to_string_lossy().into_owned(),
            file_name: destination
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("calibration.3mf")
                .to_owned(),
            byte_size: written.candidate.byte_size,
            artifact_sha256: written.candidate.sha256,
            manifest_path: written.embedded_manifest_path,
            manifest_sha256: written.embedded_manifest_sha256,
            swatch_count: written.swatch_count,
            production_qualified: written.production_qualified,
            validation,
            warnings: written.warnings,
            next_steps: vec![
                "Open this candidate in the exact supported Snapmaker Orca application, then slice and save it without changing the T1-T4 loadout.".to_owned(),
                "Close the project, reopen the saved project, and reslice it to complete the required GUI round-trip check.".to_owned(),
                "Print the numbered S01-S18 chart with the confirmed physical loadout; this candidate is not a production-qualified model.".to_owned(),
                "Measure each printed swatch as six-digit sRGB HEX and save the result with the matching manifest recipe and Flat calibration swatch geometry.".to_owned(),
            ],
        })
    })
    .await
    .map_err(|error| format!("Calibration project worker failed: {error}"))?
}

#[tauri::command]
async fn validate_cmyx_calibration_project(
    path: String,
) -> Result<CmyxCalibrationProjectValidationReport, String> {
    let path = PathBuf::from(path.trim())
        .canonicalize()
        .map_err(|error| format!("Failed to resolve the calibration project: {error}"))?;
    if !path.is_file() || path.extension().and_then(|extension| extension.to_str()) != Some("3mf") {
        return Err("Calibration project must be an existing regular .3mf file.".to_owned());
    }
    tauri::async_runtime::spawn_blocking(move || validate_cmyx_calibration_artifact(&path))
        .await
        .map_err(|error| format!("Calibration validation worker failed: {error}"))?
        .map_err(|error| error.to_string())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversionCapabilityView {
    available: bool,
    reason: String,
    plan_fingerprint: Option<String>,
    source_dialect_support: &'static str,
    experimental_dialect_approval_required: bool,
    experimental_dialect_fingerprint: Option<String>,
    adapters: Vec<AdapterCapabilityView>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExperimentalDialectApproval {
    source_fingerprint: String,
}

fn dialect_support_name(support: DialectSupport) -> &'static str {
    match support {
        DialectSupport::Supported => "Supported",
        DialectSupport::Experimental => "Experimental",
        DialectSupport::Limited => "Limited",
        DialectSupport::Unsupported => "Unsupported",
    }
}

fn experimental_dialect_fingerprint(analysis: &ProjectAnalysis) -> String {
    source_dialect_approval_fingerprint(analysis)
}

fn validate_source_dialect_for_adapter(
    analysis: &ProjectAnalysis,
    approval: Option<&ExperimentalDialectApproval>,
) -> Result<(), String> {
    validate_source_dialect_for_conversion(
        analysis,
        approval.map(|approval| approval.source_fingerprint.as_str()),
    )
    .map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdapterCapabilityView {
    target: String,
    adapter_id: String,
    slicer: String,
    required: bool,
    available: bool,
    reason: String,
    report: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativePreparedSlot {
    toolhead: String,
    spool_id: Option<String>,
    spool_name: Option<String>,
    material: Option<String>,
    color: Option<String>,
    profile: String,
    setting_id: String,
    filament_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct NativePreparedArtifact {
    adapter_id: String,
    target: String,
    printer: String,
    strategy: String,
    slicer: String,
    batch_id: String,
    file_name: String,
    target_plate_ids: Vec<String>,
    source_unit_ids: Vec<String>,
    loadout: Vec<NativePreparedSlot>,
    setup_actions: Vec<String>,
    adapter_evidence: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeConversionPreparation {
    adapter_id: String,
    source_sha256: String,
    plan_fingerprint: String,
    bundle_directory_name: String,
    artifacts: Vec<NativePreparedArtifact>,
    excluded_source_units: Vec<ExcludedSourceUnit>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NativePublishedArtifact {
    adapter_id: String,
    target: String,
    printer: String,
    strategy: String,
    slicer: String,
    batch_id: String,
    file_name: String,
    relative_path: String,
    path: PathBuf,
    byte_size: u64,
    sha256: String,
    plate_count: usize,
    target_plate_ids: Vec<String>,
    source_unit_ids: Vec<String>,
    loadout: Vec<NativePreparedSlot>,
    setup_actions: Vec<String>,
    validation_status: String,
    adapter_evidence: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeConversionResult {
    adapter_id: String,
    output_directory: PathBuf,
    manifest_path: PathBuf,
    report_path: PathBuf,
    artifacts: Vec<NativePublishedArtifact>,
    excluded_source_units: Vec<ExcludedSourceUnit>,
    warnings_acknowledged: bool,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PublishedBundleBuildIdentity {
    package_version: String,
    git_identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PublishedBundleConverterManifest {
    name: String,
    build_identity: PublishedBundleBuildIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PublishedBundleSourceManifest {
    file_name: String,
    byte_size: u64,
    sha256: String,
    application: SourceApplication,
    application_name: Option<String>,
    application_version: Option<String>,
    dialect: ProjectDialect,
    dialect_version: Option<String>,
    dialect_support: DialectSupport,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PublishedBundleArtifactManifest {
    adapter_id: String,
    target: String,
    printer: String,
    strategy: String,
    slicer: String,
    batch_id: String,
    file_name: String,
    relative_path: String,
    byte_size: u64,
    sha256: String,
    plate_count: usize,
    target_plate_ids: Vec<String>,
    source_unit_ids: Vec<String>,
    loadout: Vec<NativePreparedSlot>,
    setup_actions: Vec<String>,
    validation_status: String,
    adapter_evidence: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PublishedBundleManifest {
    schema_version: u32,
    adapter_id: String,
    converter: PublishedBundleConverterManifest,
    source: PublishedBundleSourceManifest,
    plan_fingerprint: String,
    target_adapters: serde_json::Value,
    inventory_snapshot: serde_json::Value,
    artifacts: Vec<PublishedBundleArtifactManifest>,
    source_to_target_map: serde_json::Value,
    excluded_source_units: Vec<ExcludedSourceUnit>,
    excluded_scopes: Vec<String>,
    user_approvals: serde_json::Value,
    partial_conversion_approval: Option<PartialConversionApproval>,
    warnings_acknowledged: bool,
    acknowledged_warnings: Vec<String>,
    loadout_timeline: serde_json::Value,
    validation_status: String,
    warnings: Vec<String>,
}

#[derive(Debug)]
struct PublishedBundleMetadataIdentity {
    path: PathBuf,
    byte_size: u64,
    sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TrustedDirectoryIdentity {
    device_id: Option<u64>,
    file_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PublicationReceiptFile {
    relative_path: String,
    byte_size: u64,
    sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PublicationReceiptArtifact {
    adapter_id: String,
    target: String,
    batch_id: String,
    file_name: String,
    relative_path: String,
    byte_size: u64,
    sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PublicationReceipt {
    schema_version: u32,
    converter: PublishedBundleConverterManifest,
    source_path: PathBuf,
    source_byte_size: u64,
    source_sha256: String,
    plan_fingerprint: String,
    canonical_preflight_sha256: String,
    output_root: PathBuf,
    output_root_identity: TrustedDirectoryIdentity,
    metadata: Vec<PublicationReceiptFile>,
    artifacts: Vec<PublicationReceiptArtifact>,
}

fn converter_git_identity() -> &'static str {
    option_env!("U1_PLANNER_GIT_IDENTITY")
        .map(str::trim)
        .filter(|identity| !identity.is_empty())
        .unwrap_or("Unknown")
}

fn published_converter_manifest() -> PublishedBundleConverterManifest {
    PublishedBundleConverterManifest {
        name: "U1 3MF Color Planner".to_owned(),
        build_identity: PublishedBundleBuildIdentity {
            package_version: env!("CARGO_PKG_VERSION").to_owned(),
            git_identity: converter_git_identity().to_owned(),
        },
    }
}

fn published_source_manifest(
    source_path: &Path,
    analysis: &ProjectAnalysis,
) -> Result<PublishedBundleSourceManifest, String> {
    let file_name = source_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            "The authoritative source file name is missing or is not valid UTF-8.".to_owned()
        })?;
    exact_leaf_name(file_name)?;
    Ok(PublishedBundleSourceManifest {
        file_name: file_name.to_owned(),
        byte_size: analysis.input.byte_size,
        sha256: analysis.input.sha256.clone(),
        application: analysis.source.application,
        application_name: analysis.source.application_name.clone(),
        application_version: analysis.source.application_version.clone(),
        dialect: analysis.source.dialect,
        dialect_version: analysis.source.dialect_version.clone(),
        dialect_support: analysis.source.support,
    })
}

fn ensure_supported_published_bundle_schema(manifest: &serde_json::Value) -> Result<(), String> {
    let schema_version = manifest
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            "conversion_revalidation_invalid_bundle: The published manifest has no numeric schemaVersion."
                .to_owned()
        })?;
    if schema_version == u64::from(PUBLISHED_BUNDLE_SCHEMA_VERSION) {
        return Ok(());
    }
    if schema_version == 1 {
        return Err(
            "conversion_revalidation_legacy_bundle: Published bundle schema 1 predates the required source and converter provenance. It cannot be recovered safely; convert the current plan again."
                .to_owned(),
        );
    }
    Err(format!(
        "conversion_revalidation_unsupported_bundle: Published bundle schema {schema_version} is not supported by this converter build."
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RegisteredSlicer {
    SnapmakerOrca,
    BambuStudio,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RegisteredPublishedOutput {
    canonical_path: PathBuf,
    adapter_id: String,
    slicer: RegisteredSlicer,
    byte_size: u64,
    sha256: String,
}

fn registered_slicer_for_target(target: &str) -> Result<RegisteredSlicer, String> {
    match target {
        "u1_direct" | "u1_cmyx_solid" | "u1_full_spectrum" => Ok(RegisteredSlicer::SnapmakerOrca),
        "a1_mini_mono" => Ok(RegisteredSlicer::BambuStudio),
        _ => Err(format!(
            "output_action_invalid_target: Native output target '{target}' has no registered slicer action."
        )),
    }
}

fn staged_published_output_registry(
    artifacts: &[NativePublishedArtifact],
    final_directory: &Path,
) -> Result<HashMap<PathBuf, RegisteredPublishedOutput>, String> {
    let mut next = HashMap::with_capacity(artifacts.len());
    for artifact in artifacts {
        if !artifact
            .file_name
            .rsplit_once('.')
            .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("3mf"))
        {
            return Err(format!(
                "output_action_invalid_artifact: Staged output {} is not a 3MF project.",
                artifact.file_name
            ));
        }
        let metadata = fs::symlink_metadata(&artifact.path).map_err(|error| {
            format!(
                "output_action_invalid_artifact: Staged output {} cannot be inspected: {error}",
                artifact.path.display()
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(format!(
                "output_action_invalid_artifact: Staged output {} is not a regular file.",
                artifact.path.display()
            ));
        }
        if metadata.len() != artifact.byte_size {
            return Err(format!(
                "output_action_invalid_artifact: Staged output {} changed before publication.",
                artifact.path.display()
            ));
        }
        validate_regular_file_identity(
            &artifact.path,
            artifact.byte_size,
            &artifact.sha256,
            "staged published output",
        )
        .map_err(|_| {
            format!(
                "output_action_invalid_artifact: Staged output {} changed before publication.",
                artifact.path.display()
            )
        })?;
        let relative = Path::new(&artifact.relative_path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(format!(
                "output_action_invalid_artifact: Staged output path {:?} is not a safe relative path.",
                artifact.relative_path
            ));
        }
        let canonical_path = final_directory.join(&artifact.relative_path);
        let record = RegisteredPublishedOutput {
            canonical_path: canonical_path.clone(),
            adapter_id: artifact.adapter_id.clone(),
            slicer: registered_slicer_for_target(&artifact.target)?,
            byte_size: artifact.byte_size,
            sha256: artifact.sha256.clone(),
        };
        if next.insert(canonical_path, record).is_some() {
            return Err(
                "output_action_invalid_artifact: Published output paths are not unique.".into(),
            );
        }
    }
    Ok(next)
}

fn revalidated_published_output_registry(
    artifacts: &[NativePublishedArtifact],
    output_directory: &Path,
) -> Result<HashMap<PathBuf, RegisteredPublishedOutput>, String> {
    let mut next = HashMap::with_capacity(artifacts.len());
    for artifact in artifacts {
        if !artifact
            .file_name
            .rsplit_once('.')
            .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("3mf"))
        {
            return Err(format!(
                "output_action_invalid_artifact: Published output {} is not a 3MF project.",
                artifact.file_name
            ));
        }
        let relative = Path::new(&artifact.relative_path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(format!(
                "output_action_invalid_artifact: Published output path {:?} is not a safe relative path.",
                artifact.relative_path
            ));
        }
        let canonical_path = output_directory.join(relative);
        if artifact.path != canonical_path {
            return Err(format!(
                "output_action_invalid_artifact: Published output {} is not bound to its verified bundle path.",
                artifact.file_name
            ));
        }
        let record = RegisteredPublishedOutput {
            canonical_path: canonical_path.clone(),
            adapter_id: artifact.adapter_id.clone(),
            slicer: registered_slicer_for_target(&artifact.target)?,
            byte_size: artifact.byte_size,
            sha256: artifact.sha256.clone(),
        };
        if next.insert(canonical_path, record).is_some() {
            return Err(
                "output_action_invalid_artifact: Published output paths are not unique.".into(),
            );
        }
    }
    Ok(next)
}

fn target_adapters_manifest(prepared: &NativeConversionPreparation) -> Vec<serde_json::Value> {
    prepared
        .artifacts
        .iter()
        .map(|artifact| {
            serde_json::json!({
                "adapterId": artifact.adapter_id,
                "target": artifact.target,
                "printer": artifact.printer,
                "strategy": artifact.strategy,
                "slicer": artifact.slicer,
            })
        })
        .collect()
}

fn user_approvals_manifest(input: &PlanningInput) -> Vec<serde_json::Value> {
    input
        .scopes
        .iter()
        .map(|scope| {
            serde_json::json!({
                "scopeId": scope.id,
                "strategy": scope.strategy,
                "directAssignments": scope.direct_assignments,
                "approvedCmyxFallbacks": scope.approved_cmyx_fallbacks,
                "approvedMaterialSubstitutions": scope.approved_material_substitutions,
                "unitPrinterPreferences": scope.units.iter().map(|unit| serde_json::json!({
                    "sourceUnitId": unit.source_unit_id,
                    "preference": unit.printer_preference,
                })).collect::<Vec<_>>(),
            })
        })
        .collect()
}

fn excluded_scope_ids(excluded_source_units: &[ExcludedSourceUnit]) -> Vec<String> {
    excluded_source_units
        .iter()
        .map(|unit| unit.scope_id.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(str::to_owned)
        .collect()
}

fn plain_u1_artifact_presentation(
    result: &PlanningResult,
    batch_id: &str,
) -> Result<(&'static str, &'static str), String> {
    let strategy = result
        .batches
        .iter()
        .find(|batch| batch.id == batch_id)
        .map(|batch| batch.strategy)
        .ok_or_else(|| {
            format!("Prepared physical U1 artifact references unknown batch '{batch_id}'.")
        })?;
    match strategy {
        ColorStrategy::DirectSpools => Ok(("u1_direct", "Direct Spools")),
        ColorStrategy::CmyxSolid => Ok(("u1_cmyx_solid", "CMY+X Solid")),
        other => Err(format!(
            "Prepared physical U1 artifact batch '{batch_id}' has unsupported strategy {other:?}."
        )),
    }
}

#[cfg(test)]
fn register_published_outputs(
    registry: &Mutex<HashMap<PathBuf, RegisteredPublishedOutput>>,
    result: &NativeConversionResult,
) -> Result<(), String> {
    let mut next = HashMap::with_capacity(result.artifacts.len());
    for artifact in &result.artifacts {
        if !artifact
            .path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("3mf"))
        {
            return Err(format!(
                "output_action_invalid_artifact: Published output {} is not a 3MF project.",
                artifact.path.display()
            ));
        }
        let metadata = fs::symlink_metadata(&artifact.path).map_err(|error| {
            format!(
                "output_action_invalid_artifact: Published output {} cannot be inspected: {error}",
                artifact.path.display()
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(format!(
                "output_action_invalid_artifact: Published output {} is not a regular file.",
                artifact.path.display()
            ));
        }
        if metadata.len() != artifact.byte_size {
            return Err(format!(
                "output_action_invalid_artifact: Published output {} changed before quick actions were registered.",
                artifact.path.display()
            ));
        }
        let canonical_path = artifact.path.canonicalize().map_err(|error| {
            format!(
                "output_action_invalid_artifact: Published output {} cannot be resolved: {error}",
                artifact.path.display()
            )
        })?;
        let record = RegisteredPublishedOutput {
            canonical_path: canonical_path.clone(),
            adapter_id: artifact.adapter_id.clone(),
            slicer: registered_slicer_for_target(&artifact.target)?,
            byte_size: artifact.byte_size,
            sha256: artifact.sha256.clone(),
        };
        if next.insert(canonical_path, record).is_some() {
            return Err(
                "output_action_invalid_artifact: Published output paths are not unique.".into(),
            );
        }
    }
    *registry
        .lock()
        .map_err(|_| "published output action registry is unavailable".to_owned())? = next;
    Ok(())
}

#[cfg(test)]
fn register_published_outputs_if_current(
    cache: &ProjectCache,
    expected_generation: u64,
    result: &NativeConversionResult,
) -> Result<bool, String> {
    let generation = cache
        .operation_generation
        .lock()
        .map_err(|_| "project operation generation is unavailable".to_owned())?;
    if *generation != expected_generation {
        return Ok(false);
    }
    register_published_outputs(&cache.published_outputs, result)?;
    Ok(true)
}

fn resolve_registered_output(
    registry: &Mutex<HashMap<PathBuf, RegisteredPublishedOutput>>,
    path: &str,
    adapter_id: &str,
) -> Result<RegisteredPublishedOutput, String> {
    let requested = Path::new(path);
    if !requested.is_absolute() {
        return Err("output_action_not_registered: Select a published output from the current conversion result.".into());
    }
    let metadata = fs::symlink_metadata(requested).map_err(|_| {
        "output_action_not_registered: The published output no longer exists.".to_owned()
    })?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(
            "output_action_not_registered: The published output is no longer a regular file."
                .into(),
        );
    }
    let canonical_path = requested.canonicalize().map_err(|_| {
        "output_action_not_registered: The published output can no longer be resolved.".to_owned()
    })?;
    let registered = registry
        .lock()
        .map_err(|_| "published output action registry is unavailable".to_owned())?
        .get(&canonical_path)
        .cloned()
        .ok_or_else(|| {
            "output_action_not_registered: Select a published output from the current conversion result."
                .to_owned()
        })?;
    if registered.adapter_id != adapter_id {
        return Err(
            "output_action_not_registered: The output adapter does not match the published conversion result."
                .into(),
        );
    }
    Ok(registered)
}

fn validate_registered_output_identity(output: &RegisteredPublishedOutput) -> Result<(), String> {
    validate_regular_file_identity(
        &output.canonical_path,
        output.byte_size,
        &output.sha256,
        "registered published output",
    )
    .map_err(|_| {
        "output_action_file_changed: The converted project changed after publication. Validate or convert it again before opening it from this result."
            .to_owned()
    })
}

fn discover_registered_slicer(slicer: RegisteredSlicer) -> Result<PathBuf, String> {
    match slicer {
        RegisteredSlicer::SnapmakerOrca => discover_installation().ok_or_else(|| {
            "output_action_slicer_missing: Snapmaker Orca is not installed in a supported location."
                .to_owned()
        }),
        RegisteredSlicer::BambuStudio => discover_bambu_studio().ok_or_else(|| {
            "output_action_slicer_missing: Bambu Studio is not installed in a supported location."
                .to_owned()
        }),
    }
}

#[cfg(target_os = "macos")]
fn launch_registered_output(output: &RegisteredPublishedOutput) -> Result<(), String> {
    validate_registered_output_identity(output)?;
    let slicer = discover_registered_slicer(output.slicer)?;
    let status = Command::new("/usr/bin/open")
        .arg("-a")
        .arg(&slicer)
        .arg(&output.canonical_path)
        .status()
        .map_err(|error| {
            format!(
                "output_action_failed: Failed to open the project in {}: {error}",
                slicer.display()
            )
        })?;
    if !status.success() {
        return Err(format!(
            "output_action_failed: macOS could not open the project in {} (status {status}).",
            slicer.display()
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn launch_registered_output(_output: &RegisteredPublishedOutput) -> Result<(), String> {
    Err(
        "output_action_unsupported: Opening a converted project is currently supported only on macOS."
            .into(),
    )
}

#[cfg(target_os = "macos")]
fn reveal_registered_output(output: &RegisteredPublishedOutput) -> Result<(), String> {
    validate_registered_output_identity(output)?;
    let status = Command::new("/usr/bin/open")
        .arg("-R")
        .arg(&output.canonical_path)
        .status()
        .map_err(|error| {
            format!("output_action_failed: Failed to show the project in Finder: {error}")
        })?;
    if !status.success() {
        return Err(format!(
            "output_action_failed: Finder could not reveal the project (status {status})."
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn reveal_registered_output(_output: &RegisteredPublishedOutput) -> Result<(), String> {
    Err(
        "output_action_unsupported: Revealing a converted project is currently supported only on macOS."
            .into(),
    )
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreparedConversionResponse {
    preparation_token: String,
    conversion_id: String,
    preparation: NativeConversionPreparation,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ConversionProgressEvent {
    conversion_id: String,
    stage: &'static str,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CancelConversionResponse {
    conversion_id: String,
    accepted: bool,
    state: Option<U1DirectConversionState>,
}

fn target_slices(
    input: &PlanningInput,
    result: &PlanningResult,
    partial_conversion_approval: Option<&PartialConversionApproval>,
) -> Result<PreparedTargetSlices, String> {
    let mut direct = None;
    let mut full_spectrum = None;
    let mut a1_mini = None;
    let partition = slice_all_conversion_targets_with_partial_approval(
        input,
        result,
        partial_conversion_approval,
    )
    .map_err(|error| error.to_string())?;
    for slice in partition.slices {
        match slice.target {
            ConversionTarget::U1Direct => direct = Some(slice),
            ConversionTarget::U1FullSpectrum => full_spectrum = Some(slice),
            ConversionTarget::A1MiniMono => a1_mini = Some(slice),
        }
    }
    Ok(PreparedTargetSlices {
        direct,
        full_spectrum,
        a1_mini,
        excluded_source_units: partition.excluded_source_units,
    })
}

fn safe_bundle_component(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("project");
    let mut safe = String::new();
    for character in stem.chars().take(72) {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
            safe.push(character);
        } else if character.is_whitespace() && !safe.ends_with('_') {
            safe.push('_');
        }
    }
    let safe = safe.trim_matches(['_', '-']);
    let identity = format!("{:x}", Sha256::digest(stem.as_bytes()));
    format!(
        "{}-{}__converted",
        if safe.is_empty() { "project" } else { safe },
        &identity[..10]
    )
}

fn json_report(value: &impl Serialize) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|error| format!("Failed to serialize capability: {error}"))
}

fn capability_entry(
    target: &str,
    adapter_id: &str,
    slicer: &str,
    available: bool,
    reason: String,
    report: serde_json::Value,
) -> AdapterCapabilityView {
    AdapterCapabilityView {
        target: target.into(),
        adapter_id: adapter_id.into(),
        slicer: slicer.into(),
        required: true,
        available,
        reason,
        report,
    }
}

fn unavailable_capability(
    target: &str,
    adapter_id: &str,
    slicer: &str,
    reason: impl Into<String>,
) -> AdapterCapabilityView {
    capability_entry(
        target,
        adapter_id,
        slicer,
        false,
        reason.into(),
        serde_json::Value::Null,
    )
}

fn native_preparation(
    source_path: &Path,
    source_sha256: &str,
    plan_fingerprint: &str,
    input: &PlanningInput,
    result: &PlanningResult,
    excluded_source_units: &[ExcludedSourceUnit],
    adapters: NativeAdapterPreparations<'_>,
) -> Result<NativeConversionPreparation, String> {
    let inventory = input
        .inventory
        .iter()
        .map(|spool| (spool.id.as_str(), spool))
        .collect::<BTreeMap<_, _>>();
    let describe_batch_actions = |batch: &u1_planner::PlannedBatch| {
        batch
            .setup_actions
            .iter()
            .map(|action| view::setup_action_description(action, &inventory))
            .collect::<Vec<_>>()
    };
    let mut artifacts = Vec::new();
    let mut warnings = Vec::new();
    if let Some(preparation) = adapters.direct {
        warnings.extend(preparation.warnings.clone());
        artifacts.extend(
            preparation
                .artifacts
                .iter()
                .map(|artifact| {
                    let (target, strategy) =
                        plain_u1_artifact_presentation(result, &artifact.batch_id)?;
                    Ok(NativePreparedArtifact {
                        adapter_id: preparation.adapter_id.clone(),
                        target: target.into(),
                        printer: "Snapmaker U1".into(),
                        strategy: strategy.into(),
                        slicer: "Snapmaker Orca".into(),
                        batch_id: artifact.batch_id.clone(),
                        file_name: artifact.file_name.clone(),
                        target_plate_ids: artifact.target_plate_ids.clone(),
                        source_unit_ids: artifact.source_unit_ids.clone(),
                        loadout: artifact
                            .loadout
                            .iter()
                            .map(|slot| NativePreparedSlot {
                                toolhead: slot.toolhead.clone(),
                                spool_id: slot.spool_id.clone(),
                                spool_name: slot.spool_name.clone(),
                                material: slot.material.clone(),
                                color: slot.color.clone(),
                                profile: slot.profile.clone(),
                                setting_id: slot.setting_id.clone(),
                                filament_id: slot.filament_id.clone(),
                            })
                            .collect(),
                        setup_actions: artifact.setup_actions.clone(),
                        adapter_evidence: json_report(artifact)?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
        );
    }
    if let Some(preparation) = adapters.full_spectrum {
        warnings.extend(preparation.warnings.clone());
        artifacts.extend(
            preparation
                .artifacts
                .iter()
                .map(|artifact| {
                    Ok(NativePreparedArtifact {
                        adapter_id: preparation.adapter_id.clone(),
                        target: "u1_full_spectrum".into(),
                        printer: "Snapmaker U1".into(),
                        strategy: "CMY+X Full Spectrum".into(),
                        slicer: "Snapmaker Orca".into(),
                        batch_id: artifact.batch_id.clone(),
                        file_name: artifact.file_name.clone(),
                        target_plate_ids: artifact
                            .plates
                            .iter()
                            .map(|plate| plate.plan_plate_id.clone())
                            .collect(),
                        source_unit_ids: artifact
                            .plates
                            .iter()
                            .flat_map(|plate| {
                                plate.units.iter().map(|unit| unit.source_unit_id.clone())
                            })
                            .collect(),
                        loadout: artifact
                            .loadout
                            .iter()
                            .map(|slot| NativePreparedSlot {
                                toolhead: format!("T{}", slot.toolhead.index() + 1),
                                spool_id: Some(slot.spool_id.clone()),
                                spool_name: Some(slot.spool_name.clone()),
                                material: Some(format!("{:?}", slot.material).to_uppercase()),
                                color: Some(format!(
                                    "#{:02X}{:02X}{:02X}",
                                    slot.color.red, slot.color.green, slot.color.blue
                                )),
                                profile: slot.profile.clone(),
                                setting_id: slot.setting_id.clone(),
                                filament_id: slot.filament_id.clone(),
                            })
                            .collect(),
                        setup_actions: result
                            .batches
                            .iter()
                            .find(|batch| batch.id == artifact.batch_id)
                            .map(&describe_batch_actions)
                            .unwrap_or_default(),
                        adapter_evidence: json_report(artifact)?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
        );
    }
    if let Some(preparation) = adapters.a1_mini {
        warnings.extend(preparation.warnings.clone());
        artifacts.extend(
            preparation
                .artifacts
                .iter()
                .map(|artifact| {
                    Ok(NativePreparedArtifact {
                        adapter_id: preparation.adapter_id.clone(),
                        target: "a1_mini_mono".into(),
                        printer: "Bambu Lab A1 mini".into(),
                        strategy: "A1 Mono".into(),
                        slicer: "Bambu Studio".into(),
                        batch_id: artifact.job_id.clone(),
                        file_name: artifact.file_name.clone(),
                        target_plate_ids: vec![artifact.plate_id.clone()],
                        source_unit_ids: artifact.source_unit_ids.clone(),
                        loadout: vec![NativePreparedSlot {
                            toolhead: "External spool".into(),
                            spool_id: Some(artifact.spool.spool_id.clone()),
                            spool_name: Some(artifact.spool.spool_name.clone()),
                            material: Some(format!("{:?}", artifact.spool.material).to_uppercase()),
                            color: Some(artifact.spool.color.clone()),
                            profile: artifact.spool.profile.clone(),
                            setting_id: artifact.spool.setting_id.clone(),
                            filament_id: artifact.spool.filament_id.clone(),
                        }],
                        setup_actions: result
                            .batches
                            .iter()
                            .find(|batch| batch.job_ids.iter().any(|id| id == &artifact.job_id))
                            .map(&describe_batch_actions)
                            .unwrap_or_default(),
                        adapter_evidence: json_report(artifact)?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
        );
    }
    let canonical_batch_index = |artifact: &NativePreparedArtifact| {
        result
            .batches
            .iter()
            .position(|batch| {
                if artifact.target == "a1_mini_mono" {
                    batch
                        .job_ids
                        .iter()
                        .any(|job_id| job_id == &artifact.batch_id)
                } else {
                    batch.id == artifact.batch_id
                }
            })
            .unwrap_or(usize::MAX)
    };
    // Keep the operator-visible order identical to the planner's physical
    // setup sequence (CMY+X, then Direct, then A1). Lexical adapter ordering
    // would make otherwise-correct setup actions unsafe to follow.
    if artifacts
        .iter()
        .any(|artifact| canonical_batch_index(artifact) == usize::MAX)
    {
        return Err("A prepared native artifact is not bound to a canonical batch.".into());
    }
    artifacts.sort_by(|left, right| {
        (canonical_batch_index(left), &left.file_name)
            .cmp(&(canonical_batch_index(right), &right.file_name))
    });
    warnings.sort();
    warnings.dedup();
    Ok(NativeConversionPreparation {
        adapter_id: "u1-planner/mixed-native".into(),
        source_sha256: source_sha256.into(),
        plan_fingerprint: plan_fingerprint.into(),
        bundle_directory_name: safe_bundle_component(source_path),
        artifacts,
        excluded_source_units: excluded_source_units.to_vec(),
        warnings,
    })
}

#[allow(clippy::too_many_arguments)]
fn prepare_required_targets(
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    plan_fingerprint: &str,
    partial_conversion_approval: Option<&PartialConversionApproval>,
    experimental_dialect_approval: Option<&ExperimentalDialectApproval>,
    control: Option<&MixedConversionControl>,
) -> Result<(NativeConversionPreparation, PreparedTargetSlices), String> {
    let (preparation, slices, _adapters) = prepare_required_targets_with_adapters(
        source_path,
        analysis,
        input,
        result,
        plan_fingerprint,
        partial_conversion_approval,
        experimental_dialect_approval,
        control,
    )?;
    Ok((preparation, slices))
}

#[allow(clippy::too_many_arguments)]
fn prepare_required_targets_with_adapters(
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    plan_fingerprint: &str,
    partial_conversion_approval: Option<&PartialConversionApproval>,
    experimental_dialect_approval: Option<&ExperimentalDialectApproval>,
    control: Option<&MixedConversionControl>,
) -> Result<
    (
        NativeConversionPreparation,
        PreparedTargetSlices,
        OwnedNativeAdapterPreparations,
    ),
    String,
> {
    let mut is_cancelled =
        || control.is_some_and(|control| control.state() == U1DirectConversionState::Cancelled);
    ensure_preflight_active(&mut is_cancelled)?;
    validate_cached_identity_cancellable(source_path, analysis, &mut is_cancelled)?;
    validate_source_dialect_for_adapter(analysis, experimental_dialect_approval)?;
    ensure_preflight_active(&mut is_cancelled)?;
    let slices = target_slices(input, result, partial_conversion_approval)?;
    let direct = if let Some(slice) = &slices.direct {
        ensure_preflight_active(&mut is_cancelled)?;
        let app = discover_installation()
            .ok_or_else(|| "Snapmaker Orca is required for U1 Direct output.".to_owned())?;
        let preparation = Some(
            match control {
                Some(control) => prepare_u1_direct_conversion_cancellable(
                    &app,
                    source_path,
                    analysis,
                    &slice.input,
                    &slice.result,
                    &control.outer,
                ),
                None => prepare_u1_direct_conversion(
                    &app,
                    source_path,
                    analysis,
                    &slice.input,
                    &slice.result,
                ),
            }
            .map_err(|error| error.to_string())?,
        );
        ensure_preflight_active(&mut is_cancelled)?;
        preparation
    } else {
        None
    };
    let full = if let Some(slice) = &slices.full_spectrum {
        ensure_preflight_active(&mut is_cancelled)?;
        let app = discover_installation()
            .ok_or_else(|| "Snapmaker Orca is required for Full Spectrum output.".to_owned())?;
        let capability =
            inspect_u1_full_spectrum_macos_application(&app).map_err(|error| error.to_string())?;
        if !capability.conversion_available {
            return Err(capability.issues.join(" "));
        }
        let preparation = prepare_u1_full_spectrum_conversion(&slice.input, &slice.result)
            .map_err(|error| error.to_string())?;
        let installation = inspect_macos_application(&app).map_err(|error| error.to_string())?;
        let profiles_root = installation.resources_path.join("profiles/Snapmaker");
        let physical_profiles_root =
            qualified_u1_physical_profiles_root(&app).map_err(|error| error.to_string())?;
        for artifact in &preparation.artifacts {
            ensure_preflight_active(&mut is_cancelled)?;
            build_u1_full_spectrum_project_settings_with_physical_profiles(
                &profiles_root,
                &physical_profiles_root,
                artifact,
            )
            .map_err(|error| error.to_string())?;
        }
        ensure_preflight_active(&mut is_cancelled)?;
        Some(preparation)
    } else {
        None
    };
    let a1 = if let Some(slice) = &slices.a1_mini {
        ensure_preflight_active(&mut is_cancelled)?;
        let app = discover_bambu_studio()
            .ok_or_else(|| "Bambu Studio is required for A1 mini output.".to_owned())?;
        let preparation = Some(
            prepare_a1mini_conversion(&app, source_path, analysis, &slice.input, &slice.result)
                .map_err(|error| error.to_string())?,
        );
        ensure_preflight_active(&mut is_cancelled)?;
        preparation
    } else {
        None
    };
    ensure_preflight_active(&mut is_cancelled)?;
    let adapters = OwnedNativeAdapterPreparations {
        direct,
        full_spectrum: full,
        a1_mini: a1,
    };
    let presentation = native_preparation(
        source_path,
        &analysis.input.sha256,
        plan_fingerprint,
        input,
        result,
        &slices.excluded_source_units,
        adapters.as_refs(),
    )?;
    ensure_preflight_active(&mut is_cancelled)?;
    Ok((presentation, slices, adapters))
}

fn ensure_prepared_contract_unchanged(
    approved: &NativeConversionPreparation,
    approved_targets: &PreparedTargetSlices,
    current: &NativeConversionPreparation,
    current_targets: &PreparedTargetSlices,
) -> Result<(), String> {
    if approved != current || approved_targets != current_targets {
        return Err(
            "conversion_preflight_changed: The installed slicer/profile contract or prepared artifacts changed after approval. Open conversion preflight again."
                .into(),
        );
    }
    Ok(())
}

#[tauri::command]
async fn inspect_conversion_capabilities(
    source_path: String,
    cache: tauri::State<'_, ProjectCache>,
) -> Result<ConversionCapabilityView, String> {
    let canonical_path = canonical_project_path(&source_path)?;
    let (analysis, input, result, fingerprint, ready, invalidated) = {
        let cached = cache
            .analyzed
            .lock()
            .map_err(|_| "project cache is unavailable".to_owned())?;
        let cached = cached
            .as_ref()
            .ok_or_else(|| "Analyze the selected 3MF before checking conversion.".to_owned())?;
        if cached.canonical_path != canonical_path {
            return Err("The conversion source does not match the analyzed project.".to_owned());
        }
        (
            cached.analysis.clone(),
            cached.planning_input.clone(),
            cached.planning_result.clone(),
            cached.plan_fingerprint.clone(),
            cached.latest_plan.is_ready(),
            cached.latest_plan.invalidated,
        )
    };
    let dialect_support = dialect_support_name(analysis.source.support);
    let experimental_approval_required = analysis.source.support == DialectSupport::Experimental;
    let experimental_fingerprint =
        experimental_approval_required.then(|| experimental_dialect_fingerprint(&analysis));
    if analysis.source.support == DialectSupport::Unsupported {
        return Ok(ConversionCapabilityView {
            available: false,
            reason: format!(
                "Unsupported source 3MF dialect {:?} cannot be sent to a writer adapter.",
                analysis.source.dialect
            ),
            plan_fingerprint: Some(fingerprint),
            source_dialect_support: dialect_support,
            experimental_dialect_approval_required: false,
            experimental_dialect_fingerprint: None,
            adapters: Vec::new(),
        });
    }
    if invalidated {
        return Ok(ConversionCapabilityView {
            available: false,
            reason: "Recalculate and validate the plan after the filament or calibration library changed."
                .into(),
            plan_fingerprint: Some(fingerprint),
            source_dialect_support: dialect_support,
            experimental_dialect_approval_required: experimental_approval_required,
            experimental_dialect_fingerprint: experimental_fingerprint,
            adapters: Vec::new(),
        });
    }
    if !ready && !result.has_hard_errors() {
        return Ok(ConversionCapabilityView {
            available: false,
            reason: "The plan omits source units without backend-approved exclusion evidence. Recalculate and validate the choices."
                .into(),
            plan_fingerprint: Some(fingerprint),
            source_dialect_support: dialect_support,
            experimental_dialect_approval_required: experimental_approval_required,
            experimental_dialect_fingerprint: experimental_fingerprint,
            adapters: Vec::new(),
        });
    }
    let worker_path = canonical_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        // Capability inspection is read-only. It may inspect the exact valid
        // subset before the user has approved exclusions, but prepare/convert
        // must receive and revalidate the backend-authored approval records.
        let partial_conversion_approval = if result.has_hard_errors() {
            Some(PartialConversionApproval {
                excluded_source_units: canonical_partial_conversion_exclusions(
                    &input, &result,
                )
                .map_err(|error| error.to_string())?,
            })
        } else {
            None
        };
        let slices = target_slices(&input, &result, partial_conversion_approval.as_ref())?;
        let excluded_count = slices.excluded_source_units.len();
        let mut adapters = Vec::new();
        if let Some(slice) = &slices.direct {
            if let Some(app) = discover_installation() {
                match inspect_u1_direct_macos_application(&app) {
                    Ok(report) => {
                        let mut available = report.conversion_available;
                        let mut reason = if available {
                            "U1 Direct conversion is ready.".to_owned()
                        } else {
                            report.issues.join(" ")
                        };
                        if available
                            && !experimental_approval_required
                            && let Err(error) = prepare_u1_direct_conversion(
                                &app,
                                &worker_path,
                                &analysis,
                                &slice.input,
                                &slice.result,
                            )
                        {
                            available = false;
                            reason = error.to_string();
                        }
                        adapters.push(capability_entry(
                            "u1_direct",
                            &report.adapter_id,
                            "Snapmaker Orca",
                            available,
                            reason,
                            json_report(&report)?,
                        ));
                    }
                    Err(error) => adapters.push(unavailable_capability(
                        "u1_direct",
                        "snapmaker-orca/u1-direct",
                        "Snapmaker Orca",
                        error.to_string(),
                    )),
                }
            } else {
                adapters.push(unavailable_capability(
                    "u1_direct",
                    "snapmaker-orca/u1-direct",
                    "Snapmaker Orca",
                    "Snapmaker Orca is not installed in a supported location.",
                ));
            }
        }
        if let Some(slice) = &slices.full_spectrum {
            if let Some(app) = discover_installation() {
                match inspect_u1_full_spectrum_macos_application(&app) {
                    Ok(report) => {
                        let mut available = report.conversion_available;
                        let mut reason = if available {
                            "U1 Full Spectrum conversion is ready.".to_owned()
                        } else {
                            report.issues.join(" ")
                        };
                        if available && !experimental_approval_required {
                            match prepare_u1_full_spectrum_conversion(&slice.input, &slice.result) {
                                Ok(preparation) => {
                                    let settings_result = inspect_macos_application(&app)
                                        .map_err(|error| error.to_string())
                                        .and_then(|installation| {
                                            let profiles_root = installation
                                                .resources_path
                                                .join("profiles/Snapmaker");
                                            let physical_profiles_root =
                                                qualified_u1_physical_profiles_root(&app)
                                                    .map_err(|error| error.to_string())?;
                                            preparation.artifacts.iter().try_for_each(
                                                |artifact| {
                                                    build_u1_full_spectrum_project_settings_with_physical_profiles(
                                                    &profiles_root,
                                                    &physical_profiles_root,
                                                    artifact,
                                                )
                                                .map(|_| ())
                                                .map_err(|error| error.to_string())
                                                },
                                            )
                                        });
                                    if let Err(error) = settings_result {
                                        available = false;
                                        reason = error;
                                    }
                                }
                                Err(error) => {
                                    available = false;
                                    reason = error.to_string();
                                }
                            }
                        }
                        adapters.push(capability_entry(
                            "u1_full_spectrum",
                            &report.adapter_id,
                            "Snapmaker Orca",
                            available,
                            reason,
                            json_report(&report)?,
                        ));
                    }
                    Err(error) => adapters.push(unavailable_capability(
                        "u1_full_spectrum",
                        "snapmaker-orca/u1-full-spectrum",
                        "Snapmaker Orca",
                        error.to_string(),
                    )),
                }
            } else {
                adapters.push(unavailable_capability(
                    "u1_full_spectrum",
                    "snapmaker-orca/u1-full-spectrum",
                    "Snapmaker Orca",
                    "Snapmaker Orca is not installed in a supported location.",
                ));
            }
        }
        if let Some(slice) = &slices.a1_mini {
            if let Some(app) = discover_bambu_studio() {
                match inspect_a1mini_macos_application(&app) {
                    Ok(report) => {
                        let mut available = report.conversion_available;
                        let mut reason = if available {
                            "A1 mini conversion is ready.".to_owned()
                        } else {
                            report.issues.join(" ")
                        };
                        if available
                            && !experimental_approval_required
                            && let Err(error) = prepare_a1mini_conversion(
                                &app,
                                &worker_path,
                                &analysis,
                                &slice.input,
                                &slice.result,
                            )
                        {
                            available = false;
                            reason = error.to_string();
                        }
                        adapters.push(capability_entry(
                            "a1_mini_mono",
                            &report.adapter_id,
                            "Bambu Studio",
                            available,
                            reason,
                            json_report(&report)?,
                        ));
                    }
                    Err(error) => adapters.push(unavailable_capability(
                        "a1_mini_mono",
                        "bambu-studio/a1-mini",
                        "Bambu Studio",
                        error.to_string(),
                    )),
                }
            } else {
                adapters.push(unavailable_capability(
                    "a1_mini_mono",
                    "bambu-studio/a1-mini",
                    "Bambu Studio",
                    "Bambu Studio is not installed in a supported location.",
                ));
            }
        }
        let available = !adapters.is_empty() && adapters.iter().all(|adapter| adapter.available);
        let reason = if available {
            if excluded_count == 0 {
                format!(
                    "{} required native adapter(s) are ready.{}",
                    adapters.len(),
                    if experimental_approval_required {
                        " Fingerprint-bound approval of the Experimental source dialect is required before preflight."
                    } else {
                        ""
                    }
                )
            } else {
                format!(
                    "{} required native adapter(s) are ready for the valid jobs; explicit approval is required to exclude {excluded_count} source unit(s).",
                    adapters.len()
                )
            }
        } else {
            adapters
                .iter()
                .filter(|adapter| !adapter.available)
                .map(|adapter| format!("{}: {}", adapter.slicer, adapter.reason))
                .collect::<Vec<_>>()
                .join(" ")
        };
        Ok(ConversionCapabilityView {
            available,
            reason,
            plan_fingerprint: Some(fingerprint),
            source_dialect_support: dialect_support,
            experimental_dialect_approval_required: experimental_approval_required,
            experimental_dialect_fingerprint: experimental_fingerprint,
            adapters,
        })
    })
    .await
    .map_err(|error| format!("conversion capability worker failed: {error}"))?
}

#[tauri::command]
async fn prepare_conversion(
    source_path: String,
    source_sha256: String,
    plan_fingerprint: String,
    partial_conversion_approval: Option<PartialConversionApproval>,
    experimental_dialect_approval: Option<ExperimentalDialectApproval>,
    cache: tauri::State<'_, ProjectCache>,
) -> Result<PreparedConversionResponse, String> {
    let canonical_path = canonical_project_path(&source_path)?;
    let generation = begin_cache_operation(&cache)?;
    let (analysis, input, result, canonical_fingerprint, ready, invalidated) = {
        let cached = cache
            .analyzed
            .lock()
            .map_err(|_| "project cache is unavailable".to_owned())?;
        let cached = cached
            .as_ref()
            .ok_or_else(|| "Analyze the selected 3MF before conversion.".to_owned())?;
        if cached.canonical_path != canonical_path {
            return Err("The conversion source does not match the analyzed project.".to_owned());
        }
        (
            cached.analysis.clone(),
            cached.planning_input.clone(),
            cached.planning_result.clone(),
            cached.plan_fingerprint.clone(),
            cached.latest_plan.is_ready(),
            cached.latest_plan.invalidated,
        )
    };
    if invalidated {
        return Err(
            "Recalculate and validate the plan after the filament or calibration library changed."
                .into(),
        );
    }
    if !ready && partial_conversion_approval.is_none() {
        return Err(
            "The plan has blocking errors. Explicitly approve the exact backend-listed exclusions to convert valid jobs."
                .into(),
        );
    }
    if source_sha256.trim_start_matches("sha256:") != analysis.input.sha256 {
        return Err("The requested source hash does not match the analyzed project.".into());
    }
    if plan_fingerprint != canonical_fingerprint {
        return Err("The requested plan fingerprint is stale. Validate choices again.".into());
    }
    validate_source_dialect_for_adapter(&analysis, experimental_dialect_approval.as_ref())?;
    let worker_path = canonical_path.clone();
    let worker_fingerprint = canonical_fingerprint.clone();
    let worker_partial_approval = partial_conversion_approval.clone();
    let worker_experimental_approval = experimental_dialect_approval.clone();
    let (preparation, targets) = tauri::async_runtime::spawn_blocking(move || {
        prepare_required_targets(
            &worker_path,
            &analysis,
            &input,
            &result,
            &worker_fingerprint,
            worker_partial_approval.as_ref(),
            worker_experimental_approval.as_ref(),
            None,
        )
    })
    .await
    .map_err(|error| format!("conversion preparation worker failed: {error}"))??;
    let token = uuid::Uuid::new_v4().to_string();
    let control = MixedConversionControl::new();
    let conversion_id = control.conversion_id().to_owned();
    let _generation = ensure_current_cache_operation(&cache, generation, "conversion preflight")?;
    {
        let cached = cache
            .analyzed
            .lock()
            .map_err(|_| "project cache is unavailable".to_owned())?;
        let cached = cached.as_ref().ok_or_else(|| {
            "The analyzed project was cleared before conversion preflight completed.".to_owned()
        })?;
        if cached.canonical_path != canonical_path
            || cached.analysis.input.sha256 != preparation.source_sha256
            || cached.plan_fingerprint != preparation.plan_fingerprint
            || cached.latest_plan.invalidated
            || (!cached.latest_plan.is_ready() && preparation.excluded_source_units.is_empty())
        {
            return Err(
                "The project or plan changed while conversion preflight was running. Open preflight again."
                    .into(),
            );
        }
    }
    *cache
        .prepared
        .lock()
        .map_err(|_| "conversion preparation cache is unavailable".to_owned())? =
        Some(CachedPreparation {
            token: token.clone(),
            cache_generation: generation,
            canonical_path,
            source_sha256: preparation.source_sha256.clone(),
            plan_fingerprint: preparation.plan_fingerprint.clone(),
            created_at: Instant::now(),
            partial_conversion_approval: if preparation.excluded_source_units.is_empty() {
                None
            } else {
                Some(PartialConversionApproval {
                    excluded_source_units: preparation.excluded_source_units.clone(),
                })
            },
            experimental_dialect_approval,
            preparation: preparation.clone(),
            targets,
            control,
        });
    Ok(PreparedConversionResponse {
        preparation_token: token,
        conversion_id,
        preparation,
    })
}

fn canonical_recovery_record_id(conversion_id: &str) -> Result<String, String> {
    let parsed = uuid::Uuid::parse_str(conversion_id)
        .map_err(|_| "Conversion recovery record has an invalid conversion ID.".to_owned())?;
    let canonical = parsed.hyphenated().to_string();
    if canonical != conversion_id {
        return Err("Conversion recovery record has a non-canonical conversion ID.".into());
    }
    Ok(canonical)
}

fn recovery_record_path(
    registry_directory: &Path,
    record: &U1DirectStagingRecoveryRecord,
) -> Result<PathBuf, String> {
    Ok(registry_directory.join(format!(
        "{}.json",
        canonical_recovery_record_id(record.conversion_id())?
    )))
}

fn ensure_recovery_registry(registry_directory: &Path) -> Result<(), String> {
    fs::create_dir_all(registry_directory)
        .map_err(|error| format!("Failed to create the conversion recovery registry: {error}"))?;
    let metadata = fs::symlink_metadata(registry_directory)
        .map_err(|error| format!("Failed to inspect the conversion recovery registry: {error}"))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err("The conversion recovery registry is not a real directory.".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(registry_directory, fs::Permissions::from_mode(0o700)).map_err(
            |error| format!("Failed to secure the conversion recovery registry: {error}"),
        )?;
    }
    Ok(())
}

fn sync_recovery_registry(registry_directory: &Path) -> Result<(), String> {
    File::open(registry_directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("Failed to synchronize the conversion recovery registry: {error}"))
}

fn write_recovery_record(
    registry_directory: &Path,
    record: &U1DirectStagingRecoveryRecord,
) -> Result<PathBuf, String> {
    ensure_recovery_registry(registry_directory)?;
    let path = recovery_record_path(registry_directory, record)?;
    let mut bytes = serde_json::to_vec(record)
        .map_err(|error| format!("Failed to serialize conversion recovery state: {error}"))?;
    bytes.push(b'\n');
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .map_err(|error| format!("Failed to register conversion recovery state: {error}"))?;
    if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&path);
        return Err(format!(
            "Failed to persist conversion recovery state: {error}"
        ));
    }
    if let Err(error) = sync_recovery_registry(registry_directory) {
        let _ = fs::remove_file(&path);
        let _ = sync_recovery_registry(registry_directory);
        return Err(error);
    }
    Ok(path)
}

fn remove_recovery_record(registry_directory: &Path, path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => sync_recovery_registry(registry_directory),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Failed to remove completed conversion recovery state: {error}"
        )),
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
struct RecoveryCleanupReport {
    removed_records: usize,
    active_records: usize,
    failed_records: usize,
}

fn cleanup_recovery_registry(registry_directory: &Path) -> Result<RecoveryCleanupReport, String> {
    if !registry_directory.exists() {
        return Ok(RecoveryCleanupReport::default());
    }
    let metadata = fs::symlink_metadata(registry_directory)
        .map_err(|error| format!("Failed to inspect conversion recovery state: {error}"))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err("The conversion recovery registry is not a real directory.".into());
    }
    let mut report = RecoveryCleanupReport::default();
    for entry in fs::read_dir(registry_directory)
        .map_err(|error| format!("Failed to read conversion recovery state: {error}"))?
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                report.failed_records += 1;
                continue;
            }
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => {
                report.failed_records += 1;
                continue;
            }
        };
        if !file_type.is_file() || file_type.is_symlink() {
            continue;
        }
        let Some(file_name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(conversion_id) = file_name.strip_suffix(".json") else {
            continue;
        };
        if canonical_recovery_record_id(conversion_id).is_err() {
            continue;
        }
        match entry.metadata() {
            Ok(metadata) if metadata.len() <= MAX_CONVERSION_RECOVERY_RECORD_BYTES => {}
            _ => {
                report.failed_records += 1;
                continue;
            }
        }
        let bytes = match fs::read(entry.path()) {
            Ok(bytes) => bytes,
            Err(_) => {
                report.failed_records += 1;
                continue;
            }
        };
        let record = match serde_json::from_slice::<U1DirectStagingRecoveryRecord>(&bytes) {
            Ok(record) if record.conversion_id() == conversion_id => record,
            _ => {
                report.failed_records += 1;
                continue;
            }
        };
        match cleanup_abandoned_u1_direct_staging(&record) {
            Ok(U1DirectStagingCleanup::OwnerStillRunning) => report.active_records += 1,
            Ok(U1DirectStagingCleanup::Removed | U1DirectStagingCleanup::Missing) => {
                if remove_recovery_record(registry_directory, &entry.path()).is_ok() {
                    report.removed_records += 1;
                } else {
                    report.failed_records += 1;
                }
            }
            Err(_) => report.failed_records += 1,
        }
    }
    Ok(report)
}

fn recovery_registry_for_app(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| path.join(CONVERSION_RECOVERY_DIRECTORY))
        .map_err(|error| format!("Failed to resolve the application data directory: {error}"))
}

fn publication_receipt_registry_for_app(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| path.join(PUBLICATION_RECEIPT_DIRECTORY))
        .map_err(|error| format!("Failed to resolve the application data directory: {error}"))
}

fn ensure_private_receipt_registry(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
                return Err(
                    "conversion_receipt_unavailable: The private publication receipt registry is not a real directory."
                        .into(),
                );
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or_else(|| {
                "conversion_receipt_unavailable: The private receipt registry has no parent directory."
                    .to_owned()
            })?;
            if !parent.is_dir() {
                fs::create_dir_all(parent).map_err(|error| {
                    format!(
                        "conversion_receipt_unavailable: Failed to create application data: {error}"
                    )
                })?;
            }
            create_private_directory(path)
                .map_err(|error| format!("conversion_receipt_unavailable: {error}"))?;
        }
        Err(error) => {
            return Err(format!(
                "conversion_receipt_unavailable: Failed to inspect the private publication receipt registry: {error}"
            ));
        }
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "conversion_receipt_unavailable: Failed to verify the private publication receipt registry: {error}"
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let parent_uid = path
            .parent()
            .and_then(|parent| fs::symlink_metadata(parent).ok())
            .map(|metadata| metadata.uid())
            .ok_or_else(|| {
                "conversion_receipt_unavailable: Failed to verify application-data ownership."
                    .to_owned()
            })?;
        if metadata.uid() != parent_uid || metadata.mode() & 0o077 != 0 {
            return Err(
                "conversion_receipt_unavailable: The private publication receipt registry has unsafe ownership or permissions."
                    .into(),
            );
        }
    }
    Ok(())
}

fn trusted_directory_identity(path: &Path) -> Result<TrustedDirectoryIdentity, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "conversion_revalidation_invalid_bundle: Failed to inspect the published output root: {error}"
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(
            "conversion_revalidation_invalid_bundle: The published output root is not a real directory."
                .into(),
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(TrustedDirectoryIdentity {
            device_id: Some(metadata.dev()),
            file_id: Some(metadata.ino()),
        })
    }
    #[cfg(not(unix))]
    {
        Ok(TrustedDirectoryIdentity {
            device_id: None,
            file_id: None,
        })
    }
}

fn publication_receipt_path(registry: &Path, output_root: &Path) -> Result<PathBuf, String> {
    let identity = serde_json::to_vec(output_root).map_err(|error| {
        format!("conversion_receipt_unavailable: Failed to identify the output root: {error}")
    })?;
    Ok(registry.join(format!("{:x}.json", Sha256::digest(identity))))
}

fn canonical_preflight_sha256(preparation: &NativeConversionPreparation) -> Result<String, String> {
    let bytes = serde_json::to_vec(preparation).map_err(|error| {
        format!(
            "conversion_receipt_unavailable: Failed to serialize canonical preflight evidence: {error}"
        )
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn receipt_artifacts(artifacts: &[NativePublishedArtifact]) -> Vec<PublicationReceiptArtifact> {
    artifacts
        .iter()
        .map(|artifact| PublicationReceiptArtifact {
            adapter_id: artifact.adapter_id.clone(),
            target: artifact.target.clone(),
            batch_id: artifact.batch_id.clone(),
            file_name: artifact.file_name.clone(),
            relative_path: artifact.relative_path.clone(),
            byte_size: artifact.byte_size,
            sha256: artifact.sha256.clone(),
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn build_publication_receipt(
    source_path: &Path,
    analysis: &ProjectAnalysis,
    plan_fingerprint: &str,
    preparation: &NativeConversionPreparation,
    output_root: &Path,
    output_identity_path: &Path,
    metadata: Vec<PublicationReceiptFile>,
    artifacts: &[NativePublishedArtifact],
) -> Result<PublicationReceipt, String> {
    Ok(PublicationReceipt {
        schema_version: PUBLICATION_RECEIPT_SCHEMA_VERSION,
        converter: published_converter_manifest(),
        source_path: source_path.to_owned(),
        source_byte_size: analysis.input.byte_size,
        source_sha256: analysis.input.sha256.clone(),
        plan_fingerprint: plan_fingerprint.to_owned(),
        canonical_preflight_sha256: canonical_preflight_sha256(preparation)?,
        output_root: output_root.to_owned(),
        output_root_identity: trusted_directory_identity(output_identity_path)?,
        metadata,
        artifacts: receipt_artifacts(artifacts),
    })
}

struct StagedPublicationReceipt {
    temporary: tempfile::NamedTempFile,
    destination: PathBuf,
}

impl StagedPublicationReceipt {
    fn publish(self, registry: &Path) -> Result<PathBuf, String> {
        self.temporary.persist(&self.destination).map_err(|error| {
            format!(
                "conversion_receipt_unavailable: Failed to publish publication receipt atomically: {}",
                error.error
            )
        })?;
        File::open(registry)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                format!(
                    "conversion_receipt_unavailable: Failed to synchronize the publication receipt registry: {error}"
                )
            })?;
        Ok(self.destination)
    }
}

fn stage_publication_receipt(
    registry: &Path,
    receipt: &PublicationReceipt,
) -> Result<StagedPublicationReceipt, String> {
    ensure_private_receipt_registry(registry)?;
    let destination = publication_receipt_path(registry, &receipt.output_root)?;
    match fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_file() => {
            return Err(
                "conversion_receipt_unavailable: The existing publication receipt is not a regular file."
                    .into(),
            );
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "conversion_receipt_unavailable: Failed to inspect the existing publication receipt: {error}"
            ));
        }
    }
    let mut bytes = serde_json::to_vec_pretty(receipt).map_err(|error| {
        format!("conversion_receipt_unavailable: Failed to serialize publication receipt: {error}")
    })?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_PUBLICATION_RECEIPT_BYTES {
        return Err(
            "conversion_receipt_unavailable: The publication receipt exceeds its size limit."
                .into(),
        );
    }
    let mut temporary = tempfile::NamedTempFile::new_in(registry).map_err(|error| {
        format!("conversion_receipt_unavailable: Failed to create publication receipt: {error}")
    })?;
    #[cfg(unix)]
    temporary
        .as_file()
        .set_permissions({
            use std::os::unix::fs::PermissionsExt;
            fs::Permissions::from_mode(0o600)
        })
        .map_err(|error| {
            format!("conversion_receipt_unavailable: Failed to secure publication receipt: {error}")
        })?;
    temporary
        .write_all(&bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| {
            format!("conversion_receipt_unavailable: Failed to write publication receipt: {error}")
        })?;
    Ok(StagedPublicationReceipt {
        temporary,
        destination,
    })
}

fn load_publication_receipt(
    registry: &Path,
    output_root: &Path,
) -> Result<PublicationReceipt, String> {
    ensure_private_receipt_registry(registry)?;
    let path = publication_receipt_path(registry, output_root)?;
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        format!(
            "conversion_revalidation_receipt_missing: The trusted publication receipt is unavailable, so the existing bundle is untrusted: {error}. {UNTRUSTED_BUNDLE_RETRY_GUIDANCE}"
        )
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.len() > MAX_PUBLICATION_RECEIPT_BYTES
    {
        return Err(format!(
            "conversion_revalidation_receipt_invalid: The trusted publication receipt is not a bounded regular file, so the existing bundle is untrusted. {UNTRUSTED_BUNDLE_RETRY_GUIDANCE}"
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let registry_uid = fs::symlink_metadata(registry)
            .map(|metadata| metadata.uid())
            .map_err(|error| {
                format!(
                    "conversion_revalidation_receipt_invalid: Failed to verify trusted registry ownership, so the existing bundle is untrusted: {error}. {UNTRUSTED_BUNDLE_RETRY_GUIDANCE}"
                )
            })?;
        if metadata.uid() != registry_uid || metadata.mode() & 0o077 != 0 {
            return Err(format!(
                "conversion_revalidation_receipt_invalid: The trusted publication receipt has unsafe ownership or permissions, so the existing bundle is untrusted. {UNTRUSTED_BUNDLE_RETRY_GUIDANCE}"
            ));
        }
    }
    let (bytes, _) = bounded_regular_file(
        &path,
        MAX_PUBLICATION_RECEIPT_BYTES,
        "trusted publication receipt",
    )?;
    serde_json::from_slice::<PublicationReceipt>(&bytes).map_err(|error| {
        format!(
            "conversion_revalidation_receipt_invalid: The trusted publication receipt is corrupt, so the existing bundle is untrusted: {error}. {UNTRUSTED_BUNDLE_RETRY_GUIDANCE}"
        )
    })
}

fn synchronize_directory(path: &Path, context: &str) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("{context}: {error}"))
}

fn synchronize_published_bundle_parent(output_root: &Path) -> Result<(), String> {
    let parent = output_root
        .parent()
        .ok_or_else(|| "the published output root has no parent directory".to_owned())?;
    synchronize_directory(
        parent,
        "failed to synchronize the destination directory after publishing the bundle",
    )
}

fn rename_directory_no_clobber(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let source = CString::new(source.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "source path contains an interior NUL byte",
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
            // SAFETY: both C strings remain alive for the call and contain no
            // interior NUL. RENAME_EXCL makes the directory move no-clobber.
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
            // SAFETY: both C strings remain alive for the call and contain no
            // interior NUL. RENAME_NOREPLACE makes the directory move no-clobber.
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
            "this platform has no qualified atomic no-replace directory rename primitive",
        ))
    }
}

fn quarantine_unreceipted_bundle(output_root: &Path, parent: &Path) -> Result<PathBuf, String> {
    for _ in 0..8 {
        let quarantine = parent.join(format!(
            "{UNRECEIPTED_QUARANTINE_PREFIX}{}",
            uuid::Uuid::new_v4()
        ));
        match rename_directory_no_clobber(output_root, &quarantine) {
            Ok(()) => return Ok(quarantine),
            Err(error)
                if error.kind() == io::ErrorKind::AlreadyExists
                    || matches!(
                        error.raw_os_error(),
                        Some(code) if code == libc::EEXIST || code == libc::ENOTEMPTY
                    ) => {}
            Err(error) => {
                return Err(format!(
                    "failed to atomically quarantine the published output: {error}"
                ));
            }
        }
    }
    Err("failed to allocate a unique private rollback quarantine path".into())
}

fn rollback_unreceipted_published_bundle(
    output_root: &Path,
    expected_identity: &TrustedDirectoryIdentity,
) -> Result<(), String> {
    rollback_unreceipted_published_bundle_with_hook(output_root, expected_identity, |_| {})
}

fn rollback_unreceipted_published_bundle_with_hook(
    output_root: &Path,
    expected_identity: &TrustedDirectoryIdentity,
    after_quarantine: impl FnOnce(&Path),
) -> Result<(), String> {
    let parent = output_root
        .parent()
        .ok_or_else(|| "the just-published output root has no parent directory".to_owned())?;
    let quarantine = quarantine_unreceipted_bundle(output_root, parent)?;
    after_quarantine(&quarantine);
    synchronize_directory(
        parent,
        "failed to synchronize the receipt-failure quarantine move",
    )?;

    let moved_identity = trusted_directory_identity(&quarantine)?;
    if moved_identity != *expected_identity {
        return match rename_directory_no_clobber(&quarantine, output_root) {
            Ok(()) => {
                synchronize_directory(
                    parent,
                    "failed to synchronize restoration of the identity-mismatched output",
                )?;
                Err(
                    "the entry moved into receipt-failure quarantine did not match the just-published bundle; it was restored without deletion"
                        .into(),
                )
            }
            Err(error) => {
                let _ = synchronize_directory(
                    parent,
                    "failed to synchronize the identity-mismatched quarantine",
                );
                Err(format!(
                    "the entry moved into receipt-failure quarantine did not match the just-published bundle and could not be restored without replacing another entry; it remains quarantined at {}: {error}",
                    quarantine.display()
                ))
            }
        };
    }

    fs::remove_dir_all(&quarantine).map_err(|error| {
        format!(
            "failed to remove the verified just-published bundle from private quarantine: {error}"
        )
    })?;
    synchronize_directory(parent, "failed to synchronize receipt-failure rollback")?;
    match fs::symlink_metadata(output_root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(
            "the just-published bundle was removed safely, but another entry now occupies its public output path"
                .into(),
        ),
        Err(error) => Err(format!(
            "the just-published bundle was removed safely, but the public output path could not be verified: {error}"
        )),
    }
}

fn complete_publication_receipt_or_rollback(
    output_root: &Path,
    expected_identity: &TrustedDirectoryIdentity,
    publish_receipt: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    complete_publication_receipt_or_rollback_with_parent_sync(
        output_root,
        expected_identity,
        || synchronize_published_bundle_parent(output_root),
        publish_receipt,
    )
}

fn complete_publication_receipt_or_rollback_with_parent_sync(
    output_root: &Path,
    expected_identity: &TrustedDirectoryIdentity,
    synchronize_parent: impl FnOnce() -> Result<(), String>,
    publish_receipt: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let receipt_result = (|| {
        // The bundle rename must be durable before its trusted receipt can be
        // committed. Otherwise a crash can preserve the receipt but lose the
        // directory entry it attests to.
        synchronize_parent()?;
        let canonical = output_root.canonicalize().map_err(|error| {
            format!("failed to canonicalize the published output root: {error}")
        })?;
        if canonical != output_root
            || trusted_directory_identity(output_root)? != *expected_identity
        {
            return Err(
                "the published output root identity differs from the staged receipt".to_owned(),
            );
        }
        publish_receipt()
    })();
    if let Err(receipt_error) = receipt_result {
        return match rollback_unreceipted_published_bundle(output_root, expected_identity) {
            Ok(()) => Err(format!(
                "conversion_receipt_publication_failed: The trusted receipt could not be published and the new bundle was rolled back; retry is safe: {receipt_error}"
            )),
            Err(rollback_error) => Err(format!(
                "conversion_published_without_recovery: Quick actions are disabled because the trusted receipt failed ({receipt_error}) and rollback could not prove that {} is clear ({rollback_error}). {UNTRUSTED_BUNDLE_RETRY_GUIDANCE}",
                output_root.display(),
            )),
        };
    }
    Ok(())
}

fn ensure_conversion_active(control: &MixedConversionControl) -> Result<(), String> {
    if control.state() == U1DirectConversionState::Cancelled {
        Err("conversion_cancelled: Conversion was cancelled before publication.".into())
    } else {
        Ok(())
    }
}

fn create_private_directory(path: &Path) -> Result<(), String> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|error| format!("Failed to create {}: {error}", path.display()))
}

fn write_staged_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("Failed to create {}: {error}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("Failed to write {}: {error}", path.display()))
}

fn ensure_generated_metadata_within_limit(
    bytes: &[u8],
    maximum_bytes: u64,
    description: &str,
) -> Result<(), String> {
    if bytes.len() as u64 > maximum_bytes {
        return Err(format!(
            "Generated {description} exceeds its {maximum_bytes}-byte publication limit."
        ));
    }
    Ok(())
}

fn relative_output_path(directory: &str, file_name: &str) -> String {
    format!("{directory}/{file_name}")
}

fn manifest_artifact(artifact: &NativePublishedArtifact) -> serde_json::Value {
    serde_json::json!({
        "adapterId": artifact.adapter_id,
        "target": artifact.target,
        "printer": artifact.printer,
        "strategy": artifact.strategy,
        "slicer": artifact.slicer,
        "batchId": artifact.batch_id,
        "fileName": artifact.file_name,
        "relativePath": artifact.relative_path,
        "byteSize": artifact.byte_size,
        "sha256": artifact.sha256,
        "plateCount": artifact.plate_count,
        "targetPlateIds": artifact.target_plate_ids,
        "sourceUnitIds": artifact.source_unit_ids,
        "loadout": artifact.loadout,
        "setupActions": artifact.setup_actions,
        "validationStatus": artifact.validation_status,
        "adapterEvidence": artifact.adapter_evidence,
    })
}

fn source_to_target_manifest(
    input: &PlanningInput,
    result: &PlanningResult,
    artifacts: &[NativePublishedArtifact],
) -> Result<Vec<serde_json::Value>, String> {
    let source_units = input
        .scopes
        .iter()
        .flat_map(|scope| {
            scope.units.iter().map(move |unit| {
                (
                    (scope.id.as_str(), unit.id.as_str()),
                    unit.source_unit_id.as_str(),
                )
            })
        })
        .collect::<BTreeMap<_, _>>();
    let plates = result
        .plates
        .iter()
        .map(|plate| (plate.id.as_str(), plate))
        .collect::<BTreeMap<_, _>>();
    let mut mappings = Vec::new();
    for artifact in artifacts {
        let expected = artifact
            .source_unit_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if expected.len() != artifact.source_unit_ids.len() {
            return Err(format!(
                "Artifact {} contains duplicate source-unit identities.",
                artifact.file_name
            ));
        }
        let mut mapped = BTreeSet::new();
        for plate_id in &artifact.target_plate_ids {
            let plate = plates.get(plate_id.as_str()).ok_or_else(|| {
                format!(
                    "Artifact {} references unknown target plate {}.",
                    artifact.file_name, plate_id
                )
            })?;
            for unit in &plate.units {
                let source_unit_id = source_units
                    .get(&(unit.scope_id.as_str(), unit.unit_id.as_str()))
                    .ok_or_else(|| {
                        format!(
                            "Target plate {} references unknown unit {}/{}.",
                            plate_id, unit.scope_id, unit.unit_id
                        )
                    })?;
                if !mapped.insert(*source_unit_id) {
                    return Err(format!(
                        "Artifact {} maps source unit {} more than once.",
                        artifact.file_name, source_unit_id
                    ));
                }
                mappings.push(serde_json::json!({
                    "sourceUnitId": source_unit_id,
                    "sourceScopeId": unit.scope_id,
                    "sourcePlanningUnitId": unit.unit_id,
                    "outputFile": artifact.file_name,
                    "batchId": artifact.batch_id,
                    "targetPlateId": plate_id,
                    "printer": artifact.printer,
                    "strategy": artifact.strategy,
                }));
            }
        }
        if mapped != expected {
            return Err(format!(
                "Artifact {} source-to-target map does not match its approved coverage.",
                artifact.file_name
            ));
        }
    }
    Ok(mappings)
}

fn validate_bundle_source_partition(
    input: &PlanningInput,
    artifacts: &[NativePublishedArtifact],
    excluded_source_units: &[ExcludedSourceUnit],
) -> Result<(), String> {
    let mut canonical = BTreeMap::new();
    for scope in &input.scopes {
        for unit in &scope.units {
            let source_plate_id = unit
                .source_plate_id
                .as_deref()
                .map(|value| value.strip_prefix("plate-").unwrap_or(value).parse::<u32>())
                .transpose()
                .map_err(|_| {
                    format!(
                        "Canonical source unit {} has an invalid source-plate identity.",
                        unit.source_unit_id
                    )
                })?;
            if canonical
                .insert(
                    unit.source_unit_id.as_str(),
                    (scope.id.as_str(), unit.id.as_str(), source_plate_id),
                )
                .is_some()
            {
                return Err(format!(
                    "Canonical source-unit identity {} is duplicated.",
                    unit.source_unit_id
                ));
            }
        }
    }

    let mut scheduled = BTreeSet::new();
    for artifact in artifacts {
        for source_unit_id in &artifact.source_unit_ids {
            if !canonical.contains_key(source_unit_id.as_str()) {
                return Err(format!(
                    "Published artifact {} references unknown source unit {}.",
                    artifact.file_name, source_unit_id
                ));
            }
            if !scheduled.insert(source_unit_id.as_str()) {
                return Err(format!(
                    "Published source unit {} is covered by more than one artifact.",
                    source_unit_id
                ));
            }
        }
    }

    let mut excluded = BTreeSet::new();
    for exclusion in excluded_source_units {
        let Some((scope_id, planning_unit_id, source_plate_id)) =
            canonical.get(exclusion.source_unit_id.as_str())
        else {
            return Err(format!(
                "Partial-conversion evidence references unknown source unit {}.",
                exclusion.source_unit_id
            ));
        };
        let valid_error_identity = exclusion
            .error_identity
            .strip_prefix("sha256:")
            .is_some_and(|digest| {
                digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            });
        if exclusion.scope_id != *scope_id
            || exclusion.planning_unit_id != *planning_unit_id
            || exclusion.source_plate_id != *source_plate_id
            || exclusion.reason.trim().is_empty()
            || !valid_error_identity
        {
            return Err(format!(
                "Partial-conversion evidence for source unit {} changed after approval.",
                exclusion.source_unit_id
            ));
        }
        if scheduled.contains(exclusion.source_unit_id.as_str()) {
            return Err(format!(
                "Source unit {} is both published and excluded.",
                exclusion.source_unit_id
            ));
        }
        if !excluded.insert(exclusion.source_unit_id.as_str()) {
            return Err(format!(
                "Source unit {} is excluded more than once.",
                exclusion.source_unit_id
            ));
        }
    }

    let covered = scheduled.union(&excluded).copied().collect::<BTreeSet<_>>();
    let expected = canonical.keys().copied().collect::<BTreeSet<_>>();
    if covered != expected {
        let missing = expected.difference(&covered).copied().collect::<Vec<_>>();
        return Err(format!(
            "Published artifacts and approved exclusions do not cover every canonical source unit; missing: {}.",
            missing.join(", ")
        ));
    }
    Ok(())
}

fn ensure_exact_warning_acknowledgement(
    expected_warnings: &[String],
    acknowledged_warnings: &[String],
) -> Result<bool, String> {
    let mut canonical_expected = expected_warnings.to_vec();
    canonical_expected.sort();
    canonical_expected.dedup();
    if canonical_expected != expected_warnings {
        return Err(
            "conversion_warning_contract_invalid: Prepared conversion warnings are not canonical. Open conversion preflight again."
                .into(),
        );
    }
    if acknowledged_warnings != expected_warnings {
        return Err(
            "conversion_warning_acknowledgement_stale: The acknowledged warning evidence does not exactly match the current conversion preflight. Review the current warnings and approve them again."
                .into(),
        );
    }
    Ok(!expected_warnings.is_empty())
}

fn ensure_runtime_warnings_match_preflight(
    prepared_warnings: &[String],
    runtime_warnings: &[String],
    acknowledged_warnings: &[String],
) -> Result<bool, String> {
    if runtime_warnings != prepared_warnings {
        return Err(
            "conversion_warning_contract_changed: Conversion produced new or different warnings after preflight. Nothing was published; open preflight and review the updated warnings."
                .into(),
        );
    }
    ensure_exact_warning_acknowledgement(runtime_warnings, acknowledged_warnings)
}

#[derive(Debug)]
struct PublishedArtifactFacts {
    adapter_id: String,
    target: &'static str,
    batch_id: String,
    file_name: String,
    relative_path: String,
    path: PathBuf,
    byte_size: u64,
    sha256: String,
    plate_count: usize,
    target_plate_ids: Vec<String>,
    source_unit_ids: Vec<String>,
    output_validation: serde_json::Value,
}

fn bind_published_artifact(
    prepared: &NativeConversionPreparation,
    consumed: &mut BTreeSet<usize>,
    facts: PublishedArtifactFacts,
) -> Result<NativePublishedArtifact, String> {
    let matches = prepared
        .artifacts
        .iter()
        .enumerate()
        .filter(|(_, artifact)| {
            artifact.adapter_id == facts.adapter_id
                && artifact.target == facts.target
                && artifact.batch_id == facts.batch_id
                && artifact.file_name == facts.file_name
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(format!(
            "Adapter output {} does not match exactly one approved preflight artifact.",
            facts.file_name
        ));
    }
    let (index, approved) = matches[0];
    if !consumed.insert(index) {
        return Err(format!(
            "Adapter output {} was produced more than once.",
            facts.file_name
        ));
    }
    let actual_units = facts.source_unit_ids.iter().collect::<BTreeSet<_>>();
    let approved_units = approved.source_unit_ids.iter().collect::<BTreeSet<_>>();
    if actual_units.len() != facts.source_unit_ids.len()
        || approved_units.len() != approved.source_unit_ids.len()
        || actual_units != approved_units
    {
        return Err(format!(
            "Adapter output {} changed the approved source-unit coverage.",
            facts.file_name
        ));
    }
    let actual_plates = facts.target_plate_ids.iter().collect::<BTreeSet<_>>();
    let approved_plates = approved.target_plate_ids.iter().collect::<BTreeSet<_>>();
    if facts.plate_count != approved.target_plate_ids.len()
        || actual_plates.len() != facts.target_plate_ids.len()
        || approved_plates.len() != approved.target_plate_ids.len()
        || actual_plates != approved_plates
    {
        return Err(format!(
            "Adapter output {} changed the approved target-plate coverage ({} produced, {} approved).",
            facts.file_name,
            facts.plate_count,
            approved.target_plate_ids.len()
        ));
    }
    Ok(NativePublishedArtifact {
        adapter_id: facts.adapter_id,
        target: approved.target.clone(),
        printer: approved.printer.clone(),
        strategy: approved.strategy.clone(),
        slicer: approved.slicer.clone(),
        batch_id: facts.batch_id,
        file_name: facts.file_name,
        relative_path: facts.relative_path,
        path: facts.path,
        byte_size: facts.byte_size,
        sha256: facts.sha256,
        plate_count: facts.plate_count,
        target_plate_ids: approved.target_plate_ids.clone(),
        source_unit_ids: facts.source_unit_ids,
        loadout: approved.loadout.clone(),
        setup_actions: approved.setup_actions.clone(),
        validation_status: "Passed".into(),
        adapter_evidence: serde_json::json!({
            "approvedPreflight": approved.adapter_evidence,
            "publishedValidation": facts.output_validation,
        }),
    })
}

fn ensure_all_prepared_artifacts_consumed(
    prepared: &NativeConversionPreparation,
    consumed: &BTreeSet<usize>,
) -> Result<(), String> {
    if consumed.len() == prepared.artifacts.len()
        && (0..prepared.artifacts.len()).all(|index| consumed.contains(&index))
    {
        return Ok(());
    }
    let missing = prepared
        .artifacts
        .iter()
        .enumerate()
        .filter(|(index, _)| !consumed.contains(index))
        .map(|(_, artifact)| artifact.file_name.as_str())
        .collect::<Vec<_>>();
    Err(format!(
        "Conversion did not produce every approved preflight artifact: {}.",
        missing.join(", ")
    ))
}

fn ensure_output_path_absent(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(format!(
            "output already exists: {}. Select a different destination, or move/remove the existing bundle; the converter never overwrites an existing output path.",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Failed to inspect output destination {}: {error}",
            path.display()
        )),
    }
}

fn exact_leaf_name(value: &str) -> Result<&str, String> {
    let mut components = Path::new(value).components();
    let Some(std::path::Component::Normal(component)) = components.next() else {
        return Err(format!("Artifact name {value:?} is not a safe leaf name."));
    };
    if components.next().is_some() || component.to_str() != Some(value) {
        return Err(format!("Artifact name {value:?} is not a safe leaf name."));
    }
    Ok(value)
}

fn normalize_direct_bundle(
    source_directory: &Path,
    target_directory: &Path,
    artifact_file_names: &[String],
) -> Result<(), String> {
    if source_directory == target_directory {
        return Err("The private Direct bundle cannot be its normalized target.".into());
    }
    let metadata = fs::symlink_metadata(source_directory).map_err(|error| {
        format!(
            "Failed to inspect private U1 Direct bundle {}: {error}",
            source_directory.display()
        )
    })?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err("The private U1 Direct bundle is not a real directory.".into());
    }
    ensure_output_path_absent(target_directory)?;
    let expected_artifacts = artifact_file_names
        .iter()
        .map(|name| exact_leaf_name(name).map(str::to_owned))
        .collect::<Result<BTreeSet<_>, _>>()?;
    if expected_artifacts.len() != artifact_file_names.len() {
        return Err("The U1 Direct adapter returned duplicate artifact names.".into());
    }
    let expected_metadata = BTreeSet::from([
        "manifest.json".to_owned(),
        "print-plan.json".to_owned(),
        "checksums.sha256".to_owned(),
    ]);
    let mut found_artifacts = BTreeSet::new();
    let mut found_metadata = BTreeSet::new();
    for entry in fs::read_dir(source_directory).map_err(|error| {
        format!(
            "Failed to read private U1 Direct bundle {}: {error}",
            source_directory.display()
        )
    })? {
        let entry =
            entry.map_err(|error| format!("Failed to read Direct bundle entry: {error}"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "The private Direct bundle contains a non-UTF-8 file name.".to_owned())?;
        let entry_metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| format!("Failed to inspect Direct bundle entry {name}: {error}"))?;
        if !entry_metadata.file_type().is_file() || entry_metadata.file_type().is_symlink() {
            return Err(format!(
                "The private Direct bundle entry {name:?} is not a regular file."
            ));
        }
        if expected_artifacts.contains(&name) {
            found_artifacts.insert(name);
        } else if expected_metadata.contains(&name) {
            found_metadata.insert(name);
        } else {
            return Err(format!(
                "The private Direct bundle contains unexpected entry {name:?}."
            ));
        }
    }
    if found_artifacts != expected_artifacts || found_metadata != expected_metadata {
        return Err("The private Direct bundle is incomplete.".into());
    }

    create_private_directory(target_directory)?;
    for name in &expected_artifacts {
        fs::rename(source_directory.join(name), target_directory.join(name))
            .map_err(|error| format!("Failed to normalize U1 Direct artifact {name}: {error}"))?;
    }
    for name in &expected_metadata {
        fs::remove_file(source_directory.join(name))
            .map_err(|error| format!("Failed to remove private Direct metadata {name}: {error}"))?;
    }
    fs::remove_dir(source_directory).map_err(|error| {
        format!(
            "Failed to remove the normalized private Direct bundle {}: {error}",
            source_directory.display()
        )
    })
}

fn read_private_direct_manifest(source_directory: &Path) -> Result<serde_json::Value, String> {
    let path = source_directory.join("manifest.json");
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        format!(
            "Failed to inspect private U1 Direct manifest {}: {error}",
            path.display()
        )
    })?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_PRIVATE_ADAPTER_MANIFEST_BYTES
    {
        return Err("The private U1 Direct manifest is not a bounded regular file.".into());
    }
    let bytes = fs::read(&path).map_err(|error| {
        format!(
            "Failed to read private U1 Direct manifest {}: {error}",
            path.display()
        )
    })?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("The private U1 Direct manifest is invalid JSON: {error}"))
}

fn direct_artifact_validation(
    manifest: &serde_json::Value,
    file_name: &str,
) -> Result<serde_json::Value, String> {
    let adapter_evidence = manifest
        .get("adapterEvidence")
        .cloned()
        .ok_or_else(|| "The private U1 Direct manifest has no adapter evidence.".to_owned())?;
    let mappings = manifest
        .get("sourceToTarget")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "The private U1 Direct manifest has no source-to-target map.".to_owned())?
        .iter()
        .filter(|entry| {
            entry.get("outputFile").and_then(serde_json::Value::as_str) == Some(file_name)
        })
        .cloned()
        .collect::<Vec<_>>();
    if mappings.is_empty() {
        return Err(format!(
            "The private U1 Direct manifest has no source mapping for {file_name}."
        ));
    }
    Ok(serde_json::json!({
        "contract": "u1_direct_source_plan_bound",
        "structuralValidationPassed": true,
        "semanticValidationPassed": true,
        "adapterCapability": adapter_evidence,
        "sourceToTarget": mappings,
    }))
}

fn validate_staged_bundle_tree(
    root: &Path,
    artifacts: &[NativePublishedArtifact],
    root_metadata_present: bool,
) -> Result<(), String> {
    let mut expected_directories = BTreeMap::<String, BTreeSet<String>>::new();
    for artifact in artifacts {
        exact_leaf_name(&artifact.file_name)?;
        let path = Path::new(&artifact.relative_path);
        let mut components = path.components();
        let Some(std::path::Component::Normal(directory)) = components.next() else {
            return Err(format!(
                "Artifact path {:?} has no target directory.",
                artifact.relative_path
            ));
        };
        let Some(std::path::Component::Normal(file_name)) = components.next() else {
            return Err(format!(
                "Artifact path {:?} has no leaf file.",
                artifact.relative_path
            ));
        };
        if components.next().is_some() || file_name.to_str() != Some(artifact.file_name.as_str()) {
            return Err(format!(
                "Artifact path {:?} does not match its approved leaf name.",
                artifact.relative_path
            ));
        }
        let directory = directory
            .to_str()
            .ok_or_else(|| "Artifact target directory is not UTF-8.".to_owned())?
            .to_owned();
        expected_directories
            .entry(directory)
            .or_default()
            .insert(artifact.file_name.clone());
    }
    let expected_root_metadata = if root_metadata_present {
        BTreeSet::from([
            PUBLISHED_MANIFEST_FILE_NAME.to_owned(),
            PUBLISHED_CONVERSION_PLAN_FILE_NAME.to_owned(),
            PUBLISHED_CONVERSION_REPORT_FILE_NAME.to_owned(),
            PUBLISHED_CHECKSUMS_FILE_NAME.to_owned(),
        ])
    } else {
        BTreeSet::new()
    };
    let mut found_directories = BTreeSet::new();
    let mut found_root_metadata = BTreeSet::new();
    for entry in fs::read_dir(root)
        .map_err(|error| format!("Failed to inspect staged conversion bundle: {error}"))?
    {
        let entry =
            entry.map_err(|error| format!("Failed to read staged bundle entry: {error}"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "The staged bundle contains a non-UTF-8 entry.".to_owned())?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| format!("Failed to inspect staged bundle entry {name}: {error}"))?;
        if let Some(expected_files) = expected_directories.get(&name) {
            if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
                return Err(format!("Staged target {name:?} is not a real directory."));
            }
            found_directories.insert(name.clone());
            let mut found_files = BTreeSet::new();
            for child in fs::read_dir(entry.path())
                .map_err(|error| format!("Failed to inspect staged target {name}: {error}"))?
            {
                let child = child
                    .map_err(|error| format!("Failed to read staged target entry: {error}"))?;
                let child_name = child
                    .file_name()
                    .into_string()
                    .map_err(|_| "A staged artifact name is not UTF-8.".to_owned())?;
                let child_metadata = fs::symlink_metadata(child.path()).map_err(|error| {
                    format!("Failed to inspect staged artifact {child_name}: {error}")
                })?;
                if !child_metadata.file_type().is_file()
                    || child_metadata.file_type().is_symlink()
                    || !expected_files.contains(&child_name)
                {
                    return Err(format!(
                        "Staged target {name:?} contains unexpected entry {child_name:?}."
                    ));
                }
                found_files.insert(child_name);
            }
            if &found_files != expected_files {
                return Err(format!("Staged target {name:?} is incomplete."));
            }
        } else if expected_root_metadata.contains(&name) {
            if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                return Err(format!("Bundle metadata {name:?} is not a regular file."));
            }
            found_root_metadata.insert(name);
        } else {
            return Err(format!("Staged bundle contains unexpected entry {name:?}."));
        }
    }
    let expected_directory_names = expected_directories
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    if found_directories != expected_directory_names
        || found_root_metadata != expected_root_metadata
    {
        return Err("The staged conversion bundle tree is incomplete.".into());
    }
    Ok(())
}

fn sync_staged_target_directories(
    root: &Path,
    artifacts: &[NativePublishedArtifact],
) -> Result<(), String> {
    let mut directories = BTreeSet::new();
    for artifact in artifacts {
        let directory = artifact.path.parent().ok_or_else(|| {
            format!(
                "Staged artifact {} has no target directory.",
                artifact.path.display()
            )
        })?;
        if directory.parent() != Some(root) {
            return Err(format!(
                "Staged artifact {} is outside its target directory.",
                artifact.path.display()
            ));
        }
        directories.insert(directory.to_path_buf());
    }
    for directory in directories {
        File::open(&directory)
            .and_then(|handle| handle.sync_all())
            .map_err(|error| {
                format!(
                    "Failed to synchronize staged target {}: {error}",
                    directory.display()
                )
            })?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn perform_native_conversion(
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    canonical_plan_fingerprint: &str,
    prepared: &NativeConversionPreparation,
    targets: &PreparedTargetSlices,
    destination_parent: &Path,
    publication_receipt_registry: &Path,
    acknowledged_warnings: &[String],
    control: &MixedConversionControl,
    operation_generation: &Mutex<u64>,
    published_outputs: &Mutex<HashMap<PathBuf, RegisteredPublishedOutput>>,
    expected_generation: u64,
) -> Result<NativeConversionResult, String> {
    ensure_conversion_active(control)?;
    ensure_exact_warning_acknowledgement(&prepared.warnings, acknowledged_warnings)?;
    if source_path.canonicalize().ok().as_deref() != Some(source_path)
        || destination_parent.canonicalize().ok().as_deref() != Some(destination_parent)
    {
        return Err(
            "Conversion source and destination paths must be canonical before publication.".into(),
        );
    }
    let final_directory = destination_parent.join(&prepared.bundle_directory_name);
    ensure_output_path_absent(&final_directory)?;
    let staging = U1NativeConversionStaging::create(destination_parent, &control.outer)
        .map_err(|error| error.to_string())?;
    let mut artifacts = Vec::<NativePublishedArtifact>::new();
    let mut consumed_prepared_artifacts = BTreeSet::new();
    let mut warnings = Vec::new();

    if let Some(slice) = &targets.direct {
        ensure_conversion_active(control)?;
        let app = discover_installation()
            .ok_or_else(|| "Snapmaker Orca is required for U1 Direct output.".to_owned())?;
        let child = U1DirectConversionControl::new();
        let child_id = child.conversion_id().to_owned();
        control.register_child(child.clone())?;
        let direct_result = convert_u1_direct_bundle_cancellable(
            &app,
            source_path,
            analysis,
            &slice.input,
            &slice.result,
            staging.path(),
            &child,
        );
        control.remove_child(&child_id);
        let direct_result = direct_result.map_err(|error| error.to_string())?;
        let private_manifest = read_private_direct_manifest(&direct_result.output_directory)?;
        let target_directory = staging.path().join("u1-direct");
        let direct_artifact_names = direct_result
            .artifacts
            .iter()
            .map(|artifact| artifact.file_name.clone())
            .collect::<Vec<_>>();
        normalize_direct_bundle(
            &direct_result.output_directory,
            &target_directory,
            &direct_artifact_names,
        )?;
        warnings.extend(direct_result.warnings);
        for artifact in direct_result.artifacts {
            let (target, _) = plain_u1_artifact_presentation(result, &artifact.batch_id)?;
            let output_validation =
                direct_artifact_validation(&private_manifest, &artifact.file_name)?;
            let relative_path = relative_output_path("u1-direct", &artifact.file_name);
            artifacts.push(bind_published_artifact(
                prepared,
                &mut consumed_prepared_artifacts,
                PublishedArtifactFacts {
                    adapter_id: direct_result.adapter_id.clone(),
                    target,
                    batch_id: artifact.batch_id,
                    file_name: artifact.file_name.clone(),
                    relative_path,
                    path: target_directory.join(&artifact.file_name),
                    byte_size: artifact.byte_size,
                    sha256: artifact.sha256,
                    plate_count: artifact.plate_count,
                    target_plate_ids: artifact.target_plate_ids,
                    source_unit_ids: artifact.source_unit_ids,
                    output_validation,
                },
            )?);
        }
    }

    if let Some(slice) = &targets.full_spectrum {
        ensure_conversion_active(control)?;
        let app = discover_installation()
            .ok_or_else(|| "Snapmaker Orca is required for Full Spectrum output.".to_owned())?;
        let installation = inspect_macos_application(&app).map_err(|error| error.to_string())?;
        let profiles_root = installation.resources_path.join("profiles/Snapmaker");
        let physical_profiles_root =
            qualified_u1_physical_profiles_root(&app).map_err(|error| error.to_string())?;
        let output_directory = staging.path().join("u1-full-spectrum");
        create_private_directory(&output_directory)?;
        let full_result = convert_u1_full_spectrum_with_substrate_builder_cancellable(
            &app,
            &slice.input,
            &slice.result,
            &output_directory,
            |artifact, scratch| {
                if control.state() == U1DirectConversionState::Cancelled {
                    return Err(U1FullSpectrumError::Plan(
                        "conversion was cancelled before publication".into(),
                    ));
                }
                let settings = build_u1_full_spectrum_project_settings_with_physical_profiles(
                    &profiles_root,
                    &physical_profiles_root,
                    artifact,
                )?;
                let substrate =
                    scratch.join(format!(".{}-normalized-substrate.3mf", artifact.batch_id));
                write_u1_full_spectrum_normalized_substrate(
                    source_path,
                    analysis,
                    &slice.input,
                    artifact,
                    &settings,
                    &substrate,
                    &control.outer,
                )
                .map_err(|error| U1FullSpectrumError::Plan(error.to_string()))?;
                Ok(substrate)
            },
            |_| control.state() == U1DirectConversionState::Cancelled,
        )
        .map_err(|error| error.to_string())?;
        warnings.extend(full_result.warnings);
        for artifact in full_result.artifacts {
            let output_validation = serde_json::json!({
                "contract": "u1_full_spectrum_source_plan_bound",
                "targetValidation": json_report(&artifact.validation)?,
            });
            let relative_path = relative_output_path("u1-full-spectrum", &artifact.file_name);
            artifacts.push(bind_published_artifact(
                prepared,
                &mut consumed_prepared_artifacts,
                PublishedArtifactFacts {
                    adapter_id: full_result.adapter_id.clone(),
                    target: "u1_full_spectrum",
                    batch_id: artifact.batch_id,
                    file_name: artifact.file_name.clone(),
                    relative_path,
                    path: output_directory.join(&artifact.file_name),
                    byte_size: artifact.byte_size,
                    sha256: artifact.sha256,
                    plate_count: artifact.plate_count,
                    target_plate_ids: artifact.target_plate_ids,
                    source_unit_ids: artifact.source_unit_ids,
                    output_validation,
                },
            )?);
        }
    }

    if let Some(slice) = &targets.a1_mini {
        ensure_conversion_active(control)?;
        let app = discover_bambu_studio()
            .ok_or_else(|| "Bambu Studio is required for A1 mini output.".to_owned())?;
        let output_directory = staging.path().join("a1-mini");
        create_private_directory(&output_directory)?;
        let a1_result = convert_a1mini_plates_cancellable(
            &app,
            source_path,
            analysis,
            &slice.input,
            &slice.result,
            &output_directory,
            || control.state() == U1DirectConversionState::Cancelled,
        )
        .map_err(|error| error.to_string())?;
        warnings.extend(a1_result.warnings);
        for artifact in a1_result.artifacts {
            let validation =
                validate_a1mini_output(&artifact.path).map_err(|error| error.to_string())?;
            if !validation.valid {
                return Err(format!(
                    "A1 mini artifact {} failed its final target validation.",
                    artifact.file_name
                ));
            }
            let output_validation = serde_json::json!({
                "contract": "a1_mini_source_plan_bound",
                "targetValidation": json_report(&validation)?,
            });
            let relative_path = relative_output_path("a1-mini", &artifact.file_name);
            artifacts.push(bind_published_artifact(
                prepared,
                &mut consumed_prepared_artifacts,
                PublishedArtifactFacts {
                    adapter_id: a1_result.adapter_id.clone(),
                    target: "a1_mini_mono",
                    batch_id: artifact.job_id,
                    file_name: artifact.file_name.clone(),
                    relative_path,
                    path: output_directory.join(&artifact.file_name),
                    byte_size: artifact.byte_size,
                    sha256: artifact.sha256,
                    plate_count: 1,
                    target_plate_ids: vec![artifact.plate_id],
                    source_unit_ids: artifact.source_unit_ids,
                    output_validation,
                },
            )?);
        }
    }

    ensure_conversion_active(control)?;
    ensure_all_prepared_artifacts_consumed(prepared, &consumed_prepared_artifacts)?;
    artifacts.sort_by_key(|artifact| {
        prepared
            .artifacts
            .iter()
            .position(|approved| {
                approved.adapter_id == artifact.adapter_id
                    && approved.target == artifact.target
                    && approved.batch_id == artifact.batch_id
                    && approved.file_name == artifact.file_name
            })
            .unwrap_or(usize::MAX)
    });
    warnings.sort();
    warnings.dedup();
    let warnings_acknowledged = ensure_runtime_warnings_match_preflight(
        &prepared.warnings,
        &warnings,
        acknowledged_warnings,
    )?;
    for artifact in &artifacts {
        let metadata = fs::symlink_metadata(&artifact.path).map_err(|error| {
            format!(
                "Failed to inspect staged artifact {}: {error}",
                artifact.path.display()
            )
        })?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(format!(
                "Staged artifact {} is not a regular file.",
                artifact.path.display()
            ));
        }
        let (byte_size, sha256) = hash_file(&artifact.path)?;
        if byte_size != artifact.byte_size || sha256 != artifact.sha256 {
            return Err(format!(
                "Staged artifact {} changed after adapter validation.",
                artifact.path.display()
            ));
        }
    }
    validate_staged_bundle_tree(staging.path(), &artifacts, false)?;
    // The public manifest is relocatable. Never persist the private staging
    // directory in it; absolute paths are returned only in the live command
    // result after the atomic publication succeeds.
    let manifest_artifacts = artifacts.iter().map(manifest_artifact).collect::<Vec<_>>();
    validate_bundle_source_partition(input, &artifacts, &prepared.excluded_source_units)?;
    let source_to_target = source_to_target_manifest(input, result, &artifacts)?;
    let excluded_scopes = excluded_scope_ids(&prepared.excluded_source_units);
    let user_approvals = user_approvals_manifest(input);
    let target_adapters = target_adapters_manifest(prepared);
    let source_provenance = published_source_manifest(source_path, analysis)?;
    let converter = published_converter_manifest();
    let manifest_value = serde_json::json!({
        "schemaVersion": PUBLISHED_BUNDLE_SCHEMA_VERSION,
        "adapterId": "u1-planner/mixed-native",
        "converter": converter,
        "source": source_provenance,
        "planFingerprint": canonical_plan_fingerprint,
        "targetAdapters": target_adapters,
        "inventorySnapshot": input.inventory,
        "artifacts": manifest_artifacts,
        "sourceToTargetMap": source_to_target,
        "excludedSourceUnits": prepared.excluded_source_units,
        "excludedScopes": excluded_scopes,
        "userApprovals": user_approvals,
        "partialConversionApproval": if prepared.excluded_source_units.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::json!({
                "excludedSourceUnits": prepared.excluded_source_units,
            })
        },
        "warningsAcknowledged": warnings_acknowledged,
        "acknowledgedWarnings": acknowledged_warnings,
        "loadoutTimeline": result.batches,
        "validationStatus": if prepared.excluded_source_units.is_empty() {
            "Passed"
        } else {
            "PassedWithApprovedExclusions"
        },
        "warnings": warnings,
    });
    let manifest_path = staging.path().join(PUBLISHED_MANIFEST_FILE_NAME);
    let mut manifest = serde_json::to_vec_pretty(&manifest_value)
        .map_err(|error| format!("Failed to serialize the bundle manifest: {error}"))?;
    manifest.push(b'\n');
    ensure_generated_metadata_within_limit(
        &manifest,
        MAX_PUBLISHED_MANIFEST_BYTES,
        "bundle manifest",
    )?;
    write_staged_file(&manifest_path, &manifest)?;
    let plan_path = staging.path().join(PUBLISHED_CONVERSION_PLAN_FILE_NAME);
    let mut plan = serde_json::to_vec_pretty(&serde_json::json!({
        "schemaVersion": PUBLISHED_BUNDLE_SCHEMA_VERSION,
        "converter": published_converter_manifest(),
        "source": published_source_manifest(source_path, analysis)?,
        "planFingerprint": canonical_plan_fingerprint,
        "planningInput": input,
        "planningResult": result,
        "excludedSourceUnits": prepared.excluded_source_units,
        "warningsAcknowledged": warnings_acknowledged,
        "acknowledgedWarnings": acknowledged_warnings,
    }))
    .map_err(|error| format!("Failed to serialize the canonical print plan: {error}"))?;
    plan.push(b'\n');
    ensure_generated_metadata_within_limit(
        &plan,
        MAX_PUBLISHED_CONVERSION_PLAN_BYTES,
        "canonical conversion plan",
    )?;
    write_staged_file(&plan_path, &plan)?;
    let report_path = staging.path().join(PUBLISHED_CONVERSION_REPORT_FILE_NAME);
    let report = render_conversion_report(&manifest_value)?;
    ensure_generated_metadata_within_limit(
        &report,
        MAX_PUBLISHED_REPORT_BYTES,
        "conversion report",
    )?;
    write_staged_file(&report_path, &report)?;
    let mut checksums = artifacts
        .iter()
        .map(|artifact| format!("{}  {}", artifact.sha256, artifact.relative_path))
        .collect::<Vec<_>>();
    checksums.push(format!(
        "{}  {PUBLISHED_MANIFEST_FILE_NAME}",
        hash_file(&manifest_path)?.1
    ));
    checksums.push(format!(
        "{}  {PUBLISHED_CONVERSION_PLAN_FILE_NAME}",
        hash_file(&plan_path)?.1
    ));
    checksums.push(format!(
        "{}  {PUBLISHED_CONVERSION_REPORT_FILE_NAME}",
        hash_file(&report_path)?.1
    ));
    checksums.sort();
    let checksums = format!("{}\n", checksums.join("\n"));
    ensure_generated_metadata_within_limit(
        checksums.as_bytes(),
        MAX_PUBLISHED_CHECKSUM_BYTES,
        "checksum list",
    )?;
    let checksums_path = staging.path().join(PUBLISHED_CHECKSUMS_FILE_NAME);
    write_staged_file(&checksums_path, checksums.as_bytes())?;
    validate_staged_bundle_tree(staging.path(), &artifacts, true)?;
    // Adapter-owned snapshots/scratch guards have been dropped and the exact
    // staged tree is final. Persist every child directory entry before the
    // parent staging directory can be atomically renamed into place.
    sync_staged_target_directories(staging.path(), &artifacts)?;
    File::open(staging.path())
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("Failed to synchronize the staged bundle: {error}"))?;
    let metadata_receipts = [
        (PUBLISHED_MANIFEST_FILE_NAME, &manifest_path),
        (PUBLISHED_CONVERSION_PLAN_FILE_NAME, &plan_path),
        (PUBLISHED_CONVERSION_REPORT_FILE_NAME, &report_path),
        (PUBLISHED_CHECKSUMS_FILE_NAME, &checksums_path),
    ]
    .into_iter()
    .map(|(relative_path, path)| {
        let (byte_size, sha256) = hash_file(path)?;
        Ok(PublicationReceiptFile {
            relative_path: relative_path.to_owned(),
            byte_size,
            sha256,
        })
    })
    .collect::<Result<Vec<_>, String>>()?;
    let publication_receipt = build_publication_receipt(
        source_path,
        analysis,
        canonical_plan_fingerprint,
        prepared,
        &final_directory,
        staging.path(),
        metadata_receipts,
        &artifacts,
    )?;
    let expected_output_identity = publication_receipt.output_root_identity.clone();
    let staged_receipt =
        stage_publication_receipt(publication_receipt_registry, &publication_receipt)?;
    let next_output_registry = staged_published_output_registry(&artifacts, &final_directory)?;
    publish_bundle_and_registry_if_cache_generation_current(
        operation_generation,
        expected_generation,
        control,
        published_outputs,
        next_output_registry,
        || {
            validate_cached_identity_cancellable(source_path, analysis, &mut || {
                control.state() == U1DirectConversionState::Cancelled
            })?;
            ensure_conversion_active(control)?;
            staging
                .publish(&final_directory, &control.outer)
                .map_err(|error| error.to_string())?;
            complete_publication_receipt_or_rollback(
                &final_directory,
                &expected_output_identity,
                || {
                    staged_receipt
                        .publish(publication_receipt_registry)
                        .map(|_| ())
                },
            )
        },
    )?;
    for artifact in &mut artifacts {
        artifact.path = final_directory.join(&artifact.relative_path);
    }
    Ok(NativeConversionResult {
        adapter_id: "u1-planner/mixed-native".into(),
        output_directory: final_directory.clone(),
        manifest_path: final_directory.join(PUBLISHED_MANIFEST_FILE_NAME),
        report_path: final_directory.join(PUBLISHED_CONVERSION_REPORT_FILE_NAME),
        artifacts,
        excluded_source_units: prepared.excluded_source_units.clone(),
        warnings_acknowledged,
        warnings,
    })
}

fn publish_bundle_and_registry_if_cache_generation_current<T>(
    operation_generation: &Mutex<u64>,
    expected_generation: u64,
    control: &MixedConversionControl,
    published_outputs: &Mutex<HashMap<PathBuf, RegisteredPublishedOutput>>,
    next_output_registry: HashMap<PathBuf, RegisteredPublishedOutput>,
    publish: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    // Keep this guard through the atomic rename. A planning-dependent mutation
    // either advances the generation first (so publication is rejected), or it
    // linearizes strictly after the already-published bundle.
    let generation = operation_generation
        .lock()
        .map_err(|_| "project operation generation is unavailable".to_owned())?;
    if *generation != expected_generation {
        let _ = control.cancel();
        return Err(
            "conversion_cancelled: The project, plan, filament library, or calibration library changed before publication."
                .into(),
        );
    }
    ensure_conversion_active(control)?;
    let mut registry = published_outputs
        .lock()
        .map_err(|_| "published output action registry is unavailable".to_owned())?;
    // Registry readers are blocked across the atomic directory rename. If the
    // rename fails, the old registry remains untouched. Once it succeeds,
    // installing the already-validated map is infallible and no caller can
    // observe a published bundle without its matching quick-action registry.
    let published = publish()?;
    *registry = next_output_registry;
    Ok(published)
}

fn bounded_regular_file(
    path: &Path,
    maximum_bytes: u64,
    description: &str,
) -> Result<(Vec<u8>, PublishedBundleMetadataIdentity), String> {
    let path_metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "conversion_revalidation_invalid_bundle: Failed to inspect {description} {}: {error}",
            path.display()
        )
    })?;
    if path_metadata.file_type().is_symlink() || !path_metadata.file_type().is_file() {
        return Err(format!(
            "conversion_revalidation_invalid_bundle: The {description} is not a regular file."
        ));
    }
    if path_metadata.len() > maximum_bytes {
        return Err(format!(
            "conversion_revalidation_invalid_bundle: The {description} exceeds its {maximum_bytes}-byte limit."
        ));
    }
    let mut open_options = fs::OpenOptions::new();
    open_options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open_options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = open_options.open(path).map_err(|error| {
        format!(
            "conversion_revalidation_invalid_bundle: Failed to open {description} {}: {error}",
            path.display()
        )
    })?;
    let opened_metadata = file.metadata().map_err(|error| {
        format!(
            "conversion_revalidation_invalid_bundle: Failed to inspect the opened {description}: {error}"
        )
    })?;
    if !opened_metadata.file_type().is_file()
        || !same_file_identity(&path_metadata, &opened_metadata)
    {
        return Err(format!(
            "conversion_revalidation_invalid_bundle: The {description} changed while it was opened."
        ));
    }
    let capacity = usize::try_from(opened_metadata.len()).map_err(|_| {
        format!(
            "conversion_revalidation_invalid_bundle: The {description} cannot be represented in memory safely."
        )
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    Read::by_ref(&mut file)
        .take(maximum_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| {
            format!("conversion_revalidation_invalid_bundle: Failed to read {description}: {error}")
        })?;
    if bytes.len() as u64 > maximum_bytes || bytes.len() as u64 != opened_metadata.len() {
        return Err(format!(
            "conversion_revalidation_invalid_bundle: The {description} changed while it was read."
        ));
    }
    let final_path_metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "conversion_revalidation_invalid_bundle: Failed to re-inspect {description}: {error}"
        )
    })?;
    if final_path_metadata.file_type().is_symlink()
        || !final_path_metadata.file_type().is_file()
        || !same_file_identity(&opened_metadata, &final_path_metadata)
        || final_path_metadata.len() != bytes.len() as u64
    {
        return Err(format!(
            "conversion_revalidation_invalid_bundle: The {description} changed during validation."
        ));
    }
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    Ok((
        bytes,
        PublishedBundleMetadataIdentity {
            path: path.to_path_buf(),
            byte_size: final_path_metadata.len(),
            sha256,
        },
    ))
}

fn same_file_identity(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        left.dev() == right.dev() && left.ino() == right.ino()
    }
    #[cfg(not(unix))]
    {
        left.len() == right.len()
    }
}

fn validate_regular_file_identity(
    path: &Path,
    expected_size: u64,
    expected_sha256: &str,
    description: &str,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!("conversion_revalidation_invalid_bundle: Failed to inspect {description}: {error}")
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.len() != expected_size
    {
        return Err(format!(
            "conversion_revalidation_invalid_bundle: The {description} changed after validation."
        ));
    }
    let mut open_options = fs::OpenOptions::new();
    open_options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open_options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = open_options.open(path).map_err(|error| {
        format!("conversion_revalidation_invalid_bundle: Failed to open {description}: {error}")
    })?;
    let opened_metadata = file.metadata().map_err(|error| {
        format!(
            "conversion_revalidation_invalid_bundle: Failed to inspect the opened {description}: {error}"
        )
    })?;
    if !opened_metadata.file_type().is_file()
        || opened_metadata.len() != expected_size
        || !same_file_identity(&metadata, &opened_metadata)
    {
        return Err(format!(
            "conversion_revalidation_invalid_bundle: The {description} changed while it was opened."
        ));
    }
    let mut hasher = Sha256::new();
    let mut byte_size = 0_u64;
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            format!("conversion_revalidation_invalid_bundle: Failed to read {description}: {error}")
        })?;
        if read == 0 {
            break;
        }
        byte_size = byte_size.checked_add(read as u64).ok_or_else(|| {
            format!(
                "conversion_revalidation_invalid_bundle: The {description} size overflowed during validation."
            )
        })?;
        hasher.update(&buffer[..read]);
    }
    let final_metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "conversion_revalidation_invalid_bundle: Failed to re-inspect {description}: {error}"
        )
    })?;
    let sha256 = format!("{:x}", hasher.finalize());
    if final_metadata.file_type().is_symlink()
        || !final_metadata.file_type().is_file()
        || !same_file_identity(&opened_metadata, &final_metadata)
        || byte_size != expected_size
        || sha256 != expected_sha256
    {
        return Err(format!(
            "conversion_revalidation_invalid_bundle: The {description} changed after validation."
        ));
    }
    Ok(())
}

fn artifact_target_directory(target: &str) -> Result<&'static str, String> {
    match target {
        "u1_direct" | "u1_cmyx_solid" => Ok("u1-direct"),
        "u1_full_spectrum" => Ok("u1-full-spectrum"),
        "a1_mini_mono" => Ok("a1-mini"),
        _ => Err(format!(
            "conversion_revalidation_invalid_bundle: Unknown published target {target:?}."
        )),
    }
}

fn validate_published_adapter_evidence(
    artifact: &PublishedBundleArtifactManifest,
    approved: &NativePreparedArtifact,
) -> Result<(), String> {
    let wrapper = artifact.adapter_evidence.as_object().ok_or_else(|| {
        format!(
            "conversion_revalidation_invalid_bundle: Artifact {} has no adapter validation evidence.",
            artifact.file_name
        )
    })?;
    if wrapper.len() != 2
        || !wrapper.contains_key("approvedPreflight")
        || !wrapper.contains_key("publishedValidation")
    {
        return Err(format!(
            "conversion_revalidation_invalid_bundle: Artifact {} has the wrong adapter evidence envelope.",
            artifact.file_name
        ));
    }
    let approved_preflight = wrapper
        .get("approvedPreflight")
        .expect("checked adapter evidence envelope");
    if !json_values_equivalent(approved_preflight, &approved.adapter_evidence) {
        return Err(format!(
            "conversion_revalidation_invalid_bundle: Artifact {} has preflight evidence that differs from the current canonical adapter approval.",
            artifact.file_name
        ));
    }
    if !wrapper
        .get("publishedValidation")
        .is_some_and(serde_json::Value::is_object)
    {
        return Err(format!(
            "conversion_revalidation_invalid_bundle: Artifact {} has no published adapter validation evidence.",
            artifact.file_name
        ));
    }
    Ok(())
}

#[allow(dead_code)]
fn snapshot_recovered_artifact(
    artifact: &NativePublishedArtifact,
) -> Result<tempfile::NamedTempFile, String> {
    if artifact.byte_size > MAX_PUBLISHED_ARTIFACT_BYTES {
        return Err(format!(
            "conversion_revalidation_invalid_bundle: Artifact {} exceeds the recovery size limit.",
            artifact.file_name
        ));
    }
    let metadata = fs::symlink_metadata(&artifact.path).map_err(|error| {
        format!(
            "conversion_revalidation_invalid_bundle: Failed to inspect artifact {}: {error}",
            artifact.file_name
        )
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.len() != artifact.byte_size
    {
        return Err(format!(
            "conversion_revalidation_invalid_bundle: Artifact {} is not the expected bounded regular file.",
            artifact.file_name
        ));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut source = options.open(&artifact.path).map_err(|error| {
        format!(
            "conversion_revalidation_invalid_bundle: Failed to open artifact {}: {error}",
            artifact.file_name
        )
    })?;
    let opened_metadata = source.metadata().map_err(|error| {
        format!(
            "conversion_revalidation_invalid_bundle: Failed to inspect opened artifact {}: {error}",
            artifact.file_name
        )
    })?;
    if !opened_metadata.file_type().is_file()
        || opened_metadata.len() != artifact.byte_size
        || !same_file_identity(&metadata, &opened_metadata)
    {
        return Err(format!(
            "conversion_revalidation_invalid_bundle: Artifact {} changed while it was opened.",
            artifact.file_name
        ));
    }

    let mut snapshot = tempfile::NamedTempFile::new().map_err(|error| {
        format!(
            "conversion_revalidation_invalid_bundle: Failed to create a private artifact snapshot: {error}"
        )
    })?;
    let mut hasher = Sha256::new();
    let mut byte_size = 0_u64;
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = source.read(&mut buffer).map_err(|error| {
            format!(
                "conversion_revalidation_invalid_bundle: Failed to read artifact {}: {error}",
                artifact.file_name
            )
        })?;
        if read == 0 {
            break;
        }
        byte_size = byte_size.checked_add(read as u64).ok_or_else(|| {
            format!(
                "conversion_revalidation_invalid_bundle: Artifact {} size overflowed during snapshotting.",
                artifact.file_name
            )
        })?;
        if byte_size > artifact.byte_size || byte_size > MAX_PUBLISHED_ARTIFACT_BYTES {
            return Err(format!(
                "conversion_revalidation_invalid_bundle: Artifact {} grew beyond its approved recovery bound.",
                artifact.file_name
            ));
        }
        snapshot.write_all(&buffer[..read]).map_err(|error| {
            format!(
                "conversion_revalidation_invalid_bundle: Failed to write the private snapshot for {}: {error}",
                artifact.file_name
            )
        })?;
        hasher.update(&buffer[..read]);
    }
    snapshot.as_file().sync_all().map_err(|error| {
        format!(
            "conversion_revalidation_invalid_bundle: Failed to synchronize the private snapshot for {}: {error}",
            artifact.file_name
        )
    })?;
    let final_metadata = fs::symlink_metadata(&artifact.path).map_err(|error| {
        format!(
            "conversion_revalidation_invalid_bundle: Failed to re-inspect artifact {}: {error}",
            artifact.file_name
        )
    })?;
    let sha256 = format!("{:x}", hasher.finalize());
    if final_metadata.file_type().is_symlink()
        || !final_metadata.file_type().is_file()
        || !same_file_identity(&opened_metadata, &final_metadata)
        || byte_size != artifact.byte_size
        || sha256 != artifact.sha256
    {
        return Err(format!(
            "conversion_revalidation_invalid_bundle: Artifact {} changed while its private snapshot was created.",
            artifact.file_name
        ));
    }
    Ok(snapshot)
}

#[allow(dead_code)]
fn exact_published_validation_evidence(
    artifact: &NativePublishedArtifact,
) -> Result<&serde_json::Value, String> {
    artifact
        .adapter_evidence
        .get("publishedValidation")
        .ok_or_else(|| {
            format!(
                "conversion_revalidation_invalid_bundle: Artifact {} has no published validation evidence.",
                artifact.file_name
            )
        })
}

#[allow(dead_code)]
fn ensure_exact_published_validation_evidence(
    artifact: &NativePublishedArtifact,
    expected_validation: &serde_json::Value,
) -> Result<(), String> {
    let expected_validation = json_wire_value(
        expected_validation,
        &format!("published validation evidence for {}", artifact.file_name),
    )?;
    if !json_values_equivalent(
        exact_published_validation_evidence(artifact)?,
        &expected_validation,
    ) {
        return Err(format!(
            "conversion_revalidation_invalid_bundle: Artifact {} has published validation evidence that differs from fresh source-plan validation.",
            artifact.file_name
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
fn validate_recovered_artifact_against_current_plan(
    artifact: &NativePublishedArtifact,
    snapshot_path: &Path,
    source_path: &Path,
    analysis: &ProjectAnalysis,
    targets: &PreparedTargetSlices,
    adapters: &OwnedNativeAdapterPreparations,
) -> Result<(), String> {
    let expected_validation = match artifact.target.as_str() {
        "a1_mini_mono" => {
            let slice = targets.a1_mini.as_ref().ok_or_else(|| {
                "conversion_revalidation_invalid_bundle: The A1 mini target slice is missing."
                    .to_owned()
            })?;
            let preparation = adapters.a1_mini.as_ref().ok_or_else(|| {
                "conversion_revalidation_invalid_bundle: The A1 mini adapter preparation is missing."
                    .to_owned()
            })?;
            let matches = preparation
                .artifacts
                .iter()
                .filter(|expected| {
                    expected.job_id == artifact.batch_id && expected.file_name == artifact.file_name
                })
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                return Err(format!(
                    "conversion_revalidation_invalid_bundle: Artifact {} does not match exactly one fresh A1 mini build plan.",
                    artifact.file_name
                ));
            }
            let app = discover_bambu_studio().ok_or_else(|| {
                "conversion_revalidation_failed: Bambu Studio is required to revalidate A1 mini output."
                    .to_owned()
            })?;
            let validation = validate_a1mini_output_against_plan(
                &app,
                source_path,
                analysis,
                &slice.input,
                &slice.result,
                matches[0],
                snapshot_path,
            )
            .map_err(|error| {
                format!(
                    "conversion_revalidation_invalid_bundle: A1 mini source-plan validation failed for {}: {error}",
                    artifact.file_name
                )
            })?;
            serde_json::json!({
                "contract": "a1_mini_source_plan_bound",
                "targetValidation": json_report(&validation)?,
            })
        }
        "u1_full_spectrum" => {
            let slice = targets.full_spectrum.as_ref().ok_or_else(|| {
                "conversion_revalidation_invalid_bundle: The Full Spectrum target slice is missing."
                    .to_owned()
            })?;
            let preparation = adapters.full_spectrum.as_ref().ok_or_else(|| {
                "conversion_revalidation_invalid_bundle: The Full Spectrum adapter preparation is missing."
                    .to_owned()
            })?;
            let matches = preparation
                .artifacts
                .iter()
                .filter(|expected| {
                    expected.batch_id == artifact.batch_id
                        && expected.file_name == artifact.file_name
                })
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                return Err(format!(
                    "conversion_revalidation_invalid_bundle: Artifact {} does not match exactly one fresh Full Spectrum build plan.",
                    artifact.file_name
                ));
            }
            let app = discover_installation().ok_or_else(|| {
                "conversion_revalidation_failed: Snapmaker Orca is required to revalidate Full Spectrum output."
                    .to_owned()
            })?;
            let installation = inspect_macos_application(&app)
                .map_err(|error| format!("conversion_revalidation_failed: {error}"))?;
            let profiles_root = installation.resources_path.join("profiles/Snapmaker");
            let physical_profiles_root = qualified_u1_physical_profiles_root(&app)
                .map_err(|error| format!("conversion_revalidation_failed: {error}"))?;
            let settings = build_u1_full_spectrum_project_settings_with_physical_profiles(
                &profiles_root,
                &physical_profiles_root,
                matches[0],
            )
            .map_err(|error| format!("conversion_revalidation_failed: {error}"))?;
            let scratch = tempfile::tempdir().map_err(|error| {
                format!(
                    "conversion_revalidation_failed: Failed to create Full Spectrum recovery scratch space: {error}"
                )
            })?;
            let substrate = scratch.path().join("normalized-substrate.3mf");
            let control = U1DirectConversionControl::new();
            write_u1_full_spectrum_normalized_substrate(
                source_path,
                analysis,
                &slice.input,
                matches[0],
                &settings,
                &substrate,
                &control,
            )
            .map_err(|error| {
                format!(
                    "conversion_revalidation_invalid_bundle: Failed to reconstruct the Full Spectrum substrate for {}: {error}",
                    artifact.file_name
                )
            })?;
            let validation = validate_u1_full_spectrum_output_against_substrate(
                snapshot_path,
                &substrate,
                &settings,
                matches[0],
            )
            .map_err(|error| {
                format!(
                    "conversion_revalidation_invalid_bundle: Full Spectrum source-plan validation failed for {}: {error}",
                    artifact.file_name
                )
            })?;
            serde_json::json!({
                "contract": "u1_full_spectrum_source_plan_bound",
                "targetValidation": json_report(&validation)?,
            })
        }
        "u1_direct" | "u1_cmyx_solid" => {
            let slice = targets.direct.as_ref().ok_or_else(|| {
                "conversion_revalidation_invalid_bundle: The Direct Spools target slice is missing."
                    .to_owned()
            })?;
            let preparation = adapters.direct.as_ref().ok_or_else(|| {
                "conversion_revalidation_invalid_bundle: The Direct Spools adapter preparation is missing."
                    .to_owned()
            })?;
            let matches = preparation
                .artifacts
                .iter()
                .filter(|expected| {
                    expected.batch_id == artifact.batch_id
                        && expected.file_name == artifact.file_name
                })
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                return Err(format!(
                    "conversion_revalidation_invalid_bundle: Artifact {} does not match exactly one fresh Direct Spools build plan.",
                    artifact.file_name
                ));
            }
            let app = discover_installation().ok_or_else(|| {
                "conversion_revalidation_failed: Snapmaker Orca is required to revalidate Direct Spools output."
                    .to_owned()
            })?;
            let validation = validate_u1_direct_output_against_plan(
                &app,
                source_path,
                analysis,
                &slice.input,
                &slice.result,
                matches[0],
                snapshot_path,
            )
            .map_err(|error| {
                format!(
                    "conversion_revalidation_invalid_bundle: Direct Spools source-plan validation failed for {}: {error}",
                    artifact.file_name
                )
            })?;
            serde_json::json!({
                "contract": "u1_direct_source_plan_bound",
                "structuralValidationPassed": true,
                "semanticValidationPassed": true,
                "adapterCapability": json_report(&validation.adapter_capability)?,
                "sourceToTarget": json_report(&validation.source_to_target)?,
            })
        }
        _ => return artifact_target_directory(&artifact.target).map(|_| ()),
    };
    ensure_exact_published_validation_evidence(artifact, &expected_validation)?;
    validate_regular_file_identity(
        &artifact.path,
        artifact.byte_size,
        &artifact.sha256,
        &format!("artifact {}", artifact.file_name),
    )
}

#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
fn validate_recovered_artifacts_against_current_plan(
    artifacts: &[NativePublishedArtifact],
    snapshots: &[tempfile::NamedTempFile],
    source_path: &Path,
    analysis: &ProjectAnalysis,
    targets: &PreparedTargetSlices,
    adapters: &OwnedNativeAdapterPreparations,
) -> Result<(), String> {
    if artifacts.len() != snapshots.len() {
        return Err(
            "conversion_revalidation_failed: The private recovery snapshot set is incomplete."
                .into(),
        );
    }
    let mut direct = Vec::new();
    let mut a1_mini = Vec::new();
    let mut full_spectrum = Vec::new();
    for (artifact, snapshot) in artifacts.iter().zip(snapshots) {
        match artifact.target.as_str() {
            "u1_direct" | "u1_cmyx_solid" => {
                let preparation = adapters.direct.as_ref().ok_or_else(|| {
                    "conversion_revalidation_invalid_bundle: The Direct Spools adapter preparation is missing."
                        .to_owned()
                })?;
                let matches = preparation
                    .artifacts
                    .iter()
                    .filter(|expected| {
                        expected.batch_id == artifact.batch_id
                            && expected.file_name == artifact.file_name
                    })
                    .collect::<Vec<_>>();
                if matches.len() != 1 {
                    return Err(format!(
                        "conversion_revalidation_invalid_bundle: Artifact {} does not match exactly one fresh Direct Spools build plan.",
                        artifact.file_name
                    ));
                }
                direct.push((artifact, snapshot.path(), matches[0]));
            }
            "a1_mini_mono" => {
                let preparation = adapters.a1_mini.as_ref().ok_or_else(|| {
                    "conversion_revalidation_invalid_bundle: The A1 mini adapter preparation is missing."
                        .to_owned()
                })?;
                let matches = preparation
                    .artifacts
                    .iter()
                    .filter(|expected| {
                        expected.job_id == artifact.batch_id
                            && expected.file_name == artifact.file_name
                    })
                    .collect::<Vec<_>>();
                if matches.len() != 1 {
                    return Err(format!(
                        "conversion_revalidation_invalid_bundle: Artifact {} does not match exactly one fresh A1 mini build plan.",
                        artifact.file_name
                    ));
                }
                a1_mini.push((artifact, snapshot.path(), matches[0]));
            }
            "u1_full_spectrum" => {
                let preparation = adapters.full_spectrum.as_ref().ok_or_else(|| {
                    "conversion_revalidation_invalid_bundle: The Full Spectrum adapter preparation is missing."
                        .to_owned()
                })?;
                let matches = preparation
                    .artifacts
                    .iter()
                    .filter(|expected| {
                        expected.batch_id == artifact.batch_id
                            && expected.file_name == artifact.file_name
                    })
                    .collect::<Vec<_>>();
                if matches.len() != 1 {
                    return Err(format!(
                        "conversion_revalidation_invalid_bundle: Artifact {} does not match exactly one fresh Full Spectrum build plan.",
                        artifact.file_name
                    ));
                }
                full_spectrum.push((artifact, snapshot.path(), matches[0]));
            }
            _ => return artifact_target_directory(&artifact.target).map(|_| ()),
        }
    }

    if !direct.is_empty() {
        let slice = targets.direct.as_ref().ok_or_else(|| {
            "conversion_revalidation_invalid_bundle: The Direct Spools target slice is missing."
                .to_owned()
        })?;
        let app = discover_installation().ok_or_else(|| {
            "conversion_revalidation_failed: Snapmaker Orca is required to revalidate Direct Spools output."
                .to_owned()
        })?;
        let candidates = direct
            .iter()
            .map(|(_, path, expected)| (*expected, *path))
            .collect::<Vec<_>>();
        let reports = validate_u1_direct_outputs_against_plan(
            &app,
            source_path,
            analysis,
            &slice.input,
            &slice.result,
            &candidates,
        )
        .map_err(|error| {
            format!(
                "conversion_revalidation_invalid_bundle: Direct Spools source-plan validation failed: {error}"
            )
        })?;
        if reports.len() != direct.len() {
            return Err(
                "conversion_revalidation_failed: Direct Spools returned an incomplete validation report set."
                    .into(),
            );
        }
        for ((artifact, _, _), validation) in direct.iter().zip(reports) {
            let expected_validation = serde_json::json!({
                "contract": "u1_direct_source_plan_bound",
                "structuralValidationPassed": true,
                "semanticValidationPassed": true,
                "adapterCapability": json_report(&validation.adapter_capability)?,
                "sourceToTarget": json_report(&validation.source_to_target)?,
            });
            ensure_exact_published_validation_evidence(artifact, &expected_validation)?;
            validate_regular_file_identity(
                &artifact.path,
                artifact.byte_size,
                &artifact.sha256,
                &format!("artifact {}", artifact.file_name),
            )?;
        }
    }

    if !a1_mini.is_empty() {
        let slice = targets.a1_mini.as_ref().ok_or_else(|| {
            "conversion_revalidation_invalid_bundle: The A1 mini target slice is missing."
                .to_owned()
        })?;
        let app = discover_bambu_studio().ok_or_else(|| {
            "conversion_revalidation_failed: Bambu Studio is required to revalidate A1 mini output."
                .to_owned()
        })?;
        let candidates = a1_mini
            .iter()
            .map(|(_, path, expected)| (*expected, *path))
            .collect::<Vec<_>>();
        let reports = validate_a1mini_outputs_against_plan(
            &app,
            source_path,
            analysis,
            &slice.input,
            &slice.result,
            &candidates,
        )
        .map_err(|error| {
            format!(
                "conversion_revalidation_invalid_bundle: A1 mini source-plan validation failed: {error}"
            )
        })?;
        if reports.len() != a1_mini.len() {
            return Err(
                "conversion_revalidation_failed: A1 mini returned an incomplete validation report set."
                    .into(),
            );
        }
        for ((artifact, _, _), validation) in a1_mini.iter().zip(reports) {
            let expected_validation = serde_json::json!({
                "contract": "a1_mini_source_plan_bound",
                "targetValidation": json_report(&validation)?,
            });
            ensure_exact_published_validation_evidence(artifact, &expected_validation)?;
            validate_regular_file_identity(
                &artifact.path,
                artifact.byte_size,
                &artifact.sha256,
                &format!("artifact {}", artifact.file_name),
            )?;
        }
    }
    if full_spectrum.len() == 1 {
        let (artifact, snapshot_path, _) = full_spectrum[0];
        validate_recovered_artifact_against_current_plan(
            artifact,
            snapshot_path,
            source_path,
            analysis,
            targets,
            adapters,
        )?;
    } else if !full_spectrum.is_empty() {
        let slice = targets.full_spectrum.as_ref().ok_or_else(|| {
            "conversion_revalidation_invalid_bundle: The Full Spectrum target slice is missing."
                .to_owned()
        })?;
        let app = discover_installation().ok_or_else(|| {
            "conversion_revalidation_failed: Snapmaker Orca is required to revalidate Full Spectrum output."
                .to_owned()
        })?;
        let installation = inspect_macos_application(&app)
            .map_err(|error| format!("conversion_revalidation_failed: {error}"))?;
        let profiles_root = installation.resources_path.join("profiles/Snapmaker");
        let physical_profiles_root = qualified_u1_physical_profiles_root(&app)
            .map_err(|error| format!("conversion_revalidation_failed: {error}"))?;
        let settings = full_spectrum
            .iter()
            .map(|(_, _, expected)| {
                build_u1_full_spectrum_project_settings_with_physical_profiles(
                    &profiles_root,
                    &physical_profiles_root,
                    expected,
                )
                .map_err(|error| format!("conversion_revalidation_failed: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let scratch = tempfile::tempdir().map_err(|error| {
            format!(
                "conversion_revalidation_failed: Failed to create Full Spectrum recovery scratch space: {error}"
            )
        })?;
        let substrate_paths = full_spectrum
            .iter()
            .enumerate()
            .map(|(index, _)| {
                scratch
                    .path()
                    .join(format!("normalized-substrate-{index}.3mf"))
            })
            .collect::<Vec<_>>();
        let substrate_requests = full_spectrum
            .iter()
            .zip(&settings)
            .zip(&substrate_paths)
            .map(|(((_, _, expected), settings), path)| {
                (*expected, settings.as_slice(), path.as_path())
            })
            .collect::<Vec<_>>();
        let control = U1DirectConversionControl::new();
        write_u1_full_spectrum_normalized_substrates(
            source_path,
            analysis,
            &slice.input,
            &substrate_requests,
            &control,
        )
        .map_err(|error| {
            format!(
                "conversion_revalidation_invalid_bundle: Failed to reconstruct Full Spectrum substrates: {error}"
            )
        })?;

        let validations = std::thread::scope(|scope| {
            let handles = full_spectrum
                .iter()
                .zip(&settings)
                .zip(&substrate_paths)
                .map(
                    |(((_artifact, snapshot_path, expected), settings), substrate_path)| {
                        scope.spawn(move || {
                            validate_u1_full_spectrum_output_against_substrate(
                                snapshot_path,
                                substrate_path,
                                settings,
                                expected,
                            )
                        })
                    },
                )
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| {
                    handle.join().map_err(|_| {
                        "conversion_revalidation_failed: A Full Spectrum validation worker terminated unexpectedly."
                            .to_owned()
                    })?
                    .map_err(|error| {
                        format!(
                            "conversion_revalidation_invalid_bundle: Full Spectrum source-plan validation failed: {error}"
                        )
                    })
                })
                .collect::<Result<Vec<_>, String>>()
        })?;
        for ((artifact, _, _), validation) in full_spectrum.iter().zip(validations) {
            let expected_validation = serde_json::json!({
                "contract": "u1_full_spectrum_source_plan_bound",
                "targetValidation": json_report(&validation)?,
            });
            ensure_exact_published_validation_evidence(artifact, &expected_validation)?;
            validate_regular_file_identity(
                &artifact.path,
                artifact.byte_size,
                &artifact.sha256,
                &format!("artifact {}", artifact.file_name),
            )?;
        }
    }
    Ok(())
}

fn recover_published_artifacts(
    root: &Path,
    manifest_artifacts: &[PublishedBundleArtifactManifest],
    prepared: &NativeConversionPreparation,
) -> Result<Vec<NativePublishedArtifact>, String> {
    if manifest_artifacts.len() != prepared.artifacts.len() || manifest_artifacts.is_empty() {
        return Err(
            "conversion_revalidation_invalid_bundle: The published artifact set does not match the current canonical preflight."
                .into(),
        );
    }
    let mut recovered = Vec::with_capacity(manifest_artifacts.len());
    let mut relative_paths = BTreeSet::new();
    for (published, approved) in manifest_artifacts.iter().zip(&prepared.artifacts) {
        exact_leaf_name(&published.file_name)?;
        let expected_relative = relative_output_path(
            artifact_target_directory(&approved.target)?,
            &approved.file_name,
        );
        let published_units = published.source_unit_ids.iter().collect::<BTreeSet<_>>();
        let approved_units = approved.source_unit_ids.iter().collect::<BTreeSet<_>>();
        if published.adapter_id != approved.adapter_id
            || published.target != approved.target
            || published.printer != approved.printer
            || published.strategy != approved.strategy
            || published.slicer != approved.slicer
            || published.batch_id != approved.batch_id
            || published.file_name != approved.file_name
            || published.relative_path != expected_relative
            || published.plate_count != approved.target_plate_ids.len()
            || published.target_plate_ids != approved.target_plate_ids
            || published_units.len() != published.source_unit_ids.len()
            || approved_units.len() != approved.source_unit_ids.len()
            || published_units != approved_units
            || published.loadout != approved.loadout
            || published.setup_actions != approved.setup_actions
            || published.validation_status != "Passed"
        {
            return Err(format!(
                "conversion_revalidation_invalid_bundle: Artifact {} differs from the current canonical preflight.",
                published.file_name
            ));
        }
        if published.sha256.len() != 64
            || !published
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || !relative_paths.insert(published.relative_path.clone())
        {
            return Err(format!(
                "conversion_revalidation_invalid_bundle: Artifact {} has invalid or duplicate identity metadata.",
                published.file_name
            ));
        }
        validate_published_adapter_evidence(published, approved)?;
        let path = root.join(&published.relative_path);
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            format!(
                "conversion_revalidation_invalid_bundle: Failed to inspect published artifact {}: {error}",
                published.file_name
            )
        })?;
        if metadata.file_type().is_symlink()
            || !metadata.file_type().is_file()
            || metadata.len() != published.byte_size
        {
            return Err(format!(
                "conversion_revalidation_invalid_bundle: Published artifact {} is not the expected regular file.",
                published.file_name
            ));
        }
        recovered.push(NativePublishedArtifact {
            adapter_id: published.adapter_id.clone(),
            target: published.target.clone(),
            printer: published.printer.clone(),
            strategy: published.strategy.clone(),
            slicer: published.slicer.clone(),
            batch_id: published.batch_id.clone(),
            file_name: published.file_name.clone(),
            relative_path: published.relative_path.clone(),
            path,
            byte_size: published.byte_size,
            sha256: published.sha256.clone(),
            plate_count: published.plate_count,
            target_plate_ids: published.target_plate_ids.clone(),
            source_unit_ids: published.source_unit_ids.clone(),
            loadout: published.loadout.clone(),
            setup_actions: published.setup_actions.clone(),
            validation_status: published.validation_status.clone(),
            adapter_evidence: published.adapter_evidence.clone(),
        });
    }
    Ok(recovered)
}

fn ensure_json_contract(
    actual: &serde_json::Value,
    expected: &impl Serialize,
    label: &str,
) -> Result<(), String> {
    let expected = json_wire_value(expected, label)?;
    if !json_values_equivalent(actual, &expected) {
        return Err(format!(
            "conversion_revalidation_invalid_bundle: The published {label} differs from the current canonical plan."
        ));
    }
    Ok(())
}

fn json_wire_value(value: &impl Serialize, label: &str) -> Result<serde_json::Value, String> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        format!("conversion_revalidation_failed: Failed to serialize {label}: {error}")
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        format!("conversion_revalidation_failed: Failed to normalize serialized {label}: {error}")
    })
}

#[allow(clippy::too_many_arguments)]
fn restore_revalidated_output_registry_if_current(
    operation_generation: &Mutex<u64>,
    expected_generation: u64,
    published_outputs: &Mutex<HashMap<PathBuf, RegisteredPublishedOutput>>,
    next_output_registry: HashMap<PathBuf, RegisteredPublishedOutput>,
    source_path: &Path,
    analysis: &ProjectAnalysis,
    root: &Path,
    artifacts: &[NativePublishedArtifact],
    metadata_identities: &[PublishedBundleMetadataIdentity],
) -> Result<(), String> {
    let generation = operation_generation
        .lock()
        .map_err(|_| "project operation generation is unavailable".to_owned())?;
    if *generation != expected_generation {
        return Err(
            "conversion_revalidation_stale: The project, plan, filament library, or calibration library changed during output validation."
                .into(),
        );
    }
    let mut registry = published_outputs
        .lock()
        .map_err(|_| "published output action registry is unavailable".to_owned())?;
    validate_cached_identity_cancellable(source_path, analysis, &mut || false)?;
    validate_staged_bundle_tree(root, artifacts, true)?;
    for metadata in metadata_identities {
        validate_regular_file_identity(
            &metadata.path,
            metadata.byte_size,
            &metadata.sha256,
            "published bundle metadata",
        )?;
    }
    for output in next_output_registry.values() {
        validate_registered_output_identity(output)?;
    }
    *registry = next_output_registry;
    drop(registry);
    drop(generation);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn revalidate_published_bundle(
    source_path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    plan_fingerprint: &str,
    output_directory: &Path,
    publication_receipt_registry: &Path,
    experimental_dialect_approval: Option<&ExperimentalDialectApproval>,
    operation_generation: &Mutex<u64>,
    published_outputs: &Mutex<HashMap<PathBuf, RegisteredPublishedOutput>>,
    expected_generation: u64,
) -> Result<NativeConversionResult, String> {
    let trusted_receipt = load_publication_receipt(publication_receipt_registry, output_directory)?;
    if trusted_receipt.schema_version != PUBLICATION_RECEIPT_SCHEMA_VERSION
        || trusted_receipt.converter != published_converter_manifest()
        || trusted_receipt.source_path != source_path
        || trusted_receipt.source_byte_size != analysis.input.byte_size
        || trusted_receipt.source_sha256 != analysis.input.sha256
        || trusted_receipt.plan_fingerprint != plan_fingerprint
        || trusted_receipt.output_root != output_directory
        || trusted_receipt.output_root_identity != trusted_directory_identity(output_directory)?
    {
        return Err(format!(
            "conversion_revalidation_receipt_mismatch: The trusted publication receipt does not identify the current source, plan, build, and output root, so the existing bundle is untrusted. {UNTRUSTED_BUNDLE_RETRY_GUIDANCE}"
        ));
    }
    let manifest_path = output_directory.join(PUBLISHED_MANIFEST_FILE_NAME);
    let (manifest_bytes, manifest_identity) = bounded_regular_file(
        &manifest_path,
        MAX_PUBLISHED_MANIFEST_BYTES,
        "published manifest",
    )?;
    let manifest_value = serde_json::from_slice::<serde_json::Value>(&manifest_bytes).map_err(
        |error| {
            format!(
                "conversion_revalidation_invalid_bundle: The published manifest is invalid JSON: {error}"
            )
        },
    )?;
    ensure_supported_published_bundle_schema(&manifest_value)?;
    let manifest = serde_json::from_value::<PublishedBundleManifest>(manifest_value.clone())
        .map_err(|error| {
            format!(
                "conversion_revalidation_invalid_bundle: The published manifest has an unsupported shape: {error}"
            )
        })?;
    let expected_source = published_source_manifest(source_path, analysis)?;
    let expected_converter = published_converter_manifest();
    if manifest.schema_version != PUBLISHED_BUNDLE_SCHEMA_VERSION
        || manifest.adapter_id != "u1-planner/mixed-native"
        || manifest.converter != expected_converter
        || manifest.source != expected_source
        || manifest.plan_fingerprint != plan_fingerprint
    {
        return Err(
            "conversion_revalidation_invalid_bundle: The published manifest does not identify the current source, plan, converter, and schema."
                .into(),
        );
    }
    let canonical_partial_approval = if manifest.excluded_source_units.is_empty() {
        None
    } else {
        Some(PartialConversionApproval {
            excluded_source_units: manifest.excluded_source_units.clone(),
        })
    };
    if manifest.partial_conversion_approval != canonical_partial_approval {
        return Err(
            "conversion_revalidation_invalid_bundle: The partial-conversion approval does not exactly match the published exclusions."
                .into(),
        );
    }
    validate_source_dialect_for_adapter(analysis, experimental_dialect_approval)?;
    let (prepared, _targets) = prepare_required_targets(
        source_path,
        analysis,
        input,
        result,
        plan_fingerprint,
        canonical_partial_approval.as_ref(),
        experimental_dialect_approval,
        None,
    )?;
    if prepared.excluded_source_units != manifest.excluded_source_units
        || output_directory.file_name().and_then(|name| name.to_str())
            != Some(prepared.bundle_directory_name.as_str())
    {
        return Err(
            "conversion_revalidation_invalid_bundle: The output bundle name or exclusions differ from the current canonical preflight."
                .into(),
        );
    }
    let warnings_acknowledged =
        ensure_exact_warning_acknowledgement(&manifest.warnings, &manifest.acknowledged_warnings)?;
    if manifest.warnings != prepared.warnings
        || manifest.warnings_acknowledged != warnings_acknowledged
    {
        return Err(
            "conversion_revalidation_invalid_bundle: Published warning evidence differs from the current canonical preflight."
                .into(),
        );
    }
    let artifacts = recover_published_artifacts(output_directory, &manifest.artifacts, &prepared)?;
    validate_bundle_source_partition(input, &artifacts, &manifest.excluded_source_units)?;
    validate_staged_bundle_tree(output_directory, &artifacts, true)?;

    ensure_json_contract(
        &manifest.target_adapters,
        &target_adapters_manifest(&prepared),
        "target adapter list",
    )?;
    ensure_json_contract(
        &manifest.inventory_snapshot,
        &input.inventory,
        "inventory snapshot",
    )?;
    ensure_json_contract(
        &manifest.source_to_target_map,
        &source_to_target_manifest(input, result, &artifacts)?,
        "source-to-target map",
    )?;
    if manifest.excluded_scopes != excluded_scope_ids(&manifest.excluded_source_units) {
        return Err(
            "conversion_revalidation_invalid_bundle: The published excluded-scope list is not canonical."
                .into(),
        );
    }
    ensure_json_contract(
        &manifest.user_approvals,
        &user_approvals_manifest(input),
        "user approval evidence",
    )?;
    ensure_json_contract(
        &manifest.loadout_timeline,
        &result.batches,
        "loadout timeline",
    )?;
    let expected_validation_status = if manifest.excluded_source_units.is_empty() {
        "Passed"
    } else {
        "PassedWithApprovedExclusions"
    };
    if manifest.validation_status != expected_validation_status {
        return Err(
            "conversion_revalidation_invalid_bundle: The published validation status is inconsistent with its exclusions."
                .into(),
        );
    }

    let conversion_plan_path = output_directory.join(PUBLISHED_CONVERSION_PLAN_FILE_NAME);
    let (conversion_plan_bytes, conversion_plan_identity) = bounded_regular_file(
        &conversion_plan_path,
        MAX_PUBLISHED_CONVERSION_PLAN_BYTES,
        "published conversion plan",
    )?;
    let actual_conversion_plan = serde_json::from_slice::<serde_json::Value>(&conversion_plan_bytes)
        .map_err(|error| {
            format!(
                "conversion_revalidation_invalid_bundle: The published conversion plan is invalid JSON: {error}"
            )
        })?;
    let expected_conversion_plan = json_wire_value(
        &serde_json::json!({
            "schemaVersion": PUBLISHED_BUNDLE_SCHEMA_VERSION,
            "converter": published_converter_manifest(),
            "source": published_source_manifest(source_path, analysis)?,
            "planFingerprint": plan_fingerprint,
            "planningInput": input,
            "planningResult": result,
            "excludedSourceUnits": manifest.excluded_source_units,
            "warningsAcknowledged": warnings_acknowledged,
            "acknowledgedWarnings": manifest.acknowledged_warnings,
        }),
        "canonical conversion plan",
    )?;
    if !json_values_equivalent(&actual_conversion_plan, &expected_conversion_plan) {
        return Err(
            "conversion_revalidation_invalid_bundle: The published conversion plan differs from the current canonical plan."
                .into(),
        );
    }

    let report_path = output_directory.join(PUBLISHED_CONVERSION_REPORT_FILE_NAME);
    let (report_bytes, report_identity) = bounded_regular_file(
        &report_path,
        MAX_PUBLISHED_REPORT_BYTES,
        "published conversion report",
    )?;
    let expected_report = render_conversion_report(&manifest_value)?;
    if report_bytes != expected_report {
        return Err(
            "conversion_revalidation_invalid_bundle: The published conversion report does not match the verified manifest."
                .into(),
        );
    }

    let mut expected_checksums = artifacts
        .iter()
        .map(|artifact| format!("{}  {}", artifact.sha256, artifact.relative_path))
        .collect::<Vec<_>>();
    expected_checksums.push(format!(
        "{}  {PUBLISHED_MANIFEST_FILE_NAME}",
        manifest_identity.sha256
    ));
    expected_checksums.push(format!(
        "{}  {PUBLISHED_CONVERSION_PLAN_FILE_NAME}",
        conversion_plan_identity.sha256
    ));
    expected_checksums.push(format!(
        "{}  {PUBLISHED_CONVERSION_REPORT_FILE_NAME}",
        report_identity.sha256
    ));
    expected_checksums.sort();
    let expected_checksums = format!("{}\n", expected_checksums.join("\n"));
    let checksums_path = output_directory.join(PUBLISHED_CHECKSUMS_FILE_NAME);
    let (checksums_bytes, checksums_identity) = bounded_regular_file(
        &checksums_path,
        MAX_PUBLISHED_CHECKSUM_BYTES,
        "published checksum list",
    )?;
    if checksums_bytes != expected_checksums.as_bytes() {
        return Err(
            "conversion_revalidation_invalid_bundle: The published checksum list does not exactly match the verified bundle."
                .into(),
        );
    }
    let expected_receipt = build_publication_receipt(
        source_path,
        analysis,
        plan_fingerprint,
        &prepared,
        output_directory,
        output_directory,
        vec![
            PublicationReceiptFile {
                relative_path: PUBLISHED_MANIFEST_FILE_NAME.into(),
                byte_size: manifest_identity.byte_size,
                sha256: manifest_identity.sha256.clone(),
            },
            PublicationReceiptFile {
                relative_path: PUBLISHED_CONVERSION_PLAN_FILE_NAME.into(),
                byte_size: conversion_plan_identity.byte_size,
                sha256: conversion_plan_identity.sha256.clone(),
            },
            PublicationReceiptFile {
                relative_path: PUBLISHED_CONVERSION_REPORT_FILE_NAME.into(),
                byte_size: report_identity.byte_size,
                sha256: report_identity.sha256.clone(),
            },
            PublicationReceiptFile {
                relative_path: PUBLISHED_CHECKSUMS_FILE_NAME.into(),
                byte_size: checksums_identity.byte_size,
                sha256: checksums_identity.sha256.clone(),
            },
        ],
        &artifacts,
    )?;
    if trusted_receipt != expected_receipt {
        return Err(format!(
            "conversion_revalidation_receipt_mismatch: The published bundle differs from its trusted publication receipt and is untrusted. {UNTRUSTED_BUNDLE_RETRY_GUIDANCE}"
        ));
    }
    validate_cached_identity_cancellable(source_path, analysis, &mut || false)?;
    for artifact in &artifacts {
        validate_regular_file_identity(
            &artifact.path,
            artifact.byte_size,
            &artifact.sha256,
            &format!("artifact {}", artifact.file_name),
        )?;
    }
    validate_cached_identity_cancellable(source_path, analysis, &mut || false)?;
    let next_output_registry = revalidated_published_output_registry(&artifacts, output_directory)?;
    let metadata_identities = [
        manifest_identity,
        conversion_plan_identity,
        report_identity,
        checksums_identity,
    ];
    restore_revalidated_output_registry_if_current(
        operation_generation,
        expected_generation,
        published_outputs,
        next_output_registry,
        source_path,
        analysis,
        output_directory,
        &artifacts,
        &metadata_identities,
    )?;
    Ok(NativeConversionResult {
        adapter_id: manifest.adapter_id,
        output_directory: output_directory.to_path_buf(),
        manifest_path,
        report_path,
        artifacts,
        excluded_source_units: manifest.excluded_source_units,
        warnings_acknowledged,
        warnings: manifest.warnings,
    })
}

#[tauri::command]
async fn convert_project(
    preparation_token: String,
    destination_directory: String,
    acknowledged_warnings: Vec<String>,
    app: tauri::AppHandle,
    cache: tauri::State<'_, ProjectCache>,
) -> Result<NativeConversionResult, String> {
    let destination = Path::new(&destination_directory)
        .canonicalize()
        .map_err(|error| format!("Failed to resolve the destination directory: {error}"))?;
    if !destination.is_dir() {
        return Err("Select an existing destination directory.".into());
    }
    let prepared = inspect_prepared_conversion(&cache.prepared, &preparation_token)?;
    ensure_exact_warning_acknowledgement(&prepared.preparation.warnings, &acknowledged_warnings)?;
    let (analysis, input, result, source_path) = {
        let cached = cache
            .analyzed
            .lock()
            .map_err(|_| "project cache is unavailable".to_owned())?;
        let cached = cached
            .as_ref()
            .ok_or_else(|| "The analyzed project is no longer cached.".to_owned())?;
        if cached.canonical_path != prepared.canonical_path
            || cached.analysis.input.sha256 != prepared.source_sha256
            || cached.plan_fingerprint != prepared.plan_fingerprint
            || cached.latest_plan.invalidated
            || (!cached.latest_plan.is_ready()
                && prepared.preparation.excluded_source_units.is_empty())
            || prepared.preparation.plan_fingerprint != cached.plan_fingerprint
        {
            return Err(
                "The project or plan changed after preflight. Prepare conversion again.".into(),
            );
        }
        (
            cached.analysis.clone(),
            cached.planning_input.clone(),
            cached.planning_result.clone(),
            cached.canonical_path.clone(),
        )
    };
    let conversion_id = prepared.control.conversion_id().to_owned();
    app.emit(
        "conversion-progress",
        ConversionProgressEvent {
            conversion_id: conversion_id.clone(),
            stage: "verifying_source",
            message: "Verifying source and canonical plan…".into(),
        },
    )
    .map_err(|error| format!("Failed to emit conversion progress: {error}"))?;
    ensure_output_path_absent(&destination.join(&prepared.preparation.bundle_directory_name))?;
    let preflight_source = source_path.clone();
    let preflight_analysis = analysis.clone();
    let preflight_input = input.clone();
    let preflight_result = result.clone();
    let preflight_fingerprint = prepared.plan_fingerprint.clone();
    let preflight_control = prepared.control.clone();
    let preflight_partial_approval = prepared.partial_conversion_approval.clone();
    let preflight_experimental_approval = prepared.experimental_dialect_approval.clone();
    let current_preflight = tauri::async_runtime::spawn_blocking(move || {
        prepare_required_targets(
            &preflight_source,
            &preflight_analysis,
            &preflight_input,
            &preflight_result,
            &preflight_fingerprint,
            preflight_partial_approval.as_ref(),
            preflight_experimental_approval.as_ref(),
            Some(&preflight_control),
        )
    })
    .await
    .map_err(|error| format!("conversion preflight worker failed: {error}"))?;
    let (current_presentation, current_targets) = match current_preflight {
        Ok(current) => current,
        Err(error)
            if prepared.control.state() == U1DirectConversionState::Cancelled
                || error.starts_with("conversion_cancelled:") =>
        {
            let _ = discard_prepared_conversion_if_matches(
                &cache.prepared,
                &preparation_token,
                &conversion_id,
            );
            let _ = app.emit(
                "conversion-progress",
                ConversionProgressEvent {
                    conversion_id,
                    stage: "cancelled",
                    message: "Conversion cancelled before publication.".into(),
                },
            );
            return Err(
                "conversion_cancelled: Conversion was cancelled before publication.".into(),
            );
        }
        Err(error) => return Err(error),
    };
    ensure_prepared_contract_unchanged(
        &prepared.preparation,
        &prepared.targets,
        &current_presentation,
        &current_targets,
    )?;
    if prepared.control.state() == U1DirectConversionState::Cancelled {
        let _ = discard_prepared_conversion_if_matches(
            &cache.prepared,
            &preparation_token,
            &conversion_id,
        );
        let _ = app.emit(
            "conversion-progress",
            ConversionProgressEvent {
                conversion_id,
                stage: "cancelled",
                message: "Conversion cancelled before publication.".into(),
            },
        );
        return Err("conversion_cancelled: Conversion was cancelled before publication.".into());
    }
    // Consume the one-shot capability only after every retryable cache,
    // installation, destination, and UI-notification precondition has passed.
    // A concurrent replan replaces the token and therefore still fails closed.
    let recovery_record = prepared
        .control
        .recovery_record(&destination)
        .map_err(|error| format!("Failed to prepare conversion recovery: {error}"))?;
    let publication_receipt_registry = publication_receipt_registry_for_app(&app)?;
    ensure_private_receipt_registry(&publication_receipt_registry)?;
    let recovery_registry = recovery_registry_for_app(&app)?;
    let recovery_path = write_recovery_record(&recovery_registry, &recovery_record)?;
    {
        let mut active = match cache.active_conversions.lock() {
            Ok(active) => active,
            Err(_) => {
                let _ = remove_recovery_record(&recovery_registry, &recovery_path);
                return Err("active conversion registry is unavailable".into());
            }
        };
        if active
            .insert(conversion_id.clone(), prepared.control.clone())
            .is_some()
        {
            let _ = remove_recovery_record(&recovery_registry, &recovery_path);
            return Err("A conversion with this ID is already running.".into());
        }
    }
    if let Err(error) = take_prepared_conversion(&cache.prepared, &preparation_token) {
        if let Ok(mut active) = cache.active_conversions.lock() {
            active.remove(&conversion_id);
        }
        let _ = remove_recovery_record(&recovery_registry, &recovery_path);
        return Err(error);
    }
    let progress_app = app.clone();
    let worker_conversion_id = conversion_id.clone();
    let control = prepared.control.clone();
    let targets = current_targets;
    let presentation = current_presentation;
    let canonical_fingerprint = prepared.plan_fingerprint.clone();
    let operation_generation = Arc::clone(&cache.operation_generation);
    let published_outputs = Arc::clone(&cache.published_outputs);
    let expected_generation = prepared.cache_generation;
    let worker_result = tauri::async_runtime::spawn_blocking(move || {
        let _ = progress_app.emit(
            "conversion-progress",
            ConversionProgressEvent {
                conversion_id: worker_conversion_id,
                stage: "writing_projects",
                message: "Building and validating native U1 and A1 mini projects…".into(),
            },
        );
        perform_native_conversion(
            &source_path,
            &analysis,
            &input,
            &result,
            &canonical_fingerprint,
            &presentation,
            &targets,
            &destination,
            &publication_receipt_registry,
            &acknowledged_warnings,
            &control,
            &operation_generation,
            &published_outputs,
            expected_generation,
        )
    })
    .await;
    if let Ok(mut active) = cache.active_conversions.lock() {
        active.remove(&conversion_id);
    }

    let cleanup_error = finalize_u1_direct_staging(&recovery_record).err();
    let registry_error = if cleanup_error.is_none() {
        remove_recovery_record(&recovery_registry, &recovery_path).err()
    } else {
        None
    };
    if let Some(error) = &cleanup_error {
        eprintln!(
            "Conversion maintenance notice: temporary staging cleanup will be retried on the next app startup: {error}"
        );
    }
    if let Some(error) = &registry_error {
        eprintln!(
            "Conversion maintenance notice: the completed recovery record will be cleared on the next app startup: {error}"
        );
    }

    let result = worker_result
        .map_err(|error| format!("conversion_worker_failed: Conversion worker failed: {error}"))?;
    match result {
        Ok(result) => {
            let _ = app.emit(
                "conversion-progress",
                ConversionProgressEvent {
                    conversion_id,
                    stage: "complete",
                    message: "Conversion bundle published and validated.".into(),
                },
            );
            Ok(result)
        }
        Err(error)
            if prepared.control.state() == U1DirectConversionState::Cancelled
                || error.starts_with("conversion_cancelled:") =>
        {
            let _ = app.emit(
                "conversion-progress",
                ConversionProgressEvent {
                    conversion_id,
                    stage: "cancelled",
                    message: "Conversion cancelled before publication.".into(),
                },
            );
            Err("conversion_cancelled: Conversion was cancelled before publication.".into())
        }
        Err(error) if error.starts_with("conversion_published_without_recovery:") => {
            let _ = app.emit(
                "conversion-progress",
                ConversionProgressEvent {
                    conversion_id,
                    stage: "published_without_recovery",
                    message: "The bundle was published, but trusted recovery registration failed; quick actions remain disabled."
                        .into(),
                },
            );
            Err(error)
        }
        Err(error) => {
            let _ = app.emit(
                "conversion-progress",
                ConversionProgressEvent {
                    conversion_id,
                    stage: "failed",
                    message: "Conversion failed before publication completed.".into(),
                },
            );
            Err(format!("conversion_failed: {error}"))
        }
    }
}

#[tauri::command]
async fn revalidate_published_conversion(
    source_path: String,
    source_sha256: String,
    plan_fingerprint: String,
    output_directory: String,
    experimental_dialect_approval: Option<ExperimentalDialectApproval>,
    app: tauri::AppHandle,
    cache: tauri::State<'_, ProjectCache>,
) -> Result<NativeConversionResult, String> {
    let canonical_source = canonical_project_path(&source_path)?;
    let requested_output = PathBuf::from(output_directory.trim());
    if !requested_output.is_absolute() {
        return Err(
            "conversion_revalidation_invalid_bundle: The published output directory must be an absolute backend-authored path."
                .into(),
        );
    }
    let requested_metadata = fs::symlink_metadata(&requested_output).map_err(|error| {
        format!(
            "conversion_revalidation_invalid_bundle: Failed to inspect the published output directory: {error}"
        )
    })?;
    if requested_metadata.file_type().is_symlink() || !requested_metadata.file_type().is_dir() {
        return Err(
            "conversion_revalidation_invalid_bundle: The published output must be a real directory."
                .into(),
        );
    }
    let canonical_output = requested_output.canonicalize().map_err(|error| {
        format!(
            "conversion_revalidation_invalid_bundle: Failed to resolve the published output directory: {error}"
        )
    })?;
    if canonical_output != requested_output {
        return Err(
            "conversion_revalidation_invalid_bundle: The published output directory is not a canonical backend-authored path."
                .into(),
        );
    }
    let (expected_generation, analysis, input, result) = {
        let generation = cache
            .operation_generation
            .lock()
            .map_err(|_| "project operation generation is unavailable".to_owned())?;
        let cached_guard = cache
            .analyzed
            .lock()
            .map_err(|_| "project cache is unavailable".to_owned())?;
        let cached = cached_guard.as_ref().ok_or_else(|| {
            "Analyze and validate the source project before restoring a published conversion."
                .to_owned()
        })?;
        if cached.canonical_path != canonical_source {
            return Err(
                "conversion_revalidation_stale: The requested source does not match the analyzed project."
                    .into(),
            );
        }
        if source_sha256.trim_start_matches("sha256:") != cached.analysis.input.sha256 {
            return Err(
                "conversion_revalidation_stale: The requested source hash does not match the analyzed project."
                    .into(),
            );
        }
        if plan_fingerprint != cached.plan_fingerprint || cached.latest_plan.invalidated {
            return Err(
                "conversion_revalidation_stale: The requested plan fingerprint is stale or invalidated."
                    .into(),
            );
        }
        (
            *generation,
            cached.analysis.clone(),
            cached.planning_input.clone(),
            cached.planning_result.clone(),
        )
    };
    let operation_generation = Arc::clone(&cache.operation_generation);
    let published_outputs = Arc::clone(&cache.published_outputs);
    let publication_receipt_registry = publication_receipt_registry_for_app(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        revalidate_published_bundle(
            &canonical_source,
            &analysis,
            &input,
            &result,
            &plan_fingerprint,
            &canonical_output,
            &publication_receipt_registry,
            experimental_dialect_approval.as_ref(),
            &operation_generation,
            &published_outputs,
            expected_generation,
        )
    })
    .await
    .map_err(|error| {
        format!(
            "conversion_revalidation_worker_failed: Published conversion validation worker failed: {error}"
        )
    })?
}

#[tauri::command]
fn cancel_conversion(
    conversion_id: String,
    cache: tauri::State<'_, ProjectCache>,
) -> Result<CancelConversionResponse, String> {
    cancel_prepared_or_active_conversion(&conversion_id, &cache.prepared, &cache.active_conversions)
}

#[tauri::command]
async fn open_output_in_slicer(
    path: String,
    adapter_id: String,
    cache: tauri::State<'_, ProjectCache>,
) -> Result<(), String> {
    let output = resolve_registered_output(&cache.published_outputs, &path, &adapter_id)?;
    tauri::async_runtime::spawn_blocking(move || launch_registered_output(&output))
        .await
        .map_err(|error| format!("output_action_worker_failed: Output action failed: {error}"))?
}

#[tauri::command]
async fn show_output_in_finder(
    path: String,
    adapter_id: String,
    cache: tauri::State<'_, ProjectCache>,
) -> Result<(), String> {
    let output = resolve_registered_output(&cache.published_outputs, &path, &adapter_id)?;
    tauri::async_runtime::spawn_blocking(move || reveal_registered_output(&output))
        .await
        .map_err(|error| format!("output_action_worker_failed: Output action failed: {error}"))?
}

fn cancel_prepared_or_active_conversion(
    conversion_id: &str,
    prepared_conversion: &Mutex<Option<CachedPreparation>>,
    active_conversions: &Mutex<HashMap<String, MixedConversionControl>>,
) -> Result<CancelConversionResponse, String> {
    let conversion_id = canonical_recovery_record_id(conversion_id)?;
    let prepared_control = prepared_conversion
        .lock()
        .map_err(|_| "conversion preparation cache is unavailable".to_owned())?
        .as_ref()
        .filter(|prepared| prepared.control.conversion_id() == conversion_id)
        .map(|prepared| prepared.control.clone());
    if let Some(control) = prepared_control {
        let accepted = control.cancel();
        return Ok(CancelConversionResponse {
            conversion_id,
            accepted,
            state: Some(control.state()),
        });
    }
    cancel_active_conversion(&conversion_id, active_conversions)
}

fn cancel_active_conversion(
    conversion_id: &str,
    active_conversions: &Mutex<HashMap<String, MixedConversionControl>>,
) -> Result<CancelConversionResponse, String> {
    let conversion_id = canonical_recovery_record_id(conversion_id)?;
    let active = active_conversions
        .lock()
        .map_err(|_| "active conversion registry is unavailable".to_owned())?;
    let Some(control) = active.get(&conversion_id) else {
        return Ok(CancelConversionResponse {
            conversion_id,
            accepted: false,
            state: None,
        });
    };
    let accepted = control.cancel();
    Ok(CancelConversionResponse {
        conversion_id,
        accepted,
        state: Some(control.state()),
    })
}

fn take_prepared_conversion(
    cache: &Mutex<Option<CachedPreparation>>,
    preparation_token: &str,
) -> Result<CachedPreparation, String> {
    let mut prepared = cache
        .lock()
        .map_err(|_| "conversion preparation cache is unavailable".to_owned())?;
    let candidate = prepared
        .as_ref()
        .ok_or_else(|| "Conversion preparation expired. Open preflight again.".to_owned())?;
    if candidate.token != preparation_token {
        return Err("Conversion preparation token is invalid or stale.".into());
    }
    if candidate.created_at.elapsed() > CONVERSION_PREPARATION_TTL {
        return Err("Conversion preparation expired. Open preflight again.".into());
    }
    prepared
        .take()
        .ok_or_else(|| "Conversion preparation expired. Open preflight again.".to_owned())
}

fn discard_prepared_conversion_if_matches(
    cache: &Mutex<Option<CachedPreparation>>,
    preparation_token: &str,
    conversion_id: &str,
) -> Result<bool, String> {
    let mut prepared = cache
        .lock()
        .map_err(|_| "conversion preparation cache is unavailable".to_owned())?;
    let matches = prepared.as_ref().is_some_and(|candidate| {
        candidate.token == preparation_token && candidate.control.conversion_id() == conversion_id
    });
    if matches {
        prepared.take();
    }
    Ok(matches)
}

fn inspect_prepared_conversion(
    cache: &Mutex<Option<CachedPreparation>>,
    preparation_token: &str,
) -> Result<CachedPreparation, String> {
    let prepared = cache
        .lock()
        .map_err(|_| "conversion preparation cache is unavailable".to_owned())?;
    let candidate = prepared
        .as_ref()
        .ok_or_else(|| "Conversion preparation expired. Open preflight again.".to_owned())?;
    if candidate.token != preparation_token {
        return Err("Conversion preparation token is invalid or stale.".into());
    }
    if candidate.created_at.elapsed() > CONVERSION_PREPARATION_TTL {
        return Err("Conversion preparation expired. Open preflight again.".into());
    }
    Ok(candidate.clone())
}

fn canonical_project_path(source_path: &str) -> Result<PathBuf, String> {
    let path = Path::new(source_path);
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("3mf"))
    {
        return Err("Select a .3mf project file.".to_owned());
    }
    path.canonicalize()
        .map_err(|error| format!("Failed to resolve the selected 3MF: {error}"))
}

fn validate_cached_identity(path: &Path, analysis: &ProjectAnalysis) -> Result<(), String> {
    validate_cached_identity_cancellable(path, analysis, &mut || false)
}

fn ensure_preflight_active<C>(is_cancelled: &mut C) -> Result<(), String>
where
    C: FnMut() -> bool + ?Sized,
{
    if is_cancelled() {
        Err("conversion_cancelled: Conversion was cancelled before publication.".into())
    } else {
        Ok(())
    }
}

fn validate_cached_identity_cancellable<C>(
    path: &Path,
    analysis: &ProjectAnalysis,
    is_cancelled: &mut C,
) -> Result<(), String>
where
    C: FnMut() -> bool + ?Sized,
{
    validate_input_identity_cancellable(path, &analysis.input, is_cancelled)
}

fn validate_input_identity_cancellable<C>(
    path: &Path,
    expected: &InputIdentity,
    is_cancelled: &mut C,
) -> Result<(), String>
where
    C: FnMut() -> bool + ?Sized,
{
    let (byte_size, sha256) = hash_file_cancellable(path, is_cancelled)?;
    if byte_size != expected.byte_size || sha256 != expected.sha256 {
        return Err(
            "The selected 3MF changed after analysis. Analyze it again before continuing."
                .to_owned(),
        );
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<(u64, String), String> {
    hash_file_cancellable(path, &mut || false)
}

fn hash_file_cancellable<C>(path: &Path, is_cancelled: &mut C) -> Result<(u64, String), String>
where
    C: FnMut() -> bool + ?Sized,
{
    ensure_preflight_active(is_cancelled)?;
    let path_metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Failed to inspect the file for verification: {error}"))?;
    if path_metadata.file_type().is_symlink() || !path_metadata.file_type().is_file() {
        return Err("The file selected for verification is not a regular file.".into());
    }
    let mut open_options = fs::OpenOptions::new();
    open_options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open_options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = open_options
        .open(path)
        .map_err(|error| format!("Failed to open the file for verification: {error}"))?;
    let opened_metadata = file
        .metadata()
        .map_err(|error| format!("Failed to inspect the opened verification file: {error}"))?;
    if !opened_metadata.file_type().is_file()
        || !same_file_identity(&path_metadata, &opened_metadata)
    {
        return Err("The file changed while it was opened for verification.".into());
    }
    let mut hasher = Sha256::new();
    let mut byte_size = 0_u64;
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        ensure_preflight_active(is_cancelled)?;
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("Failed to verify the selected 3MF: {error}"))?;
        if read == 0 {
            break;
        }
        byte_size = byte_size
            .checked_add(read as u64)
            .ok_or_else(|| "The selected 3MF size overflowed during verification.".to_owned())?;
        hasher.update(&buffer[..read]);
    }
    ensure_preflight_active(is_cancelled)?;
    let final_metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Failed to re-inspect the verified file: {error}"))?;
    if final_metadata.file_type().is_symlink()
        || !final_metadata.file_type().is_file()
        || !same_file_identity(&opened_metadata, &final_metadata)
        || final_metadata.len() != byte_size
    {
        return Err("The file changed during verification.".into());
    }
    Ok((byte_size, format!("{:x}", hasher.finalize())))
}

#[tauri::command]
async fn export_plan(
    destination_path: String,
    contents: String,
    source_path: String,
    source_hash: String,
    cache: tauri::State<'_, ProjectCache>,
) -> Result<(), String> {
    let canonical_path = canonical_project_path(&source_path)?;
    let (analysis, authoritative_plan) = {
        let cached = cache
            .analyzed
            .lock()
            .map_err(|_| "project cache is unavailable".to_owned())?;
        let cached = cached
            .as_ref()
            .ok_or_else(|| "Analyze the selected 3MF before exporting its plan.".to_owned())?;
        if cached.canonical_path != canonical_path {
            return Err("The export source does not match the analyzed project.".to_owned());
        }
        (cached.analysis.clone(), cached.latest_plan.clone())
    };
    let expected_hash = format!("sha256:{}", analysis.input.sha256);
    if source_hash != expected_hash {
        return Err("The export plan hash does not match the analyzed project.".to_owned());
    }
    tauri::async_runtime::spawn_blocking(move || {
        validate_cached_identity(&canonical_path, &analysis)?;
        write_plan_json(
            Path::new(&destination_path),
            &contents,
            &expected_hash,
            &authoritative_plan,
        )
    })
    .await
    .map_err(|error| format!("plan export worker failed: {error}"))?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportPlanEnvelope {
    summary: ExportPlanSummary,
    plates: Vec<serde_json::Value>,
    batches: Vec<serde_json::Value>,
    t4_swap_count: u32,
    a1_spool_change_count: u32,
    plan_ready: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportPlanSummary {
    source_hash: String,
}

fn write_plan_json(
    destination: &Path,
    contents: &str,
    expected_hash: &str,
    authoritative_plan: &CachedPlan,
) -> Result<(), String> {
    const MAX_PLAN_BYTES: usize = 16 * 1024 * 1024;
    if contents.len() > MAX_PLAN_BYTES {
        return Err(format!(
            "The print plan exceeds the {MAX_PLAN_BYTES} byte export limit."
        ));
    }
    if !destination
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
    {
        return Err("Print plans must be exported with a .json extension.".to_owned());
    }
    let submitted_value = serde_json::from_str::<serde_json::Value>(contents)
        .map_err(|error| format!("The print plan is not valid JSON: {error}"))?;
    if !json_values_equivalent(&submitted_value, &authoritative_plan.value) {
        return Err(
            "The print plan differs from the latest backend-validated plan. Validate the current choices again before exporting."
                .to_owned(),
        );
    }
    let plan = serde_json::from_value::<ExportPlanEnvelope>(submitted_value)
        .map_err(|error| format!("The print plan does not have the required shape: {error}"))?;
    if plan.summary.source_hash != expected_hash {
        return Err("The print plan source hash does not match the analyzed project.".to_owned());
    }
    if !plan.plan_ready || !authoritative_plan.is_ready() {
        return Err(
            "The print plan still has blocking errors or omitted units and cannot be exported."
                .to_owned(),
        );
    }
    let _plan_shape = (
        plan.plates.len(),
        plan.batches.len(),
        plan.t4_swap_count,
        plan.a1_spool_change_count,
    );
    let parent = destination
        .parent()
        .filter(|parent| parent.is_dir())
        .ok_or_else(|| "The print plan destination directory does not exist.".to_owned())?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("Failed to create the temporary print plan: {error}"))?;
    temporary
        .write_all(authoritative_plan.contents.as_bytes())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| format!("Failed to write the temporary print plan: {error}"))?;
    temporary
        .persist(destination)
        .map_err(|error| format!("Failed to publish the print plan atomically: {error}"))?;
    Ok(())
}

fn json_values_equivalent(left: &serde_json::Value, right: &serde_json::Value) -> bool {
    use serde_json::Value;

    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(left), Value::Bool(right)) => left == right,
        (Value::Number(left), Value::Number(right)) => {
            normalized_json_number(left) == normalized_json_number(right)
        }
        (Value::String(left), Value::String(right)) => left == right,
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| json_values_equivalent(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, left_value)| {
                    right
                        .get(key)
                        .is_some_and(|right_value| json_values_equivalent(left_value, right_value))
                })
        }
        _ => false,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct NormalizedJsonNumber {
    negative: bool,
    digits: String,
    exponent: i64,
}

fn normalized_json_number(number: &serde_json::Number) -> NormalizedJsonNumber {
    let representation = number.to_string();
    let (negative, unsigned) = representation
        .strip_prefix('-')
        .map_or((false, representation.as_str()), |value| (true, value));
    let (mantissa, explicit_exponent) =
        unsigned
            .split_once(['e', 'E'])
            .map_or((unsigned, 0_i64), |(mantissa, exponent)| {
                (
                    mantissa,
                    exponent
                        .parse::<i64>()
                        .expect("serde_json emits a valid numeric exponent"),
                )
            });
    let (integer, fractional) = mantissa
        .split_once('.')
        .map_or((mantissa, ""), |(integer, fractional)| {
            (integer, fractional)
        });
    let mut digits = format!("{integer}{fractional}");
    let mut exponent = explicit_exponent - fractional.len() as i64;
    let first_nonzero = digits.find(|character| character != '0');
    let Some(first_nonzero) = first_nonzero else {
        return NormalizedJsonNumber {
            negative: false,
            digits: "0".to_owned(),
            exponent: 0,
        };
    };
    digits.drain(..first_nonzero);
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
        exponent += 1;
    }
    NormalizedJsonNumber {
        negative,
        digits,
        exponent,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(ProjectCache::default())
        .setup(|app| {
            match recovery_registry_for_app(app.handle())
                .and_then(|directory| cleanup_recovery_registry(&directory))
            {
                Ok(report) if report.failed_records > 0 => {
                    eprintln!(
                        "Some abandoned conversion staging could not be cleaned and will be retried on the next startup."
                    );
                }
                Err(_) => {
                    eprintln!(
                        "Conversion staging recovery could not run and will be retried on the next startup."
                    );
                }
                _ => {}
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            analyze_project,
            cancel_analysis,
            replan_project,
            load_filament_library,
            save_filament_library,
            load_cmyx_calibration_library,
            upsert_cmyx_calibration_measurement,
            delete_cmyx_calibration_record,
            set_cmyx_calibration_geometry_context,
            build_cmyx_calibration_project,
            validate_cmyx_calibration_project,
            export_plan,
            inspect_conversion_capabilities,
            prepare_conversion,
            convert_project,
            revalidate_published_conversion,
            cancel_conversion,
            open_output_in_slicer,
            show_output_in_finder
        ])
        .run(tauri::generate_context!())
        .expect("failed to run the U1 3MF Color Planner desktop app");
}

#[cfg(test)]
mod tests {
    use super::*;
    use u1_color_engine::{GeometryClass, RecipeComponent, RecipeMode, SampleOrientation};

    fn published_fixture(root: &Path, directory: &str, file_name: &str) -> NativePublishedArtifact {
        NativePublishedArtifact {
            adapter_id: "fixture-adapter".to_owned(),
            target: "a1_mini_mono".to_owned(),
            printer: "Bambu Lab A1 mini".to_owned(),
            strategy: "A1 Mono".to_owned(),
            slicer: "Bambu Studio".to_owned(),
            batch_id: "job-001".to_owned(),
            file_name: file_name.to_owned(),
            relative_path: format!("{directory}/{file_name}"),
            path: root.join(directory).join(file_name),
            byte_size: 42,
            sha256: "fixture-sha".to_owned(),
            plate_count: 1,
            target_plate_ids: vec!["plate-1".to_owned()],
            source_unit_ids: vec!["source-unit-1".to_owned()],
            loadout: Vec::new(),
            setup_actions: vec!["Load fixture spool".to_owned()],
            validation_status: "Passed".to_owned(),
            adapter_evidence: serde_json::json!({"fixture": true}),
        }
    }

    fn prepared_fixture(
        artifact: &NativePublishedArtifact,
        adapter_evidence: serde_json::Value,
    ) -> NativePreparedArtifact {
        NativePreparedArtifact {
            adapter_id: artifact.adapter_id.clone(),
            target: artifact.target.clone(),
            printer: artifact.printer.clone(),
            strategy: artifact.strategy.clone(),
            slicer: artifact.slicer.clone(),
            batch_id: artifact.batch_id.clone(),
            file_name: artifact.file_name.clone(),
            target_plate_ids: artifact.target_plate_ids.clone(),
            source_unit_ids: artifact.source_unit_ids.clone(),
            loadout: artifact.loadout.clone(),
            setup_actions: artifact.setup_actions.clone(),
            adapter_evidence,
        }
    }

    fn source_partition_input() -> PlanningInput {
        let unit = |id: &str, source_unit_id: &str| u1_planner::PrintableUnit {
            id: id.to_owned(),
            source_unit_id: source_unit_id.to_owned(),
            source_object_id: 1,
            source_instance_id: 0,
            source_model_path: None,
            display_name: source_unit_id.to_owned(),
            source_plate_id: Some("plate-7".to_owned()),
            requirement_ids: Vec::new(),
            bounds: u1_planner::BoundsMm::from_size(10.0, 10.0, 10.0),
            source_layer_height_mm: Some(0.2),
            printer_preference: u1_planner::PrinterPreference::Auto,
        };
        PlanningInput {
            scopes: vec![u1_planner::PrintScope {
                id: "scope-7".to_owned(),
                display_name: "Fixture".to_owned(),
                requirements: Vec::new(),
                units: vec![unit("unit-a", "source-a"), unit("unit-b", "source-b")],
                strategy: u1_planner::ScopeStrategy::Auto,
                direct_assignments: Vec::new(),
                approved_cmyx_fallbacks: Vec::new(),
                approved_material_substitutions: Vec::new(),
            }],
            inventory: Vec::new(),
            current_toolheads: u1_planner::CurrentToolheadState::default(),
            config: u1_planner::PlannerConfig::with_cmy_setup(u1_planner::CmySetup {
                cyan_spool_id: "cyan".into(),
                magenta_spool_id: "magenta".into(),
                yellow_spool_id: "yellow".into(),
                default_t4_spool_id: None,
            }),
        }
    }

    fn excluded_source_b() -> ExcludedSourceUnit {
        ExcludedSourceUnit {
            scope_id: "scope-7".to_owned(),
            planning_unit_id: "unit-b".to_owned(),
            source_plate_id: Some(7),
            source_unit_id: "source-b".to_owned(),
            reason: "Fixture unit is intentionally blocked.".to_owned(),
            error_identity: format!("sha256:{}", "a".repeat(64)),
        }
    }

    #[test]
    fn bundle_partition_requires_exact_scheduled_plus_excluded_coverage() {
        let input = source_partition_input();
        let mut artifact = published_fixture(Path::new("/staging"), "a1-mini", "job.3mf");
        artifact.source_unit_ids = vec!["source-a".to_owned()];
        let exclusion = excluded_source_b();

        validate_bundle_source_partition(
            &input,
            std::slice::from_ref(&artifact),
            std::slice::from_ref(&exclusion),
        )
        .unwrap();

        artifact.source_unit_ids.push("source-b".to_owned());
        assert!(
            validate_bundle_source_partition(&input, &[artifact], &[exclusion])
                .unwrap_err()
                .contains("both published and excluded")
        );
    }

    #[test]
    fn bundle_partition_rejects_missing_or_modified_exclusion_evidence() {
        let input = source_partition_input();
        let mut artifact = published_fixture(Path::new("/staging"), "a1-mini", "job.3mf");
        artifact.source_unit_ids = vec!["source-a".to_owned()];
        assert!(
            validate_bundle_source_partition(&input, &[artifact.clone()], &[])
                .unwrap_err()
                .contains("missing: source-b")
        );

        let mut changed = excluded_source_b();
        changed.source_plate_id = Some(8);
        assert!(
            validate_bundle_source_partition(&input, &[artifact], &[changed])
                .unwrap_err()
                .contains("changed after approval")
        );
    }

    #[test]
    fn warning_acknowledgement_is_exact_and_runtime_changes_fail_closed() {
        let prepared = vec!["Review mapping".to_owned()];
        assert!(!ensure_exact_warning_acknowledgement(&[], &[]).unwrap());
        assert!(ensure_exact_warning_acknowledgement(&prepared, &prepared).unwrap());
        assert!(
            ensure_exact_warning_acknowledgement(&prepared, &[])
                .unwrap_err()
                .contains("does not exactly match")
        );
        assert!(
            ensure_runtime_warnings_match_preflight(
                &prepared,
                &["A different runtime warning".to_owned()],
                &prepared,
            )
            .unwrap_err()
            .starts_with("conversion_warning_contract_changed:")
        );
    }

    #[test]
    fn published_manifest_revalidation_rejects_unknown_fields() {
        let manifest = serde_json::json!({
            "schemaVersion": PUBLISHED_BUNDLE_SCHEMA_VERSION,
            "adapterId": "u1-planner/mixed-native",
            "converter": {
                "name": "U1 3MF Color Planner",
                "buildIdentity": {
                    "packageVersion": env!("CARGO_PKG_VERSION"),
                    "gitIdentity": converter_git_identity(),
                },
            },
            "source": {
                "fileName": "source.3mf",
                "byteSize": 1,
                "sha256": "source",
                "application": "bambu_studio",
                "applicationName": "Bambu Studio",
                "applicationVersion": null,
                "dialect": "bambu_studio_project",
                "dialectVersion": null,
                "dialectSupport": "supported",
            },
            "planFingerprint": "plan",
            "targetAdapters": [],
            "inventorySnapshot": [],
            "artifacts": [],
            "sourceToTargetMap": [],
            "excludedSourceUnits": [],
            "excludedScopes": [],
            "userApprovals": [],
            "partialConversionApproval": null,
            "warningsAcknowledged": false,
            "acknowledgedWarnings": [],
            "loadoutTimeline": [],
            "validationStatus": "Passed",
            "warnings": [],
            "frontendReceiptTrusted": true,
        });

        assert!(serde_json::from_value::<PublishedBundleManifest>(manifest).is_err());
    }

    #[test]
    fn published_provenance_is_authoritative_relocatable_and_build_bound() {
        let mut project = cached_project_with_plan(cached_plan(r#"{"planReady":true}"#));
        project.analysis.source.application_version = Some("01.10.02.00".to_owned());
        project.analysis.source.dialect_version = Some("BambuStudio-1.10".to_owned());

        let source = published_source_manifest(
            Path::new("/private/host/projects/Withered_Foxy.3mf"),
            &project.analysis,
        )
        .unwrap();
        let source_json = serde_json::to_value(&source).unwrap();
        assert_eq!(source.file_name, "Withered_Foxy.3mf");
        assert_eq!(source.byte_size, project.analysis.input.byte_size);
        assert_eq!(source.sha256, project.analysis.input.sha256);
        assert_eq!(source.application, SourceApplication::BambuStudio);
        assert_eq!(source.application_version.as_deref(), Some("01.10.02.00"));
        assert_eq!(source.dialect, ProjectDialect::BambuStudioProject);
        assert_eq!(source.dialect_version.as_deref(), Some("BambuStudio-1.10"));
        assert!(!source_json.to_string().contains("/private/host"));

        let converter = published_converter_manifest();
        assert_eq!(
            converter.build_identity.package_version,
            env!("CARGO_PKG_VERSION")
        );
        assert_eq!(
            converter.build_identity.git_identity,
            converter_git_identity()
        );
        assert!(!converter.build_identity.git_identity.is_empty());
    }

    #[test]
    fn recovery_recognizes_legacy_schema_and_fails_closed() {
        let legacy = serde_json::json!({ "schemaVersion": 1 });
        let error = ensure_supported_published_bundle_schema(&legacy).unwrap_err();
        assert!(error.starts_with("conversion_revalidation_legacy_bundle:"));
        assert!(error.contains("cannot be recovered safely"));

        let future = serde_json::json!({ "schemaVersion": 99 });
        assert!(
            ensure_supported_published_bundle_schema(&future)
                .unwrap_err()
                .starts_with("conversion_revalidation_unsupported_bundle:")
        );
        assert!(
            ensure_supported_published_bundle_schema(&serde_json::json!({}))
                .unwrap_err()
                .starts_with("conversion_revalidation_invalid_bundle:")
        );
    }

    #[test]
    fn mixed_manifest_artifacts_are_relocatable() {
        let artifact = published_fixture(Path::new("/private/staging"), "a1-mini", "job-001.3mf");

        let value = manifest_artifact(&artifact);
        assert_eq!(value["relativePath"], "a1-mini/job-001.3mf");
        assert_eq!(value["targetPlateIds"][0], "plate-1");
        assert_eq!(value["setupActions"][0], "Load fixture spool");
        assert_eq!(value["validationStatus"], "Passed");
        assert_eq!(value["adapterEvidence"]["fixture"], true);
        assert!(value.get("path").is_none());
        assert!(!value.to_string().contains("/private/staging"));
    }

    #[test]
    fn mixed_manifest_preserves_full_spectrum_calibration_provenance() {
        let mut artifact = published_fixture(
            Path::new("/private/staging"),
            "u1-full-spectrum",
            "batch.3mf",
        );
        artifact.target = "u1_full_spectrum".to_owned();
        artifact.adapter_evidence = serde_json::json!({
            "approvedPreflight": {
                "recipeCalibrationSampleIds": ["measured-coupon-r1"]
            },
            "publishedValidation": {
                "targetValidation": {
                    "recipeCalibrationSampleIds": ["measured-coupon-r1"]
                }
            }
        });

        let value = manifest_artifact(&artifact);
        assert_eq!(
            value["adapterEvidence"]["approvedPreflight"]["recipeCalibrationSampleIds"][0],
            "measured-coupon-r1"
        );
        assert_eq!(
            value["adapterEvidence"]["publishedValidation"]["targetValidation"]["recipeCalibrationSampleIds"]
                [0],
            "measured-coupon-r1"
        );
    }

    #[test]
    fn published_adapter_evidence_validates_both_bound_layers() {
        let mut artifact = published_fixture(
            Path::new("/private/staging"),
            "u1-full-spectrum",
            "batch.3mf",
        );
        artifact.target = "u1_full_spectrum".to_owned();
        let approved_evidence = serde_json::json!({
            "qualification": {"valid": true},
            "recipeCalibrationSampleIds": ["measured-coupon-r1"]
        });
        let approved = prepared_fixture(&artifact, approved_evidence.clone());
        artifact.adapter_evidence = serde_json::json!({
            "approvedPreflight": approved_evidence,
            "publishedValidation": {
                "contract": "u1_full_spectrum_source_plan_bound",
                "targetValidation": {"valid": true}
            }
        });
        let manifest =
            serde_json::from_value::<PublishedBundleArtifactManifest>(manifest_artifact(&artifact))
                .unwrap();

        validate_published_adapter_evidence(&manifest, &approved).unwrap();

        let mut changed_preflight = manifest.clone();
        changed_preflight.adapter_evidence["approvedPreflight"]["qualification"]["valid"] =
            serde_json::Value::Bool(false);
        assert!(
            validate_published_adapter_evidence(&changed_preflight, &approved)
                .unwrap_err()
                .contains("preflight evidence")
        );

        let mut legacy_shape = manifest.clone();
        legacy_shape.adapter_evidence = serde_json::json!({
            "contract": "u1_full_spectrum_source_plan_bound",
            "targetValidation": {"valid": true}
        });
        assert!(
            validate_published_adapter_evidence(&legacy_shape, &approved)
                .unwrap_err()
                .contains("evidence envelope")
        );

        let mut changed_validation = artifact.clone();
        let expected_validation =
            changed_validation.adapter_evidence["publishedValidation"].clone();
        ensure_exact_published_validation_evidence(&changed_validation, &expected_validation)
            .unwrap();
        changed_validation.adapter_evidence["publishedValidation"]["targetValidation"]["valid"] =
            serde_json::Value::Bool(false);
        assert!(
            ensure_exact_published_validation_evidence(&changed_validation, &expected_validation,)
                .unwrap_err()
                .contains("differs from fresh source-plan validation")
        );
    }

    #[test]
    fn published_json_contract_uses_exact_wire_canonical_numbers() {
        let expected = serde_json::json!({"deltaE00": 93.25235931303799});
        let actual =
            serde_json::from_str::<serde_json::Value>(r#"{"deltaE00":93.252359313038}"#).unwrap();

        ensure_json_contract(&actual, &expected, "numeric fixture").unwrap();

        let tampered =
            serde_json::from_str::<serde_json::Value>(r#"{"deltaE00":93.252359313039}"#).unwrap();
        assert!(
            ensure_json_contract(&tampered, &expected, "numeric fixture")
                .unwrap_err()
                .contains("differs from the current canonical plan")
        );
    }

    #[test]
    fn receipt_publication_failure_rolls_back_only_the_exact_new_bundle() {
        let parent = tempfile::tempdir().unwrap();
        let parent = parent.path().canonicalize().unwrap();
        let output = parent.join("new-bundle");
        fs::create_dir(&output).unwrap();
        fs::write(output.join("manifest.json"), b"fixture").unwrap();
        let identity = trusted_directory_identity(&output).unwrap();

        let error = complete_publication_receipt_or_rollback(&output, &identity, || {
            Err("injected receipt failure".into())
        })
        .unwrap_err();

        assert!(error.starts_with("conversion_receipt_publication_failed:"));
        assert!(!output.exists());
    }

    #[test]
    fn destination_sync_failure_rolls_back_before_receipt_commit() {
        let parent = tempfile::tempdir().unwrap();
        let parent = parent.path().canonicalize().unwrap();
        let output = parent.join("unsynchronized-bundle");
        fs::create_dir(&output).unwrap();
        fs::write(output.join("manifest.json"), b"fixture").unwrap();
        let identity = trusted_directory_identity(&output).unwrap();
        let receipt_was_committed = std::cell::Cell::new(false);

        let error = complete_publication_receipt_or_rollback_with_parent_sync(
            &output,
            &identity,
            || Err("injected destination sync failure".into()),
            || {
                receipt_was_committed.set(true);
                Ok(())
            },
        )
        .unwrap_err();

        assert!(error.starts_with("conversion_receipt_publication_failed:"));
        assert!(error.contains("injected destination sync failure"));
        assert!(!receipt_was_committed.get());
        assert!(!output.exists());
    }

    #[test]
    fn receipt_rollback_never_deletes_a_racing_public_path_replacement() {
        let parent = tempfile::tempdir().unwrap();
        let parent = parent.path().canonicalize().unwrap();
        let output = parent.join("racing-bundle");
        fs::create_dir(&output).unwrap();
        fs::write(output.join("published-file"), b"published").unwrap();
        let identity = trusted_directory_identity(&output).unwrap();
        let quarantine = std::cell::RefCell::new(None::<PathBuf>);

        let error = rollback_unreceipted_published_bundle_with_hook(
            &output,
            &identity,
            |quarantine_path| {
                *quarantine.borrow_mut() = Some(quarantine_path.to_owned());
                fs::create_dir(&output).unwrap();
                fs::write(output.join("replacement-file"), b"preserve").unwrap();
            },
        )
        .unwrap_err();

        assert!(error.contains("another entry now occupies its public output path"));
        assert_eq!(
            fs::read(output.join("replacement-file")).unwrap(),
            b"preserve"
        );
        assert!(!quarantine.borrow().as_ref().unwrap().exists());
    }

    #[test]
    fn receipt_rollback_leaves_identity_mismatch_quarantined_when_path_is_replaced() {
        let parent = tempfile::tempdir().unwrap();
        let parent = parent.path().canonicalize().unwrap();
        let output = parent.join("mismatched-bundle");
        fs::create_dir(&output).unwrap();
        let expected_identity = trusted_directory_identity(&output).unwrap();
        fs::rename(&output, parent.join("expected-bundle-elsewhere")).unwrap();
        fs::create_dir(&output).unwrap();
        fs::write(output.join("mismatched-file"), b"quarantine").unwrap();
        let quarantine = std::cell::RefCell::new(None::<PathBuf>);

        let error = rollback_unreceipted_published_bundle_with_hook(
            &output,
            &expected_identity,
            |quarantine_path| {
                *quarantine.borrow_mut() = Some(quarantine_path.to_owned());
                fs::create_dir(&output).unwrap();
                fs::write(output.join("replacement-file"), b"preserve").unwrap();
            },
        )
        .unwrap_err();

        let quarantine = quarantine.into_inner().unwrap();
        assert!(error.contains("remains quarantined"));
        assert_eq!(
            fs::read(output.join("replacement-file")).unwrap(),
            b"preserve"
        );
        assert_eq!(
            fs::read(quarantine.join("mismatched-file")).unwrap(),
            b"quarantine"
        );
    }

    #[test]
    fn missing_receipt_explains_no_clobber_retry_options() {
        let parent = tempfile::tempdir().unwrap();
        let parent = parent.path().canonicalize().unwrap();
        let registry = parent.join("receipts");
        let output = parent.join("untrusted-bundle");
        fs::create_dir(&output).unwrap();

        let error = load_publication_receipt(&registry, &output).unwrap_err();
        assert!(error.contains("Select a different destination"));
        assert!(error.contains("move/remove the untrusted existing bundle"));
        assert!(
            ensure_output_path_absent(&output)
                .unwrap_err()
                .contains("the converter never overwrites an existing output path")
        );
    }

    #[test]
    fn private_publication_receipt_round_trips_and_rejects_unknown_fields() {
        let parent = tempfile::tempdir().unwrap();
        let parent = parent.path().canonicalize().unwrap();
        let registry = parent.join("receipts");
        let output = parent.join("bundle");
        fs::create_dir(&output).unwrap();
        let receipt = PublicationReceipt {
            schema_version: PUBLICATION_RECEIPT_SCHEMA_VERSION,
            converter: published_converter_manifest(),
            source_path: parent.join("source.3mf"),
            source_byte_size: 42,
            source_sha256: "a".repeat(64),
            plan_fingerprint: "b".repeat(64),
            canonical_preflight_sha256: "c".repeat(64),
            output_root: output.clone(),
            output_root_identity: trusted_directory_identity(&output).unwrap(),
            metadata: vec![PublicationReceiptFile {
                relative_path: PUBLISHED_MANIFEST_FILE_NAME.into(),
                byte_size: 7,
                sha256: "d".repeat(64),
            }],
            artifacts: vec![PublicationReceiptArtifact {
                adapter_id: "fixture-adapter".into(),
                target: "a1_mini_mono".into(),
                batch_id: "fixture-batch".into(),
                file_name: "fixture.3mf".into(),
                relative_path: "a1-mini/fixture.3mf".into(),
                byte_size: 11,
                sha256: "e".repeat(64),
            }],
        };
        stage_publication_receipt(&registry, &receipt)
            .unwrap()
            .publish(&registry)
            .unwrap();
        assert_eq!(
            load_publication_receipt(&registry, &output).unwrap(),
            receipt
        );

        let receipt_path = publication_receipt_path(&registry, &output).unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
        value["untrustedField"] = serde_json::Value::Bool(true);
        fs::write(&receipt_path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(
            load_publication_receipt(&registry, &output)
                .unwrap_err()
                .contains("receipt is corrupt")
        );
    }

    #[test]
    fn receipt_rollback_preserves_a_replaced_output_root() {
        let parent = tempfile::tempdir().unwrap();
        let parent = parent.path().canonicalize().unwrap();
        let output = parent.join("replaced-bundle");
        fs::create_dir(&output).unwrap();
        let original_identity = trusted_directory_identity(&output).unwrap();
        fs::rename(&output, parent.join("original-bundle")).unwrap();
        fs::create_dir(&output).unwrap();
        fs::write(output.join("user-file"), b"preserve").unwrap();

        let error = complete_publication_receipt_or_rollback(&output, &original_identity, || {
            Err("injected receipt failure".into())
        })
        .unwrap_err();

        assert!(error.starts_with("conversion_published_without_recovery:"));
        assert_eq!(fs::read(output.join("user-file")).unwrap(), b"preserve");
    }

    #[test]
    fn json_number_equivalence_is_exact_at_wire_boundaries() {
        let number = |text: &str| {
            serde_json::from_str::<serde_json::Value>(text)
                .unwrap_or_else(|error| panic!("invalid numeric fixture {text:?}: {error}"))
        };

        assert!(json_values_equivalent(&number("-0"), &number("0.0")));
        assert!(json_values_equivalent(&number("1e+3"), &number("1000")));
        assert!(json_values_equivalent(
            &number("4.9406564584124654e-324"),
            &number("5e-324"),
        ));
        assert!(!json_values_equivalent(
            &number("5e-324"),
            &number("1e-323"),
        ));
        assert!(!json_values_equivalent(
            &number("9007199254740993"),
            &number("9007199254740992"),
        ));
        assert!(!json_values_equivalent(
            &number("9007199254740993"),
            &number("9.007199254740992e15"),
        ));
    }

    #[test]
    fn direct_adapter_evidence_is_preserved_per_output_file() {
        let manifest = serde_json::json!({
            "adapterEvidence": {"status": "qualified"},
            "sourceToTarget": [
                {"sourceUnitId": "unit-1", "outputFile": "first.3mf"},
                {"sourceUnitId": "unit-2", "outputFile": "second.3mf"}
            ]
        });

        let evidence = direct_artifact_validation(&manifest, "second.3mf").unwrap();

        assert_eq!(evidence["adapterCapability"]["status"], "qualified");
        assert_eq!(evidence["sourceToTarget"].as_array().unwrap().len(), 1);
        assert_eq!(evidence["sourceToTarget"][0]["sourceUnitId"], "unit-2");
    }

    #[test]
    fn bundle_names_are_ascii_bounded_and_collision_resistant() {
        let first = safe_bundle_component(Path::new("Лиса.3mf"));
        let second = safe_bundle_component(Path::new("Дракон.3mf"));
        assert!(first.is_ascii());
        assert!(second.is_ascii());
        assert_ne!(first, second);
        assert!(first.starts_with("project-"));
        assert!(first.ends_with("__converted"));
        assert!(first.len() < 128);
    }

    #[test]
    fn staged_bundle_tree_accepts_only_the_declared_closure() {
        let temporary = tempfile::tempdir().unwrap();
        let target = temporary.path().join("a1-mini");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("job-001.3mf"), b"fixture").unwrap();
        let artifacts = vec![published_fixture(
            temporary.path(),
            "a1-mini",
            "job-001.3mf",
        )];
        validate_staged_bundle_tree(temporary.path(), &artifacts, false).unwrap();

        fs::write(temporary.path().join(PUBLISHED_MANIFEST_FILE_NAME), b"{}").unwrap();
        fs::write(
            temporary.path().join(PUBLISHED_CONVERSION_PLAN_FILE_NAME),
            b"{}",
        )
        .unwrap();
        fs::write(
            temporary.path().join(PUBLISHED_CONVERSION_REPORT_FILE_NAME),
            b"<!doctype html>",
        )
        .unwrap();
        fs::write(temporary.path().join(PUBLISHED_CHECKSUMS_FILE_NAME), b"").unwrap();
        validate_staged_bundle_tree(temporary.path(), &artifacts, true).unwrap();

        fs::write(target.join("unexpected.txt"), b"unexpected").unwrap();
        assert!(
            validate_staged_bundle_tree(temporary.path(), &artifacts, true)
                .unwrap_err()
                .contains("unexpected entry")
        );
    }

    #[test]
    fn staged_target_sync_covers_each_final_child_directory() {
        let temporary = tempfile::tempdir().unwrap();
        for directory in ["u1-direct", "u1-full-spectrum", "a1-mini"] {
            fs::create_dir(temporary.path().join(directory)).unwrap();
        }
        let artifacts = vec![
            published_fixture(temporary.path(), "u1-direct", "solid.3mf"),
            published_fixture(temporary.path(), "u1-direct", "direct.3mf"),
            published_fixture(temporary.path(), "u1-full-spectrum", "full-spectrum.3mf"),
            published_fixture(temporary.path(), "a1-mini", "mono.3mf"),
        ];

        sync_staged_target_directories(temporary.path(), &artifacts).unwrap();

        let outside = tempfile::tempdir().unwrap();
        let escaped = vec![published_fixture(outside.path(), "a1-mini", "escaped.3mf")];
        assert!(
            sync_staged_target_directories(temporary.path(), &escaped)
                .unwrap_err()
                .contains("outside its target directory")
        );
    }

    #[cfg(unix)]
    #[test]
    fn staged_bundle_tree_rejects_symlinked_artifacts() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let target = temporary.path().join("a1-mini");
        fs::create_dir(&target).unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        symlink(outside.path(), target.join("job-001.3mf")).unwrap();
        let artifacts = vec![published_fixture(
            temporary.path(),
            "a1-mini",
            "job-001.3mf",
        )];
        assert!(validate_staged_bundle_tree(temporary.path(), &artifacts, false).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn bounded_bundle_metadata_reader_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let regular = temporary.path().join("manifest-source.json");
        fs::write(&regular, b"{}\n").unwrap();
        let linked = temporary.path().join("manifest.json");
        symlink(&regular, &linked).unwrap();

        let error = bounded_regular_file(&linked, 1024, "published manifest").unwrap_err();
        assert!(error.contains("not a regular file"));
    }

    #[test]
    fn failed_revalidation_preserves_previous_output_registry() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source.3mf");
        fs::write(&source, b"source identity").unwrap();
        let (source_size, source_sha256) = hash_file(&source).unwrap();
        let mut project = cached_project_with_plan(cached_plan(r#"{"planReady":true}"#));
        project.analysis.input = InputIdentity {
            byte_size: source_size,
            sha256: source_sha256,
        };

        let root = temporary.path().join("bundle");
        let target = root.join("a1-mini");
        fs::create_dir_all(&target).unwrap();
        let artifact_path = target.join("job-001.3mf");
        fs::write(&artifact_path, b"published artifact").unwrap();
        let (artifact_size, artifact_sha256) = hash_file(&artifact_path).unwrap();
        let mut artifact = published_fixture(&root, "a1-mini", "job-001.3mf");
        artifact.byte_size = artifact_size;
        artifact.sha256 = artifact_sha256.clone();

        let mut metadata_identities = Vec::new();
        for name in [
            PUBLISHED_MANIFEST_FILE_NAME,
            PUBLISHED_CONVERSION_PLAN_FILE_NAME,
            PUBLISHED_CONVERSION_REPORT_FILE_NAME,
            PUBLISHED_CHECKSUMS_FILE_NAME,
        ] {
            let path = root.join(name);
            fs::write(&path, format!("{name}\n")).unwrap();
            metadata_identities.push(
                bounded_regular_file(&path, 1024, "published bundle metadata")
                    .unwrap()
                    .1,
            );
        }

        let previous_path = PathBuf::from("/previous/published.3mf");
        let previous = RegisteredPublishedOutput {
            canonical_path: previous_path.clone(),
            adapter_id: "previous-adapter".into(),
            slicer: RegisteredSlicer::SnapmakerOrca,
            byte_size: 1,
            sha256: "previous-sha".into(),
        };
        let registry = Mutex::new(HashMap::from([(previous_path.clone(), previous.clone())]));
        let next = HashMap::from([(
            artifact_path.clone(),
            RegisteredPublishedOutput {
                canonical_path: artifact_path,
                adapter_id: artifact.adapter_id.clone(),
                slicer: RegisteredSlicer::BambuStudio,
                byte_size: artifact_size,
                sha256: artifact_sha256,
            },
        )]);
        let generation = Mutex::new(7);

        let stale_error = restore_revalidated_output_registry_if_current(
            &generation,
            6,
            &registry,
            next.clone(),
            &source,
            &project.analysis,
            &root,
            &[artifact.clone()],
            &metadata_identities,
        )
        .unwrap_err();
        assert!(stale_error.contains("changed during output validation"));
        assert_eq!(
            registry.lock().unwrap().get(&previous_path),
            Some(&previous)
        );

        fs::write(
            root.join(PUBLISHED_CONVERSION_REPORT_FILE_NAME),
            b"changed after validation",
        )
        .unwrap();

        let error = restore_revalidated_output_registry_if_current(
            &generation,
            7,
            &registry,
            next,
            &source,
            &project.analysis,
            &root,
            &[artifact],
            &metadata_identities,
        )
        .unwrap_err();
        assert!(error.contains("changed after validation"));
        assert_eq!(
            registry.lock().unwrap().get(&previous_path),
            Some(&previous)
        );
    }

    fn cached_preparation(token: &str, created_at: Instant) -> CachedPreparation {
        CachedPreparation {
            token: token.to_owned(),
            cache_generation: 0,
            canonical_path: PathBuf::from("fixture.3mf"),
            source_sha256: "fixture-source".to_owned(),
            plan_fingerprint: "fixture-plan".to_owned(),
            created_at,
            preparation: NativeConversionPreparation {
                adapter_id: "fixture-adapter".to_owned(),
                source_sha256: "fixture-source".to_owned(),
                plan_fingerprint: "fixture-plan".to_owned(),
                bundle_directory_name: "fixture-bundle".to_owned(),
                artifacts: Vec::new(),
                excluded_source_units: Vec::new(),
                warnings: Vec::new(),
            },
            targets: PreparedTargetSlices {
                direct: None,
                full_spectrum: None,
                a1_mini: None,
                excluded_source_units: Vec::new(),
            },
            partial_conversion_approval: None,
            experimental_dialect_approval: None,
            control: MixedConversionControl::new(),
        }
    }

    fn cached_plan(contents: &str) -> CachedPlan {
        let value = serde_json::from_str(contents).expect("valid cached plan fixture");
        CachedPlan {
            contents: serde_json::to_string_pretty(&value).expect("serialize cached plan fixture"),
            value,
            invalidated: false,
        }
    }

    fn cached_project_with_plan(latest_plan: CachedPlan) -> CachedProject {
        CachedProject {
            canonical_path: PathBuf::from("fixture.3mf"),
            analysis: ProjectAnalysis {
                input: InputIdentity {
                    byte_size: 42,
                    sha256: "fixture-source".into(),
                },
                source: u1_three_mf::SourceInformation {
                    application: u1_three_mf::SourceApplication::BambuStudio,
                    application_name: Some("Bambu Studio".into()),
                    application_version: None,
                    title: Some("Fixture".into()),
                    dialect: u1_three_mf::ProjectDialect::BambuStudioProject,
                    dialect_version: None,
                    support: u1_three_mf::DialectSupport::Supported,
                    has_bambu_or_orca_metadata: true,
                    has_sliced_artifacts: false,
                    sliced_artifact_entries: Vec::new(),
                },
                archive: u1_three_mf::ArchiveStatistics {
                    entry_count: 0,
                    archive_bytes: 42,
                    total_compressed_bytes: 0,
                    total_uncompressed_bytes: 0,
                    largest_entry_uncompressed_bytes: 0,
                    maximum_compression_ratio: 0.0,
                },
                printer: u1_three_mf::PrinterInformation::default(),
                process: u1_three_mf::ProcessInformation::default(),
                filaments: Vec::new(),
                effective_material_colors: Vec::new(),
                plates: Vec::new(),
                objects: Vec::new(),
                summary: u1_three_mf::AnalysisSummary::default(),
                warnings: Vec::new(),
            },
            latest_plan,
            planning_input: PlanningInput {
                scopes: Vec::new(),
                inventory: Vec::new(),
                current_toolheads: u1_planner::CurrentToolheadState::default(),
                config: u1_planner::PlannerConfig::with_cmy_setup(u1_planner::CmySetup {
                    cyan_spool_id: "cyan".into(),
                    magenta_spool_id: "magenta".into(),
                    yellow_spool_id: "yellow".into(),
                    default_t4_spool_id: None,
                }),
            },
            planning_result: PlanningResult {
                scope_options: Vec::new(),
                jobs: Vec::new(),
                plates: Vec::new(),
                batches: Vec::new(),
                t4_swap_count: 0,
                a1_spool_change_count: 0,
                final_toolheads: u1_planner::CurrentToolheadState::default(),
                warnings: Vec::new(),
                errors: Vec::new(),
            },
            plan_fingerprint: "fixture-plan".into(),
        }
    }

    #[test]
    fn experimental_dialect_approval_is_exactly_source_fingerprint_bound() {
        let mut project = cached_project_with_plan(cached_plan(r#"{"planReady":true}"#));
        project.analysis.source.support = DialectSupport::Experimental;
        project.analysis.source.application_version = Some("future-version".into());
        let fingerprint = experimental_dialect_fingerprint(&project.analysis);

        assert!(validate_source_dialect_for_adapter(&project.analysis, None).is_err());
        assert!(
            validate_source_dialect_for_adapter(
                &project.analysis,
                Some(&ExperimentalDialectApproval {
                    source_fingerprint: "sha256:stale".into(),
                }),
            )
            .is_err()
        );
        validate_source_dialect_for_adapter(
            &project.analysis,
            Some(&ExperimentalDialectApproval {
                source_fingerprint: fingerprint.clone(),
            }),
        )
        .unwrap();

        project.analysis.input.sha256 = "different-source".into();
        assert_ne!(
            fingerprint,
            experimental_dialect_fingerprint(&project.analysis)
        );
    }

    #[test]
    fn unsupported_dialect_cannot_be_approved_for_any_adapter() {
        let mut project = cached_project_with_plan(cached_plan(r#"{"planReady":true}"#));
        project.analysis.source.support = DialectSupport::Unsupported;
        let approval = ExperimentalDialectApproval {
            source_fingerprint: experimental_dialect_fingerprint(&project.analysis),
        };
        assert!(
            validate_source_dialect_for_adapter(&project.analysis, Some(&approval))
                .unwrap_err()
                .to_ascii_lowercase()
                .contains("unsupported")
        );
    }

    fn calibration_measurement(id: &str) -> CmyxCalibrationMeasurementInput {
        CmyxCalibrationMeasurementInput {
            id: id.to_owned(),
            loadout: CmyxCalibrationLoadout::new(
                "panchroma-translucent-cyan",
                "panchroma-translucent-magenta",
                "panchroma-translucent-yellow",
                "panchroma-basic-black",
            ),
            recipe: MixRecipe {
                mode: RecipeMode::Ratio,
                components: vec![
                    RecipeComponent { slot: 1, weight: 3 },
                    RecipeComponent { slot: 4, weight: 1 },
                ],
            },
            measured_output_hex: "#2A6F51".to_owned(),
            geometry_context: CmyxGeometryContext {
                orientation: SampleOrientation::Upright,
                geometry_class: GeometryClass::CalibrationSwatch,
            },
            provenance: CmyxMeasurementProvenance::verified(
                "2026-08-01T18:30:00Z",
                CmyxMeasurementMethod::InstrumentLab,
            ),
            physical_measurement_confirmed: true,
        }
    }

    #[test]
    fn measurement_input_builds_authoritative_profile_and_process_identity() {
        let measurement = calibration_measurement("black-r1");
        let expected_context =
            full_spectrum_calibration_context(&measurement.loadout, &measurement.geometry_context);
        let record = measurement.into_record().unwrap();

        assert_eq!(record.context, expected_context);
        record.validate().unwrap();
        assert!(
            serde_json::from_value::<CmyxCalibrationMeasurementInput>(serde_json::json!({
                "id": "forged",
                "loadout": {
                    "t1CalibrationId": "c",
                    "t2CalibrationId": "m",
                    "t3CalibrationId": "y",
                    "t4CalibrationId": "x"
                },
                "recipe": { "mode": "solid", "components": [{ "slot": 4, "weight": 1 }] },
                "measuredOutputHex": "#000000",
                "geometryContext": { "orientation": "upright", "geometryClass": "calibration_swatch" },
                "context": { "processFingerprint": "frontend-forgery" }
            }))
            .is_err(),
            "the simple UI input must reject a caller-supplied CalibrationContext"
        );

        let mut nominal = calibration_measurement("nominal-preview");
        nominal.physical_measurement_confirmed = false;
        assert!(
            nominal
                .into_record()
                .unwrap_err()
                .contains("physical printed swatch")
        );

        let mut legacy = calibration_measurement("legacy-forgery");
        legacy.provenance = CmyxMeasurementProvenance::legacy_unverified();
        assert!(
            legacy
                .into_record()
                .unwrap_err()
                .contains("legacy unverified")
        );
    }

    #[test]
    fn calibration_project_request_uses_exact_available_cmyx_physical_loadout() {
        let inventory = u1_application::built_in_spool_inventory().unwrap();
        let spool_ids = [
            "panchroma-translucent-cyan".to_owned(),
            "panchroma-translucent-magenta".to_owned(),
            "panchroma-translucent-yellow".to_owned(),
            "panchroma-translucent-grey".to_owned(),
        ];

        let spec = build_calibration_project_spec(
            validate_calibration_project_id("grey_chart_01").unwrap(),
            spool_ids.clone(),
            &inventory,
        )
        .unwrap();

        assert_eq!(spec.project_id, "grey_chart_01");
        assert_eq!(spec.swatches.len(), 18);
        assert_eq!(
            spec.loadout
                .iter()
                .map(|slot| slot.spool_id.clone())
                .collect::<Vec<_>>(),
            spool_ids.to_vec()
        );
        assert!(
            spec.loadout
                .iter()
                .all(|slot| slot.material == u1_planner::Material::Pla)
        );
    }

    #[test]
    fn calibration_project_request_rejects_role_changes_and_unavailable_spools() {
        let mut inventory = u1_application::built_in_spool_inventory().unwrap();
        let valid_ids = [
            "panchroma-translucent-cyan".to_owned(),
            "panchroma-translucent-magenta".to_owned(),
            "panchroma-translucent-yellow".to_owned(),
            "panchroma-translucent-grey".to_owned(),
        ];
        let mut wrong_roles = valid_ids.clone();
        wrong_roles.swap(0, 1);
        assert!(
            build_calibration_project_spec("wrong-roles".to_owned(), wrong_roles, &inventory)
                .unwrap_err()
                .contains("Calibration T1")
        );

        inventory
            .iter_mut()
            .find(|spool| spool.id == valid_ids[3])
            .expect("built-in T4")
            .available = false;
        assert!(
            build_calibration_project_spec("out-of-stock".to_owned(), valid_ids, &inventory)
                .unwrap_err()
                .contains("out of stock")
        );
    }

    #[test]
    fn calibration_destination_is_absolute_3mf_and_never_clobbers() {
        let directory = tempfile::tempdir().unwrap();
        let candidate = directory.path().join("chart.3mf");
        assert_eq!(
            new_calibration_destination(candidate.to_str().unwrap()).unwrap(),
            directory.path().canonicalize().unwrap().join("chart.3mf")
        );
        File::create(&candidate).unwrap();
        assert!(
            new_calibration_destination(candidate.to_str().unwrap())
                .unwrap_err()
                .contains("will not be overwritten")
        );
        assert!(
            new_calibration_destination("relative.3mf")
                .unwrap_err()
                .contains("absolute")
        );
        assert!(
            new_calibration_destination(directory.path().join("chart.zip").to_str().unwrap())
                .unwrap_err()
                .contains(".3mf extension")
        );
    }

    #[test]
    fn wrong_preparation_token_does_not_consume_a_valid_preparation() {
        let cache = Mutex::new(Some(cached_preparation("valid-token", Instant::now())));

        let error = match take_prepared_conversion(&cache, "wrong-token") {
            Ok(_) => panic!("a mismatched token must fail"),
            Err(error) => error,
        };

        assert!(error.contains("invalid or stale"));
        assert_eq!(
            cache
                .lock()
                .expect("preparation cache lock")
                .as_ref()
                .expect("valid preparation must remain cached")
                .token,
            "valid-token"
        );
        assert_eq!(
            take_prepared_conversion(&cache, "valid-token")
                .expect("the valid token must still work")
                .token,
            "valid-token"
        );
        assert!(cache.lock().expect("preparation cache lock").is_none());
    }

    #[test]
    fn newer_project_operation_cancels_and_supersedes_old_preflight() {
        let cache = ProjectCache::default();
        let previous = cached_preparation("valid-token", Instant::now());
        let previous_control = previous.control.clone();
        *cache.prepared.lock().unwrap() = Some(previous);
        let active_control = MixedConversionControl::new();
        cache.active_conversions.lock().unwrap().insert(
            active_control.conversion_id().to_owned(),
            active_control.clone(),
        );

        let first = begin_cache_operation(&cache).unwrap();
        assert!(cache.prepared.lock().unwrap().is_none());
        assert_eq!(previous_control.state(), U1DirectConversionState::Cancelled);
        assert_eq!(active_control.state(), U1DirectConversionState::Cancelled);
        let second = begin_cache_operation(&cache).unwrap();
        assert!(second > first);
        assert!(
            ensure_current_cache_operation(&cache, first, "fixture")
                .unwrap_err()
                .contains("superseded")
        );
        let _generation = ensure_current_cache_operation(&cache, second, "fixture").unwrap();
    }

    #[test]
    fn analysis_cancellation_invalidates_its_generation_once() {
        let cache = ProjectCache::default();
        let analysis_generation = begin_analysis_operation(&cache).unwrap();
        assert_eq!(
            *cache.active_analysis.lock().unwrap(),
            Some(analysis_generation)
        );

        assert_eq!(
            cancel_active_analysis(&cache).unwrap(),
            CancelAnalysisResult { accepted: true }
        );
        assert!(cache.active_analysis.lock().unwrap().is_none());
        assert!(
            ensure_current_cache_operation(&cache, analysis_generation, "analysis")
                .unwrap_err()
                .contains("superseded")
        );
        let generation_after_cancel = *cache.operation_generation.lock().unwrap();

        assert_eq!(
            cancel_active_analysis(&cache).unwrap(),
            CancelAnalysisResult { accepted: false }
        );
        assert_eq!(
            *cache.operation_generation.lock().unwrap(),
            generation_after_cancel
        );
    }

    #[test]
    fn stale_analysis_cleanup_cannot_clear_a_newer_registration() {
        let cache = ProjectCache::default();
        let first = begin_analysis_operation(&cache).unwrap();
        let second = begin_analysis_operation(&cache).unwrap();

        clear_analysis_registration(&cache, first).unwrap();

        assert_eq!(*cache.active_analysis.lock().unwrap(), Some(second));
        let _generation = ensure_current_cache_operation(&cache, second, "analysis").unwrap();
    }

    #[test]
    fn inventory_invalidation_rejects_prepared_conversion_and_clears_quick_actions() {
        let cache = ProjectCache::default();
        let previous = cached_preparation("inventory-bound-token", Instant::now());
        let previous_control = previous.control.clone();
        *cache.prepared.lock().unwrap() = Some(previous);
        *cache.analyzed.lock().unwrap() = Some(cached_project_with_plan(cached_plan(
            r#"{"planReady":true,"plates":[]}"#,
        )));
        cache.published_outputs.lock().unwrap().insert(
            PathBuf::from("/fixture/published.3mf"),
            RegisteredPublishedOutput {
                canonical_path: PathBuf::from("/fixture/published.3mf"),
                adapter_id: "fixture-adapter".into(),
                slicer: RegisteredSlicer::SnapmakerOrca,
                byte_size: 42,
                sha256: "fixture-sha".into(),
            },
        );

        invalidate_inventory_dependent_state(&cache).unwrap();

        assert!(cache.prepared.lock().unwrap().is_none());
        assert!(cache.published_outputs.lock().unwrap().is_empty());
        assert!(
            !cache
                .analyzed
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .latest_plan
                .is_ready()
        );
        assert_eq!(previous_control.state(), U1DirectConversionState::Cancelled);
        let error = match inspect_prepared_conversion(&cache.prepared, "inventory-bound-token") {
            Ok(_) => panic!("an inventory change must invalidate the preparation token"),
            Err(error) => error,
        };
        assert!(error.contains("expired"));
    }

    #[test]
    fn generation_change_cancels_active_conversion_and_blocks_publication() {
        let cache = Arc::new(ProjectCache::default());
        let approved_generation = begin_cache_operation(&cache).unwrap();
        let control = MixedConversionControl::new();
        let conversion_id = control.conversion_id().to_owned();
        cache
            .active_conversions
            .lock()
            .unwrap()
            .insert(conversion_id, control.clone());

        let temporary = tempfile::tempdir().unwrap();
        let final_bundle = temporary.path().join("stale-plan__converted");
        let worker_bundle = final_bundle.clone();
        let worker_control = control.clone();
        let operation_generation = Arc::clone(&cache.operation_generation);
        let published_outputs = Arc::clone(&cache.published_outputs);
        let publication_barrier = Arc::new(std::sync::Barrier::new(2));
        let worker_barrier = Arc::clone(&publication_barrier);
        let worker = std::thread::spawn(move || {
            worker_barrier.wait();
            publish_bundle_and_registry_if_cache_generation_current(
                &operation_generation,
                approved_generation,
                &worker_control,
                &published_outputs,
                HashMap::new(),
                || {
                    fs::create_dir(&worker_bundle)
                        .map_err(|error| format!("fixture publication failed: {error}"))
                },
            )
        });

        invalidate_inventory_dependent_state(&cache).unwrap();
        publication_barrier.wait();
        let error = worker.join().unwrap().unwrap_err();

        assert!(error.starts_with("conversion_cancelled:"));
        assert!(!final_bundle.exists());
        assert_eq!(control.state(), U1DirectConversionState::Cancelled);
        assert_eq!(
            *cache.operation_generation.lock().unwrap(),
            approved_generation + 1
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn failed_bundle_commit_cleans_private_staging_and_preserves_previous_registry() {
        let destination = tempfile::tempdir().unwrap();
        let final_bundle = destination.path().join("existing__converted");
        fs::create_dir(&final_bundle).unwrap();
        fs::write(final_bundle.join("user-file.txt"), b"keep me").unwrap();
        let control = MixedConversionControl::new();
        let recovery = control.recovery_record(destination.path()).unwrap();
        let staging = U1NativeConversionStaging::create(destination.path(), &control.outer)
            .expect("private staging");
        let staging_path = staging.path().to_path_buf();
        fs::write(staging.path().join("manifest.json"), b"fixture").unwrap();

        let previous_path = PathBuf::from("/previous/output.3mf");
        let previous = RegisteredPublishedOutput {
            canonical_path: previous_path.clone(),
            adapter_id: "previous-adapter".into(),
            slicer: RegisteredSlicer::SnapmakerOrca,
            byte_size: 7,
            sha256: "previous-sha".into(),
        };
        let registry = Mutex::new(HashMap::from([(previous_path.clone(), previous)]));
        let generation = Mutex::new(4_u64);

        let error = publish_bundle_and_registry_if_cache_generation_current(
            &generation,
            4,
            &control,
            &registry,
            HashMap::new(),
            || {
                staging
                    .publish(&final_bundle, &control.outer)
                    .map_err(|error| error.to_string())
            },
        )
        .unwrap_err();

        assert!(error.contains("output already exists"));
        assert!(!staging_path.exists());
        assert_eq!(
            fs::read(final_bundle.join("user-file.txt")).unwrap(),
            b"keep me"
        );
        assert!(registry.lock().unwrap().contains_key(&previous_path));
        assert_eq!(
            finalize_u1_direct_staging(&recovery).unwrap(),
            U1DirectStagingCleanup::Missing
        );
    }

    #[test]
    fn calibration_transaction_invalidates_only_after_persistence_succeeds() {
        let cache = ProjectCache::default();
        let previous = cached_preparation("calibration-bound-token", Instant::now());
        let previous_control = previous.control.clone();
        *cache.prepared.lock().unwrap() = Some(previous);
        *cache.analyzed.lock().unwrap() = Some(cached_project_with_plan(cached_plan(
            r#"{"planReady":true,"plates":[]}"#,
        )));
        cache.published_outputs.lock().unwrap().insert(
            PathBuf::from("/fixture/calibrated.3mf"),
            RegisteredPublishedOutput {
                canonical_path: PathBuf::from("/fixture/calibrated.3mf"),
                adapter_id: "fixture-adapter".into(),
                slicer: RegisteredSlicer::SnapmakerOrca,
                byte_size: 42,
                sha256: "fixture-sha".into(),
            },
        );
        let initial_generation = *cache.operation_generation.lock().unwrap();

        let error = commit_calibration_library_change::<()>(&cache, || {
            Err("simulated persistence failure".to_owned())
        })
        .unwrap_err();
        assert!(error.contains("persistence failure"));
        assert_eq!(
            *cache.operation_generation.lock().unwrap(),
            initial_generation
        );
        assert!(cache.prepared.lock().unwrap().is_some());
        assert_eq!(cache.published_outputs.lock().unwrap().len(), 1);
        assert!(
            cache
                .analyzed
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .latest_plan
                .is_ready()
        );

        assert_eq!(
            commit_calibration_library_change(&cache, || Ok("saved".to_owned())).unwrap(),
            "saved"
        );
        assert_eq!(
            *cache.operation_generation.lock().unwrap(),
            initial_generation + 1
        );
        assert!(cache.prepared.lock().unwrap().is_none());
        assert!(cache.published_outputs.lock().unwrap().is_empty());
        assert!(
            !cache
                .analyzed
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .latest_plan
                .is_ready()
        );
        assert_eq!(previous_control.state(), U1DirectConversionState::Cancelled);
    }

    #[test]
    fn inventory_invalidation_marks_the_authoritative_plan_not_ready() {
        let mut plan = cached_plan(r#"{"planReady":true,"plates":[]}"#);

        plan.invalidate_inventory();

        assert!(!plan.is_ready());
        assert!(plan.invalidated);
        assert_eq!(plan.value["planReady"], serde_json::Value::Bool(false));
        assert!(plan.contents.contains("\"planReady\": false"));
    }

    #[test]
    fn quick_output_actions_accept_only_the_current_published_artifacts() {
        let temporary = tempfile::tempdir().unwrap();
        let target_directory = temporary.path().join("a1-mini");
        fs::create_dir(&target_directory).unwrap();
        let file_name = "fixture.3mf";
        let path = target_directory.join(file_name);
        fs::write(&path, b"published fixture").unwrap();
        let (byte_size, sha256) = hash_file(&path).unwrap();
        let mut artifact = published_fixture(temporary.path(), "a1-mini", file_name);
        artifact.adapter_id = "bambu-studio/fixture".into();
        artifact.byte_size = byte_size;
        artifact.sha256 = sha256;
        let result = NativeConversionResult {
            adapter_id: "fixture".into(),
            output_directory: temporary.path().to_path_buf(),
            manifest_path: temporary.path().join("manifest.json"),
            report_path: temporary.path().join(PUBLISHED_CONVERSION_REPORT_FILE_NAME),
            artifacts: vec![artifact.clone()],
            excluded_source_units: Vec::new(),
            warnings_acknowledged: true,
            warnings: Vec::new(),
        };
        let registry = Mutex::new(HashMap::new());

        register_published_outputs(&registry, &result).unwrap();

        let resolved = resolve_registered_output(
            &registry,
            artifact.path.to_str().unwrap(),
            &artifact.adapter_id,
        )
        .unwrap();
        assert_eq!(resolved.slicer, RegisteredSlicer::BambuStudio);
        validate_registered_output_identity(&resolved).unwrap();
        assert!(
            resolve_registered_output(
                &registry,
                artifact.path.to_str().unwrap(),
                "snapmaker-orca/wrong-adapter"
            )
            .unwrap_err()
            .contains("adapter does not match")
        );
        let unrelated = temporary.path().join("unrelated.3mf");
        fs::write(&unrelated, b"not published").unwrap();
        assert!(
            resolve_registered_output(&registry, unrelated.to_str().unwrap(), &artifact.adapter_id)
                .unwrap_err()
                .contains("current conversion result")
        );
    }

    #[test]
    fn quick_output_actions_reject_a_changed_published_file() {
        let temporary = tempfile::tempdir().unwrap();
        let target_directory = temporary.path().join("u1-direct");
        fs::create_dir(&target_directory).unwrap();
        let file_name = "fixture.3mf";
        let path = target_directory.join(file_name);
        fs::write(&path, b"published fixture").unwrap();
        let (byte_size, sha256) = hash_file(&path).unwrap();
        let mut artifact = published_fixture(temporary.path(), "u1-direct", file_name);
        artifact.target = "u1_direct".into();
        artifact.adapter_id = "snapmaker-orca/fixture".into();
        artifact.byte_size = byte_size;
        artifact.sha256 = sha256;
        let result = NativeConversionResult {
            adapter_id: "fixture".into(),
            output_directory: temporary.path().to_path_buf(),
            manifest_path: temporary.path().join("manifest.json"),
            report_path: temporary.path().join(PUBLISHED_CONVERSION_REPORT_FILE_NAME),
            artifacts: vec![artifact.clone()],
            excluded_source_units: Vec::new(),
            warnings_acknowledged: true,
            warnings: Vec::new(),
        };
        let registry = Mutex::new(HashMap::new());
        register_published_outputs(&registry, &result).unwrap();
        fs::write(&path, b"changed fixture!!").unwrap();

        let resolved = resolve_registered_output(
            &registry,
            artifact.path.to_str().unwrap(),
            &artifact.adapter_id,
        )
        .unwrap();
        assert!(
            validate_registered_output_identity(&resolved)
                .unwrap_err()
                .starts_with("output_action_file_changed:")
        );
    }

    #[test]
    fn published_actions_register_only_for_the_approved_cache_generation() {
        let temporary = tempfile::tempdir().unwrap();
        let target_directory = temporary.path().join("u1-direct");
        fs::create_dir(&target_directory).unwrap();
        let file_name = "fixture.3mf";
        let path = target_directory.join(file_name);
        fs::write(&path, b"published fixture").unwrap();
        let (byte_size, sha256) = hash_file(&path).unwrap();
        let mut artifact = published_fixture(temporary.path(), "u1-direct", file_name);
        artifact.target = "u1_cmyx_solid".into();
        artifact.adapter_id = "snapmaker-orca/fixture".into();
        artifact.byte_size = byte_size;
        artifact.sha256 = sha256;
        let result = NativeConversionResult {
            adapter_id: "fixture".into(),
            output_directory: temporary.path().to_path_buf(),
            manifest_path: temporary.path().join("manifest.json"),
            report_path: temporary.path().join(PUBLISHED_CONVERSION_REPORT_FILE_NAME),
            artifacts: vec![artifact],
            excluded_source_units: Vec::new(),
            warnings_acknowledged: true,
            warnings: Vec::new(),
        };
        let cache = ProjectCache::default();
        *cache.operation_generation.lock().unwrap() = 5;

        assert!(!register_published_outputs_if_current(&cache, 4, &result).unwrap());
        assert!(cache.published_outputs.lock().unwrap().is_empty());
        assert!(register_published_outputs_if_current(&cache, 5, &result).unwrap());
        assert_eq!(cache.published_outputs.lock().unwrap().len(), 1);

        invalidate_inventory_dependent_state(&cache).unwrap();
        assert!(cache.published_outputs.lock().unwrap().is_empty());
        assert_eq!(*cache.operation_generation.lock().unwrap(), 6);
    }

    #[test]
    fn source_hashing_observes_cooperative_cancellation() {
        let temporary = tempfile::NamedTempFile::new().unwrap();
        temporary.as_file().set_len(3 * 1024 * 1024).unwrap();
        let mut checkpoints = 0_usize;

        let error = hash_file_cancellable(temporary.path(), &mut || {
            checkpoints += 1;
            checkpoints >= 3
        })
        .unwrap_err();

        assert!(error.starts_with("conversion_cancelled:"));
        assert!(checkpoints >= 3);
    }

    #[test]
    fn source_identity_barrier_rejects_same_size_late_mutation() {
        let temporary = tempfile::NamedTempFile::new().unwrap();
        fs::write(temporary.path(), b"approved-source").unwrap();
        let (byte_size, sha256) = hash_file(temporary.path()).unwrap();
        let expected = InputIdentity { byte_size, sha256 };
        validate_input_identity_cancellable(temporary.path(), &expected, &mut || false).unwrap();

        fs::write(temporary.path(), b"mutated-source!").unwrap();

        let error = validate_input_identity_cancellable(temporary.path(), &expected, &mut || false)
            .unwrap_err();
        assert!(error.contains("changed after analysis"));
    }

    #[test]
    fn expired_preparation_is_rejected_before_it_is_taken() {
        let created_at = Instant::now()
            .checked_sub(CONVERSION_PREPARATION_TTL + Duration::from_secs(1))
            .expect("fixture instant");
        let cache = Mutex::new(Some(cached_preparation("expired-token", created_at)));

        let error = match take_prepared_conversion(&cache, "expired-token") {
            Ok(_) => panic!("an expired preparation must fail"),
            Err(error) => error,
        };

        assert!(error.contains("expired"));
        assert_eq!(
            cache
                .lock()
                .expect("preparation cache lock")
                .as_ref()
                .expect("expired preparation must be inspected before take")
                .token,
            "expired-token"
        );
    }

    #[test]
    fn active_conversion_can_be_cancelled_only_before_publication() {
        let control = MixedConversionControl::new();
        let conversion_id = control.conversion_id().to_owned();
        let active = Mutex::new(HashMap::from([(conversion_id.clone(), control)]));

        let accepted = cancel_active_conversion(&conversion_id, &active).unwrap();
        assert!(accepted.accepted);
        assert_eq!(accepted.state, Some(U1DirectConversionState::Cancelled));

        let repeated = cancel_active_conversion(&conversion_id, &active).unwrap();
        assert!(!repeated.accepted);
        assert_eq!(repeated.state, Some(U1DirectConversionState::Cancelled));

        let missing_id = uuid::Uuid::new_v4().hyphenated().to_string();
        let missing = cancel_active_conversion(&missing_id, &active).unwrap();
        assert!(!missing.accepted);
        assert_eq!(missing.state, None);
    }

    #[test]
    fn prepared_conversion_can_be_cancelled_before_worker_registration() {
        let prepared = cached_preparation("valid-token", Instant::now());
        let conversion_id = prepared.control.conversion_id().to_owned();
        let prepared = Mutex::new(Some(prepared));
        let active = Mutex::new(HashMap::new());

        let response =
            cancel_prepared_or_active_conversion(&conversion_id, &prepared, &active).unwrap();

        assert!(response.accepted);
        assert_eq!(response.state, Some(U1DirectConversionState::Cancelled));
        assert_eq!(
            prepared.lock().unwrap().as_ref().unwrap().control.state(),
            U1DirectConversionState::Cancelled
        );
    }

    #[test]
    fn cancelled_preflight_discards_only_its_matching_one_shot_token() {
        let candidate = cached_preparation("valid-token", Instant::now());
        let conversion_id = candidate.control.conversion_id().to_owned();
        let prepared = Mutex::new(Some(candidate));

        assert!(
            !discard_prepared_conversion_if_matches(&prepared, "wrong-token", &conversion_id)
                .unwrap()
        );
        assert!(prepared.lock().unwrap().is_some());
        assert!(
            discard_prepared_conversion_if_matches(&prepared, "valid-token", &conversion_id)
                .unwrap()
        );
        assert!(prepared.lock().unwrap().is_none());
    }

    #[test]
    fn startup_recovery_removes_completed_records_and_preserves_unknown_files() {
        let temporary = tempfile::tempdir().unwrap();
        let destination = temporary.path().join("destination");
        let registry = temporary.path().join("registry");
        fs::create_dir(&destination).unwrap();
        let control = U1DirectConversionControl::new();
        let record = control.recovery_record(&destination).unwrap();
        let mut record_value = serde_json::to_value(record).unwrap();
        record_value["ownerProcessId"] = serde_json::json!(u32::MAX);
        let record = serde_json::from_value(record_value).unwrap();
        let record_path = write_recovery_record(&registry, &record).unwrap();
        let unknown = registry.join("user-notes.txt");
        fs::write(&unknown, b"do not remove").unwrap();

        let report = cleanup_recovery_registry(&registry).unwrap();

        assert_eq!(
            report,
            RecoveryCleanupReport {
                removed_records: 1,
                active_records: 0,
                failed_records: 0,
            }
        );
        assert!(!record_path.exists());
        assert_eq!(fs::read(unknown).unwrap(), b"do not remove");
        assert!(destination.is_dir());
    }

    #[test]
    fn exports_a_valid_plan_atomically() {
        let directory = tempfile::tempdir().expect("temporary export directory");
        let destination = directory.path().join("fixture-print-plan.json");

        let contents = r#"{"summary":{"sourceHash":"sha256:fixture"},"plates":[],"batches":[],"t4SwapCount":0,"a1SpoolChangeCount":0,"planReady":true}"#;
        let authoritative_plan = cached_plan(contents);
        write_plan_json(
            &destination,
            contents,
            "sha256:fixture",
            &authoritative_plan,
        )
        .expect("export must succeed");

        assert_eq!(
            std::fs::read_to_string(destination).expect("read exported plan"),
            authoritative_plan.contents
        );
    }

    #[test]
    fn accepts_javascript_number_formatting_without_trusting_other_mutations() {
        let directory = tempfile::tempdir().expect("temporary export directory");
        let destination = directory.path().join("fixture-print-plan.json");
        let authoritative = r#"{"summary":{"sourceHash":"sha256:fixture"},"plates":[{"deltaE":0.0,"ratio":1e2}],"batches":[],"t4SwapCount":0,"a1SpoolChangeCount":0,"planReady":true}"#;
        let javascript = r#"{"summary":{"sourceHash":"sha256:fixture"},"plates":[{"deltaE":0,"ratio":100}],"batches":[],"t4SwapCount":0,"a1SpoolChangeCount":0,"planReady":true}"#;
        let authoritative_plan = cached_plan(authoritative);

        write_plan_json(
            &destination,
            javascript,
            "sha256:fixture",
            &authoritative_plan,
        )
        .expect("equivalent JavaScript number formatting must succeed");

        assert_eq!(
            std::fs::read_to_string(destination).expect("read exported plan"),
            authoritative_plan.contents
        );
    }

    #[test]
    fn rejects_non_json_plan_exports() {
        let directory = tempfile::tempdir().expect("temporary export directory");
        let destination = directory.path().join("fixture.3mf");

        assert!(
            write_plan_json(&destination, "{}", "sha256:fixture", &cached_plan("{}"))
                .expect_err("wrong extension must fail")
                .contains(".json extension")
        );
    }

    #[test]
    fn rejects_a_mutated_frontend_plan() {
        let directory = tempfile::tempdir().expect("temporary export directory");
        let destination = directory.path().join("fixture-print-plan.json");
        let authoritative = r#"{"summary":{"sourceHash":"sha256:fixture"},"plates":[{"id":"plate-1"}],"batches":[],"t4SwapCount":0,"a1SpoolChangeCount":0,"planReady":true}"#;
        let mutated = r#"{"summary":{"sourceHash":"sha256:fixture"},"plates":[],"batches":[],"t4SwapCount":0,"a1SpoolChangeCount":0,"planReady":true}"#;

        let error = write_plan_json(
            &destination,
            mutated,
            "sha256:fixture",
            &cached_plan(authoritative),
        )
        .expect_err("a client-side mutation must fail");

        assert!(error.contains("differs from the latest backend-validated plan"));
        assert!(!destination.exists());
    }

    #[test]
    fn rejects_an_empty_frontend_plan() {
        let directory = tempfile::tempdir().expect("temporary export directory");
        let destination = directory.path().join("fixture-print-plan.json");
        let authoritative = r#"{"summary":{"sourceHash":"sha256:fixture"},"plates":[],"batches":[],"t4SwapCount":0,"a1SpoolChangeCount":0,"planReady":true}"#;

        let error = write_plan_json(
            &destination,
            "{}",
            "sha256:fixture",
            &cached_plan(authoritative),
        )
        .expect_err("an empty client plan must fail");

        assert!(error.contains("differs from the latest backend-validated plan"));
        assert!(!destination.exists());
    }
}
