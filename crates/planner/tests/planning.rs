use std::collections::BTreeSet;

use u1_planner::{
    A1MiniConfig, BestEffortCmyxCandidate, BoundsMm, BuildVolumeMm, CmySetup, CmyxColorCandidate,
    CmyxFallbackApproval, CmyxRecipe, ColorConfidence, ColorStrategy, CurrentToolheadState,
    DirectIneligibility, DirectSpoolCandidate, DirectSpoolEligibility, ErrorCode, FullSpectrumMode,
    FullSpectrumProcessCompatibility, FullSpectrumSubdivisionPolicy, MappingStatus, Material,
    MaterialColorRequirement, MaterialRole, MaterialSubstitutionApproval, PackingStatus,
    PlannerConfig, PlanningInput, PrintScope, PrintableUnit, Printer, PrinterLoadout,
    PrinterPreference, RgbColor, ScopeStrategy, SetupActionKind, Spool, Toolhead,
    ToolheadSlotState, WarningCode, plan,
};

fn rgb(red: u8, green: u8, blue: u8) -> RgbColor {
    RgbColor::new(red, green, blue)
}

fn spool(id: &str, material: Material, color: RgbColor) -> Spool {
    Spool {
        id: id.into(),
        calibration_id: None,
        display_name: id.into(),
        color_name: Some(id.into()),
        material,
        nominal_color: color,
        measured_color: Some(color),
        sku: Some(format!("sku-{id}")),
        profile_id: Some(format!("profile-{id}")),
        available: true,
    }
}

fn base_inventory() -> Vec<Spool> {
    vec![
        spool("cmy-c", Material::Pla, rgb(0, 180, 220)),
        spool("cmy-m", Material::Pla, rgb(220, 0, 150)),
        spool("cmy-y", Material::Pla, rgb(245, 225, 0)),
        spool("cmy-grey", Material::Pla, rgb(128, 128, 128)),
    ]
}

fn config() -> PlannerConfig {
    let mut config = PlannerConfig::with_cmy_setup(CmySetup {
        cyan_spool_id: "cmy-c".into(),
        magenta_spool_id: "cmy-m".into(),
        yellow_spool_id: "cmy-y".into(),
        default_t4_spool_id: Some("cmy-grey".into()),
    });
    config.supported_full_spectrum_processes = vec![full_spectrum_process()];
    config
}

fn full_spectrum_process() -> FullSpectrumProcessCompatibility {
    FullSpectrumProcessCompatibility {
        printer_profile_fingerprint: "snapmaker-u1-profile-a".into(),
        plate_layer_height_microns: 80,
        subdivision_policy: FullSpectrumSubdivisionPolicy::SubdivideMixLayer,
        subdivision_factor: 4,
        effective_sublayer_height_microns: 20,
        process_fingerprint: "snapmaker-u1-process-a".into(),
    }
}

fn current_cmy(t4: ToolheadSlotState) -> CurrentToolheadState {
    CurrentToolheadState {
        slots: [
            ToolheadSlotState::Loaded("cmy-c".into()),
            ToolheadSlotState::Loaded("cmy-m".into()),
            ToolheadSlotState::Loaded("cmy-y".into()),
            t4,
        ],
    }
}

fn mixed_candidate(source: RgbColor) -> CmyxColorCandidate {
    CmyxColorCandidate {
        recipe: CmyxRecipe::FullSpectrum {
            mode: FullSpectrumMode::Ratio,
            sequence: vec![Toolhead::T1, Toolhead::T2, Toolhead::T3],
        },
        calibration_sample_id: None,
        process_compatibility: Some(full_spectrum_process()),
        required_t4_spool_id: None,
        predicted_color: Some(source),
        delta_e00: Some(2.0),
        confidence: ColorConfidence::Calibrated,
        warnings: Vec::new(),
    }
}

fn direct_requirement(
    index: usize,
    material: Material,
    color: RgbColor,
    spool_id: &str,
) -> MaterialColorRequirement {
    MaterialColorRequirement {
        id: format!("color-{index}"),
        material,
        role: MaterialRole::Cosmetic,
        source_color: color,
        source_slots: vec![format!("source-slot-{index}")],
        source_profile_ids: vec![format!("profile-{spool_id}")],
        cmyx_candidate: mixed_candidate(color),
        best_effort_cmyx_candidate: None,
        direct_candidates: vec![DirectSpoolCandidate {
            spool_id: spool_id.into(),
            delta_e00: Some(0.0),
            confidence: ColorConfidence::Measured,
        }],
    }
}

fn unit(id: &str, requirement_ids: Vec<String>, height: f64) -> PrintableUnit {
    PrintableUnit {
        id: id.into(),
        source_unit_id: id.into(),
        source_object_id: 1,
        source_instance_id: 0,
        source_model_path: None,
        display_name: id.into(),
        source_plate_id: Some("source-plate".into()),
        requirement_ids,
        bounds: BoundsMm::from_size(50.0, 50.0, height),
        source_layer_height_mm: Some(0.2),
        printer_preference: PrinterPreference::Auto,
    }
}

fn direct_input(color_count: usize) -> PlanningInput {
    let palette = [
        rgb(20, 30, 40),
        rgb(240, 240, 240),
        rgb(220, 80, 10),
        rgb(90, 20, 130),
        rgb(20, 170, 60),
    ];
    let mut inventory = base_inventory();
    let mut requirements = Vec::new();
    for (index, color) in palette.into_iter().take(color_count).enumerate() {
        let spool_id = format!("direct-{index}");
        inventory.push(spool(&spool_id, Material::Pla, color));
        requirements.push(direct_requirement(index, Material::Pla, color, &spool_id));
    }
    let requirement_ids = requirements
        .iter()
        .map(|requirement| requirement.id.clone())
        .collect();
    PlanningInput {
        scopes: vec![PrintScope {
            id: "scope".into(),
            display_name: "Scope".into(),
            requirements,
            units: vec![unit("unit", requirement_ids, 40.0)],
            strategy: ScopeStrategy::DirectSpools,
            direct_assignments: Vec::new(),
            approved_cmyx_fallbacks: Vec::new(),
            approved_material_substitutions: Vec::new(),
        }],
        inventory,
        current_toolheads: current_cmy(ToolheadSlotState::Loaded("cmy-grey".into())),
        config: config(),
    }
}

