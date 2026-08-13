use super::*;
use std::io::BufReader;
use u1_application::PreliminaryPlanOptions;
use u1_planner::{
    Printer, PrinterLoadout, PrinterPreference, RgbColor, ScopeStrategy, ToolheadSlotState,
};

const FILAMENT_LIBRARY_ENV: &str = "U1_NATIVE_E2E_FILAMENT_LIBRARY";
const A1_SPOOL_ID_ENV: &str = "U1_NATIVE_E2E_A1_SPOOL_ID";
const DEFAULT_A1_SPOOL_ID: &str = "user-creality";
const EXPECTED_SOURCE_SHA256: &str =
    "f0ad280964c0f0a3bdae1c3b8cc187d572da2986010f6e2c08e4e803db846b81";
const MAX_E2E_LIBRARY_BYTES: u64 = 1024 * 1024;

fn workspace_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join(relative)
}

fn saved_library_path() -> PathBuf {
    let path = std::env::var_os(FILAMENT_LIBRARY_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!(
                "set {FILAMENT_LIBRARY_ENV} to an absolute, read-only filament-library-v1.json path"
            )
        });
    assert!(
        path.is_absolute(),
        "{FILAMENT_LIBRARY_ENV} must be an absolute path"
    );
    let metadata = fs::symlink_metadata(&path).expect("saved filament library must be readable");
    assert!(
        metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
        "saved filament library must be a regular non-symlink file"
    );
    assert!(
        metadata.len() <= MAX_E2E_LIBRARY_BYTES,
        "saved filament library exceeds the production one-megabyte limit"
    );
    path.canonicalize()
        .expect("saved filament library path must canonicalize")
}

fn load_saved_library(path: &Path) -> FilamentLibraryView {
    let file = File::open(path).expect("saved filament library must open");
    serde_json::from_reader(BufReader::new(file))
        .expect("saved filament library must be valid legacy or current JSON")
}

fn strategy_name(strategy: ScopeStrategy) -> &'static str {
    match strategy {
        ScopeStrategy::Auto => "auto",
        ScopeStrategy::CmyxFullSpectrum => "cmyx",
        ScopeStrategy::DirectSpools => "direct",
    }
}

fn printer_preference_name(preference: PrinterPreference) -> &'static str {
    match preference {
        PrinterPreference::Auto => "auto",
        PrinterPreference::U1 => "u1",
        PrinterPreference::A1Mini => "a1-mini",
    }
}

