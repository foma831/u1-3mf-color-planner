use super::*;

#[test]
fn pva_diagnostic_geometry_covers_each_physical_failure_signal() {
    let root_model = String::from_utf8(pva_diagnostic_model()).unwrap();
    assert!(root_model.contains("/3D/Objects/pva-diagnostic.model"));
    assert!(root_model.contains("BambuStudio-2.3.6"));

    let model = String::from_utf8(pva_diagnostic_object_model()).unwrap();
    assert_eq!(model.matches("<vertex ").count(), 88);
    assert_eq!(model.matches("<triangle ").count(), 132);
    assert!(model.contains("PVA drying and stringing diagnostic"));
    assert!(model.contains("x=\"166.000\""));
    assert!(model.contains("z=\"16.000\""));

    let settings = String::from_utf8(pva_diagnostic_model_settings()).unwrap();
    assert!(settings.contains("key=\"extruder\" value=\"4\""));
    assert!(settings.contains("<part id=\"1\" subtype=\"normal_part\">"));
    assert!(settings.contains("key=\"identify_id\" value=\"91004001\""));
}

#[test]
fn pva_diagnostic_disables_the_unnecessary_prime_tower() {
    let settings = serde_json::to_vec(&serde_json::json!({
        "enable_prime_tower": "1",
        "layer_height": "0.2"
    }))
    .unwrap();
    let configured = configure_pva_diagnostic_project_settings(settings).unwrap();
    let configured: serde_json::Value = serde_json::from_slice(&configured).unwrap();
    assert_eq!(configured["enable_prime_tower"], "0");
    assert_eq!(configured["layer_height"], "0.2");
}

use quick_xml::events::BytesStart;
use serde_json::json;
use std::io::{Cursor, Read};
use tempfile::TempDir;
use u1_planner::{
    BoundsMm, CmySetup, CmyxColorCandidate, CmyxRecipe, ColorConfidence, CurrentToolheadState,
    Estimate, MappingStatus, PackingStatus, PlannerConfig, PlanningInput, PlanningResult,
    PrintScope, PrinterPreference, ScopeStrategy, SetupAction, SetupActionKind, SetupPhase,
    SourceToActualMapping, ToolheadSlotState, U1Loadout,
};
use u1_three_mf::{AxisAlignedBounds, ColorClassification, PaintNode, PlateAnalysis};

#[test]
fn full_spectrum_footprints_allow_planner_epsilon_at_touching_edges() {
    // Reproduces the Springtrap cross-source packing boundary: the first
    // rectangle's calculated right edge differs from the second rectangle's
    // left edge only by floating-point roundoff (~1.4e-14 mm).
    let touching = vec![
        (
            "source-build-item-18".to_owned(),
            [86.66644321615406, 1.0, 125.07314407715405, 75.022698845],
        ),
        (
            "source-build-item-20".to_owned(),
            [125.07314407715404, 1.0, 147.39017595565064, 56.354063076736],
        ),
    ];

    validate_full_spectrum_footprints("plate-001", &touching, None)
        .expect("touching planner envelopes must not be treated as overlapping");

    let overlapping = vec![
        ("left".to_owned(), [0.0, 0.0, 10.0, 10.0]),
        ("right".to_owned(), [9.999, 0.0, 20.0, 10.0]),
    ];
    let error = validate_full_spectrum_footprints("plate-001", &overlapping, None)
        .expect_err("a physical overlap beyond the shared epsilon must still fail");
    assert!(error.to_string().contains("units left and right overlap"));
}

fn color(red: u8, green: u8, blue: u8) -> RgbColor {
    RgbColor::new(red, green, blue)
}

fn spool(
    id: &str,
    display_name: &str,
    material: Material,
    nominal_color: RgbColor,
    profile_id: Option<&str>,
) -> Spool {
    Spool {
        id: id.into(),
        calibration_id: None,
        display_name: display_name.into(),
        color_name: Some(display_name.into()),
        material,
        nominal_color,
        measured_color: None,
        sku: None,
        profile_id: profile_id.map(str::to_owned),
        available: true,
    }
}

fn dummy_capability() -> U1DirectCapabilityReport {
    U1DirectCapabilityReport {
        adapter_id: U1_DIRECT_ADAPTER_ID.into(),
        status: U1DirectCapabilityStatus::StructurallyReadyNeedsGuiQualification,
        conversion_available: false,
        application_version: Some(SUPPORTED_ORCA_VERSION.into()),
        executable_sha256: Some(U1_DIRECT_EXECUTABLE_SHA256.into()),
        profile_pack: U1DirectProfilePackEvidence {
            source: U1DirectProfileSource::InstalledSystem,
            version: Some(U1_DIRECT_PROFILE_PACK_VERSION.into()),
            manifest_sha256: U1_DIRECT_PROFILE_MANIFEST_SHA256.into(),
            valid: true,
        },
        profiles: Vec::new(),
        auxiliary_profiles: Vec::new(),
        issues: Vec::new(),
    }
}

fn context(profiles_root: &Path) -> AdapterContext {
    AdapterContext {
        profiles_root: profiles_root.to_path_buf(),
        profile_manifest_path: profiles_root.join("../Snapmaker.json"),
        capability: dummy_capability(),
    }
}

fn planning_input() -> PlanningInput {
    PlanningInput {
        scopes: vec![PrintScope {
            id: "scope-1".into(),
            display_name: "Scope 1".into(),
            requirements: Vec::new(),
            units: Vec::new(),
            strategy: ScopeStrategy::DirectSpools,
            direct_assignments: Vec::new(),
            dedicated_support: None,
            approved_cmyx_fallbacks: Vec::new(),
            approved_material_substitutions: Vec::new(),
        }],
        inventory: vec![spool(
            "black",
            "Black",
            Material::Pla,
            color(1, 2, 3),
            Some("Generic PLA"),
        )],
        current_toolheads: CurrentToolheadState::default(),
        config: PlannerConfig::with_cmy_setup(CmySetup {
            cyan_spool_id: "cyan".into(),
            magenta_spool_id: "magenta".into(),
            yellow_spool_id: "yellow".into(),
            default_t4_spool_id: None,
        }),
    }
}

fn planning_result() -> PlanningResult {
    PlanningResult {
        scope_options: Vec::new(),
        jobs: Vec::new(),
        plates: Vec::new(),
        batches: Vec::new(),
        t4_swap_count: 0,
        a1_spool_change_count: 0,
        final_toolheads: CurrentToolheadState::default(),
        warnings: Vec::new(),
        errors: Vec::new(),
    }
}

fn manual_review_candidate() -> CmyxColorCandidate {
    CmyxColorCandidate {
        recipe: CmyxRecipe::ManualReview {
            reason: "Direct-only fixture".into(),
        },
        calibration_sample_id: None,
        process_compatibility: None,
        required_t4_spool_id: None,
        predicted_color: None,
        delta_e00: None,
        confidence: ColorConfidence::Nominal,
        warnings: Vec::new(),
    }
}

fn direct_mapping(
    requirement: &str,
    source_slots: &[&str],
    toolhead: Toolhead,
    spool_id: &str,
) -> SourceToActualMapping {
    SourceToActualMapping {
        scope_id: "scope-1".into(),
        source_requirement_ids: vec![requirement.into()],
        source_slots: source_slots.iter().map(|slot| (*slot).into()).collect(),
        source_profile_ids: Vec::new(),
        source_material: Material::Petg,
        source_color: color(20, 30, 40),
        strategy: ColorStrategy::DirectSpools,
        cmyx_comparison: manual_review_candidate(),
        direct_toolhead: Some(toolhead),
        actual_spool_id: Some(spool_id.into()),
        actual_material: Some(Material::Petg),
        actual_color: Some(color(10, 20, 30)),
        delta_e00: Some(1.0),
        confidence: ColorConfidence::Nominal,
        status: MappingStatus::Close,
    }
}

fn unit(requirements: &[&str]) -> PrintableUnit {
    PrintableUnit {
        id: "unit-1".into(),
        source_unit_id: "source-unit-1".into(),
        source_object_id: 42,
        source_instance_id: 0,
        source_model_path: None,
        display_name: "Unit 1".into(),
        source_plate_id: Some("plate-1".into()),
        requirement_ids: requirements.iter().map(|id| (*id).into()).collect(),
        bounds: BoundsMm::from_size(10.0, 10.0, 10.0),
        source_layer_height_mm: Some(0.2),
        printer_preference: PrinterPreference::U1,
    }
}

fn direct_job(mappings: Vec<SourceToActualMapping>, loadout: U1Loadout) -> PlannedJob {
    PlannedJob {
        id: "job-1".into(),
        scope_ids: vec!["scope-1".into()],
        units: Vec::new(),
        printer: Printer::U1,
        strategy: ColorStrategy::DirectSpools,
        loadout: PrinterLoadout::U1 { loadout },
        printable_materials: vec![Material::Petg],
        fast_mono: false,
        full_spectrum_process: None,
        color_mappings: mappings,
        estimated_tool_changes: Estimate::RequiresSlicing,
    }
}