#[test]
fn direct_spools_accepts_one_through_four_effective_pairs() {
    for color_count in 1..=4 {
        let result = plan(&direct_input(color_count));
        assert!(result.errors.is_empty(), "{:#?}", result.errors);
        assert_eq!(result.scope_options[0].effective_pair_count, color_count);
        assert_eq!(result.scope_options[0].direct_pair_count, color_count);
        let DirectSpoolEligibility::Eligible { assignments } =
            &result.scope_options[0].direct_spools
        else {
            panic!("{color_count} colors should be Direct Spool eligible");
        };
        assert_eq!(assignments.len(), color_count);
        assert_eq!(
            assignments
                .iter()
                .map(|assignment| assignment.spool_id.as_str())
                .collect::<BTreeSet<_>>()
                .len(),
            color_count
        );
        assert_eq!(
            assignments
                .iter()
                .map(|assignment| assignment.toolhead)
                .collect::<BTreeSet<_>>()
                .len(),
            color_count
        );
        assert!(
            result
                .jobs
                .iter()
                .all(|job| job.strategy == ColorStrategy::DirectSpools)
        );
        assert_eq!(result.jobs[0].color_mappings.len(), color_count);
    }
}

#[test]
fn five_role_pairs_with_four_declared_physical_identities_share_one_spool_and_toolhead() {
    let mut input = direct_input(4);
    let mut support = input.scopes[0].requirements[0].clone();
    support.id = "color-0-support".into();
    support.role = MaterialRole::Support;
    input.scopes[0].requirements.push(support);
    input.scopes[0].units[0]
        .requirement_ids
        .push("color-0-support".into());

    let result = plan(&input);

    assert!(result.errors.is_empty(), "{:#?}", result.errors);
    assert_eq!(result.scope_options[0].effective_pair_count, 5);
    assert_eq!(result.scope_options[0].direct_pair_count, 4);
    let DirectSpoolEligibility::Eligible { assignments } = &result.scope_options[0].direct_spools
    else {
        panic!("role-separated semantics should fit four physical Direct identities");
    };
    assert_eq!(assignments.len(), 5);
    assert_eq!(
        assignments
            .iter()
            .map(|assignment| assignment.spool_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        4
    );
    assert_eq!(
        assignments
            .iter()
            .map(|assignment| assignment.toolhead)
            .collect::<BTreeSet<_>>()
            .len(),
        4
    );
    let cosmetic = assignments
        .iter()
        .find(|assignment| assignment.source_requirement_ids == ["color-0"])
        .expect("cosmetic assignment");
    let support = assignments
        .iter()
        .find(|assignment| assignment.source_requirement_ids == ["color-0-support"])
        .expect("support assignment");
    assert_eq!(cosmetic.spool_id, support.spool_id);
    assert_eq!(cosmetic.toolhead, support.toolhead);
    assert_eq!(result.jobs[0].color_mappings.len(), 5);
    assert!(!result.warnings.iter().any(|warning| {
        warning.code == WarningCode::IntentionalColorMerge
            && warning.message.contains("physical spool 'direct-0'")
    }));
}

#[test]
fn unknown_profiles_do_not_merge_across_roles_for_direct_eligibility() {
    let mut input = direct_input(4);
    let mut support = input.scopes[0].requirements[0].clone();
    input.scopes[0].requirements[0].source_profile_ids.clear();
    support.id = "color-0-support".into();
    support.role = MaterialRole::Support;
    support.source_profile_ids.clear();
    input.scopes[0].requirements.push(support);
    input.scopes[0].units[0]
        .requirement_ids
        .push("color-0-support".into());

    let result = plan(&input);

    assert_eq!(result.scope_options[0].effective_pair_count, 5);
    assert_eq!(result.scope_options[0].direct_pair_count, 5);
    assert!(matches!(
        result.scope_options[0].direct_spools,
        DirectSpoolEligibility::Ineligible {
            reason: DirectIneligibility::TooManyEffectivePairs {
                count: 5,
                maximum: 4
            }
        }
    ));
}

#[test]
fn shared_physical_identity_requires_material_risk_consent_for_each_role_pair() {
    let mut input = direct_input(4);
    input.scopes[0].requirements[0].material = Material::Petg;
    let mut support = input.scopes[0].requirements[0].clone();
    support.id = "color-0-support".into();
    support.role = MaterialRole::Support;
    input.scopes[0].requirements.push(support);
    input.scopes[0].units[0]
        .requirement_ids
        .push("color-0-support".into());
    input.scopes[0].direct_assignments = vec![
        u1_planner::DirectAssignmentRequest {
            requirement_id: "color-0".into(),
            spool_id: "direct-0".into(),
            toolhead: Some(Toolhead::T1),
            allow_material_substitution: true,
        },
        u1_planner::DirectAssignmentRequest {
            requirement_id: "color-0-support".into(),
            spool_id: "direct-0".into(),
            toolhead: Some(Toolhead::T1),
            allow_material_substitution: false,
        },
    ];

    let rejected = plan(&input);
    assert!(matches!(
        &rejected.scope_options[0].direct_spools,
        DirectSpoolEligibility::Ineligible {
            reason: DirectIneligibility::InvalidManualAssignment { message }
        } if message.contains("separate explicit mechanical-risk acknowledgement")
            && message.contains("color-0-support")
    ));

    input.scopes[0].direct_assignments[1].allow_material_substitution = true;
    let accepted = plan(&input);
    assert!(accepted.errors.is_empty(), "{:#?}", accepted.errors);
    let DirectSpoolEligibility::Eligible { assignments } = &accepted.scope_options[0].direct_spools
    else {
        panic!("both role-specific acknowledgements should permit the shared spool");
    };
    let shared = assignments
        .iter()
        .filter(|assignment| assignment.spool_id == "direct-0")
        .collect::<Vec<_>>();
    assert_eq!(shared.len(), 2);
    assert_eq!(shared[0].toolhead, shared[1].toolhead);
}

#[test]
fn forced_direct_material_change_requires_an_explicit_acknowledgement() {
    let mut input = direct_input(1);
    input.scopes[0].requirements[0].material = Material::Petg;
    input.scopes[0].direct_assignments = vec![u1_planner::DirectAssignmentRequest {
        requirement_id: "color-0".into(),
        spool_id: "direct-0".into(),
        toolhead: None,
        allow_material_substitution: false,
    }];

    let rejected = plan(&input);
    assert!(matches!(
        &rejected.scope_options[0].direct_spools,
        DirectSpoolEligibility::Ineligible {
            reason: DirectIneligibility::InvalidManualAssignment { message }
        } if message.contains("mechanical-risk acknowledgement")
    ));
    assert!(
        rejected
            .errors
            .iter()
            .any(|error| error.code == ErrorCode::RequestedDirectSpoolsUnavailable)
    );

    input.scopes[0].direct_assignments[0].allow_material_substitution = true;
    let accepted = plan(&input);
    assert!(accepted.errors.is_empty(), "{:#?}", accepted.errors);
    let mapping = &accepted.jobs[0].color_mappings[0];
    assert_eq!(mapping.source_material, Material::Petg);
    assert_eq!(mapping.actual_material, Some(Material::Pla));
    assert_eq!(mapping.status, MappingStatus::MaterialMismatch);
    assert!(accepted.warnings.iter().any(|warning| {
        warning.code == WarningCode::ApprovedMaterialSubstitution
            && warning.message.contains("Direct Spools")
    }));
}

#[test]
fn automatic_direct_matching_never_changes_material_family() {
    let mut input = direct_input(1);
    input.scopes[0].requirements[0].material = Material::Petg;

    let result = plan(&input);
    assert!(matches!(
        result.scope_options[0].direct_spools,
        DirectSpoolEligibility::Ineligible {
            reason: DirectIneligibility::MissingCompatibleSpools {
                material: Material::Petg
            }
        }
    ));
}

#[test]
fn identical_material_colors_with_distinct_profiles_remain_distinct_requirements() {
    let color = rgb(80, 90, 100);
    let mut inventory = base_inventory();
    inventory.push(spool("basic", Material::Pla, color));
    inventory.push(spool("matte", Material::Pla, color));
    let requirements = vec![
        direct_requirement(0, Material::Pla, color, "basic"),
        direct_requirement(1, Material::Pla, color, "matte"),
    ];
    let input = PlanningInput {
        scopes: vec![PrintScope {
            id: "scope".into(),
            display_name: "Scope".into(),
            requirements,
            units: vec![unit("unit", vec!["color-0".into(), "color-1".into()], 40.0)],
            strategy: ScopeStrategy::DirectSpools,
            direct_assignments: Vec::new(),
            approved_cmyx_fallbacks: Vec::new(),
            approved_material_substitutions: Vec::new(),
        }],
        inventory,
        current_toolheads: CurrentToolheadState::default(),
        config: config(),
    };

    let result = plan(&input);

    assert!(result.errors.is_empty(), "{:#?}", result.errors);
    assert_eq!(result.scope_options[0].effective_pair_count, 2);
    let DirectSpoolEligibility::Eligible { assignments } = &result.scope_options[0].direct_spools
    else {
        panic!("profile-distinct requirements should remain Direct Spool eligible");
    };
    assert_eq!(assignments.len(), 2);
    assert!(assignments.iter().any(|assignment| {
        assignment.spool_id == "basic" && assignment.source_profile_ids == vec!["profile-basic"]
    }));
    assert!(assignments.iter().any(|assignment| {
        assignment.spool_id == "matte" && assignment.source_profile_ids == vec!["profile-matte"]
    }));
}

#[test]
fn source_profile_identity_does_not_block_a_compatible_target_spool() {
    let mut input = direct_input(1);
    input
        .inventory
        .iter_mut()
        .find(|spool| spool.id == "direct-0")
        .unwrap()
        .profile_id = Some("wrong-profile".into());

    let result = plan(&input);

    let DirectSpoolEligibility::Eligible { assignments } = &result.scope_options[0].direct_spools
    else {
        panic!("an explicit source profile may map to a compatible target material profile");
    };
    assert_eq!(assignments.len(), 1);
    assert_eq!(assignments[0].spool_id, "direct-0");
    assert_eq!(assignments[0].source_profile_ids, vec!["profile-direct-0"]);
}

#[test]
fn direct_mono_can_route_to_a1_with_its_confirmed_spool() {
    let mut input = direct_input(1);
    input.config.a1_mini.enabled = true;

    let result = plan(&input);

    assert!(result.errors.is_empty(), "{:#?}", result.errors);
    assert_eq!(result.jobs.len(), 1);
    assert_eq!(result.jobs[0].printer, Printer::A1Mini);
    assert_eq!(result.jobs[0].strategy, ColorStrategy::A1Mono);
    assert!(matches!(
        &result.jobs[0].loadout,
        PrinterLoadout::A1Mini { spool_id } if spool_id == "direct-0"
    ));
    assert!(
        result.jobs[0]
            .color_mappings
            .iter()
            .all(|mapping| mapping.strategy == ColorStrategy::A1Mono
                && mapping.direct_toolhead.is_none())
    );
}

#[test]
fn explicitly_merged_source_colors_share_one_spool_and_route_as_a1_mono() {
    let mut input = direct_input(2);
    input.scopes[0].direct_assignments = vec![
        u1_planner::DirectAssignmentRequest {
            requirement_id: "color-0".into(),
            spool_id: "direct-0".into(),
            toolhead: Some(Toolhead::T4),
            allow_material_substitution: false,
        },
        u1_planner::DirectAssignmentRequest {
            requirement_id: "color-1".into(),
            spool_id: "direct-0".into(),
            toolhead: Some(Toolhead::T4),
            allow_material_substitution: false,
        },
    ];
    input.config.a1_mini.enabled = true;

    let result = plan(&input);

    assert!(result.errors.is_empty(), "{:#?}", result.errors);
    let DirectSpoolEligibility::Eligible { assignments } = &result.scope_options[0].direct_spools
    else {
        panic!("an explicit many-to-one assignment should remain eligible");
    };
    assert_eq!(assignments.len(), 2);
    assert!(assignments.iter().all(|assignment| {
        assignment.spool_id == "direct-0" && assignment.toolhead == Toolhead::T4
    }));
    assert_eq!(result.jobs.len(), 1);
    assert_eq!(result.jobs[0].printer, Printer::A1Mini);
    assert_eq!(result.jobs[0].strategy, ColorStrategy::A1Mono);
    assert!(matches!(
        &result.jobs[0].loadout,
        PrinterLoadout::A1Mini { spool_id } if spool_id == "direct-0"
    ));
    assert!(result.warnings.iter().any(|warning| {
        warning.code == WarningCode::IntentionalColorMerge
            && warning.message.contains("2 source colors")
    }));
}

#[test]
fn a1_does_not_share_a_physical_spool_with_a_u1_job() {
    let mut input = direct_input(2);
    input.scopes[0].units.push(unit(
        "mono-sharing-direct-zero",
        vec!["color-0".into()],
        20.0,
    ));
    input.config.a1_mini.enabled = true;

    let result = plan(&input);

    assert!(result.errors.is_empty(), "{:#?}", result.errors);
    assert!(result.jobs.iter().all(|job| job.printer == Printer::U1));
    assert!(result.warnings.iter().any(|warning| {
        warning.code == WarningCode::A1SpoolReservedForU1
            && warning.unit_id.as_deref() == Some("mono-sharing-direct-zero")
    }));
}

#[test]
fn explicit_a1_pin_does_not_silently_fall_back_on_spool_conflict() {
    let mut input = direct_input(2);
    let mut pinned = unit("pinned-a1", vec!["color-0".into()], 20.0);
    pinned.printer_preference = PrinterPreference::A1Mini;
    input.scopes[0].units.push(pinned);
    input.config.a1_mini.enabled = true;

    let result = plan(&input);

    assert!(result.errors.iter().any(|error| {
        error.code == ErrorCode::RequestedA1Unavailable
            && error.unit_id.as_deref() == Some("pinned-a1")
            && error.message.contains("also committed to the U1 schedule")
    }));
    assert!(result.jobs.iter().all(|job| job.printer == Printer::U1));
    assert!(!result.warnings.iter().any(|warning| {
        warning.code == WarningCode::A1SpoolReservedForU1
            && warning.unit_id.as_deref() == Some("pinned-a1")
    }));
}

#[test]
fn fifth_effective_pair_rejects_direct_spools() {
    let result = plan(&direct_input(5));
    assert_eq!(result.scope_options[0].direct_pair_count, 5);
    assert!(matches!(
        result.scope_options[0].direct_spools,
        DirectSpoolEligibility::Ineligible {
            reason: DirectIneligibility::TooManyEffectivePairs {
                count: 5,
                maximum: 4
            }
        }
    ));
    assert!(result.jobs.is_empty());
    assert!(
        result
            .errors
            .iter()
            .any(|error| { error.code == ErrorCode::RequestedDirectSpoolsUnavailable })
    );
}

#[test]
fn identical_rgb_in_different_materials_stays_distinct_and_separate() {
    let color = rgb(12, 34, 56);
    let mut input = direct_input(0);
    input
        .inventory
        .push(spool("pla-match", Material::Pla, color));
    input
        .inventory
        .push(spool("petg-match", Material::Petg, color));
    input.scopes[0].requirements = vec![
        direct_requirement(0, Material::Pla, color, "pla-match"),
        direct_requirement(1, Material::Petg, color, "petg-match"),
    ];
    input.scopes[0].units = vec![
        unit("pla-unit", vec!["color-0".into()], 20.0),
        unit("petg-unit", vec!["color-1".into()], 20.0),
    ];

    let result = plan(&input);
    assert!(result.errors.is_empty(), "{:#?}", result.errors);
    assert_eq!(result.scope_options[0].effective_pair_count, 2);
    let DirectSpoolEligibility::Eligible { assignments } = &result.scope_options[0].direct_spools
    else {
        panic!("scope should be eligible");
    };
    assert_ne!(assignments[0].spool_id, assignments[1].spool_id);
    assert_eq!(result.jobs.len(), 2);
    assert!(
        result
            .jobs
            .iter()
            .all(|job| job.printable_materials.len() == 1)
    );
}

#[test]
fn unknown_initial_t4_counts_as_a_swap_and_emits_conservative_actions() {
    let black = rgb(5, 5, 5);
    let mut inventory = base_inventory();
    inventory.push(spool("black", Material::Pla, black));
    let requirement = MaterialColorRequirement {
        id: "black".into(),
        material: Material::Pla,
        role: MaterialRole::Functional,
        source_color: black,
        source_slots: vec!["4".into()],
        source_profile_ids: vec!["profile-black".into()],
        cmyx_candidate: CmyxColorCandidate {
            recipe: CmyxRecipe::DedicatedT4,
            calibration_sample_id: None,
            process_compatibility: None,
            required_t4_spool_id: Some("black".into()),
            predicted_color: Some(black),
            delta_e00: Some(0.0),
            confidence: ColorConfidence::Measured,
            warnings: Vec::new(),
        },
        best_effort_cmyx_candidate: None,
        direct_candidates: Vec::new(),
    };
    let input = PlanningInput {
        scopes: vec![PrintScope {
            id: "scope".into(),
            display_name: "Scope".into(),
            requirements: vec![requirement],
            units: vec![unit("unit", vec!["black".into()], 20.0)],
            strategy: ScopeStrategy::CmyxFullSpectrum,
            direct_assignments: Vec::new(),
            approved_cmyx_fallbacks: Vec::new(),
            approved_material_substitutions: Vec::new(),
        }],
        inventory,
        current_toolheads: current_cmy(ToolheadSlotState::Unknown),
        config: config(),
    };

    let result = plan(&input);
    assert!(result.errors.is_empty(), "{:#?}", result.errors);
    assert_eq!(result.t4_swap_count, 1);
    assert!(result.batches[0].t4_swap_before);
    assert!(result.batches[0].setup_actions.iter().any(|action| {
        action.toolhead == Some(Toolhead::T4)
            && action.kind == SetupActionKind::Unload
            && action.from == ToolheadSlotState::Unknown
    }));
    assert!(
        result
            .warnings
            .iter()
            .any(|warning| { warning.code == WarningCode::UnknownCurrentToolhead })
    );
}

#[test]
fn a1_rejects_a_unit_taller_than_180_mm_and_leaves_it_on_u1() {
    let black = rgb(5, 5, 5);
    let mut inventory = base_inventory();
    inventory.push(spool("black", Material::Pla, black));
    let requirement = MaterialColorRequirement {
        id: "black".into(),
        material: Material::Pla,
        role: MaterialRole::Functional,
        source_color: black,
        source_slots: vec!["4".into()],
        source_profile_ids: vec!["profile-black".into()],
        cmyx_candidate: CmyxColorCandidate {
            recipe: CmyxRecipe::DedicatedT4,
            calibration_sample_id: None,
            process_compatibility: None,
            required_t4_spool_id: Some("black".into()),
            predicted_color: Some(black),
            delta_e00: Some(0.0),
            confidence: ColorConfidence::Measured,
            warnings: Vec::new(),
        },
        best_effort_cmyx_candidate: None,
        direct_candidates: Vec::new(),
    };
    let mut planner_config = config();
    planner_config.a1_mini = A1MiniConfig {
        enabled: true,
        route_fast_mono: true,
        build_volume: BuildVolumeMm {
            width: 180.0,
            depth: 180.0,
            height: 180.0,
        },
        supported_materials: vec![Material::Pla],
        reserved_spool_ids: Vec::new(),
    };
    let input = PlanningInput {
        scopes: vec![PrintScope {
            id: "scope".into(),
            display_name: "Scope".into(),
            requirements: vec![requirement],
            units: vec![unit("too-tall", vec!["black".into()], 181.0)],
            strategy: ScopeStrategy::CmyxFullSpectrum,
            direct_assignments: Vec::new(),
            approved_cmyx_fallbacks: Vec::new(),
            approved_material_substitutions: Vec::new(),
        }],
        inventory,
        current_toolheads: current_cmy(ToolheadSlotState::Loaded("black".into())),
        config: planner_config,
    };

    let result = plan(&input);
    assert!(result.errors.is_empty(), "{:#?}", result.errors);
    assert_eq!(result.jobs.len(), 1);
    assert_eq!(result.jobs[0].printer, Printer::U1);
    assert!(
        result
            .warnings
            .iter()
            .any(|warning| { warning.code == WarningCode::A1OutOfBounds })
    );
}

#[test]
fn a1_routes_an_eligible_single_physical_spool_unit() {
    let black = rgb(5, 5, 5);
    let mut inventory = base_inventory();
    inventory.push(spool("black", Material::Pla, black));
    let requirement = MaterialColorRequirement {
        id: "black".into(),
        material: Material::Pla,
        role: MaterialRole::Functional,
        source_color: black,
        source_slots: vec!["4".into()],
        source_profile_ids: vec!["profile-black".into()],
        cmyx_candidate: CmyxColorCandidate {
            recipe: CmyxRecipe::DedicatedT4,
            calibration_sample_id: None,
            process_compatibility: None,
            required_t4_spool_id: Some("black".into()),
            predicted_color: Some(black),
            delta_e00: Some(0.0),
            confidence: ColorConfidence::Measured,
            warnings: Vec::new(),
        },
        best_effort_cmyx_candidate: None,
        direct_candidates: Vec::new(),
    };
    let mut planner_config = config();
    planner_config.a1_mini.enabled = true;
    let input = PlanningInput {
        scopes: vec![PrintScope {
            id: "scope".into(),
            display_name: "Scope".into(),
            requirements: vec![requirement],
            units: vec![unit("a1-unit", vec!["black".into()], 180.0)],
            strategy: ScopeStrategy::CmyxFullSpectrum,
            direct_assignments: Vec::new(),
            approved_cmyx_fallbacks: Vec::new(),
            approved_material_substitutions: Vec::new(),
        }],
        inventory,
        current_toolheads: current_cmy(ToolheadSlotState::Loaded("black".into())),
        config: planner_config,
    };

    let result = plan(&input);
    assert!(result.errors.is_empty(), "{:#?}", result.errors);
    assert_eq!(result.jobs.len(), 1);
    assert_eq!(result.jobs[0].printer, Printer::A1Mini);
    assert_eq!(result.jobs[0].strategy, ColorStrategy::A1Mono);
}

#[test]
fn unknown_bounds_remain_provisional_and_never_route_to_a1() {
    let mut input = direct_input(1);
    input.scopes[0].strategy = ScopeStrategy::CmyxFullSpectrum;
    input.scopes[0].units[0].bounds = BoundsMm::from_size(0.0, 0.0, 0.0);
    input.scopes[0].requirements[0].cmyx_candidate.recipe = CmyxRecipe::Solid {
        toolhead: Toolhead::T1,
    };
    input.config.a1_mini.enabled = true;

    let result = plan(&input);
    assert!(result.errors.is_empty(), "{:#?}", result.errors);
    assert!(result.jobs.iter().all(|job| job.printer == Printer::U1));
    assert!(
        result
            .plates
            .iter()
            .all(|plate| !plate.individual_bounds_validated)
    );
    assert!(
        result
            .warnings
            .iter()
            .any(|warning| warning.code == WarningCode::UnknownBounds)
    );
}

#[test]
fn provisional_packing_preserves_source_plate_boundaries() {
    let mut input = direct_input(1);
    input.scopes[0].id = "scope-a".into();
    input.scopes[0].units[0].source_plate_id = Some("source-plate-a".into());
    input.scopes[0].units[0].bounds = BoundsMm::from_size(0.0, 0.0, 0.0);

    let mut second = input.scopes[0].clone();
    second.id = "scope-b".into();
    second.units[0].source_plate_id = Some("source-plate-b".into());
    input.scopes.push(second);

    let result = plan(&input);
    assert!(result.errors.is_empty(), "{:#?}", result.errors);
    assert_eq!(result.jobs.len(), 2);
    assert_eq!(result.plates.len(), 2);
    assert_eq!(
        result.batches.len(),
        1,
        "the shared loadout stays one batch"
    );
    assert!(result.jobs.iter().all(|job| job.scope_ids.len() == 1));
    assert_eq!(
        result
            .jobs
            .iter()
            .flat_map(|job| job.scope_ids.iter().map(String::as_str))
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["scope-a", "scope-b"])
    );
}

