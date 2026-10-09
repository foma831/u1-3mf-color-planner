use crate::filament_library::FilamentSpoolView;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use u1_application::{
    ApplicationError, CmyxGeometryContext, ExcludedSourceUnit, PreliminaryPlanOptions,
    ScopePlanningOverride, UnitPrinterOverride, UserCmyxCalibrationRecord,
    alternative_plate_candidates, analyze_and_plan_with_options, build_planning_input,
    canonical_partial_conversion_exclusions, preliminary_plan_limitations,
};
use u1_planner::{
    A1MiniConfig, CmyxFallbackApproval, CmyxRecipe, ColorConfidence, ColorStrategy,
    CurrentToolheadState, DedicatedSupportMaterial, DirectAssignmentRequest, DirectIneligibility,
    DirectSpoolEligibility, Estimate, FullSpectrumMode, Material, MaterialColorRequirement,
    MaterialRole, MaterialSubstitutionApproval, PlanningInput, PlanningResult, Printer,
    PrinterLoadout, PrinterPreference, RgbColor, ScopeStrategy, ScopeStrategyOptions,
    SetupActionKind, SetupPhase, Spool, SupportMaterialUsage, Toolhead, ToolheadSlotState, plan,
};
use u1_three_mf::{DetectedAdhesionMode, ProjectAnalysis, ProjectDialect};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct PlanningRequestView {
    default_strategy: Option<PlanningStrategyView>,
    confirmed_spools: Vec<ConfirmedSpoolInputView>,
    scope_overrides: Vec<ScopePlanningInputView>,
    unit_printer_overrides: Vec<UnitPrinterInputView>,
    current_loadout: Option<Vec<LoadedToolheadInputView>>,
    current_a1_spool_id: Option<String>,
    restore_cmy_after_direct: Option<bool>,
    #[serde(default)]
    allow_direct_palette_reduction: bool,
    #[serde(default)]
    allow_u1_cross_source_repacking: bool,
    dedicated_support_spool_id: Option<String>,
    #[serde(default)]
    dedicated_support_usage: SupportMaterialUsageView,
    a1_mini_enabled: bool,
    included_alternative_plate_ids: Vec<u32>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InitialPlanningIntentView {
    default_strategy: PlanningStrategyView,
    a1_mini_enabled: bool,
    /// Exact U1 starting state. `None` retains the legacy CMY+Grey defaults;
    /// an explicitly supplied empty array means every U1 toolhead is unknown.
    #[serde(default)]
    current_loadout: Option<Vec<LoadedToolheadInputView>>,
    /// Legacy T4-only input retained for saved v1 frontend state.
    current_t4_spool_id: Option<String>,
    current_a1_spool_id: Option<String>,
    #[serde(default)]
    allow_u1_cross_source_repacking: bool,
    dedicated_support_spool_id: Option<String>,
    #[serde(default)]
    dedicated_support_usage: SupportMaterialUsageView,
}

impl Default for InitialPlanningIntentView {
    fn default() -> Self {
        Self {
            default_strategy: PlanningStrategyView::Auto,
            a1_mini_enabled: false,
            current_loadout: None,
            current_t4_spool_id: None,
            current_a1_spool_id: None,
            allow_u1_cross_source_repacking: false,
            dedicated_support_spool_id: None,
            dedicated_support_usage: SupportMaterialUsageView::InterfaceOnly,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum SupportMaterialUsageView {
    #[default]
    InterfaceOnly,
    BodyAndInterface,
}

impl SupportMaterialUsageView {
    fn into_domain(self) -> SupportMaterialUsage {
        match self {
            Self::InterfaceOnly => SupportMaterialUsage::InterfaceOnly,
            Self::BodyAndInterface => SupportMaterialUsage::BodyAndInterface,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UnitPrinterInputView {
    source_unit_id: String,
    preference: PrinterPreferenceView,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum PrinterPreferenceView {
    Auto,
    U1,
    A1Mini,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfirmedSpoolInputView {
    id: String,
    name: String,
    #[serde(default)]
    color_name: String,
    hex: String,
    material: String,
    #[serde(default)]
    sku: String,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default)]
    color_basis: Option<String>,
    #[serde(default = "default_available")]
    available: bool,
}

fn default_available() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScopePlanningInputView {
    scope_id: String,
    strategy: PlanningStrategyView,
    #[serde(default)]
    assignments: Vec<DirectAssignmentInputView>,
    #[serde(default)]
    approved_color_fallbacks: Vec<ColorFallbackApprovalInputView>,
    #[serde(default)]
    material_substitutions: Vec<MaterialSubstitutionInputView>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ColorFallbackApprovalInputView {
    requirement_id: String,
    candidate_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MaterialSubstitutionInputView {
    requirement_id: String,
    candidate_id: String,
    source_material: String,
    target_material: String,
    #[serde(default)]
    acknowledged: bool,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum PlanningStrategyView {
    Auto,
    Cmyx,
    Direct,
}

impl PlanningStrategyView {
    fn into_scope_strategy(self) -> ScopeStrategy {
        match self {
            Self::Auto => ScopeStrategy::Auto,
            Self::Cmyx => ScopeStrategy::CmyxFullSpectrum,
            Self::Direct => ScopeStrategy::DirectSpools,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DirectAssignmentInputView {
    requirement_id: String,
    spool_id: String,
    #[serde(default)]
    toolhead: Option<String>,
    #[serde(default)]
    allow_material_substitution: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoadedToolheadInputView {
    toolhead: String,
    spool_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPlanView {
    summary: ProjectSummaryView,
    detected_adhesion: DetectedAdhesionPolicyView,
    alternative_plates: Vec<AlternativePlateView>,
    project_direct_palette: ProjectDirectPaletteView,
    scope_selections: Vec<ScopeSelectionView>,
    unit_printer_selections: Vec<UnitPrinterSelectionView>,
    color_resolutions: Vec<ColorResolutionView>,
    plates: Vec<PlatePlanView>,
    batches: Vec<BatchSegmentView>,
    t4_swap_count: u32,
    a1_spool_change_count: u32,
    blocking_errors: Vec<String>,
    global_warnings: Vec<String>,
    omitted_unit_count: usize,
    plan_ready: bool,
    partial_conversion: PartialConversionView,
    spools: Vec<FilamentSpoolView>,
    current_loadout: Vec<LoadedToolheadView>,
    current_a1_spool_id: Option<String>,
    planned_final_loadout: Vec<LoadedToolheadView>,
    planned_final_a1_spool_id: Option<String>,
    restore_cmy_by_default: bool,
    custom_direct_palettes_enabled: bool,
    u1_cross_source_repacking_enabled: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DetectedAdhesionPolicyView {
    mode: DetectedAdhesionMode,
    profile_name: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectDirectPaletteView {
    available: bool,
    /// Semantic pairs retain material role for CMY+X and per-row consent.
    effective_pair_count: usize,
    /// Physical Direct identities consume U1 toolheads; declared-profile role
    /// duplicates share one identity.
    direct_pair_count: usize,
    maximum_pair_count: usize,
    unavailable_reason: Option<String>,
    mappings: Vec<ProjectDirectPaletteMappingView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectDirectPaletteMappingView {
    id: String,
    physical_identity_id: String,
    source_material: String,
    source_role: &'static str,
    source_hex: String,
    source_slots: Vec<String>,
    source_profile_ids: Vec<String>,
    used_by: Vec<String>,
    references: Vec<ProjectDirectPaletteReferenceView>,
    current_cmyx_results: Vec<ProjectDirectCmyxResultView>,
    selected_spool_id: String,
    direct_toolhead: &'static str,
    material_substitution_acknowledged: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectDirectPaletteReferenceView {
    scope_id: String,
    scope_name: String,
    requirement_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectDirectCmyxResultView {
    scope_id: String,
    scope_name: String,
    recipe: String,
    predicted_hex: Option<String>,
    delta_e00: Option<f64>,
    confidence: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PartialConversionView {
    available: bool,
    exclusions: Vec<ExcludedSourceUnit>,
    reason: String,
}

pub struct NativePlanningOutcome {
    pub view: ProjectPlanView,
    pub input: PlanningInput,
    pub result: PlanningResult,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UnitPrinterSelectionView {
    source_unit_id: String,
    preference: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AlternativePlateView {
    id: u32,
    name: String,
    included: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScopeSelectionView {
    scope_id: String,
    strategy: &'static str,
    assignments: Vec<DirectAssignmentSelectionView>,
    approved_color_fallbacks: Vec<ColorFallbackApprovalView>,
    material_substitutions: Vec<MaterialSubstitutionView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DirectAssignmentSelectionView {
    requirement_id: String,
    spool_id: String,
    toolhead: Option<&'static str>,
    allow_material_substitution: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ColorFallbackApprovalView {
    requirement_id: String,
    candidate_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MaterialSubstitutionView {
    requirement_id: String,
    candidate_id: String,
    source_material: String,
    target_material: String,
    acknowledged: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ColorResolutionView {
    scope_id: String,
    scope_name: String,
    requirement_id: String,
    source_identity_key: String,
    candidate_id: String,
    source_material: String,
    source_hex: String,
    target_material: String,
    target_hex: String,
    predicted_hex: Option<String>,
    recipe: String,
    delta_e00: Option<f64>,
    confidence: &'static str,
    required_t4_spool_id: Option<String>,
    required_t4_name: Option<String>,
    required_t4_hex: Option<String>,
    color_approved: bool,
    material_approved: bool,
    requires_material_substitution: bool,
    can_add_dedicated_spool: bool,
    recommendation: String,
    palette_options: Vec<CmyxPaletteOptionView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CmyxPaletteOptionView {
    candidate_id: String,
    target_material: String,
    target_hex: String,
    predicted_hex: Option<String>,
    recipe: String,
    delta_e00: Option<f64>,
    confidence: &'static str,
    required_t4_spool_id: Option<String>,
    required_t4_name: Option<String>,
    required_t4_hex: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectSummaryView {
    file_name: String,
    source_hash: String,
    source_dialect: String,
    source_application: String,
    source_byte_size: u64,
    source_plate_count: usize,
    object_count: usize,
    instance_count: usize,
    part_count: usize,
    printable_part_count: usize,
    painted_part_count: usize,
    bounded_instance_count: usize,
    used_filament_count: usize,
    unused_filament_count: usize,
    alternative_plate_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LoadedToolheadView {
    toolhead: &'static str,
    spool_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchSegmentView {
    id: String,
    label: String,
    detail: String,
    strategy: &'static str,
    start_order: usize,
    end_order: usize,
    plate_count: usize,
    printer: &'static str,
    setup_actions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    t4_change: Option<T4ChangeView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct T4ChangeView {
    from_spool_id: Option<String>,
    from_spool_name: Option<String>,
    to_spool_id: Option<String>,
    to_spool_name: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlatePlanView {
    id: String,
    planning_status: &'static str,
    scope_id: String,
    scope_ids: Vec<String>,
    source_unit_ids: Vec<String>,
    order: usize,
    printer: &'static str,
    title: String,
    source: String,
    strategy: &'static str,
    object_count: usize,
    loadout_label: String,
    loadout_colors: Vec<String>,
    logical_color_count: usize,
    effective_pair_count: usize,
    direct_pair_count: usize,
    source_colors: Vec<SourceColorView>,
    recipe_summary: String,
    color_quality: &'static str,
    estimated_delta_e00: f64,
    material: String,
    tool_changes: ToolChangeView,
    t4_action: String,
    warnings: Vec<String>,
    is_fast_mono: bool,
    direct_eligible: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    direct_eligibility_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mappings: Option<Vec<DirectColorMappingView>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceColorView {
    source_slots: Vec<String>,
    source_material: String,
    source_hex: String,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ToolChangeView {
    Count(u64),
    RequiresSlicing(&'static str),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DirectColorMappingView {
    id: String,
    scope_id: String,
    physical_identity_id: String,
    inheritance_key: String,
    source_slot: String,
    source_name: String,
    source_hex: String,
    source_material: &'static str,
    used_by: String,
    cmy_recipe: String,
    cmy_predicted_hex: Option<String>,
    cmy_delta_e00: Option<f64>,
    cmy_confidence: &'static str,
    direct_toolhead: &'static str,
    selected_spool_id: String,
    dedicated_support: bool,
    material_substitution_acknowledged: bool,
}

impl PlanningRequestView {
    #[cfg(test)]
    fn into_options(self) -> Result<PreliminaryPlanOptions, String> {
        self.into_options_with_library(Vec::new())
    }

    #[cfg(test)]
    fn into_options_with_library(
        self,
        library_spools: Vec<Spool>,
    ) -> Result<PreliminaryPlanOptions, String> {
        self.into_options_with_backend_state(
            library_spools,
            Vec::new(),
            CmyxGeometryContext::default(),
        )
    }

    fn into_options_with_backend_state(
        self,
        library_spools: Vec<Spool>,
        confirmed_calibration_samples: Vec<UserCmyxCalibrationRecord>,
        cmyx_geometry_context: CmyxGeometryContext,
    ) -> Result<PreliminaryPlanOptions, String> {
        let PlanningRequestView {
            default_strategy,
            confirmed_spools,
            scope_overrides,
            unit_printer_overrides,
            current_loadout,
            current_a1_spool_id,
            restore_cmy_after_direct,
            allow_direct_palette_reduction,
            allow_u1_cross_source_repacking,
            dedicated_support_spool_id,
            dedicated_support_usage,
            a1_mini_enabled,
            included_alternative_plate_ids,
        } = self;
        let defaults = PreliminaryPlanOptions::default();
        let requested_spools = confirmed_spools
            .into_iter()
            .map(ConfirmedSpoolInputView::into_spool)
            .collect::<Result<Vec<_>, _>>()?;
        let confirmed_spools = merge_confirmed_spools(library_spools, requested_spools)?;
        let dedicated_support = dedicated_support_spool_id
            .map(|spool_id| spool_id.trim().to_owned())
            .filter(|spool_id| !spool_id.is_empty())
            .map(|spool_id| DedicatedSupportMaterial {
                spool_id,
                usage: dedicated_support_usage.into_domain(),
                toolhead: Toolhead::T4,
            });
        let scope_overrides = scope_overrides
            .into_iter()
            .map(ScopePlanningInputView::into_override)
            .collect::<Result<Vec<_>, _>>()?;
        let unit_printer_overrides = unit_printer_overrides
            .into_iter()
            .map(|printer_override| UnitPrinterOverride {
                source_unit_id: printer_override.source_unit_id,
                preference: match printer_override.preference {
                    PrinterPreferenceView::Auto => PrinterPreference::Auto,
                    PrinterPreferenceView::U1 => PrinterPreference::U1,
                    PrinterPreferenceView::A1Mini => PrinterPreference::A1Mini,
                },
            })
            .collect();
        let current_toolheads = match current_loadout {
            Some(loadout) => current_toolhead_request(loadout)?,
            None => defaults.current_toolheads,
        };
        let current_a1_spool_id = current_a1_spool_id
            .map(|spool_id| spool_id.trim().to_owned())
            .filter(|spool_id| !spool_id.is_empty());
        validate_cross_printer_current_loadout(&current_toolheads, current_a1_spool_id.as_deref())?;

        Ok(PreliminaryPlanOptions {
            current_toolheads,
            scope_strategy: default_strategy
                .map(PlanningStrategyView::into_scope_strategy)
                .unwrap_or(defaults.scope_strategy),
            restore_cmy_after_direct: restore_cmy_after_direct
                .unwrap_or(defaults.restore_cmy_after_direct),
            allow_direct_palette_reduction,
            allow_u1_cross_source_repacking,
            dedicated_support,
            confirmed_spools,
            scope_overrides,
            unit_printer_overrides,
            a1_mini: A1MiniConfig {
                enabled: a1_mini_enabled,
                current_spool_id: current_a1_spool_id,
                ..A1MiniConfig::default()
            },
            included_alternative_plate_ids,
            confirmed_calibration_samples,
            cmyx_geometry_context,
        })
    }
}

impl InitialPlanningIntentView {
    fn into_options_with_backend_state(
        self,
        library_spools: Vec<Spool>,
        confirmed_calibration_samples: Vec<UserCmyxCalibrationRecord>,
        cmyx_geometry_context: CmyxGeometryContext,
    ) -> Result<PreliminaryPlanOptions, ApplicationError> {
        let InitialPlanningIntentView {
            default_strategy,
            a1_mini_enabled,
            current_loadout,
            current_t4_spool_id,
            current_a1_spool_id,
            allow_u1_cross_source_repacking,
            dedicated_support_spool_id,
            dedicated_support_usage,
        } = self;
        let defaults = PreliminaryPlanOptions::default();
        let current_toolheads = match current_loadout {
            Some(loadout) => current_toolhead_request(loadout)
                .map_err(|message| ApplicationError::InvalidCurrentPrinterLoadout { message })?,
            None => {
                let mut current_toolheads = defaults.current_toolheads.clone();
                if let Some(spool_id) = normalized_optional_spool_id(current_t4_spool_id) {
                    current_toolheads.slots[Toolhead::T4.index()] =
                        ToolheadSlotState::Loaded(spool_id);
                }
                current_toolheads
            }
        };
        let current_a1_spool_id = normalized_optional_spool_id(current_a1_spool_id);
        let dedicated_support =
            normalized_optional_spool_id(dedicated_support_spool_id).map(|spool_id| {
                DedicatedSupportMaterial {
                    spool_id,
                    usage: dedicated_support_usage.into_domain(),
                    toolhead: Toolhead::T4,
                }
            });
        validate_cross_printer_current_loadout(&current_toolheads, current_a1_spool_id.as_deref())
            .map_err(|message| ApplicationError::InvalidCurrentPrinterLoadout { message })?;

        Ok(PreliminaryPlanOptions {
            current_toolheads,
            scope_strategy: default_strategy.into_scope_strategy(),
            allow_u1_cross_source_repacking,
            dedicated_support,
            confirmed_spools: library_spools,
            a1_mini: A1MiniConfig {
                enabled: a1_mini_enabled,
                current_spool_id: current_a1_spool_id,
                ..A1MiniConfig::default()
            },
            confirmed_calibration_samples,
            cmyx_geometry_context,
            ..defaults
        })
    }
}

fn normalized_optional_spool_id(spool_id: Option<String>) -> Option<String> {
    spool_id
        .map(|spool_id| spool_id.trim().to_owned())
        .filter(|spool_id| !spool_id.is_empty())
}

impl ConfirmedSpoolInputView {
    fn into_spool(self) -> Result<Spool, String> {
        let id = self.id.trim().to_owned();
        if id.is_empty() {
            return Err("A confirmed spool must have a non-empty ID.".to_owned());
        }
        if id.starts_with("source-") {
            return Err("Confirmed spool IDs beginning with 'source-' are reserved.".to_owned());
        }
        let color = parse_hex_color(&self.hex)?;
        let material = parse_spool_material(&self.material)?;
        let measured = self
            .color_basis
            .as_deref()
            .is_some_and(|basis| basis.eq_ignore_ascii_case("measured"));
        let profile_id = self
            .profile
            .filter(|profile| !profile.trim().is_empty())
            .map(|profile| profile.trim().to_owned());
        Ok(Spool {
            id,
            calibration_id: None,
            display_name: self.name.trim().to_owned(),
            color_name: (!self.color_name.trim().is_empty())
                .then(|| self.color_name.trim().to_owned()),
            material,
            nominal_color: color,
            measured_color: measured.then_some(color),
            sku: (!self.sku.trim().is_empty()).then(|| self.sku.trim().to_owned()),
            profile_id,
            available: self.available,
        })
    }
}

fn merge_confirmed_spools(
    library_spools: Vec<Spool>,
    requested_spools: Vec<Spool>,
) -> Result<Vec<Spool>, String> {
    let mut merged = BTreeMap::new();
    for spool in library_spools {
        if merged.insert(spool.id.clone(), spool).is_some() {
            return Err("The saved filament library contains duplicate spool IDs.".to_owned());
        }
    }
    let mut requested_ids = BTreeSet::new();
    for mut spool in requested_spools {
        if !requested_ids.insert(spool.id.clone()) {
            return Err(format!(
                "Confirmed spool ID '{}' is duplicated in the planning request.",
                spool.id
            ));
        }
        if let Some(persisted) = merged.get(&spool.id) {
            // Planning requests may refresh display/color fields, but they
            // cannot replace the physical batch identity owned by the native
            // Filament Library.
            spool.calibration_id = persisted.calibration_id.clone();
        }
        merged.insert(spool.id.clone(), spool);
    }
    Ok(merged.into_values().collect())
}

impl ScopePlanningInputView {
    fn into_override(self) -> Result<ScopePlanningOverride, String> {
        let ScopePlanningInputView {
            scope_id,
            strategy,
            assignments,
            approved_color_fallbacks,
            material_substitutions,
        } = self;
        let strategy = strategy.into_scope_strategy();
        let direct_assignments = assignments
            .into_iter()
            .map(|assignment| {
                Ok(DirectAssignmentRequest {
                    requirement_id: assignment.requirement_id,
                    spool_id: assignment.spool_id,
                    toolhead: assignment
                        .toolhead
                        .as_deref()
                        .map(parse_toolhead)
                        .transpose()?,
                    allow_material_substitution: assignment.allow_material_substitution,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let approved_cmyx_fallbacks = approved_color_fallbacks
            .into_iter()
            .map(|approval| CmyxFallbackApproval {
                requirement_id: approval.requirement_id,
                candidate_id: approval.candidate_id,
            })
            .collect();
        let approved_material_substitutions = material_substitutions
            .into_iter()
            .map(|approval| {
                Ok(MaterialSubstitutionApproval {
                    requirement_id: approval.requirement_id,
                    candidate_id: approval.candidate_id,
                    source_material: parse_source_material(&approval.source_material)?,
                    target_material: parse_spool_material(&approval.target_material)?,
                    acknowledged: approval.acknowledged,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(ScopePlanningOverride {
            scope_id,
            strategy: Some(strategy),
            direct_assignments,
            approved_cmyx_fallbacks,
            approved_material_substitutions,
        })
    }
}

fn current_toolhead_request(
    loadout: Vec<LoadedToolheadInputView>,
) -> Result<CurrentToolheadState, String> {
    let mut slots = std::array::from_fn(|_| ToolheadSlotState::Unknown);
    let mut seen = BTreeSet::new();
    let mut seen_spools = BTreeSet::new();
    for loaded in loadout {
        let toolhead = parse_toolhead(&loaded.toolhead)?;
        let spool_id = loaded.spool_id.trim().to_owned();
        if !seen.insert(toolhead) {
            return Err(format!(
                "Current loadout lists {} more than once.",
                toolhead_name(toolhead)
            ));
        }
        if spool_id.is_empty() {
            return Err(format!(
                "Current {} spool ID cannot be empty.",
                toolhead_name(toolhead)
            ));
        }
        if !seen_spools.insert(spool_id.clone()) {
            return Err(format!(
                "Physical spool '{}' cannot be loaded in more than one toolhead.",
                spool_id
            ));
        }
        slots[toolhead.index()] = ToolheadSlotState::Loaded(spool_id);
    }
    Ok(CurrentToolheadState { slots })
}

fn validate_cross_printer_current_loadout(
    current_toolheads: &CurrentToolheadState,
    current_a1_spool_id: Option<&str>,
) -> Result<(), String> {
    let Some(current_a1_spool_id) = current_a1_spool_id else {
        return Ok(());
    };
    if current_toolheads.slots.iter().any(|slot| match slot {
        ToolheadSlotState::Loaded(spool_id) => spool_id == current_a1_spool_id,
        ToolheadSlotState::Unknown | ToolheadSlotState::Empty => false,
    }) {
        return Err(format!(
            "Physical spool '{current_a1_spool_id}' cannot be loaded on the U1 and A1 mini at the same time."
        ));
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

fn parse_spool_material(value: &str) -> Result<Material, String> {
    match value.trim().to_ascii_uppercase().as_str() {
        "PLA" => Ok(Material::Pla),
        "PETG" => Ok(Material::Petg),
        "PVA" => Ok(Material::Pva),
        _ => Err(format!(
            "Unsupported spool material '{value}'; the desktop inventory currently accepts PLA, PETG, or PVA."
        )),
    }
}

/// Parses the source side of an explicit material-substitution decision.
///
/// Unlike a physical inventory entry, this value is an exact acknowledgement
/// of source metadata already emitted by the backend. Keeping the full domain
/// here lets an ABS/ASA/TPU (or vendor-specific) source explicitly approve a
/// supported PLA/PETG target without pretending that an unsupported spool was
/// added to the desktop inventory.
fn parse_source_material(value: &str) -> Result<Material, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("A material-substitution source material cannot be empty.".to_owned());
    }
    match trimmed.to_ascii_uppercase().as_str() {
        "PLA" => Ok(Material::Pla),
        "PETG" | "PET" => Ok(Material::Petg),
        "PVA" => Ok(Material::Pva),
        "ABS" => Ok(Material::Abs),
        "ASA" => Ok(Material::Asa),
        "TPU" => Ok(Material::Tpu),
        _ => Ok(Material::Other(trimmed.to_owned())),
    }
}

fn parse_toolhead(value: &str) -> Result<Toolhead, String> {
    match value.trim().to_ascii_uppercase().as_str() {
        "T1" => Ok(Toolhead::T1),
        "T2" => Ok(Toolhead::T2),
        "T3" => Ok(Toolhead::T3),
        "T4" => Ok(Toolhead::T4),
        _ => Err(format!("Unknown U1 toolhead '{value}'.")),
    }
}

#[cfg(test)]
pub fn analyze_project_data(
    path: &str,
) -> Result<(ProjectAnalysis, ProjectPlanView), ApplicationError> {
    analyze_project_data_with_library(path, Vec::new())
}

#[cfg(test)]
pub fn analyze_project_data_with_library(
    path: &str,
    library_spools: Vec<Spool>,
) -> Result<(ProjectAnalysis, ProjectPlanView), ApplicationError> {
    let (analysis, outcome) = analyze_native_project_data_with_library(path, library_spools)?;
    Ok((analysis, outcome.view))
}

#[cfg(test)]
pub fn analyze_native_project_data_with_library(
    path: &str,
    library_spools: Vec<Spool>,
) -> Result<(ProjectAnalysis, NativePlanningOutcome), ApplicationError> {
    analyze_native_project_data_with_backend_state(
        path,
        InitialPlanningIntentView::default(),
        library_spools,
        Vec::new(),
        CmyxGeometryContext::default(),
    )
}

pub fn analyze_native_project_data_with_backend_state(
    path: &str,
    planning_intent: InitialPlanningIntentView,
    library_spools: Vec<Spool>,
    confirmed_calibration_samples: Vec<UserCmyxCalibrationRecord>,
    cmyx_geometry_context: CmyxGeometryContext,
) -> Result<(ProjectAnalysis, NativePlanningOutcome), ApplicationError> {
    let options = planning_intent.into_options_with_backend_state(
        library_spools,
        confirmed_calibration_samples,
        cmyx_geometry_context,
    )?;
    let report = analyze_and_plan_with_options(path, &options)?;
    let input = report.planning_input;
    let view = project_plan_view(
        Path::new(path),
        &report.analysis,
        &input,
        &report.plan,
        &report.limitations,
        &options.included_alternative_plate_ids,
    );
    Ok((
        report.analysis,
        NativePlanningOutcome {
            view,
            input,
            result: report.plan,
        },
    ))
}

#[cfg(test)]
pub fn replan_project_view(
    path: &str,
    analysis: &ProjectAnalysis,
    request: PlanningRequestView,
) -> Result<ProjectPlanView, String> {
    replan_project_view_with_library(path, analysis, request, Vec::new())
}

#[cfg(test)]
pub fn replan_project_view_with_library(
    path: &str,
    analysis: &ProjectAnalysis,
    request: PlanningRequestView,
    library_spools: Vec<Spool>,
) -> Result<ProjectPlanView, String> {
    replan_native_project_view_with_library(path, analysis, request, library_spools)
        .map(|outcome| outcome.view)
}

#[cfg(test)]
pub fn replan_native_project_view_with_library(
    path: &str,
    analysis: &ProjectAnalysis,
    request: PlanningRequestView,
    library_spools: Vec<Spool>,
) -> Result<NativePlanningOutcome, String> {
    replan_native_project_view_with_backend_state(
        path,
        analysis,
        request,
        library_spools,
        Vec::new(),
        CmyxGeometryContext::default(),
    )
}

pub fn replan_native_project_view_with_backend_state(
    path: &str,
    analysis: &ProjectAnalysis,
    request: PlanningRequestView,
    library_spools: Vec<Spool>,
    confirmed_calibration_samples: Vec<UserCmyxCalibrationRecord>,
    cmyx_geometry_context: CmyxGeometryContext,
) -> Result<NativePlanningOutcome, String> {
    let options = request.into_options_with_backend_state(
        library_spools,
        confirmed_calibration_samples,
        cmyx_geometry_context,
    )?;
    let input = build_planning_input(analysis, &options).map_err(|error| error.to_string())?;
    let result = plan(&input);
    let view = project_plan_view(
        Path::new(path),
        analysis,
        &input,
        &result,
        &preliminary_plan_limitations(),
        &options.included_alternative_plate_ids,
    );
    Ok(NativePlanningOutcome {
        view,
        input,
        result,
    })
}

fn project_plan_view(
    path: &Path,
    analysis: &ProjectAnalysis,
    input: &PlanningInput,
    result: &PlanningResult,
    limitations: &[String],
    included_alternative_plate_ids: &[u32],
) -> ProjectPlanView {
    let inventory = input
        .inventory
        .iter()
        .map(|spool| (spool.id.as_str(), spool))
        .collect::<BTreeMap<_, _>>();
    let scopes = input
        .scopes
        .iter()
        .map(|scope| (scope.id.as_str(), scope))
        .collect::<BTreeMap<_, _>>();
    let options = result
        .scope_options
        .iter()
        .map(|option| (option.scope_id.as_str(), option))
        .collect::<BTreeMap<_, _>>();
    let batches_by_plate = result
        .batches
        .iter()
        .flat_map(|batch| {
            batch
                .plate_ids
                .iter()
                .map(move |plate| (plate.as_str(), batch))
        })
        .collect::<BTreeMap<_, _>>();
    let result_plates_by_id = result
        .plates
        .iter()
        .map(|plate| (plate.id.as_str(), plate))
        .collect::<BTreeMap<_, _>>();
    let mut scheduled_plate_ids = BTreeSet::new();
    let mut scheduled_plates = result
        .batches
        .iter()
        .flat_map(|batch| batch.plate_ids.iter())
        .filter_map(|plate_id| {
            let plate = result_plates_by_id.get(plate_id.as_str()).copied()?;
            scheduled_plate_ids.insert(plate_id.as_str());
            Some(plate)
        })
        .collect::<Vec<_>>();
    scheduled_plates.extend(
        result
            .plates
            .iter()
            .filter(|plate| !scheduled_plate_ids.contains(plate.id.as_str())),
    );

    let mut plates = scheduled_plates
        .iter()
        .enumerate()
        .filter_map(|(index, plate)| {
            let job = result.jobs.iter().find(|job| job.id == plate.job_id)?;
            let scope_names = job
                .scope_ids
                .iter()
                .filter_map(|id| {
                    scopes
                        .get(id.as_str())
                        .map(|scope| scope.display_name.as_str())
                })
                .collect::<Vec<_>>();
            let title_base = if scope_names.is_empty() {
                "Planned objects".to_owned()
            } else {
                scope_names.join(" + ")
            };
            let direct = direct_status(&job.scope_ids, &options);
            let mappings = color_mappings(job, &options, &scopes);
            let deltas = job
                .color_mappings
                .iter()
                .filter_map(|mapping| mapping.delta_e00.or(mapping.cmyx_comparison.delta_e00))
                .collect::<Vec<_>>();
            let average_delta = if deltas.is_empty() {
                0.0
            } else {
                deltas.iter().sum::<f64>() / deltas.len() as f64
            };
            let mut warnings = result
                .warnings
                .iter()
                .filter(|warning| {
                    warning
                        .unit_id
                        .as_ref()
                        .is_some_and(|id| plate.units.iter().any(|unit| unit.unit_id == *id))
                        || warning
                            .scope_id
                            .as_ref()
                            .is_some_and(|id| job.scope_ids.contains(id))
                })
                .map(|warning| warning.message.clone())
                .collect::<Vec<_>>();
            if !plate.individual_bounds_validated {
                warnings
                    .push("Object bounds and final packing still require validation.".to_owned());
            }
            warnings.sort();
            warnings.dedup();

            let loadout_colors = loadout_spool_ids(&job.loadout)
                .into_iter()
                .filter_map(|id| {
                    inventory
                        .get(id)
                        .map(|spool| color_hex(spool.actual_color()))
                })
                .collect::<Vec<_>>();
            let batch = batches_by_plate.get(plate.id.as_str()).copied();
            let source_colors = source_colors_from_mappings(&job.color_mappings);
            let effective_pair_count = job
                .scope_ids
                .iter()
                .filter_map(|scope_id| options.get(scope_id.as_str()))
                .map(|option| option.effective_pair_count)
                .sum::<usize>()
                .max(job.color_mappings.len());
            let direct_pair_count = job
                .scope_ids
                .iter()
                .filter_map(|scope_id| options.get(scope_id.as_str()))
                .map(|option| option.direct_pair_count)
                .sum::<usize>();
            Some(PlatePlanView {
                id: plate.id.clone(),
                planning_status: "printable",
                scope_id: job.scope_ids.first().cloned().unwrap_or_default(),
                scope_ids: job.scope_ids.clone(),
                source_unit_ids: plate
                    .units
                    .iter()
                    .filter_map(|unit_ref| {
                        scopes
                            .get(unit_ref.scope_id.as_str())
                            .and_then(|scope| {
                                scope.units.iter().find(|unit| unit.id == unit_ref.unit_id)
                            })
                            .map(|unit| unit.source_unit_id.clone())
                    })
                    .collect(),
                order: index + 1,
                printer: printer_name(job.printer),
                title: format!("{title_base} — Plate {:02}", index + 1),
                source: scope_names.join(", "),
                strategy: strategy_name(job.strategy),
                object_count: plate.units.len(),
                loadout_label: loadout_label(&job.loadout, &inventory),
                loadout_colors,
                logical_color_count: source_colors.len(),
                effective_pair_count,
                direct_pair_count,
                source_colors,
                recipe_summary: recipe_summary(job.strategy, &job.color_mappings),
                color_quality: quality(average_delta),
                estimated_delta_e00: round_one(average_delta),
                material: material_summary(&job.printable_materials),
                tool_changes: match job.estimated_tool_changes {
                    Estimate::Estimated(count) => ToolChangeView::Count(count),
                    Estimate::RequiresSlicing => {
                        ToolChangeView::RequiresSlicing("Requires slicing")
                    }
                },
                t4_action: t4_action(batch, &inventory),
                warnings,
                is_fast_mono: job.fast_mono,
                direct_eligible: direct.0,
                direct_eligibility_reason: direct.1,
                mappings,
            })
        })
        .collect::<Vec<_>>();

    let planned_units = result
        .plates
        .iter()
        .flat_map(|plate| &plate.units)
        .map(|unit| (unit.scope_id.as_str(), unit.unit_id.as_str()))
        .collect::<BTreeSet<_>>();
    let mut omitted_unit_count = 0_usize;
    // Every hard error remains visible, including partially planned scopes.
    // This prevents a valid sibling unit from hiding an omitted object.
    for scope in &input.scopes {
        let errors = result
            .errors
            .iter()
            .filter(|error| error.scope_id.as_deref() == Some(scope.id.as_str()))
            .map(|error| error.message.clone())
            .collect::<Vec<_>>();
        if errors.is_empty() {
            continue;
        }
        let omitted_units = scope
            .units
            .iter()
            .filter(|unit| !planned_units.contains(&(scope.id.as_str(), unit.id.as_str())))
            .collect::<Vec<_>>();
        let source_unit_ids = omitted_units
            .iter()
            .map(|unit| unit.source_unit_id.clone())
            .collect::<Vec<_>>();
        let omitted_requirement_ids = omitted_units
            .iter()
            .flat_map(|unit| unit.requirement_ids.iter().map(String::as_str))
            .collect::<BTreeSet<_>>();
        let source_colors =
            source_colors_from_requirements(scope.requirements.iter().filter(|requirement| {
                omitted_requirement_ids.is_empty()
                    || omitted_requirement_ids.contains(requirement.id.as_str())
            }));
        let omitted_units = omitted_units.len();
        omitted_unit_count = omitted_unit_count.saturating_add(omitted_units);
        let direct = direct_status(std::slice::from_ref(&scope.id), &options);
        let option = options.get(scope.id.as_str()).copied();
        let mappings = unresolved_mappings(scope, option);
        let order = plates.len() + 1;
        plates.push(PlatePlanView {
            id: format!("blocked-{}", scope.id),
            planning_status: "blocked",
            scope_id: scope.id.clone(),
            scope_ids: vec![scope.id.clone()],
            source_unit_ids,
            order,
            printer: "U1",
            title: if omitted_units > 0 {
                format!("{} — omitted units", scope.display_name)
            } else {
                format!("{} — blocked", scope.display_name)
            },
            source: scope.display_name.clone(),
            strategy: match option.map(|option| option.selected_strategy) {
                Some(u1_planner::ScopeStrategy::DirectSpools) => "direct",
                _ => "cmyx",
            },
            object_count: omitted_units,
            loadout_label: "Manual review".to_owned(),
            loadout_colors: Vec::new(),
            logical_color_count: source_colors.len(),
            effective_pair_count: option.map_or(scope.requirements.len(), |option| {
                option.effective_pair_count
            }),
            direct_pair_count: option
                .map_or(scope.requirements.len(), |option| option.direct_pair_count),
            source_colors,
            recipe_summary: "Unresolved".to_owned(),
            color_quality: "Review",
            estimated_delta_e00: 0.0,
            material: material_summary(
                &scope
                    .requirements
                    .iter()
                    .map(|requirement| requirement.material.clone())
                    .collect::<Vec<_>>(),
            ),
            tool_changes: ToolChangeView::RequiresSlicing("Requires slicing"),
            t4_action: "Resolve errors".to_owned(),
            warnings: errors,
            is_fast_mono: false,
            direct_eligible: direct.0,
            direct_eligibility_reason: direct.1,
            mappings,
        });
    }

    let mut blocking_errors = result
        .errors
        .iter()
        .map(|error| error.message.clone())
        .collect::<Vec<_>>();
    blocking_errors.sort();
    blocking_errors.dedup();
    let mut global_warnings = result
        .warnings
        .iter()
        .filter(|warning| warning.scope_id.is_none() && warning.unit_id.is_none())
        .map(|warning| warning.message.clone())
        .chain(limitations.iter().cloned())
        .chain(
            analysis
                .warnings
                .iter()
                .map(|warning| warning.message.clone()),
        )
        .collect::<Vec<_>>();
    global_warnings.sort();
    global_warnings.dedup();

    let batches = batch_views(result, &plates, &inventory);
    let included_alternative_plate_ids = included_alternative_plate_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let alternative_plates = alternative_plate_candidates(analysis)
        .into_iter()
        .map(|plate| AlternativePlateView {
            included: included_alternative_plate_ids.contains(&plate.id),
            id: plate.id,
            name: plate.name,
        })
        .collect::<Vec<_>>();
    let color_resolutions = color_resolutions(&input.scopes, &inventory);
    let unit_printer_selections = input
        .scopes
        .iter()
        .flat_map(|scope| scope.units.iter())
        .map(|unit| {
            (
                unit.source_unit_id.clone(),
                preference_name(unit.printer_preference),
            )
        })
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .map(|(source_unit_id, preference)| UnitPrinterSelectionView {
            source_unit_id,
            preference,
        })
        .collect();
    let partial_conversion = if result.has_hard_errors() {
        match canonical_partial_conversion_exclusions(input, result) {
            Ok(exclusions) if !exclusions.is_empty() => PartialConversionView {
                available: true,
                reason: format!(
                    "{} source unit(s) can be explicitly excluded while the remaining valid jobs are converted.",
                    exclusions.len()
                ),
                exclusions,
            },
            Ok(_) => PartialConversionView {
                available: false,
                exclusions: Vec::new(),
                reason: "No backend-approved source-unit exclusions are available.".to_owned(),
            },
            Err(error) => PartialConversionView {
                available: false,
                exclusions: Vec::new(),
                reason: format!("Valid jobs cannot be isolated safely from this plan: {error}"),
            },
        }
    } else {
        PartialConversionView {
            available: false,
            exclusions: Vec::new(),
            reason: if omitted_unit_count == 0 {
                "The complete plan can be converted without exclusions.".to_owned()
            } else {
                "Omitted units have no backend-authored blocking error and cannot be excluded safely."
                    .to_owned()
            },
        }
    };
    let planned_final_a1 = planned_final_a1_spool_id(
        &result.batches,
        input.config.a1_mini.current_spool_id.as_deref(),
    );
    let planned_final_loadout =
        planned_final_u1_loadout(&result.final_toolheads, planned_final_a1.as_deref());

    ProjectPlanView {
        summary: ProjectSummaryView {
            file_name: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Selected project.3mf")
                .to_owned(),
            source_hash: format!("sha256:{}", analysis.input.sha256),
            source_dialect: dialect_name(analysis.source.dialect).to_owned(),
            source_application: source_application_label(analysis),
            source_byte_size: analysis.input.byte_size,
            source_plate_count: analysis.summary.plate_count,
            object_count: analysis.summary.object_count,
            instance_count: analysis.summary.instance_count,
            part_count: analysis.summary.part_count,
            printable_part_count: analysis.summary.printable_part_count,
            painted_part_count: analysis
                .objects
                .iter()
                .flat_map(|object| &object.parts)
                .filter(|part| !part.painted_slots.is_empty())
                .count(),
            bounded_instance_count: analysis.summary.bounded_instance_count,
            used_filament_count: analysis.summary.used_filament_count,
            unused_filament_count: analysis
                .filaments
                .iter()
                .filter(|filament| !filament.used)
                .count(),
            alternative_plate_count: alternative_plates.len(),
        },
        detected_adhesion: DetectedAdhesionPolicyView {
            mode: analysis.process.adhesion.mode,
            profile_name: analysis.process.adhesion.profile_name.clone(),
        },
        alternative_plates,
        project_direct_palette: project_direct_palette(&input.scopes, &options),
        scope_selections: scope_selections(&input.scopes),
        unit_printer_selections,
        color_resolutions,
        plates,
        batches,
        t4_swap_count: result.t4_swap_count,
        a1_spool_change_count: result.a1_spool_change_count,
        plan_ready: blocking_errors.is_empty() && omitted_unit_count == 0,
        partial_conversion,
        blocking_errors,
        global_warnings,
        omitted_unit_count,
        spools: input.inventory.iter().filter_map(spool_view).collect(),
        current_loadout: current_loadout(input),
        current_a1_spool_id: input.config.a1_mini.current_spool_id.clone(),
        planned_final_loadout,
        planned_final_a1_spool_id: planned_final_a1,
        restore_cmy_by_default: input.config.restore_cmy_after_direct,
        custom_direct_palettes_enabled: input.config.allow_direct_palette_reduction,
        u1_cross_source_repacking_enabled: input.config.allow_u1_cross_source_repacking,
    }
}

fn batch_views(
    result: &PlanningResult,
    plates: &[PlatePlanView],
    inventory: &BTreeMap<&str, &Spool>,
) -> Vec<BatchSegmentView> {
    let order_by_plate = plates
        .iter()
        .map(|plate| (plate.id.as_str(), plate.order))
        .collect::<BTreeMap<_, _>>();

    result
        .batches
        .iter()
        .filter_map(|batch| {
            let orders = batch
                .plate_ids
                .iter()
                .filter_map(|plate_id| order_by_plate.get(plate_id.as_str()).copied())
                .collect::<Vec<_>>();
            let start_order = orders.iter().min().copied()?;
            let end_order = orders.iter().max().copied()?;
            let loadout = loadout_label(&batch.loadout, inventory);
            let (label, detail) = match batch.strategy {
                ColorStrategy::CmyxFullSpectrum => (loadout, "CMY+X Full Spectrum".to_owned()),
                ColorStrategy::CmyxSolid => (loadout, "CMY+X Solid".to_owned()),
                ColorStrategy::DirectSpools => (
                    "Direct Spools".to_owned(),
                    "Full T1–T4 setup boundary".to_owned(),
                ),
                ColorStrategy::A1Mono => ("A1 Mono".to_owned(), loadout),
            };
            Some(BatchSegmentView {
                id: batch.id.clone(),
                label,
                detail,
                strategy: strategy_name(batch.strategy),
                start_order,
                end_order,
                plate_count: orders.len(),
                printer: printer_name(batch.printer),
                setup_actions: batch
                    .setup_actions
                    .iter()
                    .map(|action| setup_action_description(action, inventory))
                    .collect(),
                t4_change: t4_change(batch, inventory),
            })
        })
        .collect()
}

fn t4_change(
    batch: &u1_planner::PlannedBatch,
    inventory: &BTreeMap<&str, &Spool>,
) -> Option<T4ChangeView> {
    let actions = batch
        .setup_actions
        .iter()
        .filter(|action| {
            action.phase == SetupPhase::BeforeBatch
                && action.toolhead == Some(Toolhead::T4)
                && action.kind != SetupActionKind::Keep
        })
        .collect::<Vec<_>>();
    let first = actions.first()?;
    let last = actions.last()?;
    let from_spool_id = loaded_spool_id(&first.from);
    let to_spool_id = loaded_spool_id(&last.to);
    if from_spool_id == to_spool_id {
        return None;
    }
    Some(T4ChangeView {
        from_spool_name: spool_name(from_spool_id, inventory),
        from_spool_id: from_spool_id.map(str::to_owned),
        to_spool_name: spool_name(to_spool_id, inventory),
        to_spool_id: to_spool_id.map(str::to_owned),
    })
}

fn loaded_spool_id(state: &ToolheadSlotState) -> Option<&str> {
    match state {
        ToolheadSlotState::Loaded(spool_id) => Some(spool_id),
        ToolheadSlotState::Unknown | ToolheadSlotState::Empty => None,
    }
}

fn spool_name(spool_id: Option<&str>, inventory: &BTreeMap<&str, &Spool>) -> Option<String> {
    spool_id.map(|id| {
        inventory
            .get(id)
            .map_or_else(|| id.to_owned(), |spool| spool.display_name.clone())
    })
}

pub(crate) fn setup_action_description(
    action: &u1_planner::SetupAction,
    inventory: &BTreeMap<&str, &Spool>,
) -> String {
    let phase = match action.phase {
        SetupPhase::BeforeBatch => "Before batch",
        SetupPhase::AfterBatch => "After batch",
    };
    let toolhead = action.toolhead.map(toolhead_name).unwrap_or("A1");
    let from = slot_state_label(&action.from, inventory);
    let to = slot_state_label(&action.to, inventory);
    let instruction = match action.kind {
        SetupActionKind::Keep => format!("keep {to}"),
        SetupActionKind::Unload => format!("unload {from}"),
        SetupActionKind::Load => format!("load {to}"),
        SetupActionKind::Restore => format!("restore {to} (replace {from})"),
    };
    format!("{phase}: {toolhead} — {instruction}")
}

fn slot_state_label(state: &ToolheadSlotState, inventory: &BTreeMap<&str, &Spool>) -> String {
    match state {
        ToolheadSlotState::Unknown => "unknown spool".to_owned(),
        ToolheadSlotState::Empty => "empty".to_owned(),
        ToolheadSlotState::Loaded(spool_id) => inventory
            .get(spool_id.as_str())
            .map_or_else(|| spool_id.clone(), |spool| spool.display_name.clone()),
    }
}

fn spool_view(spool: &Spool) -> Option<FilamentSpoolView> {
    if spool.id.starts_with("source-") {
        return None;
    }
    FilamentSpoolView::from_domain(spool)
}

fn current_loadout(input: &PlanningInput) -> Vec<LoadedToolheadView> {
    loaded_toolhead_views(&input.current_toolheads)
}

fn loaded_toolhead_views(current: &CurrentToolheadState) -> Vec<LoadedToolheadView> {
    current
        .slots
        .iter()
        .enumerate()
        .filter_map(|(index, slot)| match slot {
            ToolheadSlotState::Loaded(spool_id) => Some(LoadedToolheadView {
                toolhead: toolhead_name(Toolhead::ALL[index]),
                spool_id: spool_id.clone(),
            }),
            ToolheadSlotState::Unknown | ToolheadSlotState::Empty => None,
        })
        .collect()
}

fn planned_final_u1_loadout(
    final_toolheads: &CurrentToolheadState,
    planned_final_a1_spool_id: Option<&str>,
) -> Vec<LoadedToolheadView> {
    loaded_toolhead_views(final_toolheads)
        .into_iter()
        .filter(|loaded| Some(loaded.spool_id.as_str()) != planned_final_a1_spool_id)
        .collect()
}

fn planned_final_a1_spool_id(
    batches: &[u1_planner::PlannedBatch],
    current_a1_spool_id: Option<&str>,
) -> Option<String> {
    batches
        .iter()
        .rev()
        .find_map(|batch| match &batch.loadout {
            PrinterLoadout::A1Mini { spool_id } => Some(spool_id.clone()),
            PrinterLoadout::U1 { .. } => None,
        })
        .or_else(|| current_a1_spool_id.map(str::to_owned))
}

fn scope_selections(scopes: &[u1_planner::PrintScope]) -> Vec<ScopeSelectionView> {
    scopes
        .iter()
        .map(|scope| ScopeSelectionView {
            scope_id: scope.id.clone(),
            strategy: match scope.strategy {
                ScopeStrategy::DirectSpools => "direct",
                ScopeStrategy::Auto | ScopeStrategy::CmyxFullSpectrum => "cmyx",
            },
            assignments: scope
                .direct_assignments
                .iter()
                .map(|assignment| DirectAssignmentSelectionView {
                    requirement_id: assignment.requirement_id.clone(),
                    spool_id: assignment.spool_id.clone(),
                    toolhead: assignment.toolhead.map(toolhead_name),
                    allow_material_substitution: assignment.allow_material_substitution,
                })
                .collect(),
            approved_color_fallbacks: scope
                .approved_cmyx_fallbacks
                .iter()
                .map(|approval| ColorFallbackApprovalView {
                    requirement_id: approval.requirement_id.clone(),
                    candidate_id: approval.candidate_id.clone(),
                })
                .collect(),
            material_substitutions: scope
                .approved_material_substitutions
                .iter()
                .map(|approval| MaterialSubstitutionView {
                    requirement_id: approval.requirement_id.clone(),
                    candidate_id: approval.candidate_id.clone(),
                    source_material: material_name(&approval.source_material).to_owned(),
                    target_material: material_name(&approval.target_material).to_owned(),
                    acknowledged: approval.acknowledged,
                })
                .collect(),
        })
        .collect()
}

#[derive(Debug)]
struct ProjectDirectPairAccumulator<'a> {
    requirement: &'a MaterialColorRequirement,
    physical_identity: DirectPhysicalMappingIdentity,
    source_slots: BTreeSet<String>,
    source_profile_ids: BTreeSet<String>,
    used_by: BTreeSet<String>,
    references: BTreeMap<String, ProjectDirectReferenceAccumulator>,
    current_cmyx_results: Vec<ProjectDirectCmyxResultView>,
    resolved_assignments: Vec<(String, Toolhead, bool)>,
    unresolved_reference_count: usize,
}

#[derive(Debug)]
struct ProjectDirectReferenceAccumulator {
    scope_name: String,
    requirement_ids: BTreeSet<String>,
}

fn project_direct_palette(
    scopes: &[u1_planner::PrintScope],
    options: &BTreeMap<&str, &ScopeStrategyOptions>,
) -> ProjectDirectPaletteView {
    const MAXIMUM_PAIR_COUNT: usize = 4;

    let mut grouped = BTreeMap::<FallbackMappingIdentity, ProjectDirectPairAccumulator<'_>>::new();
    let mut effective_inventory_complete = true;
    for scope in scopes {
        let used_requirement_ids = scope
            .units
            .iter()
            .flat_map(|unit| unit.requirement_ids.iter().cloned())
            .collect::<BTreeSet<_>>();
        let mut requirements_by_pair =
            BTreeMap::<FallbackMappingIdentity, Vec<&MaterialColorRequirement>>::new();
        for requirement in &scope.requirements {
            if used_requirement_ids.contains(&requirement.id) {
                requirements_by_pair
                    .entry(fallback_mapping_identity(requirement))
                    .or_default()
                    .push(requirement);
            }
        }
        let direct_identity_count = requirements_by_pair
            .values()
            .filter_map(|requirements| requirements.first().copied())
            .map(direct_physical_mapping_identity)
            .collect::<BTreeSet<_>>()
            .len();
        let option = options.get(scope.id.as_str());
        if option.map(|option| option.effective_pair_count) != Some(requirements_by_pair.len())
            || option.map(|option| option.direct_pair_count) != Some(direct_identity_count)
        {
            effective_inventory_complete = false;
        }

        for (identity, mut requirements) in requirements_by_pair {
            requirements.sort_by(|left, right| left.id.cmp(&right.id));
            let canonical = requirements[0];
            let requirement_ids = requirements
                .iter()
                .map(|requirement| requirement.id.clone())
                .collect::<BTreeSet<_>>();
            let explicit_assignment = scope
                .direct_assignments
                .iter()
                .find(|assignment| requirement_ids.contains(&assignment.requirement_id));
            let automatic_assignment =
                options
                    .get(scope.id.as_str())
                    .and_then(|option| match &option.direct_spools {
                        DirectSpoolEligibility::Eligible { assignments } => {
                            assignments.iter().find(|assignment| {
                                assignment
                                    .source_requirement_ids
                                    .iter()
                                    .any(|id| requirement_ids.contains(id))
                            })
                        }
                        DirectSpoolEligibility::Ineligible { .. } => None,
                    });
            let resolved_assignment = explicit_assignment
                .map(|assignment| {
                    (
                        assignment.spool_id.clone(),
                        assignment.toolhead.unwrap_or(Toolhead::T1),
                        assignment.allow_material_substitution,
                    )
                })
                .or_else(|| {
                    automatic_assignment
                        .map(|assignment| (assignment.spool_id.clone(), assignment.toolhead, false))
                });

            let accumulator =
                grouped
                    .entry(identity)
                    .or_insert_with(|| ProjectDirectPairAccumulator {
                        requirement: canonical,
                        physical_identity: direct_physical_mapping_identity(canonical),
                        source_slots: BTreeSet::new(),
                        source_profile_ids: BTreeSet::new(),
                        used_by: BTreeSet::new(),
                        references: BTreeMap::new(),
                        current_cmyx_results: Vec::new(),
                        resolved_assignments: Vec::new(),
                        unresolved_reference_count: 0,
                    });
            accumulator.source_slots.extend(
                requirements
                    .iter()
                    .flat_map(|requirement| requirement.source_slots.iter().cloned()),
            );
            accumulator.source_profile_ids.extend(
                requirements
                    .iter()
                    .flat_map(|requirement| requirement.source_profile_ids.iter().cloned())
                    .filter(|profile| !profile.trim().is_empty()),
            );
            accumulator.used_by.insert(scope.display_name.clone());
            accumulator.references.insert(
                scope.id.clone(),
                ProjectDirectReferenceAccumulator {
                    scope_name: scope.display_name.clone(),
                    requirement_ids,
                },
            );
            accumulator
                .current_cmyx_results
                .push(ProjectDirectCmyxResultView {
                    scope_id: scope.id.clone(),
                    scope_name: scope.display_name.clone(),
                    recipe: recipe_name(&canonical.cmyx_candidate.recipe),
                    predicted_hex: canonical.cmyx_candidate.predicted_color.map(color_hex),
                    delta_e00: canonical.cmyx_candidate.delta_e00.map(round_one),
                    confidence: confidence_name(canonical.cmyx_candidate.confidence),
                });
            if let Some(assignment) = resolved_assignment {
                accumulator.resolved_assignments.push(assignment);
            } else {
                accumulator.unresolved_reference_count += 1;
            }
        }
    }

    let effective_pair_count = grouped.len();
    let physical_identities = grouped
        .values()
        .map(|pair| pair.physical_identity.clone())
        .collect::<BTreeSet<_>>();
    let direct_pair_count = physical_identities.len();
    let physical_identity_metadata = physical_identities
        .into_iter()
        .enumerate()
        .map(|(index, identity)| {
            (
                identity,
                (format!("project-physical-{:02}", index + 1), index),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let available =
        effective_inventory_complete && (1..=MAXIMUM_PAIR_COUNT).contains(&direct_pair_count);
    let unavailable_reason = if !effective_inventory_complete {
        Some(
            "Project-wide Direct Spools eligibility is unavailable because one or more included source scopes could not produce a complete effective-pair inventory."
                .to_owned(),
        )
    } else if direct_pair_count == 0 {
        Some(
            "Project-wide Direct Spools requires at least one physical Direct identity.".to_owned(),
        )
    } else if direct_pair_count > MAXIMUM_PAIR_COUNT {
        Some(format!(
            "Project-wide Direct Spools is unavailable because {effective_pair_count} semantic material-color-role pairs require {direct_pair_count} unique physical Direct identities; the maximum is {MAXIMUM_PAIR_COUNT}."
        ))
    } else {
        None
    };
    let mappings = grouped
        .into_values()
        .enumerate()
        .map(|(index, mut pair)| {
            let (physical_identity_id, physical_index) = physical_identity_metadata
                .get(&pair.physical_identity)
                .expect("every semantic pair has a physical Direct identity");
            let first_assignment = pair.resolved_assignments.first();
            let common_assignment = first_assignment.filter(|first| {
                pair.unresolved_reference_count == 0
                    && pair
                        .resolved_assignments
                        .iter()
                        .all(|candidate| candidate.0 == first.0 && candidate.1 == first.1)
            });
            pair.current_cmyx_results
                .sort_by(|left, right| left.scope_id.cmp(&right.scope_id));
            ProjectDirectPaletteMappingView {
                id: format!("project-pair-{:02}", index + 1),
                physical_identity_id: physical_identity_id.clone(),
                source_material: material_name(&pair.requirement.material).to_owned(),
                source_role: material_role_name(pair.requirement.role),
                source_hex: color_hex(pair.requirement.source_color),
                source_slots: pair.source_slots.into_iter().collect(),
                source_profile_ids: pair.source_profile_ids.into_iter().collect(),
                used_by: pair.used_by.into_iter().collect(),
                references: pair
                    .references
                    .into_iter()
                    .map(|(scope_id, reference)| ProjectDirectPaletteReferenceView {
                        scope_id,
                        scope_name: reference.scope_name,
                        requirement_ids: reference.requirement_ids.into_iter().collect(),
                    })
                    .collect(),
                current_cmyx_results: pair.current_cmyx_results,
                selected_spool_id: common_assignment
                    .map_or_else(String::new, |assignment| assignment.0.clone()),
                direct_toolhead: common_assignment.map_or_else(
                    || toolhead_name(Toolhead::ALL[(*physical_index).min(Toolhead::ALL.len() - 1)]),
                    |assignment| toolhead_name(assignment.1),
                ),
                material_substitution_acknowledged: common_assignment.is_some()
                    && pair
                        .resolved_assignments
                        .iter()
                        .all(|candidate| candidate.2),
            }
        })
        .collect();

    ProjectDirectPaletteView {
        available,
        effective_pair_count,
        direct_pair_count,
        maximum_pair_count: MAXIMUM_PAIR_COUNT,
        unavailable_reason,
        mappings,
    }
}

fn color_resolutions(
    scopes: &[u1_planner::PrintScope],
    inventory: &BTreeMap<&str, &Spool>,
) -> Vec<ColorResolutionView> {
    let mut resolutions = Vec::new();
    for scope in scopes {
        if scope.strategy == ScopeStrategy::DirectSpools {
            continue;
        }
        let mut grouped =
            BTreeMap::<FallbackMappingIdentity, Vec<&MaterialColorRequirement>>::new();
        for requirement in &scope.requirements {
            grouped
                .entry(fallback_mapping_identity(requirement))
                .or_default()
                .push(requirement);
        }
        for mut requirements in grouped.into_values() {
            requirements.sort_by(|left, right| left.id.cmp(&right.id));
            let requirement = requirements[0];
            let Some(default_fallback) = requirement.best_effort_cmyx_candidate.as_ref() else {
                continue;
            };
            let requirement_ids = requirements
                .iter()
                .map(|requirement| requirement.id.as_str())
                .collect::<BTreeSet<_>>();
            let approved_candidate_id = scope
                .approved_cmyx_fallbacks
                .iter()
                .find(|approval| requirement_ids.contains(approval.requirement_id.as_str()))
                .map(|approval| approval.candidate_id.as_str());
            let fallback = std::iter::once(default_fallback)
                .chain(requirement.cmyx_palette_candidates.iter())
                .find(|candidate| Some(candidate.candidate_id.as_str()) == approved_candidate_id)
                .unwrap_or(default_fallback);
            let color_approved = approved_candidate_id == Some(fallback.candidate_id.as_str());
            let material_approved = scope
                .approved_material_substitutions
                .iter()
                .any(|approval| {
                    requirement_ids.contains(approval.requirement_id.as_str())
                        && approval.candidate_id == fallback.candidate_id
                        && approval.source_material == requirement.material
                        && approval.target_material == fallback.target_material
                        && approval.acknowledged
                });
            let required_t4 = fallback
                .candidate
                .required_t4_spool_id
                .as_deref()
                .and_then(|spool_id| inventory.get(spool_id).copied());
            let requires_material_substitution = requirement.material != fallback.target_material;
            let can_add_dedicated_spool = ui_material(&requirement.material).is_some();
            let recommendation = if requires_material_substitution {
                if can_add_dedicated_spool {
                    format!(
                        "Approve the color and explicitly acknowledge the {} to {} material substitution, or add a dedicated {} spool near {}.",
                        material_name(&requirement.material),
                        material_name(&fallback.target_material),
                        material_name(&requirement.material),
                        color_hex(requirement.source_color)
                    )
                } else {
                    format!(
                        "Approve the color and explicitly acknowledge the {} to {} material substitution. Dedicated {} spools are not supported by the current desktop inventory.",
                        material_name(&requirement.material),
                        material_name(&fallback.target_material),
                        material_name(&requirement.material)
                    )
                }
            } else {
                format!(
                    "Approve the closest color, or add a dedicated {} spool near {} for a better match.",
                    material_name(&requirement.material),
                    color_hex(requirement.source_color)
                )
            };
            let palette_options = std::iter::once(default_fallback)
                .chain(requirement.cmyx_palette_candidates.iter())
                .map(|candidate| cmyx_palette_option(candidate, inventory))
                .collect::<Vec<_>>();
            resolutions.push(ColorResolutionView {
                scope_id: scope.id.clone(),
                scope_name: scope.display_name.clone(),
                requirement_id: requirement.id.clone(),
                source_identity_key: fallback_mapping_inheritance_key(requirement),
                candidate_id: fallback.candidate_id.clone(),
                source_material: material_name(&requirement.material).to_owned(),
                source_hex: color_hex(requirement.source_color),
                target_material: material_name(&fallback.target_material).to_owned(),
                target_hex: color_hex(fallback.target_color),
                predicted_hex: fallback.candidate.predicted_color.map(color_hex),
                recipe: recipe_name(&fallback.candidate.recipe),
                delta_e00: fallback.candidate.delta_e00.map(round_one),
                confidence: confidence_name(fallback.candidate.confidence),
                required_t4_spool_id: fallback.candidate.required_t4_spool_id.clone(),
                required_t4_name: required_t4.map(|spool| spool.display_name.clone()),
                required_t4_hex: required_t4.map(|spool| color_hex(spool.actual_color())),
                color_approved,
                material_approved,
                requires_material_substitution,
                can_add_dedicated_spool,
                recommendation,
                palette_options,
            });
        }
    }
    resolutions.sort_by(|left, right| {
        left.scope_id
            .cmp(&right.scope_id)
            .then_with(|| left.requirement_id.cmp(&right.requirement_id))
    });
    resolutions
}

fn cmyx_palette_option(
    fallback: &u1_planner::BestEffortCmyxCandidate,
    inventory: &BTreeMap<&str, &Spool>,
) -> CmyxPaletteOptionView {
    let required_t4 = fallback
        .candidate
        .required_t4_spool_id
        .as_deref()
        .and_then(|spool_id| inventory.get(spool_id).copied());
    CmyxPaletteOptionView {
        candidate_id: fallback.candidate_id.clone(),
        target_material: material_name(&fallback.target_material).to_owned(),
        target_hex: color_hex(fallback.target_color),
        predicted_hex: fallback.candidate.predicted_color.map(color_hex),
        recipe: recipe_name(&fallback.candidate.recipe),
        delta_e00: fallback.candidate.delta_e00.map(round_one),
        confidence: confidence_name(fallback.candidate.confidence),
        required_t4_spool_id: fallback.candidate.required_t4_spool_id.clone(),
        required_t4_name: required_t4.map(|spool| spool.display_name.clone()),
        required_t4_hex: required_t4.map(|spool| color_hex(spool.actual_color())),
    }
}

fn color_mappings(
    job: &u1_planner::PlannedJob,
    options: &BTreeMap<&str, &ScopeStrategyOptions>,
    scopes: &BTreeMap<&str, &u1_planner::PrintScope>,
) -> Option<Vec<DirectColorMappingView>> {
    let structurally_direct = job.scope_ids.iter().all(|scope| {
        options
            .get(scope.as_str())
            .is_some_and(|option| direct_option_is_exposable(option))
    });
    if !structurally_direct {
        return None;
    }
    let source_requirement_for_mapping = |mapping: &u1_planner::SourceToActualMapping| {
        scopes.get(mapping.scope_id.as_str()).and_then(|scope| {
            mapping.source_requirement_ids.iter().find_map(|id| {
                scope
                    .requirements
                    .iter()
                    .find(|requirement| requirement.id == *id)
            })
        })
    };
    let physical_identity_metadata = direct_physical_identity_metadata(
        job.color_mappings
            .iter()
            .filter_map(source_requirement_for_mapping),
    );
    let mappings = job
        .color_mappings
        .iter()
        .enumerate()
        .filter_map(|(index, mapping)| {
            let material = ui_material(&mapping.source_material)?;
            let source_requirement = source_requirement_for_mapping(mapping);
            let physical_metadata = source_requirement
                .map(direct_physical_mapping_identity)
                .and_then(|identity| physical_identity_metadata.get(&identity));
            let physical_identity_id = physical_metadata.map_or_else(
                || format!("plate-physical-unresolved-{:02}", index + 1),
                |metadata| metadata.0.clone(),
            );
            let physical_index = physical_metadata.map_or(index, |metadata| metadata.1);
            let assignment =
                options.get(mapping.scope_id.as_str()).and_then(|option| {
                    match &option.direct_spools {
                        DirectSpoolEligibility::Eligible { assignments } => {
                            assignments.iter().find(|assignment| {
                                assignment
                                    .source_requirement_ids
                                    .iter()
                                    .any(|id| mapping.source_requirement_ids.contains(id))
                            })
                        }
                        DirectSpoolEligibility::Ineligible { .. } => None,
                    }
                });
            let requested_assignment = scopes.get(mapping.scope_id.as_str()).and_then(|scope| {
                scope.direct_assignments.iter().find(|request| {
                    mapping
                        .source_requirement_ids
                        .contains(&request.requirement_id)
                })
            });
            let toolhead = mapping
                .direct_toolhead
                .or_else(|| assignment.map(|assignment| assignment.toolhead))
                .unwrap_or(Toolhead::ALL[physical_index.min(Toolhead::ALL.len() - 1)]);
            let spool_id = assignment
                .map(|assignment| assignment.spool_id.clone())
                .or_else(|| mapping.actual_spool_id.clone())
                .unwrap_or_default();
            let used_by = scopes
                .get(mapping.scope_id.as_str())
                .map(|scope| scope.display_name.clone())
                .unwrap_or_else(|| mapping.scope_id.clone());
            let dedicated_support = source_requirement.is_some_and(|requirement| {
                requirement.material == Material::Pva
                    && requirement.role == MaterialRole::Support
                    && scopes
                        .get(mapping.scope_id.as_str())
                        .is_some_and(|scope| scope.dedicated_support.is_some())
            });
            let source_slot = if dedicated_support {
                "Generated support".to_owned()
            } else {
                mapping.source_slots.join(", ")
            };
            Some(DirectColorMappingView {
                id: mapping
                    .source_requirement_ids
                    .first()
                    .cloned()
                    .unwrap_or_else(|| mapping.scope_id.clone()),
                scope_id: mapping.scope_id.clone(),
                physical_identity_id,
                inheritance_key: source_requirement.map_or_else(
                    || format!("scope:{}:mapping:{}", mapping.scope_id, index + 1),
                    direct_mapping_inheritance_key,
                ),
                source_slot,
                source_name: if dedicated_support {
                    "Dedicated PVA support".to_owned()
                } else {
                    format!("Source {}", mapping.source_slots.join(" + "))
                },
                source_hex: color_hex(mapping.source_color),
                source_material: material,
                used_by,
                cmy_recipe: recipe_name(&mapping.cmyx_comparison.recipe),
                cmy_predicted_hex: mapping.cmyx_comparison.predicted_color.map(color_hex),
                cmy_delta_e00: mapping.cmyx_comparison.delta_e00.map(round_one),
                cmy_confidence: confidence_name(mapping.cmyx_comparison.confidence),
                direct_toolhead: toolhead_name(toolhead),
                selected_spool_id: spool_id,
                dedicated_support,
                material_substitution_acknowledged: requested_assignment
                    .is_some_and(|request| request.allow_material_substitution),
            })
        })
        .collect::<Vec<_>>();
    (!mappings.is_empty()).then_some(mappings)
}

fn unresolved_mappings(
    scope: &u1_planner::PrintScope,
    option: Option<&ScopeStrategyOptions>,
) -> Option<Vec<DirectColorMappingView>> {
    let option = option?;
    if !direct_option_is_exposable(option) {
        return None;
    }
    let assignments = match &option.direct_spools {
        DirectSpoolEligibility::Eligible { assignments } => Some(assignments.as_slice()),
        DirectSpoolEligibility::Ineligible { .. } => None,
    };
    let mut seen = BTreeSet::new();
    let requirements = scope
        .requirements
        .iter()
        .filter(|requirement| seen.insert(fallback_mapping_identity(requirement)))
        .collect::<Vec<_>>();
    let physical_identity_metadata =
        direct_physical_identity_metadata(requirements.iter().copied());
    let mappings = requirements
        .into_iter()
        .filter_map(|requirement| {
            let material = ui_material(&requirement.material)?;
            let physical_identity = direct_physical_mapping_identity(requirement);
            let (physical_identity_id, physical_index) = physical_identity_metadata
                .get(&physical_identity)
                .expect("every semantic mapping has a physical Direct identity");
            let assignment = assignments.and_then(|assignments| {
                assignments
                    .iter()
                    .find(|assignment| assignment.source_requirement_ids.contains(&requirement.id))
            });
            let requested_assignment = scope
                .direct_assignments
                .iter()
                .find(|request| request.requirement_id == requirement.id);
            let dedicated_support = requirement.material == Material::Pva
                && requirement.role == MaterialRole::Support
                && scope.dedicated_support.is_some();
            Some(DirectColorMappingView {
                id: requirement.id.clone(),
                scope_id: scope.id.clone(),
                physical_identity_id: physical_identity_id.clone(),
                inheritance_key: direct_mapping_inheritance_key(requirement),
                source_slot: if dedicated_support {
                    "Generated support".to_owned()
                } else {
                    requirement.source_slots.join(", ")
                },
                source_name: if dedicated_support {
                    "Dedicated PVA support".to_owned()
                } else {
                    format!("Source {}", requirement.source_slots.join(" + "))
                },
                source_hex: color_hex(requirement.source_color),
                source_material: material,
                used_by: scope.display_name.clone(),
                cmy_recipe: recipe_name(&requirement.cmyx_candidate.recipe),
                cmy_predicted_hex: requirement.cmyx_candidate.predicted_color.map(color_hex),
                cmy_delta_e00: requirement.cmyx_candidate.delta_e00.map(round_one),
                cmy_confidence: confidence_name(requirement.cmyx_candidate.confidence),
                direct_toolhead: toolhead_name(
                    assignment
                        .map(|assignment| assignment.toolhead)
                        .unwrap_or(Toolhead::ALL[(*physical_index).min(Toolhead::ALL.len() - 1)]),
                ),
                selected_spool_id: assignment
                    .map(|assignment| assignment.spool_id.clone())
                    .unwrap_or_default(),
                dedicated_support,
                material_substitution_acknowledged: requested_assignment
                    .is_some_and(|request| request.allow_material_substitution),
            })
        })
        .collect::<Vec<_>>();
    (!mappings.is_empty()).then_some(mappings)
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct FallbackMappingIdentity {
    material: Material,
    role: MaterialRole,
    color: RgbColor,
    source_profile: FallbackSourceProfileIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct DirectPhysicalMappingIdentity {
    material: Material,
    color: RgbColor,
    source_profile: DirectPhysicalMappingSourceProfile,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum DirectPhysicalMappingSourceProfile {
    Declared(Vec<String>),
    Unknown {
        role: MaterialRole,
        discriminator: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum FallbackSourceProfileIdentity {
    Declared(Vec<String>),
    Unknown(Vec<String>),
}

fn fallback_mapping_identity(requirement: &MaterialColorRequirement) -> FallbackMappingIdentity {
    let source_profile_ids = canonical_mapping_strings(&requirement.source_profile_ids);
    let source_profile = if !source_profile_ids.is_empty()
        && source_profile_ids
            .iter()
            .all(|profile| !profile.trim().is_empty())
    {
        FallbackSourceProfileIdentity::Declared(source_profile_ids)
    } else {
        let mut discriminator = canonical_mapping_strings(&requirement.source_slots);
        if discriminator.is_empty() {
            discriminator.push(requirement.id.clone());
        }
        FallbackSourceProfileIdentity::Unknown(discriminator)
    };
    FallbackMappingIdentity {
        material: requirement.material.clone(),
        role: requirement.role,
        color: requirement.source_color,
        source_profile,
    }
}

fn direct_physical_mapping_identity(
    requirement: &MaterialColorRequirement,
) -> DirectPhysicalMappingIdentity {
    let semantic = fallback_mapping_identity(requirement);
    let source_profile = match semantic.source_profile {
        FallbackSourceProfileIdentity::Declared(profiles) => {
            DirectPhysicalMappingSourceProfile::Declared(profiles)
        }
        FallbackSourceProfileIdentity::Unknown(discriminator) => {
            DirectPhysicalMappingSourceProfile::Unknown {
                role: semantic.role,
                discriminator,
            }
        }
    };
    DirectPhysicalMappingIdentity {
        material: semantic.material,
        color: semantic.color,
        source_profile,
    }
}

fn direct_mapping_inheritance_key(requirement: &MaterialColorRequirement) -> String {
    let identity = direct_physical_mapping_identity(requirement);
    let material = material_name(&identity.material).to_owned();
    let color = color_hex(identity.color);
    match identity.source_profile {
        DirectPhysicalMappingSourceProfile::Declared(profiles) => {
            serde_json::to_string(&("direct-source-v1", material, color, "declared", profiles))
        }
        DirectPhysicalMappingSourceProfile::Unknown {
            role,
            discriminator,
        } => serde_json::to_string(&(
            "direct-source-v1",
            material,
            color,
            "unknown",
            material_role_name(role),
            discriminator,
        )),
    }
    .expect("Direct inheritance identity is JSON serializable")
}

fn fallback_mapping_inheritance_key(requirement: &MaterialColorRequirement) -> String {
    let identity = fallback_mapping_identity(requirement);
    let material = material_name(&identity.material).to_owned();
    let color = color_hex(identity.color);
    match identity.source_profile {
        FallbackSourceProfileIdentity::Declared(profiles) => serde_json::to_string(&(
            "cmyx-source-v1",
            material,
            color,
            material_role_name(identity.role),
            "declared",
            profiles,
        )),
        FallbackSourceProfileIdentity::Unknown(discriminator) => serde_json::to_string(&(
            "cmyx-source-v1",
            material,
            color,
            material_role_name(identity.role),
            "unknown",
            discriminator,
        )),
    }
    .expect("CMY+X inheritance identity is JSON serializable")
}

fn direct_physical_identity_metadata<'a>(
    requirements: impl Iterator<Item = &'a MaterialColorRequirement>,
) -> BTreeMap<DirectPhysicalMappingIdentity, (String, usize)> {
    let mut metadata = BTreeMap::new();
    for identity in requirements.map(direct_physical_mapping_identity) {
        if metadata.contains_key(&identity) {
            continue;
        }

        // The requirement iterator is already in canonical UI order. Preserve
        // the first semantic occurrence so fallback T1-T4 assignments cannot
        // be reordered by lexical details such as `slot-10` versus `slot-8`.
        let index = metadata.len();
        metadata.insert(
            identity,
            (format!("plate-physical-{:02}", index + 1), index),
        );
    }
    metadata
}

fn canonical_mapping_strings(values: &[String]) -> Vec<String> {
    let mut values = values.to_vec();
    values.sort();
    values.dedup();
    values
}

fn direct_status(
    scope_ids: &[String],
    options: &BTreeMap<&str, &ScopeStrategyOptions>,
) -> (bool, Option<String>) {
    let selected = scope_ids
        .iter()
        .filter_map(|id| options.get(id.as_str()).copied())
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return (
            false,
            Some("Direct Spools eligibility is unavailable.".to_owned()),
        );
    }
    if selected.len() == scope_ids.len()
        && selected
            .iter()
            .all(|option| direct_option_is_exposable(option))
    {
        return (true, None);
    }
    let reason = selected
        .iter()
        .find_map(|option| match &option.direct_spools {
            DirectSpoolEligibility::Ineligible { reason } => Some(direct_reason(reason)),
            DirectSpoolEligibility::Eligible { .. } => None,
        });
    (false, reason)
}

fn direct_option_has_printable_loadout(option: &ScopeStrategyOptions) -> bool {
    match &option.direct_spools {
        DirectSpoolEligibility::Eligible { assignments } => {
            let spools = assignments
                .iter()
                .map(|assignment| assignment.spool_id.as_str())
                .filter(|spool_id| !spool_id.is_empty())
                .collect::<BTreeSet<_>>();
            !assignments.is_empty()
                && assignments
                    .iter()
                    .all(|assignment| !assignment.spool_id.is_empty())
                && (1..=4).contains(&spools.len())
        }
        DirectSpoolEligibility::Ineligible { .. } => false,
    }
}

fn direct_option_is_exposable(option: &ScopeStrategyOptions) -> bool {
    (1..=4).contains(&option.direct_pair_count) || direct_option_has_printable_loadout(option)
}

fn direct_reason(reason: &DirectIneligibility) -> String {
    match reason {
        DirectIneligibility::TooManyEffectivePairs { count, maximum } => format!(
            "Direct Spools is unavailable because this scope requires {count} physical Direct identities; the maximum is {maximum}. Semantic role pairs sharing the same material, color, and declared source profile use one identity."
        ),
        DirectIneligibility::NoEffectivePairs => {
            "Direct Spools requires at least one effective material-color pair.".to_owned()
        }
        DirectIneligibility::MissingCompatibleSpools { material } => format!(
            "Direct Spools has no confirmed compatible {} inventory.",
            material_name(material)
        ),
        DirectIneligibility::InvalidManualAssignment { message } => message.clone(),
    }
}

fn t4_action(
    batch: Option<&u1_planner::PlannedBatch>,
    inventory: &BTreeMap<&str, &Spool>,
) -> String {
    let Some(batch) = batch else {
        return "Verify setup".to_owned();
    };
    let action = batch.setup_actions.iter().find(|action| {
        action.phase == SetupPhase::BeforeBatch
            && action.toolhead == Some(Toolhead::T4)
            && action.kind != SetupActionKind::Keep
    });
    let Some(action) = action else {
        return "Keep T4".to_owned();
    };
    match &action.to {
        ToolheadSlotState::Loaded(id) => inventory
            .get(id.as_str())
            .map(|spool| format!("Load {}", spool.display_name))
            .unwrap_or_else(|| format!("Load {id}")),
        ToolheadSlotState::Unknown => "Verify T4".to_owned(),
        ToolheadSlotState::Empty => "Unload T4".to_owned(),
    }
}

fn loadout_spool_ids(loadout: &PrinterLoadout) -> Vec<&str> {
    match loadout {
        PrinterLoadout::U1 { loadout } => loadout
            .slots
            .iter()
            .filter_map(|spool| spool.as_deref())
            .collect(),
        PrinterLoadout::A1Mini { spool_id } => vec![spool_id.as_str()],
    }
}

fn loadout_label(loadout: &PrinterLoadout, inventory: &BTreeMap<&str, &Spool>) -> String {
    match loadout {
        PrinterLoadout::A1Mini { spool_id } => inventory
            .get(spool_id.as_str())
            .map(|spool| spool.display_name.clone())
            .unwrap_or_else(|| spool_id.clone()),
        PrinterLoadout::U1 { loadout } => {
            let loaded = loadout.slots.iter().filter(|slot| slot.is_some()).count();
            let is_cmy = loadout.slots[0].as_deref() == Some("panchroma-translucent-cyan")
                && loadout.slots[1].as_deref() == Some("panchroma-translucent-magenta")
                && loadout.slots[2].as_deref() == Some("panchroma-translucent-yellow");
            if is_cmy {
                let t4 = loadout.slots[3]
                    .as_deref()
                    .and_then(|id| inventory.get(id))
                    .map(|spool| spool.display_name.as_str())
                    .unwrap_or("No T4");
                format!("CMY + {t4}")
            } else {
                format!("{loaded} direct spools")
            }
        }
    }
}

fn source_colors_from_mappings(
    mappings: &[u1_planner::SourceToActualMapping],
) -> Vec<SourceColorView> {
    collect_source_colors(mappings.iter().map(|mapping| {
        (
            &mapping.source_material,
            mapping.source_color,
            mapping.source_slots.as_slice(),
        )
    }))
}

fn source_colors_from_requirements<'a>(
    requirements: impl IntoIterator<Item = &'a MaterialColorRequirement>,
) -> Vec<SourceColorView> {
    collect_source_colors(requirements.into_iter().map(|requirement| {
        (
            &requirement.material,
            requirement.source_color,
            requirement.source_slots.as_slice(),
        )
    }))
}

fn collect_source_colors<'a>(
    colors: impl IntoIterator<Item = (&'a Material, RgbColor, &'a [String])>,
) -> Vec<SourceColorView> {
    let mut grouped = BTreeMap::<(String, String), BTreeSet<String>>::new();
    for (material, color, source_slots) in colors {
        grouped
            .entry((material_name(material).to_owned(), color_hex(color)))
            .or_default()
            .extend(source_slots.iter().cloned());
    }
    grouped
        .into_iter()
        .map(
            |((source_material, source_hex), source_slots)| SourceColorView {
                source_slots: source_slots.into_iter().collect(),
                source_material,
                source_hex,
            },
        )
        .collect()
}

fn unique_mapping_count(mappings: &[u1_planner::SourceToActualMapping]) -> usize {
    mappings
        .iter()
        .map(|mapping| {
            (
                material_name(&mapping.source_material).to_owned(),
                color_hex(mapping.source_color),
            )
        })
        .collect::<BTreeSet<_>>()
        .len()
}

fn recipe_summary(
    strategy: ColorStrategy,
    mappings: &[u1_planner::SourceToActualMapping],
) -> String {
    if strategy == ColorStrategy::DirectSpools {
        return format!("{} direct solids", unique_mapping_count(mappings));
    }
    if strategy == ColorStrategy::A1Mono {
        return "1 solid".to_owned();
    }
    let mixed = mappings
        .iter()
        .filter(|mapping| {
            matches!(
                mapping.cmyx_comparison.recipe,
                CmyxRecipe::FullSpectrum { .. }
            )
        })
        .count();
    let solid = mappings.len().saturating_sub(mixed);
    format!("{mixed} mixed · {solid} solid")
}

fn recipe_name(recipe: &CmyxRecipe) -> String {
    match recipe {
        CmyxRecipe::Solid { toolhead } => format!("Solid {}", toolhead_name(*toolhead)),
        CmyxRecipe::FullSpectrum { mode, sequence } => format!(
            "{} {}",
            mode_name(*mode),
            sequence
                .iter()
                .map(|toolhead| toolhead_name(*toolhead))
                .collect::<Vec<_>>()
                .join("·")
        ),
        CmyxRecipe::DedicatedT4 => "Dedicated T4".to_owned(),
        CmyxRecipe::Fallback { description, .. } => description.clone(),
        CmyxRecipe::ManualReview { reason } | CmyxRecipe::Unreachable { reason } => reason.clone(),
    }
}

fn material_summary(materials: &[Material]) -> String {
    let names = materials.iter().map(material_name).collect::<BTreeSet<_>>();
    names.into_iter().collect::<Vec<_>>().join(" / ")
}

fn source_application_label(analysis: &ProjectAnalysis) -> String {
    let name = analysis
        .source
        .application_name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(match analysis.source.application {
            u1_three_mf::SourceApplication::BambuStudio => "Bambu Studio",
            u1_three_mf::SourceApplication::OrcaSlicer => "OrcaSlicer",
            u1_three_mf::SourceApplication::SnapmakerOrca => "Snapmaker Orca",
            u1_three_mf::SourceApplication::Unknown => "Unknown application",
        });
    match analysis
        .source
        .application_version
        .as_deref()
        .filter(|version| !version.trim().is_empty())
    {
        Some(version) => format!("{name} {version}"),
        None => name.to_owned(),
    }
}

fn material_name(material: &Material) -> &str {
    match material {
        Material::Pla => "PLA",
        Material::Petg => "PETG",
        Material::Pva => "PVA",
        Material::Abs => "ABS",
        Material::Asa => "ASA",
        Material::Tpu => "TPU",
        Material::Other(name) => name,
    }
}

fn material_role_name(role: MaterialRole) -> &'static str {
    match role {
        MaterialRole::Cosmetic => "Cosmetic",
        MaterialRole::Functional => "Functional",
        MaterialRole::Support => "Support",
        MaterialRole::Unknown => "Unknown",
    }
}

fn ui_material(material: &Material) -> Option<&'static str> {
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

fn dialect_name(dialect: ProjectDialect) -> &'static str {
    match dialect {
        ProjectDialect::BambuStudioProject => "Bambu Studio 3MF",
        ProjectDialect::OrcaSlicerProject => "OrcaSlicer 3MF",
        ProjectDialect::SnapmakerOrcaProject => "Snapmaker Orca 3MF",
        ProjectDialect::Standard3mf => "Standard 3MF",
        ProjectDialect::Unknown => "Unknown 3MF dialect",
    }
}

fn printer_name(printer: Printer) -> &'static str {
    match printer {
        Printer::U1 => "U1",
        Printer::A1Mini => "A1 mini",
    }
}

fn preference_name(preference: PrinterPreference) -> &'static str {
    match preference {
        PrinterPreference::Auto => "auto",
        PrinterPreference::U1 => "u1",
        PrinterPreference::A1Mini => "a1-mini",
    }
}

fn strategy_name(strategy: ColorStrategy) -> &'static str {
    match strategy {
        ColorStrategy::CmyxFullSpectrum => "cmyx",
        ColorStrategy::CmyxSolid => "cmyx-solid",
        ColorStrategy::DirectSpools => "direct",
        ColorStrategy::A1Mono => "a1-mono",
    }
}

fn quality(delta: f64) -> &'static str {
    if delta <= 3.0 {
        "Excellent"
    } else if delta <= 6.0 {
        "Very Good"
    } else {
        "Review"
    }
}

fn confidence_name(confidence: ColorConfidence) -> &'static str {
    match confidence {
        ColorConfidence::Measured => "Measured",
        ColorConfidence::Calibrated => "High",
        ColorConfidence::Nominal | ColorConfidence::Unknown => "Nominal",
    }
}

fn toolhead_name(toolhead: Toolhead) -> &'static str {
    match toolhead {
        Toolhead::T1 => "T1",
        Toolhead::T2 => "T2",
        Toolhead::T3 => "T3",
        Toolhead::T4 => "T4",
    }
}

fn mode_name(mode: FullSpectrumMode) -> &'static str {
    match mode {
        FullSpectrumMode::Gradient => "Gradient",
        FullSpectrumMode::Ratio => "Ratio",
        FullSpectrumMode::Match => "Match",
        FullSpectrumMode::Cycle => "Cycle",
    }
}

fn round_one(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use u1_application::{
        CmyxCalibrationLoadout, CmyxMeasurementMethod, CmyxMeasurementProvenance,
        full_spectrum_calibration_context,
    };
    use u1_color_engine::{
        GeometryClass, MixRecipe, RecipeComponent, RecipeMode, SampleOrientation,
    };

    fn fallback_requirement(
        id: &str,
        role: MaterialRole,
        source_slots: &[&str],
        source_profile_ids: &[&str],
    ) -> MaterialColorRequirement {
        let source_color = RgbColor::new(25, 50, 75);
        MaterialColorRequirement {
            id: id.to_owned(),
            material: Material::Pla,
            role,
            source_color,
            source_slots: source_slots
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            source_profile_ids: source_profile_ids
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            cmyx_candidate: u1_planner::CmyxColorCandidate {
                recipe: CmyxRecipe::Solid {
                    toolhead: Toolhead::T1,
                },
                calibration_sample_id: None,
                process_compatibility: None,
                required_t4_spool_id: None,
                predicted_color: Some(source_color),
                delta_e00: Some(0.0),
                confidence: ColorConfidence::Nominal,
                warnings: Vec::new(),
            },
            best_effort_cmyx_candidate: None,
            cmyx_palette_candidates: Vec::new(),
            direct_candidates: Vec::new(),
        }
    }

    fn fallback_scope(requirements: Vec<MaterialColorRequirement>) -> u1_planner::PrintScope {
        u1_planner::PrintScope {
            id: "scope".to_owned(),
            display_name: "Scope".to_owned(),
            requirements,
            units: Vec::new(),
            strategy: ScopeStrategy::Auto,
            direct_assignments: Vec::new(),
            dedicated_support: None,
            approved_cmyx_fallbacks: Vec::new(),
            approved_material_substitutions: Vec::new(),
        }
    }

    fn make_scope_requirements_printable(scope: &mut u1_planner::PrintScope) {
        scope.units = vec![u1_planner::PrintableUnit {
            id: format!("{}-unit", scope.id),
            source_unit_id: format!("{}-source-unit", scope.id),
            source_object_id: 1,
            source_instance_id: 0,
            source_model_path: None,
            display_name: format!("{} unit", scope.display_name),
            source_plate_id: Some(scope.id.clone()),
            requirement_ids: scope
                .requirements
                .iter()
                .map(|requirement| requirement.id.clone())
                .collect(),
            bounds: u1_planner::BoundsMm::from_size(20.0, 20.0, 20.0),
            source_layer_height_mm: Some(0.2),
            printer_preference: PrinterPreference::Auto,
        }];
    }

    fn fallback_option(effective_pair_count: usize) -> ScopeStrategyOptions {
        ScopeStrategyOptions {
            scope_id: "scope".to_owned(),
            effective_pair_count,
            direct_pair_count: effective_pair_count,
            cmyx_available: true,
            direct_spools: DirectSpoolEligibility::Ineligible {
                reason: DirectIneligibility::MissingCompatibleSpools {
                    material: Material::Pla,
                },
            },
            selected_strategy: ScopeStrategy::Auto,
        }
    }

    #[test]
    fn view_literals_match_the_typescript_contract() {
        assert_eq!(printer_name(Printer::A1Mini), "A1 mini");
        assert_eq!(strategy_name(ColorStrategy::CmyxFullSpectrum), "cmyx");
        assert_eq!(strategy_name(ColorStrategy::CmyxSolid), "cmyx-solid");
        assert_eq!(confidence_name(ColorConfidence::Calibrated), "High");
        assert_eq!(color_hex(RgbColor::new(8, 171, 251)), "#08ABFB");
    }

    #[test]
    fn quality_uses_ui_thresholds() {
        assert_eq!(quality(3.0), "Excellent");
        assert_eq!(quality(3.1), "Very Good");
        assert_eq!(quality(6.1), "Review");
    }

    #[test]
    fn source_color_metadata_coalesces_slots_but_preserves_material_identity() {
        let first = fallback_requirement("first", MaterialRole::Cosmetic, &["F1"], &["profile-a"]);
        let second =
            fallback_requirement("second", MaterialRole::Cosmetic, &["F2"], &["profile-b"]);
        let mut petg =
            fallback_requirement("petg", MaterialRole::Cosmetic, &["F6"], &["profile-c"]);
        petg.material = Material::Petg;

        let colors = source_colors_from_requirements([&first, &second, &petg]);

        assert_eq!(colors.len(), 2);
        let pla = colors
            .iter()
            .find(|color| color.source_material == "PLA")
            .expect("PLA source color");
        assert_eq!(pla.source_slots, vec!["F1", "F2"]);
        assert_eq!(pla.source_hex, "#19324B");
        let petg = colors
            .iter()
            .find(|color| color.source_material == "PETG")
            .expect("PETG source color");
        assert_eq!(petg.source_slots, vec!["F6"]);
        assert_eq!(petg.source_hex, "#19324B");
    }

    #[test]
    fn serialized_view_matches_the_native_frontend_envelope() {
        let view = ProjectPlanView {
            summary: ProjectSummaryView {
                file_name: "fixture.3mf".to_owned(),
                source_hash: "sha256:fixture".to_owned(),
                source_dialect: "Bambu Studio 3MF".to_owned(),
                source_application: "Bambu Studio 02.02.00.85".to_owned(),
                source_byte_size: 42,
                source_plate_count: 1,
                object_count: 1,
                instance_count: 1,
                part_count: 1,
                printable_part_count: 1,
                painted_part_count: 0,
                bounded_instance_count: 1,
                used_filament_count: 1,
                unused_filament_count: 0,
                alternative_plate_count: 1,
            },
            detected_adhesion: DetectedAdhesionPolicyView {
                mode: DetectedAdhesionMode::Reliable,
                profile_name: Some("U1 Planner Reliable Adhesion v2".to_owned()),
            },
            alternative_plates: vec![AlternativePlateView {
                id: 7,
                name: "Updated Ball Joints".to_owned(),
                included: false,
            }],
            project_direct_palette: ProjectDirectPaletteView {
                available: true,
                effective_pair_count: 1,
                direct_pair_count: 1,
                maximum_pair_count: 4,
                unavailable_reason: None,
                mappings: Vec::new(),
            },
            scope_selections: vec![ScopeSelectionView {
                scope_id: "plate-1".to_owned(),
                strategy: "direct",
                assignments: vec![DirectAssignmentSelectionView {
                    requirement_id: "plate-1-requirement-1".to_owned(),
                    spool_id: "user-orange".to_owned(),
                    toolhead: Some("T4"),
                    allow_material_substitution: false,
                }],
                approved_color_fallbacks: Vec::new(),
                material_substitutions: Vec::new(),
            }],
            unit_printer_selections: vec![UnitPrinterSelectionView {
                source_unit_id: "plate-1-object-1-instance-1".to_owned(),
                preference: "auto",
            }],
            color_resolutions: Vec::new(),
            plates: Vec::new(),
            batches: Vec::new(),
            t4_swap_count: 0,
            a1_spool_change_count: 0,
            blocking_errors: Vec::new(),
            global_warnings: Vec::new(),
            omitted_unit_count: 0,
            plan_ready: true,
            partial_conversion: PartialConversionView {
                available: false,
                exclusions: Vec::new(),
                reason: "The complete plan can be converted without exclusions.".to_owned(),
            },
            spools: vec![FilamentSpoolView {
                id: "user-orange".to_owned(),
                calibration_identity: "calibration-user-orange".to_owned(),
                name: "User Orange".to_owned(),
                color_name: "Orange".to_owned(),
                hex: "#D97813".to_owned(),
                material: "PLA".to_owned(),
                sku: String::new(),
                profile: String::new(),
                vendor: String::new(),
                product_line: String::new(),
                optical_descriptor: String::new(),
                min_nozzle_temperature_c: None,
                max_nozzle_temperature_c: None,
                batch_lot: String::new(),
                calibration_reference: String::new(),
                notes: String::new(),
                color_basis: "Nominal".to_owned(),
                source: "user".to_owned(),
                available: false,
            }],
            current_loadout: Vec::new(),
            current_a1_spool_id: Some("user-orange".to_owned()),
            planned_final_loadout: vec![LoadedToolheadView {
                toolhead: "T4",
                spool_id: "user-orange".to_owned(),
            }],
            planned_final_a1_spool_id: Some("user-white".to_owned()),
            restore_cmy_by_default: true,
            custom_direct_palettes_enabled: false,
            u1_cross_source_repacking_enabled: false,
        };
        let json = serde_json::to_value(view).expect("view must serialize");

        assert_eq!(json["summary"]["fileName"], "fixture.3mf");
        assert_eq!(json["summary"]["sourcePlateCount"], 1);
        assert_eq!(json["detectedAdhesion"]["mode"], "reliable");
        assert_eq!(
            json["detectedAdhesion"]["profileName"],
            "U1 Planner Reliable Adhesion v2"
        );
        assert_eq!(json["alternativePlates"][0]["id"], 7);
        assert_eq!(json["alternativePlates"][0]["name"], "Updated Ball Joints");
        assert_eq!(json["alternativePlates"][0]["included"], false);
        assert_eq!(json["projectDirectPalette"]["available"], true);
        assert_eq!(json["projectDirectPalette"]["effectivePairCount"], 1);
        assert_eq!(json["projectDirectPalette"]["directPairCount"], 1);
        assert_eq!(json["projectDirectPalette"]["maximumPairCount"], 4);
        assert_eq!(json["scopeSelections"][0]["scopeId"], "plate-1");
        assert_eq!(json["scopeSelections"][0]["strategy"], "direct");
        assert_eq!(
            json["scopeSelections"][0]["assignments"][0]["toolhead"],
            "T4"
        );
        assert!(json["scopeSelections"][0]["approvedColorFallbacks"].is_array());
        assert!(json["scopeSelections"][0]["materialSubstitutions"].is_array());
        assert_eq!(
            json["unitPrinterSelections"][0]["sourceUnitId"],
            "plate-1-object-1-instance-1"
        );
        assert_eq!(json["unitPrinterSelections"][0]["preference"], "auto");
        assert!(json["colorResolutions"].is_array());
        assert!(json["plates"].is_array());
        assert_eq!(json["currentA1SpoolId"], "user-orange");
        assert!(json["batches"].is_array());
        assert_eq!(json["t4SwapCount"], 0);
        assert_eq!(json["a1SpoolChangeCount"], 0);
        assert!(json["blockingErrors"].is_array());
        assert!(json["globalWarnings"].is_array());
        assert_eq!(json["omittedUnitCount"], 0);
        assert_eq!(json["planReady"], true);
        assert_eq!(json["partialConversion"]["available"], false);
        assert!(json["partialConversion"]["exclusions"].is_array());
        assert_eq!(json["spools"][0]["available"], false);
        assert_eq!(json["spools"][0]["source"], "user");
        assert!(json["currentLoadout"].is_array());
        assert_eq!(json["plannedFinalLoadout"][0]["toolhead"], "T4");
        assert_eq!(json["plannedFinalLoadout"][0]["spoolId"], "user-orange");
        assert_eq!(json["plannedFinalA1SpoolId"], "user-white");
        assert_eq!(json["restoreCmyByDefault"], true);
        assert_eq!(json["customDirectPalettesEnabled"], false);
        assert_eq!(json["u1CrossSourceRepackingEnabled"], false);
    }

    #[test]
    fn initial_planning_intent_maps_full_u1_and_a1_loadouts() {
        let intent: InitialPlanningIntentView = serde_json::from_value(serde_json::json!({
            "defaultStrategy": "direct",
            "a1MiniEnabled": true,
            "currentLoadout": [
                { "toolhead": " T1 ", "spoolId": "  user-cyan  " },
                { "toolhead": "T2", "spoolId": "user-magenta" },
                { "toolhead": "T3", "spoolId": "user-yellow" },
                { "toolhead": "T4", "spoolId": "user-black" }
            ],
            "currentT4SpoolId": "legacy-grey-is-ignored",
            "currentA1SpoolId": "  user-white  ",
            "allowU1CrossSourceRepacking": true
        }))
        .expect("camelCase initial planning intent must deserialize");
        let options = intent
            .into_options_with_backend_state(Vec::new(), Vec::new(), CmyxGeometryContext::default())
            .expect("the exact current loadout must validate");

        assert_eq!(options.scope_strategy, ScopeStrategy::DirectSpools);
        assert!(options.a1_mini.enabled);
        assert!(options.allow_u1_cross_source_repacking);
        assert_eq!(
            options.a1_mini.current_spool_id,
            Some("user-white".to_owned())
        );
        assert_eq!(
            options.current_toolheads.slots,
            [
                ToolheadSlotState::Loaded("user-cyan".to_owned()),
                ToolheadSlotState::Loaded("user-magenta".to_owned()),
                ToolheadSlotState::Loaded("user-yellow".to_owned()),
                ToolheadSlotState::Loaded("user-black".to_owned()),
            ]
        );
    }

    #[test]
    fn initial_planning_intent_maps_cmyx_and_ignores_blank_t4() {
        let intent: InitialPlanningIntentView = serde_json::from_value(serde_json::json!({
            "defaultStrategy": "cmyx",
            "a1MiniEnabled": false,
            "currentT4SpoolId": "   ",
            "currentA1SpoolId": null
        }))
        .expect("camelCase initial planning intent must deserialize");
        let defaults = PreliminaryPlanOptions::default();
        let options = intent
            .into_options_with_backend_state(Vec::new(), Vec::new(), CmyxGeometryContext::default())
            .expect("legacy initial intent must remain valid");

        assert_eq!(options.scope_strategy, ScopeStrategy::CmyxFullSpectrum);
        assert!(!options.a1_mini.enabled);
        assert!(!options.allow_u1_cross_source_repacking);
        assert_eq!(options.a1_mini.current_spool_id, None);
        assert_eq!(options.current_toolheads, defaults.current_toolheads);
    }

    #[test]
    fn dedicated_pva_defaults_to_interface_only_and_accepts_explicit_full_support() {
        let pva = Spool {
            id: "support-pva".to_owned(),
            calibration_id: Some("support-pva".to_owned()),
            display_name: "Reli3D PVA".to_owned(),
            color_name: Some("Natural".to_owned()),
            material: Material::Pva,
            nominal_color: RgbColor::new(225, 214, 120),
            measured_color: None,
            sku: None,
            profile_id: Some("Reli3D PVA @U1".to_owned()),
            available: true,
        };
        let intent = |usage: Option<&str>| {
            let mut value = serde_json::json!({
                "defaultStrategy": "cmyx",
                "a1MiniEnabled": false,
                "currentT4SpoolId": null,
                "currentA1SpoolId": null,
                "dedicatedSupportSpoolId": "support-pva"
            });
            if let Some(usage) = usage {
                value["dedicatedSupportUsage"] = serde_json::json!(usage);
            }
            serde_json::from_value::<InitialPlanningIntentView>(value).unwrap()
        };

        let default_options = intent(None)
            .into_options_with_backend_state(
                vec![pva.clone()],
                Vec::new(),
                CmyxGeometryContext::default(),
            )
            .unwrap();
        assert_eq!(
            default_options.dedicated_support.unwrap().usage,
            SupportMaterialUsage::InterfaceOnly
        );

        let full_options = intent(Some("body-and-interface"))
            .into_options_with_backend_state(vec![pva], Vec::new(), CmyxGeometryContext::default())
            .unwrap();
        assert_eq!(
            full_options.dedicated_support.unwrap().usage,
            SupportMaterialUsage::BodyAndInterface
        );
    }

    #[test]
    fn explicit_empty_initial_u1_loadout_is_all_unknown() {
        let intent: InitialPlanningIntentView = serde_json::from_value(serde_json::json!({
            "defaultStrategy": "auto",
            "a1MiniEnabled": false,
            "currentLoadout": [],
            "currentT4SpoolId": "legacy-grey-is-ignored",
            "currentA1SpoolId": null
        }))
        .expect("explicit empty loadout must deserialize");

        let options = intent
            .into_options_with_backend_state(Vec::new(), Vec::new(), CmyxGeometryContext::default())
            .expect("unknown U1 state is a valid conservative input");

        assert_eq!(options.current_toolheads, CurrentToolheadState::default());
    }

    #[test]
    fn initial_planning_intent_rejects_one_spool_on_u1_and_a1() {
        let intent: InitialPlanningIntentView = serde_json::from_value(serde_json::json!({
            "defaultStrategy": "auto",
            "a1MiniEnabled": true,
            "currentLoadout": [
                { "toolhead": "T1", "spoolId": " shared-spool " }
            ],
            "currentT4SpoolId": null,
            "currentA1SpoolId": "shared-spool"
        }))
        .expect("duplicate physical placement must deserialize before validation");

        let error = intent
            .into_options_with_backend_state(Vec::new(), Vec::new(), CmyxGeometryContext::default())
            .expect_err("one physical spool cannot start on both printers");

        assert!(matches!(
            error,
            ApplicationError::InvalidCurrentPrinterLoadout { message }
                if message.contains("cannot be loaded on the U1 and A1 mini")
        ));
    }

    #[test]
    fn replan_request_rejects_one_spool_on_u1_and_a1() {
        let request: PlanningRequestView = serde_json::from_value(serde_json::json!({
            "currentLoadout": [
                { "toolhead": "T2", "spoolId": "shared-spool" }
            ],
            "currentA1SpoolId": " shared-spool ",
            "a1MiniEnabled": true
        }))
        .expect("duplicate physical placement must deserialize before validation");

        let error = request
            .into_options()
            .expect_err("one physical spool cannot start on both printers");

        assert!(error.contains("cannot be loaded on the U1 and A1 mini"));
    }

    #[test]
    fn current_loadout_normalizes_before_duplicate_spool_validation() {
        let error = current_toolhead_request(vec![
            LoadedToolheadInputView {
                toolhead: "T1".to_owned(),
                spool_id: " shared-spool".to_owned(),
            },
            LoadedToolheadInputView {
                toolhead: "T2".to_owned(),
                spool_id: "shared-spool ".to_owned(),
            },
        ])
        .expect_err("trim-equivalent physical IDs must remain unique");

        assert!(error.contains("cannot be loaded in more than one toolhead"));
    }

    #[test]
    fn planned_final_a1_uses_the_last_batch_or_the_current_spool() {
        let batches = vec![
            u1_planner::PlannedBatch {
                id: "a1-first".to_owned(),
                printer: Printer::A1Mini,
                strategy: ColorStrategy::A1Mono,
                loadout: PrinterLoadout::A1Mini {
                    spool_id: "first-spool".to_owned(),
                },
                job_ids: Vec::new(),
                plate_ids: Vec::new(),
                setup_actions: Vec::new(),
                t4_swap_before: false,
            },
            u1_planner::PlannedBatch {
                id: "a1-last".to_owned(),
                printer: Printer::A1Mini,
                strategy: ColorStrategy::A1Mono,
                loadout: PrinterLoadout::A1Mini {
                    spool_id: "last-spool".to_owned(),
                },
                job_ids: Vec::new(),
                plate_ids: Vec::new(),
                setup_actions: Vec::new(),
                t4_swap_before: false,
            },
        ];

        assert_eq!(
            planned_final_a1_spool_id(&batches, Some("starting-spool")),
            Some("last-spool".to_owned())
        );
        assert_eq!(
            planned_final_a1_spool_id(&[], Some("starting-spool")),
            Some("starting-spool".to_owned())
        );
        assert_eq!(planned_final_a1_spool_id(&[], None), None);
    }

    #[test]
    fn loaded_toolhead_views_preserve_exact_u1_positions() {
        let final_toolheads = CurrentToolheadState {
            slots: [
                ToolheadSlotState::Loaded("cyan".to_owned()),
                ToolheadSlotState::Unknown,
                ToolheadSlotState::Empty,
                ToolheadSlotState::Loaded("black".to_owned()),
            ],
        };

        let loadout = loaded_toolhead_views(&final_toolheads);

        assert_eq!(loadout.len(), 2);
        assert_eq!(loadout[0].toolhead, "T1");
        assert_eq!(loadout[0].spool_id, "cyan");
        assert_eq!(loadout[1].toolhead, "T4");
        assert_eq!(loadout[1].spool_id, "black");
    }

    #[test]
    fn planned_final_u1_omits_the_spool_transferred_to_a1() {
        let final_toolheads = CurrentToolheadState {
            slots: [
                ToolheadSlotState::Loaded("shared-spool".to_owned()),
                ToolheadSlotState::Loaded("magenta".to_owned()),
                ToolheadSlotState::Unknown,
                ToolheadSlotState::Empty,
            ],
        };

        let loadout = planned_final_u1_loadout(&final_toolheads, Some("shared-spool"));

        assert_eq!(loadout.len(), 1);
        assert_eq!(loadout[0].toolhead, "T2");
        assert_eq!(loadout[0].spool_id, "magenta");
        assert!(
            loadout
                .iter()
                .all(|loaded| loaded.spool_id != "shared-spool")
        );
    }

    #[test]
    fn initial_planning_intent_rejects_unknown_fields() {
        assert!(
            serde_json::from_value::<InitialPlanningIntentView>(serde_json::json!({
                "defaultStrategy": "auto",
                "a1MiniEnabled": false,
                "currentT4SpoolId": null,
                "currentA1SpoolId": null,
                "confirmedSpools": []
            }))
            .is_err(),
            "initial planning intent must not inject authoritative backend state"
        );
    }

    #[test]
    fn native_replan_request_accepts_confirmed_spools_and_exact_assignments() {
        let request: PlanningRequestView = serde_json::from_value(serde_json::json!({
            "defaultStrategy": "direct",
            "confirmedSpools": [{
                "id": "user-orange",
                "name": "User Orange",
                "hex": "#D97813",
                "material": "PLA",
                "sku": "user-profile",
                "colorBasis": "Measured"
            }],
            "scopeOverrides": [{
                "scopeId": "plate-1",
                "strategy": "direct",
                "assignments": [{
                    "requirementId": "plate-1-requirement-1",
                    "spoolId": "user-orange",
                    "toolhead": "T4",
                    "allowMaterialSubstitution": true
                }],
                "approvedColorFallbacks": [{
                    "requirementId": "plate-1-requirement-2",
                    "candidateId": "candidate-v1"
                }],
                "materialSubstitutions": [{
                    "requirementId": "plate-1-requirement-3",
                    "candidateId": "candidate-v2",
                    "sourceMaterial": "PETG",
                    "targetMaterial": "PLA",
                    "acknowledged": true
                }]
            }],
            "unitPrinterOverrides": [{
                "sourceUnitId": "plate-1-object-10-instance-1",
                "preference": "a1-mini"
            }],
            "includedAlternativePlateIds": [7],
            "currentA1SpoolId": "user-orange",
            "restoreCmyAfterDirect": false,
            "allowDirectPaletteReduction": true,
            "allowU1CrossSourceRepacking": true,
            "a1MiniEnabled": true
        }))
        .expect("camelCase request must deserialize");
        let options = request.into_options().expect("request must validate");

        assert_eq!(options.scope_strategy, ScopeStrategy::DirectSpools);
        assert_eq!(options.confirmed_spools.len(), 1);
        assert_eq!(
            options.confirmed_spools[0].measured_color,
            Some(RgbColor::new(217, 120, 19))
        );
        assert_eq!(options.scope_overrides.len(), 1);
        assert_eq!(
            options.scope_overrides[0].strategy,
            Some(ScopeStrategy::DirectSpools)
        );
        assert_eq!(
            options.scope_overrides[0].direct_assignments[0].toolhead,
            Some(Toolhead::T4)
        );
        assert!(options.scope_overrides[0].direct_assignments[0].allow_material_substitution);
        assert_eq!(
            options.a1_mini.current_spool_id,
            Some("user-orange".to_owned())
        );
        assert_eq!(
            options.unit_printer_overrides,
            vec![UnitPrinterOverride {
                source_unit_id: "plate-1-object-10-instance-1".to_owned(),
                preference: PrinterPreference::A1Mini,
            }]
        );
        assert_eq!(
            options.scope_overrides[0].approved_cmyx_fallbacks,
            vec![CmyxFallbackApproval {
                requirement_id: "plate-1-requirement-2".to_owned(),
                candidate_id: "candidate-v1".to_owned(),
            }]
        );
        assert_eq!(
            options.scope_overrides[0].approved_material_substitutions,
            vec![MaterialSubstitutionApproval {
                requirement_id: "plate-1-requirement-3".to_owned(),
                candidate_id: "candidate-v2".to_owned(),
                source_material: Material::Petg,
                target_material: Material::Pla,
                acknowledged: true,
            }]
        );
        assert!(!options.restore_cmy_after_direct);
        assert!(options.allow_direct_palette_reduction);
        assert!(options.allow_u1_cross_source_repacking);
        assert!(options.a1_mini.enabled);
        assert_eq!(options.included_alternative_plate_ids, vec![7]);
    }

    #[test]
    fn calibration_samples_are_injected_only_from_trusted_backend_state() {
        let geometry = CmyxGeometryContext {
            orientation: SampleOrientation::Upright,
            geometry_class: GeometryClass::CalibrationSwatch,
        };
        let loadout = CmyxCalibrationLoadout::new(
            "panchroma-translucent-cyan",
            "panchroma-translucent-magenta",
            "panchroma-translucent-yellow",
            "panchroma-basic-black",
        );
        let trusted = UserCmyxCalibrationRecord {
            id: "black-r1".to_owned(),
            context: full_spectrum_calibration_context(&loadout, &geometry),
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
                "2026-08-01T18:30:00Z",
                CmyxMeasurementMethod::ReliableManualSrgb,
            ),
        };
        let request: PlanningRequestView =
            serde_json::from_value(serde_json::json!({})).expect("empty frontend request");
        let options = request
            .into_options_with_backend_state(Vec::new(), vec![trusted.clone()], geometry.clone())
            .unwrap();
        assert_eq!(options.scope_strategy, ScopeStrategy::Auto);
        assert_eq!(options.confirmed_calibration_samples, vec![trusted]);
        assert_eq!(options.cmyx_geometry_context, geometry);

        assert!(
            serde_json::from_value::<PlanningRequestView>(serde_json::json!({
                "confirmedCalibrationSamples": []
            }))
            .is_err(),
            "frontend planning requests must not inject calibration provenance"
        );
    }

    #[test]
    fn scope_selection_round_trip_preserves_direct_intent_for_a1_routing() {
        let mut scope = fallback_scope(Vec::new());
        scope.id = "a1-routed-scope".to_owned();
        scope.strategy = ScopeStrategy::DirectSpools;
        scope.direct_assignments = vec![DirectAssignmentRequest {
            requirement_id: "mono-grey".to_owned(),
            spool_id: "user-grey".to_owned(),
            toolhead: Some(Toolhead::T3),
            allow_material_substitution: false,
        }];
        scope.approved_cmyx_fallbacks = vec![CmyxFallbackApproval {
            requirement_id: "mono-grey".to_owned(),
            candidate_id: "candidate-v1".to_owned(),
        }];

        let json = serde_json::to_value(scope_selections(&[scope]))
            .expect("scope selection must serialize");
        assert_eq!(json[0]["scopeId"], "a1-routed-scope");
        assert_eq!(json[0]["strategy"], "direct");
        assert_eq!(json[0]["assignments"][0]["requirementId"], "mono-grey");
        assert_eq!(json[0]["assignments"][0]["spoolId"], "user-grey");
        assert_eq!(json[0]["assignments"][0]["toolhead"], "T3");
        assert_eq!(
            json[0]["approvedColorFallbacks"][0]["candidateId"],
            "candidate-v1"
        );

        let request: ScopePlanningInputView = serde_json::from_value(json[0].clone())
            .expect("serialized scope selection must be a valid replan input");
        let planning_override = request
            .into_override()
            .expect("persisted selection must validate");
        assert_eq!(
            planning_override.strategy,
            Some(ScopeStrategy::DirectSpools)
        );
        assert_eq!(
            planning_override.direct_assignments,
            vec![DirectAssignmentRequest {
                requirement_id: "mono-grey".to_owned(),
                spool_id: "user-grey".to_owned(),
                toolhead: Some(Toolhead::T3),
                allow_material_substitution: false,
            }]
        );
        assert_eq!(
            planning_override.approved_cmyx_fallbacks,
            vec![CmyxFallbackApproval {
                requirement_id: "mono-grey".to_owned(),
                candidate_id: "candidate-v1".to_owned(),
            }]
        );
    }

    #[test]
    fn native_replan_request_rejects_invalid_spool_color() {
        let request: PlanningRequestView = serde_json::from_value(serde_json::json!({
            "confirmedSpools": [{
                "id": "bad",
                "name": "Bad",
                "hex": "orange",
                "material": "PLA"
            }]
        }))
        .expect("request envelope must deserialize");

        assert!(
            request
                .into_options()
                .expect_err("invalid HEX must fail")
                .contains("Invalid spool HEX")
        );
    }

    #[test]
    fn native_replan_request_preserves_explicit_out_of_stock_status() {
        let request: PlanningRequestView = serde_json::from_value(serde_json::json!({
            "confirmedSpools": [{
                "id": "user-orange",
                "name": "User Orange",
                "hex": "#D97813",
                "material": "PLA",
                "available": false
            }]
        }))
        .expect("request envelope must deserialize");
        let options = request.into_options().expect("spool must validate");
        assert_eq!(options.confirmed_spools.len(), 1);
        assert!(!options.confirmed_spools[0].available);
    }

    #[test]
    fn project_spool_view_keeps_out_of_stock_library_rows() {
        let spool = Spool {
            id: "user-orange".to_owned(),
            calibration_id: None,
            display_name: "User Orange".to_owned(),
            color_name: Some("Orange".to_owned()),
            material: Material::Pla,
            nominal_color: RgbColor::new(217, 120, 19),
            measured_color: None,
            sku: None,
            profile_id: None,
            available: false,
        };
        let view = spool_view(&spool).expect("out-of-stock library row must remain visible");
        assert_eq!(view.id, "user-orange");
        assert_eq!(view.source, "user");
        assert!(!view.available);
    }

    #[test]
    fn planning_request_cannot_replace_persisted_calibration_identity() {
        let persisted = Spool {
            id: "user-orange".to_owned(),
            calibration_id: Some("persisted-lot-identity".to_owned()),
            display_name: "User Orange".to_owned(),
            color_name: Some("Orange".to_owned()),
            material: Material::Pla,
            nominal_color: RgbColor::new(217, 120, 19),
            measured_color: None,
            sku: None,
            profile_id: None,
            available: true,
        };
        let mut requested = persisted.clone();
        requested.display_name = "Renamed Orange".to_owned();
        requested.calibration_id = Some("untrusted-request-identity".to_owned());

        let merged = merge_confirmed_spools(vec![persisted], vec![requested]).unwrap();

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].display_name, "Renamed Orange");
        assert_eq!(
            merged[0].calibration_id.as_deref(),
            Some("persisted-lot-identity")
        );
    }

    #[test]
    fn structured_t4_change_uses_before_batch_actions() {
        let old = Spool {
            id: "grey".to_owned(),
            calibration_id: None,
            display_name: "Translucent Grey".to_owned(),
            color_name: Some("Grey".to_owned()),
            material: Material::Pla,
            nominal_color: RgbColor::new(128, 128, 128),
            measured_color: None,
            sku: None,
            profile_id: None,
            available: true,
        };
        let new = Spool {
            id: "black".to_owned(),
            calibration_id: None,
            display_name: "Black".to_owned(),
            color_name: Some("Black".to_owned()),
            material: Material::Pla,
            nominal_color: RgbColor::new(0, 0, 0),
            measured_color: None,
            sku: None,
            profile_id: None,
            available: true,
        };
        let spools = [old, new];
        let inventory = spools
            .iter()
            .map(|spool| (spool.id.as_str(), spool))
            .collect::<BTreeMap<_, _>>();
        let batch = u1_planner::PlannedBatch {
            id: "batch-2".to_owned(),
            printer: Printer::U1,
            strategy: ColorStrategy::CmyxFullSpectrum,
            loadout: PrinterLoadout::U1 {
                loadout: u1_planner::U1Loadout {
                    slots: [None, None, None, Some("black".to_owned())],
                },
            },
            job_ids: Vec::new(),
            plate_ids: Vec::new(),
            setup_actions: vec![
                u1_planner::SetupAction {
                    phase: SetupPhase::BeforeBatch,
                    kind: SetupActionKind::Unload,
                    toolhead: Some(Toolhead::T4),
                    from: ToolheadSlotState::Loaded("grey".to_owned()),
                    to: ToolheadSlotState::Empty,
                },
                u1_planner::SetupAction {
                    phase: SetupPhase::BeforeBatch,
                    kind: SetupActionKind::Load,
                    toolhead: Some(Toolhead::T4),
                    from: ToolheadSlotState::Empty,
                    to: ToolheadSlotState::Loaded("black".to_owned()),
                },
            ],
            t4_swap_before: true,
        };

        let change = t4_change(&batch, &inventory).expect("T4 transition must be structured");
        assert_eq!(change.from_spool_id.as_deref(), Some("grey"));
        assert_eq!(change.from_spool_name.as_deref(), Some("Translucent Grey"));
        assert_eq!(change.to_spool_id.as_deref(), Some("black"));
        assert_eq!(change.to_spool_name.as_deref(), Some("Black"));
    }

    #[test]
    fn material_substitution_accepts_declared_source_families_but_restricts_targets() {
        let cases = [
            ("ABS", Material::Abs),
            ("ASA", Material::Asa),
            ("TPU", Material::Tpu),
            ("PA-CF", Material::Other("PA-CF".to_owned())),
        ];

        for (source, expected) in cases {
            let request: ScopePlanningInputView = serde_json::from_value(serde_json::json!({
                "scopeId": "plate-1",
                "strategy": "cmyx",
                "approvedColorFallbacks": [{
                    "requirementId": "requirement-1",
                    "candidateId": "candidate-v1"
                }],
                "materialSubstitutions": [{
                    "requirementId": "requirement-1",
                    "candidateId": "candidate-v1",
                    "sourceMaterial": source,
                    "targetMaterial": "PLA",
                    "acknowledged": true
                }]
            }))
            .expect("material decision must deserialize");
            let planning_override = request
                .into_override()
                .expect("declared source material must validate");
            assert_eq!(
                planning_override.approved_material_substitutions[0].source_material,
                expected
            );
            assert_eq!(
                planning_override.approved_material_substitutions[0].target_material,
                Material::Pla
            );
        }

        let unsupported_target: ScopePlanningInputView =
            serde_json::from_value(serde_json::json!({
                "scopeId": "plate-1",
                "strategy": "cmyx",
                "materialSubstitutions": [{
                    "requirementId": "requirement-1",
                    "candidateId": "candidate-v1",
                    "sourceMaterial": "ABS",
                    "targetMaterial": "ABS",
                    "acknowledged": true
                }]
            }))
            .expect("request envelope must deserialize");
        assert!(
            unsupported_target
                .into_override()
                .expect_err("unsupported target must fail")
                .contains("desktop inventory currently accepts PLA, PETG, or PVA")
        );
    }

    #[test]
    fn color_resolutions_cover_large_scopes_and_expose_fingerprinted_material_decisions() {
        let mut requirements = (0..5)
            .map(|index| {
                let mut requirement = fallback_requirement(
                    &format!("requirement-{index}"),
                    MaterialRole::Cosmetic,
                    &[&format!("F{}", index + 1)],
                    &[&format!("profile-{index}")],
                );
                requirement.material = match index {
                    0 => Material::Petg,
                    4 => Material::Abs,
                    _ => Material::Pla,
                };
                requirement.best_effort_cmyx_candidate =
                    Some(u1_planner::BestEffortCmyxCandidate {
                        candidate_id: format!("candidate-{index}"),
                        target_material: Material::Pla,
                        target_color: requirement.source_color,
                        candidate: u1_planner::CmyxColorCandidate {
                            recipe: CmyxRecipe::DedicatedT4,
                            calibration_sample_id: None,
                            process_compatibility: None,
                            required_t4_spool_id: Some("grey".to_owned()),
                            predicted_color: Some(RgbColor::new(30, 55, 80)),
                            delta_e00: Some(9.25),
                            confidence: ColorConfidence::Nominal,
                            warnings: vec!["Approval required.".to_owned()],
                        },
                    });
                if index == 0 {
                    requirement
                        .cmyx_palette_candidates
                        .push(u1_planner::BestEffortCmyxCandidate {
                            candidate_id: "candidate-0-cyan".to_owned(),
                            target_material: Material::Pla,
                            target_color: requirement.source_color,
                            candidate: u1_planner::CmyxColorCandidate {
                                recipe: CmyxRecipe::Solid {
                                    toolhead: Toolhead::T1,
                                },
                                calibration_sample_id: None,
                                process_compatibility: None,
                                required_t4_spool_id: None,
                                predicted_color: Some(RgbColor::new(8, 171, 251)),
                                delta_e00: Some(22.0),
                                confidence: ColorConfidence::Nominal,
                                warnings: vec!["Manual palette choice.".to_owned()],
                            },
                        });
                }
                requirement
            })
            .collect::<Vec<_>>();
        let mut scope = fallback_scope(std::mem::take(&mut requirements));
        scope.approved_cmyx_fallbacks = vec![CmyxFallbackApproval {
            requirement_id: "requirement-0".to_owned(),
            candidate_id: "candidate-0".to_owned(),
        }];
        scope.approved_material_substitutions = vec![MaterialSubstitutionApproval {
            requirement_id: "requirement-0".to_owned(),
            candidate_id: "candidate-0".to_owned(),
            source_material: Material::Petg,
            target_material: Material::Pla,
            acknowledged: true,
        }];
        let spools = [Spool {
            id: "grey".to_owned(),
            calibration_id: None,
            display_name: "Panchroma Grey".to_owned(),
            color_name: Some("Grey".to_owned()),
            material: Material::Pla,
            nominal_color: RgbColor::new(145, 153, 164),
            measured_color: None,
            sku: None,
            profile_id: None,
            available: true,
        }];
        let inventory = spools
            .iter()
            .map(|spool| (spool.id.as_str(), spool))
            .collect::<BTreeMap<_, _>>();

        let resolutions = color_resolutions(&[scope], &inventory);
        assert_eq!(resolutions.len(), 5);
        let json = serde_json::to_value(&resolutions[0]).unwrap();
        assert_eq!(json["scopeId"], "scope");
        assert_eq!(json["requirementId"], "requirement-0");
        assert_eq!(json["candidateId"], "candidate-0");
        assert_eq!(json["sourceMaterial"], "PETG");
        assert_eq!(json["targetMaterial"], "PLA");
        assert_eq!(json["predictedHex"], "#1E3750");
        assert_eq!(json["deltaE00"], 9.3);
        assert_eq!(json["requiredT4SpoolId"], "grey");
        assert_eq!(json["requiredT4Name"], "Panchroma Grey");
        assert_eq!(json["requiredT4Hex"], "#9199A4");
        assert_eq!(json["colorApproved"], true);
        assert_eq!(json["materialApproved"], true);
        assert_eq!(json["requiresMaterialSubstitution"], true);
        assert_eq!(json["canAddDedicatedSpool"], true);
        assert!(
            json["sourceIdentityKey"]
                .as_str()
                .is_some_and(|identity| identity.contains("cmyx-source-v1"))
        );
        assert_eq!(json["paletteOptions"].as_array().map(Vec::len), Some(2));
        assert_eq!(json["paletteOptions"][1]["candidateId"], "candidate-0-cyan");

        let unsupported_source = serde_json::to_value(&resolutions[4]).unwrap();
        assert_eq!(unsupported_source["sourceMaterial"], "ABS");
        assert_eq!(unsupported_source["targetMaterial"], "PLA");
        assert_eq!(unsupported_source["requiresMaterialSubstitution"], true);
        assert_eq!(unsupported_source["canAddDedicatedSpool"], false);
        assert!(
            unsupported_source["recommendation"]
                .as_str()
                .expect("recommendation must be text")
                .contains("Dedicated ABS spools are not supported")
        );
    }

    #[test]
    fn unresolved_fallback_preserves_effective_requirement_identity() {
        let declared = fallback_scope(vec![
            fallback_requirement(
                "basic-first",
                MaterialRole::Cosmetic,
                &["slot-1"],
                &["profile-b", "profile-a", "profile-a"],
            ),
            fallback_requirement(
                "basic-alias",
                MaterialRole::Cosmetic,
                &["slot-2"],
                &["profile-a", "profile-b"],
            ),
            fallback_requirement(
                "matte",
                MaterialRole::Cosmetic,
                &["slot-3"],
                &["profile-matte"],
            ),
            fallback_requirement(
                "support",
                MaterialRole::Support,
                &["slot-4"],
                &["profile-a", "profile-b"],
            ),
        ]);
        let mappings = unresolved_mappings(&declared, Some(&fallback_option(3)))
            .expect("declared identities must be available to the UI");
        assert_eq!(
            mappings
                .iter()
                .map(|mapping| mapping.id.as_str())
                .collect::<Vec<_>>(),
            vec!["basic-first", "matte", "support"]
        );

        let unknown = fallback_scope(vec![
            fallback_requirement(
                "unknown-first",
                MaterialRole::Cosmetic,
                &["slot-9", "slot-8"],
                &[""],
            ),
            fallback_requirement(
                "unknown-alias",
                MaterialRole::Cosmetic,
                &["slot-8", "slot-9"],
                &[],
            ),
            fallback_requirement(
                "unknown-other-slot",
                MaterialRole::Cosmetic,
                &["slot-10"],
                &[],
            ),
            fallback_requirement("unknown-id-a", MaterialRole::Cosmetic, &[], &[]),
            fallback_requirement("unknown-id-b", MaterialRole::Cosmetic, &[], &[]),
        ]);
        let mappings = unresolved_mappings(&unknown, Some(&fallback_option(4)))
            .expect("unknown identities must be available to the UI");
        assert_eq!(
            mappings
                .iter()
                .map(|mapping| mapping.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "unknown-first",
                "unknown-other-slot",
                "unknown-id-a",
                "unknown-id-b"
            ]
        );
        assert_eq!(
            mappings
                .iter()
                .map(|mapping| mapping.direct_toolhead)
                .collect::<Vec<_>>(),
            vec!["T1", "T2", "T3", "T4"]
        );
    }

    #[test]
    fn project_direct_palette_groups_effective_pairs_across_multiple_scopes() {
        let first_pair = fallback_requirement(
            "plate-1-grey",
            MaterialRole::Cosmetic,
            &["F6"],
            &["profile-grey"],
        );
        let mut second_pair = fallback_requirement(
            "plate-1-red",
            MaterialRole::Cosmetic,
            &["F8"],
            &["profile-red"],
        );
        second_pair.source_color = RgbColor::new(190, 35, 40);
        let mut first = fallback_scope(vec![first_pair.clone(), second_pair.clone()]);
        first.id = "plate-1".to_owned();
        first.display_name = "First plate".to_owned();
        first.direct_assignments = vec![
            DirectAssignmentRequest {
                requirement_id: "plate-1-grey".to_owned(),
                spool_id: "workshop-grey".to_owned(),
                toolhead: Some(Toolhead::T2),
                allow_material_substitution: false,
            },
            DirectAssignmentRequest {
                requirement_id: "plate-1-red".to_owned(),
                spool_id: "workshop-red".to_owned(),
                toolhead: Some(Toolhead::T3),
                allow_material_substitution: false,
            },
        ];
        make_scope_requirements_printable(&mut first);

        let mut second_grey = first_pair;
        second_grey.id = "plate-2-grey".to_owned();
        let mut second_red = second_pair;
        second_red.id = "plate-2-red".to_owned();
        second_red.cmyx_candidate.recipe = CmyxRecipe::Solid {
            toolhead: Toolhead::T2,
        };
        second_red.cmyx_candidate.predicted_color = Some(RgbColor::new(175, 45, 50));
        second_red.cmyx_candidate.delta_e00 = Some(7.25);
        let mut second = fallback_scope(vec![second_grey, second_red]);
        second.id = "plate-2".to_owned();
        second.display_name = "Second plate".to_owned();
        second.direct_assignments = vec![
            DirectAssignmentRequest {
                requirement_id: "plate-2-grey".to_owned(),
                spool_id: "workshop-grey".to_owned(),
                toolhead: Some(Toolhead::T2),
                allow_material_substitution: false,
            },
            DirectAssignmentRequest {
                requirement_id: "plate-2-red".to_owned(),
                spool_id: "workshop-red".to_owned(),
                toolhead: Some(Toolhead::T3),
                allow_material_substitution: false,
            },
        ];
        make_scope_requirements_printable(&mut second);

        let mut first_option = fallback_option(2);
        first_option.scope_id = "plate-1".to_owned();
        let mut second_option = fallback_option(2);
        second_option.scope_id = "plate-2".to_owned();
        let options = BTreeMap::from([("plate-1", &first_option), ("plate-2", &second_option)]);
        let palette = project_direct_palette(&[first, second], &options);
        assert!(palette.available);
        assert_eq!(palette.effective_pair_count, 2);
        assert_eq!(palette.direct_pair_count, 2);
        assert_eq!(palette.mappings.len(), 2);
        assert!(palette.unavailable_reason.is_none());
        assert!(
            palette
                .mappings
                .iter()
                .all(|mapping| mapping.references.len() == 2)
        );
        let grey = palette
            .mappings
            .iter()
            .find(|mapping| mapping.source_hex == "#19324B")
            .expect("grey pair");
        assert_eq!(grey.selected_spool_id, "workshop-grey");
        assert_eq!(grey.direct_toolhead, "T2");
        assert_eq!(grey.used_by, vec!["First plate", "Second plate"]);
        assert_eq!(grey.current_cmyx_results.len(), 2);
        assert_eq!(
            grey.references
                .iter()
                .map(|reference| reference.scope_id.as_str())
                .collect::<Vec<_>>(),
            vec!["plate-1", "plate-2"]
        );
        let red = palette
            .mappings
            .iter()
            .find(|mapping| mapping.source_hex == "#BE2328")
            .expect("red pair");
        assert_eq!(red.current_cmyx_results.len(), 2);
        assert_eq!(red.current_cmyx_results[0].recipe, "Solid T1");
        assert_eq!(red.current_cmyx_results[1].recipe, "Solid T2");
        assert_eq!(
            red.current_cmyx_results[1].predicted_hex.as_deref(),
            Some("#AF2D32")
        );
        assert_eq!(red.current_cmyx_results[1].delta_e00, Some(7.3));
    }

    #[test]
    fn project_direct_palette_keeps_five_role_rows_but_counts_four_physical_identities() {
        let definitions = [
            (
                "grey-cosmetic",
                MaterialRole::Cosmetic,
                [25, 50, 75],
                "profile-grey",
            ),
            (
                "grey-support",
                MaterialRole::Support,
                [25, 50, 75],
                "profile-grey",
            ),
            ("red", MaterialRole::Cosmetic, [190, 35, 40], "profile-red"),
            (
                "green",
                MaterialRole::Functional,
                [40, 150, 70],
                "profile-green",
            ),
            (
                "blue",
                MaterialRole::Cosmetic,
                [30, 70, 180],
                "profile-blue",
            ),
        ];
        let requirements = definitions
            .into_iter()
            .enumerate()
            .map(|(index, (id, role, [red, green, blue], profile))| {
                let mut requirement =
                    fallback_requirement(id, role, &[&format!("F{}", index + 1)], &[profile]);
                requirement.source_color = RgbColor::new(red, green, blue);
                requirement
            })
            .collect();
        let mut scope = fallback_scope(requirements);
        scope.id = "role-separated".to_owned();
        scope.display_name = "Role-separated plate".to_owned();
        make_scope_requirements_printable(&mut scope);

        let mut option = fallback_option(5);
        option.scope_id = scope.id.clone();
        option.direct_pair_count = 4;
        let plate_mappings = unresolved_mappings(&scope, Some(&option))
            .expect("four physical identities must expose all five semantic rows");
        assert_eq!(plate_mappings.len(), 5);
        let cosmetic_plate_mapping = plate_mappings
            .iter()
            .find(|mapping| mapping.id == "grey-cosmetic")
            .expect("cosmetic plate mapping");
        let support_plate_mapping = plate_mappings
            .iter()
            .find(|mapping| mapping.id == "grey-support")
            .expect("support plate mapping");
        assert_eq!(
            cosmetic_plate_mapping.physical_identity_id,
            support_plate_mapping.physical_identity_id
        );
        assert_eq!(
            cosmetic_plate_mapping.direct_toolhead,
            support_plate_mapping.direct_toolhead
        );
        let serialized_mapping = serde_json::to_value(cosmetic_plate_mapping)
            .expect("per-plate Direct mapping serializes");
        assert_eq!(
            serialized_mapping["physicalIdentityId"],
            cosmetic_plate_mapping.physical_identity_id
        );
        let options = BTreeMap::from([("role-separated", &option)]);
        let palette = project_direct_palette(&[scope], &options);

        assert!(palette.available);
        assert_eq!(palette.effective_pair_count, 5);
        assert_eq!(palette.direct_pair_count, 4);
        assert_eq!(palette.mappings.len(), 5);
        let cosmetic = palette
            .mappings
            .iter()
            .find(|mapping| mapping.source_role == "Cosmetic" && mapping.source_hex == "#19324B")
            .expect("cosmetic grey row");
        let support = palette
            .mappings
            .iter()
            .find(|mapping| mapping.source_role == "Support" && mapping.source_hex == "#19324B")
            .expect("support grey row");
        assert_eq!(cosmetic.physical_identity_id, support.physical_identity_id);
        assert_eq!(cosmetic.direct_toolhead, support.direct_toolhead);
    }

    #[test]
    fn custom_direct_palette_exposes_seven_sources_mapped_to_four_spools() {
        let requirements = (0..7)
            .map(|index| {
                let id = format!("source-{index}");
                let profile = format!("profile-{index}");
                let slot = format!("F{}", index + 1);
                let mut requirement =
                    fallback_requirement(&id, MaterialRole::Cosmetic, &[&slot], &[&profile]);
                requirement.source_color =
                    RgbColor::new(20 + index * 20, 40 + index * 10, 70 + index * 5);
                requirement
            })
            .collect::<Vec<_>>();
        let mut scope = fallback_scope(requirements.clone());
        scope.id = "seven-color-scope".to_owned();
        scope.display_name = "Seven color plate".to_owned();
        let assignments = requirements
            .iter()
            .enumerate()
            .map(
                |(index, requirement)| u1_planner::DirectToolheadAssignment {
                    source_requirement_ids: vec![requirement.id.clone()],
                    source_profile_ids: requirement.source_profile_ids.clone(),
                    source_material: requirement.material.clone(),
                    source_color: requirement.source_color,
                    toolhead: Toolhead::ALL[index % 4],
                    spool_id: format!("spool-{}", index % 4),
                    actual_color: requirement.source_color,
                    delta_e00: Some(0.0),
                    confidence: ColorConfidence::Nominal,
                    status: u1_planner::MappingStatus::Exact,
                },
            )
            .collect();
        let option = ScopeStrategyOptions {
            scope_id: scope.id.clone(),
            effective_pair_count: 7,
            direct_pair_count: 7,
            cmyx_available: true,
            direct_spools: DirectSpoolEligibility::Eligible { assignments },
            selected_strategy: ScopeStrategy::Auto,
        };

        let mappings = unresolved_mappings(&scope, Some(&option))
            .expect("the reduced four-spool palette must remain editable");

        assert_eq!(mappings.len(), 7);
        assert_eq!(
            mappings
                .iter()
                .map(|mapping| mapping.selected_spool_id.as_str())
                .collect::<BTreeSet<_>>()
                .len(),
            4
        );
        assert!(
            mappings
                .iter()
                .all(|mapping| !mapping.inheritance_key.is_empty())
        );
        let scope_ids = vec![scope.id.clone()];
        let options = BTreeMap::from([(scope.id.as_str(), &option)]);
        assert_eq!(direct_status(&scope_ids, &options), (true, None));
    }

    #[test]
    fn project_direct_palette_reports_the_exact_over_four_reason() {
        let requirements = (0..5)
            .map(|index| {
                let mut requirement = fallback_requirement(
                    &format!("pair-{index}"),
                    MaterialRole::Cosmetic,
                    &[&format!("F{}", index + 1)],
                    &[&format!("profile-{index}")],
                );
                requirement.source_color = RgbColor::new(index as u8, 40, 80);
                requirement
            })
            .collect();
        let mut scope = fallback_scope(requirements);
        scope.id = "five-pair-scope".to_owned();
        make_scope_requirements_printable(&mut scope);

        let mut option = fallback_option(5);
        option.scope_id = "five-pair-scope".to_owned();
        let options = BTreeMap::from([("five-pair-scope", &option)]);
        let palette = project_direct_palette(&[scope], &options);
        assert!(!palette.available);
        assert_eq!(palette.effective_pair_count, 5);
        assert_eq!(palette.direct_pair_count, 5);
        assert_eq!(palette.maximum_pair_count, 4);
        assert_eq!(palette.mappings.len(), 5);
        assert_eq!(
            palette.unavailable_reason.as_deref(),
            Some(
                "Project-wide Direct Spools is unavailable because 5 semantic material-color-role pairs require 5 unique physical Direct identities; the maximum is 4."
            )
        );
    }

    #[test]
    #[ignore = "slow real-project contract fixture"]
    fn native_view_maps_the_real_sample_without_inventing_spools() {
        let sample =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../Sample/Withered_Foxy.3mf");
        let sample_path = sample.to_str().expect("UTF-8 sample path");
        let (analysis, view) =
            analyze_project_data(sample_path).expect("sample must map to the frontend view");

        assert_eq!(view.summary.source_plate_count, 12);
        assert_eq!(view.summary.object_count, 89);
        assert_eq!(view.summary.used_filament_count, 10);
        assert!(!view.alternative_plates.is_empty());
        assert!(view.alternative_plates.iter().all(|plate| !plate.included));
        assert!(
            view.spools
                .iter()
                .all(|spool| !spool.id.starts_with("source-"))
        );
        assert!(view.plates.iter().any(|plate| plate.direct_eligible));
        assert!(view.plates.iter().any(|plate| !plate.direct_eligible));
        assert!(view.color_resolutions.iter().any(|resolution| {
            resolution.scope_id == "plate-1"
                && resolution.source_hex == "#8E9089"
                && !resolution.requires_material_substitution
        }));

        // Direct Spools is now the safe default for scopes that could preserve
        // their source material. The CMY+X fallback (and its separate material
        // acknowledgement) becomes actionable only after the operator chooses
        // CMY+X, matching the two-step desktop workflow.
        let cmyx_request: PlanningRequestView = serde_json::from_value(serde_json::json!({
            "scopeOverrides": view
                .scope_selections
                .iter()
                .map(|selection| serde_json::json!({
                    "scopeId": selection.scope_id,
                    "strategy": "cmyx",
                    "assignments": [],
                    "approvedColorFallbacks": [],
                    "materialSubstitutions": [],
                }))
                .collect::<Vec<_>>(),
        }))
        .expect("real-sample CMY+X selection must match the native request contract");
        let cmyx_view = replan_project_view(sample_path, &analysis, cmyx_request)
            .expect("real-sample CMY+X selection must expose fallback decisions");
        assert!(cmyx_view.color_resolutions.iter().any(|resolution| {
            resolution.source_material == "PETG"
                && resolution.target_material == "PLA"
                && resolution.requires_material_substitution
        }));

        let scope_overrides = cmyx_view
            .scope_selections
            .iter()
            .map(|selection| {
                let resolutions = cmyx_view
                    .color_resolutions
                    .iter()
                    .filter(|resolution| resolution.scope_id == selection.scope_id)
                    .collect::<Vec<_>>();
                serde_json::json!({
                    "scopeId": selection.scope_id,
                    "strategy": "cmyx",
                    "assignments": [],
                    "approvedColorFallbacks": resolutions
                        .iter()
                        .map(|resolution| serde_json::json!({
                            "requirementId": resolution.requirement_id,
                            "candidateId": resolution.candidate_id,
                        }))
                        .collect::<Vec<_>>(),
                    "materialSubstitutions": resolutions
                        .iter()
                        .filter(|resolution| resolution.requires_material_substitution)
                        .map(|resolution| serde_json::json!({
                            "requirementId": resolution.requirement_id,
                            "candidateId": resolution.candidate_id,
                            "sourceMaterial": resolution.source_material,
                            "targetMaterial": resolution.target_material,
                            "acknowledged": true,
                        }))
                        .collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>();
        let request: PlanningRequestView = serde_json::from_value(serde_json::json!({
            "scopeOverrides": scope_overrides,
        }))
        .expect("real-sample best-effort approvals must match the native request contract");
        let accepted = replan_project_view(sample_path, &analysis, request)
            .expect("accepted real-sample fallbacks must replan");
        eprintln!(
            "accepted sample plan: plates={}, batches={}, t4_swaps={}, a1_spool_changes={}",
            accepted.plates.len(),
            accepted.batches.len(),
            accepted.t4_swap_count,
            accepted.a1_spool_change_count
        );
        for batch in &accepted.batches {
            eprintln!(
                "batch {}: {} ({}), orders {}-{}, actions={:?}",
                batch.id,
                batch.label,
                batch.detail,
                batch.start_order,
                batch.end_order,
                batch.setup_actions
            );
        }
        assert!(
            accepted.blocking_errors.is_empty(),
            "{:#?}",
            accepted.blocking_errors
        );
        assert_eq!(accepted.omitted_unit_count, 0);
        assert!(accepted.plan_ready);
        assert_eq!(accepted.plates.len(), 8);
        assert_eq!(accepted.batches.len(), 3);
        assert_eq!(accepted.t4_swap_count, 1);
        assert!(accepted.batches[0].t4_change.is_none());
        assert!(accepted.batches[1].t4_change.is_none());
        assert_eq!(accepted.batches[2].start_order, 3);
        let change = accepted.batches[2]
            .t4_change
            .as_ref()
            .expect("third batch must expose the Grey to Black T4 boundary");
        assert_eq!(
            change.from_spool_id.as_deref(),
            Some("panchroma-translucent-grey")
        );
        assert_eq!(change.to_spool_id.as_deref(), Some("panchroma-basic-black"));
    }
}