#[test]
fn canonical_fingerprint_is_stable_and_binds_input_and_result() {
    let input = planning_input();
    let result = planning_result();
    let first = canonical_plan_fingerprint(&input, &result).unwrap();
    let repeated = canonical_plan_fingerprint(&input.clone(), &result.clone()).unwrap();

    assert_eq!(first, repeated);
    assert_eq!(first.len(), 64);
    assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));

    let mut changed_input = input.clone();
    changed_input.inventory[0].nominal_color = color(1, 2, 4);
    assert_ne!(
        first,
        canonical_plan_fingerprint(&changed_input, &result).unwrap()
    );

    let mut without_calibration = input.clone();
    without_calibration.scopes[0]
        .requirements
        .push(u1_planner::MaterialColorRequirement {
            id: "color-1".into(),
            material: Material::Pla,
            role: u1_planner::MaterialRole::Cosmetic,
            source_color: color(20, 30, 40),
            source_slots: vec!["F1".into()],
            source_profile_ids: vec!["source-profile".into()],
            cmyx_candidate: manual_review_candidate(),
            best_effort_cmyx_candidate: None,
            cmyx_palette_candidates: Vec::new(),
            direct_candidates: Vec::new(),
        });
    let without_calibration_fingerprint =
        canonical_plan_fingerprint(&without_calibration, &result).unwrap();
    let mut changed_calibration = without_calibration;
    changed_calibration.scopes[0].requirements[0]
        .cmyx_candidate
        .calibration_sample_id = Some("sample-r1".into());
    assert_ne!(
        without_calibration_fingerprint,
        canonical_plan_fingerprint(&changed_calibration, &result).unwrap()
    );

    let mut changed_result = result;
    changed_result.t4_swap_count = 1;
    assert_ne!(
        first,
        canonical_plan_fingerprint(&input, &changed_result).unwrap()
    );
}

#[test]
fn capability_gate_rejects_a_missing_application_bundle() {
    let temporary = TempDir::new().unwrap();
    let missing = temporary.path().join("Missing Snapmaker Orca.app");
    let error = inspect_u1_direct_macos_application(&missing).unwrap_err();

    assert!(matches!(error, U1DirectError::Capability(_)));
    assert!(error.to_string().contains("was not found"));
}

#[test]
fn macos_profile_source_prefers_portable_then_installed_system_state() {
    let temporary = TempDir::new().unwrap();
    let application_parent = temporary.path().join("Applications");
    let application = application_parent.join("Snapmaker Orca.app");
    let resources = application.join("Contents/Resources");
    let home = temporary.path().join("home");
    let installed_system = home.join("Library/Application Support/Snapmaker_Orca/system");
    fs::create_dir_all(installed_system.join(SNAPMAKER_VENDOR_NAME)).unwrap();

    let installed = resolve_macos_profile_source(&application, &resources, Some(&home)).unwrap();
    assert_eq!(installed.kind, U1DirectProfileSource::InstalledSystem);
    assert_eq!(
        installed.profiles_root,
        installed_system.join(SNAPMAKER_VENDOR_NAME)
    );

    let portable_system = application_parent.join("data_dir/system");
    fs::create_dir_all(portable_system.join(SNAPMAKER_VENDOR_NAME)).unwrap();
    fs::write(
        portable_system.join(format!("{SNAPMAKER_VENDOR_NAME}.json")),
        b"{}",
    )
    .unwrap();
    let portable = resolve_macos_profile_source(&application, &resources, Some(&home)).unwrap();
    assert_eq!(portable.kind, U1DirectProfileSource::InstalledSystem);
    assert_eq!(
        portable.profiles_root,
        portable_system.join(SNAPMAKER_VENDOR_NAME)
    );
}

#[test]
fn macos_profile_source_uses_bundled_seed_only_without_installed_vendor_state() {
    let temporary = TempDir::new().unwrap();
    let application = temporary.path().join("Applications/Snapmaker Orca.app");
    let resources = application.join("Contents/Resources");
    let home = temporary.path().join("home");

    let bundled = resolve_macos_profile_source(&application, &resources, Some(&home)).unwrap();
    assert_eq!(bundled.kind, U1DirectProfileSource::BundledSeed);
    assert_eq!(
        bundled.profiles_root,
        resources.join(BUNDLED_SNAPMAKER_PROFILE_ROOT)
    );
    assert_eq!(
        bundled.manifest_path,
        resources.join(BUNDLED_SNAPMAKER_PROFILE_MANIFEST)
    );

    let partial_manifest =
        home.join("Library/Application Support/Snapmaker_Orca/system/Snapmaker.json");
    fs::create_dir_all(partial_manifest.parent().unwrap()).unwrap();
    fs::write(&partial_manifest, b"partial").unwrap();
    let partial = resolve_macos_profile_source(&application, &resources, Some(&home)).unwrap();
    assert_eq!(partial.kind, U1DirectProfileSource::InstalledSystem);
    assert_eq!(partial.manifest_path, partial_manifest);
}

fn synthetic_qualified_report() -> Value {
    let profiles = DIRECT_PROFILE_BASELINE
        .iter()
        .map(|(relative_path, sha256)| json!({ "relativePath": relative_path, "sha256": sha256 }))
        .collect::<Vec<_>>();
    let auxiliary_profiles = DIRECT_AUXILIARY_PROFILE_BASELINE
        .iter()
        .map(|(relative_path, sha256)| json!({ "relativePath": relative_path, "sha256": sha256 }))
        .collect::<Vec<_>>();
    let checks = json!({
        "machineProfileLoaded": true,
        "processProfileLoaded": true,
        "t1T4MappingVerified": true,
        "primeTowerVerified": true,
        "plateCountStable": true,
        "objectCountStable": true,
        "sliceCompletedWithoutRepairWarning": true,
        "noIncompatibleProfileWarning": true,
        "noMissingProfileWarning": true,
        "noCustomProfileWarning": true,
        "firstGuiSavedStructurallyValid": true,
        "reopenedGuiSavedStructurallyValid": true,
        "noEmbeddedPresetsAfterFirstSave": true,
        "noEmbeddedPresetsAfterReopenedSave": true,
        "geometryAndPlacementsStable": true,
        "targetGlobalsStable": true,
        "writerCandidateIdentifyIdsPositiveAndUnique": true,
        "writerCandidateSourceIdentifyIdsPreserved": true,
        "guiIdentifyIdsUnique": true,
        "instanceIdentityBijectionStable": true,
        "derivedProfileIdentityStable": true
    });
    let qualification = json!({
        "fixtureName": U1_DIRECT_QUALIFICATION_FIXTURE,
        "sourceFixtureSha256": U1_DIRECT_QUALIFICATION_FIXTURE_SHA256,
        "writerCandidateSha256": "1".repeat(64),
        "firstGuiSavedSha256": "2".repeat(64),
        "reopenedGuiSavedSha256": "3".repeat(64),
        "opened": true,
        "sliced": true,
        "saved": true,
        "closed": true,
        "reopened": true,
        "resliced": true,
        "resaved": true,
        "qualifiedAtUtc": "2026-08-03T00:00:00Z",
        "profiles": profiles,
        "auxiliaryProfiles": auxiliary_profiles,
        "checks": checks
    });
    json!({
        "schemaVersion": 2,
        "adapterId": U1_DIRECT_ADAPTER_ID,
        "applicationVersion": SUPPORTED_ORCA_VERSION,
        "executableSha256": U1_DIRECT_EXECUTABLE_SHA256,
        "profileSource": "installed_system",
        "profilePackVersion": U1_DIRECT_PROFILE_PACK_VERSION,
        "profileManifestSha256": U1_DIRECT_PROFILE_MANIFEST_SHA256,
        "status": "qualified",
        "qualification": qualification,
        "note": "Synthetic test evidence only."
    })
}

fn synthetic_qualified_record(report_bytes: &[u8]) -> Value {
    json!({
        "schemaVersion": 2,
        "adapterId": U1_DIRECT_ADAPTER_ID,
        "applicationVersion": SUPPORTED_ORCA_VERSION,
        "executableSha256": U1_DIRECT_EXECUTABLE_SHA256,
        "profileSource": "installed_system",
        "profilePackVersion": U1_DIRECT_PROFILE_PACK_VERSION,
        "profileManifestSha256": U1_DIRECT_PROFILE_MANIFEST_SHA256,
        "guiRoundTripPassed": true,
        "status": "qualified",
        "evidence": {
            "fixtureName": U1_DIRECT_QUALIFICATION_FIXTURE,
            "sourceFixtureSha256": U1_DIRECT_QUALIFICATION_FIXTURE_SHA256,
            "writerCandidateSha256": "1".repeat(64),
            "firstGuiSavedSha256": "2".repeat(64),
            "reopenedGuiSavedSha256": "3".repeat(64),
            "qualificationReportSha256": format!("{:x}", Sha256::digest(report_bytes)),
            "opened": true,
            "sliced": true,
            "saved": true,
            "closed": true,
            "reopened": true,
            "resliced": true,
            "resaved": true,
            "qualifiedAtUtc": "2026-08-03T00:00:00Z"
        },
        "note": "Synthetic test evidence only."
    })
}