#[test]
fn proven_units_from_split_scopes_share_a_deterministically_packed_u1_plate() {
    let mut input = direct_input(1);
    input.scopes[0].id = "scope-a".into();
    input.scopes[0].units[0].id = "unit-a".into();
    input.scopes[0].units[0].source_unit_id = "source-unit-a".into();
    input.scopes[0].units[0].source_plate_id = Some("source-plate-a".into());
    input.scopes[0].units[0].bounds = BoundsMm {
        width: 60.0,
        depth: 40.0,
        height: 20.0,
        clearance_x: 5.0,
        clearance_y: 5.0,
        clearance_z: 0.5,
    };

    let mut second = input.scopes[0].clone();
    second.id = "scope-b".into();
    second.units[0].id = "unit-b".into();
    second.units[0].source_unit_id = "source-unit-b".into();
    second.units[0].source_object_id = 2;
    second.units[0].source_plate_id = Some("source-plate-a".into());
    input.scopes.push(second);

    let result = plan(&input);
    assert!(result.errors.is_empty(), "{:#?}", result.errors);
    assert_eq!(result.jobs.len(), 1);
    assert_eq!(result.plates.len(), 1);
    let plate = &result.plates[0];
    assert_eq!(plate.packing_status, PackingStatus::PackedAabb);
    assert_eq!(plate.placements.len(), 2);
    assert!(plate.prime_tower.is_none());

    let left = &plate.placements[0];
    let right = &plate.placements[1];
    let separated = left.target_min_x_mm + 65.0 <= right.target_min_x_mm - 5.0
        || right.target_min_x_mm + 65.0 <= left.target_min_x_mm - 5.0
        || left.target_min_y_mm + 45.0 <= right.target_min_y_mm - 5.0
        || right.target_min_y_mm + 45.0 <= left.target_min_y_mm - 5.0;
    assert!(separated, "packed clearance envelopes must not overlap");
}

