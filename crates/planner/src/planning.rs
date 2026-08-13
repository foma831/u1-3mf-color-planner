use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use crate::{
    BestEffortCmyxCandidate, CmyxColorCandidate, CmyxRecipe, ColorConfidence, ColorStrategy,
    CurrentToolheadState, DirectIneligibility, DirectSpoolCandidate, DirectSpoolEligibility,
    DirectToolheadAssignment, ErrorCode, Estimate, FullSpectrumProcessCompatibility,
    FullSpectrumSubdivisionPolicy, MappingStatus, Material, MaterialColorRequirement, MaterialRole,
    MaterialSubstitutionApproval, PackingStatus, PlanError, PlanWarning, PlannedBatch, PlannedJob,
    PlannedPlacement, PlannedPlate, PlannedPrimeTower, PlannerConfig, PlanningInput,
    PlanningResult, PrintScope, PrintableUnit, Printer, PrinterLoadout, PrinterPreference,
    RgbColor, ScopeStrategy, ScopeStrategyOptions, ScopedUnitRef, SetupAction, SetupActionKind,
    SetupPhase, SourceToActualMapping, Spool, Toolhead, ToolheadSlotState, U1Loadout, WarningCode,
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct EffectiveKey {
    material: Material,
    role: MaterialRole,
    color: RgbColor,
    source_profile: SourceProfileIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct DirectPhysicalKey {
    material: Material,
    color: RgbColor,
    source_profile: DirectSourceProfileIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum DirectSourceProfileIdentity {
    /// A declared source profile is a physical filament identity. Its semantic
    /// role remains separate in `EffectiveKey`, but does not consume another
    /// spool or toolhead.
    Declared(Vec<String>),
    /// Without a declared profile, role remains a conservative discriminator;
    /// we must not infer that two unknown source filaments are interchangeable.
    Unknown {
        role: MaterialRole,
        discriminator: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum SourceProfileIdentity {
    Declared(Vec<String>),
    /// Missing profile IDs remain distinct by source slot (or requirement ID).
    Unknown(Vec<String>),
}

#[derive(Clone, Debug)]
struct EffectiveRequirement {
    key: EffectiveKey,
    ids: Vec<String>,
    source_slots: Vec<String>,
    source_profile_ids: Vec<String>,
    cmyx: CmyxColorCandidate,
    best_effort_cmyx: Option<BestEffortCmyxCandidate>,
    approved_color_fallback: bool,
    approved_material_substitution: Option<MaterialSubstitutionApproval>,
    direct_candidates: Vec<DirectSpoolCandidate>,
}

#[derive(Clone, Debug)]
struct DirectPhysicalRequirement {
    key: DirectPhysicalKey,
    effective_indices: Vec<usize>,
    direct_candidates: Vec<DirectSpoolCandidate>,
}

struct DirectEligibilityContext<'a> {
    inventory: &'a BTreeMap<String, Spool>,
    current: &'a CurrentToolheadState,
    allow_palette_reduction: bool,
    requirement_to_key: &'a BTreeMap<String, EffectiveKey>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct JobKey {
    printer: Printer,
    strategy: ColorStrategy,
    loadout: PrinterLoadout,
    printable_materials: Vec<Material>,
    fast_mono: bool,
    mono_spool_id: Option<String>,
    full_spectrum_process: Option<FullSpectrumProcessCompatibility>,
}

#[derive(Clone, Debug)]
struct UnitPlan {
    unit_ref: ScopedUnitRef,
    scope_id: String,
    source_boundary: SourceBoundary,
    key: JobKey,
    mappings: Vec<SourceToActualMapping>,
    bounds: crate::BoundsMm,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum SourceBoundary {
    SourcePlate(String),
    Scope(String),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PackingGroupKey {
    job: JobKey,
    /// By default U1 project metadata remains plate-scoped. A1 mono packing,
    /// and explicitly opted-in U1 cross-source packing, omit this boundary.
    u1_source_boundary: Option<SourceBoundary>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct DirectPackingFamilyKey {
    printable_materials: Vec<Material>,
    mono_spool_id: Option<String>,
    full_spectrum_process: Option<FullSpectrumProcessCompatibility>,
    u1_source_boundary: Option<SourceBoundary>,
}

#[derive(Debug)]
struct CompatibleDirectPackingGroup {
    loadout: U1Loadout,
    units: Vec<UnitPlan>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct BatchKey {
    printer: Printer,
    strategy: ColorStrategy,
    loadout: PrinterLoadout,
}

#[derive(Clone, Debug)]
struct MatchChoice {
    spool_id: String,
    score: f64,
    delta_e00: Option<f64>,
    confidence: ColorConfidence,
}

#[derive(Clone, Debug)]
struct MatchState {
    score: f64,
    current_misses: usize,
    choices: Vec<Option<MatchChoice>>,
}

#[derive(Clone, Debug)]
struct ReducedPaletteMatch {
    score: f64,
    current_misses: usize,
    spool_ids: Vec<String>,
    choices: Vec<MatchChoice>,
}

/// Build a deterministic, conservative first-pass print plan.
///
/// The function intentionally returns a partial plan plus structured hard
/// errors. Invalid scopes are omitted while independently valid scopes remain
/// available for an explicitly approved partial export.
#[must_use]
pub fn plan(input: &PlanningInput) -> PlanningResult {
    let mut warnings = Vec::new();
    let mut errors = Vec::new();

    validate_build_volumes(&input.config, &mut errors);
    let inventory = normalize_inventory(&input.inventory, &mut errors);
    let scopes = unique_scopes(&input.scopes, &mut errors);

    let mut options = Vec::new();
    let mut unit_plans = Vec::new();

    for scope in scopes {
        plan_scope(
            scope,
            &inventory,
            &input.current_toolheads,
            &input.config,
            &mut options,
            &mut unit_plans,
            &mut warnings,
            &mut errors,
        );
    }
    warn_about_cross_printer_spool_transfers(&unit_plans, &mut warnings);

    let (jobs, plates) =
        group_jobs_and_plates(unit_plans, &input.config, &mut warnings, &mut errors);
    let (batches, t4_swap_count, a1_spool_change_count, final_toolheads) = schedule_batches(
        &jobs,
        &plates,
        &input.current_toolheads,
        &input.config,
        &mut warnings,
    );

    options.sort_by(|left, right| left.scope_id.cmp(&right.scope_id));
    sort_warnings(&mut warnings);
    sort_errors(&mut errors);

    PlanningResult {
        scope_options: options,
        jobs,
        plates,
        batches,
        t4_swap_count,
        a1_spool_change_count,
        final_toolheads,
        warnings,
        errors,
    }
}

fn validate_build_volumes(config: &PlannerConfig, errors: &mut Vec<PlanError>) {
    if !config.u1_build_volume.is_valid() {
        errors.push(PlanError {
            code: ErrorCode::InvalidBuildVolume,
            scope_id: None,
            unit_id: None,
            message: "U1 build volume must contain finite positive dimensions.".into(),
        });
    }
    if !config.a1_mini.build_volume.is_valid() {
        errors.push(PlanError {
            code: ErrorCode::InvalidBuildVolume,
            scope_id: None,
            unit_id: None,
            message: "A1 mini build volume must contain finite positive dimensions.".into(),
        });
    }
}

fn normalize_inventory(
    inventory: &[Spool],
    errors: &mut Vec<PlanError>,
) -> BTreeMap<String, Spool> {
    let mut grouped: BTreeMap<&str, Vec<&Spool>> = BTreeMap::new();
    for spool in inventory {
        grouped.entry(&spool.id).or_default().push(spool);
    }

    let mut normalized = BTreeMap::new();
    for (id, spools) in grouped {
        if spools.len() != 1 {
            errors.push(PlanError {
                code: ErrorCode::DuplicateSpoolId,
                scope_id: None,
                unit_id: None,
                message: format!("Inventory contains duplicate spool ID '{id}'."),
            });
            continue;
        }
        normalized.insert(id.to_owned(), spools[0].clone());
    }
    normalized
}

fn unique_scopes<'a>(scopes: &'a [PrintScope], errors: &mut Vec<PlanError>) -> Vec<&'a PrintScope> {
    let mut grouped: BTreeMap<&str, Vec<&PrintScope>> = BTreeMap::new();
    for scope in scopes {
        grouped.entry(&scope.id).or_default().push(scope);
    }

    let mut unique = Vec::new();
    for (id, matches) in grouped {
        if matches.len() != 1 {
            errors.push(PlanError {
                code: ErrorCode::DuplicateScopeId,
                scope_id: Some(id.to_owned()),
                unit_id: None,
                message: format!("Project contains duplicate print scope ID '{id}'."),
            });
        } else {
            unique.push(matches[0]);
        }
    }
    unique
}

#[allow(clippy::too_many_arguments)]
fn plan_scope(
    scope: &PrintScope,
    inventory: &BTreeMap<String, Spool>,
    current: &CurrentToolheadState,
    config: &PlannerConfig,
    options: &mut Vec<ScopeStrategyOptions>,
    unit_plans: &mut Vec<UnitPlan>,
    warnings: &mut Vec<PlanWarning>,
    errors: &mut Vec<PlanError>,
) {
    let requirements = unique_requirements(scope, errors);
    let units = unique_units(scope, errors);
    let mut valid_units = Vec::new();
    let mut used_requirement_ids = BTreeSet::new();

    for unit in units {
        let mut valid = true;
        if !unit.bounds.is_valid() {
            errors.push(unit_error(
                ErrorCode::InvalidBounds,
                scope,
                unit,
                "Unit bounds and clearances must be finite and non-negative.",
            ));
            valid = false;
        } else if !unit.bounds.has_known_size() {
            warnings.push(unit_warning(
                WarningCode::UnknownBounds,
                scope,
                unit,
                "Unit bounds are unavailable; U1 assignment is provisional and A1 mini routing is disabled.",
            ));
        }

        let mut requirement_ids = unit.requirement_ids.clone();
        requirement_ids.sort();
        requirement_ids.dedup();
        if requirement_ids.is_empty() {
            errors.push(unit_error(
                ErrorCode::UnknownRequirement,
                scope,
                unit,
                "Printable unit has no effective material-color requirement.",
            ));
            valid = false;
        }
        for requirement_id in &requirement_ids {
            if !requirements.contains_key(requirement_id) {
                errors.push(unit_error(
                    ErrorCode::UnknownRequirement,
                    scope,
                    unit,
                    &format!("Unit references unknown requirement '{requirement_id}'."),
                ));
                valid = false;
            }
        }

        if valid {
            used_requirement_ids.extend(requirement_ids);
            valid_units.push(unit);
        }
    }

    let (mut effective, requirement_to_key) =
        build_effective_requirements(scope, &requirements, &used_requirement_ids, errors);
    apply_cmyx_fallback_approvals(scope, &mut effective, &requirement_to_key, errors);
    let (direct_physical, physical_index_by_effective) =
        build_direct_physical_requirements(&effective);
    let direct = direct_eligibility(
        scope,
        &effective,
        &direct_physical,
        &physical_index_by_effective,
        DirectEligibilityContext {
            inventory,
            current,
            allow_palette_reduction: config.allow_direct_palette_reduction,
            requirement_to_key: &requirement_to_key,
        },
    );
    let cmyx_available = effective.iter().all(|requirement| {
        recipe_slots(&requirement.cmyx.recipe).is_some()
            && cmy_candidate_is_structurally_valid(&requirement.cmyx)
            && cmy_candidate_process_compatible(&requirement.cmyx, config)
    });

    options.push(ScopeStrategyOptions {
        scope_id: scope.id.clone(),
        effective_pair_count: effective.len(),
        direct_pair_count: direct_physical.len(),
        cmyx_available,
        direct_spools: direct.clone(),
        selected_strategy: scope.strategy,
    });

    if matches!(direct, DirectSpoolEligibility::Ineligible { .. }) {
        warnings.push(PlanWarning {
            code: WarningCode::DirectAlternativeUnavailable,
            scope_id: Some(scope.id.clone()),
            unit_id: None,
            message: direct_ineligibility_message(&direct),
        });
    }

    let selected = match scope.strategy {
        ScopeStrategy::Auto | ScopeStrategy::CmyxFullSpectrum => ColorStrategy::CmyxFullSpectrum,
        ScopeStrategy::DirectSpools => ColorStrategy::DirectSpools,
    };

    let direct_assignments = match (&selected, &direct) {
        (ColorStrategy::DirectSpools, DirectSpoolEligibility::Eligible { assignments }) => {
            Some(assignments)
        }
        (ColorStrategy::DirectSpools, DirectSpoolEligibility::Ineligible { .. }) => {
            errors.push(PlanError {
                code: ErrorCode::RequestedDirectSpoolsUnavailable,
                scope_id: Some(scope.id.clone()),
                unit_id: None,
                message: direct_ineligibility_message(&direct),
            });
            None
        }
        _ => None,
    };

    if selected == ColorStrategy::DirectSpools && direct_assignments.is_none() {
        return;
    }

    let effective_by_key: BTreeMap<_, _> = effective
        .iter()
        .map(|requirement| (requirement.key.clone(), requirement))
        .collect();

    for unit in valid_units {
        let keys = unit_effective_keys(unit, &requirement_to_key);
        let requirements_for_unit: Vec<_> = keys
            .iter()
            .filter_map(|key| effective_by_key.get(key).copied())
            .collect();

        let candidate = if selected == ColorStrategy::DirectSpools {
            plan_direct_unit(
                scope,
                unit,
                &requirements_for_unit,
                direct_assignments.expect("checked above"),
                inventory,
                config,
                warnings,
                errors,
            )
        } else {
            plan_cmyx_unit(
                scope,
                unit,
                &requirements_for_unit,
                inventory,
                config,
                warnings,
                errors,
            )
        };

        if let Some(candidate) = candidate {
            unit_plans.push(candidate);
        }
    }
}

fn unique_requirements<'a>(
    scope: &'a PrintScope,
    errors: &mut Vec<PlanError>,
) -> BTreeMap<String, &'a MaterialColorRequirement> {
    let mut grouped: BTreeMap<&str, Vec<&MaterialColorRequirement>> = BTreeMap::new();
    for requirement in &scope.requirements {
        grouped
            .entry(&requirement.id)
            .or_default()
            .push(requirement);
    }

    let mut unique = BTreeMap::new();
    for (id, matches) in grouped {
        if matches.len() != 1 {
            errors.push(PlanError {
                code: ErrorCode::DuplicateRequirementId,
                scope_id: Some(scope.id.clone()),
                unit_id: None,
                message: format!("Scope contains duplicate requirement ID '{id}'."),
            });
        } else {
            unique.insert(id.to_owned(), matches[0]);
        }
    }
    unique
}

fn unique_units<'a>(scope: &'a PrintScope, errors: &mut Vec<PlanError>) -> Vec<&'a PrintableUnit> {
    let mut grouped: BTreeMap<&str, Vec<&PrintableUnit>> = BTreeMap::new();
    for unit in &scope.units {
        grouped.entry(&unit.id).or_default().push(unit);
    }

    let mut unique = Vec::new();
    for (id, matches) in grouped {
        if matches.len() != 1 {
            errors.push(PlanError {
                code: ErrorCode::DuplicateUnitId,
                scope_id: Some(scope.id.clone()),
                unit_id: Some(id.to_owned()),
                message: format!("Scope contains duplicate printable unit ID '{id}'."),
            });
        } else {
            unique.push(matches[0]);
        }
    }
    unique
}

fn canonical_strings(values: &[String]) -> Vec<String> {
    let mut values = values.to_vec();
    values.sort();
    values.dedup();
    values
}

fn source_profile_identity(requirement: &MaterialColorRequirement) -> SourceProfileIdentity {
    let profiles = canonical_strings(&requirement.source_profile_ids);
    let mut discriminator = canonical_strings(&requirement.source_slots);
    if discriminator.is_empty() {
        discriminator.push(requirement.id.clone());
    }
    match profiles.as_slice() {
        [] => SourceProfileIdentity::Unknown(discriminator),
        _ if profiles.iter().all(|profile| !profile.trim().is_empty()) => {
            SourceProfileIdentity::Declared(profiles)
        }
        _ => SourceProfileIdentity::Unknown(discriminator),
    }
}

fn direct_physical_key(key: &EffectiveKey) -> DirectPhysicalKey {
    let source_profile = match &key.source_profile {
        SourceProfileIdentity::Declared(profiles) => {
            DirectSourceProfileIdentity::Declared(profiles.clone())
        }
        SourceProfileIdentity::Unknown(discriminator) => DirectSourceProfileIdentity::Unknown {
            role: key.role,
            discriminator: discriminator.clone(),
        },
    };
    DirectPhysicalKey {
        material: key.material.clone(),
        color: key.color,
        source_profile,
    }
}

fn build_direct_physical_requirements(
    effective: &[EffectiveRequirement],
) -> (Vec<DirectPhysicalRequirement>, Vec<usize>) {
    let mut grouped = BTreeMap::<DirectPhysicalKey, Vec<usize>>::new();
    for (index, requirement) in effective.iter().enumerate() {
        grouped
            .entry(direct_physical_key(&requirement.key))
            .or_default()
            .push(index);
    }

    let mut physical = Vec::with_capacity(grouped.len());
    let mut physical_index_by_effective = vec![0; effective.len()];
    for (physical_index, (key, effective_indices)) in grouped.into_iter().enumerate() {
        for effective_index in &effective_indices {
            physical_index_by_effective[*effective_index] = physical_index;
        }
        let direct_candidates = normalize_direct_candidates(
            effective_indices
                .iter()
                .flat_map(|index| effective[*index].direct_candidates.iter()),
        );
        physical.push(DirectPhysicalRequirement {
            key,
            effective_indices,
            direct_candidates,
        });
    }
    (physical, physical_index_by_effective)
}

fn build_effective_requirements(
    scope: &PrintScope,
    requirements: &BTreeMap<String, &MaterialColorRequirement>,
    used_ids: &BTreeSet<String>,
    errors: &mut Vec<PlanError>,
) -> (Vec<EffectiveRequirement>, BTreeMap<String, EffectiveKey>) {
    let mut grouped: BTreeMap<EffectiveKey, Vec<&MaterialColorRequirement>> = BTreeMap::new();
    let mut id_to_key = BTreeMap::new();

    for id in used_ids {
        let Some(requirement) = requirements.get(id).copied() else {
            continue;
        };
        let key = EffectiveKey {
            material: requirement.material.clone(),
            role: requirement.role,
            color: requirement.source_color,
            source_profile: source_profile_identity(requirement),
        };
        id_to_key.insert(id.clone(), key.clone());
        grouped.entry(key).or_default().push(requirement);
    }

    let mut effective = Vec::new();
    for (key, mut aliases) in grouped {
        aliases.sort_by(|left, right| left.id.cmp(&right.id));
        let canonical = aliases[0];
        if aliases
            .iter()
            .skip(1)
            .any(|alias| alias.cmyx_candidate != canonical.cmyx_candidate)
        {
            errors.push(PlanError {
                code: ErrorCode::InvalidCmyxRecipe,
                scope_id: Some(scope.id.clone()),
                unit_id: None,
                message: format!(
                    "Equivalent material-color requirements in scope '{}' have different CMY+X candidates.",
                    scope.id
                ),
            });
        }
        if aliases
            .iter()
            .skip(1)
            .any(|alias| alias.best_effort_cmyx_candidate != canonical.best_effort_cmyx_candidate)
        {
            errors.push(PlanError {
                code: ErrorCode::InvalidCmyxRecipe,
                scope_id: Some(scope.id.clone()),
                unit_id: None,
                message: format!(
                    "Equivalent material-color requirements in scope '{}' have different best-effort CMY+X candidates.",
                    scope.id
                ),
            });
        }

        let mut ids: Vec<_> = aliases.iter().map(|alias| alias.id.clone()).collect();
        ids.sort();
        ids.dedup();
        let mut source_slots: Vec<_> = aliases
            .iter()
            .flat_map(|alias| alias.source_slots.iter().cloned())
            .collect();
        source_slots.sort();
        source_slots.dedup();
        let source_profile_ids = canonical_strings(
            &aliases
                .iter()
                .flat_map(|alias| alias.source_profile_ids.iter().cloned())
                .collect::<Vec<_>>(),
        );
        let direct_candidates = normalize_direct_candidates(
            aliases
                .iter()
                .flat_map(|alias| alias.direct_candidates.iter()),
        );
        let mut cmyx = canonical.cmyx_candidate.clone();
        cmyx.warnings.sort();
        cmyx.warnings.dedup();

        effective.push(EffectiveRequirement {
            key,
            ids,
            source_slots,
            source_profile_ids,
            cmyx,
            best_effort_cmyx: canonical.best_effort_cmyx_candidate.clone(),
            approved_color_fallback: false,
            approved_material_substitution: None,
            direct_candidates,
        });
    }
    (effective, id_to_key)
}

fn apply_cmyx_fallback_approvals(
    scope: &PrintScope,
    effective: &mut [EffectiveRequirement],
    requirement_to_key: &BTreeMap<String, EffectiveKey>,
    errors: &mut Vec<PlanError>,
) {
    let index_by_key = effective
        .iter()
        .enumerate()
        .map(|(index, requirement)| (requirement.key.clone(), index))
        .collect::<BTreeMap<_, _>>();

    let mut color_approvals = BTreeMap::<EffectiveKey, String>::new();
    for approval in &scope.approved_cmyx_fallbacks {
        let Some(key) = requirement_to_key.get(&approval.requirement_id) else {
            errors.push(invalid_fallback_approval(
                scope,
                &approval.requirement_id,
                "Color fallback approval references an unused or unknown requirement.",
            ));
            continue;
        };
        if let Some(previous) = color_approvals.insert(key.clone(), approval.candidate_id.clone())
            && previous != approval.candidate_id
        {
            errors.push(invalid_fallback_approval(
                scope,
                &approval.requirement_id,
                "Equivalent source requirements approve different color candidates.",
            ));
        }
    }

    for (key, candidate_id) in color_approvals {
        let index = index_by_key[&key];
        let requirement = &mut effective[index];
        let Some(fallback) = requirement.best_effort_cmyx.as_ref() else {
            errors.push(invalid_fallback_approval(
                scope,
                &requirement.ids[0],
                "Color fallback approval does not match a pending backend candidate.",
            ));
            continue;
        };
        if fallback.candidate_id != candidate_id {
            errors.push(invalid_fallback_approval(
                scope,
                &requirement.ids[0],
                "Color fallback approval is stale because the backend candidate changed.",
            ));
            continue;
        }
        requirement.cmyx = fallback.candidate.clone();
        requirement.approved_color_fallback = true;
    }

    let mut material_approvals = BTreeMap::<EffectiveKey, MaterialSubstitutionApproval>::new();
    for approval in &scope.approved_material_substitutions {
        let Some(key) = requirement_to_key.get(&approval.requirement_id) else {
            errors.push(invalid_fallback_approval(
                scope,
                &approval.requirement_id,
                "Material substitution approval references an unused or unknown requirement.",
            ));
            continue;
        };
        if let Some(previous) = material_approvals.insert(key.clone(), approval.clone())
            && !same_material_substitution_decision(&previous, approval)
        {
            errors.push(invalid_fallback_approval(
                scope,
                &approval.requirement_id,
                "Equivalent source requirements contain different material substitution approvals.",
            ));
        }
    }

    for (key, approval) in material_approvals {
        let index = index_by_key[&key];
        let requirement = &mut effective[index];
        let Some(fallback) = requirement.best_effort_cmyx.as_ref() else {
            errors.push(invalid_fallback_approval(
                scope,
                &requirement.ids[0],
                "Material substitution approval does not match a pending backend candidate.",
            ));
            continue;
        };
        if !approval.acknowledged
            || approval.candidate_id != fallback.candidate_id
            || approval.source_material != requirement.key.material
            || approval.target_material != fallback.target_material
            || approval.source_material == approval.target_material
            || !requirement.approved_color_fallback
        {
            errors.push(invalid_fallback_approval(
                scope,
                &requirement.ids[0],
                "Material substitution approval is stale, incomplete, or does not match the backend candidate.",
            ));
            continue;
        }
        requirement.approved_material_substitution = Some(approval);
    }
}

fn same_material_substitution_decision(
    left: &MaterialSubstitutionApproval,
    right: &MaterialSubstitutionApproval,
) -> bool {
    left.candidate_id == right.candidate_id
        && left.source_material == right.source_material
        && left.target_material == right.target_material
        && left.acknowledged == right.acknowledged
}

fn invalid_fallback_approval(scope: &PrintScope, requirement_id: &str, detail: &str) -> PlanError {
    PlanError {
        code: ErrorCode::InvalidCmyxFallbackApproval,
        scope_id: Some(scope.id.clone()),
        unit_id: None,
        message: format!("Requirement '{requirement_id}': {detail}"),
    }
}

fn normalize_direct_candidates<'a>(
    candidates: impl Iterator<Item = &'a DirectSpoolCandidate>,
) -> Vec<DirectSpoolCandidate> {
    let mut by_spool: BTreeMap<String, Vec<&DirectSpoolCandidate>> = BTreeMap::new();
    for candidate in candidates {
        by_spool
            .entry(candidate.spool_id.clone())
            .or_default()
            .push(candidate);
    }

    by_spool
        .into_values()
        .map(|candidates| {
            candidates
                .into_iter()
                .min_by(|left, right| compare_direct_candidate(left, right))
                .expect("group is non-empty")
                .clone()
        })
        .collect()
}

fn compare_direct_candidate(left: &DirectSpoolCandidate, right: &DirectSpoolCandidate) -> Ordering {
    option_f64_key(left.delta_e00)
        .cmp(&option_f64_key(right.delta_e00))
        .then_with(|| left.confidence.cmp(&right.confidence))
        .then_with(|| left.spool_id.cmp(&right.spool_id))
}

fn option_f64_key(value: Option<f64>) -> (bool, u64) {
    match value.filter(|value| value.is_finite() && *value >= 0.0) {
        Some(value) => (false, value.to_bits()),
        None => (true, 0),
    }
}

fn unit_effective_keys(
    unit: &PrintableUnit,
    requirement_to_key: &BTreeMap<String, EffectiveKey>,
) -> Vec<EffectiveKey> {
    let mut keys: Vec<_> = unit
        .requirement_ids
        .iter()
        .filter_map(|id| requirement_to_key.get(id).cloned())
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

fn direct_eligibility(
    scope: &PrintScope,
    effective: &[EffectiveRequirement],
    physical: &[DirectPhysicalRequirement],
    physical_index_by_effective: &[usize],
    context: DirectEligibilityContext<'_>,
) -> DirectSpoolEligibility {
    if physical.is_empty() {
        return DirectSpoolEligibility::Ineligible {
            reason: DirectIneligibility::NoEffectivePairs,
        };
    }
    if physical.len() > 4 && !context.allow_palette_reduction {
        return DirectSpoolEligibility::Ineligible {
            reason: DirectIneligibility::TooManyEffectivePairs {
                count: physical.len(),
                maximum: 4,
            },
        };
    }

    let index_by_key: BTreeMap<_, _> = effective
        .iter()
        .enumerate()
        .map(|(index, requirement)| (requirement.key.clone(), index))
        .collect();
    let mut forced_spools: Vec<Option<String>> = vec![None; physical.len()];
    let mut forced_toolheads: Vec<Option<Toolhead>> = vec![None; physical.len()];
    let mut forced_material_substitutions: Vec<Option<bool>> = vec![None; effective.len()];
    let mut requests = scope.direct_assignments.clone();
    requests.sort_by(|left, right| {
        left.requirement_id
            .cmp(&right.requirement_id)
            .then_with(|| left.spool_id.cmp(&right.spool_id))
            .then_with(|| left.toolhead.cmp(&right.toolhead))
    });

    for request in requests {
        let Some(key) = context.requirement_to_key.get(&request.requirement_id) else {
            return invalid_direct(format!(
                "Direct assignment references unused or unknown requirement '{}'.",
                request.requirement_id
            ));
        };
        let effective_index = index_by_key[key];
        let physical_index = physical_index_by_effective[effective_index];
        if let Some(existing) = &forced_spools[physical_index]
            && existing != &request.spool_id
        {
            return invalid_direct(format!(
                "Source requirements that share one physical Direct identity select both '{}' and '{}'.",
                existing, request.spool_id
            ));
        }
        if let (Some(existing), Some(requested)) =
            (forced_toolheads[physical_index], request.toolhead)
            && existing != requested
        {
            return invalid_direct(
                "Source requirements that share one physical Direct identity select different toolheads."
                    .into(),
            );
        }
        if let Some(existing) = forced_material_substitutions[effective_index]
            && existing != request.allow_material_substitution
        {
            return invalid_direct(
                "Equivalent source requirements contain different material-substitution decisions."
                    .into(),
            );
        }
        forced_spools[physical_index] = Some(request.spool_id);
        forced_material_substitutions[effective_index] = Some(request.allow_material_substitution);
        if request.toolhead.is_some() {
            forced_toolheads[physical_index] = request.toolhead;
        }
    }

    let forced_spool_ids = forced_spools
        .iter()
        .filter_map(Option::as_deref)
        .collect::<BTreeSet<_>>();
    if forced_spool_ids.len() > Toolhead::ALL.len() {
        return invalid_direct(format!(
            "Direct assignments select {} physical spools; the U1 can load at most four per plate.",
            forced_spool_ids.len()
        ));
    }

    let mut forced_spool_toolheads = BTreeMap::new();
    let mut forced_toolhead_spools = BTreeMap::new();
    for (physical_index, forced) in forced_spools.iter().enumerate() {
        if let Some(spool_id) = forced {
            let Some(spool) = context.inventory.get(spool_id) else {
                return invalid_direct(format!("Selected spool '{spool_id}' is not in inventory."));
            };
            if !spool.available {
                return invalid_direct(format!("Selected spool '{spool_id}' is unavailable."));
            }
            if spool.material != physical[physical_index].key.material {
                for effective_index in &physical[physical_index].effective_indices {
                    if forced_material_substitutions[*effective_index] != Some(true) {
                        return invalid_direct(format!(
                            "Requirement '{}' maps physical Direct identity to spool '{spool_id}', changing the source material from {:?} to {:?}; separate explicit mechanical-risk acknowledgement is required for this semantic source requirement.",
                            effective[*effective_index].ids[0],
                            effective[*effective_index].key.material,
                            spool.material
                        ));
                    }
                }
            }
        }
        if let (Some(spool_id), Some(toolhead)) = (
            &forced_spools[physical_index],
            forced_toolheads[physical_index],
        ) {
            if let Some(existing) = forced_spool_toolheads.insert(spool_id.clone(), toolhead)
                && existing != toolhead
            {
                return invalid_direct(format!(
                    "Spool '{spool_id}' is assigned to both {existing:?} and {toolhead:?}. Source colors merged into one spool must share one toolhead."
                ));
            }
            if let Some(existing) = forced_toolhead_spools.insert(toolhead, spool_id.clone())
                && existing != *spool_id
            {
                return invalid_direct(format!(
                    "Toolhead {toolhead:?} is assigned to both '{existing}' and '{spool_id}'."
                ));
            }
        }
    }

    let available: Vec<_> = context
        .inventory
        .values()
        .filter(|spool| spool.available)
        .collect();
    for (index, requirement) in physical.iter().enumerate() {
        if forced_spools[index].is_none()
            && !available
                .iter()
                .any(|spool| spool.material == requirement.key.material)
        {
            return DirectSpoolEligibility::Ineligible {
                reason: DirectIneligibility::MissingCompatibleSpools {
                    material: requirement.key.material.clone(),
                },
            };
        }
    }

    let palette_reduction = physical.len() > Toolhead::ALL.len();
    let selected = if palette_reduction {
        match_reduced_spools(physical, &available, &forced_spools, context.current)
    } else {
        match_spools(physical, &available, &forced_spools, context.current)
    };
    let Some(selected) = selected else {
        if palette_reduction {
            return invalid_direct(
                "Custom Direct palette could not cover every source material with at most four in-stock physical spools. Choose explicit replacements or add compatible spools."
                    .into(),
            );
        }
        let material = physical
            .iter()
            .find_map(|requirement| {
                let required = physical
                    .iter()
                    .filter(|candidate| candidate.key.material == requirement.key.material)
                    .count();
                let available_count = available
                    .iter()
                    .filter(|spool| spool.material == requirement.key.material)
                    .count();
                (available_count < required).then(|| requirement.key.material.clone())
            })
            .unwrap_or_else(|| physical[0].key.material.clone());
        return DirectSpoolEligibility::Ineligible {
            reason: DirectIneligibility::MissingCompatibleSpools { material },
        };
    };

    let toolheads = assign_toolheads(&selected, &forced_toolheads, context.current);
    let assignments = effective
        .iter()
        .enumerate()
        .map(|(effective_index, requirement)| {
            let physical_index = physical_index_by_effective[effective_index];
            let choice = &selected[physical_index];
            let toolhead = toolheads[physical_index];
            let spool = context
                .inventory
                .get(&choice.spool_id)
                .expect("matched inventory spool exists");
            DirectToolheadAssignment {
                source_requirement_ids: requirement.ids.clone(),
                source_profile_ids: requirement.source_profile_ids.clone(),
                source_material: requirement.key.material.clone(),
                source_color: requirement.key.color,
                toolhead,
                spool_id: spool.id.clone(),
                actual_color: spool.actual_color(),
                delta_e00: choice.delta_e00,
                confidence: choice.confidence,
                status: mapping_status(
                    &requirement.key.material,
                    requirement.key.color,
                    spool,
                    choice.delta_e00,
                ),
            }
        })
        .collect();

    DirectSpoolEligibility::Eligible { assignments }
}

fn invalid_direct(message: String) -> DirectSpoolEligibility {
    DirectSpoolEligibility::Ineligible {
        reason: DirectIneligibility::InvalidManualAssignment { message },
    }
}

fn match_reduced_spools(
    physical: &[DirectPhysicalRequirement],
    available: &[&Spool],
    forced_spools: &[Option<String>],
    current: &CurrentToolheadState,
) -> Option<Vec<MatchChoice>> {
    const MAXIMUM_SPOOLS: usize = 4;
    const MAXIMUM_AUTO_CANDIDATES: usize = 20;
    const NEAREST_PER_REQUIREMENT: usize = 4;

    let forced_ids = forced_spools
        .iter()
        .filter_map(Clone::clone)
        .collect::<BTreeSet<_>>();
    if forced_ids.len() > MAXIMUM_SPOOLS {
        return None;
    }

    let mut required_materials = physical
        .iter()
        .enumerate()
        .filter(|(index, _)| forced_spools[*index].is_none())
        .map(|(_, requirement)| requirement.key.material.clone())
        .collect::<BTreeSet<_>>();
    for forced_id in &forced_ids {
        if let Some(spool) = available.iter().find(|spool| spool.id == *forced_id) {
            required_materials.remove(&spool.material);
        }
    }

    let available_by_id = available
        .iter()
        .map(|spool| (spool.id.as_str(), *spool))
        .collect::<BTreeMap<_, _>>();
    let mut candidate_ids = forced_ids.clone();

    // Preserve at least one compatible candidate for every unforced material
    // before applying the bounded optimization candidate set.
    for material in &required_materials {
        let best = available
            .iter()
            .filter(|spool| spool.material == *material)
            .min_by(|left, right| {
                aggregate_spool_score(physical, forced_spools, left)
                    .total_cmp(&aggregate_spool_score(physical, forced_spools, right))
                    .then_with(|| left.id.cmp(&right.id))
            })?;
        candidate_ids.insert(best.id.clone());
    }

    let mut suggestions = BTreeSet::new();
    for (index, requirement) in physical.iter().enumerate() {
        if forced_spools[index].is_some() {
            continue;
        }
        let mut compatible = available
            .iter()
            .filter(|spool| spool.material == requirement.key.material)
            .map(|spool| match_choice(requirement, spool))
            .collect::<Vec<_>>();
        compatible.sort_by(compare_match_choice);
        suggestions.extend(
            compatible
                .into_iter()
                .take(NEAREST_PER_REQUIREMENT)
                .map(|choice| choice.spool_id),
        );
    }
    for material in physical
        .iter()
        .map(|requirement| requirement.key.material.clone())
        .collect::<BTreeSet<_>>()
    {
        let mut aggregate = available
            .iter()
            .filter(|spool| spool.material == material)
            .map(|spool| {
                (
                    aggregate_spool_score(physical, forced_spools, spool),
                    spool.id.clone(),
                )
            })
            .collect::<Vec<_>>();
        aggregate.sort_by(|left, right| {
            left.0
                .total_cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
        });
        suggestions.extend(
            aggregate
                .into_iter()
                .take(MAXIMUM_SPOOLS)
                .map(|(_, spool_id)| spool_id),
        );
    }

    let mut ranked_suggestions = suggestions.into_iter().collect::<Vec<_>>();
    ranked_suggestions.sort_by(|left, right| {
        let left_spool = available_by_id[left.as_str()];
        let right_spool = available_by_id[right.as_str()];
        aggregate_spool_score(physical, forced_spools, left_spool)
            .total_cmp(&aggregate_spool_score(physical, forced_spools, right_spool))
            .then_with(|| left.cmp(right))
    });
    for spool_id in ranked_suggestions {
        if candidate_ids.len() >= MAXIMUM_AUTO_CANDIDATES {
            break;
        }
        candidate_ids.insert(spool_id);
    }

    let optional_ids = candidate_ids
        .difference(&forced_ids)
        .cloned()
        .collect::<Vec<_>>();
    let mut selected_ids = forced_ids.iter().cloned().collect::<Vec<_>>();
    let mut best = evaluate_reduced_palette(
        physical,
        forced_spools,
        current,
        &available_by_id,
        &selected_ids,
    );
    search_reduced_palettes(
        physical,
        forced_spools,
        current,
        &available_by_id,
        &optional_ids,
        0,
        &mut selected_ids,
        &mut best,
    );
    best.map(|candidate| candidate.choices)
}

fn aggregate_spool_score(
    physical: &[DirectPhysicalRequirement],
    forced_spools: &[Option<String>],
    spool: &Spool,
) -> f64 {
    physical
        .iter()
        .enumerate()
        .filter(|(index, requirement)| {
            forced_spools[*index].is_none() && requirement.key.material == spool.material
        })
        .map(|(_, requirement)| match_choice(requirement, spool).score)
        .sum()
}

#[allow(clippy::too_many_arguments)]
fn search_reduced_palettes(
    physical: &[DirectPhysicalRequirement],
    forced_spools: &[Option<String>],
    current: &CurrentToolheadState,
    available_by_id: &BTreeMap<&str, &Spool>,
    optional_ids: &[String],
    start: usize,
    selected_ids: &mut Vec<String>,
    best: &mut Option<ReducedPaletteMatch>,
) {
    const MAXIMUM_SPOOLS: usize = 4;
    if selected_ids.len() >= MAXIMUM_SPOOLS {
        return;
    }

    for index in start..optional_ids.len() {
        selected_ids.push(optional_ids[index].clone());
        if let Some(candidate) = evaluate_reduced_palette(
            physical,
            forced_spools,
            current,
            available_by_id,
            selected_ids,
        ) && reduced_palette_is_better(&candidate, best.as_ref())
        {
            *best = Some(candidate);
        }
        search_reduced_palettes(
            physical,
            forced_spools,
            current,
            available_by_id,
            optional_ids,
            index + 1,
            selected_ids,
            best,
        );
        selected_ids.pop();
    }
}

fn evaluate_reduced_palette(
    physical: &[DirectPhysicalRequirement],
    forced_spools: &[Option<String>],
    current: &CurrentToolheadState,
    available_by_id: &BTreeMap<&str, &Spool>,
    selected_ids: &[String],
) -> Option<ReducedPaletteMatch> {
    if selected_ids.is_empty() {
        return None;
    }
    let mut choices = Vec::with_capacity(physical.len());
    for (index, requirement) in physical.iter().enumerate() {
        let choice = if let Some(forced_id) = &forced_spools[index] {
            match_choice(requirement, available_by_id[forced_id.as_str()])
        } else {
            selected_ids
                .iter()
                .filter_map(|spool_id| available_by_id.get(spool_id.as_str()).copied())
                .filter(|spool| spool.material == requirement.key.material)
                .map(|spool| match_choice(requirement, spool))
                .min_by(compare_match_choice)?
        };
        choices.push(choice);
    }

    let spool_ids = choices
        .iter()
        .map(|choice| choice.spool_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let current_misses = spool_ids
        .iter()
        .filter(|spool_id| {
            !current
                .slots
                .iter()
                .any(|slot| matches!(slot, ToolheadSlotState::Loaded(id) if id == *spool_id))
        })
        .count();
    Some(ReducedPaletteMatch {
        score: choices.iter().map(|choice| choice.score).sum(),
        current_misses,
        spool_ids,
        choices,
    })
}

fn compare_match_choice(left: &MatchChoice, right: &MatchChoice) -> Ordering {
    left.score
        .total_cmp(&right.score)
        .then_with(|| left.confidence.cmp(&right.confidence))
        .then_with(|| left.spool_id.cmp(&right.spool_id))
}

fn reduced_palette_is_better(
    candidate: &ReducedPaletteMatch,
    incumbent: Option<&ReducedPaletteMatch>,
) -> bool {
    let Some(incumbent) = incumbent else {
        return true;
    };
    candidate
        .score
        .total_cmp(&incumbent.score)
        .then_with(|| candidate.current_misses.cmp(&incumbent.current_misses))
        .then_with(|| candidate.spool_ids.len().cmp(&incumbent.spool_ids.len()))
        .then_with(|| candidate.spool_ids.cmp(&incumbent.spool_ids))
        == Ordering::Less
}

fn match_spools(
    physical: &[DirectPhysicalRequirement],
    available: &[&Spool],
    forced_spools: &[Option<String>],
    current: &CurrentToolheadState,
) -> Option<Vec<MatchChoice>> {
    let full_mask = (1_usize << physical.len()) - 1;
    let mut states: Vec<Option<MatchState>> = vec![None; 1_usize << physical.len()];
    let mut initial_mask = 0_usize;
    let mut initial_missed_spools = BTreeSet::new();
    let mut initial = MatchState {
        score: 0.0,
        current_misses: 0,
        choices: vec![None; physical.len()],
    };
    for (index, forced_spool_id) in forced_spools.iter().enumerate() {
        let Some(forced_spool_id) = forced_spool_id else {
            continue;
        };
        let spool = available
            .iter()
            .find(|spool| spool.id == *forced_spool_id)
            .expect("forced available spool was validated before matching");
        let choice = match_choice(&physical[index], spool);
        initial.score += choice.score;
        if !current
            .slots
            .iter()
            .any(|slot| matches!(slot, ToolheadSlotState::Loaded(id) if id == forced_spool_id))
            && initial_missed_spools.insert(forced_spool_id)
        {
            initial.current_misses += 1;
        }
        initial.choices[index] = Some(choice);
        initial_mask |= 1 << index;
    }
    states[initial_mask] = Some(initial);

    for spool in available {
        let previous = states.clone();
        for (mask, state) in previous.into_iter().enumerate() {
            let Some(state) = state else { continue };
            for index in 0..physical.len() {
                if mask & (1 << index) != 0 {
                    continue;
                }
                if let Some(forced) = &forced_spools[index]
                    && forced != &spool.id
                {
                    continue;
                }
                if forced_spools[index].is_none() && spool.material != physical[index].key.material
                {
                    continue;
                }
                if forced_spools
                    .iter()
                    .enumerate()
                    .any(|(other, forced)| other != index && forced.as_ref() == Some(&spool.id))
                {
                    continue;
                }

                let choice = match_choice(&physical[index], spool);
                let mut candidate = state.clone();
                candidate.score += choice.score;
                if !current
                    .slots
                    .iter()
                    .any(|slot| matches!(slot, ToolheadSlotState::Loaded(id) if id == &spool.id))
                {
                    candidate.current_misses += 1;
                }
                candidate.choices[index] = Some(choice);
                let next_mask = mask | (1 << index);
                if is_better_match(&candidate, states[next_mask].as_ref()) {
                    states[next_mask] = Some(candidate);
                }
            }
        }
    }

    states[full_mask].take().map(|state| {
        state
            .choices
            .into_iter()
            .map(|choice| choice.expect("full assignment fills every choice"))
            .collect()
    })
}

fn match_choice(requirement: &DirectPhysicalRequirement, spool: &Spool) -> MatchChoice {
    let explicit = requirement
        .direct_candidates
        .iter()
        .find(|candidate| candidate.spool_id == spool.id);
    let delta_e00 = explicit.and_then(|candidate| {
        candidate
            .delta_e00
            .filter(|value| value.is_finite() && *value >= 0.0)
    });
    let confidence = explicit.map_or_else(
        || {
            if spool.measured_color.is_some() {
                ColorConfidence::Measured
            } else {
                ColorConfidence::Nominal
            }
        },
        |candidate| candidate.confidence,
    );
    MatchChoice {
        spool_id: spool.id.clone(),
        score: delta_e00
            .unwrap_or_else(|| rgb_distance(requirement.key.color, spool.actual_color())),
        delta_e00,
        confidence,
    }
}

fn rgb_distance(left: RgbColor, right: RgbColor) -> f64 {
    let red = f64::from(left.red) - f64::from(right.red);
    let green = f64::from(left.green) - f64::from(right.green);
    let blue = f64::from(left.blue) - f64::from(right.blue);
    (red.mul_add(red, green.mul_add(green, blue * blue))).sqrt()
}

fn is_better_match(candidate: &MatchState, incumbent: Option<&MatchState>) -> bool {
    let Some(incumbent) = incumbent else {
        return true;
    };
    candidate
        .score
        .total_cmp(&incumbent.score)
        .then_with(|| candidate.current_misses.cmp(&incumbent.current_misses))
        .then_with(|| match_choice_ids(candidate).cmp(&match_choice_ids(incumbent)))
        == Ordering::Less
}

fn match_choice_ids(state: &MatchState) -> Vec<Option<&str>> {
    state
        .choices
        .iter()
        .map(|choice| choice.as_ref().map(|choice| choice.spool_id.as_str()))
        .collect()
}

fn assign_toolheads(
    choices: &[MatchChoice],
    forced: &[Option<Toolhead>],
    current: &CurrentToolheadState,
) -> Vec<Toolhead> {
    let mut assigned = vec![None; choices.len()];
    let mut toolhead_by_spool = BTreeMap::new();
    for (choice, forced_toolhead) in choices.iter().zip(forced) {
        if let Some(toolhead) = forced_toolhead {
            toolhead_by_spool.insert(choice.spool_id.clone(), *toolhead);
        }
    }
    let mut used: BTreeSet<_> = toolhead_by_spool.values().copied().collect();

    for (index, choice) in choices.iter().enumerate() {
        if let Some(toolhead) = toolhead_by_spool.get(&choice.spool_id).copied() {
            assigned[index] = Some(toolhead);
            continue;
        }
        let current_slot = Toolhead::ALL.into_iter().find(|toolhead| {
            !used.contains(toolhead)
                && matches!(
                    &current.slots[toolhead.index()],
                    ToolheadSlotState::Loaded(spool_id) if spool_id == &choice.spool_id
                )
        });
        let toolhead = current_slot.unwrap_or_else(|| {
            Toolhead::ALL
                .into_iter()
                .find(|toolhead| !used.contains(toolhead))
                .expect("at most four Direct Spool assignments")
        });
        assigned[index] = Some(toolhead);
        toolhead_by_spool.insert(choice.spool_id.clone(), toolhead);
        used.insert(toolhead);
    }

    assigned
        .into_iter()
        .map(|toolhead| toolhead.expect("every assignment receives a toolhead"))
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn plan_direct_unit(
    scope: &PrintScope,
    unit: &PrintableUnit,
    requirements: &[&EffectiveRequirement],
    assignments: &[DirectToolheadAssignment],
    inventory: &BTreeMap<String, Spool>,
    config: &PlannerConfig,
    warnings: &mut Vec<PlanWarning>,
    errors: &mut Vec<PlanError>,
) -> Option<UnitPlan> {
    let assignment_by_id: BTreeMap<_, _> = assignments
        .iter()
        .flat_map(|assignment| {
            assignment
                .source_requirement_ids
                .iter()
                .map(move |id| (id.as_str(), assignment))
        })
        .collect();
    let mut loadout = [None, None, None, None];
    for assignment in assignments {
        loadout[assignment.toolhead.index()] = Some(assignment.spool_id.clone());
    }

    let mut materials = BTreeSet::new();
    let mut used_spool_ids = BTreeSet::new();
    let mut mappings = Vec::new();
    for requirement in requirements {
        let assignment = assignment_by_id
            .get(requirement.ids[0].as_str())
            .expect("eligible Direct Spool mapping covers every requirement");
        let spool = inventory
            .get(&assignment.spool_id)
            .expect("eligible Direct Spool mapping uses inventory");
        materials.insert(spool.material.clone());
        used_spool_ids.insert(spool.id.clone());
        if assignment.delta_e00.is_none() {
            warnings.push(unit_warning(
                WarningCode::DirectColorDistanceUnavailable,
                scope,
                unit,
                &format!(
                    "Direct spool '{}' was selected by nominal RGB distance; ΔE00 requires the color engine.",
                    spool.id
                ),
            ));
        }
        if spool.measured_color.is_none() {
            warnings.push(unit_warning(
                WarningCode::NominalSpoolColor,
                scope,
                unit,
                &format!("Spool '{}' uses its nominal color.", spool.id),
            ));
        }
        if spool.material != requirement.key.material {
            warnings.push(unit_warning(
                WarningCode::ApprovedMaterialSubstitution,
                scope,
                unit,
                &format!(
                    "Requirement '{}' explicitly substitutes {:?} with {:?} using Direct Spools.",
                    requirement.ids[0], requirement.key.material, spool.material
                ),
            ));
        }
        mappings.push(SourceToActualMapping {
            scope_id: scope.id.clone(),
            source_requirement_ids: requirement.ids.clone(),
            source_slots: requirement.source_slots.clone(),
            source_profile_ids: requirement.source_profile_ids.clone(),
            source_material: requirement.key.material.clone(),
            source_color: requirement.key.color,
            strategy: ColorStrategy::DirectSpools,
            cmyx_comparison: requirement.cmyx.clone(),
            direct_toolhead: Some(assignment.toolhead),
            actual_spool_id: Some(spool.id.clone()),
            actual_material: Some(spool.material.clone()),
            actual_color: Some(spool.actual_color()),
            delta_e00: assignment.delta_e00,
            confidence: assignment.confidence,
            status: assignment.status,
        });
    }

    let mut physical_sources_by_spool: BTreeMap<&str, BTreeSet<DirectPhysicalKey>> =
        BTreeMap::new();
    for requirement in requirements {
        let assignment = assignment_by_id
            .get(requirement.ids[0].as_str())
            .expect("eligible Direct Spool mapping covers every requirement");
        physical_sources_by_spool
            .entry(assignment.spool_id.as_str())
            .or_default()
            .insert(direct_physical_key(&requirement.key));
    }
    for (spool_id, physical_sources) in physical_sources_by_spool {
        let merged_count = physical_sources.len();
        if merged_count > 1 {
            warnings.push(unit_warning(
                WarningCode::IntentionalColorMerge,
                scope,
                unit,
                &format!(
                    "{merged_count} source colors intentionally use physical spool '{spool_id}'. Their visible differences are reduced to one printed color."
                ),
            ));
        }
    }

    let materials: Vec<_> = materials.into_iter().collect();
    if materials.len() > 1 && !config.allow_mixed_materials_on_plate {
        errors.push(unit_error(
            ErrorCode::MixedPrintableMaterials,
            scope,
            unit,
            "Unit would print multiple material families on one plate; the default policy forbids this.",
        ));
        return None;
    }
    mappings.sort_by(mapping_order);

    let mono_spool = (used_spool_ids.len() == 1)
        .then(|| used_spool_ids.iter().next().cloned())
        .flatten();
    let fast_mono = mono_spool.is_some();
    let mono_spool_id = mono_spool.clone();
    let a1_spool_available = mono_spool
        .as_ref()
        .is_some_and(|spool_id| !config.a1_mini.reserved_spool_ids.contains(spool_id));
    let a1_requested = unit.printer_preference == PrinterPreference::A1Mini;
    let a1_considered = config.a1_mini.enabled
        && config.a1_mini.route_fast_mono
        && unit.printer_preference != PrinterPreference::U1;
    let a1_fit = unit.bounds.has_known_size()
        && config
            .a1_mini
            .build_volume
            .contains(unit.bounds.required_volume());
    let a1_material_supported =
        materials.len() == 1 && config.a1_mini.supported_materials.contains(&materials[0]);
    let printer =
        if a1_considered && fast_mono && a1_spool_available && a1_fit && a1_material_supported {
            Printer::A1Mini
        } else {
            if a1_considered || a1_requested {
                if !fast_mono {
                    warnings.push(unit_warning(
                        WarningCode::A1NotSinglePhysicalSpool,
                        scope,
                        unit,
                        "A1 mini accepts only single-physical-spool units in this planner.",
                    ));
                }
                if fast_mono && !a1_spool_available {
                    warnings.push(unit_warning(
                        WarningCode::A1SpoolReservedForU1,
                        scope,
                        unit,
                        "The physical spool is explicitly marked unavailable to the A1 mini.",
                    ));
                }
                if unit.bounds.has_known_size() && !a1_fit {
                    warnings.push(unit_warning(
                        WarningCode::A1OutOfBounds,
                        scope,
                        unit,
                        "Unit including clearance exceeds the 180 × 180 × 180 mm A1 mini volume.",
                    ));
                }
                if !a1_material_supported {
                    warnings.push(unit_warning(
                        WarningCode::A1UnsupportedMaterial,
                        scope,
                        unit,
                        "A1 mini profile does not support the unit's printable material.",
                    ));
                }
            }
            if a1_requested {
                errors.push(unit_error(
                    ErrorCode::RequestedA1Unavailable,
                    scope,
                    unit,
                    "Unit is pinned to A1 mini but is not an eligible A1 mono unit.",
                ));
                return None;
            }
            Printer::U1
        };

    if printer == Printer::U1
        && !config
            .u1_build_volume
            .contains(unit.bounds.required_volume())
    {
        errors.push(unit_error(
            ErrorCode::UnitOutOfBounds,
            scope,
            unit,
            "Unit does not fit within the configured U1 build volume.",
        ));
        return None;
    }
    if printer == Printer::A1Mini {
        for mapping in &mut mappings {
            mapping.strategy = ColorStrategy::A1Mono;
            mapping.direct_toolhead = None;
        }
    }

    let (strategy, printer_loadout) = if printer == Printer::A1Mini {
        (
            ColorStrategy::A1Mono,
            PrinterLoadout::A1Mini {
                spool_id: mono_spool.expect("A1 eligibility requires one physical spool"),
            },
        )
    } else {
        (
            ColorStrategy::DirectSpools,
            PrinterLoadout::U1 {
                loadout: U1Loadout { slots: loadout },
            },
        )
    };

    Some(UnitPlan {
        unit_ref: ScopedUnitRef {
            scope_id: scope.id.clone(),
            unit_id: unit.id.clone(),
        },
        scope_id: scope.id.clone(),
        source_boundary: source_boundary(scope, unit),
        key: JobKey {
            printer,
            strategy,
            loadout: printer_loadout,
            printable_materials: materials,
            fast_mono,
            mono_spool_id,
            full_spectrum_process: None,
        },
        mappings,
        bounds: unit.bounds,
    })
}

#[allow(clippy::too_many_arguments)]
fn plan_cmyx_unit(
    scope: &PrintScope,
    unit: &PrintableUnit,
    requirements: &[&EffectiveRequirement],
    inventory: &BTreeMap<String, Spool>,
    config: &PlannerConfig,
    warnings: &mut Vec<PlanWarning>,
    errors: &mut Vec<PlanError>,
) -> Option<UnitPlan> {
    let mut required_t4 = BTreeSet::new();
    let mut used_spool_ids = BTreeSet::new();
    let mut full_spectrum_processes = BTreeSet::new();
    let mut mappings = Vec::new();
    let mut all_solid = true;

    for requirement in requirements {
        if requirement.approved_color_fallback {
            warnings.push(unit_warning(
                WarningCode::ApprovedColorFallback,
                scope,
                unit,
                &format!(
                    "Requirement '{}' uses an explicitly approved best-effort CMY+X color candidate.",
                    requirement.ids[0]
                ),
            ));
        }
        if !cmy_candidate_process_compatible(&requirement.cmyx, config) {
            errors.push(unit_error(
                ErrorCode::FullSpectrumProcessMismatch,
                scope,
                unit,
                &format!(
                    "Full Spectrum recipe for requirement '{}' is not reproducible by the selected target process (profile, layer height, subdivision or effective sublayer differs).",
                    requirement.ids[0]
                ),
            ));
            return None;
        }
        if matches!(requirement.cmyx.recipe, CmyxRecipe::FullSpectrum { .. }) {
            full_spectrum_processes.extend(requirement.cmyx.process_compatibility.iter().cloned());
        }
        let slots = match recipe_slots(&requirement.cmyx.recipe) {
            Some(slots) if cmy_candidate_is_structurally_valid(&requirement.cmyx) => slots,
            _ => {
                errors.push(unit_error(
                    ErrorCode::CmyxRecipeUnavailable,
                    scope,
                    unit,
                    &format!(
                        "Requirement '{}' has no schedulable CMY+X recipe.",
                        requirement.ids[0]
                    ),
                ));
                return None;
            }
        };
        all_solid &= matches!(
            requirement.cmyx.recipe,
            CmyxRecipe::Solid { .. } | CmyxRecipe::DedicatedT4
        );
        if matches!(requirement.cmyx.recipe, CmyxRecipe::Fallback { .. }) {
            warnings.push(unit_warning(
                WarningCode::FallbackRecipe,
                scope,
                unit,
                &format!(
                    "Requirement '{}' uses a fallback CMY+X recipe.",
                    requirement.ids[0]
                ),
            ));
        }

        let mut recipe_spools = Vec::new();
        for slot in slots {
            let spool_id = match slot {
                Toolhead::T1 => Some(config.cmy_setup.cyan_spool_id.as_str()),
                Toolhead::T2 => Some(config.cmy_setup.magenta_spool_id.as_str()),
                Toolhead::T3 => Some(config.cmy_setup.yellow_spool_id.as_str()),
                Toolhead::T4 => requirement.cmyx.required_t4_spool_id.as_deref(),
            };
            let Some(spool_id) = spool_id else {
                errors.push(unit_error(
                    ErrorCode::InvalidCmyxRecipe,
                    scope,
                    unit,
                    "A CMY+X recipe uses T4 without naming its physical spool.",
                ));
                return None;
            };
            let Some(spool) = inventory.get(spool_id).filter(|spool| spool.available) else {
                errors.push(unit_error(
                    ErrorCode::UnknownSpool,
                    scope,
                    unit,
                    &format!("CMY+X recipe requires unavailable spool '{spool_id}'."),
                ));
                return None;
            };
            if slot == Toolhead::T4 {
                required_t4.insert(spool_id.to_owned());
            }
            used_spool_ids.insert(spool_id.to_owned());
            recipe_spools.push(spool);
        }

        let solid_spool = if all_solid_for(&requirement.cmyx.recipe) && recipe_spools.len() == 1 {
            Some(recipe_spools[0])
        } else {
            None
        };
        let material = common_material(recipe_spools.iter().copied());
        if requirement.approved_color_fallback
            && requirement
                .best_effort_cmyx
                .as_ref()
                .is_some_and(|fallback| material.as_ref() != Some(&fallback.target_material))
        {
            errors.push(unit_error(
                ErrorCode::InvalidCmyxRecipe,
                scope,
                unit,
                "The approved fallback recipe no longer resolves to its fingerprinted target material.",
            ));
            return None;
        }
        if let Some(actual_material) = &material
            && actual_material != &requirement.key.material
        {
            let approved = requirement
                .approved_material_substitution
                .as_ref()
                .is_some_and(|approval| {
                    approval.source_material == requirement.key.material
                        && &approval.target_material == actual_material
                });
            if !approved {
                errors.push(unit_error(
                    ErrorCode::MaterialSubstitutionApprovalRequired,
                    scope,
                    unit,
                    &format!(
                        "CMY+X recipe for requirement '{}' would convert {:?} to {:?}; a separate material-substitution approval is required.",
                        requirement.ids[0], requirement.key.material, actual_material
                    ),
                ));
                return None;
            }
            warnings.push(unit_warning(
                WarningCode::ApprovedMaterialSubstitution,
                scope,
                unit,
                &format!(
                    "Requirement '{}' explicitly substitutes {:?} with {:?}.",
                    requirement.ids[0], requirement.key.material, actual_material
                ),
            ));
        }
        let status = if material
            .as_ref()
            .is_some_and(|actual_material| actual_material != &requirement.key.material)
        {
            MappingStatus::MaterialMismatch
        } else {
            cmy_mapping_status(requirement)
        };
        mappings.push(SourceToActualMapping {
            scope_id: scope.id.clone(),
            source_requirement_ids: requirement.ids.clone(),
            source_slots: requirement.source_slots.clone(),
            source_profile_ids: requirement.source_profile_ids.clone(),
            source_material: requirement.key.material.clone(),
            source_color: requirement.key.color,
            strategy: ColorStrategy::CmyxFullSpectrum,
            cmyx_comparison: requirement.cmyx.clone(),
            direct_toolhead: None,
            actual_spool_id: solid_spool.map(|spool| spool.id.clone()),
            actual_material: material,
            actual_color: requirement.cmyx.predicted_color,
            delta_e00: requirement.cmyx.delta_e00,
            confidence: requirement.cmyx.confidence,
            status,
        });
    }

    if required_t4.len() > 1 {
        errors.push(unit_error(
            ErrorCode::MultipleT4SpoolsInUnit,
            scope,
            unit,
            "An indivisible unit requires more than one T4 spool.",
        ));
        return None;
    }
    if full_spectrum_processes.len() > 1 {
        errors.push(unit_error(
            ErrorCode::FullSpectrumProcessMismatch,
            scope,
            unit,
            "An indivisible unit requires Full Spectrum recipes with different process contracts.",
        ));
        return None;
    }
    let full_spectrum_process = full_spectrum_processes.into_iter().next();

    let mut materials = BTreeSet::new();
    for spool_id in &used_spool_ids {
        if let Some(spool) = inventory.get(spool_id) {
            materials.insert(spool.material.clone());
        }
    }
    let materials: Vec<_> = materials.into_iter().collect();
    if materials.len() > 1 && !config.allow_mixed_materials_on_plate {
        errors.push(unit_error(
            ErrorCode::MixedPrintableMaterials,
            scope,
            unit,
            "Unit would print multiple material families on one plate; the default policy forbids this.",
        ));
        return None;
    }

    let mono_spool = if all_solid && used_spool_ids.len() == 1 {
        used_spool_ids.iter().next().cloned()
    } else {
        None
    };
    let fast_mono = mono_spool.is_some();
    let mono_spool_id = mono_spool.clone();
    let a1_spool_available = mono_spool
        .as_ref()
        .is_some_and(|spool_id| !config.a1_mini.reserved_spool_ids.contains(spool_id));
    let a1_requested = unit.printer_preference == PrinterPreference::A1Mini;
    let a1_considered = config.a1_mini.enabled
        && config.a1_mini.route_fast_mono
        && unit.printer_preference != PrinterPreference::U1;
    let a1_fit = unit.bounds.has_known_size()
        && config
            .a1_mini
            .build_volume
            .contains(unit.bounds.required_volume());
    let a1_material_supported =
        materials.len() == 1 && config.a1_mini.supported_materials.contains(&materials[0]);

    let printer =
        if a1_considered && fast_mono && a1_spool_available && a1_fit && a1_material_supported {
            Printer::A1Mini
        } else {
            if a1_considered || a1_requested {
                if !fast_mono {
                    warnings.push(unit_warning(
                        WarningCode::A1NotSinglePhysicalSpool,
                        scope,
                        unit,
                        "A1 mini accepts only single-physical-spool units in this planner.",
                    ));
                }
                if fast_mono && !a1_spool_available {
                    warnings.push(unit_warning(
                        WarningCode::A1SpoolReservedForU1,
                        scope,
                        unit,
                        "The physical spool is explicitly marked unavailable to the A1 mini.",
                    ));
                }
                if unit.bounds.has_known_size() && !a1_fit {
                    warnings.push(unit_warning(
                        WarningCode::A1OutOfBounds,
                        scope,
                        unit,
                        "Unit including clearance exceeds the 180 × 180 × 180 mm A1 mini volume.",
                    ));
                }
                if !a1_material_supported {
                    warnings.push(unit_warning(
                        WarningCode::A1UnsupportedMaterial,
                        scope,
                        unit,
                        "A1 mini profile does not support the unit's printable material.",
                    ));
                }
            }
            if a1_requested {
                errors.push(unit_error(
                    ErrorCode::RequestedA1Unavailable,
                    scope,
                    unit,
                    "Unit is pinned to A1 mini but is not an eligible A1 mono unit.",
                ));
                return None;
            }
            Printer::U1
        };

    let u1_t4 = required_t4.iter().next().cloned();
    let u1_loadout = PrinterLoadout::U1 {
        loadout: U1Loadout {
            slots: [
                Some(config.cmy_setup.cyan_spool_id.clone()),
                Some(config.cmy_setup.magenta_spool_id.clone()),
                Some(config.cmy_setup.yellow_spool_id.clone()),
                u1_t4,
            ],
        },
    };
    let u1_strategy = if all_solid && !mappings.is_empty() && full_spectrum_process.is_none() {
        ColorStrategy::CmyxSolid
    } else {
        ColorStrategy::CmyxFullSpectrum
    };
    for mapping in &mut mappings {
        mapping.strategy = u1_strategy;
    }
    let strategy = if printer == Printer::A1Mini {
        ColorStrategy::A1Mono
    } else {
        u1_strategy
    };
    if strategy == ColorStrategy::A1Mono {
        for mapping in &mut mappings {
            mapping.strategy = ColorStrategy::A1Mono;
        }
    }

    let loadout = if printer == Printer::A1Mini {
        PrinterLoadout::A1Mini {
            spool_id: mono_spool.expect("A1 eligibility requires a mono spool"),
        }
    } else {
        if !config
            .u1_build_volume
            .contains(unit.bounds.required_volume())
        {
            errors.push(unit_error(
                ErrorCode::UnitOutOfBounds,
                scope,
                unit,
                "Unit does not fit within the configured U1 build volume.",
            ));
            return None;
        }
        // T4 is a wildcard when no recipe uses it. The scheduler keeps the
        // physically loaded T4 instead of forcing the configured restore spool.
        u1_loadout
    };

    mappings.sort_by(mapping_order);
    Some(UnitPlan {
        unit_ref: ScopedUnitRef {
            scope_id: scope.id.clone(),
            unit_id: unit.id.clone(),
        },
        scope_id: scope.id.clone(),
        source_boundary: source_boundary(scope, unit),
        key: JobKey {
            printer,
            strategy,
            loadout,
            printable_materials: materials,
            fast_mono,
            mono_spool_id,
            full_spectrum_process: if strategy == ColorStrategy::CmyxFullSpectrum {
                full_spectrum_process
            } else {
                None
            },
        },
        mappings,
        bounds: unit.bounds,
    })
}

fn recipe_slots(recipe: &CmyxRecipe) -> Option<Vec<Toolhead>> {
    let mut slots = match recipe {
        CmyxRecipe::Solid { toolhead } => vec![*toolhead],
        CmyxRecipe::FullSpectrum { sequence, .. } => sequence.clone(),
        CmyxRecipe::DedicatedT4 => vec![Toolhead::T4],
        CmyxRecipe::Fallback { physical_slots, .. } => physical_slots.clone(),
        CmyxRecipe::ManualReview { .. } | CmyxRecipe::Unreachable { .. } => return None,
    };
    slots.sort();
    slots.dedup();
    Some(slots)
}

fn cmy_candidate_is_structurally_valid(candidate: &CmyxColorCandidate) -> bool {
    let Some(slots) = recipe_slots(&candidate.recipe) else {
        return false;
    };
    !slots.is_empty()
        && (slots.contains(&Toolhead::T4) == candidate.required_t4_spool_id.is_some())
        && match candidate.recipe {
            CmyxRecipe::FullSpectrum { .. } => candidate
                .process_compatibility
                .as_ref()
                .is_some_and(full_spectrum_process_is_valid),
            _ => true,
        }
        && candidate
            .delta_e00
            .is_none_or(|value| value.is_finite() && value >= 0.0)
}

fn cmy_candidate_process_compatible(
    candidate: &CmyxColorCandidate,
    config: &PlannerConfig,
) -> bool {
    if !matches!(candidate.recipe, CmyxRecipe::FullSpectrum { .. }) {
        return true;
    }
    candidate
        .process_compatibility
        .as_ref()
        .is_some_and(|process| {
            full_spectrum_process_is_valid(process)
                && config
                    .supported_full_spectrum_processes
                    .iter()
                    .any(|supported| supported == process)
        })
}

fn full_spectrum_process_is_valid(process: &FullSpectrumProcessCompatibility) -> bool {
    if process.printer_profile_fingerprint.trim().is_empty()
        || process.process_fingerprint.trim().is_empty()
        || process.plate_layer_height_microns == 0
        || process.subdivision_factor == 0
        || process.effective_sublayer_height_microns == 0
    {
        return false;
    }
    if u64::from(process.effective_sublayer_height_microns) * u64::from(process.subdivision_factor)
        != u64::from(process.plate_layer_height_microns)
    {
        return false;
    }
    process.subdivision_policy != FullSpectrumSubdivisionPolicy::Disabled
        || process.subdivision_factor == 1
}

fn all_solid_for(recipe: &CmyxRecipe) -> bool {
    matches!(recipe, CmyxRecipe::Solid { .. } | CmyxRecipe::DedicatedT4)
}

fn common_material<'a>(mut spools: impl Iterator<Item = &'a Spool>) -> Option<Material> {
    let first = spools.next()?.material.clone();
    spools.all(|spool| spool.material == first).then_some(first)
}

fn cmy_mapping_status(requirement: &EffectiveRequirement) -> MappingStatus {
    match requirement.cmyx.delta_e00 {
        Some(delta) => delta_status(delta),
        None if requirement.cmyx.predicted_color == Some(requirement.key.color) => {
            MappingStatus::Exact
        }
        None => MappingStatus::Review,
    }
}

fn mapping_status(
    source_material: &Material,
    source_color: RgbColor,
    spool: &Spool,
    delta_e00: Option<f64>,
) -> MappingStatus {
    if source_material != &spool.material {
        MappingStatus::MaterialMismatch
    } else if let Some(delta) = delta_e00 {
        delta_status(delta)
    } else if source_color == spool.actual_color() {
        MappingStatus::Exact
    } else {
        MappingStatus::Review
    }
}

fn delta_status(delta: f64) -> MappingStatus {
    if delta <= f64::EPSILON {
        MappingStatus::Exact
    } else if delta <= 3.0 {
        MappingStatus::Close
    } else if delta <= 6.0 {
        MappingStatus::Review
    } else {
        MappingStatus::Poor
    }
}

fn source_boundary(scope: &PrintScope, unit: &PrintableUnit) -> SourceBoundary {
    unit.source_plate_id
        .clone()
        .map(SourceBoundary::SourcePlate)
        .unwrap_or_else(|| SourceBoundary::Scope(scope.id.clone()))
}

fn warn_about_cross_printer_spool_transfers(
    unit_plans: &[UnitPlan],
    warnings: &mut Vec<PlanWarning>,
) {
    let u1_spool_ids = unit_plans
        .iter()
        .filter_map(|unit_plan| match &unit_plan.key.loadout {
            PrinterLoadout::U1 { loadout } => Some(loadout.slots.iter()),
            PrinterLoadout::A1Mini { .. } => None,
        })
        .flatten()
        .flatten()
        .cloned()
        .collect::<BTreeSet<_>>();

    let shared_spool_ids = unit_plans
        .iter()
        .filter_map(|unit_plan| match &unit_plan.key.loadout {
            PrinterLoadout::A1Mini { spool_id } if u1_spool_ids.contains(spool_id) => {
                Some(spool_id.clone())
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();

    for spool_id in shared_spool_ids {
        warnings.push(PlanWarning {
            code: WarningCode::A1SpoolSharedWithU1,
            scope_id: None,
            unit_id: None,
            message: format!(
                "Physical spool '{spool_id}' is used by both printers. Follow the sequential batch order and move this spool from the U1 to the A1 mini when instructed; these batches cannot run in parallel with one spool."
            ),
        });
    }
}

fn group_jobs_and_plates(
    unit_plans: Vec<UnitPlan>,
    config: &PlannerConfig,
    warnings: &mut Vec<PlanWarning>,
    errors: &mut Vec<PlanError>,
) -> (Vec<PlannedJob>, Vec<PlannedPlate>) {
    let unit_plans = promote_compatible_direct_loadouts(unit_plans, config);
    let mut groups: BTreeMap<PackingGroupKey, Vec<UnitPlan>> = BTreeMap::new();
    for plan in unit_plans {
        groups
            .entry(PackingGroupKey {
                job: plan.key.clone(),
                u1_source_boundary: (plan.key.printer == Printer::U1
                    && !config.allow_u1_cross_source_repacking)
                    .then(|| plan.source_boundary.clone()),
            })
            .or_default()
            .push(plan);
    }

    let mut jobs = Vec::new();
    let mut plates = Vec::new();
    for (group_key, units) in groups {
        let key = group_key.job;
        let build_volume = match key.printer {
            Printer::U1 => config.u1_build_volume,
            Printer::A1Mini => config.a1_mini.build_volume,
        };
        let packing_origin = match key.printer {
            // Snapmaker Orca's qualified U1 coordinate contract is
            // 0.5..270.5 × 1.0..271.0 for the 270 × 270 mm printable area.
            Printer::U1 => (0.5, 1.0),
            Printer::A1Mini => (0.0, 0.0),
        };
        let requires_prime_tower = group_requires_prime_tower(&key, &units);
        let pages = pack_unit_group(
            units,
            build_volume,
            packing_origin,
            requires_prime_tower,
            errors,
        );
        if pages
            .iter()
            .any(|page| page.units.iter().any(|(_, placement)| placement.is_none()))
        {
            warnings.push(PlanWarning {
                code: WarningCode::PackingNotPerformed,
                scope_id: None,
                unit_id: None,
                message: "A target plate remains provisional because at least one unit has no proven geometry bounds."
                    .into(),
            });
        }
        if pages.len() > 1 {
            warnings.push(PlanWarning {
                code: WarningCode::PackingSplitAcrossPlates,
                scope_id: None,
                unit_id: None,
                message: format!(
                    "A {:?} {:?} loadout was deterministically split across {} target plates to preserve clearances.",
                    key.printer,
                    key.strategy,
                    pages.len()
                ),
            });
        }
        for mut page in pages {
            page.units
                .sort_by(|left, right| left.0.unit_ref.cmp(&right.0.unit_ref));
            let id = format!("job-{:03}", jobs.len() + 1);
            let plate_id = format!("plate-{:03}", plates.len() + 1);
            let scope_ids: Vec<_> = page
                .units
                .iter()
                .map(|(unit, _)| unit.scope_id.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let scoped_units: Vec<_> = page
                .units
                .iter()
                .map(|(unit, _)| unit.unit_ref.clone())
                .collect();
            let placements = page
                .units
                .iter()
                .filter_map(|(_, placement)| placement.clone())
                .collect();
            let mut mappings: BTreeMap<(String, Vec<String>), SourceToActualMapping> =
                BTreeMap::new();
            let individual_bounds_validated =
                page.units.iter().all(|(_, placement)| placement.is_some());
            for mapping in page.units.into_iter().flat_map(|(unit, _)| unit.mappings) {
                mappings.insert(
                    (
                        mapping.scope_id.clone(),
                        mapping.source_requirement_ids.clone(),
                    ),
                    mapping,
                );
            }

            jobs.push(PlannedJob {
                id: id.clone(),
                scope_ids,
                units: scoped_units.clone(),
                printer: key.printer,
                strategy: key.strategy,
                loadout: key.loadout.clone(),
                printable_materials: key.printable_materials.clone(),
                fast_mono: key.fast_mono,
                full_spectrum_process: key.full_spectrum_process.clone(),
                color_mappings: mappings.into_values().collect(),
                estimated_tool_changes: Estimate::RequiresSlicing,
            });
            plates.push(PlannedPlate {
                id: plate_id,
                job_id: id,
                printer: key.printer,
                units: scoped_units,
                placements,
                prime_tower: page.prime_tower,
                packing_status: if individual_bounds_validated {
                    PackingStatus::PackedAabb
                } else {
                    PackingStatus::RequiresGeometryPacking
                },
                individual_bounds_validated,
                full_spectrum_process: key.full_spectrum_process.clone(),
                estimated_print_time_seconds: Estimate::RequiresSlicing,
                estimated_material_grams: Estimate::RequiresSlicing,
            });
        }
    }
    (jobs, plates)
}

fn promote_compatible_direct_loadouts(
    unit_plans: Vec<UnitPlan>,
    config: &PlannerConfig,
) -> Vec<UnitPlan> {
    let mut unchanged = Vec::new();
    let mut families = BTreeMap::<DirectPackingFamilyKey, Vec<UnitPlan>>::new();

    for plan in unit_plans {
        let eligible = plan.key.printer == Printer::U1
            && plan.key.strategy == ColorStrategy::DirectSpools
            && !plan.key.fast_mono
            && matches!(plan.key.loadout, PrinterLoadout::U1 { .. });
        if !eligible {
            unchanged.push(plan);
            continue;
        }
        families
            .entry(DirectPackingFamilyKey {
                printable_materials: plan.key.printable_materials.clone(),
                mono_spool_id: plan.key.mono_spool_id.clone(),
                full_spectrum_process: plan.key.full_spectrum_process.clone(),
                u1_source_boundary: (!config.allow_u1_cross_source_repacking)
                    .then(|| plan.source_boundary.clone()),
            })
            .or_default()
            .push(plan);
    }

    for (_, mut plans) in families {
        plans.sort_by(|left, right| {
            u1_loadout_specificity(&right.key.loadout)
                .cmp(&u1_loadout_specificity(&left.key.loadout))
                .then_with(|| left.key.loadout.cmp(&right.key.loadout))
                .then_with(|| left.unit_ref.cmp(&right.unit_ref))
        });
        let mut compatible_groups = Vec::<CompatibleDirectPackingGroup>::new();

        for plan in plans {
            let PrinterLoadout::U1 {
                loadout: plan_loadout,
            } = &plan.key.loadout
            else {
                unreachable!("eligible Direct packing plans always use a U1 loadout");
            };
            let best = compatible_groups
                .iter()
                .enumerate()
                .filter_map(|(index, group)| {
                    merge_compatible_u1_loadouts(&group.loadout, plan_loadout).map(|merged| {
                        let added_slots = merged
                            .slots
                            .iter()
                            .zip(&group.loadout.slots)
                            .filter(|(merged, current)| merged.is_some() && current.is_none())
                            .count();
                        (index, merged, added_slots)
                    })
                })
                .min_by(|left, right| {
                    left.2
                        .cmp(&right.2)
                        .then_with(|| left.1.slots.cmp(&right.1.slots))
                        .then_with(|| left.0.cmp(&right.0))
                });

            if let Some((index, merged, _)) = best {
                compatible_groups[index].loadout = merged;
                compatible_groups[index].units.push(plan);
            } else {
                compatible_groups.push(CompatibleDirectPackingGroup {
                    loadout: plan_loadout.clone(),
                    units: vec![plan],
                });
            }
        }

        for mut group in compatible_groups {
            for unit in &mut group.units {
                unit.key.loadout = PrinterLoadout::U1 {
                    loadout: group.loadout.clone(),
                };
            }
            unchanged.extend(group.units);
        }
    }

    unchanged
}

fn u1_loadout_specificity(loadout: &PrinterLoadout) -> usize {
    match loadout {
        PrinterLoadout::U1 { loadout } => {
            loadout.slots.iter().filter(|slot| slot.is_some()).count()
        }
        PrinterLoadout::A1Mini { .. } => 0,
    }
}

fn merge_compatible_u1_loadouts(left: &U1Loadout, right: &U1Loadout) -> Option<U1Loadout> {
    let mut slots = std::array::from_fn(|_| None);
    for (index, slot) in slots.iter_mut().enumerate() {
        *slot = match (&left.slots[index], &right.slots[index]) {
            (Some(left), Some(right)) if left != right => return None,
            (Some(spool), _) | (_, Some(spool)) => Some(spool.clone()),
            (None, None) => None,
        };
    }
    Some(U1Loadout { slots })
}

const PACKING_EPSILON_MM: f64 = 1.0e-6;
const U1_PRIME_TOWER_BODY_WIDTH_MM: f64 = 30.0;
const U1_PRIME_TOWER_BODY_DEPTH_MM: f64 = 45.0;
// The exact U1 profiles use a 15 degree stabilization cone. At the qualified
// 270 mm printable height its half-width is 35.55 mm. Add the 5 mm brim,
// 8 mm rib, and 1 mm tower clearance used by both U1 writers. Rounding upward
// to 49.6 mm keeps packing conservative for Direct and Full Spectrum.
const U1_PRIME_TOWER_MAX_HALF_EXTENT_MM: f64 = 49.6;
// Keep the writer's worst-case 18 mm automatic brim plus 1 mm brim gap
// between the complete tower envelope and any packed unit. Unit-specific
// clearances remain additional to this process-level separation.
const U1_PRIME_TOWER_OBJECT_CLEARANCE_MM: f64 = 19.0;
const U1_PRIME_TOWER_RESERVE_WIDTH_MM: f64 =
    2.0 * U1_PRIME_TOWER_MAX_HALF_EXTENT_MM + U1_PRIME_TOWER_OBJECT_CLEARANCE_MM;
const U1_PRIME_TOWER_RESERVE_DEPTH_MM: f64 =
    2.0 * U1_PRIME_TOWER_MAX_HALF_EXTENT_MM + U1_PRIME_TOWER_OBJECT_CLEARANCE_MM;

#[derive(Clone, Copy, Debug, PartialEq)]
struct PackRect {
    x: f64,
    y: f64,
    width: f64,
    depth: f64,
}

impl PackRect {
    fn right(self) -> f64 {
        self.x + self.width
    }

    fn top(self) -> f64 {
        self.y + self.depth
    }
}

#[derive(Debug)]
struct PackingPage {
    free: Vec<PackRect>,
    units: Vec<(UnitPlan, Option<PlannedPlacement>)>,
    prime_tower: Option<PlannedPrimeTower>,
}

impl PackingPage {
    fn new(
        build_volume: crate::BuildVolumeMm,
        origin: (f64, f64),
        reserve_prime_tower: bool,
    ) -> Self {
        let mut page = Self {
            free: vec![PackRect {
                x: origin.0,
                y: origin.1,
                width: build_volume.width,
                depth: build_volume.depth,
            }],
            units: Vec::new(),
            prime_tower: None,
        };
        if reserve_prime_tower {
            let reserve = PackRect {
                x: origin.0 + build_volume.width - U1_PRIME_TOWER_RESERVE_WIDTH_MM,
                y: origin.1 + build_volume.depth - U1_PRIME_TOWER_RESERVE_DEPTH_MM,
                width: U1_PRIME_TOWER_RESERVE_WIDTH_MM,
                depth: U1_PRIME_TOWER_RESERVE_DEPTH_MM,
            };
            page.commit(reserve);
            page.prime_tower = Some(PlannedPrimeTower {
                x_mm: reserve.x
                    + U1_PRIME_TOWER_OBJECT_CLEARANCE_MM
                    + U1_PRIME_TOWER_MAX_HALF_EXTENT_MM
                    - U1_PRIME_TOWER_BODY_WIDTH_MM / 2.0,
                y_mm: reserve.y
                    + U1_PRIME_TOWER_OBJECT_CLEARANCE_MM
                    + U1_PRIME_TOWER_MAX_HALF_EXTENT_MM
                    - U1_PRIME_TOWER_BODY_DEPTH_MM / 2.0,
            });
        }
        page
    }

    fn candidate(&self, width: f64, depth: f64) -> Option<PackRect> {
        self.free
            .iter()
            .filter(|free| {
                width <= free.width + PACKING_EPSILON_MM && depth <= free.depth + PACKING_EPSILON_MM
            })
            .map(|free| {
                let remaining_width = (free.width - width).max(0.0);
                let remaining_depth = (free.depth - depth).max(0.0);
                (
                    PackRect {
                        x: free.x,
                        y: free.y,
                        width,
                        depth,
                    },
                    remaining_width.min(remaining_depth),
                    remaining_width.max(remaining_depth),
                )
            })
            .min_by(|left, right| {
                left.1
                    .total_cmp(&right.1)
                    .then_with(|| left.2.total_cmp(&right.2))
                    .then_with(|| left.0.y.total_cmp(&right.0.y))
                    .then_with(|| left.0.x.total_cmp(&right.0.x))
            })
            .map(|candidate| candidate.0)
    }

    fn commit(&mut self, used: PackRect) {
        let mut split = Vec::new();
        for free in self.free.drain(..) {
            if !rectangles_intersect(free, used) {
                split.push(free);
                continue;
            }
            if used.x > free.x + PACKING_EPSILON_MM {
                split.push(PackRect {
                    x: free.x,
                    y: free.y,
                    width: used.x - free.x,
                    depth: free.depth,
                });
            }
            if used.right() < free.right() - PACKING_EPSILON_MM {
                split.push(PackRect {
                    x: used.right(),
                    y: free.y,
                    width: free.right() - used.right(),
                    depth: free.depth,
                });
            }
            if used.y > free.y + PACKING_EPSILON_MM {
                split.push(PackRect {
                    x: free.x,
                    y: free.y,
                    width: free.width,
                    depth: used.y - free.y,
                });
            }
            if used.top() < free.top() - PACKING_EPSILON_MM {
                split.push(PackRect {
                    x: free.x,
                    y: used.top(),
                    width: free.width,
                    depth: free.top() - used.top(),
                });
            }
        }
        split.retain(|rect| rect.width > PACKING_EPSILON_MM && rect.depth > PACKING_EPSILON_MM);
        let mut retained = Vec::new();
        for (index, candidate) in split.iter().copied().enumerate() {
            let contained = split.iter().enumerate().any(|(other_index, other)| {
                index != other_index && rectangle_contains(*other, candidate)
            });
            if !contained && !retained.contains(&candidate) {
                retained.push(candidate);
            }
        }
        retained.sort_by(|left, right| {
            left.y
                .total_cmp(&right.y)
                .then_with(|| left.x.total_cmp(&right.x))
                .then_with(|| left.width.total_cmp(&right.width))
                .then_with(|| left.depth.total_cmp(&right.depth))
        });
        self.free = retained;
    }
}

fn rectangles_intersect(left: PackRect, right: PackRect) -> bool {
    left.x < right.right() - PACKING_EPSILON_MM
        && left.right() > right.x + PACKING_EPSILON_MM
        && left.y < right.top() - PACKING_EPSILON_MM
        && left.top() > right.y + PACKING_EPSILON_MM
}

fn rectangle_contains(outer: PackRect, inner: PackRect) -> bool {
    inner.x >= outer.x - PACKING_EPSILON_MM
        && inner.y >= outer.y - PACKING_EPSILON_MM
        && inner.right() <= outer.right() + PACKING_EPSILON_MM
        && inner.top() <= outer.top() + PACKING_EPSILON_MM
}

fn group_requires_prime_tower(key: &JobKey, units: &[UnitPlan]) -> bool {
    if key.printer != Printer::U1 {
        return false;
    }
    let mut toolheads = BTreeSet::new();
    for mapping in units.iter().flat_map(|unit| &unit.mappings) {
        if let Some(toolhead) = mapping.direct_toolhead {
            toolheads.insert(toolhead);
        }
        if matches!(
            mapping.strategy,
            ColorStrategy::CmyxFullSpectrum | ColorStrategy::CmyxSolid
        ) && let Some(slots) = recipe_slots(&mapping.cmyx_comparison.recipe)
        {
            toolheads.extend(slots);
        }
    }
    toolheads.len() > 1
}

fn pack_unit_group(
    mut units: Vec<UnitPlan>,
    build_volume: crate::BuildVolumeMm,
    packing_origin: (f64, f64),
    reserve_prime_tower: bool,
    errors: &mut Vec<PlanError>,
) -> Vec<PackingPage> {
    units.sort_by(|left, right| {
        let left_volume = left.bounds.required_volume();
        let right_volume = right.bounds.required_volume();
        (right_volume.width * right_volume.depth)
            .total_cmp(&(left_volume.width * left_volume.depth))
            .then_with(|| {
                right_volume
                    .width
                    .max(right_volume.depth)
                    .total_cmp(&left_volume.width.max(left_volume.depth))
            })
            .then_with(|| left.unit_ref.cmp(&right.unit_ref))
    });

    let mut pages = Vec::<PackingPage>::new();
    for unit in units {
        if !unit.bounds.has_known_size() {
            pages.push(PackingPage {
                free: Vec::new(),
                units: vec![(unit, None)],
                prime_tower: None,
            });
            continue;
        }
        let required = unit.bounds.required_volume();
        let target = pages.iter().enumerate().find_map(|(index, page)| {
            page.candidate(required.width, required.depth)
                .map(|rect| (index, rect))
        });
        let (page_index, placed) = if let Some(target) = target {
            target
        } else {
            let page = PackingPage::new(build_volume, packing_origin, reserve_prime_tower);
            let Some(placed) = page.candidate(required.width, required.depth) else {
                errors.push(PlanError {
                    code: ErrorCode::PackingFailed,
                    scope_id: Some(unit.scope_id.clone()),
                    unit_id: Some(unit.unit_ref.unit_id.clone()),
                    message: format!(
                        "Unit requires a {:.1} × {:.1} mm cleared footprint that cannot share the target bed with the required process structures.",
                        required.width, required.depth
                    ),
                });
                continue;
            };
            pages.push(page);
            (pages.len() - 1, placed)
        };
        let placement = PlannedPlacement {
            unit: unit.unit_ref.clone(),
            target_min_x_mm: placed.x + unit.bounds.clearance_x,
            target_min_y_mm: placed.y + unit.bounds.clearance_y,
        };
        pages[page_index].commit(placed);
        pages[page_index].units.push((unit, Some(placement)));
    }
    pages
}

fn schedule_batches(
    jobs: &[PlannedJob],
    plates: &[PlannedPlate],
    initial: &CurrentToolheadState,
    config: &PlannerConfig,
    warnings: &mut Vec<PlanWarning>,
) -> (Vec<PlannedBatch>, u32, u32, CurrentToolheadState) {
    let plate_by_job: BTreeMap<_, _> = plates
        .iter()
        .map(|plate| (plate.job_id.as_str(), plate.id.clone()))
        .collect();
    let mut grouped: BTreeMap<BatchKey, Vec<&PlannedJob>> = BTreeMap::new();
    for job in jobs {
        grouped
            .entry(BatchKey {
                printer: job.printer,
                strategy: job.strategy,
                loadout: job.loadout.clone(),
            })
            .or_default()
            .push(job);
    }

    let mut u1 = Vec::new();
    let mut a1 = Vec::new();
    for group in grouped {
        match group.0.printer {
            Printer::U1 => u1.push(group),
            Printer::A1Mini => a1.push(group),
        }
    }

    a1.sort_by(|left, right| {
        let current_spool = config.a1_mini.current_spool_id.as_deref();
        a1_initial_spool_rank(&left.0, current_spool)
            .cmp(&a1_initial_spool_rank(&right.0, current_spool))
            .then_with(|| left.0.cmp(&right.0))
    });

    let mut batches = Vec::new();
    let mut current = initial.clone();
    let mut t4_swap_count = 0_u32;

    while !u1.is_empty() {
        let next_index = u1
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                u1_batch_transition_cost(&current, &left.0, config)
                    .cmp(&u1_batch_transition_cost(&current, &right.0, config))
                    .then_with(|| {
                        u1_batch_specificity(&right.0).cmp(&u1_batch_specificity(&left.0))
                    })
                    .then_with(|| left.0.cmp(&right.0))
            })
            .map(|(index, _)| index)
            .expect("a non-empty U1 queue has a next batch");
        let (key, jobs) = u1.remove(next_index);
        let mut actions = Vec::new();
        let PrinterLoadout::U1 { loadout } = &key.loadout else {
            unreachable!("U1 strategies always have a U1 loadout")
        };
        let t4_swap_before = if matches!(
            key.strategy,
            ColorStrategy::CmyxFullSpectrum | ColorStrategy::CmyxSolid
        ) {
            cmy_t4_swap_needed(
                &current.slots[Toolhead::T4.index()],
                loadout.spool(Toolhead::T4),
            )
        } else {
            false
        };
        if t4_swap_before {
            t4_swap_count += 1;
        }
        transition_to_u1_loadout(
            &mut current,
            loadout,
            SetupPhase::BeforeBatch,
            &mut actions,
            warnings,
        );

        if key.strategy == ColorStrategy::DirectSpools && config.restore_cmy_after_direct {
            let restore = U1Loadout {
                slots: [
                    Some(config.cmy_setup.cyan_spool_id.clone()),
                    Some(config.cmy_setup.magenta_spool_id.clone()),
                    Some(config.cmy_setup.yellow_spool_id.clone()),
                    config.cmy_setup.default_t4_spool_id.clone(),
                ],
            };
            restore_cmy_loadout(&mut current, &restore, &mut actions);
        }

        let id = format!("batch-{:03}", batches.len() + 1);
        let mut job_ids: Vec<_> = jobs.iter().map(|job| job.id.clone()).collect();
        job_ids.sort();
        let mut plate_ids: Vec<_> = job_ids
            .iter()
            .filter_map(|job_id| plate_by_job.get(job_id.as_str()).cloned())
            .collect();
        plate_ids.sort();
        batches.push(PlannedBatch {
            id,
            printer: Printer::U1,
            strategy: key.strategy,
            loadout: key.loadout,
            job_ids,
            plate_ids,
            setup_actions: actions,
            t4_swap_before,
        });
    }

    let mut a1_spool_change_count = 0_u32;
    let mut current_a1_spool = config
        .a1_mini
        .current_spool_id
        .clone()
        .map_or(ToolheadSlotState::Unknown, ToolheadSlotState::Loaded);
    for (key, jobs) in a1 {
        let PrinterLoadout::A1Mini { spool_id } = &key.loadout else {
            unreachable!("A1 strategy always has an A1 loadout")
        };
        let mut actions = Vec::new();
        match &current_a1_spool {
            ToolheadSlotState::Loaded(current) if current == spool_id => {
                actions.push(SetupAction {
                    phase: SetupPhase::BeforeBatch,
                    kind: SetupActionKind::Keep,
                    toolhead: None,
                    from: current_a1_spool.clone(),
                    to: current_a1_spool.clone(),
                });
            }
            ToolheadSlotState::Loaded(_) => {
                actions.push(SetupAction {
                    phase: SetupPhase::BeforeBatch,
                    kind: SetupActionKind::Unload,
                    toolhead: None,
                    from: current_a1_spool.clone(),
                    to: ToolheadSlotState::Empty,
                });
                actions.push(SetupAction {
                    phase: SetupPhase::BeforeBatch,
                    kind: SetupActionKind::Load,
                    toolhead: None,
                    from: ToolheadSlotState::Empty,
                    to: ToolheadSlotState::Loaded(spool_id.clone()),
                });
                a1_spool_change_count += 1;
            }
            ToolheadSlotState::Unknown => {
                warnings.push(PlanWarning {
                    code: WarningCode::UnknownCurrentToolhead,
                    scope_id: None,
                    unit_id: None,
                    message: "Current A1 mini spool is unknown; the first A1 spool setup is counted conservatively."
                        .to_owned(),
                });
                actions.push(SetupAction {
                    phase: SetupPhase::BeforeBatch,
                    kind: SetupActionKind::Unload,
                    toolhead: None,
                    from: ToolheadSlotState::Unknown,
                    to: ToolheadSlotState::Empty,
                });
                actions.push(SetupAction {
                    phase: SetupPhase::BeforeBatch,
                    kind: SetupActionKind::Load,
                    toolhead: None,
                    from: ToolheadSlotState::Empty,
                    to: ToolheadSlotState::Loaded(spool_id.clone()),
                });
                a1_spool_change_count += 1;
            }
            ToolheadSlotState::Empty => {
                actions.push(SetupAction {
                    phase: SetupPhase::BeforeBatch,
                    kind: SetupActionKind::Load,
                    toolhead: None,
                    from: ToolheadSlotState::Empty,
                    to: ToolheadSlotState::Loaded(spool_id.clone()),
                });
                a1_spool_change_count += 1;
            }
        }
        current_a1_spool = ToolheadSlotState::Loaded(spool_id.clone());
        let mut job_ids: Vec<_> = jobs.iter().map(|job| job.id.clone()).collect();
        job_ids.sort();
        let mut plate_ids: Vec<_> = job_ids
            .iter()
            .filter_map(|job_id| plate_by_job.get(job_id.as_str()).cloned())
            .collect();
        plate_ids.sort();
        batches.push(PlannedBatch {
            id: format!("batch-{:03}", batches.len() + 1),
            printer: Printer::A1Mini,
            strategy: ColorStrategy::A1Mono,
            loadout: key.loadout,
            job_ids,
            plate_ids,
            setup_actions: actions,
            t4_swap_before: false,
        });
    }

    (batches, t4_swap_count, a1_spool_change_count, current)
}

fn a1_initial_spool_rank(key: &BatchKey, current_spool: Option<&str>) -> u8 {
    match (&key.loadout, current_spool) {
        (PrinterLoadout::A1Mini { spool_id }, Some(current)) if spool_id == current => 0,
        _ => 1,
    }
}

fn u1_batch_transition_cost(
    current: &CurrentToolheadState,
    key: &BatchKey,
    config: &PlannerConfig,
) -> usize {
    let PrinterLoadout::U1 { loadout } = &key.loadout else {
        return usize::MAX;
    };
    let mut simulated = current.clone();
    let mut cost = apply_u1_loadout_for_cost(&mut simulated, loadout);
    if key.strategy == ColorStrategy::DirectSpools && config.restore_cmy_after_direct {
        let restore = U1Loadout {
            slots: [
                Some(config.cmy_setup.cyan_spool_id.clone()),
                Some(config.cmy_setup.magenta_spool_id.clone()),
                Some(config.cmy_setup.yellow_spool_id.clone()),
                config.cmy_setup.default_t4_spool_id.clone(),
            ],
        };
        cost += apply_u1_loadout_for_cost(&mut simulated, &restore);
    }
    cost
}

fn u1_batch_specificity(key: &BatchKey) -> usize {
    match &key.loadout {
        PrinterLoadout::U1 { loadout } => loadout.slots.iter().flatten().count(),
        PrinterLoadout::A1Mini { .. } => 0,
    }
}

fn apply_u1_loadout_for_cost(current: &mut CurrentToolheadState, target: &U1Loadout) -> usize {
    let mut changes = 0;
    for toolhead in Toolhead::ALL {
        let Some(target_id) = target.slots[toolhead.index()].as_ref() else {
            continue;
        };
        let target_state = ToolheadSlotState::Loaded(target_id.clone());
        if current.slots[toolhead.index()] != target_state {
            changes += 1;
            current.slots[toolhead.index()] = target_state;
        }
    }
    changes
}

fn cmy_t4_swap_needed(current: &ToolheadSlotState, target: Option<&str>) -> bool {
    let Some(target) = target else { return false };
    !matches!(current, ToolheadSlotState::Loaded(current) if current == target)
}

fn transition_to_u1_loadout(
    current: &mut CurrentToolheadState,
    target: &U1Loadout,
    phase: SetupPhase,
    actions: &mut Vec<SetupAction>,
    warnings: &mut Vec<PlanWarning>,
) {
    for toolhead in Toolhead::ALL {
        let current_slot = current.slots[toolhead.index()].clone();
        let target_spool = target.slots[toolhead.index()].as_ref();
        match (&current_slot, target_spool) {
            (ToolheadSlotState::Loaded(current_id), Some(target_id)) if current_id == target_id => {
                actions.push(SetupAction {
                    phase,
                    kind: SetupActionKind::Keep,
                    toolhead: Some(toolhead),
                    from: current_slot.clone(),
                    to: current_slot,
                });
            }
            (ToolheadSlotState::Loaded(_), Some(target_id)) => {
                actions.push(SetupAction {
                    phase,
                    kind: SetupActionKind::Unload,
                    toolhead: Some(toolhead),
                    from: current_slot.clone(),
                    to: ToolheadSlotState::Empty,
                });
                let loaded = ToolheadSlotState::Loaded(target_id.clone());
                actions.push(SetupAction {
                    phase,
                    kind: SetupActionKind::Load,
                    toolhead: Some(toolhead),
                    from: ToolheadSlotState::Empty,
                    to: loaded.clone(),
                });
                current.slots[toolhead.index()] = loaded;
            }
            (ToolheadSlotState::Unknown, Some(target_id)) => {
                warnings.push(PlanWarning {
                    code: WarningCode::UnknownCurrentToolhead,
                    scope_id: None,
                    unit_id: None,
                    message: format!(
                        "Current {toolhead:?} state is unknown; setup is counted conservatively."
                    ),
                });
                actions.push(SetupAction {
                    phase,
                    kind: SetupActionKind::Unload,
                    toolhead: Some(toolhead),
                    from: ToolheadSlotState::Unknown,
                    to: ToolheadSlotState::Empty,
                });
                let loaded = ToolheadSlotState::Loaded(target_id.clone());
                actions.push(SetupAction {
                    phase,
                    kind: SetupActionKind::Load,
                    toolhead: Some(toolhead),
                    from: ToolheadSlotState::Empty,
                    to: loaded.clone(),
                });
                current.slots[toolhead.index()] = loaded;
            }
            (ToolheadSlotState::Empty, Some(target_id)) => {
                let loaded = ToolheadSlotState::Loaded(target_id.clone());
                actions.push(SetupAction {
                    phase,
                    kind: SetupActionKind::Load,
                    toolhead: Some(toolhead),
                    from: ToolheadSlotState::Empty,
                    to: loaded.clone(),
                });
                current.slots[toolhead.index()] = loaded;
            }
            (_, None) => {
                actions.push(SetupAction {
                    phase,
                    kind: SetupActionKind::Keep,
                    toolhead: Some(toolhead),
                    from: current_slot.clone(),
                    to: current_slot,
                });
            }
        }
    }
}

fn restore_cmy_loadout(
    current: &mut CurrentToolheadState,
    target: &U1Loadout,
    actions: &mut Vec<SetupAction>,
) {
    for toolhead in Toolhead::ALL {
        let from = current.slots[toolhead.index()].clone();
        let Some(target_id) = &target.slots[toolhead.index()] else {
            actions.push(SetupAction {
                phase: SetupPhase::AfterBatch,
                kind: SetupActionKind::Keep,
                toolhead: Some(toolhead),
                from: from.clone(),
                to: from,
            });
            continue;
        };
        let to = ToolheadSlotState::Loaded(target_id.clone());
        let kind = if from == to {
            SetupActionKind::Keep
        } else {
            SetupActionKind::Restore
        };
        actions.push(SetupAction {
            phase: SetupPhase::AfterBatch,
            kind,
            toolhead: Some(toolhead),
            from,
            to: to.clone(),
        });
        current.slots[toolhead.index()] = to;
    }
}

fn direct_ineligibility_message(eligibility: &DirectSpoolEligibility) -> String {
    match eligibility {
        DirectSpoolEligibility::Eligible { .. } => "Direct Spools is available.".into(),
        DirectSpoolEligibility::Ineligible { reason } => match reason {
            DirectIneligibility::TooManyEffectivePairs { count, maximum } => format!(
                "Direct Spools supports at most {maximum} effective material-color pairs; this scope uses {count}."
            ),
            DirectIneligibility::NoEffectivePairs => {
                "Direct Spools requires at least one effective material-color pair.".into()
            }
            DirectIneligibility::MissingCompatibleSpools { material } => format!(
                "Inventory does not contain enough available one-to-one spools for material {material:?}."
            ),
            DirectIneligibility::InvalidManualAssignment { message } => message.clone(),
        },
    }
}

fn mapping_order(left: &SourceToActualMapping, right: &SourceToActualMapping) -> Ordering {
    left.scope_id
        .cmp(&right.scope_id)
        .then_with(|| left.source_material.cmp(&right.source_material))
        .then_with(|| left.source_color.cmp(&right.source_color))
        .then_with(|| {
            left.source_requirement_ids
                .cmp(&right.source_requirement_ids)
        })
}

fn unit_warning(
    code: WarningCode,
    scope: &PrintScope,
    unit: &PrintableUnit,
    message: &str,
) -> PlanWarning {
    PlanWarning {
        code,
        scope_id: Some(scope.id.clone()),
        unit_id: Some(unit.id.clone()),
        message: message.into(),
    }
}

fn unit_error(
    code: ErrorCode,
    scope: &PrintScope,
    unit: &PrintableUnit,
    message: &str,
) -> PlanError {
    PlanError {
        code,
        scope_id: Some(scope.id.clone()),
        unit_id: Some(unit.id.clone()),
        message: message.into(),
    }
}

fn sort_warnings(warnings: &mut Vec<PlanWarning>) {
    warnings.sort_by(|left, right| {
        left.code
            .cmp(&right.code)
            .then_with(|| left.scope_id.cmp(&right.scope_id))
            .then_with(|| left.unit_id.cmp(&right.unit_id))
            .then_with(|| left.message.cmp(&right.message))
    });
    warnings.dedup();
}

fn sort_errors(errors: &mut Vec<PlanError>) {
    errors.sort_by(|left, right| {
        left.code
            .cmp(&right.code)
            .then_with(|| left.scope_id.cmp(&right.scope_id))
            .then_with(|| left.unit_id.cmp(&right.unit_id))
            .then_with(|| left.message.cmp(&right.message))
    });
    errors.dedup();
}