#[test]
fn embedded_qualification_and_gate_require_a_hash_bound_typed_round_trip_report() {
    assert!(qualification_bundle_is_valid(
        U1_DIRECT_QUALIFICATION_RECORD,
        U1_DIRECT_QUALIFICATION_REPORT,
        U1DirectProfileSource::InstalledSystem,
    ));
    assert!(!qualification_bundle_is_valid(
        U1_DIRECT_QUALIFICATION_RECORD,
        U1_DIRECT_QUALIFICATION_REPORT,
        U1DirectProfileSource::BundledSeed,
    ));

    let mut pending: Value =
        serde_json::from_slice(U1_DIRECT_QUALIFICATION_RECORD).expect("embedded record is JSON");
    pending["guiRoundTripPassed"] = json!(false);
    pending["status"] = json!("pending_gui_round_trip");
    pending["evidence"] = Value::Null;
    assert!(!qualification_bundle_is_valid(
        &serde_json::to_vec(&pending).unwrap(),
        U1_DIRECT_QUALIFICATION_REPORT,
        U1DirectProfileSource::InstalledSystem,
    ));

    pending["guiRoundTripPassed"] = json!(true);
    pending["status"] = json!("qualified");
    assert!(!qualification_bundle_is_valid(
        &serde_json::to_vec(&pending).unwrap(),
        U1_DIRECT_QUALIFICATION_REPORT,
        U1DirectProfileSource::InstalledSystem,
    ));

    let report_bytes = serde_json::to_vec(&synthetic_qualified_report()).unwrap();
    let record_bytes = serde_json::to_vec(&synthetic_qualified_record(&report_bytes)).unwrap();
    assert!(qualification_bundle_is_valid(
        &record_bytes,
        &report_bytes,
        U1DirectProfileSource::InstalledSystem,
    ));

    let mut changed_after_hash = report_bytes.clone();
    changed_after_hash.push(b'\n');
    assert!(!qualification_bundle_is_valid(
        &record_bytes,
        &changed_after_hash,
        U1DirectProfileSource::InstalledSystem,
    ));

    let mut mismatched_report = synthetic_qualified_report();
    mismatched_report["qualification"]["writerCandidateSha256"] = json!("4".repeat(64));
    let mismatched_report_bytes = serde_json::to_vec(&mismatched_report).unwrap();
    let mismatched_record_bytes =
        serde_json::to_vec(&synthetic_qualified_record(&mismatched_report_bytes)).unwrap();
    assert!(!qualification_bundle_is_valid(
        &mismatched_record_bytes,
        &mismatched_report_bytes,
        U1DirectProfileSource::InstalledSystem,
    ));

    let mut report_with_unknown_field = synthetic_qualified_report();
    report_with_unknown_field["unexpected"] = json!(true);
    let report_with_unknown_field_bytes = serde_json::to_vec(&report_with_unknown_field).unwrap();
    let record_for_unknown_field = serde_json::to_vec(&synthetic_qualified_record(
        &report_with_unknown_field_bytes,
    ))
    .unwrap();
    assert!(!qualification_bundle_is_valid(
        &record_for_unknown_field,
        &report_with_unknown_field_bytes,
        U1DirectProfileSource::InstalledSystem,
    ));

    let mut reordered_profiles = synthetic_qualified_report();
    reordered_profiles["qualification"]["profiles"]
        .as_array_mut()
        .unwrap()
        .reverse();
    let reordered_profile_bytes = serde_json::to_vec(&reordered_profiles).unwrap();
    let record_for_reordered_profiles =
        serde_json::to_vec(&synthetic_qualified_record(&reordered_profile_bytes)).unwrap();
    assert!(!qualification_bundle_is_valid(
        &record_for_reordered_profiles,
        &reordered_profile_bytes,
        U1DirectProfileSource::InstalledSystem,
    ));
}

#[test]
fn profile_chain_rejects_an_unresolved_parent_inside_the_snapmaker_namespace() {
    let temporary = TempDir::new().unwrap();
    let directory = temporary.path().join("filament");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("Child.json"),
        br#"{"name":"Child","inherits":"Missing Parent","setting_id":"child"}"#,
    )
    .unwrap();

    let error = resolve_profile_chain(temporary.path(), "filament/Child.json").unwrap_err();
    assert!(matches!(error, U1DirectError::Capability(_)));
    assert!(error.to_string().contains("resolves to 0 files"));
}

#[test]
fn stage_b_rejects_non_u1_and_non_direct_plan_nodes() {
    let loadout = U1Loadout {
        slots: [Some("black".into()), None, None, None],
    };
    let valid_batch = PlannedBatch {
        id: "batch-1".into(),
        printer: Printer::U1,
        strategy: ColorStrategy::DirectSpools,
        loadout: PrinterLoadout::U1 {
            loadout: loadout.clone(),
        },
        job_ids: vec!["job-1".into()],
        plate_ids: vec!["plate-1".into()],
        setup_actions: Vec::new(),
        t4_swap_before: false,
    };
    ensure_direct_u1_batch(&valid_batch).unwrap();

    let mut cmy_batch = valid_batch.clone();
    cmy_batch.strategy = ColorStrategy::CmyxFullSpectrum;
    assert!(
        ensure_direct_u1_batch(&cmy_batch)
            .unwrap_err()
            .to_string()
            .contains("plain U1 writer supports only")
    );

    let mut solid_batch = valid_batch.clone();
    solid_batch.strategy = ColorStrategy::CmyxSolid;
    ensure_direct_u1_batch(&solid_batch).unwrap();

    let mut a1_batch = valid_batch;
    a1_batch.printer = Printer::A1Mini;
    a1_batch.strategy = ColorStrategy::A1Mono;
    a1_batch.loadout = PrinterLoadout::A1Mini {
        spool_id: "black".into(),
    };
    assert!(ensure_direct_u1_batch(&a1_batch).is_err());

    let mut cmy_job = direct_job(Vec::new(), loadout.clone());
    cmy_job.strategy = ColorStrategy::CmyxFullSpectrum;
    assert!(ensure_direct_u1_job(&cmy_job).is_err());

    let mut solid_job = direct_job(Vec::new(), loadout.clone());
    solid_job.strategy = ColorStrategy::CmyxSolid;
    ensure_direct_u1_job(&solid_job).unwrap();

    let a1_plate = PlannedPlate {
        id: "plate-1".into(),
        job_id: "job-1".into(),
        printer: Printer::A1Mini,
        units: Vec::new(),
        placements: Vec::new(),
        prime_tower: None,
        packing_status: PackingStatus::RequiresGeometryPacking,
        individual_bounds_validated: true,
        full_spectrum_process: None,
        estimated_print_time_seconds: Estimate::RequiresSlicing,
        estimated_material_grams: Estimate::RequiresSlicing,
    };
    assert!(ensure_direct_u1_plate(&a1_plate).is_err());
}

#[test]
fn physically_solid_cmyx_mapping_uses_its_canonical_t4_spool() {
    let loadout = U1Loadout {
        slots: [
            Some("cyan".into()),
            Some("magenta".into()),
            Some("yellow".into()),
            Some("petg-blue".into()),
        ],
    };
    let mut mapping = direct_mapping("petg", &["F6", "F10"], Toolhead::T4, "petg-blue");
    mapping.strategy = ColorStrategy::CmyxSolid;
    mapping.direct_toolhead = None;
    mapping.cmyx_comparison = CmyxColorCandidate {
        recipe: CmyxRecipe::DedicatedT4,
        calibration_sample_id: None,
        process_compatibility: None,
        required_t4_spool_id: Some("petg-blue".into()),
        predicted_color: Some(color(10, 20, 30)),
        delta_e00: Some(1.0),
        confidence: ColorConfidence::Nominal,
        warnings: Vec::new(),
    };
    let mut job = direct_job(vec![mapping], loadout.clone());
    job.strategy = ColorStrategy::CmyxSolid;

    let slot_map = direct_slot_map_for_unit(&unit(&["petg"]), &job, &loadout).unwrap();

    assert_eq!(slot_map, BTreeMap::from([(6, 4), (10, 4)]));
}

#[test]
fn several_source_colors_can_share_one_physical_toolhead() {
    let loadout = U1Loadout {
        slots: [None, Some("petg-blue".into()), None, None],
    };
    let job = direct_job(
        vec![
            direct_mapping("r-black", &["F1", "f6"], Toolhead::T2, "petg-blue"),
            direct_mapping("r-grey", &[" F10 "], Toolhead::T2, "petg-blue"),
        ],
        loadout.clone(),
    );
    let mapping = direct_slot_map_for_unit(&unit(&["r-black", "r-grey"]), &job, &loadout).unwrap();

    assert_eq!(mapping, BTreeMap::from([(1, 2), (6, 2), (10, 2)]));
}

#[test]
fn direct_slot_map_rejects_a_source_color_sent_to_two_toolheads() {
    let loadout = U1Loadout {
        slots: [Some("black".into()), Some("blue".into()), None, None],
    };
    let job = direct_job(
        vec![
            direct_mapping("r1", &["F6"], Toolhead::T1, "black"),
            direct_mapping("r2", &["F6"], Toolhead::T2, "blue"),
        ],
        loadout.clone(),
    );
    let error = direct_slot_map_for_unit(&unit(&["r1", "r2"]), &job, &loadout).unwrap_err();

    assert!(error.to_string().contains("maps to multiple U1 toolheads"));
}

#[test]
fn physical_spools_resolve_only_to_qualified_profile_families() {
    let temporary = TempDir::new().unwrap();
    let directory = temporary.path().join("filament");
    fs::create_dir_all(&directory).unwrap();
    for (file, name, setting_id) in [
        ("Generic PLA.json", "Generic PLA", "pla-id"),
        (
            "Polymaker General PLA Family @U1.json",
            "Polymaker General PLA Family @U1",
            "poly-id",
        ),
        ("Generic PETG.json", "Generic PETG", "petg-id"),
        ("Snapmaker PVA @U1.json", "Snapmaker PVA @U1", "pva-id"),
    ] {
        fs::write(
            directory.join(file),
            serde_json::to_vec(&json!({
                "name": name,
                "setting_id": setting_id,
                "filament_id": format!("{setting_id}-filament"),
                "filament_type": [if name.contains("PETG") { "PETG" } else { "PLA" }]
            }))
            .unwrap(),
        )
        .unwrap();
    }
    let context = context(temporary.path());

    let generic = resolve_physical_profile(
        &context,
        Some(&spool(
            "white",
            "White",
            Material::Pla,
            color(239, 247, 255),
            Some("Generic PLA"),
        )),
    )
    .unwrap();
    assert_eq!(generic.name, "Generic PLA");
    assert_eq!(generic.setting_id, "pla-id");
    assert_eq!(generic.filament_id, "pla-id-filament");
    assert_eq!(generic.color, "#EFF7FF");
    assert_eq!(generic.material, "PLA");

    let polymaker = resolve_physical_profile(
        &context,
        Some(&spool(
            "poly",
            "Polymaker Black",
            Material::Pla,
            color(1, 1, 1),
            None,
        )),
    )
    .unwrap();
    assert_eq!(polymaker.name, "Polymaker General PLA Family @U1");

    let petg = resolve_physical_profile(
        &context,
        Some(&spool(
            "petg",
            "PETG Blue",
            Material::Petg,
            color(0, 80, 180),
            None,
        )),
    )
    .unwrap();
    assert_eq!(petg.name, "Generic PETG");
    assert_eq!(petg.material, "PETG");

    let reli3d = resolve_physical_profile(
        &context,
        Some(&spool(
            "reli3d-pva",
            "Reli3D PVA",
            Material::Pva,
            color(225, 214, 120),
            Some(RELI3D_PVA_PROFILE_NAME),
        )),
    )
    .unwrap();
    assert_eq!(reli3d.name, RELI3D_PVA_PROFILE_NAME);
    assert_eq!(reli3d.setting_id, crate::pva_profile::RELI3D_PVA_SETTING_ID);
    assert_eq!(
        reli3d.filament_id,
        crate::pva_profile::RELI3D_PVA_FILAMENT_ID
    );
    assert_eq!(reli3d.resolved["nozzle_temperature"], json!(["210"]));
    assert_eq!(
        reli3d.resolved["nozzle_temperature_range_high"],
        json!(["230"])
    );
    assert_eq!(
        reli3d.resolved["filament_max_volumetric_speed"],
        json!(["3"])
    );

    let unspecified_pva = resolve_physical_profile(
        &context,
        Some(&spool(
            "unknown-pva",
            "Unknown PVA",
            Material::Pva,
            color(225, 214, 120),
            None,
        )),
    )
    .unwrap_err();
    assert!(
        unspecified_pva
            .to_string()
            .contains("no qualified U1 profile")
    );

    let unsupported = resolve_physical_profile(
        &context,
        Some(&spool(
            "custom",
            "Custom",
            Material::Pla,
            color(1, 2, 3),
            Some("Unreviewed Custom Profile"),
        )),
    )
    .unwrap_err();
    assert!(unsupported.to_string().contains("unqualified U1 profile"));
}