#[test]
fn packer_splits_a_shared_loadout_across_as_many_plates_as_needed() {
    let mut input = direct_input(1);
    input.scopes[0].id = "scope-a".into();
    input.scopes[0].units[0].id = "unit-a".into();
    input.scopes[0].units[0].source_unit_id = "source-unit-a".into();
    input.scopes[0].units[0].bounds = BoundsMm::from_size(150.0, 150.0, 20.0);

    let mut second = input.scopes[0].clone();
    second.id = "scope-b".into();
    second.units[0].id = "unit-b".into();
    second.units[0].source_unit_id = "source-unit-b".into();
    second.units[0].source_object_id = 2;
    input.scopes.push(second);

    let result = plan(&input);
    assert!(result.errors.is_empty(), "{:#?}", result.errors);
    assert_eq!(result.jobs.len(), 2);
    assert_eq!(result.plates.len(), 2);
    assert_eq!(result.batches.len(), 1, "the loadout remains one batch");
    assert!(
        result
            .warnings
            .iter()
            .any(|warning| warning.code == WarningCode::PackingSplitAcrossPlates)
    );
    assert!(result.plates.iter().all(|plate| {
        plate.packing_status == PackingStatus::PackedAabb && plate.placements.len() == 1
    }));
}

#[test]
fn multitool_u1_packing_reserves_the_qualified_prime_tower_envelope() {
    let result = plan(&direct_input(2));
    assert!(result.errors.is_empty(), "{:#?}", result.errors);
    let plate = &result.plates[0];
    let tower = plate
        .prime_tower
        .expect("a two-toolhead Direct plate needs a prime tower");
    assert!((tower.x_mm - 205.9).abs() < 1.0e-9);
    assert!((tower.y_mm - 198.9).abs() < 1.0e-9);
    assert_eq!(plate.packing_status, PackingStatus::PackedAabb);
}

