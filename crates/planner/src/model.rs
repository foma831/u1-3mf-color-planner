use serde::{Deserialize, Serialize};

/// An sRGB color with eight-bit channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RgbColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl RgbColor {
    #[must_use]
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }
}

/// A printable polymer family. Unknown families remain distinct by name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Material {
    Pla,
    Petg,
    Abs,
    Asa,
    Tpu,
    Other(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterialRole {
    Cosmetic,
    Functional,
    Support,
    Unknown,
}

/// The physical U1 toolhead slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Toolhead {
    T1,
    T2,
    T3,
    T4,
}

impl Toolhead {
    pub const ALL: [Self; 4] = [Self::T1, Self::T2, Self::T3, Self::T4];

    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::T1 => 0,
            Self::T2 => 1,
            Self::T3 => 2,
            Self::T4 => 3,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "spool_id")]
pub enum ToolheadSlotState {
    Unknown,
    Empty,
    Loaded(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentToolheadState {
    /// Slots in T1, T2, T3, T4 order.
    pub slots: [ToolheadSlotState; 4],
}

impl Default for CurrentToolheadState {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| ToolheadSlotState::Unknown),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Spool {
    pub id: String,
    /// Stable physical/calibration-batch identity used for measured CMY+X
    /// evidence. It may differ from the catalogue ID when a spool's lot or
    /// calibration reference changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration_id: Option<String>,
    pub display_name: String,
    #[serde(default)]
    pub color_name: Option<String>,
    pub material: Material,
    pub nominal_color: RgbColor,
    pub measured_color: Option<RgbColor>,
    #[serde(default)]
    pub sku: Option<String>,
    pub profile_id: Option<String>,
    pub available: bool,
}

impl Spool {
    #[must_use]
    pub fn actual_color(&self) -> RgbColor {
        self.measured_color.unwrap_or(self.nominal_color)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorConfidence {
    Measured,
    Calibrated,
    Nominal,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FullSpectrumMode {
    Gradient,
    Ratio,
    Match,
    Cycle,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FullSpectrumSubdivisionPolicy {
    Disabled,
    SubdivideMixLayer,
    AdapterDefined(String),
}

/// Process contract required to reproduce an optical Full Spectrum recipe.
/// Integer micrometres make equality stable across serialization boundaries.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FullSpectrumProcessCompatibility {
    pub printer_profile_fingerprint: String,
    pub plate_layer_height_microns: u32,
    pub subdivision_policy: FullSpectrumSubdivisionPolicy,
    pub subdivision_factor: u8,
    pub effective_sublayer_height_microns: u32,
    /// Stable identity for temperatures, flow and every other process setting
    /// that can change the optical result.
    pub process_fingerprint: String,
}

/// A color recipe already selected by the color engine for CMY+X planning.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CmyxRecipe {
    Solid {
        toolhead: Toolhead,
    },
    FullSpectrum {
        mode: FullSpectrumMode,
        sequence: Vec<Toolhead>,
    },
    DedicatedT4,
    Fallback {
        description: String,
        physical_slots: Vec<Toolhead>,
    },
    ManualReview {
        reason: String,
    },
    Unreachable {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CmyxColorCandidate {
    pub recipe: CmyxRecipe,
    /// Exact user-confirmed measurement used for this prediction. Nominal and
    /// otherwise uncalibrated candidates deliberately omit this provenance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration_sample_id: Option<String>,
    /// Required for a mixed `FullSpectrum` recipe. Solid recipes remain valid
    /// regardless of this field because they do not rely on layer subdivision.
    pub process_compatibility: Option<FullSpectrumProcessCompatibility>,
    /// Required only when the recipe consumes T4.
    pub required_t4_spool_id: Option<String>,
    pub predicted_color: Option<RgbColor>,
    pub delta_e00: Option<f64>,
    pub confidence: ColorConfidence,
    pub warnings: Vec<String>,
}

/// A schedulable CMY+X candidate that was deliberately withheld from
/// automatic planning (for example, because its nominal color distance is
/// poor or because it changes the source material family).
///
/// `candidate_id` is an application-generated decision fingerprint. An
/// approval must echo it so a stale UI cannot accidentally approve a newly
/// generated recipe after inventory or calibration data changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BestEffortCmyxCandidate {
    pub candidate_id: String,
    pub target_material: Material,
    pub target_color: RgbColor,
    pub candidate: CmyxColorCandidate,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CmyxFallbackApproval {
    pub requirement_id: String,
    pub candidate_id: String,
}

/// A separate, explicit acknowledgement that a best-effort color recipe will
/// print one source requirement with a different polymer family.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterialSubstitutionApproval {
    pub requirement_id: String,
    pub candidate_id: String,
    pub source_material: Material,
    pub target_material: Material,
    pub acknowledged: bool,
}

/// A source-specific inventory match computed by a color engine or selected by
/// a user. When absent, the planner can still make a nominal RGB-based choice.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DirectSpoolCandidate {
    pub spool_id: String,
    pub delta_e00: Option<f64>,
    pub confidence: ColorConfidence,
}

/// One effective source material-color requirement. Multiple source slot IDs
/// may point to the same requirement.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaterialColorRequirement {
    pub id: String,
    pub material: Material,
    pub role: MaterialRole,
    pub source_color: RgbColor,
    pub source_slots: Vec<String>,
    /// Source filament preset/profile identities that must not be aliased.
    #[serde(default)]
    pub source_profile_ids: Vec<String>,
    pub cmyx_candidate: CmyxColorCandidate,
    /// Closest backend-generated candidate that requires explicit approval.
    #[serde(default)]
    pub best_effort_cmyx_candidate: Option<BestEffortCmyxCandidate>,
    pub direct_candidates: Vec<DirectSpoolCandidate>,
}

/// An axis-aligned size plus conservative clearance on both sides of each axis.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BoundsMm {
    pub width: f64,
    pub depth: f64,
    pub height: f64,
    pub clearance_x: f64,
    pub clearance_y: f64,
    pub clearance_z: f64,
}

impl BoundsMm {
    #[must_use]
    pub const fn from_size(width: f64, depth: f64, height: f64) -> Self {
        Self {
            width,
            depth,
            height,
            clearance_x: 0.0,
            clearance_y: 0.0,
            clearance_z: 0.0,
        }
    }

    #[must_use]
    pub fn required_volume(self) -> BuildVolumeMm {
        BuildVolumeMm {
            width: self.width + 2.0 * self.clearance_x,
            depth: self.depth + 2.0 * self.clearance_y,
            height: self.height + 2.0 * self.clearance_z,
        }
    }

    #[must_use]
    pub fn is_valid(self) -> bool {
        [
            self.width,
            self.depth,
            self.height,
            self.clearance_x,
            self.clearance_y,
            self.clearance_z,
        ]
        .into_iter()
        .all(|value| value.is_finite() && value >= 0.0)
    }

    /// Zero-sized geometry is accepted as an explicit "bounds unavailable"
    /// sentinel for parser milestones that cannot calculate mesh bounds yet.
    #[must_use]
    pub fn has_known_size(self) -> bool {
        self.width > 0.0 && self.depth > 0.0 && self.height > 0.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BuildVolumeMm {
    pub width: f64,
    pub depth: f64,
    pub height: f64,
}

impl BuildVolumeMm {
    #[must_use]
    pub fn contains(self, required: Self) -> bool {
        required.width <= self.width
            && required.depth <= self.depth
            && required.height <= self.height
    }

    #[must_use]
    pub fn is_valid(self) -> bool {
        [self.width, self.depth, self.height]
            .into_iter()
            .all(|value| value.is_finite() && value > 0.0)
    }
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PrinterPreference {
    #[default]
    Auto,
    U1,
    A1Mini,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrintableUnit {
    pub id: String,
    /// Stable identity from the immutable source project. Unlike `id`, this
    /// value is not rewritten when a source plate is split into planning scopes.
    pub source_unit_id: String,
    /// Object resource ID exactly as serialized in the source package. The
    /// analyzer may use a different package-wide ID when Production Extension
    /// parts reuse local resource IDs, so the writer must retain both notions.
    pub source_object_id: u32,
    /// Bambu/Orca zero-based instance ID for this object's build item.
    pub source_instance_id: u32,
    /// Production Extension part containing the source object. `None` denotes
    /// the primary `3D/3dmodel.model` part.
    #[serde(default)]
    pub source_model_path: Option<String>,
    pub display_name: String,
    pub source_plate_id: Option<String>,
    pub requirement_ids: Vec<String>,
    pub bounds: BoundsMm,
    pub source_layer_height_mm: Option<f64>,
    pub printer_preference: PrinterPreference,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ScopeStrategy {
    #[default]
    Auto,
    CmyxFullSpectrum,
    DirectSpools,
}

/// A user override for one semantic source requirement. Aliases, plus
/// role-separated requirements with the same material, color, and declared
/// source profile, must resolve to the same physical spool and toolhead.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectAssignmentRequest {
    pub requirement_id: String,
    pub spool_id: String,
    pub toolhead: Option<Toolhead>,
    /// Explicit operator acknowledgement for a forced Direct Spools mapping
    /// that changes the source polymer family. Automatic matching never uses
    /// this flag and continues to preserve material identity.
    #[serde(default)]
    pub allow_material_substitution: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrintScope {
    pub id: String,
    pub display_name: String,
    pub requirements: Vec<MaterialColorRequirement>,
    pub units: Vec<PrintableUnit>,
    pub strategy: ScopeStrategy,
    pub direct_assignments: Vec<DirectAssignmentRequest>,
    #[serde(default)]
    pub approved_cmyx_fallbacks: Vec<CmyxFallbackApproval>,
    #[serde(default)]
    pub approved_material_substitutions: Vec<MaterialSubstitutionApproval>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CmySetup {
    pub cyan_spool_id: String,
    pub magenta_spool_id: String,
    pub yellow_spool_id: String,
    pub default_t4_spool_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct A1MiniConfig {
    pub enabled: bool,
    pub route_fast_mono: bool,
    pub build_volume: BuildVolumeMm,
    pub supported_materials: Vec<Material>,
    /// Physical spools explicitly unavailable to the A1 mini. The canonical
    /// schedule is sequential, so a spool may otherwise move between the U1
    /// and A1 mini when the setup actions say so.
    #[serde(default)]
    pub reserved_spool_ids: Vec<String>,
    /// Physical spool currently loaded on the A1 mini. This only determines
    /// the first setup transition and never limits later A1 plate colors.
    #[serde(default)]
    pub current_spool_id: Option<String>,
}

impl Default for A1MiniConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            route_fast_mono: true,
            build_volume: BuildVolumeMm {
                width: 180.0,
                depth: 180.0,
                height: 180.0,
            },
            supported_materials: vec![Material::Pla, Material::Petg],
            reserved_spool_ids: Vec::new(),
            current_spool_id: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlannerConfig {
    pub cmy_setup: CmySetup,
    /// Exact target-process contracts supported by the selected Orca adapter.
    /// A mixed recipe is schedulable only when its contract appears here.
    pub supported_full_spectrum_processes: Vec<FullSpectrumProcessCompatibility>,
    pub u1_build_volume: BuildVolumeMm,
    pub a1_mini: A1MiniConfig,
    /// Expert override. It is intentionally false by default.
    pub allow_mixed_materials_on_plate: bool,
    /// Explicit opt-in for a lossy per-scope Direct palette. When enabled,
    /// more than four source physical identities may intentionally share at
    /// most four physical spools and U1 toolheads.
    #[serde(default)]
    pub allow_direct_palette_reduction: bool,
    /// Explicit opt-in for deterministic U1 packing across source-plate
    /// boundaries. Units still share a target plate only when their complete
    /// physical job contract (strategy, loadout, materials and process) is
    /// identical and their cleared geometry fits the qualified bed envelope.
    #[serde(default)]
    pub allow_u1_cross_source_repacking: bool,
    pub restore_cmy_after_direct: bool,
}

impl PlannerConfig {
    #[must_use]
    pub fn with_cmy_setup(cmy_setup: CmySetup) -> Self {
        Self {
            cmy_setup,
            supported_full_spectrum_processes: Vec::new(),
            u1_build_volume: BuildVolumeMm {
                width: 270.0,
                depth: 270.0,
                height: 270.0,
            },
            a1_mini: A1MiniConfig::default(),
            allow_mixed_materials_on_plate: false,
            allow_direct_palette_reduction: false,
            allow_u1_cross_source_repacking: false,
            restore_cmy_after_direct: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanningInput {
    pub scopes: Vec<PrintScope>,
    pub inventory: Vec<Spool>,
    pub current_toolheads: CurrentToolheadState,
    pub config: PlannerConfig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorStrategy {
    CmyxFullSpectrum,
    /// Uses the fixed CMY+X physical loadout, but every selected color is a
    /// direct solid physical toolhead assignment. No Full Spectrum process or
    /// virtual mixed-filament definition is required.
    CmyxSolid,
    DirectSpools,
    A1Mono,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Printer {
    U1,
    A1Mini,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct U1Loadout {
    /// Physical spool IDs in T1, T2, T3, T4 order. An unused Direct Spool slot
    /// is `None`; a CMY+X plan always fixes T1-T3.
    pub slots: [Option<String>; 4],
}

impl U1Loadout {
    #[must_use]
    pub fn spool(&self, toolhead: Toolhead) -> Option<&str> {
        self.slots[toolhead.index()].as_deref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "printer")]
pub enum PrinterLoadout {
    U1 { loadout: U1Loadout },
    A1Mini { spool_id: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MappingStatus {
    Exact,
    Close,
    Review,
    Poor,
    MaterialMismatch,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceToActualMapping {
    pub scope_id: String,
    pub source_requirement_ids: Vec<String>,
    pub source_slots: Vec<String>,
    #[serde(default)]
    pub source_profile_ids: Vec<String>,
    pub source_material: Material,
    pub source_color: RgbColor,
    pub strategy: ColorStrategy,
    /// Included in every strategy so the UI can compare CMY+X and Direct Spool.
    pub cmyx_comparison: CmyxColorCandidate,
    pub direct_toolhead: Option<Toolhead>,
    pub actual_spool_id: Option<String>,
    pub actual_material: Option<Material>,
    pub actual_color: Option<RgbColor>,
    pub delta_e00: Option<f64>,
    pub confidence: ColorConfidence,
    pub status: MappingStatus,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DirectToolheadAssignment {
    pub source_requirement_ids: Vec<String>,
    #[serde(default)]
    pub source_profile_ids: Vec<String>,
    pub source_material: Material,
    pub source_color: RgbColor,
    pub toolhead: Toolhead,
    pub spool_id: String,
    pub actual_color: RgbColor,
    pub delta_e00: Option<f64>,
    pub confidence: ColorConfidence,
    pub status: MappingStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reason")]
pub enum DirectIneligibility {
    TooManyEffectivePairs { count: usize, maximum: usize },
    NoEffectivePairs,
    MissingCompatibleSpools { material: Material },
    InvalidManualAssignment { message: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum DirectSpoolEligibility {
    Eligible {
        assignments: Vec<DirectToolheadAssignment>,
    },
    Ineligible {
        reason: DirectIneligibility,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScopeStrategyOptions {
    pub scope_id: String,
    /// Semantic source requirements. Material role remains part of this
    /// identity so CMY+X decisions and operator acknowledgements stay scoped
    /// to the exact source use.
    pub effective_pair_count: usize,
    /// Physical one-to-one identities that must occupy U1 toolheads in Direct
    /// Spools mode. Cosmetic/support uses of the same material, color, and
    /// declared source profile share one physical identity.
    pub direct_pair_count: usize,
    pub cmyx_available: bool,
    pub direct_spools: DirectSpoolEligibility,
    pub selected_strategy: ScopeStrategy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Estimate<T> {
    Estimated(T),
    RequiresSlicing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackingStatus {
    /// Individual object bounds were checked, but objects were not positioned.
    RequiresGeometryPacking,
    /// Every unit has a deterministic translation-only AABB placement with
    /// conservative process clearance inside the target build volume.
    PackedAabb,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ScopedUnitRef {
    pub scope_id: String,
    pub unit_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlannedPlacement {
    pub unit: ScopedUnitRef,
    /// Target X coordinate of the printable geometry AABB minimum. The writer
    /// derives a translation from the immutable source-instance AABB.
    pub target_min_x_mm: f64,
    /// Target Y coordinate of the printable geometry AABB minimum. Object
    /// scale, rotation, and Z translation remain unchanged.
    pub target_min_y_mm: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlannedPrimeTower {
    /// Target lower-left X coordinate of the prime-tower body. The packer
    /// reserves the qualified brim and object-clearance envelope around it.
    pub x_mm: f64,
    /// Target lower-left Y coordinate of the prime-tower body.
    pub y_mm: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlannedJob {
    pub id: String,
    pub scope_ids: Vec<String>,
    pub units: Vec<ScopedUnitRef>,
    pub printer: Printer,
    pub strategy: ColorStrategy,
    pub loadout: PrinterLoadout,
    pub printable_materials: Vec<Material>,
    pub fast_mono: bool,
    pub full_spectrum_process: Option<FullSpectrumProcessCompatibility>,
    pub color_mappings: Vec<SourceToActualMapping>,
    pub estimated_tool_changes: Estimate<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlannedPlate {
    pub id: String,
    pub job_id: String,
    pub printer: Printer,
    pub units: Vec<ScopedUnitRef>,
    pub placements: Vec<PlannedPlacement>,
    pub prime_tower: Option<PlannedPrimeTower>,
    pub packing_status: PackingStatus,
    pub individual_bounds_validated: bool,
    pub full_spectrum_process: Option<FullSpectrumProcessCompatibility>,
    pub estimated_print_time_seconds: Estimate<u64>,
    pub estimated_material_grams: Estimate<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupActionKind {
    Keep,
    Unload,
    Load,
    Restore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupPhase {
    BeforeBatch,
    AfterBatch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupAction {
    pub phase: SetupPhase,
    pub kind: SetupActionKind,
    pub toolhead: Option<Toolhead>,
    pub from: ToolheadSlotState,
    pub to: ToolheadSlotState,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlannedBatch {
    pub id: String,
    pub printer: Printer,
    pub strategy: ColorStrategy,
    pub loadout: PrinterLoadout,
    pub job_ids: Vec<String>,
    pub plate_ids: Vec<String>,
    pub setup_actions: Vec<SetupAction>,
    /// Standard CMY+X (mixed or solid-only) T4 transitions only. Direct setup
    /// actions are separate.
    pub t4_swap_before: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarningCode {
    PackingNotPerformed,
    PackingSplitAcrossPlates,
    UnknownBounds,
    DirectAlternativeUnavailable,
    DirectColorDistanceUnavailable,
    NominalSpoolColor,
    A1OutOfBounds,
    A1UnsupportedMaterial,
    A1NotSinglePhysicalSpool,
    A1PinnedUnitRejected,
    A1SpoolReservedForU1,
    A1SpoolSharedWithU1,
    UnknownCurrentToolhead,
    FallbackRecipe,
    ApprovedColorFallback,
    ApprovedMaterialSubstitution,
    IntentionalColorMerge,
    MaterialFamiliesSeparated,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanWarning {
    pub code: WarningCode,
    pub scope_id: Option<String>,
    pub unit_id: Option<String>,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    DuplicateScopeId,
    DuplicateRequirementId,
    DuplicateUnitId,
    DuplicateSpoolId,
    UnknownRequirement,
    UnknownSpool,
    InvalidBounds,
    InvalidBuildVolume,
    RequestedDirectSpoolsUnavailable,
    RequestedA1Unavailable,
    CmyxRecipeUnavailable,
    InvalidCmyxRecipe,
    InvalidCmyxFallbackApproval,
    MaterialSubstitutionApprovalRequired,
    FullSpectrumProcessMismatch,
    MultipleT4SpoolsInUnit,
    MaterialMismatch,
    MixedPrintableMaterials,
    UnitOutOfBounds,
    PackingFailed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanError {
    pub code: ErrorCode,
    pub scope_id: Option<String>,
    pub unit_id: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanningResult {
    pub scope_options: Vec<ScopeStrategyOptions>,
    pub jobs: Vec<PlannedJob>,
    pub plates: Vec<PlannedPlate>,
    pub batches: Vec<PlannedBatch>,
    pub t4_swap_count: u32,
    pub a1_spool_change_count: u32,
    pub final_toolheads: CurrentToolheadState,
    pub warnings: Vec<PlanWarning>,
    pub errors: Vec<PlanError>,
}

impl PlanningResult {
    #[must_use]
    pub fn has_hard_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}