#[test]
fn project_settings_keep_t1_through_t4_profile_and_color_order() {
    let temporary = TempDir::new().unwrap();
    for (relative_path, profile) in [
        (
            MACHINE_PATH,
            json!({"name": U1_DIRECT_MACHINE_PROFILE, "machine_key": "machine-value"}),
        ),
        (
            DIRECT_PROCESS_PATH,
            json!({
                "name": U1_DIRECT_PROCESS_PROFILE,
                "process_key": "process-value",
                "enable_prime_tower": "1",
                "prime_tower_width": "30",
                "prime_tower_brim_width": "5",
                "prime_volume": "45",
                "wipe_tower_cone_angle": "15",
                "wipe_tower_extra_rib_length": "8",
                "wipe_tower_extra_spacing": "120%",
                "wipe_tower_wall_type": "rib",
                "layer_height": "0.2",
                "wall_generator": "classic",
                "outer_wall_speed": "200",
                "inner_wall_speed": "300",
                "top_surface_speed": "200",
                "outer_wall_acceleration": "5000",
                "wall_loops": "2",
                "top_shell_layers": "5",
                "bottom_shell_layers": "3",
                "support_top_z_distance": "0.2",
                "support_bottom_z_distance": "0.2",
                "brim_type": "auto_brim",
                "brim_width": "5",
                "brim_object_gap": "0.1"
            }),
        ),
    ] {
        let path = temporary.path().join(relative_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec(&profile).unwrap()).unwrap();
    }
    let profiles = [
        ("T1 profile", "T1-id", "#001122", "PLA"),
        ("T2 profile", "T2-id", "#334455", "PETG"),
        ("T3 profile", "T3-id", "#667788", "PLA"),
        ("T4 profile", "T4-id", "#99AABB", "PETG"),
    ]
    .map(|(name, setting_id, color, material)| PhysicalProfile {
        name: name.into(),
        setting_id: setting_id.into(),
        filament_id: format!("{setting_id}-filament"),
        color: color.into(),
        material: material.into(),
        resolved: BTreeMap::from([(
            "filament_temperature".into(),
            json!([format!("{}", 200 + setting_id.as_bytes()[1] as u16)]),
        )]),
    });

    let plates = (1..=3)
        .map(|id| ArtifactPlate {
            source_plate_ids: vec![id],
            target_plate_id: id,
            name: format!("Plate {id}"),
            units: Vec::new(),
            source_identify_ids: BTreeMap::new(),
            wipe_tower_x: 40.0,
            wipe_tower_y: 200.0,
        })
        .collect::<Vec<_>>();
    let source_process = u1_three_mf::ProcessInformation {
        layer_height_mm: Some(0.12),
        quality: u1_three_mf::QualityInformation {
            wall_generator: Some(u1_three_mf::WallGenerator::Arachne),
            outer_wall_speed_mm_s: Some(60.0),
            inner_wall_speed_mm_s: Some(150.0),
            top_surface_speed_mm_s: Some(150.0),
            outer_wall_acceleration_mm_s2: Some(2000.0),
            wall_loops: Some(2),
            top_shell_layers: Some(5),
            bottom_shell_layers: Some(5),
        },
        support: u1_three_mf::SupportInformation {
            enabled: Some(true),
            support_type: Some(u1_three_mf::SupportType::TreeAuto),
            threshold_angle_degrees: Some(20),
            on_build_plate_only: Some(true),
        },
        ..Default::default()
    };
    let bytes = build_project_settings(
        &context(temporary.path()),
        &profiles,
        &plates,
        &source_process,
    )
    .unwrap();
    let settings: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        settings["filament_settings_id"],
        json!(["T1 profile", "T2 profile", "T3 profile", "T4 profile"])
    );
    assert_eq!(
        settings["filament_ids"],
        json!(["T1-id", "T2-id", "T3-id", "T4-id"])
    );
    assert_eq!(
        settings["filament_colour"],
        json!(["#001122", "#334455", "#667788", "#99AABB"])
    );
    assert_eq!(
        settings["filament_type"],
        json!(["PLA", "PETG", "PLA", "PETG"])
    );
    assert_eq!(settings["nozzle_diameter"].as_array().unwrap().len(), 4);
    assert_eq!(
        settings["flush_volumes_matrix"].as_array().unwrap().len(),
        16
    );
    assert_eq!(
        settings["flush_volumes_vector"].as_array().unwrap().len(),
        8
    );
    assert_eq!(settings["wipe_tower_x"].as_array().unwrap().len(), 3);
    assert_eq!(settings["wipe_tower_y"].as_array().unwrap().len(), 3);
    assert_eq!(settings["curr_bed_type"], json!("Textured PEI Plate"));
    assert_eq!(settings["brim_type"], json!("auto_brim"));
    assert_eq!(settings["print_sequence"], json!("by layer"));
    assert_eq!(settings["first_layer_print_sequence"], json!(["0"]));
    assert_eq!(settings["other_layers_print_sequence"], json!(["0"]));
    assert_eq!(settings["other_layers_print_sequence_nums"], json!("0"));
    assert_eq!(settings["spiral_mode"], json!("0"));
    assert_eq!(settings["timelapse_type"], json!("0"));
    assert_eq!(settings["wipe_tower_rotation_angle"], json!("0"));
    assert_eq!(settings["brim_object_gap"], json!("0.1"));
    assert_eq!(settings["layer_height"], json!("0.12"));
    assert_eq!(settings["wall_generator"], json!("arachne"));
    assert_eq!(settings["outer_wall_speed"], json!("60"));
    assert_eq!(settings["inner_wall_speed"], json!("150"));
    assert_eq!(settings["top_surface_speed"], json!("150"));
    assert_eq!(settings["outer_wall_acceleration"], json!("2000"));
    assert_eq!(settings["wall_loops"], json!("2"));
    assert_eq!(settings["top_shell_layers"], json!("5"));
    assert_eq!(settings["bottom_shell_layers"], json!("5"));
    assert_eq!(settings["enable_support"], json!("1"));
    assert_eq!(settings["support_type"], json!("tree(auto)"));
    assert_eq!(settings["support_threshold_angle"], json!("20"));
    assert_eq!(settings["support_on_build_plate_only"], json!("1"));
    assert_eq!(settings["support_top_z_distance"], json!("0.12"));
    assert_eq!(settings["support_bottom_z_distance"], json!("0.12"));
    assert_eq!(
        settings["different_settings_to_system"],
        json!([
            "bottom_shell_layers;enable_support;inner_wall_speed;layer_height;outer_wall_acceleration;outer_wall_speed;support_bottom_z_distance;support_on_build_plate_only;support_threshold_angle;support_top_z_distance;support_type;top_shell_layers;top_surface_speed;wall_generator;wall_loops",
            "",
            "",
            "",
            "",
            ""
        ])
    );

    let mut pva_profiles = profiles.clone();
    pva_profiles[3].name = "Snapmaker PVA @U1".into();
    pva_profiles[3].setting_id = "41452139080".into();
    pva_profiles[3].filament_id = "31046369800".into();
    pva_profiles[3].material = "PVA".into();
    let pva_settings = build_project_settings_from_profiles(
        resolve_profile_chain(temporary.path(), MACHINE_PATH).unwrap(),
        resolve_profile_chain(temporary.path(), DIRECT_PROCESS_PATH).unwrap(),
        &pva_profiles,
        &plates,
        &source_process,
        Some(&DedicatedSupportMaterial {
            spool_id: "support-pva".into(),
            usage: SupportMaterialUsage::InterfaceOnly,
            toolhead: Toolhead::T4,
        }),
    )
    .unwrap();
    let pva_settings: Value = serde_json::from_slice(&pva_settings).unwrap();
    assert_eq!(pva_settings["enable_support"], json!("1"));
    assert_eq!(pva_settings["support_filament"], json!("0"));
    assert_eq!(pva_settings["support_interface_filament"], json!("4"));
    assert_eq!(pva_settings["support_top_z_distance"], json!("0"));
    assert_eq!(pva_settings["support_bottom_z_distance"], json!("0"));
    assert_eq!(pva_settings["support_interface_top_layers"], json!("3"));
    assert_eq!(pva_settings["support_interface_speed"], json!("30"));
    assert_eq!(
        pva_settings["filament_type"],
        json!(["PLA", "PETG", "PLA", "PVA"])
    );

    let mut invalid_gap = serde_json::from_slice::<BTreeMap<String, Value>>(&bytes).unwrap();
    invalid_gap.insert("brim_object_gap".into(), json!("1.01"));
    let error = validate_prime_tower_profile_contract(&invalid_gap).unwrap_err();
    assert!(matches!(error, U1DirectError::Capability(_)));
}