#[test]
fn direct_batch_reports_keep_unload_load_and_restore_actions() {
    let mut input = direct_input(2);
    input.scopes[0].direct_assignments = vec![
        u1_planner::DirectAssignmentRequest {
            requirement_id: "color-0".into(),
            spool_id: "cmy-c".into(),
            toolhead: Some(Toolhead::T1),
            allow_material_substitution: false,
        },
        u1_planner::DirectAssignmentRequest {
            requirement_id: "color-1".into(),
            spool_id: "direct-1".into(),
            toolhead: Some(Toolhead::T2),
            allow_material_substitution: false,
        },
    ];

    let result = plan(&input);
    assert!(result.errors.is_empty(), "{:#?}", result.errors);
    let kinds: BTreeSet<_> = result.batches[0]
        .setup_actions
        .iter()
        .map(|action| action.kind)
        .collect();
    assert!(kinds.contains(&SetupActionKind::Keep));
    assert!(kinds.contains(&SetupActionKind::Unload));
    assert!(kinds.contains(&SetupActionKind::Load));
    assert!(kinds.contains(&SetupActionKind::Restore));
}

#[test]
fn fast_mono_and_full_spectrum_units_are_separate_jobs() {
    let black = rgb(5, 5, 5);
    let orange = rgb(230, 100, 20);
    let mut inventory = base_inventory();
    inventory.push(spool("black", Material::Pla, black));
    let solid = MaterialColorRequirement {
        id: "solid".into(),
        material: Material::Pla,
        role: MaterialRole::Functional,
        source_color: black,
        source_slots: vec!["4".into()],
        source_profile_ids: vec!["profile-black".into()],
        cmyx_candidate: CmyxColorCandidate {
            recipe: CmyxRecipe::DedicatedT4,
            calibration_sample_id: None,
            process_compatibility: None,
            required_t4_spool_id: Some("black".into()),
            predicted_color: Some(black),
            delta_e00: Some(0.0),
            confidence: ColorConfidence::Measured,
            warnings: Vec::new(),
        },
        best_effort_cmyx_candidate: None,
        direct_candidates: Vec::new(),
    };
    let mixed = MaterialColorRequirement {
        id: "mixed".into(),
        material: Material::Pla,
        role: MaterialRole::Cosmetic,
        source_color: orange,
        source_slots: vec!["2".into()],
        source_profile_ids: vec!["profile-orange".into()],
        cmyx_candidate: CmyxColorCandidate {
            recipe: CmyxRecipe::FullSpectrum {
                mode: FullSpectrumMode::Ratio,
                sequence: vec![Toolhead::T1, Toolhead::T2, Toolhead::T4],
            },
            calibration_sample_id: None,
            process_compatibility: Some(full_spectrum_process()),
            required_t4_spool_id: Some("black".into()),
            predicted_color: Some(orange),
            delta_e00: Some(2.0),
            confidence: ColorConfidence::Calibrated,
            warnings: Vec::new(),
        },
        best_effort_cmyx_candidate: None,
        direct_candidates: Vec::new(),
    };
    let input = PlanningInput {
        scopes: vec![PrintScope {
            id: "scope".into(),
            display_name: "Scope".into(),
            requirements: vec![solid, mixed],
            units: vec![
                unit("mono", vec!["solid".into()], 20.0),
                unit("mixed", vec!["mixed".into()], 20.0),
            ],
            strategy: ScopeStrategy::CmyxFullSpectrum,
            direct_assignments: Vec::new(),
            approved_cmyx_fallbacks: Vec::new(),
            approved_material_substitutions: Vec::new(),
        }],
        inventory,
        current_toolheads: current_cmy(ToolheadSlotState::Loaded("black".into())),
        config: config(),
    };

    let result = plan(&input);
    assert!(result.errors.is_empty(), "{:#?}", result.errors);
    assert_eq!(result.jobs.len(), 2);
    assert_eq!(
        result.batches.len(),
        2,
        "solid and mixed jobs remain in distinct conversion-target batches"
    );
    assert_eq!(
        result
            .jobs
            .iter()
            .map(|job| job.strategy)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([ColorStrategy::CmyxSolid, ColorStrategy::CmyxFullSpectrum,])
    );
    assert!(result.jobs.iter().any(|job| {
        job.strategy == ColorStrategy::CmyxSolid && job.full_spectrum_process.is_none()
    }));
    assert!(result.jobs.iter().any(|job| {
        job.strategy == ColorStrategy::CmyxFullSpectrum
            && job.full_spectrum_process == Some(full_spectrum_process())
    }));
    assert_eq!(result.jobs.iter().filter(|job| job.fast_mono).count(), 1);
}

