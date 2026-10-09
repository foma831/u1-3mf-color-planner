use serde::{Deserialize, Serialize};

fn default_max_model_entry_uncompressed_bytes() -> u64 {
    1024 * 1024 * 1024
}

fn default_max_xml_token_bytes() -> usize {
    8 * 1024 * 1024
}

/// Resource limits applied before any ZIP entry is decompressed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnalysisLimits {
    pub max_entries: usize,
    pub max_entry_name_bytes: usize,
    /// Maximum uncompressed size of a single non-model ZIP entry.
    pub max_entry_uncompressed_bytes: u64,
    /// Maximum uncompressed size of one 3MF model XML part below `3D/`.
    ///
    /// Model parts are parsed as streams and can legitimately be much larger
    /// than package metadata. The archive-wide and compression-ratio limits
    /// still apply independently.
    #[serde(default = "default_max_model_entry_uncompressed_bytes")]
    pub max_model_entry_uncompressed_bytes: u64,
    pub max_total_uncompressed_bytes: u64,
    pub max_compression_ratio: f64,
    pub max_config_bytes: u64,
    pub max_relationship_bytes: u64,
    pub max_xml_depth: usize,
    /// Maximum raw bytes in one XML lexical token. This bounds quick-xml's
    /// per-event buffer even for very large streamed model parts.
    #[serde(default = "default_max_xml_token_bytes")]
    pub max_xml_token_bytes: usize,
    pub max_paint_annotation_chars: usize,
}