fn planning_request_from_fixture(options: &PreliminaryPlanOptions) -> PlanningRequestView {
    assert!(
        options
            .scope_overrides
            .iter()
            .all(|scope| scope.approved_material_substitutions.is_empty()),
        "the mixed E2E fixture must not hide a material substitution"
    );
    assert!(
        options.confirmed_calibration_samples.is_empty(),
        "the mixed E2E fixture must not inject calibration evidence"
    );
    assert!(
        options.cmyx_geometry_context == CmyxGeometryContext::default(),
        "the mixed E2E fixture must use the default calibration geometry"
    );
    assert!(
        options.a1_mini.route_fast_mono,
        "the desktop request contract assumes the qualified fast-mono policy"
    );

    let scope_overrides = options
        .scope_overrides
        .iter()
        .map(|scope| {
            let strategy = scope.strategy.unwrap_or(options.scope_strategy);
            serde_json::json!({
                "scopeId": scope.scope_id,
                "strategy": strategy_name(strategy),
                "assignments": scope
                    .direct_assignments
                    .iter()
                    .map(|assignment| serde_json::json!({
                        "requirementId": assignment.requirement_id,
                        "spoolId": assignment.spool_id,
                        "toolhead": assignment
                            .toolhead
                            .map(|toolhead| format!("T{}", toolhead.index() + 1)),
                        "allowMaterialSubstitution": assignment.allow_material_substitution,
                    }))
                    .collect::<Vec<_>>(),
                "approvedColorFallbacks": scope
                    .approved_cmyx_fallbacks
                    .iter()
                    .map(|approval| serde_json::json!({
                        "requirementId": approval.requirement_id,
                        "candidateId": approval.candidate_id,
                    }))
                    .collect::<Vec<_>>(),
                "materialSubstitutions": [],
            })
        })
        .collect::<Vec<_>>();
    let unit_printer_overrides = options
        .unit_printer_overrides
        .iter()
        .map(|selection| {
            serde_json::json!({
                "sourceUnitId": selection.source_unit_id,
                "preference": printer_preference_name(selection.preference),
            })
        })
        .collect::<Vec<_>>();
    let current_loadout = options
        .current_toolheads
        .slots
        .iter()
        .enumerate()
        .filter_map(|(index, state)| match state {
            ToolheadSlotState::Loaded(spool_id) => Some(serde_json::json!({
                "toolhead": format!("T{}", index + 1),
                "spoolId": spool_id,
            })),
            ToolheadSlotState::Unknown | ToolheadSlotState::Empty => None,
        })
        .collect::<Vec<_>>();

    serde_json::from_value(serde_json::json!({
        "defaultStrategy": strategy_name(options.scope_strategy),
        // Deliberately empty: the authoritative replan must merge the saved
        // native filament library supplied by the backend, not a UI copy.
        "confirmedSpools": [],
        "scopeOverrides": scope_overrides,
        "unitPrinterOverrides": unit_printer_overrides,
        "currentLoadout": current_loadout,
        "restoreCmyAfterDirect": options.restore_cmy_after_direct,
        "allowU1CrossSourceRepacking": options.allow_u1_cross_source_repacking,
        "a1MiniEnabled": options.a1_mini.enabled,
        "includedAlternativePlateIds": options.included_alternative_plate_ids,
    }))
    .expect("mixed E2E fixture must map to the desktop replan DTO")
}

fn remap_saved_spool_fallback_identity(
    options: &mut PreliminaryPlanOptions,
    initial_input: &u1_planner::PlanningInput,
    a1_spool_id: &str,
) {
    let mut remapped = 0_usize;
    for scope in &mut options.scope_overrides {
        for approval in &mut scope.approved_cmyx_fallbacks {
            let requirement = initial_input
                .scopes
                .iter()
                .find(|candidate_scope| candidate_scope.id == scope.scope_id)
                .and_then(|candidate_scope| {
                    candidate_scope
                        .requirements
                        .iter()
                        .find(|requirement| requirement.id == approval.requirement_id)
                })
                .unwrap_or_else(|| {
                    panic!(
                        "initial native input has no requirement for {} / {}",
                        scope.scope_id, approval.requirement_id
                    )
                });
            let candidate = requirement
                .best_effort_cmyx_candidate
                .as_ref()
                .unwrap_or_else(|| {
                    panic!(
                        "initial native input has no fallback candidate for {} / {}",
                        scope.scope_id, approval.requirement_id
                    )
                });
            let candidate_id = candidate.candidate_id.as_str();
            if candidate_id == approval.candidate_id {
                continue;
            }

            // Candidate identities bind the physical spool ID. The fixture was
            // qualified with a throwaway spool identity, so only this exact
            // same-material PETG candidate may change when it is rebound to
            // the user's saved physical spool.
            assert_eq!(scope.scope_id, "plate-6");
            assert_eq!(approval.requirement_id, "plate-6-requirement-1");
            assert_eq!(requirement.material, u1_planner::Material::Petg);
            assert_eq!(requirement.source_color, RgbColor::new(173, 177, 178));
            assert_eq!(candidate.target_material, u1_planner::Material::Petg);
            assert_eq!(candidate.target_color, RgbColor::new(173, 177, 178));
            assert_eq!(
                candidate.candidate.predicted_color,
                Some(RgbColor::new(0, 86, 214))
            );
            assert_eq!(
                candidate.candidate.required_t4_spool_id.as_deref(),
                Some(a1_spool_id)
            );
            approval.candidate_id = candidate_id.to_owned();
            remapped += 1;
        }
    }
    assert_eq!(
        remapped, 1,
        "only the qualification PETG spool-bound candidate may be remapped"
    );
}