#[test]
fn multi_toolhead_solid_job_uses_direct_conversion_with_a_prime_tower() {
    let mut input = direct_input(2);
    input.scopes[0].strategy = ScopeStrategy::CmyxFullSpectrum;
    input.scopes[0].requirements[0].cmyx_candidate = CmyxColorCandidate {
        recipe: CmyxRecipe::Solid {
            toolhead: Toolhead::T1,
        },
        calibration_sample_id: None,
        process_compatibility: None,
        required_t4_spool_id: None,
        predicted_color: Some(input.scopes[0].requirements[0].source_color),
        delta_e00: Some(0.0),
        confidence: ColorConfidence::Measured,
        warnings: Vec::new(),
    };
    input.scopes[0].requirements[1].cmyx_candidate = CmyxColorCandidate {
        recipe: CmyxRecipe::Solid {
            toolhead: Toolhead::T2,
        },
        calibration_sample_id: None,
        process_compatibility: None,
        required_t4_spool_id: None,
        predicted_color: Some(input.scopes[0].requirements[1].source_color),
        delta_e00: Some(0.0),
        confidence: ColorConfidence::Measured,
        warnings: Vec::new(),
    };

    let result = plan(&input);

    assert!(result.errors.is_empty(), "{:#?}", result.errors);
    assert_eq!(result.jobs.len(), 1);
    assert_eq!(result.jobs[0].strategy, ColorStrategy::CmyxSolid);
    assert_eq!(result.jobs[0].full_spectrum_process, None);
    assert!(
        result.jobs[0]
            .color_mappings
            .iter()
            .all(|mapping| mapping.strategy == ColorStrategy::CmyxSolid)
    );
    assert_eq!(result.batches[0].strategy, ColorStrategy::CmyxSolid);
    assert!(result.plates[0].prime_tower.is_some());
}