#[test]
fn quality_transfer_fails_closed_below_the_qualified_u1_layer_height() {
    let mut settings = BTreeMap::from([("layer_height".into(), json!("0.2"))]);
    let process = u1_three_mf::ProcessInformation {
        layer_height_mm: Some(0.04),
        ..Default::default()
    };
    let error = apply_source_quality_intent(&mut settings, &process, &mut BTreeSet::new())
        .expect_err("unsupported fine source layers must not be silently coarsened");
    assert!(
        error
            .to_string()
            .contains("finer than the qualified U1 minimum")
    );
}

#[test]
fn paint_rewrite_preserves_shape_while_merging_many_source_states() {
    let tree = PaintNode::Split {
        split_sides: 3,
        special_side: 2,
        children: vec![
            PaintNode::Leaf { state: 0 },
            PaintNode::Leaf { state: 1 },
            PaintNode::Leaf { state: 6 },
            PaintNode::Leaf { state: 10 },
        ],
    };
    let encoded = encode_paint_annotation(&tree).unwrap();
    let mut event = BytesStart::new("triangle");
    event.push_attribute(("paint_color", encoded.as_str()));
    event.push_attribute(("s:mmu_segmentation", encoded.as_str()));
    let reader = Reader::from_reader(Cursor::new(Vec::<u8>::new()));
    let slot_map = BTreeMap::from([(1, 2), (6, 2), (10, 2)]);

    let rewritten =
        rewrite_model_element(&reader, &event, MAIN_MODEL_PATH, Some(&slot_map)).unwrap();
    let attributes = decoded_attributes(&reader, &rewritten, MAIN_MODEL_PATH).unwrap();
    for key in ["paint_color", "s:mmu_segmentation"] {
        let encoded = attributes
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value)
            .unwrap();
        let rewritten_tree = decode_paint_annotation(encoded).unwrap();
        assert_eq!(rewritten_tree.used_states(), BTreeSet::from([0, 2]));
        assert!(matches!(
            rewritten_tree,
            PaintNode::Split {
                split_sides: 3,
                special_side: 2,
                children
            } if children == vec![
                PaintNode::Leaf { state: 0 },
                PaintNode::Leaf { state: 2 },
                PaintNode::Leaf { state: 2 },
                PaintNode::Leaf { state: 2 },
            ]
        ));
    }
}

#[test]
fn canonical_geometry_fingerprint_ignores_paint_but_detects_coordinate_changes() {
    let reader = Reader::from_reader(Cursor::new(Vec::<u8>::new()));
    let vertex = |x: &'static str, paint: &'static str| {
        let mut event = BytesStart::new("vertex");
        event.push_attribute(("x", x));
        event.push_attribute(("y", "2"));
        event.push_attribute(("z", "3"));
        event.push_attribute(("paint_color", paint));
        event
    };

    let mut original = GeometryCounts::default();
    begin_geometry_resource(MAIN_MODEL_PATH, 42, &mut original);
    record_geometry_element(
        &reader,
        &vertex("1", "source-paint"),
        MAIN_MODEL_PATH,
        &mut original,
    )
    .unwrap();

    let mut repainted = GeometryCounts::default();
    begin_geometry_resource(MAIN_MODEL_PATH, 42, &mut repainted);
    record_geometry_element(
        &reader,
        &vertex("1", "target-paint"),
        MAIN_MODEL_PATH,
        &mut repainted,
    )
    .unwrap();

    let mut moved_vertex = GeometryCounts::default();
    begin_geometry_resource(MAIN_MODEL_PATH, 42, &mut moved_vertex);
    record_geometry_element(
        &reader,
        &vertex("1.001", "target-paint"),
        MAIN_MODEL_PATH,
        &mut moved_vertex,
    )
    .unwrap();

    assert_eq!(original.vertices, 1);
    assert_eq!(original.sha256(), repainted.sha256());
    assert_ne!(original.sha256(), moved_vertex.sha256());
}

#[test]
fn flush_matrix_matches_the_snapmaker_orca_2_3_5_sailfin_golden() {
    let settings = BTreeMap::from([
        ("nozzle_volume".into(), json!("143")),
        ("filament_is_support".into(), json!(["0", "0", "0", "0"])),
    ]);
    let slots = ["#FEE5A5", "#161616", "#519F61", "#89CEE5"].map(|color| PhysicalProfile {
        name: "Golden fixture".into(),
        setting_id: "fixture".into(),
        filament_id: "fixture-filament".into(),
        color: color.into(),
        material: "PLA".into(),
        resolved: BTreeMap::new(),
    });

    assert_eq!(
        build_orca_flush_matrix(&settings, &slots).unwrap(),
        vec![
            0, 269, 267, 308, 672, 0, 488, 604, 500, 225, 0, 394, 416, 259, 234, 0,
        ]
    );
}

#[test]
fn source_root_model_rejects_non_millimeter_units() {
    let temporary = TempDir::new().unwrap();
    let source = temporary.path().join("inch.3mf");
    let file = File::create(&source).unwrap();
    let mut archive = ZipWriter::new(file);
    archive
        .start_file(MAIN_MODEL_PATH, SimpleFileOptions::default())
        .unwrap();
    archive
        .write_all(
            br#"<?xml version="1.0"?><model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" unit="inch"><resources/><build/></model>"#,
        )
        .unwrap();
    archive.finish().unwrap();

    let error = ensure_source_model_uses_millimeters(&source).unwrap_err();
    assert!(matches!(error, U1DirectError::Plan(_)));
    assert!(error.to_string().contains("only millimeter"));
}

#[test]
fn source_plate_overrides_are_normalized_only_when_they_match_target_defaults() {
    let equivalent = br#"<?xml version="1.0"?>
        <config>
          <plate>
            <metadata key="plater_id" value="7"/>
            <metadata key="bed_type" value="Textured PEI Plate"/>
            <metadata key="print_sequence" value="by layer"/>
            <metadata key="first_layer_print_sequence" value="0"/>
            <metadata key="other_layers_print_sequence" value="0"/>
            <metadata key="other_layers_print_sequence_nums" value="0"/>
            <metadata key="spiral_mode" value="false"/>
            <metadata key="timelapse_type" value="0"/>
          </plate>
        </config>"#;
    assert_eq!(
        validate_source_plate_process_overrides_xml(Cursor::new(equivalent)).unwrap(),
        7
    );

    for (key, value) in [
        ("bed_type", "Cool Plate"),
        ("print_sequence", "by object"),
        ("first_layer_print_sequence", "2 1"),
        ("other_layers_print_sequence", "1 2"),
        ("other_layers_print_sequence_nums", "2"),
        ("spiral_mode", "true"),
        ("timelapse_type", "1"),
    ] {
        let xml = format!(
            r#"<?xml version="1.0"?><config><plate><metadata key="plater_id" value="3"/><metadata key="{key}" value="{value}"/></plate></config>"#
        );
        let error =
            validate_source_plate_process_overrides_xml(Cursor::new(xml.as_bytes())).unwrap_err();
        assert!(matches!(error, U1DirectError::Plan(_)));
        assert!(error.to_string().contains("source plate 3"));
        assert!(error.to_string().contains(key));
    }
}

#[test]
fn object_footprint_overrides_cannot_escape_the_prime_tower_clearance() {
    validate_object_footprint_override("brim_width", "5").unwrap();
    validate_object_footprint_override("brim_object_gap", "1").unwrap();
    for (key, value) in [
        ("brim_width", "5.01"),
        ("brim_object_gap", "1.01"),
        ("brim_width", "-1"),
        ("brim_width", "auto"),
    ] {
        assert!(validate_object_footprint_override(key, value).is_err());
    }
    assert_eq!(target_filament_maps(4), "1 1 1 1");
}

#[test]
fn canonical_target_provenance_accepts_multiple_source_plates() {
    let first = unit(&[]);
    let mut second = first.clone();
    second.id = "unit-2".into();
    second.source_unit_id = "source-unit-2".into();
    second.source_object_id = 43;
    second.source_plate_id = Some("plate-2".into());

    assert_eq!(
        canonical_source_plate_ids(&[first, second]).unwrap(),
        [1, 2]
    );
}

fn artifact_plan_fixture(
    target_plate_ids: &[u32],
    selected_object_ids: &[u32],
) -> ArtifactBuildPlan {
    let physical_slots = std::array::from_fn(|index| PhysicalProfile {
        name: format!("T{} fixture", index + 1),
        setting_id: format!("T{}-setting", index + 1),
        filament_id: format!("T{}-filament", index + 1),
        color: "#000000".into(),
        material: "PLA".into(),
        resolved: BTreeMap::new(),
    });
    let plates = target_plate_ids
        .iter()
        .copied()
        .map(|id| ArtifactPlate {
            source_plate_ids: vec![id],
            target_plate_id: id,
            name: format!("Plate {id}"),
            units: Vec::new(),
            source_identify_ids: BTreeMap::new(),
            wipe_tower_x: 40.0,
            wipe_tower_y: 200.0,
        })
        .collect::<Vec<_>>();
    ArtifactBuildPlan {
        prepared: U1DirectPreparedArtifact {
            batch_id: "batch-fixture".into(),
            file_name: "fixture.3mf".into(),
            target_plate_ids: target_plate_ids.iter().map(|id| id.to_string()).collect(),
            source_plate_ids: target_plate_ids.to_vec(),
            source_unit_ids: Vec::new(),
            loadout: Vec::new(),
            setup_actions: Vec::new(),
        },
        plates,
        object_slot_maps: selected_object_ids
            .iter()
            .copied()
            .map(|id| (id, BTreeMap::from([(1, 1)])))
            .collect(),
        resource_slot_maps: BTreeMap::new(),
        selected_root_resource_ids: BTreeSet::new(),
        external_paths: BTreeSet::new(),
        selected_instances: BTreeMap::new(),
        physical_slots,
        project_settings: Vec::new(),
    }
}