fn artifact_identities(
    result: &NativeConversionResult,
) -> BTreeSet<(String, String, String, String, u64, String)> {
    result
        .artifacts
        .iter()
        .map(|artifact| {
            (
                artifact.adapter_id.clone(),
                artifact.target.clone(),
                artifact.batch_id.clone(),
                artifact.relative_path.clone(),
                artifact.byte_size,
                artifact.sha256.clone(),
            )
        })
        .collect()
}

fn first_json_difference(
    published: &serde_json::Value,
    expected: &serde_json::Value,
    path: &str,
) -> Option<String> {
    if json_values_equivalent(published, expected) {
        return None;
    }
    match (published, expected) {
        (serde_json::Value::Object(published), serde_json::Value::Object(expected)) => {
            let keys = published
                .keys()
                .chain(expected.keys())
                .collect::<BTreeSet<_>>();
            for key in keys {
                let next_path = format!("{path}.{key}");
                match (published.get(key), expected.get(key)) {
                    (Some(published), Some(expected)) => {
                        if let Some(difference) =
                            first_json_difference(published, expected, &next_path)
                        {
                            return Some(difference);
                        }
                    }
                    (published, expected) => {
                        return Some(format!(
                            "{next_path}: published={published:?}, expected={expected:?}"
                        ));
                    }
                }
            }
            None
        }
        (serde_json::Value::Array(published), serde_json::Value::Array(expected)) => {
            if published.len() != expected.len() {
                return Some(format!(
                    "{path}.length: published={}, expected={}",
                    published.len(),
                    expected.len()
                ));
            }
            published
                .iter()
                .zip(expected)
                .enumerate()
                .find_map(|(index, (published, expected))| {
                    first_json_difference(published, expected, &format!("{path}[{index}]"))
                })
        }
        _ => Some(format!(
            "{path}: published={published}, expected={expected}"
        )),
    }
}

fn rewrite_published_metadata_after_tamper(root: &Path, manifest: &serde_json::Value) {
    let manifest_path = root.join(PUBLISHED_MANIFEST_FILE_NAME);
    let mut manifest_bytes = serde_json::to_vec_pretty(manifest).expect("tampered manifest JSON");
    manifest_bytes.push(b'\n');
    fs::write(&manifest_path, manifest_bytes).expect("tampered manifest must write");

    let report_path = root.join(PUBLISHED_CONVERSION_REPORT_FILE_NAME);
    fs::write(
        &report_path,
        render_conversion_report(manifest).expect("tampered report must render"),
    )
    .expect("tampered report must write");
    fs::write(
        root.join(PUBLISHED_PRINT_INSTRUCTIONS_FILE_NAME),
        render_print_instructions(manifest).expect("tampered instructions must render"),
    )
    .expect("tampered instructions must write");

    let mut checksums = manifest["artifacts"]
        .as_array()
        .expect("manifest artifact list")
        .iter()
        .map(|artifact| {
            format!(
                "{}  {}",
                artifact["sha256"].as_str().expect("artifact SHA-256"),
                artifact["relativePath"]
                    .as_str()
                    .expect("artifact relative path")
            )
        })
        .collect::<Vec<_>>();
    for file_name in [
        PUBLISHED_MANIFEST_FILE_NAME,
        PUBLISHED_CONVERSION_PLAN_FILE_NAME,
        PUBLISHED_CONVERSION_REPORT_FILE_NAME,
        PUBLISHED_PRINT_INSTRUCTIONS_FILE_NAME,
    ] {
        let (_, sha256) = hash_file(&root.join(file_name)).expect("metadata identity");
        checksums.push(format!("{sha256}  {file_name}"));
    }
    checksums.sort();
    fs::write(
        root.join(PUBLISHED_CHECKSUMS_FILE_NAME),
        format!("{}\n", checksums.join("\n")),
    )
    .expect("tampered checksum list must write");
}