#[test]
fn dedicated_t4_solid_job_counts_a_cmyx_t4_swap() {
    let mut input = direct_input(1);
    input.scopes[0].strategy = ScopeStrategy::CmyxFullSpectrum;
    input.scopes[0].requirements[0].cmyx_candidate = CmyxColorCandidate {
        recipe: CmyxRecipe::DedicatedT4,
        calibration_sample_id: None,
        process_compatibility: None,
        required_t4_spool_id: Some("direct-0".into()),
        predicted_color: Some(input.scopes[0].requirements[0].source_color),
        delta_e00: Some(0.0),
        confidence: ColorConfidence::Measured,
        warnings: Vec::new(),
    };

    let result = plan(&input);

    assert!(result.errors.is_empty(), "{:#?}", result.errors);
    assert_eq!(result.jobs[0].strategy, ColorStrategy::CmyxSolid);
    assert_eq!(result.batches[0].strategy, ColorStrategy::CmyxSolid);
    assert!(result.batches[0].t4_swap_before);
    assert_eq!(result.t4_swap_count, 1);
    assert_eq!(
        result.final_toolheads.slots[Toolhead::T4.index()],
        ToolheadSlotState::Loaded("direct-0".into())
    );
}

#[test]
fn fast_mono_units_are_grouped_by_their_physical_spool() {
    let mut input = direct_input(2);
    input.scopes[0].strategy = ScopeStrategy::CmyxFullSpectrum;
    input.scopes[0].requirements[0].cmyx_candidate = CmyxColorCandidate {
        recipe: CmyxRecipe::Solid {
            toolhead: Toolhead::T1,
        },
        calibration_sample_id: None,
        process_compatibility: None,
        required_t4_spool_id: None,
        predicted_color: Some(input.scopes[0].requirements[0].source_color),
        delta_e00: Some(0.0),
        confidence: ColorConfidence::Measured,
        warnings: Vec::new(),
    };
    input.scopes[0].requirements[1].cmyx_candidate = CmyxColorCandidate {
        recipe: CmyxRecipe::Solid {
            toolhead: Toolhead::T2,
        },
        calibration_sample_id: None,
        process_compatibility: None,
        required_t4_spool_id: None,
        predicted_color: Some(input.scopes[0].requirements[1].source_color),
        delta_e00: Some(0.0),
        confidence: ColorConfidence::Measured,
        warnings: Vec::new(),
    };
    input.scopes[0].units = vec![
        unit("cyan-mono", vec!["color-0".into()], 20.0),
        unit("magenta-mono", vec!["color-1".into()], 20.0),
    ];

    let result = plan(&input);

    assert!(result.errors.is_empty(), "{:#?}", result.errors);
    assert_eq!(result.jobs.len(), 2);
    assert_eq!(result.plates.len(), 2);
    assert!(result.jobs.iter().all(|job| job.fast_mono));
}

#[test]
fn mixed_recipe_is_blocked_when_target_process_does_not_match() {
    let mut incompatible_processes = Vec::new();
    let mut process = full_spectrum_process();
    process.process_fingerprint = "different-temperatures-or-flow".into();
    incompatible_processes.push(process);
    let mut process = full_spectrum_process();
    process.plate_layer_height_microns = 120;
    process.subdivision_factor = 6;
    incompatible_processes.push(process);
    let mut process = full_spectrum_process();
    process.subdivision_policy = FullSpectrumSubdivisionPolicy::Disabled;
    process.subdivision_factor = 1;
    process.effective_sublayer_height_microns = 80;
    incompatible_processes.push(process);
    let mut process = full_spectrum_process();
    process.subdivision_factor = 2;
    process.effective_sublayer_height_microns = 40;
    incompatible_processes.push(process);

    for incompatible in incompatible_processes {
        let mut input = direct_input(1);
        input.scopes[0].strategy = ScopeStrategy::CmyxFullSpectrum;
        input.scopes[0].requirements[0]
            .cmyx_candidate
            .process_compatibility = Some(incompatible);

        let result = plan(&input);
        assert!(result.jobs.is_empty());
        assert!(result.errors.iter().any(|error| {
            error.code == ErrorCode::FullSpectrumProcessMismatch
                && error.scope_id.as_deref() == Some("scope")
        }));
    }
}

#[test]
fn solid_recipe_is_not_blocked_by_full_spectrum_process_mismatch() {
    let mut input = direct_input(1);
    input.scopes[0].strategy = ScopeStrategy::CmyxFullSpectrum;
    let candidate = &mut input.scopes[0].requirements[0].cmyx_candidate;
    candidate.recipe = CmyxRecipe::Solid {
        toolhead: Toolhead::T1,
    };
    let mut irrelevant = full_spectrum_process();
    irrelevant.process_fingerprint = "not-the-target-process".into();
    candidate.process_compatibility = Some(irrelevant);
    candidate.required_t4_spool_id = None;
    input.current_toolheads.slots[Toolhead::T4.index()] =
        ToolheadSlotState::Loaded("direct-0".into());

    let result = plan(&input);
    assert!(result.errors.is_empty(), "{:#?}", result.errors);
    assert_eq!(result.jobs.len(), 1);
    assert!(result.jobs[0].fast_mono);
    assert_eq!(result.jobs[0].strategy, ColorStrategy::CmyxSolid);
    assert_eq!(result.jobs[0].full_spectrum_process, None);
    let PrinterLoadout::U1 { loadout } = &result.jobs[0].loadout else {
        panic!("solid CMY component remains a U1 job");
    };
    assert_eq!(loadout.spool(Toolhead::T4), None);
    assert_eq!(result.t4_swap_count, 0);
    assert_eq!(
        result.final_toolheads.slots[Toolhead::T4.index()],
        ToolheadSlotState::Loaded("direct-0".into())
    );
}