fn write_model_settings_fixture(path: &Path, xml: &str) {
    let file = File::create(path).unwrap();
    let mut archive = ZipWriter::new(file);
    archive
        .start_file(MODEL_SETTINGS_PATH, SimpleFileOptions::default())
        .unwrap();
    archive.write_all(xml.as_bytes()).unwrap();
    archive.finish().unwrap();
}

fn filament_map_plate_xml(id: u32) -> String {
    format!(
        r#"<plate><metadata key="plater_id" value="{id}"/><metadata key="filament_map_mode" value="Auto For Flush"/><metadata key="filament_maps" value="1 1 1 1"/></plate>"#
    )
}

#[test]
fn generated_plate_filament_map_contract_rejects_mutated_metadata() {
    let temporary = TempDir::new().unwrap();
    let plan = artifact_plan_fixture(&[1, 2], &[]);
    let valid_plate_1 = filament_map_plate_xml(1);
    let valid_plate_2 = filament_map_plate_xml(2);
    let valid_xml = format!("<config>{valid_plate_1}{valid_plate_2}</config>");
    let valid_path = temporary.path().join("valid.3mf");
    write_model_settings_fixture(&valid_path, &valid_xml);
    validate_generated_plate_filament_maps(&valid_path, &plan).unwrap();

    let mutations = [
        (
            "missing-mode",
            format!(
                r#"<config><plate><metadata key="plater_id" value="1"/><metadata key="filament_maps" value="1 1 1 1"/></plate>{valid_plate_2}</config>"#
            ),
            "exact four-filament",
        ),
        (
            "duplicate-mode",
            format!(
                r#"<config><plate><metadata key="plater_id" value="1"/><metadata key="filament_map_mode" value="Auto For Flush"/><metadata key="filament_map_mode" value="Auto For Flush"/><metadata key="filament_maps" value="1 1 1 1"/></plate>{valid_plate_2}</config>"#
            ),
            "duplicate filament_map_mode",
        ),
        (
            "wrong-mode",
            format!(
                r#"<config><plate><metadata key="plater_id" value="1"/><metadata key="filament_map_mode" value="Manual"/><metadata key="filament_maps" value="1 1 1 1"/></plate>{valid_plate_2}</config>"#
            ),
            "exact four-filament",
        ),
        (
            "missing-maps",
            format!(
                r#"<config><plate><metadata key="plater_id" value="1"/><metadata key="filament_map_mode" value="Auto For Flush"/></plate>{valid_plate_2}</config>"#
            ),
            "exact four-filament",
        ),
        (
            "duplicate-maps",
            format!(
                r#"<config><plate><metadata key="plater_id" value="1"/><metadata key="filament_map_mode" value="Auto For Flush"/><metadata key="filament_maps" value="1 1 1 1"/><metadata key="filament_maps" value="1 1 1 1"/></plate>{valid_plate_2}</config>"#
            ),
            "duplicate filament_maps",
        ),
        (
            "wrong-maps",
            format!(
                r#"<config><plate><metadata key="plater_id" value="1"/><metadata key="filament_map_mode" value="Auto For Flush"/><metadata key="filament_maps" value="1 1 1"/></plate>{valid_plate_2}</config>"#
            ),
            "exact four-filament",
        ),
        (
            "missing-plate-id",
            format!(
                r#"<config><plate><metadata key="filament_map_mode" value="Auto For Flush"/><metadata key="filament_maps" value="1 1 1 1"/></plate>{valid_plate_2}</config>"#
            ),
            "missing plater_id",
        ),
        (
            "duplicate-plate-id-metadata",
            format!(
                r#"<config><plate><metadata key="plater_id" value="1"/><metadata key="plater_id" value="1"/><metadata key="filament_map_mode" value="Auto For Flush"/><metadata key="filament_maps" value="1 1 1 1"/></plate>{valid_plate_2}</config>"#
            ),
            "duplicate plater_id",
        ),
        (
            "wrong-plate-id-set",
            format!(
                "<config>{valid_plate_1}{}</config>",
                filament_map_plate_xml(3)
            ),
            "do not match planned plate IDs",
        ),
        (
            "duplicate-plate-block-id",
            format!("<config>{valid_plate_1}{valid_plate_1}</config>"),
            "duplicate plate 1",
        ),
    ];

    for (name, xml, expected_message) in mutations {
        let path = temporary.path().join(format!("{name}.3mf"));
        write_model_settings_fixture(&path, &xml);
        let error = validate_generated_plate_filament_maps(&path, &plan).unwrap_err();
        assert!(matches!(error, U1DirectError::SemanticValidation(_)));
        assert!(
            error.to_string().contains(expected_message),
            "mutation {name:?} returned an unexpected error: {error}"
        );
    }
}