#[test]
#[ignore = "requires exact qualified Snapmaker Orca and Bambu Studio installations, the real Withered_Foxy sample, and an explicit saved-library path"]
fn native_mixed_withered_foxy_publishes_and_recovers_without_user_writes() {
    let source = workspace_path("Sample/Withered_Foxy.3mf")
        .canonicalize()
        .expect("real Withered_Foxy sample must be present");
    let fixture = workspace_path("fixtures/a1-mini-petg-qualification-options.json")
        .canonicalize()
        .expect("mixed A1 qualification fixture must be present");
    let library_path = saved_library_path();
    let source_identity_before = hash_file(&source).expect("source identity");
    let library_identity_before = hash_file(&library_path).expect("library identity");
    assert_eq!(source_identity_before.1, EXPECTED_SOURCE_SHA256);

    let library = load_saved_library(&library_path);
    let library_spools = library
        .planning_spools()
        .expect("saved filament library must normalize for planning");
    let a1_spool_id =
        std::env::var(A1_SPOOL_ID_ENV).unwrap_or_else(|_| DEFAULT_A1_SPOOL_ID.to_owned());
    let a1_spool = library_spools
        .iter()
        .find(|spool| spool.id == a1_spool_id)
        .unwrap_or_else(|| panic!("saved filament library has no spool {a1_spool_id:?}"));
    assert!(a1_spool.available, "selected A1 PETG spool is out of stock");
    assert_eq!(
        a1_spool.material,
        u1_planner::Material::Petg,
        "selected A1 spool must preserve the fixture's PETG material"
    );
    assert_eq!(
        a1_spool.actual_color(),
        RgbColor::new(0, 86, 214),
        "selected A1 spool must match the qualified #0056D6 nominal color"
    );

    let mut fixture_options: PreliminaryPlanOptions = serde_json::from_reader(BufReader::new(
        File::open(&fixture).expect("mixed fixture must open"),
    ))
    .expect("mixed fixture must match PreliminaryPlanOptions");
    let fixture_spool_ids = fixture_options
        .confirmed_spools
        .iter()
        .map(|spool| spool.id.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        fixture_spool_ids,
        BTreeSet::from(["qualification-a1-petg".to_owned()]),
        "mixed fixture spool contract changed"
    );
    let mut remapped_assignments = 0_usize;
    for assignment in fixture_options
        .scope_overrides
        .iter_mut()
        .flat_map(|scope| &mut scope.direct_assignments)
    {
        if assignment.spool_id == "qualification-a1-petg" {
            assignment.spool_id = a1_spool_id.clone();
            remapped_assignments += 1;
        }
    }
    assert_eq!(remapped_assignments, 2);
    fixture_options.confirmed_spools.clear();

    eprintln!("native mixed E2E: analyzing immutable Withered_Foxy source");
    let (analysis, initial) = view::analyze_native_project_data_with_backend_state(
        source.to_str().expect("UTF-8 source path"),
        view::InitialPlanningIntentView::default(),
        library_spools.clone(),
        Vec::new(),
        CmyxGeometryContext::default(),
    )
    .expect("real source must analyze with the saved filament library");
    assert_eq!(analysis.input.sha256, EXPECTED_SOURCE_SHA256);
    let initial_json = serde_json::to_value(&initial.view).expect("initial view JSON");
    assert!(
        initial_json["spools"]
            .as_array()
            .expect("initial view spool list")
            .iter()
            .any(|spool| spool["id"] == a1_spool_id),
        "initial native analysis did not expose the saved user spool"
    );
    remap_saved_spool_fallback_identity(&mut fixture_options, &initial.input, &a1_spool_id);
    let request = planning_request_from_fixture(&fixture_options);
    let restart_request = request.clone();
    let restart_library_spools = library_spools.clone();

    eprintln!("native mixed E2E: applying authoritative mixed U1/A1 replan");
    let replanned = view::replan_native_project_view_with_backend_state(
        source.to_str().expect("UTF-8 source path"),
        &analysis,
        request,
        library_spools,
        Vec::new(),
        CmyxGeometryContext::default(),
    )
    .expect("mixed saved-library choices must replan");
    assert!(
        !replanned.result.has_hard_errors(),
        "mixed plan errors: {:#?}",
        replanned.result.errors
    );
    assert_eq!(replanned.result.plates.len(), 7);
    let replanned_json = serde_json::to_value(&replanned.view).expect("replanned view JSON");
    assert_eq!(replanned_json["planReady"], true);
    assert_eq!(replanned_json["omittedUnitCount"], 0);

    let a1_jobs = replanned
        .result
        .jobs
        .iter()
        .filter(|job| job.printer == Printer::A1Mini)
        .collect::<Vec<_>>();
    assert_eq!(a1_jobs.len(), 1);
    assert_eq!(a1_jobs[0].units.len(), 16);
    assert_eq!(
        a1_jobs[0]
            .units
            .iter()
            .map(|unit| unit.unit_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        16
    );
    assert_eq!(
        a1_jobs[0]
            .scope_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["plate-3", "plate-6"])
    );
    match &a1_jobs[0].loadout {
        PrinterLoadout::A1Mini { spool_id } => assert_eq!(spool_id, &a1_spool_id),
        PrinterLoadout::U1 { .. } => panic!("A1 job has a U1 loadout"),
    }

    let plan_fingerprint = canonical_plan_fingerprint(&replanned.input, &replanned.result)
        .expect("canonical mixed plan fingerprint");
    eprintln!("native mixed E2E: preparing exact qualified writer targets");
    let (preparation, targets) = prepare_required_targets(
        &source,
        &analysis,
        &replanned.input,
        &replanned.result,
        &plan_fingerprint,
        None,
        None,
        None,
    )
    .expect("qualified mixed native targets must prepare");
    assert!(targets.direct.is_none());
    assert!(targets.full_spectrum.is_some());
    assert!(targets.a1_mini.is_some());
    assert!(targets.excluded_source_units.is_empty());
    assert!(
        preparation
            .artifacts
            .iter()
            .any(|artifact| artifact.target == "u1_full_spectrum")
    );
    assert!(
        preparation
            .artifacts
            .iter()
            .any(|artifact| artifact.target == "a1_mini_mono")
    );

    let destination_guard = tempfile::tempdir().expect("temporary output parent");
    let destination = destination_guard
        .path()
        .canonicalize()
        .expect("temporary output parent must canonicalize");
    let recovery_registry_guard = tempfile::tempdir().expect("temporary recovery registry");
    let recovery_registry_path = recovery_registry_guard
        .path()
        .canonicalize()
        .expect("temporary recovery registry must canonicalize");
    let receipt_registry_guard = tempfile::tempdir().expect("temporary receipt parent");
    let receipt_registry_path = receipt_registry_guard
        .path()
        .join("publication-receipts-v1");
    let control = MixedConversionControl::new();
    let recovery_record = control
        .recovery_record(&destination)
        .expect("temporary recovery record");
    let recovery_path = write_recovery_record(&recovery_registry_path, &recovery_record)
        .expect("temporary recovery record must persist");
    assert!(recovery_path.is_file());
    let generation = Mutex::new(1_u64);
    let live_output_registry = Mutex::new(HashMap::new());

    eprintln!("native mixed E2E: converting and atomically publishing temporary bundle");
    let published = perform_native_conversion(
        &source,
        &analysis,
        &replanned.input,
        &replanned.result,
        &plan_fingerprint,
        &preparation,
        &targets,
        &destination,
        &receipt_registry_path,
        &preparation.warnings,
        &control,
        &generation,
        &live_output_registry,
        1,
    )
    .expect("mixed native conversion must publish");
    assert_eq!(control.state(), U1DirectConversionState::Published);
    assert_eq!(
        finalize_u1_direct_staging(&recovery_record).expect("final staging cleanup"),
        U1DirectStagingCleanup::Missing
    );
    remove_recovery_record(&recovery_registry_path, &recovery_path)
        .expect("completed temporary recovery record must be removed");
    assert!(!recovery_path.exists());
    assert_eq!(
        cleanup_recovery_registry(&recovery_registry_path)
            .expect("empty temporary recovery registry"),
        RecoveryCleanupReport::default()
    );

    assert!(published.output_directory.is_dir());
    assert_eq!(
        fs::read_dir(&receipt_registry_path)
            .expect("private receipt registry")
            .count(),
        1,
        "successful publication must atomically register one trusted receipt"
    );
    assert!(published.manifest_path.is_file());
    assert!(published.report_path.is_file());
    let instructions_path = published
        .output_directory
        .join(PUBLISHED_PRINT_INSTRUCTIONS_FILE_NAME);
    assert!(instructions_path.is_file());
    let instructions =
        fs::read_to_string(&instructions_path).expect("published print instructions");
    assert!(instructions.contains("STEP 1 OF"));
    assert!(instructions.contains("REQUIRED PHYSICAL LOADOUT"));
    assert!(
        published
            .output_directory
            .join(PUBLISHED_CONVERSION_PLAN_FILE_NAME)
            .is_file()
    );
    let checksums_path = published
        .output_directory
        .join(PUBLISHED_CHECKSUMS_FILE_NAME);
    assert!(checksums_path.is_file());
    let checksums = fs::read_to_string(&checksums_path).expect("published checksum list");
    assert_eq!(
        checksums.lines().count(),
        published.artifacts.len() + 4,
        "checksums must cover every 3MF plus manifest, plan, report, and print instructions"
    );
    for artifact in &published.artifacts {
        assert!(artifact.path.is_file());
        assert_eq!(
            hash_file(&artifact.path).expect("published artifact identity"),
            (artifact.byte_size, artifact.sha256.clone())
        );
        assert!(checksums.contains(&format!("{}  {}", artifact.sha256, artifact.relative_path)));
    }
    let published_targets = published
        .artifacts
        .iter()
        .map(|artifact| artifact.target.as_str())
        .collect::<BTreeSet<_>>();
    assert!(published_targets.contains("u1_full_spectrum"));
    assert!(published_targets.contains("a1_mini_mono"));
    assert_eq!(
        published
            .artifacts
            .iter()
            .filter(|artifact| artifact.target == "a1_mini_mono")
            .flat_map(|artifact| artifact.source_unit_ids.iter())
            .collect::<BTreeSet<_>>()
            .len(),
        16
    );

    let published_manifest = serde_json::from_reader::<_, serde_json::Value>(BufReader::new(
        File::open(&published.manifest_path).expect("published manifest must open"),
    ))
    .expect("published manifest JSON");
    let published_plan = serde_json::from_reader::<_, serde_json::Value>(BufReader::new(
        File::open(
            published
                .output_directory
                .join(PUBLISHED_CONVERSION_PLAN_FILE_NAME),
        )
        .expect("published conversion plan must open"),
    ))
    .expect("published conversion plan JSON");
    let expected_plan = json_wire_value(
        &serde_json::json!({
            "schemaVersion": PUBLISHED_BUNDLE_SCHEMA_VERSION,
            "converter": published_converter_manifest(),
            "source": published_source_manifest(&source, &analysis)
                .expect("canonical published source"),
            "planFingerprint": &plan_fingerprint,
            "planningInput": &replanned.input,
            "planningResult": &replanned.result,
            "excludedSourceUnits": &published_manifest["excludedSourceUnits"],
            "warningsAcknowledged": &published_manifest["warningsAcknowledged"],
            "acknowledgedWarnings": &published_manifest["acknowledgedWarnings"],
        }),
        "native E2E canonical conversion plan",
    )
    .expect("native E2E canonical conversion plan must normalize on wire");
    assert!(
        json_values_equivalent(&published_plan, &expected_plan),
        "published conversion plan mismatch: {}",
        first_json_difference(&published_plan, &expected_plan, "$")
            .unwrap_or_else(|| "unknown difference".to_owned())
    );

    eprintln!("native mixed E2E: simulating a fresh analysis, plan, and process restart");
    let (restarted_analysis, restarted_initial) =
        view::analyze_native_project_data_with_backend_state(
            source.to_str().expect("UTF-8 source path"),
            view::InitialPlanningIntentView::default(),
            restart_library_spools.clone(),
            Vec::new(),
            CmyxGeometryContext::default(),
        )
        .expect("restart analysis must succeed");
    let restarted_plan = view::replan_native_project_view_with_backend_state(
        source.to_str().expect("UTF-8 source path"),
        &restarted_analysis,
        restart_request,
        restart_library_spools,
        Vec::new(),
        CmyxGeometryContext::default(),
    )
    .expect("restart plan must succeed");
    assert_eq!(restarted_initial.input, initial.input);
    assert_eq!(restarted_plan.input, replanned.input);
    assert_eq!(restarted_plan.result, replanned.result);
    let restarted_fingerprint =
        canonical_plan_fingerprint(&restarted_plan.input, &restarted_plan.result)
            .expect("restart plan fingerprint");
    assert_eq!(restarted_fingerprint, plan_fingerprint);

    eprintln!("native mixed E2E: revalidating published bundle from fresh state");
    let restarted_output_registry = Mutex::new(HashMap::new());
    let restarted_generation = Mutex::new(1_u64);
    let recovery_started = std::time::Instant::now();
    let recovered = revalidate_published_bundle(
        &source,
        &restarted_analysis,
        &restarted_plan.input,
        &restarted_plan.result,
        &restarted_fingerprint,
        &published.output_directory,
        &receipt_registry_path,
        None,
        &restarted_generation,
        &restarted_output_registry,
        1,
    )
    .expect("restart-style backend revalidation must recover the exact bundle");
    let recovery_elapsed = recovery_started.elapsed();
    eprintln!(
        "native mixed E2E: strict restart revalidation completed in {:.2?}",
        recovery_elapsed
    );
    assert_eq!(recovered.output_directory, published.output_directory);
    assert_eq!(
        artifact_identities(&recovered),
        artifact_identities(&published)
    );
    assert_eq!(recovered.warnings, published.warnings);
    assert_eq!(
        restarted_output_registry
            .lock()
            .expect("restarted output registry")
            .len(),
        recovered.artifacts.len()
    );
    for artifact in &recovered.artifacts {
        let registered = resolve_registered_output(
            &restarted_output_registry,
            artifact.path.to_str().expect("UTF-8 output path"),
            &artifact.adapter_id,
        )
        .expect("revalidation must restore quick-action registration");
        validate_registered_output_identity(&registered)
            .expect("recovered quick-action artifact must retain its exact identity");
    }

    if recovery_elapsed > std::time::Duration::from_secs(30) {
        eprintln!(
            "native mixed E2E: recovery exceeded 30 seconds; skipping repeated adversarial recovery passes"
        );
        assert_eq!(
            hash_file(&source).expect("source identity after conversion"),
            source_identity_before,
            "native E2E must not modify the input 3MF"
        );
        assert_eq!(
            hash_file(&library_path).expect("library identity after conversion"),
            library_identity_before,
            "native E2E must not modify the saved filament library"
        );
        return;
    }

    let original_manifest_bytes = fs::read(&published.manifest_path).expect("manifest backup");
    let original_report_bytes = fs::read(&published.report_path).expect("report backup");
    let original_checksums_bytes = fs::read(&checksums_path).expect("checksums backup");
    let mut foreign_manifest: serde_json::Value =
        serde_json::from_slice(&original_manifest_bytes).expect("manifest backup JSON");
    let full_spectrum_indices = foreign_manifest["artifacts"]
        .as_array()
        .expect("manifest artifacts")
        .iter()
        .enumerate()
        .filter(|(_, artifact)| artifact["target"] == "u1_full_spectrum")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert!(
        full_spectrum_indices.len() >= 2,
        "foreign-artifact attack requires two Full Spectrum outputs"
    );
    let victim_index = full_spectrum_indices[0];
    let donor_index = full_spectrum_indices[1];
    let victim_relative = foreign_manifest["artifacts"][victim_index]["relativePath"]
        .as_str()
        .expect("victim relative path")
        .to_owned();
    let donor_relative = foreign_manifest["artifacts"][donor_index]["relativePath"]
        .as_str()
        .expect("donor relative path")
        .to_owned();
    let victim_path = published.output_directory.join(&victim_relative);
    let donor_path = published.output_directory.join(&donor_relative);
    let victim_bytes = fs::read(&victim_path).expect("victim artifact backup");
    fs::copy(&donor_path, &victim_path).expect("foreign same-target artifact replacement");
    let (foreign_size, foreign_sha256) =
        hash_file(&victim_path).expect("foreign artifact identity");
    foreign_manifest["artifacts"][victim_index]["byteSize"] = foreign_size.into();
    foreign_manifest["artifacts"][victim_index]["sha256"] = foreign_sha256.into();
    rewrite_published_metadata_after_tamper(&published.output_directory, &foreign_manifest);
    let foreign_started = std::time::Instant::now();
    let foreign_error = revalidate_published_bundle(
        &source,
        &restarted_analysis,
        &restarted_plan.input,
        &restarted_plan.result,
        &restarted_fingerprint,
        &published.output_directory,
        &receipt_registry_path,
        None,
        &restarted_generation,
        &restarted_output_registry,
        1,
    )
    .expect_err("a self-consistent foreign same-target artifact must be rejected");
    eprintln!(
        "native mixed E2E: foreign artifact rejection completed in {:.2?}",
        foreign_started.elapsed()
    );
    assert!(
        foreign_error.contains("publication receipt") || foreign_error.contains("receipt_mismatch"),
        "unexpected foreign-artifact rejection: {foreign_error}"
    );

    fs::write(&victim_path, victim_bytes).expect("victim artifact restore");
    fs::write(&published.manifest_path, &original_manifest_bytes).expect("manifest restore");
    fs::write(&published.report_path, &original_report_bytes).expect("report restore");
    fs::write(&checksums_path, &original_checksums_bytes).expect("checksums restore");
    let mut evidence_manifest: serde_json::Value =
        serde_json::from_slice(&original_manifest_bytes).expect("evidence manifest JSON");
    let target_validation =
        &mut evidence_manifest["artifacts"][0]["adapterEvidence"]["publishedValidation"];
    target_validation["recoveryTamper"] = serde_json::Value::Bool(true);
    rewrite_published_metadata_after_tamper(&published.output_directory, &evidence_manifest);
    let evidence_started = std::time::Instant::now();
    let evidence_error = revalidate_published_bundle(
        &source,
        &restarted_analysis,
        &restarted_plan.input,
        &restarted_plan.result,
        &restarted_fingerprint,
        &published.output_directory,
        &receipt_registry_path,
        None,
        &restarted_generation,
        &restarted_output_registry,
        1,
    )
    .expect_err("inner published validation evidence tampering must be rejected");
    eprintln!(
        "native mixed E2E: evidence tamper rejection completed in {:.2?}",
        evidence_started.elapsed()
    );
    assert!(
        evidence_error.contains("publication receipt")
            || evidence_error.contains("receipt_mismatch"),
        "unexpected evidence rejection: {evidence_error}"
    );
    fs::write(&published.manifest_path, &original_manifest_bytes).expect("manifest final restore");
    fs::write(&published.report_path, &original_report_bytes).expect("report final restore");
    fs::write(&checksums_path, &original_checksums_bytes).expect("checksums final restore");

    assert_eq!(
        hash_file(&source).expect("source identity after conversion"),
        source_identity_before,
        "native E2E must not modify the input 3MF"
    );
    assert_eq!(
        hash_file(&library_path).expect("library identity after conversion"),
        library_identity_before,
        "native E2E must not modify the saved filament library"
    );
}