#[test]
fn output_is_deterministic_across_unordered_input_collections() {
    let first = direct_input(4);
    let mut reordered = first.clone();
    reordered.inventory.reverse();
    reordered.scopes.reverse();
    for scope in &mut reordered.scopes {
        scope.requirements.reverse();
        scope.units.reverse();
        scope.direct_assignments.reverse();
        for requirement in &mut scope.requirements {
            requirement.source_slots.reverse();
            requirement.direct_candidates.reverse();
        }
        for printable_unit in &mut scope.units {
            printable_unit.requirement_ids.reverse();
        }
    }

    let first_json = serde_json::to_string_pretty(&plan(&first)).unwrap();
    let reordered_json = serde_json::to_string_pretty(&plan(&reordered)).unwrap();
    assert_eq!(first_json, reordered_json);
}

#[test]
fn planning_domain_is_serde_round_trip_ready() {
    let input = direct_input(2);
    let encoded = serde_json::to_string(&input).unwrap();
    let decoded: PlanningInput = serde_json::from_str(&encoded).unwrap();
    assert_eq!(plan(&input), plan(&decoded));
}

fn best_effort_input(source_material: Material, target_material: Material) -> PlanningInput {
    let mut input = direct_input(1);
    let requirement = &mut input.scopes[0].requirements[0];
    requirement.material = source_material;
    requirement.cmyx_candidate = CmyxColorCandidate {
        recipe: CmyxRecipe::ManualReview {
            reason: "Automatic color threshold exceeded.".into(),
        },
        calibration_sample_id: None,
        process_compatibility: None,
        required_t4_spool_id: None,
        predicted_color: Some(rgb(40, 50, 60)),
        delta_e00: Some(12.5),
        confidence: ColorConfidence::Nominal,
        warnings: vec!["Explicit approval required.".into()],
    };
    requirement.best_effort_cmyx_candidate = Some(BestEffortCmyxCandidate {
        candidate_id: "candidate-v1".into(),
        target_material,
        target_color: requirement.source_color,
        candidate: CmyxColorCandidate {
            recipe: CmyxRecipe::Solid {
                toolhead: Toolhead::T1,
            },
            calibration_sample_id: None,
            process_compatibility: None,
            required_t4_spool_id: None,
            predicted_color: Some(rgb(40, 50, 60)),
            delta_e00: Some(12.5),
            confidence: ColorConfidence::Nominal,
            warnings: vec!["Explicitly approved nominal fallback.".into()],
        },
    });
    input.scopes[0].strategy = ScopeStrategy::CmyxFullSpectrum;
    input
}

#[test]
fn best_effort_color_requires_an_exact_candidate_approval() {
    let mut input = best_effort_input(Material::Pla, Material::Pla);

    let blocked = plan(&input);
    assert!(blocked.jobs.is_empty());
    assert!(
        blocked
            .errors
            .iter()
            .any(|error| error.code == ErrorCode::CmyxRecipeUnavailable)
    );

    input.scopes[0].approved_cmyx_fallbacks = vec![CmyxFallbackApproval {
        requirement_id: "color-0".into(),
        candidate_id: "stale-candidate".into(),
    }];
    let stale = plan(&input);
    assert!(stale.jobs.is_empty());
    assert!(
        stale
            .errors
            .iter()
            .any(|error| error.code == ErrorCode::InvalidCmyxFallbackApproval)
    );

    input.scopes[0].approved_cmyx_fallbacks[0].candidate_id = "candidate-v1".into();
    let approved = plan(&input);
    assert!(approved.errors.is_empty(), "{:#?}", approved.errors);
    assert_eq!(approved.jobs.len(), 1);
    assert!(
        approved
            .warnings
            .iter()
            .any(|warning| { warning.code == WarningCode::ApprovedColorFallback })
    );
}

#[test]
fn material_substitution_requires_a_separate_fingerprinted_acknowledgement() {
    let mut input = best_effort_input(Material::Petg, Material::Pla);
    input.scopes[0].approved_cmyx_fallbacks = vec![CmyxFallbackApproval {
        requirement_id: "color-0".into(),
        candidate_id: "candidate-v1".into(),
    }];

    let color_only = plan(&input);
    assert!(color_only.jobs.is_empty());
    assert!(
        color_only
            .errors
            .iter()
            .any(|error| { error.code == ErrorCode::MaterialSubstitutionApprovalRequired })
    );

    input.scopes[0].approved_material_substitutions = vec![MaterialSubstitutionApproval {
        requirement_id: "color-0".into(),
        candidate_id: "candidate-v1".into(),
        source_material: Material::Petg,
        target_material: Material::Pla,
        acknowledged: true,
    }];
    let approved = plan(&input);
    assert!(approved.errors.is_empty(), "{:#?}", approved.errors);
    assert_eq!(approved.jobs[0].printable_materials, vec![Material::Pla]);
    assert_eq!(
        approved.jobs[0].color_mappings[0].source_material,
        Material::Petg
    );
    assert_eq!(
        approved.jobs[0].color_mappings[0].actual_material,
        Some(Material::Pla)
    );
    assert!(
        approved
            .warnings
            .iter()
            .any(|warning| { warning.code == WarningCode::ApprovedMaterialSubstitution })
    );
}

#[test]
fn effective_alias_can_approve_the_shared_backend_candidate() {
    let mut input = best_effort_input(Material::Pla, Material::Pla);
    let mut alias = input.scopes[0].requirements[0].clone();
    alias.id = "color-alias".into();
    alias.source_slots = vec!["alias-slot".into()];
    input.scopes[0].requirements.push(alias);
    input.scopes[0].units[0]
        .requirement_ids
        .push("color-alias".into());
    input.scopes[0].approved_cmyx_fallbacks = vec![CmyxFallbackApproval {
        requirement_id: "color-alias".into(),
        candidate_id: "candidate-v1".into(),
    }];

    let result = plan(&input);
    assert!(result.errors.is_empty(), "{:#?}", result.errors);
    assert_eq!(result.jobs.len(), 1);
    assert_eq!(result.jobs[0].color_mappings.len(), 1);
    assert_eq!(
        result.jobs[0].color_mappings[0].source_requirement_ids,
        vec!["color-0".to_owned(), "color-alias".to_owned()]
    );
}