impl Default for AnalysisLimits {
    fn default() -> Self {
        Self {
            max_entries: 4_096,
            max_entry_name_bytes: 1_024,
            max_entry_uncompressed_bytes: 512 * 1024 * 1024,
            max_model_entry_uncompressed_bytes: default_max_model_entry_uncompressed_bytes(),
            max_total_uncompressed_bytes: 2 * 1024 * 1024 * 1024,
            max_compression_ratio: 200.0,
            max_config_bytes: 64 * 1024 * 1024,
            max_relationship_bytes: 8 * 1024 * 1024,
            max_xml_depth: 256,
            max_xml_token_bytes: default_max_xml_token_bytes(),
            max_paint_annotation_chars: 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectAnalysis {
    pub input: InputIdentity,
    pub source: SourceInformation,
    pub archive: ArchiveStatistics,
    pub printer: PrinterInformation,
    pub process: ProcessInformation,
    pub filaments: Vec<DeclaredFilament>,
    pub effective_material_colors: Vec<EffectiveMaterialColor>,
    pub plates: Vec<PlateAnalysis>,
    pub objects: Vec<ObjectAnalysis>,
    pub summary: AnalysisSummary,
    pub warnings: Vec<AnalysisWarning>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputIdentity {
    pub byte_size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceInformation {
    pub application: SourceApplication,
    pub application_name: Option<String>,
    pub application_version: Option<String>,
    pub title: Option<String>,
    pub dialect: ProjectDialect,
    pub dialect_version: Option<String>,
    pub support: DialectSupport,
    pub has_bambu_or_orca_metadata: bool,
    pub has_sliced_artifacts: bool,
    pub sliced_artifact_entries: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceApplication {
    BambuStudio,
    OrcaSlicer,
    SnapmakerOrca,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectDialect {
    BambuStudioProject,
    OrcaSlicerProject,
    SnapmakerOrcaProject,
    Standard3mf,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DialectSupport {
    Supported,
    Experimental,
    Limited,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArchiveStatistics {
    pub entry_count: usize,
    pub archive_bytes: u64,
    pub total_compressed_bytes: u64,
    pub total_uncompressed_bytes: u64,
    pub largest_entry_uncompressed_bytes: u64,
    pub maximum_compression_ratio: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PrinterInformation {
    pub model: Option<String>,
    pub variant: Option<String>,
    pub nozzle_diameters_mm: Vec<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProcessInformation {
    pub name: Option<String>,
    pub layer_height_mm: Option<f64>,
    pub initial_layer_height_mm: Option<f64>,
    pub prime_tower_enabled: Option<bool>,
    #[serde(default)]
    pub adhesion: DetectedAdhesionPolicy,
    #[serde(default)]
    pub quality: QualityInformation,
    #[serde(default)]
    pub support: SupportInformation,
}

/// Adhesion settings recovered from a project profile written by U1 Planner.
/// The mode is recognized only when every relevant setting still matches the
/// profile contract; the profile name alone is never trusted.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DetectedAdhesionPolicy {
    pub mode: DetectedAdhesionMode,
    pub profile_name: Option<String>,
    pub brim_type: Option<String>,
    pub brim_width_mm: Option<f64>,
    pub brim_object_gap_mm: Option<f64>,
    pub raft_layers: Option<f64>,
    pub raft_first_layer_expansion_mm: Option<f64>,
    pub slow_layers: Option<f64>,
    pub initial_layer_speed_mm_s: Option<f64>,
    pub initial_layer_infill_speed_mm_s: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectedAdhesionMode {
    Standard,
    Reliable,
    Maximum,
    Custom,
    #[default]
    None,
}

/// Target-independent quality ceilings recovered from the source process.
/// Writers may make a target profile slower or more detailed to honor these
/// values, but must not use them to exceed a qualified target limit.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QualityInformation {
    pub wall_generator: Option<WallGenerator>,
    pub outer_wall_speed_mm_s: Option<f64>,
    pub inner_wall_speed_mm_s: Option<f64>,
    pub top_surface_speed_mm_s: Option<f64>,
    pub outer_wall_acceleration_mm_s2: Option<f64>,
    pub wall_loops: Option<u16>,
    pub top_shell_layers: Option<u16>,
    pub bottom_shell_layers: Option<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WallGenerator {
    Classic,
    Arachne,
}

impl WallGenerator {
    #[must_use]
    pub const fn slicer_value(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Arachne => "arachne",
        }
    }
}

/// Safe, target-independent support-generation intent recovered from the
/// source project. Target writers translate this intent onto their qualified
/// process profiles instead of copying an arbitrary slicer profile wholesale.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupportInformation {
    pub enabled: Option<bool>,
    pub support_type: Option<SupportType>,
    pub threshold_angle_degrees: Option<u8>,
    pub on_build_plate_only: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportType {
    NormalAuto,
    TreeAuto,
}

impl SupportType {
    #[must_use]
    pub const fn slicer_value(self) -> &'static str {
        match self {
            Self::NormalAuto => "normal(auto)",
            Self::TreeAuto => "tree(auto)",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclaredFilament {
    /// One-based logical source slot.
    pub slot: u16,
    pub material: Option<String>,
    pub color: Option<String>,
    pub preset: Option<String>,
    pub used: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveMaterialColor {
    pub source_slots: Vec<u16>,
    /// Distinct source filament preset/profile IDs from `filament_settings_id`.
    #[serde(default)]
    pub source_profile_ids: Vec<String>,
    pub material: Option<String>,
    pub color: Option<String>,
    pub roles: Vec<MaterialRole>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterialRole {
    Model,
    Support,
    SupportInterface,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlateAnalysis {
    pub id: u32,
    pub name: Option<String>,
    pub instances: Vec<ObjectInstance>,
    /// Bounds of all printable instances in source plate coordinates.
    /// `None` means at least one printable instance has unknown bounds.
    #[serde(default)]
    pub printable_bounds: Option<AxisAlignedBounds>,
    pub object_count: usize,
    pub part_count: usize,
    pub effective_slots: Vec<u16>,
    pub effective_material_colors: Vec<EffectiveMaterialColor>,
    pub classification: ColorClassification,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ObjectInstance {
    pub object_id: u32,
    pub instance_id: u32,
    /// Zero-based ordinal of the immutable `<build><item>` entry in the
    /// source model. This is the canonical identity of a printable copy;
    /// plate grouping and planning scope IDs are deliberately excluded.
    #[serde(default)]
    pub source_build_item_index: Option<u32>,
    pub identify_id: Option<u64>,
    pub printable: bool,
    pub transform: Option<Transform3mf>,
    /// Printable positive geometry after the source build-item transform.
    #[serde(default)]
    pub printable_bounds: Option<AxisAlignedBounds>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ObjectAnalysis {
    /// Analyzer-stable object ID. It equals the source resource ID unless that
    /// ID is reused by more than one model part in the package.
    pub id: u32,
    /// Model part containing the source resource. `None` denotes the primary
    /// `3D/3dmodel.model` part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_model_path: Option<String>,
    /// Original resource ID when `id` had to be made package-unique.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_object_id: Option<u32>,
    pub name: Option<String>,
    pub parts: Vec<PartAnalysis>,
    pub part_count: usize,
    pub printable_part_count: usize,
    pub instance_count: usize,
    pub plate_ids: Vec<u32>,
    pub object_extruder_slot: Option<u16>,
    pub effective_slots: Vec<u16>,
    pub effective_material_colors: Vec<EffectiveMaterialColor>,
    pub classification: ColorClassification,
    /// Union of every printable positive part in object coordinates.
    /// Negative, modifier, blocker, and enforcer volumes are excluded.
    #[serde(default)]
    pub printable_bounds: Option<AxisAlignedBounds>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartAnalysis {
    pub id: u32,
    pub name: Option<String>,
    pub volume_type: VolumeType,
    pub printable: bool,
    pub extruder_slot: Option<u16>,
    pub inherited_extruder_slot: Option<u16>,
    pub painted_slots: Vec<u16>,
    pub effective_slots: Vec<u16>,
    pub component_path: Option<String>,
    pub component_transform: Option<Transform3mf>,
    /// Geometry bounds after component transforms, in owning-object coordinates.
    /// This may be present for non-printable volumes, but those bounds are never
    /// included in object or instance printable bounds.
    #[serde(default)]
    pub object_space_bounds: Option<AxisAlignedBounds>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeType {
    NormalPart,
    NegativePart,
    Modifier,
    SupportBlocker,
    SupportEnforcer,
    Unknown(String),
}

impl VolumeType {
    pub fn is_printable_positive(&self) -> bool {
        matches!(self, Self::NormalPart)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorClassification {
    Mono,
    MultiColor,
    Unassigned,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform3mf {
    /// 3MF's row-vector 4x3 affine transform in serialized field order:
    /// `m00 m01 m02 m10 m11 m12 m20 m21 m22 m30 m31 m32`.
    pub values: [f64; 12],
}

impl Transform3mf {
    pub const IDENTITY: Self = Self {
        values: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
    };

    #[must_use]
    pub fn is_finite(self) -> bool {
        self.values.into_iter().all(f64::is_finite)
    }

    /// Transform one point. Returns `None` if input or output is non-finite.
    #[must_use]
    pub fn transform_point(self, point: [f64; 3]) -> Option<[f64; 3]> {
        if !self.is_finite() || !point.into_iter().all(f64::is_finite) {
            return None;
        }
        let [x, y, z] = point;
        let value = self.values;
        let transformed = [
            x.mul_add(value[0], y.mul_add(value[3], z.mul_add(value[6], value[9]))),
            x.mul_add(
                value[1],
                y.mul_add(value[4], z.mul_add(value[7], value[10])),
            ),
            x.mul_add(
                value[2],
                y.mul_add(value[5], z.mul_add(value[8], value[11])),
            ),
        ];
        transformed
            .into_iter()
            .all(f64::is_finite)
            .then_some(transformed)
    }
}

/// Finite axis-aligned geometry bounds in millimetres.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AxisAlignedBounds {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrientationOptimizationOptions {
    pub overhang_threshold_degrees: f64,
    pub direction_samples: usize,
    pub finalist_count: usize,
    pub alternatives: usize,
    pub connectivity_cell_mm: f64,
    pub max_vertices_per_resource: usize,
    pub max_total_transformed_vertices: usize,
    pub max_triangles_per_part: usize,
}

impl Default for OrientationOptimizationOptions {
    fn default() -> Self {
        Self {
            overhang_threshold_degrees: 45.0,
            direction_samples: 144,
            finalist_count: 12,
            alternatives: 5,
            connectivity_cell_mm: 2.0,
            max_vertices_per_resource: 5_000_000,
            max_total_transformed_vertices: 10_000_000,
            max_triangles_per_part: 25_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrientationOptimizationReport {
    pub source: InputIdentity,
    pub object_id: u32,
    pub instance_id: u32,
    pub object_name: Option<String>,
    pub source_triangle_count: u64,
    pub sampled_triangle_count: usize,
    pub evaluated_orientation_count: usize,
    pub overhang_threshold_degrees: f64,
    pub source_orientation: OrientationCandidate,
    pub recommendation: OrientationCandidate,
    pub alternatives: Vec<OrientationCandidate>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrientationCandidate {
    /// Build-platform up direction expressed in the source instance's current
    /// oriented coordinate system.
    pub build_up: [f64; 3],
    /// Row-vector 3x3 rotation in 3MF serialized order. Translation is applied
    /// separately after the rotated geometry is seated on Z=0.
    pub rotation: [f64; 9],
    /// Complete recommended build-item transform. The object is centered on
    /// its source XY location and seated on Z=0 after applying `rotation`.
    pub target_transform: Transform3mf,
    pub metrics: OrientationMetrics,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrientationMetrics {
    pub score: f64,
    pub estimated_support_volume_mm3: f64,
    pub overhang_contact_area_mm2: f64,
    pub overhang_component_count: usize,
    pub small_overhang_component_count: usize,
    pub bed_contact_area_mm2: f64,
    pub height_mm: f64,
    pub footprint_width_mm: f64,
    pub footprint_depth_mm: f64,
    pub footprint_area_mm2: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlateOrientationOptimizationOptions {
    pub orientation: OrientationOptimizationOptions,
    pub bed_min_x_mm: f64,
    pub bed_min_y_mm: f64,
    pub bed_width_mm: f64,
    pub bed_depth_mm: f64,
    pub bed_height_mm: f64,
    /// Per-side space reserved beyond the model footprint for generated
    /// supports, brims, and other first-layer process geometry. Every mode
    /// keeps these envelopes disjoint because Snapmaker Orca 2.3.5 does not
    /// merge overlapping per-object brim toolpaths safely.
    #[serde(default = "default_support_envelope_mm")]
    pub support_envelope_mm: f64,
    /// Optional first-layer reliability policy. The same policy determines
    /// both the packing envelope and the project settings written to an
    /// optimized copy.
    #[serde(default)]
    pub adhesion_mode: AdhesionMode,
    pub object_clearance_mm: f64,
    pub max_repair_attempts: usize,
    pub max_instances: usize,
}

const fn default_support_envelope_mm() -> f64 {
    // Snapmaker Orca's Auto Brim can extend 18 mm beyond an object. Keeping
    // this value aligned with the writer prevents a layout that fits the mesh
    // but collides once first-layer process geometry is generated.
    18.0
}

impl Default for PlateOrientationOptimizationOptions {
    fn default() -> Self {
        Self {
            orientation: OrientationOptimizationOptions::default(),
            bed_min_x_mm: 0.5,
            bed_min_y_mm: 1.0,
            bed_width_mm: 270.0,
            bed_depth_mm: 270.0,
            bed_height_mm: 270.0,
            support_envelope_mm: default_support_envelope_mm(),
            adhesion_mode: AdhesionMode::Standard,
            object_clearance_mm: 2.0,
            max_repair_attempts: 50,
            max_instances: 64,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdhesionMode {
    #[default]
    Standard,
    Reliable,
    Maximum,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdhesionRiskLevel {
    Low,
    Moderate,
    High,
    Critical,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdhesionRiskAssessment {
    pub score: f64,
    pub level: AdhesionRiskLevel,
    pub recommended_brim_width_mm: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlateOrientationInstanceReport {
    pub object_id: u32,
    pub instance_id: u32,
    pub source_build_item_index: u32,
    pub candidate_rank: usize,
    pub used_source_orientation: bool,
    pub target_min_x_mm: f64,
    pub target_min_y_mm: f64,
    pub adhesion_risk: AdhesionRiskAssessment,
    pub selected: OrientationCandidate,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlateOrientationOptimizationReport {
    pub source: InputIdentity,
    pub plate_id: u32,
    pub repair_attempts: usize,
    pub source_score: f64,
    pub selected_score: f64,
    pub estimated_support_volume_improvement: f64,
    pub adhesion_mode: AdhesionMode,
    pub reserved_process_envelope_mm: f64,
    pub maximum_adhesion_risk: AdhesionRiskAssessment,
    pub instances: Vec<PlateOrientationInstanceReport>,
}

impl AxisAlignedBounds {
    #[must_use]
    pub fn from_point(point: [f64; 3]) -> Option<Self> {
        point.into_iter().all(f64::is_finite).then_some(Self {
            min: point,
            max: point,
        })
    }

    #[must_use]
    pub fn is_valid(self) -> bool {
        self.min.into_iter().all(f64::is_finite)
            && self.max.into_iter().all(f64::is_finite)
            && (0..3).all(|axis| self.min[axis] <= self.max[axis])
    }

    #[must_use]
    pub fn size(self) -> Option<[f64; 3]> {
        self.is_valid().then(|| {
            [
                self.max[0] - self.min[0],
                self.max[1] - self.min[1],
                self.max[2] - self.min[2],
            ]
        })
    }

    #[must_use]
    pub fn union(self, other: Self) -> Option<Self> {
        if !self.is_valid() || !other.is_valid() {
            return None;
        }
        Some(Self {
            min: [
                self.min[0].min(other.min[0]),
                self.min[1].min(other.min[1]),
                self.min[2].min(other.min[2]),
            ],
            max: [
                self.max[0].max(other.max[0]),
                self.max[1].max(other.max[1]),
                self.max[2].max(other.max[2]),
            ],
        })
    }

    #[must_use]
    pub fn transformed(self, transform: Transform3mf) -> Option<Self> {
        if !self.is_valid() {
            return None;
        }
        let mut transformed: Option<Self> = None;
        for x in [self.min[0], self.max[0]] {
            for y in [self.min[1], self.max[1]] {
                for z in [self.min[2], self.max[2]] {
                    let point = transform.transform_point([x, y, z])?;
                    let point_bounds = Self::from_point(point)?;
                    transformed = Some(match transformed {
                        Some(bounds) => bounds.union(point_bounds)?,
                        None => point_bounds,
                    });
                }
            }
        }
        transformed
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisSummary {
    pub plate_count: usize,
    pub object_count: usize,
    pub instance_count: usize,
    pub part_count: usize,
    pub printable_part_count: usize,
    pub declared_filament_count: usize,
    pub used_filament_count: usize,
    pub mono_object_count: usize,
    pub multi_color_object_count: usize,
    pub unassigned_object_count: usize,
    #[serde(default)]
    pub bounded_printable_part_count: usize,
    #[serde(default)]
    pub bounded_object_count: usize,
    #[serde(default)]
    pub bounded_instance_count: usize,
    pub vertex_count: u64,
    pub triangle_count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisWarning {
    pub code: WarningCode,
    pub message: String,
    pub entry_path: Option<String>,
    pub object_id: Option<u32>,
    pub plate_id: Option<u32>,
}

impl AnalysisWarning {
    pub(crate) fn new(code: WarningCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            entry_path: None,
            object_id: None,
            plate_id: None,
        }
    }

    pub(crate) fn for_object(mut self, object_id: u32) -> Self {
        self.object_id = Some(object_id);
        self
    }

    pub(crate) fn for_plate(mut self, plate_id: u32) -> Self {
        self.plate_id = Some(plate_id);
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarningCode {
    ExperimentalDialect,
    MissingProjectSettings,
    MissingModelSettings,
    InvalidSetting,
    MismatchedFilamentArrays,
    InvalidColor,
    DuplicateObjectId,
    DuplicateUuid,
    DanglingObjectReference,
    UnassignedInstance,
    InstanceOnMultiplePlates,
    UnknownVolumeType,
    MissingColorAssignment,
    UnknownGeometryBounds,
    /// The source settings/build graph cannot be mapped one-to-one. Planning
    /// and conversion must fail closed instead of treating missing copies as
    /// safely excludable print units.
    UnsafeBuildInstanceGraph,
    UndeclaredFilamentSlot,
    PossibleAlternativePlates,
}