#[test]
fn model_settings_rewrite_enforces_object_and_part_brim_metadata() {
    let temporary = TempDir::new().unwrap();
    let plan = artifact_plan_fixture(&[1], &[42]);
    let valid_path = temporary.path().join("valid-brim.3mf");
    write_model_settings_fixture(
        &valid_path,
        r#"<config><object id="42"><metadata key="brim_width" value="5"/><metadata key="brim_object_gap" value="1"/><metadata key="brim_type" value="brim_ears"/><metadata key="wall_loops" value="3"/><metadata face_count="12"/><part id="43"><metadata key="name" value="Part"/><metadata key="matrix" value="1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1"/><metadata key="source_object_id" value="1"/><metadata key="source_volume_id" value="0"/><metadata key="source_offset_x" value="0"/><metadata key="source_offset_y" value="0"/><metadata key="source_offset_z" value="0"/><text_info text="R" font_name="Courier New"/></part></object></config>"#,
    );
    let rewritten = String::from_utf8(rewrite_model_settings(&valid_path, &plan).unwrap()).unwrap();
    assert!(rewritten.contains(r#"<metadata key="brim_width" value="5"/>"#));
    assert!(rewritten.contains(r#"<metadata key="brim_object_gap" value="1"/>"#));
    assert!(!rewritten.contains(r#"<metadata face_count="12"/>"#));
    assert!(!rewritten.contains("text_info"));

    for (name, metadata) in [
        (
            "object-brim-too-wide",
            r#"<metadata key="brim_width" value="5.01"/>"#,
        ),
        (
            "part-brim-gap-too-wide",
            r#"<part id="43"><metadata key="brim_object_gap" value="1.01"/></part>"#,
        ),
        (
            "support-can-expand-footprint",
            r#"<metadata key="enable_support" value="1"/>"#,
        ),
        (
            "xy-compensation-can-expand-footprint",
            r#"<metadata key="xy_contour_compensation" value="0.1"/>"#,
        ),
        (
            "unknown-process-override",
            r#"<metadata key="future_footprint_option" value="1"/>"#,
        ),
        (
            "object-cannot-carry-part-matrix",
            r#"<metadata key="matrix" value="1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1"/>"#,
        ),
        (
            "part-cannot-carry-process-override",
            r#"<part id="43"><metadata key="wall_loops" value="3"/></part>"#,
        ),
        (
            "painted-brim-needs-unsupported-sidecar",
            r#"<metadata key="brim_type" value="painted"/>"#,
        ),
        ("unknown-empty-child", r#"<future_config value="1"/>"#),
        (
            "unknown-child-subtree",
            r#"<future_config><opaque value="1"/></future_config>"#,
        ),
        ("unknown-unkeyed-metadata", r#"<metadata opaque="1"/>"#),
    ] {
        let path = temporary.path().join(format!("{name}.3mf"));
        write_model_settings_fixture(
            &path,
            &format!(r#"<config><object id="42">{metadata}</object></config>"#),
        );
        let error = rewrite_model_settings(&path, &plan).unwrap_err();
        assert!(matches!(
            error,
            U1DirectError::Plan(_) | U1DirectError::Xml { .. }
        ));
        assert!(
            error.to_string().contains("object footprint override")
                || error.to_string().contains("qualified Stage B allowlist")
                || error
                    .to_string()
                    .contains("unsupported retained object child element")
                || error
                    .to_string()
                    .contains("unkeyed retained object metadata")
        );
    }

    validate_retained_object_metadata(RetainedMetadataScope::Object, "enable_support", "false")
        .unwrap();
    validate_retained_object_metadata(RetainedMetadataScope::Object, "raft_layers", "0").unwrap();
    validate_retained_object_metadata(
        RetainedMetadataScope::Object,
        "xy_contour_compensation",
        "0",
    )
    .unwrap();
    validate_retained_object_metadata(
        RetainedMetadataScope::Object,
        "embedding_wall_into_infill",
        "1",
    )
    .unwrap();
}

#[test]
fn model_settings_author_fresh_target_metadata_for_cross_source_plate() {
    let temporary = TempDir::new().unwrap();
    let mut plan = artifact_plan_fixture(&[1], &[42, 43]);
    let first = unit(&[]);
    let mut second = first.clone();
    second.id = "unit-2".into();
    second.source_unit_id = "source-unit-2".into();
    second.source_object_id = 43;
    second.source_plate_id = Some("plate-2".into());
    plan.plates[0].source_plate_ids = vec![1, 2];
    plan.plates[0].name = "Packed U1 plate 01 — target-plate-1".into();
    plan.plates[0].units = vec![first, second];
    plan.plates[0].source_identify_ids = BTreeMap::from([((42, 0), 101), ((43, 0), 202)]);

    let source_path = temporary.path().join("cross-source-settings.3mf");
    write_model_settings_fixture(
        &source_path,
        r#"<config><object id="42"/><object id="43"/><plate><metadata key="plater_id" value="9"/><metadata key="plater_name" value="Source plate name"/><metadata key="locked" value="true"/></plate></config>"#,
    );

    let rewritten =
        String::from_utf8(rewrite_model_settings(&source_path, &plan).unwrap()).unwrap();
    assert_eq!(rewritten.matches("<plate>").count(), 1);
    assert!(rewritten.contains(r#"<metadata key="plater_id" value="1"/>"#));
    assert!(
        rewritten.contains(
            r#"<metadata key="plater_name" value="Packed U1 plate 01 — target-plate-1"/>"#
        )
    );
    assert!(rewritten.contains(r#"<metadata key="object_id" value="42"/>"#));
    assert!(rewritten.contains(r#"<metadata key="object_id" value="43"/>"#));
    assert!(rewritten.contains(r#"<metadata key="identify_id" value="101"/>"#));
    assert!(rewritten.contains(r#"<metadata key="identify_id" value="202"/>"#));
    assert!(!rewritten.contains("Source plate name"));
    assert!(!rewritten.contains(r#"key="locked" value="true""#));
}

fn write_cross_source_project_fixture(path: &Path) {
    const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
  <Override PartName="/Metadata/project_settings.config" ContentType="application/json"/>
  <Override PartName="/Metadata/model_settings.config" ContentType="application/xml"/>
</Types>"#;
    const ROOT_RELATIONSHIPS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Target="/3D/3dmodel.model" Id="rel-1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>"#;
    const MODEL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" unit="millimeter">
  <metadata name="Application">Snapmaker Orca 2.3.5</metadata>
  <resources>
    <object id="42" type="model"><mesh><vertices>
      <vertex x="0" y="0" z="0"/><vertex x="10" y="0" z="0"/>
      <vertex x="0" y="10" z="0"/><vertex x="0" y="0" z="1"/>
    </vertices><triangles>
      <triangle v1="0" v2="1" v3="2"/><triangle v1="0" v2="1" v3="3"/>
      <triangle v1="0" v2="2" v3="3"/><triangle v1="1" v2="2" v3="3"/>
    </triangles></mesh></object>
    <object id="43" type="model"><mesh><vertices>
      <vertex x="0" y="0" z="0"/><vertex x="10" y="0" z="0"/>
      <vertex x="0" y="10" z="0"/><vertex x="0" y="0" z="1"/>
    </vertices><triangles>
      <triangle v1="0" v2="1" v3="2"/><triangle v1="0" v2="1" v3="3"/>
      <triangle v1="0" v2="2" v3="3"/><triangle v1="1" v2="2" v3="3"/>
    </triangles></mesh></object>
  </resources>
  <build><item objectid="42" printable="1"/><item objectid="43" printable="1"/></build>
</model>"#;
    const MODEL_SETTINGS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
  <object id="42"><metadata key="extruder" value="1"/></object>
  <object id="43"><metadata key="extruder" value="1"/></object>
  <plate><metadata key="plater_id" value="1"/><model_instance><metadata key="object_id" value="42"/><metadata key="instance_id" value="0"/><metadata key="identify_id" value="101"/></model_instance></plate>
  <plate><metadata key="plater_id" value="2"/><model_instance><metadata key="object_id" value="43"/><metadata key="instance_id" value="0"/><metadata key="identify_id" value="202"/></model_instance></plate>
</config>"#;

    let file = File::create(path).unwrap();
    let mut archive = ZipWriter::new(file);
    for (name, bytes) in [
        ("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        ("_rels/.rels", ROOT_RELATIONSHIPS.as_bytes()),
        (MAIN_MODEL_PATH, MODEL.as_bytes()),
        (PROJECT_SETTINGS_PATH, b"{}"),
        (MODEL_SETTINGS_PATH, MODEL_SETTINGS.as_bytes()),
    ] {
        archive
            .start_file(name, SimpleFileOptions::default())
            .unwrap();
        archive.write_all(bytes).unwrap();
    }
    archive.finish().unwrap();
}

#[test]
fn full_spectrum_substrate_plan_accepts_one_target_from_two_source_plates() {
    let temporary = TempDir::new().unwrap();
    let source_path = temporary.path().join("cross-source.3mf");
    write_cross_source_project_fixture(&source_path);
    let analysis = analyze_project(&source_path).unwrap();
    assert_eq!(analysis.plates.len(), 2);

    let mut input = planning_input();
    let mut first = unit(&[]);
    first.id = "unit-1".into();
    first.source_unit_id = "source-unit-1".into();
    first.source_object_id = 42;
    first.source_plate_id = Some("plate-1".into());
    first.bounds = BoundsMm::from_size(10.0, 10.0, 1.0);
    let mut second = first.clone();
    second.id = "unit-2".into();
    second.source_unit_id = "source-unit-2".into();
    second.source_object_id = 43;
    second.source_plate_id = Some("plate-2".into());
    input.scopes[0].units = vec![first, second];

    let loadout = std::array::from_fn(|index| crate::U1FullSpectrumPhysicalSlot {
        toolhead: Toolhead::ALL[index],
        spool_id: format!("spool-{}", index + 1),
        spool_name: format!("Spool {}", index + 1),
        material: Material::Pla,
        color: color(index as u8, index as u8, index as u8),
        profile: format!("Profile {}", index + 1),
        setting_id: format!("setting-{}", index + 1),
        filament_id: format!("filament-{}", index + 1),
        wildcard_resolved: false,
    });
    let prepared_unit =
        |unit_id: &str,
         source_unit_id: &str,
         source_object_id: u32,
         source_plate_id: &str,
         target_min_x_mm: f64| crate::U1FullSpectrumPreparedUnit {
            unit: u1_planner::ScopedUnitRef {
                scope_id: "scope-1".into(),
                unit_id: unit_id.into(),
            },
            source_unit_id: source_unit_id.into(),
            source_object_id,
            source_instance_id: 0,
            source_model_path: None,
            source_plate_id: Some(source_plate_id.into()),
            target_min_x_mm,
            target_min_y_mm: 10.0,
            source_to_target_slots: BTreeMap::from([(1, 1)]),
        };
    let artifact = crate::U1FullSpectrumPreparedArtifact {
        batch_id: "batch-1".into(),
        file_name: "cross-source.3mf".into(),
        loadout,
        calibration_fingerprint: "fixture".into(),
        process: crate::u1_full_spectrum_process_contract(),
        support: u1_three_mf::SupportInformation::default(),
        dedicated_support: None,
        recipe_table: crate::U1FullSpectrumRecipeTable {
            schema_version: crate::U1_FULL_SPECTRUM_SCHEMA_VERSION,
            physical_filament_count: 4,
            definitions: Vec::new(),
            serialized_definitions: String::new(),
            targets: Vec::new(),
        },
        recipe_calibration_sample_ids: Vec::new(),
        assignments: Vec::new(),
        plates: vec![crate::U1FullSpectrumPreparedPlate {
            plan_plate_id: "target-plate-1".into(),
            target_plate_id: 1,
            job_id: "job-1".into(),
            prime_tower: None,
            units: vec![
                prepared_unit("unit-1", "source-unit-1", 42, "plate-1", 10.0),
                prepared_unit("unit-2", "source-unit-2", 43, "plate-2", 40.0),
            ],
        }],
    };

    let plan = build_full_spectrum_substrate_plan(&analysis, &input, &artifact, Vec::new())
        .expect("one Full Spectrum target may combine validated units from two source plates");

    assert_eq!(plan.plates.len(), 1);
    assert_eq!(plan.plates[0].source_plate_ids, [1, 2]);
    assert_eq!(plan.plates[0].units.len(), 2);
    assert_eq!(plan.selected_instances.len(), 2);
    assert_eq!(plan.prepared.source_plate_ids, [1, 2]);
    assert_eq!(
        plan.plates[0].name,
        "Packed Full Spectrum plate 01 — target-plate-1"
    );
}

fn plate_with_bounds(bounds: AxisAlignedBounds) -> PlateAnalysis {
    PlateAnalysis {
        id: 1,
        name: Some("Bounds fixture".into()),
        instances: Vec::new(),
        printable_bounds: Some(bounds),
        object_count: 0,
        part_count: 0,
        effective_slots: Vec::new(),
        effective_material_colors: Vec::new(),
        classification: ColorClassification::Mono,
    }
}

#[test]
fn preserved_plate_bounds_use_the_exact_u1_profile_and_allow_sinking_geometry() {
    let exact = plate_with_bounds(AxisAlignedBounds {
        min: [TARGET_MIN_X_MM, TARGET_MIN_Y_MM, 0.0],
        max: [TARGET_MAX_X_MM, TARGET_MAX_Y_MM, TARGET_PRINTABLE_HEIGHT_MM],
    });
    validate_preserved_plate_bounds(&exact, 0, 1, TARGET_BED_SIZE_MM).unwrap();

    let sinking = plate_with_bounds(AxisAlignedBounds {
        min: [TARGET_MIN_X_MM, TARGET_MIN_Y_MM, -25.0],
        max: [TARGET_MAX_X_MM, TARGET_MAX_Y_MM, 1.0],
    });
    validate_preserved_plate_bounds(&sinking, 0, 1, TARGET_BED_SIZE_MM).unwrap();

    let fully_below_bed = plate_with_bounds(AxisAlignedBounds {
        min: [TARGET_MIN_X_MM, TARGET_MIN_Y_MM, -2.0],
        max: [TARGET_MAX_X_MM, TARGET_MAX_Y_MM, -0.1],
    });
    let error =
        validate_preserved_plate_bounds(&fully_below_bed, 0, 1, TARGET_BED_SIZE_MM).unwrap_err();
    assert!(matches!(error, U1DirectError::Plan(_)));
    assert!(error.to_string().contains("build volume"));

    let outside_x = plate_with_bounds(AxisAlignedBounds {
        min: [TARGET_MIN_X_MM - 0.03, TARGET_MIN_Y_MM, 0.0],
        max: [TARGET_MAX_X_MM, TARGET_MAX_Y_MM, 1.0],
    });
    assert!(validate_preserved_plate_bounds(&outside_x, 0, 1, TARGET_BED_SIZE_MM).is_err());
}

#[test]
fn multicolor_prime_tower_position_uses_a_bed_guarded_conservative_envelope() {
    let plate = plate_with_bounds(AxisAlignedBounds {
        min: [100.0, 100.0, 0.0],
        max: [110.0, 110.0, 10.0],
    });
    let position = choose_wipe_tower_position(&plate, (0.0, 0.0), 4).unwrap();
    let envelope = conservative_prime_tower_envelope(position.0, position.1, 10.0);

    assert!(envelope[0] >= TARGET_MIN_X_MM);
    assert!(envelope[1] >= TARGET_MIN_Y_MM);
    assert!(envelope[2] <= TARGET_MAX_X_MM);
    assert!(envelope[3] <= TARGET_MAX_Y_MM);

    let maximum_height_default =
        conservative_prime_tower_envelope(40.0, 200.0, TARGET_PRINTABLE_HEIGHT_MM);
    assert!(maximum_height_default[3] > TARGET_MAX_Y_MM);

    assert_eq!(
        choose_wipe_tower_position(&plate, (0.0, 0.0), 1).unwrap(),
        (40.0, 200.0)
    );
}

#[test]
fn virtual_plate_grid_matches_the_2_3_5_column_and_gap_contract() {
    for (index, expected) in [
        (0, (0.0, 0.0)),
        (1, (307.2, 0.0)),
        (3, (921.6, 0.0)),
        (4, (0.0, -307.2)),
        (11, (921.6, -614.4)),
    ] {
        let actual = virtual_plate_origin(index, 12, 256.0);
        assert!((actual.0 - expected.0).abs() < 1.0e-9);
        assert!((actual.1 - expected.1).abs() < 1.0e-9);
    }
}

#[test]
fn staged_bundle_allowlist_rejects_hidden_or_unmanifested_files() {
    let temporary = TempDir::new().unwrap();
    for name in [
        "plate.3mf",
        "manifest.json",
        "print-plan.json",
        "checksums.sha256",
    ] {
        fs::write(temporary.path().join(name), b"fixture").unwrap();
    }
    let artifacts = vec![PublishedArtifact {
        batch_id: "batch-1".into(),
        file_name: "plate.3mf".into(),
        path: temporary.path().join("plate.3mf"),
        byte_size: 7,
        sha256: "fixture".into(),
        plate_count: 1,
        target_plate_ids: vec!["plate-1".into()],
        source_unit_ids: vec!["unit-1".into()],
    }];
    validate_bundle_entries(temporary.path(), &artifacts).unwrap();

    fs::write(temporary.path().join(".source-copy.3mf"), b"secret").unwrap();
    let error = validate_bundle_entries(temporary.path(), &artifacts).unwrap_err();
    assert!(matches!(error, U1DirectError::Publish(_)));
    assert!(
        error
            .to_string()
            .contains("does not match the strict allowlist")
    );
}

#[test]
fn durable_setup_actions_are_human_readable_and_resolve_spool_names() {
    let inventory = vec![spool(
        "black",
        "Panchroma Black",
        Material::Pla,
        color(8, 10, 13),
        Some("Polymaker General PLA Family @U1"),
    )];
    let action = SetupAction {
        phase: SetupPhase::BeforeBatch,
        kind: SetupActionKind::Load,
        toolhead: Some(Toolhead::T4),
        from: ToolheadSlotState::Empty,
        to: ToolheadSlotState::Loaded("black".into()),
    };

    assert_eq!(
        format_setup_action(&action, &inventory),
        "Before batch — T4 — Load: empty → Panchroma Black (black)"
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn atomic_publication_never_replaces_an_existing_bundle_directory() {
    let temporary = TempDir::new().unwrap();
    let staged = temporary.path().join("staged");
    let existing = temporary.path().join("project__converted");
    fs::create_dir(&staged).unwrap();
    fs::create_dir(&existing).unwrap();
    fs::write(staged.join("new-output.3mf"), b"new bundle").unwrap();
    fs::write(existing.join("user-file.txt"), b"must survive").unwrap();

    let error = publish_directory_no_clobber(&staged, &existing).unwrap_err();

    assert!(
        error.kind() == io::ErrorKind::AlreadyExists
            || matches!(
                error.raw_os_error(),
                Some(code) if code == libc::EEXIST || code == libc::ENOTEMPTY
            ),
        "unexpected no-clobber error: {error}"
    );
    assert_eq!(
        fs::read(existing.join("user-file.txt")).unwrap(),
        b"must survive"
    );
    assert!(staged.join("new-output.3mf").is_file());
    assert!(!existing.join("new-output.3mf").exists());
}

#[test]
fn cooperative_cancel_interrupts_streaming_model_rewrite() {
    struct CancellingReader {
        inner: Cursor<Vec<u8>>,
        control: U1DirectConversionControl,
        requested: bool,
    }

    impl Read for CancellingReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let read = self.inner.read(buffer)?;
            if read > 0 && !self.requested {
                self.requested = true;
                assert!(self.control.cancel());
            }
            Ok(read)
        }
    }

    let mut plan = artifact_plan_fixture(&[1], &[42]);
    plan.selected_root_resource_ids.insert(42);
    let source = br#"<?xml version="1.0"?>
        <model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
          <resources><object id="42" type="model"><mesh>
            <vertices><vertex x="0" y="0" z="0"/></vertices>
            <triangles><triangle v1="0" v2="0" v3="0"/></triangles>
          </mesh></object></resources><build/>
        </model>"#
        .to_vec();
    let control = U1DirectConversionControl::new();
    let reader = CancellingReader {
        inner: Cursor::new(source),
        control: control.clone(),
        requested: false,
    };
    let mut output = Vec::new();
    let mut geometry = GeometryCounts::default();

    let error = rewrite_model_xml(
        reader,
        &mut output,
        MAIN_MODEL_PATH,
        &plan,
        None,
        &mut geometry,
        &control,
    )
    .expect_err("a cancellation requested by the streaming reader must stop the rewrite");

    assert!(matches!(error, U1DirectError::Cancelled));
    assert_eq!(control.state(), U1DirectConversionState::Cancelled);
}

#[test]
fn cancel_before_publication_leaves_no_final_or_staged_artifact() {
    let destination = TempDir::new().unwrap();
    let control = U1DirectConversionControl::new();
    let record = control.recovery_record(destination.path()).unwrap();
    let (staging_path, marker_path) = staging_paths(&record).unwrap();
    let final_directory = destination.path().join("valid-existing-name__converted");
    let mut staging = OwnedConversionStaging::create(destination.path(), &control).unwrap();
    fs::write(staging.path().join("partial.3mf"), b"partial").unwrap();

    assert!(control.cancel());
    let error = publish_owned_staging(&mut staging, &final_directory, &control)
        .expect_err("cancel must win before the atomic publication boundary");
    assert!(matches!(error, U1DirectError::Cancelled));
    drop(staging);

    assert!(!final_directory.exists());
    assert!(!staging_path.exists());
    assert!(!marker_path.exists());
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn cancellation_cannot_remove_an_atomically_published_bundle() {
    let destination = TempDir::new().unwrap();
    let control = U1DirectConversionControl::new();
    let record = control.recovery_record(destination.path()).unwrap();
    let (_, marker_path) = staging_paths(&record).unwrap();
    let final_directory = destination.path().join("fixture__converted");
    let mut staging = OwnedConversionStaging::create(destination.path(), &control).unwrap();
    fs::write(staging.path().join("complete.3mf"), b"complete").unwrap();

    publish_owned_staging(&mut staging, &final_directory, &control).unwrap();
    assert!(!control.cancel());
    assert_eq!(control.state(), U1DirectConversionState::Published);
    drop(staging);

    assert_eq!(
        fs::read(final_directory.join("complete.3mf")).unwrap(),
        b"complete"
    );
    assert!(!marker_path.exists());
}

#[test]
fn abandoned_staging_cleanup_requires_its_secret_marker_and_preserves_lookalikes() {
    let destination = TempDir::new().unwrap();
    let sentinel = destination.path().join("user-file.txt");
    fs::write(&sentinel, b"keep me").unwrap();
    let lookalike = destination.path().join(".u1-3mf-conversion-user-created");
    fs::create_dir(&lookalike).unwrap();
    fs::write(lookalike.join("keep.txt"), b"keep me too").unwrap();

    let control = U1DirectConversionControl::new();
    let mut record = control.recovery_record(destination.path()).unwrap();
    record.owner_process_id = u32::MAX;
    let (staging_path, marker_path) = staging_paths(&record).unwrap();
    let staging = OwnedConversionStaging::create(destination.path(), &control).unwrap();
    fs::write(staging.path().join("abandoned.3mf"), b"partial").unwrap();
    std::mem::forget(staging);

    assert_eq!(
        cleanup_abandoned_u1_direct_staging(&record).unwrap(),
        U1DirectStagingCleanup::Removed
    );
    assert!(!staging_path.exists());
    assert!(!marker_path.exists());
    assert_eq!(fs::read(&sentinel).unwrap(), b"keep me");
    assert_eq!(
        fs::read(lookalike.join("keep.txt")).unwrap(),
        b"keep me too"
    );

    let forged_control = U1DirectConversionControl::new();
    let mut forged_record = forged_control.recovery_record(destination.path()).unwrap();
    forged_record.owner_process_id = u32::MAX;
    let (forged_staging, _) = staging_paths(&forged_record).unwrap();
    fs::create_dir(&forged_staging).unwrap();
    fs::write(forged_staging.join("user-file.txt"), b"not ours").unwrap();

    assert!(cleanup_abandoned_u1_direct_staging(&forged_record).is_err());
    assert_eq!(
        fs::read(forged_staging.join("user-file.txt")).unwrap(),
        b"not ours"
    );
}
