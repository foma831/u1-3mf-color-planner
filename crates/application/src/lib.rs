//! Application orchestration between 3MF analysis, color matching, physical
//! loadout planning, and version-gated Snapmaker Orca integration.

mod calibration;
mod calibration_project;
mod conversion_plan;

pub use calibration::*;
pub use calibration_project::*;
pub use conversion_plan::*;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use thiserror::Error;
use u1_color_engine::{
    CalibrationContext, CalibrationSample, CatalogSpool, ColorBasis, DefaultSpoolRole,
    LayerSubdivisionPolicy, MatchQuality, MaterialCatalog, MixRecipe, MixingPolicy, PhysicalColor,
    PredictionConfidence, PrinterProfileIdentity, RecipeMatch, RecipeMode, RecipeSearchSettings,
    SrgbColor, built_in_material_catalog, classify_delta_e, delta_e_for_srgb, loadout_fingerprint,
    search_recipes,
};
use u1_orca_adapter::{
    FULL_SPECTRUM_FILAMENT_BASE_SHA256, FULL_SPECTRUM_PROCESS_BASE_SHA256,
    FULL_SPECTRUM_PROCESS_PROFILE_NAME, FULL_SPECTRUM_PROCESS_PROFILE_SHA256,
    FULL_SPECTRUM_PROCESS_SETTING_ID, FULL_SPECTRUM_PROFILE_NAME, FULL_SPECTRUM_PROFILE_SHA256,
    FULL_SPECTRUM_SETTING_ID, KLIPPER_MACHINE_BASE_SHA256, SUPPORTED_ORCA_VERSION,
    TOOLCHANGER_MACHINE_BASE_SHA256, U1_MACHINE_BASE_SHA256, U1_MACHINE_PROFILE_NAME,
    U1_MACHINE_PROFILE_SHA256, U1_MACHINE_SETTING_ID, U1_PROCESS_BASE_SHA256,
    U1_PROCESS_COMMON_SHA256,
};
use u1_planner::{
    A1MiniConfig, BestEffortCmyxCandidate, BoundsMm, BuildVolumeMm, CmySetup, CmyxColorCandidate,
    CmyxFallbackApproval, CmyxRecipe, ColorConfidence, CurrentToolheadState,
    DirectAssignmentRequest, DirectSpoolCandidate, FullSpectrumMode,
    FullSpectrumProcessCompatibility, FullSpectrumSubdivisionPolicy, Material,
    MaterialColorRequirement, MaterialRole as PlannerMaterialRole, MaterialSubstitutionApproval,
    PlannerConfig, PlanningInput, PlanningResult, PrintScope, PrintableUnit, PrinterPreference,
    RgbColor, ScopeStrategy, Spool, Toolhead, ToolheadSlotState, plan,
};
use u1_three_mf::{
    AnalysisError, AxisAlignedBounds, DialectSupport, EffectiveMaterialColor,
    MaterialRole as SourceMaterialRole, ObjectAnalysis, PlateAnalysis, ProjectAnalysis,
    ProjectDialect, WarningCode, analyze_project,
};

const CYAN_ID: &str = "panchroma-translucent-cyan";
const MAGENTA_ID: &str = "panchroma-translucent-magenta";
const YELLOW_ID: &str = "panchroma-translucent-yellow";
const GREY_ID: &str = "panchroma-translucent-grey";

#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error("3MF analysis failed: {0}")]
    Analysis(#[from] AnalysisError),
    #[error("failed to load the built-in material catalog: {0}")]
    Catalog(#[from] u1_color_engine::CatalogError),
    #[error("built-in catalog spool {spool_id:?} is missing or invalid")]
    InvalidCatalogSpool { spool_id: String },
    #[error("spool ID {spool_id:?} is duplicated between {first_source} and {second_source}")]
    DuplicateSpoolId {
        spool_id: String,
        first_source: String,
        second_source: String,
    },
    #[error("planning override references unknown scope {scope_id:?}")]
    UnknownScopeOverride { scope_id: String },
    #[error("planning override for scope {scope_id:?} is duplicated")]
    DuplicateScopeOverride { scope_id: String },
    #[error("printer override references unknown source unit {source_unit_id:?}")]
    UnknownSourceUnitOverride { source_unit_id: String },
    #[error("printer override for source unit {source_unit_id:?} is duplicated")]
    DuplicateSourceUnitOverride { source_unit_id: String },
    #[error("physical spool {spool_id:?} is loaded in more than one U1 toolhead")]
    DuplicateLoadedSpool { spool_id: String },
    #[error("current printer loadout is invalid: {message}")]
    InvalidCurrentPrinterLoadout { message: String },
    #[error(
        "planning options include plate {plate_id}, which is not an alternative plate candidate"
    )]
    InvalidAlternativePlateSelection { plate_id: u32 },
    #[error("CMY+X calibration data is invalid: {0}")]
    Calibration(#[from] CmyxCalibrationError),
    #[error("source 3MF build-instance graph is unsafe for conversion: {issues:?}")]
    UnsafeSourceGraph { issues: Vec<String> },
    #[error(
        "printable source instance for object {object_id} has no immutable build-item identity"
    )]
    MissingBuildItemIdentity { object_id: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlternativePlateCandidate {
    pub id: u32,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PreliminaryPlanOptions {
    pub current_toolheads: CurrentToolheadState,
    /// Default strategy for scopes without an explicit override.
    pub scope_strategy: ScopeStrategy,
    pub restore_cmy_after_direct: bool,
    /// Explicit operator opt-in for reducing more than four Direct source
    /// identities to a maximum of four physical U1 spools per scope.
    pub allow_direct_palette_reduction: bool,
    /// Physical spools explicitly confirmed by the user. Project metadata is
    /// never promoted into this collection automatically.
    pub confirmed_spools: Vec<Spool>,
    pub scope_overrides: Vec<ScopePlanningOverride>,
    /// Stable per-source-unit printer intent. Target plate IDs are provisional
    /// and must never be used as persistence keys.
    pub unit_printer_overrides: Vec<UnitPrinterOverride>,
    pub a1_mini: A1MiniConfig,
    /// Alternative source plates explicitly selected for this plan. The
    /// structurally detected alternative group is excluded by default.
    pub included_alternative_plate_ids: Vec<u32>,
    /// User-confirmed measurements only. No catalog or nominal color is ever
    /// promoted into this collection automatically.
    pub confirmed_calibration_samples: Vec<UserCmyxCalibrationRecord>,
    /// Explicit geometry identity for the current color prediction. Unknown
    /// geometry safely prevents measured samples from being reused.
    pub cmyx_geometry_context: CmyxGeometryContext,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScopePlanningOverride {
    pub scope_id: String,
    #[serde(default)]
    pub strategy: Option<ScopeStrategy>,
    #[serde(default)]
    pub direct_assignments: Vec<DirectAssignmentRequest>,
    #[serde(default)]
    pub approved_cmyx_fallbacks: Vec<CmyxFallbackApproval>,
    #[serde(default)]
    pub approved_material_substitutions: Vec<MaterialSubstitutionApproval>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitPrinterOverride {
    pub source_unit_id: String,
    pub preference: PrinterPreference,
}

impl Default for PreliminaryPlanOptions {
    fn default() -> Self {
        Self {
            current_toolheads: CurrentToolheadState {
                slots: [
                    ToolheadSlotState::Loaded(CYAN_ID.to_owned()),
                    ToolheadSlotState::Loaded(MAGENTA_ID.to_owned()),
                    ToolheadSlotState::Loaded(YELLOW_ID.to_owned()),
                    ToolheadSlotState::Loaded(GREY_ID.to_owned()),
                ],
            },
            scope_strategy: ScopeStrategy::Auto,
            restore_cmy_after_direct: true,
            allow_direct_palette_reduction: false,
            confirmed_spools: Vec::new(),
            scope_overrides: Vec::new(),
            unit_printer_overrides: Vec::new(),
            a1_mini: A1MiniConfig::default(),
            included_alternative_plate_ids: Vec::new(),
            confirmed_calibration_samples: Vec::new(),
            cmyx_geometry_context: CmyxGeometryContext::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApplicationReport {
    pub source_path: PathBuf,
    pub analysis: ProjectAnalysis,
    pub planning_input: PlanningInput,
    pub plan: PlanningResult,
    pub limitations: Vec<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SourceDialectConversionError {
    #[error("unsupported source 3MF dialect {dialect} cannot be sent to a writer adapter")]
    Unsupported { dialect: &'static str },
    #[error(
        "this source uses an Experimental 3MF dialect; explicit fingerprint-bound approval is required before conversion"
    )]
    ExperimentalApprovalRequired,
    #[error("the Experimental dialect approval is stale or belongs to another source file")]
    StaleExperimentalApproval,
}

fn project_dialect_identity(dialect: ProjectDialect) -> &'static str {
    match dialect {
        ProjectDialect::BambuStudioProject => "bambu_studio_project",
        ProjectDialect::OrcaSlicerProject => "orca_slicer_project",
        ProjectDialect::SnapmakerOrcaProject => "snapmaker_orca_project",
        ProjectDialect::Standard3mf => "standard_3mf",
        ProjectDialect::Unknown => "unknown",
    }
}

#[must_use]
pub fn source_dialect_approval_fingerprint(analysis: &ProjectAnalysis) -> String {
    let evidence = format!(
        "u1-experimental-dialect-approval-v1\nsource_sha256={}\ndialect={}\ndialect_version={}\napplication_version={}",
        analysis.input.sha256,
        project_dialect_identity(analysis.source.dialect),
        analysis.source.dialect_version.as_deref().unwrap_or(""),
        analysis.source.application_version.as_deref().unwrap_or(""),
    );
    format!("sha256:{:x}", Sha256::digest(evidence.as_bytes()))
}

pub fn validate_source_dialect_for_conversion(
    analysis: &ProjectAnalysis,
    approved_source_fingerprint: Option<&str>,
) -> Result<(), SourceDialectConversionError> {
    match analysis.source.support {
        DialectSupport::Unsupported => Err(SourceDialectConversionError::Unsupported {
            dialect: project_dialect_identity(analysis.source.dialect),
        }),
        DialectSupport::Experimental => {
            let expected = source_dialect_approval_fingerprint(analysis);
            match approved_source_fingerprint {
                Some(approval) if approval == expected => Ok(()),
                Some(_) => Err(SourceDialectConversionError::StaleExperimentalApproval),
                None => Err(SourceDialectConversionError::ExperimentalApprovalRequired),
            }
        }
        DialectSupport::Supported | DialectSupport::Limited => Ok(()),
    }
}

pub fn analyze_and_plan(path: impl AsRef<Path>) -> Result<ApplicationReport, ApplicationError> {
    analyze_and_plan_with_options(path, &PreliminaryPlanOptions::default())
}

pub fn analyze_and_plan_with_options(
    path: impl AsRef<Path>,
    options: &PreliminaryPlanOptions,
) -> Result<ApplicationReport, ApplicationError> {
    let path = path.as_ref();
    let analysis = analyze_project(path)?;
    let planning_input = build_planning_input(&analysis, options)?;
    let plan = plan(&planning_input);
    Ok(ApplicationReport {
        source_path: path.to_path_buf(),
        analysis,
        planning_input,
        plan,
        limitations: preliminary_plan_limitations(),
    })
}

#[must_use]
pub fn preliminary_plan_limitations() -> Vec<String> {
    vec![
        "Geometry-based A1 eligibility is evaluated per instance; unknown or degenerate bounds remain pinned to U1.".to_owned(),
        "Known instance bounds validate individual fit only; final multi-object positioning and clearance still require packing validation.".to_owned(),
        "Uncalibrated Full Spectrum predictions are nominal candidates and must not be treated as measured print colors.".to_owned(),
        "Source-material spools inferred from the project are unavailable placeholders; Direct Spools remains blocked until the user confirms real inventory.".to_owned(),
    ]
}

/// Returns the joint plates that form the source project's alternative group.
///
/// This mirrors the analyzer warning contract: a group exists only when at
/// least two named plates contain `joint` (case-insensitive).
#[must_use]
pub fn alternative_plate_candidates(analysis: &ProjectAnalysis) -> Vec<AlternativePlateCandidate> {
    let candidates = analysis
        .plates
        .iter()
        .filter_map(|plate| {
            let name = plate.name.as_ref()?;
            name.to_ascii_lowercase()
                .contains("joint")
                .then(|| AlternativePlateCandidate {
                    id: plate.id,
                    name: name.clone(),
                })
        })
        .collect::<Vec<_>>();

    if candidates.len() >= 2 {
        candidates
    } else {
        Vec::new()
    }
}

pub fn build_planning_input(
    analysis: &ProjectAnalysis,
    options: &PreliminaryPlanOptions,
) -> Result<PlanningInput, ApplicationError> {
    let source_graph_issues = analysis
        .warnings
        .iter()
        .filter(|warning| warning.code == WarningCode::UnsafeBuildInstanceGraph)
        .map(|warning| warning.message.clone())
        .collect::<Vec<_>>();
    if !source_graph_issues.is_empty() {
        return Err(ApplicationError::UnsafeSourceGraph {
            issues: source_graph_issues,
        });
    }
    if let Some(instance) = analysis
        .plates
        .iter()
        .flat_map(|plate| &plate.instances)
        .find(|instance| instance.printable && instance.source_build_item_index.is_none())
    {
        return Err(ApplicationError::MissingBuildItemIdentity {
            object_id: instance.object_id,
        });
    }
    validate_current_toolheads(&options.current_toolheads)?;
    let alternative_plate_ids = alternative_plate_candidates(analysis)
        .into_iter()
        .map(|plate| plate.id)
        .collect::<BTreeSet<_>>();
    let included_alternative_plate_ids = options
        .included_alternative_plate_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if let Some(plate_id) = included_alternative_plate_ids
        .difference(&alternative_plate_ids)
        .next()
    {
        return Err(ApplicationError::InvalidAlternativePlateSelection {
            plate_id: *plate_id,
        });
    }

    let catalog = built_in_material_catalog()?;
    let inventory = build_inventory(&catalog, analysis, &options.confirmed_spools)?;
    let calibration_library =
        UserCmyxCalibrationLibrary::from_records(options.confirmed_calibration_samples.clone())?;
    let matcher = CmyxMatcher::new(
        &catalog,
        &inventory,
        &options.confirmed_spools,
        calibration_library.engine_samples()?,
        options.cmyx_geometry_context.clone(),
    )?;
    let objects = analysis
        .objects
        .iter()
        .map(|object| (object.id, object))
        .collect::<BTreeMap<_, _>>();
    let mut scopes = analysis
        .plates
        .iter()
        .filter(|plate| {
            !alternative_plate_ids.contains(&plate.id)
                || included_alternative_plate_ids.contains(&plate.id)
        })
        .flat_map(|plate| {
            build_scopes_for_plate(plate, &objects, analysis, &inventory, &matcher, options)
        })
        .collect::<Vec<_>>();
    assign_best_effort_candidate_ids(&mut scopes);
    apply_scope_overrides(&mut scopes, &options.scope_overrides)?;
    apply_unit_printer_overrides(&mut scopes, &options.unit_printer_overrides)?;

    let restore_t4_spool_id = match &options.current_toolheads.slots[Toolhead::T4.index()] {
        ToolheadSlotState::Loaded(spool_id) => Some(spool_id.clone()),
        ToolheadSlotState::Unknown | ToolheadSlotState::Empty => Some(GREY_ID.to_owned()),
    };
    let mut config = PlannerConfig::with_cmy_setup(CmySetup {
        cyan_spool_id: CYAN_ID.to_owned(),
        magenta_spool_id: MAGENTA_ID.to_owned(),
        yellow_spool_id: YELLOW_ID.to_owned(),
        default_t4_spool_id: restore_t4_spool_id,
    });
    config.supported_full_spectrum_processes = vec![full_spectrum_process_compatibility()];
    config.u1_build_volume = BuildVolumeMm {
        width: 270.0,
        depth: 270.0,
        height: 270.0,
    };
    config.a1_mini = options.a1_mini.clone();
    config.restore_cmy_after_direct = options.restore_cmy_after_direct;
    config.allow_direct_palette_reduction = options.allow_direct_palette_reduction;

    Ok(PlanningInput {
        scopes,
        inventory,
        current_toolheads: options.current_toolheads.clone(),
        config,
    })
}

fn validate_current_toolheads(current: &CurrentToolheadState) -> Result<(), ApplicationError> {
    let mut loaded = BTreeSet::new();
    for slot in &current.slots {
        if let ToolheadSlotState::Loaded(spool_id) = slot
            && !loaded.insert(spool_id.as_str())
        {
            return Err(ApplicationError::DuplicateLoadedSpool {
                spool_id: spool_id.clone(),
            });
        }
    }
    Ok(())
}

fn build_inventory(
    catalog: &MaterialCatalog,
    analysis: &ProjectAnalysis,
    confirmed_spools: &[Spool],
) -> Result<Vec<Spool>, ApplicationError> {
    let mut inventory = BTreeMap::<String, (Spool, &'static str)>::new();
    for spool in inventory_from_catalog(catalog)? {
        insert_inventory_spool(&mut inventory, spool, "built-in catalog")?;
    }
    let mut confirmed_ids = BTreeSet::new();
    for spool in confirmed_spools.iter().cloned() {
        if !confirmed_ids.insert(spool.id.clone()) {
            return Err(ApplicationError::DuplicateSpoolId {
                spool_id: spool.id,
                first_source: "confirmed inventory".to_owned(),
                second_source: "confirmed inventory".to_owned(),
            });
        }
        if let Some((built_in, source)) = inventory.get_mut(&spool.id)
            && *source == "built-in catalog"
        {
            // A library entry with a built-in ID is an availability override,
            // not a second physical-spool definition. Catalog identity and
            // calibration metadata remain authoritative.
            built_in.available = spool.available;
            continue;
        }
        insert_inventory_spool(&mut inventory, spool, "confirmed inventory")?;
    }

    let known_spools = inventory
        .values()
        .map(|(spool, _)| spool.clone())
        .collect::<Vec<_>>();
    for spool in source_material_inventory(analysis, &known_spools) {
        insert_inventory_spool(&mut inventory, spool, "inferred source placeholder")?;
    }

    Ok(inventory.into_values().map(|(spool, _)| spool).collect())
}

fn insert_inventory_spool(
    inventory: &mut BTreeMap<String, (Spool, &'static str)>,
    spool: Spool,
    source: &'static str,
) -> Result<(), ApplicationError> {
    if let Some((_, existing_source)) = inventory.get(&spool.id) {
        return Err(ApplicationError::DuplicateSpoolId {
            spool_id: spool.id,
            first_source: (*existing_source).to_owned(),
            second_source: source.to_owned(),
        });
    }
    inventory.insert(spool.id.clone(), (spool, source));
    Ok(())
}

fn apply_scope_overrides(
    scopes: &mut [PrintScope],
    overrides: &[ScopePlanningOverride],
) -> Result<(), ApplicationError> {
    let mut by_scope = BTreeMap::new();
    for planning_override in overrides {
        if by_scope
            .insert(planning_override.scope_id.as_str(), planning_override)
            .is_some()
        {
            return Err(ApplicationError::DuplicateScopeOverride {
                scope_id: planning_override.scope_id.clone(),
            });
        }
    }

    for scope_id in by_scope.keys() {
        if !scopes.iter().any(|scope| scope.id.as_str() == *scope_id) {
            return Err(ApplicationError::UnknownScopeOverride {
                scope_id: (*scope_id).to_owned(),
            });
        }
    }

    for scope in scopes {
        let Some(planning_override) = by_scope.get(scope.id.as_str()) else {
            continue;
        };
        if let Some(strategy) = planning_override.strategy {
            scope.strategy = strategy;
        }
        scope.direct_assignments = planning_override.direct_assignments.clone();
        scope.approved_cmyx_fallbacks = planning_override.approved_cmyx_fallbacks.clone();
        scope.approved_material_substitutions =
            planning_override.approved_material_substitutions.clone();
    }
    Ok(())
}

fn apply_unit_printer_overrides(
    scopes: &mut [PrintScope],
    overrides: &[UnitPrinterOverride],
) -> Result<(), ApplicationError> {
    let mut by_source_unit = BTreeMap::new();
    for printer_override in overrides {
        if by_source_unit
            .insert(
                printer_override.source_unit_id.as_str(),
                printer_override.preference,
            )
            .is_some()
        {
            return Err(ApplicationError::DuplicateSourceUnitOverride {
                source_unit_id: printer_override.source_unit_id.clone(),
            });
        }
    }

    let known_source_units = scopes
        .iter()
        .flat_map(|scope| scope.units.iter().map(|unit| unit.source_unit_id.as_str()))
        .collect::<BTreeSet<_>>();
    for source_unit_id in by_source_unit.keys() {
        if !known_source_units.contains(source_unit_id) {
            return Err(ApplicationError::UnknownSourceUnitOverride {
                source_unit_id: (*source_unit_id).to_owned(),
            });
        }
    }

    for unit in scopes.iter_mut().flat_map(|scope| scope.units.iter_mut()) {
        if let Some(preference) = by_source_unit.get(unit.source_unit_id.as_str()) {
            unit.printer_preference = *preference;
        }
    }
    Ok(())
}

fn inventory_from_catalog(catalog: &MaterialCatalog) -> Result<Vec<Spool>, ApplicationError> {
    catalog
        .spools
        .iter()
        .map(|spool| {
            let color = SrgbColor::from_hex(&spool.hex).map_err(|_| {
                ApplicationError::InvalidCatalogSpool {
                    spool_id: spool.id.clone(),
                }
            })?;
            Ok(Spool {
                id: spool.id.clone(),
                calibration_id: None,
                display_name: format!(
                    "{} {} — {}",
                    spool.manufacturer, spool.product, spool.color_name
                ),
                color_name: Some(spool.color_name.clone()),
                material: match spool.material {
                    u1_color_engine::CatalogMaterial::Pla => Material::Pla,
                    u1_color_engine::CatalogMaterial::Petg => Material::Petg,
                },
                nominal_color: planner_color(color),
                measured_color: None,
                sku: None,
                profile_id: None,
                available: true,
            })
        })
        .collect()
}

/// Returns the canonical built-in physical-spool catalog used by the desktop
/// filament library. Callers may persist availability overrides, but should
/// keep the returned identity and calibration fields authoritative.
pub fn built_in_spool_inventory() -> Result<Vec<Spool>, ApplicationError> {
    let catalog = built_in_material_catalog()?;
    inventory_from_catalog(&catalog)
}

fn build_scopes_for_plate(
    plate: &PlateAnalysis,
    objects: &BTreeMap<u32, &ObjectAnalysis>,
    analysis: &ProjectAnalysis,
    inventory: &[Spool],
    matcher: &CmyxMatcher,
    options: &PreliminaryPlanOptions,
) -> Vec<PrintScope> {
    let selected_t4 = matcher.best_loadout_for_sources(&plate.effective_material_colors);
    let default_scope = build_scope(
        plate,
        objects,
        analysis,
        inventory,
        matcher,
        options,
        selected_t4,
        format!("plate-{}", plate.id),
        plate
            .name
            .clone()
            .unwrap_or_else(|| format!("Source Plate {}", plate.id)),
    );
    let default_manual_count = default_scope
        .requirements
        .iter()
        .filter(|requirement| {
            matches!(
                requirement.cmyx_candidate.recipe,
                CmyxRecipe::ManualReview { .. }
            )
        })
        .count();
    if let Some(scopes) =
        split_plate_by_object_palette(plate, objects, analysis, inventory, matcher, options)
    {
        return scopes;
    }
    if default_scope.strategy == ScopeStrategy::DirectSpools {
        return vec![default_scope];
    }

    let mut groups = BTreeMap::<String, Vec<_>>::new();
    for instance in plate.instances.iter().filter(|instance| instance.printable) {
        let object_colors = objects
            .get(&instance.object_id)
            .map_or(plate.effective_material_colors.as_slice(), |object| {
                object.effective_material_colors.as_slice()
            });
        let loadout = matcher
            .best_loadout_for_sources(object_colors)
            .or(selected_t4)
            .unwrap_or("unresolved")
            .to_owned();
        groups.entry(loadout).or_default().push(instance.clone());
    }
    if groups.len() <= 1 {
        return vec![default_scope];
    }

    let base_name = plate
        .name
        .clone()
        .unwrap_or_else(|| format!("Source Plate {}", plate.id));
    let mut split_scopes = groups
        .into_iter()
        .map(|(t4_spool_id, instances)| {
            let mut split_plate = plate.clone();
            split_plate.instances = instances;
            split_plate.object_count = split_plate.instances.len();
            split_plate.effective_material_colors = merged_instance_colors(
                &split_plate.instances,
                objects,
                &plate.effective_material_colors,
            );
            let scope_id = format!("plate-{}-loadout-{}", plate.id, stable_slug(&t4_spool_id));
            build_scope(
                &split_plate,
                objects,
                analysis,
                inventory,
                matcher,
                options,
                Some(&t4_spool_id),
                scope_id,
                format!(
                    "{base_name} — {}",
                    spool_display_name(&t4_spool_id, inventory)
                ),
            )
        })
        .collect::<Vec<_>>();
    let split_manual_count = split_scopes
        .iter()
        .flat_map(|scope| &scope.requirements)
        .filter(|requirement| {
            matches!(
                requirement.cmyx_candidate.recipe,
                CmyxRecipe::ManualReview { .. }
            )
        })
        .count();
    let default_color_cost = scope_color_cost(&default_scope);
    let split_color_cost = split_scopes.iter().map(scope_color_cost).sum::<f64>();
    let split_mapping_count = split_scopes
        .iter()
        .flat_map(|scope| &scope.units)
        .map(|unit| unit.requirement_ids.len())
        .sum::<usize>()
        .max(1);
    let average_delta_improvement =
        (default_color_cost - split_color_cost) / split_mapping_count as f64;
    // One additional physical loadout is worthwhile only when it avoids a
    // blocked recipe or improves the average object-level match materially.
    const MIN_AVERAGE_DELTA_IMPROVEMENT: f64 = 1.0;
    if split_manual_count < default_manual_count
        || average_delta_improvement >= MIN_AVERAGE_DELTA_IMPROVEMENT
    {
        split_scopes.sort_by(|left, right| left.id.cmp(&right.id));
        split_scopes
    } else {
        vec![default_scope]
    }
}

fn split_plate_by_object_palette(
    plate: &PlateAnalysis,
    objects: &BTreeMap<u32, &ObjectAnalysis>,
    analysis: &ProjectAnalysis,
    inventory: &[Spool],
    matcher: &CmyxMatcher,
    options: &PreliminaryPlanOptions,
) -> Option<Vec<PrintScope>> {
    type PaletteKey = (
        Option<String>,
        Option<String>,
        Vec<SourceMaterialRole>,
        Vec<String>,
        Option<Vec<u16>>,
    );
    let palette_keys = |colors: &[EffectiveMaterialColor]| {
        colors
            .iter()
            .map(|source| {
                let mut roles = source.roles.clone();
                roles.sort();
                roles.dedup();
                let source_profile_ids = canonical_source_profile_ids(source);
                let missing_discriminator = (source.material.is_none()
                    || source.color.is_none()
                    || source_profile_ids.is_empty())
                .then(|| source.source_slots.clone());
                (
                    source.material.clone(),
                    source.color.clone(),
                    roles,
                    source_profile_ids,
                    missing_discriminator,
                )
            })
            .collect::<BTreeSet<PaletteKey>>()
    };

    if palette_keys(&plate.effective_material_colors).len() <= 4 {
        return None;
    }
    let mut groups = Vec::<(BTreeSet<PaletteKey>, Vec<u1_three_mf::ObjectInstance>)>::new();
    for instance in plate.instances.iter().filter(|instance| instance.printable) {
        let object_colors = objects
            .get(&instance.object_id)
            .map_or(plate.effective_material_colors.as_slice(), |object| {
                object.effective_material_colors.as_slice()
            });
        let object_palette = palette_keys(object_colors);
        if object_palette.len() > 4 {
            return None;
        }
        if let Some((group_palette, group_instances)) = groups.last_mut() {
            let union_count = group_palette.union(&object_palette).count();
            if union_count <= 4 {
                group_palette.extend(object_palette);
                group_instances.push(instance.clone());
                continue;
            }
        }
        groups.push((object_palette, vec![instance.clone()]));
    }
    if groups.len() <= 1 {
        return None;
    }

    let base_name = plate
        .name
        .clone()
        .unwrap_or_else(|| format!("Source Plate {}", plate.id));
    Some(
        groups
            .into_iter()
            .enumerate()
            .flat_map(|(index, (_, instances))| {
                let mut split_plate = plate.clone();
                split_plate.instances = instances;
                split_plate.object_count = split_plate.instances.len();
                split_plate.effective_material_colors = merged_instance_colors(
                    &split_plate.instances,
                    objects,
                    &plate.effective_material_colors,
                );
                split_plate.name = Some(format!("{base_name} — palette group {}", index + 1));
                let nested_scopes = build_scopes_for_plate(
                    &split_plate,
                    objects,
                    analysis,
                    inventory,
                    matcher,
                    options,
                );
                let nested_count = nested_scopes.len();
                nested_scopes
                    .into_iter()
                    .enumerate()
                    .map(move |(nested_index, scope)| {
                        let scope_id = if nested_count == 1 {
                            format!("plate-{}-palette-{:02}", plate.id, index + 1)
                        } else {
                            format!(
                                "plate-{}-palette-{:02}-loadout-{:02}",
                                plate.id,
                                index + 1,
                                nested_index + 1
                            )
                        };
                        rekey_scope(scope, scope_id)
                    })
                    .collect::<Vec<_>>()
            })
            .collect(),
    )
}

fn rekey_scope(mut scope: PrintScope, scope_id: String) -> PrintScope {
    let requirement_ids = scope
        .requirements
        .iter_mut()
        .enumerate()
        .map(|(index, requirement)| {
            let old_id = requirement.id.clone();
            requirement.id = format!("{scope_id}-requirement-{}", index + 1);
            (old_id, requirement.id.clone())
        })
        .collect::<BTreeMap<_, _>>();
    for (index, unit) in scope.units.iter_mut().enumerate() {
        unit.id = format!("{scope_id}-unit-{}", index + 1);
        for requirement_id in &mut unit.requirement_ids {
            if let Some(replacement) = requirement_ids.get(requirement_id) {
                *requirement_id = replacement.clone();
            }
        }
    }
    scope.id = scope_id;
    scope
}

fn scope_color_cost(scope: &PrintScope) -> f64 {
    const MANUAL_REVIEW_PENALTY: f64 = 100.0;
    let requirements = scope
        .requirements
        .iter()
        .map(|requirement| (requirement.id.as_str(), requirement))
        .collect::<BTreeMap<_, _>>();
    scope
        .units
        .iter()
        .flat_map(|unit| unit.requirement_ids.iter())
        .map(|id| {
            requirements
                .get(id.as_str())
                .map_or(MANUAL_REVIEW_PENALTY, |requirement| {
                    match &requirement.cmyx_candidate.recipe {
                        CmyxRecipe::ManualReview { .. } => MANUAL_REVIEW_PENALTY,
                        _ => requirement
                            .cmyx_candidate
                            .delta_e00
                            .filter(|delta| delta.is_finite() && *delta >= 0.0)
                            .unwrap_or(MANUAL_REVIEW_PENALTY),
                    }
                })
        })
        .sum()
}

#[allow(clippy::too_many_arguments)]
fn build_scope(
    plate: &PlateAnalysis,
    objects: &BTreeMap<u32, &ObjectAnalysis>,
    analysis: &ProjectAnalysis,
    inventory: &[Spool],
    matcher: &CmyxMatcher,
    options: &PreliminaryPlanOptions,
    selected_t4: Option<&str>,
    scope_id: String,
    display_name: String,
) -> PrintScope {
    let requirements = plate
        .effective_material_colors
        .iter()
        .enumerate()
        .map(|(index, source)| {
            build_requirement(
                format!("{scope_id}-requirement-{}", index + 1),
                source,
                inventory,
                matcher,
                selected_t4,
            )
        })
        .collect::<Vec<_>>();

    let units = plate
        .instances
        .iter()
        .enumerate()
        .filter(|(_, instance)| instance.printable)
        .map(|(_index, instance)| {
            let object = objects.get(&instance.object_id).copied();
            let proven_bounds = instance.printable_bounds.and_then(planner_bounds);
            let requirement_ids = object
                .map(|object| {
                    matching_requirement_ids(&object.effective_material_colors, &requirements)
                })
                .unwrap_or_default();
            let source_unit_id = format!(
                "source-build-item-{}",
                u64::from(
                    instance
                        .source_build_item_index
                        .expect("printable source build identities are validated before planning"),
                ) + 1
            );
            PrintableUnit {
                id: source_unit_id.clone(),
                source_unit_id,
                source_object_id: object
                    .map(|object| object.source_object_id.unwrap_or(object.id))
                    .unwrap_or(instance.object_id),
                source_instance_id: instance.instance_id,
                source_model_path: object.and_then(|object| object.source_model_path.clone()),
                display_name: object
                    .and_then(|object| object.name.clone())
                    .unwrap_or_else(|| format!("Object {}", instance.object_id)),
                source_plate_id: Some(format!("plate-{}", plate.id)),
                requirement_ids,
                bounds: proven_bounds.unwrap_or_else(|| BoundsMm::from_size(0.0, 0.0, 0.0)),
                source_layer_height_mm: analysis.process.layer_height_mm,
                printer_preference: if proven_bounds.is_some() {
                    PrinterPreference::Auto
                } else {
                    PrinterPreference::U1
                },
            }
        })
        .collect();

    let strategy = if options.scope_strategy == ScopeStrategy::Auto
        && requirements.iter().any(|requirement| {
            requirement.material != Material::Pla
                && matches!(
                    requirement.cmyx_candidate.recipe,
                    CmyxRecipe::ManualReview { .. } | CmyxRecipe::Unreachable { .. }
                )
        }) {
        ScopeStrategy::DirectSpools
    } else {
        options.scope_strategy
    };

    PrintScope {
        id: scope_id,
        display_name,
        requirements,
        units,
        strategy,
        direct_assignments: Vec::new(),
        approved_cmyx_fallbacks: Vec::new(),
        approved_material_substitutions: Vec::new(),
    }
}

fn merged_instance_colors(
    instances: &[u1_three_mf::ObjectInstance],
    objects: &BTreeMap<u32, &ObjectAnalysis>,
    fallback: &[EffectiveMaterialColor],
) -> Vec<EffectiveMaterialColor> {
    let mut merged = BTreeMap::<
        (
            Option<String>,
            Option<String>,
            Vec<SourceMaterialRole>,
            Vec<String>,
            Option<Vec<u16>>,
        ),
        (BTreeSet<u16>, BTreeSet<SourceMaterialRole>),
    >::new();
    for source in instances
        .iter()
        .filter_map(|instance| objects.get(&instance.object_id))
        .flat_map(|object| object.effective_material_colors.iter())
    {
        let mut role_identity = source.roles.clone();
        role_identity.sort();
        role_identity.dedup();
        let source_profile_ids = canonical_source_profile_ids(source);
        let missing_discriminator =
            (source.material.is_none() || source.color.is_none() || source_profile_ids.is_empty())
                .then(|| source.source_slots.clone());
        let entry = merged
            .entry((
                source.material.clone(),
                source.color.clone(),
                role_identity,
                source_profile_ids,
                missing_discriminator,
            ))
            .or_default();
        entry.0.extend(source.source_slots.iter().copied());
        entry.1.extend(source.roles.iter().copied());
    }
    if merged.is_empty() {
        return fallback.to_vec();
    }
    merged
        .into_iter()
        .map(
            |((material, color, _, source_profile_ids, _), (source_slots, roles))| {
                EffectiveMaterialColor {
                    source_slots: source_slots.into_iter().collect(),
                    source_profile_ids,
                    material,
                    color,
                    roles: roles.into_iter().collect(),
                }
            },
        )
        .collect()
}

fn canonical_source_profile_ids(source: &EffectiveMaterialColor) -> Vec<String> {
    let mut source_profile_ids = source.source_profile_ids.clone();
    source_profile_ids.sort();
    source_profile_ids.dedup();
    source_profile_ids
}

fn stable_slug(value: &str) -> String {
    let slug = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    slug.trim_matches('-').to_owned()
}

fn spool_display_name(spool_id: &str, inventory: &[Spool]) -> String {
    inventory
        .iter()
        .find(|spool| spool.id == spool_id)
        .map_or_else(|| spool_id.to_owned(), |spool| spool.display_name.clone())
}

fn planner_bounds(bounds: AxisAlignedBounds) -> Option<BoundsMm> {
    // Until process-specific brim/skirt extents are imported, reserve a
    // conservative envelope instead of treating the raw mesh AABB as the
    // complete printable footprint.
    const XY_CLEARANCE_MM: f64 = 5.0;
    const Z_CLEARANCE_MM: f64 = 0.5;
    let [width, depth, height] = bounds.size()?;
    let bounds = BoundsMm {
        width,
        depth,
        height,
        clearance_x: XY_CLEARANCE_MM,
        clearance_y: XY_CLEARANCE_MM,
        clearance_z: Z_CLEARANCE_MM,
    };
    (bounds.is_valid() && bounds.has_known_size()).then_some(bounds)
}

fn build_requirement(
    id: String,
    source: &EffectiveMaterialColor,
    inventory: &[Spool],
    matcher: &CmyxMatcher,
    selected_t4: Option<&str>,
) -> MaterialColorRequirement {
    let material = parse_material(source.material.as_deref());
    let source_color = parse_source_color(source.color.as_deref());
    let role = planner_role(&source.roles);
    let cmyx_candidate = match source_color {
        Some(color) => selected_t4
            .and_then(|t4| {
                matcher.best_in_loadout(t4, &material, color, role == PlannerMaterialRole::Support)
            })
            .unwrap_or_else(|| manual_cmyx("No compatible CMY+X candidate was found.")),
        None => manual_cmyx("The source color is missing or invalid."),
    };
    let best_effort_cmyx_candidate = source_color
        .filter(|_| {
            matches!(
                cmyx_candidate.recipe,
                CmyxRecipe::ManualReview { .. } | CmyxRecipe::Unreachable { .. }
            )
        })
        .and_then(|color| {
            selected_t4.and_then(|t4| {
                matcher.best_effort_in_loadout(
                    t4,
                    &material,
                    color,
                    role == PlannerMaterialRole::Support,
                )
            })
        })
        .map(|best_effort| BestEffortCmyxCandidate {
            // Filled only after recursive plate splitting has assigned the
            // final scope and requirement identities.
            candidate_id: String::new(),
            target_material: best_effort.target_material,
            target_color: planner_color(best_effort.target),
            candidate: planner_cmyx_unchecked(best_effort.candidate, best_effort.t4_spool_id),
        });
    let source_profile_ids = canonical_source_profile_ids(source);
    let direct_candidates = source_color
        .map(|color| direct_candidates(color, &material, inventory))
        .unwrap_or_default();

    MaterialColorRequirement {
        id,
        material,
        role,
        source_color: source_color
            .map(planner_color)
            .unwrap_or_else(|| RgbColor::new(0, 0, 0)),
        source_slots: source
            .source_slots
            .iter()
            .map(|slot| format!("F{slot}"))
            .collect(),
        source_profile_ids,
        cmyx_candidate,
        best_effort_cmyx_candidate,
        direct_candidates,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CandidateDecisionFingerprint<'a> {
    version: u8,
    scope_id: &'a str,
    source_material: &'a Material,
    source_role: PlannerMaterialRole,
    source_color: RgbColor,
    source_identity_kind: &'a str,
    source_identity: Vec<String>,
    target_material: &'a Material,
    target_color: RgbColor,
    candidate: &'a CmyxColorCandidate,
}

fn assign_best_effort_candidate_ids(scopes: &mut [PrintScope]) {
    for scope in scopes {
        for requirement in &mut scope.requirements {
            let Some(fallback) = requirement.best_effort_cmyx_candidate.as_ref() else {
                continue;
            };
            let mut profiles = requirement.source_profile_ids.clone();
            profiles.sort();
            profiles.dedup();
            let (source_identity_kind, source_identity) = if !profiles.is_empty()
                && profiles.iter().all(|profile| !profile.trim().is_empty())
            {
                ("declared_profiles", profiles)
            } else {
                let mut discriminator = requirement.source_slots.clone();
                discriminator.sort();
                discriminator.dedup();
                if discriminator.is_empty() {
                    discriminator.push(requirement.id.clone());
                }
                ("unknown_profile", discriminator)
            };
            let payload = CandidateDecisionFingerprint {
                version: 1,
                scope_id: &scope.id,
                source_material: &requirement.material,
                source_role: requirement.role,
                source_color: requirement.source_color,
                source_identity_kind,
                source_identity,
                target_material: &fallback.target_material,
                target_color: fallback.target_color,
                candidate: &fallback.candidate,
            };
            let serialized = serde_json::to_vec(&payload)
                .expect("candidate fingerprint payload contains only serializable domain data");
            let digest = Sha256::digest(serialized);
            let mut candidate_id = String::from("cmyx-fallback-v1-");
            for byte in digest {
                write!(&mut candidate_id, "{byte:02x}")
                    .expect("writing a SHA-256 digest into a String cannot fail");
            }
            requirement
                .best_effort_cmyx_candidate
                .as_mut()
                .expect("fallback was checked above")
                .candidate_id = candidate_id;
        }
    }
}

fn matching_requirement_ids(
    colors: &[EffectiveMaterialColor],
    requirements: &[MaterialColorRequirement],
) -> Vec<String> {
    let mut ids = Vec::new();
    for source in colors {
        let material = parse_material(source.material.as_deref());
        let color = parse_source_color(source.color.as_deref()).map(planner_color);
        let role = planner_role(&source.roles);
        let source_profile_ids = canonical_source_profile_ids(source);
        let source_slots = source
            .source_slots
            .iter()
            .map(|slot| format!("F{slot}"))
            .collect::<Vec<_>>();
        for requirement in requirements {
            if requirement.material == material
                && requirement.role == role
                && color == Some(requirement.source_color)
                && requirement.source_profile_ids == source_profile_ids
                && (!source_profile_ids.is_empty() || requirement.source_slots == source_slots)
            {
                ids.push(requirement.id.clone());
            }
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

fn manual_cmyx(reason: &str) -> CmyxColorCandidate {
    CmyxColorCandidate {
        recipe: CmyxRecipe::ManualReview {
            reason: reason.to_owned(),
        },
        calibration_sample_id: None,
        process_compatibility: None,
        required_t4_spool_id: None,
        predicted_color: None,
        delta_e00: None,
        confidence: ColorConfidence::Unknown,
        warnings: vec![reason.to_owned()],
    }
}

fn direct_candidates(
    source: SrgbColor,
    material: &Material,
    inventory: &[Spool],
) -> Vec<DirectSpoolCandidate> {
    let mut candidates = inventory
        .iter()
        .filter(|spool| spool.available && &spool.material == material)
        .map(|spool| {
            let actual = engine_color(spool.actual_color());
            DirectSpoolCandidate {
                spool_id: spool.id.clone(),
                delta_e00: Some(delta_e_for_srgb(source, actual)),
                confidence: if spool.measured_color.is_some() {
                    ColorConfidence::Measured
                } else {
                    ColorConfidence::Nominal
                },
            }
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|first, second| {
        first
            .delta_e00
            .unwrap_or(f64::INFINITY)
            .total_cmp(&second.delta_e00.unwrap_or(f64::INFINITY))
            .then_with(|| first.spool_id.cmp(&second.spool_id))
    });
    candidates
}

fn source_material_inventory(analysis: &ProjectAnalysis, existing: &[Spool]) -> Vec<Spool> {
    let existing_pairs = existing
        .iter()
        .map(|spool| {
            (
                spool.material.clone(),
                spool.actual_color(),
                spool.profile_id.clone(),
            )
        })
        .collect::<std::collections::BTreeSet<_>>();
    let declared = analysis
        .filaments
        .iter()
        .map(|filament| (filament.slot, filament))
        .collect::<BTreeMap<_, _>>();
    let mut pairs =
        BTreeMap::<(Material, RgbColor, Option<String>, Option<u16>), (u16, Option<String>)>::new();
    for source in &analysis.effective_material_colors {
        let material = parse_material(source.material.as_deref());
        if material == Material::Pla {
            continue;
        }
        let Some(color) = parse_source_color(source.color.as_deref()).map(planner_color) else {
            continue;
        };
        let slot = source.source_slots.first().copied().unwrap_or(0);
        let source_profile_ids = canonical_source_profile_ids(source);
        let preset = match source_profile_ids.as_slice() {
            [profile_id] => Some(profile_id.clone()),
            [] => declared
                .get(&slot)
                .and_then(|filament| filament.preset.clone()),
            _ => continue,
        };
        if existing_pairs.contains(&(material.clone(), color, preset.clone())) {
            continue;
        }
        let unknown_profile_slot = preset.is_none().then_some(slot);
        pairs
            .entry((material, color, preset.clone(), unknown_profile_slot))
            .or_insert((slot, preset));
    }

    let mut used_ids = BTreeSet::new();
    pairs
        .into_iter()
        .map(|((material, color, _, _), (slot, preset))| {
            let hex = engine_color(color).to_hex();
            let base_id = format!(
                "source-{}-{}",
                material_slug(&material),
                hex.trim_start_matches('#').to_ascii_lowercase()
            );
            let id = if used_ids.insert(base_id.clone()) {
                base_id
            } else {
                format!("{base_id}-f{slot}")
            };
            Spool {
                id,
                calibration_id: None,
                display_name: format!(
                    "Unconfirmed source {} spool — F{} {}",
                    material_label(&material),
                    slot,
                    hex
                ),
                color_name: Some(hex.clone()),
                material,
                nominal_color: color,
                measured_color: None,
                sku: None,
                profile_id: preset,
                // Project metadata describes the slicer's intended filament,
                // not a physical spool that the current user owns. Keeping
                // the placeholder unavailable prevents an inferred source
                // color from becoming a false Exact Direct Spool match.
                available: false,
            }
        })
        .collect()
}

fn material_slug(material: &Material) -> String {
    material_label(material)
        .to_ascii_lowercase()
        .replace(|character: char| !character.is_ascii_alphanumeric(), "-")
}

fn material_label(material: &Material) -> &str {
    match material {
        Material::Pla => "PLA",
        Material::Petg => "PETG",
        Material::Abs => "ABS",
        Material::Asa => "ASA",
        Material::Tpu => "TPU",
        Material::Other(name) => name,
    }
}

struct CmyxMatcher {
    loadouts: Vec<MatcherLoadout>,
    calibration_samples: Vec<CalibrationSample>,
    geometry_context: CmyxGeometryContext,
}

struct MatcherLoadout {
    t4_spool_id: String,
    t4_material: Material,
    t4_policy: MixingPolicy,
    colors: Vec<PhysicalColor>,
}

struct BestEffortMatch {
    candidate: RecipeMatch,
    target: SrgbColor,
    target_material: Material,
    t4_spool_id: String,
}

impl CmyxMatcher {
    fn new(
        catalog: &MaterialCatalog,
        inventory: &[Spool],
        confirmed_spools: &[Spool],
        calibration_samples: Vec<CalibrationSample>,
        geometry_context: CmyxGeometryContext,
    ) -> Result<Self, ApplicationError> {
        let by_id = catalog
            .spools
            .iter()
            .map(|spool| (spool.id.as_str(), spool))
            .collect::<BTreeMap<_, _>>();
        let cyan = required_catalog_spool(&by_id, CYAN_ID)?;
        let magenta = required_catalog_spool(&by_id, MAGENTA_ID)?;
        let yellow = required_catalog_spool(&by_id, YELLOW_ID)?;
        let available_ids = inventory
            .iter()
            .filter(|spool| spool.available)
            .map(|spool| spool.id.as_str())
            .collect::<BTreeSet<_>>();
        if ![CYAN_ID, MAGENTA_ID, YELLOW_ID]
            .iter()
            .all(|id| available_ids.contains(id))
        {
            return Ok(Self {
                loadouts: Vec::new(),
                calibration_samples,
                geometry_context,
            });
        }
        let mut t4_spools = catalog
            .spools
            .iter()
            .filter(|spool| {
                available_ids.contains(spool.id.as_str())
                    && matches!(
                        spool.default_role,
                        DefaultSpoolRole::DefaultT4 | DefaultSpoolRole::Optional
                    )
            })
            .collect::<Vec<_>>();
        t4_spools.sort_by_key(|spool| match spool.default_role {
            DefaultSpoolRole::DefaultT4 => 0,
            _ => 1,
        });

        let loadouts = t4_spools
            .into_iter()
            .map(|t4| {
                Ok(MatcherLoadout {
                    t4_spool_id: t4.id.clone(),
                    t4_material: match t4.material {
                        u1_color_engine::CatalogMaterial::Pla => Material::Pla,
                        u1_color_engine::CatalogMaterial::Petg => Material::Petg,
                    },
                    t4_policy: t4.mixing_policy,
                    colors: vec![
                        physical_color(1, cyan)?,
                        physical_color(2, magenta)?,
                        physical_color(3, yellow)?,
                        physical_color(4, t4)?,
                    ],
                })
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?;
        let mut loadouts = loadouts;
        let mut confirmed_t4 = confirmed_spools
            .iter()
            .filter(|spool| spool.available && !by_id.contains_key(spool.id.as_str()))
            .collect::<Vec<_>>();
        confirmed_t4.sort_by(|left, right| left.id.cmp(&right.id));
        for t4 in confirmed_t4 {
            loadouts.push(MatcherLoadout {
                t4_spool_id: t4.id.clone(),
                t4_material: t4.material.clone(),
                t4_policy: MixingPolicy::SolidOnlyUntilCalibrated,
                colors: vec![
                    physical_color(1, cyan)?,
                    physical_color(2, magenta)?,
                    physical_color(3, yellow)?,
                    physical_color_from_spool(4, t4),
                ],
            });
        }
        Ok(Self {
            loadouts,
            calibration_samples,
            geometry_context,
        })
    }

    fn best_loadout_for_sources(&self, sources: &[EffectiveMaterialColor]) -> Option<&str> {
        let targets = sources
            .iter()
            .filter_map(|source| {
                parse_source_color(source.color.as_deref()).map(|color| {
                    (
                        parse_material(source.material.as_deref()),
                        color,
                        planner_role(&source.roles) == PlannerMaterialRole::Support,
                    )
                })
            })
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return self
                .loadouts
                .first()
                .map(|loadout| loadout.t4_spool_id.as_str());
        }

        let strict = self
            .loadouts
            .iter()
            .filter_map(|loadout| {
                let mut score = 0.0;
                for (material, target, solid_only) in &targets {
                    score += self
                        .match_source(loadout, material, *target, *solid_only)?
                        .delta_e_00;
                }
                Some((score, loadout.t4_spool_id.as_str()))
            })
            .min_by(|(first_score, first_id), (second_score, second_id)| {
                first_score
                    .total_cmp(second_score)
                    .then_with(|| first_id.cmp(second_id))
            })
            .map(|(_, id)| id);
        strict.or_else(|| {
            self.loadouts
                .iter()
                .filter_map(|loadout| {
                    let mut score = 0.0;
                    for (_, target, solid_only) in &targets {
                        score += self
                            .search_loadout(loadout, *target, *solid_only)?
                            .delta_e_00;
                    }
                    Some((score, loadout.t4_spool_id.as_str()))
                })
                .min_by(|(first_score, first_id), (second_score, second_id)| {
                    first_score
                        .total_cmp(second_score)
                        .then_with(|| first_id.cmp(second_id))
                })
                .map(|(_, id)| id)
        })
    }

    fn best_in_loadout(
        &self,
        t4_spool_id: &str,
        material: &Material,
        target: SrgbColor,
        solid_only: bool,
    ) -> Option<CmyxColorCandidate> {
        let loadout = self
            .loadouts
            .iter()
            .find(|loadout| loadout.t4_spool_id == t4_spool_id)?;
        let candidate = self.match_source(loadout, material, target, solid_only)?;
        Some(planner_cmyx(candidate, loadout.t4_spool_id.clone()))
    }

    fn best_effort_in_loadout(
        &self,
        t4_spool_id: &str,
        source_material: &Material,
        target: SrgbColor,
        solid_only: bool,
    ) -> Option<BestEffortMatch> {
        let loadout = self
            .loadouts
            .iter()
            .find(|loadout| loadout.t4_spool_id == t4_spool_id)?;
        let candidate = self
            .match_source(loadout, source_material, target, solid_only)
            .or_else(|| self.search_loadout(loadout, target, solid_only))?;
        let target_material = recipe_target_material(&candidate.recipe, loadout)?;
        Some(BestEffortMatch {
            candidate,
            target,
            target_material,
            t4_spool_id: loadout.t4_spool_id.clone(),
        })
    }

    fn match_source(
        &self,
        loadout: &MatcherLoadout,
        material: &Material,
        target: SrgbColor,
        solid_only: bool,
    ) -> Option<RecipeMatch> {
        if material == &Material::Pla {
            return self.search_loadout(loadout, target, solid_only);
        }
        if &loadout.t4_material != material {
            return None;
        }
        let t4 = loadout.colors.get(3)?;
        let delta = delta_e_for_srgb(target, t4.srgb);
        Some(RecipeMatch {
            recipe: MixRecipe::solid(4),
            predicted_srgb: t4.srgb,
            delta_e_00: delta,
            quality: classify_delta_e(delta),
            confidence: match t4.basis {
                ColorBasis::Measured => PredictionConfidence::Measured,
                ColorBasis::Nominal => PredictionConfidence::Nominal,
            },
            calibration_sample_id: None,
        })
    }

    fn search_loadout(
        &self,
        loadout: &MatcherLoadout,
        target: SrgbColor,
        solid_only: bool,
    ) -> Option<RecipeMatch> {
        let settings = RecipeSearchSettings {
            ratio_denominator: 8,
            include_cycle: true,
            max_results: 512,
        };
        let context = calibration_context(&loadout.colors, &self.geometry_context);
        let matches = search_recipes(
            target,
            &loadout.colors,
            &context,
            &self.calibration_samples,
            settings,
        )
        .ok()?;
        matches.into_iter().find(|candidate| {
            if solid_only && candidate.recipe.mode != RecipeMode::Solid {
                return false;
            }
            if loadout.t4_material != Material::Pla
                && candidate
                    .recipe
                    .components
                    .iter()
                    .any(|component| component.slot == 4)
            {
                return false;
            }
            t4_recipe_allowed(candidate, loadout.t4_policy)
        })
    }
}

fn required_catalog_spool<'a>(
    by_id: &BTreeMap<&str, &'a CatalogSpool>,
    id: &str,
) -> Result<&'a CatalogSpool, ApplicationError> {
    by_id
        .get(id)
        .copied()
        .ok_or_else(|| ApplicationError::InvalidCatalogSpool {
            spool_id: id.to_owned(),
        })
}

fn physical_color(slot: u8, spool: &CatalogSpool) -> Result<PhysicalColor, ApplicationError> {
    let srgb =
        SrgbColor::from_hex(&spool.hex).map_err(|_| ApplicationError::InvalidCatalogSpool {
            spool_id: spool.id.clone(),
        })?;
    Ok(PhysicalColor {
        slot,
        calibration_id: spool.id.clone(),
        name: format!("{} — {}", spool.product, spool.color_name),
        srgb,
        basis: match spool.color_basis {
            ColorBasis::Measured => ColorBasis::Measured,
            ColorBasis::Nominal => ColorBasis::Nominal,
        },
    })
}

fn physical_color_from_spool(slot: u8, spool: &Spool) -> PhysicalColor {
    PhysicalColor {
        slot,
        calibration_id: spool
            .calibration_id
            .clone()
            .unwrap_or_else(|| spool.id.clone()),
        name: spool.display_name.clone(),
        srgb: engine_color(spool.actual_color()),
        basis: if spool.measured_color.is_some() {
            ColorBasis::Measured
        } else {
            ColorBasis::Nominal
        },
    }
}

fn t4_recipe_allowed(candidate: &RecipeMatch, policy: MixingPolicy) -> bool {
    let recipe = &candidate.recipe;
    let uses_t4 = recipe
        .components
        .iter()
        .any(|component| component.slot == 4);
    if !uses_t4 {
        return true;
    }
    match policy {
        MixingPolicy::FullSpectrum | MixingPolicy::FixedComponent => true,
        MixingPolicy::SolidOnlyUntilCalibrated => {
            (recipe.mode == RecipeMode::Solid
                && recipe.components.len() == 1
                && recipe.components[0].slot == 4)
                || (candidate.confidence == PredictionConfidence::Measured
                    && candidate.calibration_sample_id.is_some())
        }
    }
}

fn recipe_target_material(recipe: &MixRecipe, loadout: &MatcherLoadout) -> Option<Material> {
    let materials = recipe
        .components
        .iter()
        .map(|component| {
            if component.slot == 4 {
                loadout.t4_material.clone()
            } else {
                Material::Pla
            }
        })
        .collect::<BTreeSet<_>>();
    (materials.len() == 1)
        .then(|| materials.into_iter().next())
        .flatten()
}

fn planner_cmyx(candidate: RecipeMatch, t4_spool_id: String) -> CmyxColorCandidate {
    if candidate.confidence == PredictionConfidence::Nominal
        && candidate.quality == MatchQuality::Poor
    {
        let reason = format!(
            "The closest uncalibrated CMY+X estimate has ΔE00 {:.2}, above the automatic acceptance limit of 6.00. Choose a dedicated spool, calibrate a recipe, or explicitly review this color.",
            candidate.delta_e_00
        );
        return CmyxColorCandidate {
            recipe: CmyxRecipe::ManualReview {
                reason: reason.clone(),
            },
            calibration_sample_id: None,
            process_compatibility: None,
            required_t4_spool_id: None,
            predicted_color: Some(planner_color(candidate.predicted_srgb)),
            delta_e00: Some(candidate.delta_e_00),
            confidence: ColorConfidence::Nominal,
            warnings: vec![reason],
        };
    }

    planner_cmyx_unchecked(candidate, t4_spool_id)
}

fn planner_cmyx_unchecked(candidate: RecipeMatch, t4_spool_id: String) -> CmyxColorCandidate {
    let calibration_sample_id = candidate.calibration_sample_id.clone();
    let calibrated = calibration_sample_id.is_some();
    let uses_t4 = candidate
        .recipe
        .components
        .iter()
        .any(|component| component.slot == 4);
    let recipe = if candidate.recipe.mode == RecipeMode::Solid {
        let solid_toolhead = toolhead(candidate.recipe.components[0].slot);
        if solid_toolhead == Toolhead::T4 {
            CmyxRecipe::DedicatedT4
        } else {
            CmyxRecipe::Solid {
                toolhead: solid_toolhead,
            }
        }
    } else {
        let mode = match candidate.recipe.mode {
            RecipeMode::Cycle => FullSpectrumMode::Cycle,
            RecipeMode::Ratio => FullSpectrumMode::Ratio,
            RecipeMode::Match => FullSpectrumMode::Match,
            RecipeMode::Gradient => FullSpectrumMode::Gradient,
            RecipeMode::Solid => unreachable!("solid handled above"),
        };
        let sequence = candidate
            .recipe
            .components
            .iter()
            .flat_map(|component| {
                std::iter::repeat_n(toolhead(component.slot), usize::from(component.weight))
            })
            .collect();
        CmyxRecipe::FullSpectrum { mode, sequence }
    };
    CmyxColorCandidate {
        process_compatibility: matches!(&recipe, CmyxRecipe::FullSpectrum { .. })
            .then(full_spectrum_process_compatibility),
        recipe,
        calibration_sample_id,
        required_t4_spool_id: uses_t4.then_some(t4_spool_id),
        predicted_color: Some(planner_color(candidate.predicted_srgb)),
        delta_e00: Some(candidate.delta_e_00),
        confidence: match (calibrated, candidate.confidence) {
            (true, PredictionConfidence::Measured) => ColorConfidence::Calibrated,
            (false, PredictionConfidence::Measured) => ColorConfidence::Measured,
            (_, PredictionConfidence::Nominal) => ColorConfidence::Nominal,
        },
        warnings: match candidate.confidence {
            PredictionConfidence::Measured => Vec::new(),
            PredictionConfidence::Nominal => vec![
                "Nominal optical-mix estimate; print and measure a calibration chart before production conversion."
                    .to_owned(),
            ],
        },
    }
}

fn calibration_context(
    loadout: &[PhysicalColor],
    geometry_context: &CmyxGeometryContext,
) -> CalibrationContext {
    full_spectrum_calibration_context_for_fingerprint(
        loadout_fingerprint(loadout),
        geometry_context,
    )
}

/// Returns the exact active U1/Orca process and profile identity a persistent
/// measurement must carry to qualify the supplied physical loadout.
#[must_use]
pub fn full_spectrum_calibration_context(
    loadout: &CmyxCalibrationLoadout,
    geometry_context: &CmyxGeometryContext,
) -> CalibrationContext {
    full_spectrum_calibration_context_for_fingerprint(loadout.fingerprint(), geometry_context)
}

fn full_spectrum_calibration_context_for_fingerprint(
    loadout_fingerprint: String,
    geometry_context: &CmyxGeometryContext,
) -> CalibrationContext {
    CalibrationContext {
        loadout_fingerprint,
        printer_profile: PrinterProfileIdentity {
            printer_model: U1_MACHINE_PROFILE_NAME.to_owned(),
            printer_variant: U1_MACHINE_SETTING_ID.to_owned(),
            profile_id: FULL_SPECTRUM_SETTING_ID.to_owned(),
            profile_fingerprint: format!(
                "machine:{U1_MACHINE_PROFILE_SHA256}|filament:{FULL_SPECTRUM_PROFILE_SHA256}"
            ),
        },
        nozzle_diameter_microns: 400,
        plate_layer_height_microns: 80,
        subdivision_policy: LayerSubdivisionPolicy::SubdivideMixLayer,
        subdivision_factor: 4,
        effective_sublayer_height_microns: 20,
        process_fingerprint: full_spectrum_process_fingerprint(),
        orientation: geometry_context.orientation.clone(),
        geometry_class: geometry_context.geometry_class.clone(),
    }
}

fn full_spectrum_process_compatibility() -> FullSpectrumProcessCompatibility {
    FullSpectrumProcessCompatibility {
        printer_profile_fingerprint: format!(
            "machine:{U1_MACHINE_PROFILE_SHA256}:{U1_MACHINE_BASE_SHA256}:{TOOLCHANGER_MACHINE_BASE_SHA256}:{KLIPPER_MACHINE_BASE_SHA256}|filament:{FULL_SPECTRUM_PROFILE_SHA256}:{FULL_SPECTRUM_FILAMENT_BASE_SHA256}"
        ),
        plate_layer_height_microns: 80,
        subdivision_policy: FullSpectrumSubdivisionPolicy::SubdivideMixLayer,
        subdivision_factor: 4,
        effective_sublayer_height_microns: 20,
        process_fingerprint: full_spectrum_process_fingerprint(),
    }
}

fn full_spectrum_process_fingerprint() -> String {
    format!(
        "orca:{SUPPORTED_ORCA_VERSION}|machine:{U1_MACHINE_SETTING_ID}:{U1_MACHINE_PROFILE_SHA256}|machine-u1:{U1_MACHINE_BASE_SHA256}|machine-toolchanger:{TOOLCHANGER_MACHINE_BASE_SHA256}|machine-klipper:{KLIPPER_MACHINE_BASE_SHA256}|filament:{FULL_SPECTRUM_SETTING_ID}:{FULL_SPECTRUM_PROFILE_NAME}:{FULL_SPECTRUM_PROFILE_SHA256}|filament-base:{FULL_SPECTRUM_FILAMENT_BASE_SHA256}|process:{FULL_SPECTRUM_PROCESS_SETTING_ID}:{FULL_SPECTRUM_PROCESS_PROFILE_NAME}:{FULL_SPECTRUM_PROCESS_PROFILE_SHA256}|process-base:{FULL_SPECTRUM_PROCESS_BASE_SHA256}|process-common:{U1_PROCESS_COMMON_SHA256}|process-u1:{U1_PROCESS_BASE_SHA256}|layer:80um|subdivide:4|effective:20um"
    )
}

fn toolhead(slot: u8) -> Toolhead {
    match slot {
        1 => Toolhead::T1,
        2 => Toolhead::T2,
        3 => Toolhead::T3,
        4 => Toolhead::T4,
        _ => unreachable!("color engine validates U1 slot range"),
    }
}

fn parse_material(value: Option<&str>) -> Material {
    let Some(value) = value else {
        return Material::Other("Unknown".to_owned());
    };
    let normalized = value.trim().to_ascii_uppercase();
    match normalized.as_str() {
        "PLA" => Material::Pla,
        "PETG" | "PET" => Material::Petg,
        "ABS" => Material::Abs,
        "ASA" => Material::Asa,
        "TPU" => Material::Tpu,
        // Composite, support, and vendor-specific variants must not silently
        // match an ordinary family spool just because their name contains PLA
        // or PETG. They remain distinct until an explicit profile is modeled.
        _ => Material::Other(value.trim().to_owned()),
    }
}

fn planner_role(roles: &[SourceMaterialRole]) -> PlannerMaterialRole {
    if roles.iter().any(|role| {
        matches!(
            role,
            SourceMaterialRole::Support | SourceMaterialRole::SupportInterface
        )
    }) {
        PlannerMaterialRole::Support
    } else if roles.contains(&SourceMaterialRole::Model) {
        PlannerMaterialRole::Cosmetic
    } else {
        PlannerMaterialRole::Unknown
    }
}

fn parse_source_color(value: Option<&str>) -> Option<SrgbColor> {
    let value = value?.trim();
    let digits = value.strip_prefix('#').unwrap_or(value);
    let rgb = if digits.len() == 8 {
        &digits[..6]
    } else {
        digits
    };
    SrgbColor::from_hex(rgb).ok()
}

fn planner_color(color: SrgbColor) -> RgbColor {
    RgbColor::new(color.red, color.green, color.blue)
}

fn engine_color(color: RgbColor) -> SrgbColor {
    SrgbColor::new(color.red, color.green, color.blue)
}

#[cfg(test)]
mod tests {
    use super::*;
    use u1_color_engine::{GeometryClass, RecipeComponent, SampleOrientation};
    use u1_planner::PrinterLoadout;
    use u1_three_mf::{
        AnalysisSummary, AnalysisWarning, ArchiveStatistics, ColorClassification, DialectSupport,
        InputIdentity, ObjectInstance, PlateAnalysis, PrinterInformation, ProcessInformation,
        ProjectDialect, SourceApplication, SourceInformation,
    };

    #[test]
    fn source_colors_accept_bambu_rgba_and_preserve_rgb() {
        assert_eq!(
            parse_source_color(Some("#08ABFBFF")),
            Some(SrgbColor::new(8, 171, 251))
        );
        assert_eq!(parse_source_color(Some("#xyz")), None);
    }

    #[test]
    fn experimental_source_approval_is_exact_and_unsupported_is_never_approvable() {
        let mut analysis = minimal_analysis();
        analysis.source.support = DialectSupport::Experimental;
        analysis.source.application_version = Some("future".into());
        let fingerprint = source_dialect_approval_fingerprint(&analysis);
        assert_eq!(
            validate_source_dialect_for_conversion(&analysis, None),
            Err(SourceDialectConversionError::ExperimentalApprovalRequired)
        );
        assert_eq!(
            validate_source_dialect_for_conversion(&analysis, Some("sha256:stale")),
            Err(SourceDialectConversionError::StaleExperimentalApproval)
        );
        validate_source_dialect_for_conversion(&analysis, Some(&fingerprint)).unwrap();

        analysis.source.support = DialectSupport::Unsupported;
        assert!(matches!(
            validate_source_dialect_for_conversion(&analysis, Some(&fingerprint)),
            Err(SourceDialectConversionError::Unsupported { .. })
        ));
    }

    #[test]
    fn composite_and_support_material_names_do_not_collapse_to_plain_pla() {
        assert_eq!(parse_material(Some("PLA")), Material::Pla);
        assert_eq!(parse_material(Some("PETG")), Material::Petg);
        assert_eq!(
            parse_material(Some("PLA-CF")),
            Material::Other("PLA-CF".to_owned())
        );
        assert_eq!(
            parse_material(Some("Support for PLA")),
            Material::Other("Support for PLA".to_owned())
        );
    }

    #[test]
    fn direct_spools_are_offered_only_with_material_preservation() {
        let catalog = built_in_material_catalog().unwrap();
        let inventory = inventory_from_catalog(&catalog).unwrap();
        let candidates = direct_candidates(SrgbColor::new(8, 171, 251), &Material::Pla, &inventory);
        assert_eq!(candidates[0].spool_id, CYAN_ID);
        assert!(direct_candidates(SrgbColor::new(0, 0, 0), &Material::Petg, &inventory).is_empty());
    }

    #[test]
    fn built_in_library_entries_override_availability_without_redefining_identity() {
        let mut cyan_override = confirmed_spool(CYAN_ID, "#FFFFFF");
        cyan_override.display_name = "Untrusted replacement metadata".to_owned();
        cyan_override.available = false;
        let options = PreliminaryPlanOptions {
            confirmed_spools: vec![cyan_override],
            scope_overrides: vec![ScopePlanningOverride {
                scope_id: "plate-1".to_owned(),
                strategy: Some(ScopeStrategy::CmyxFullSpectrum),
                direct_assignments: Vec::new(),
                approved_cmyx_fallbacks: Vec::new(),
                approved_material_substitutions: Vec::new(),
            }],
            ..PreliminaryPlanOptions::default()
        };

        let input = build_planning_input(&minimal_analysis(), &options).unwrap();
        let cyan = input
            .inventory
            .iter()
            .find(|spool| spool.id == CYAN_ID)
            .expect("canonical cyan remains in inventory");
        assert!(!cyan.available);
        assert_ne!(cyan.display_name, "Untrusted replacement metadata");
        assert_eq!(cyan.nominal_color, RgbColor::new(8, 171, 251));
        assert!(matches!(
            input.scopes[0].requirements[0].cmyx_candidate.recipe,
            CmyxRecipe::ManualReview { .. }
        ));
        let result = plan(&input);
        assert!(result.jobs.is_empty());
        assert!(
            result
                .errors
                .iter()
                .any(|error| { error.code == u1_planner::ErrorCode::CmyxRecipeUnavailable })
        );
    }

    #[test]
    fn unavailable_user_spools_remain_visible_but_are_not_direct_candidates() {
        let mut unavailable = confirmed_spool("library-blue", "#08ABFB");
        unavailable.available = false;
        let options = PreliminaryPlanOptions {
            confirmed_spools: vec![unavailable],
            ..PreliminaryPlanOptions::default()
        };

        let input = build_planning_input(&minimal_analysis(), &options).unwrap();
        let stored = input
            .inventory
            .iter()
            .find(|spool| spool.id == "library-blue")
            .expect("out-of-stock user spool remains in the inventory view");
        assert!(!stored.available);
        assert!(
            input.scopes[0].requirements[0]
                .direct_candidates
                .iter()
                .all(|candidate| candidate.spool_id != "library-blue")
        );
    }

    #[test]
    fn preliminary_bridge_keeps_auto_intent_while_a1_is_disabled() {
        let analysis = minimal_analysis();
        let input = build_planning_input(&analysis, &PreliminaryPlanOptions::default()).unwrap();
        assert_eq!(input.scopes.len(), 1);
        assert_eq!(input.scopes[0].requirements.len(), 1);
        assert_eq!(input.scopes[0].units.len(), 1);
        assert_eq!(
            input.scopes[0].units[0].printer_preference,
            PrinterPreference::Auto
        );
        assert!(!input.config.a1_mini.enabled);

        let result = plan(&input);
        assert_eq!(result.scope_options.len(), 1);
        assert!(!result.jobs.is_empty());
    }

    #[test]
    fn stable_source_unit_override_pins_the_requested_printer() {
        let analysis = minimal_analysis();
        let options = PreliminaryPlanOptions {
            a1_mini: A1MiniConfig {
                enabled: true,
                ..A1MiniConfig::default()
            },
            unit_printer_overrides: vec![UnitPrinterOverride {
                source_unit_id: "source-build-item-1".to_owned(),
                preference: PrinterPreference::A1Mini,
            }],
            ..PreliminaryPlanOptions::default()
        };

        let input = build_planning_input(&analysis, &options).unwrap();
        assert_eq!(
            input.scopes[0].units[0].source_unit_id,
            "source-build-item-1"
        );
        assert_eq!(
            input.scopes[0].units[0].printer_preference,
            PrinterPreference::A1Mini
        );
    }

    #[test]
    fn source_unit_identity_is_independent_of_plate_and_scope_regrouping() {
        let mut analysis = minimal_analysis();
        let first = build_planning_input(&analysis, &PreliminaryPlanOptions::default()).unwrap();
        analysis.plates[0].id = 73;
        analysis.plates[0].name = Some("Regrouped source plate".into());
        let regrouped =
            build_planning_input(&analysis, &PreliminaryPlanOptions::default()).unwrap();

        assert_ne!(first.scopes[0].id, regrouped.scopes[0].id);
        assert_eq!(
            first.scopes[0].units[0].source_unit_id,
            "source-build-item-1"
        );
        assert_eq!(
            first.scopes[0].units[0].source_unit_id,
            regrouped.scopes[0].units[0].source_unit_id
        );
    }

    #[test]
    fn unsafe_or_missing_build_instance_identity_fails_before_planning() {
        let mut unsafe_analysis = minimal_analysis();
        unsafe_analysis.warnings.push(AnalysisWarning {
            code: WarningCode::UnsafeBuildInstanceGraph,
            message: "Source build item 0 is referenced twice".into(),
            entry_path: None,
            object_id: Some(10),
            plate_id: Some(1),
        });
        assert!(matches!(
            build_planning_input(&unsafe_analysis, &PreliminaryPlanOptions::default()),
            Err(ApplicationError::UnsafeSourceGraph { issues })
                if issues == ["Source build item 0 is referenced twice"]
        ));

        let mut missing_identity = minimal_analysis();
        missing_identity.plates[0].instances[0].source_build_item_index = None;
        assert!(matches!(
            build_planning_input(&missing_identity, &PreliminaryPlanOptions::default()),
            Err(ApplicationError::MissingBuildItemIdentity { object_id: 10 })
        ));
    }

    #[test]
    fn source_unit_override_rejects_unknown_and_duplicate_keys() {
        let analysis = minimal_analysis();
        let unknown = PreliminaryPlanOptions {
            unit_printer_overrides: vec![UnitPrinterOverride {
                source_unit_id: "missing-source-unit".to_owned(),
                preference: PrinterPreference::U1,
            }],
            ..PreliminaryPlanOptions::default()
        };
        assert!(matches!(
            build_planning_input(&analysis, &unknown),
            Err(ApplicationError::UnknownSourceUnitOverride { source_unit_id })
                if source_unit_id == "missing-source-unit"
        ));

        let duplicate = PreliminaryPlanOptions {
            unit_printer_overrides: vec![
                UnitPrinterOverride {
                    source_unit_id: "source-build-item-1".to_owned(),
                    preference: PrinterPreference::Auto,
                },
                UnitPrinterOverride {
                    source_unit_id: "source-build-item-1".to_owned(),
                    preference: PrinterPreference::U1,
                },
            ],
            ..PreliminaryPlanOptions::default()
        };
        assert!(matches!(
            build_planning_input(&analysis, &duplicate),
            Err(ApplicationError::DuplicateSourceUnitOverride { source_unit_id })
                if source_unit_id == "source-build-item-1"
        ));
    }

    #[test]
    fn proven_instance_bounds_enable_auto_a1_routing() {
        let options = PreliminaryPlanOptions {
            a1_mini: A1MiniConfig {
                enabled: true,
                ..A1MiniConfig::default()
            },
            ..PreliminaryPlanOptions::default()
        };
        let input = build_planning_input(&minimal_analysis(), &options).unwrap();
        let unit = &input.scopes[0].units[0];
        assert_eq!(
            unit.bounds,
            BoundsMm {
                width: 20.0,
                depth: 30.0,
                height: 40.0,
                clearance_x: 5.0,
                clearance_y: 5.0,
                clearance_z: 0.5,
            }
        );
        assert_eq!(unit.printer_preference, PrinterPreference::Auto);
    }

    #[test]
    fn raw_180mm_bounds_do_not_route_to_a1_without_clearance() {
        let mut analysis = minimal_analysis();
        let boundary = AxisAlignedBounds {
            min: [0.0, 0.0, 0.0],
            max: [180.0, 100.0, 40.0],
        };
        analysis.plates[0].instances[0].printable_bounds = Some(boundary);
        analysis.objects[0].printable_bounds = Some(boundary);
        let options = PreliminaryPlanOptions {
            a1_mini: A1MiniConfig {
                enabled: true,
                ..A1MiniConfig::default()
            },
            ..PreliminaryPlanOptions::default()
        };

        let input = build_planning_input(&analysis, &options).unwrap();
        assert_eq!(
            input.scopes[0].units[0].bounds.required_volume().width,
            190.0
        );
        let result = plan(&input);
        assert!(result.errors.is_empty(), "{:#?}", result.errors);
        assert!(
            result
                .jobs
                .iter()
                .all(|job| job.printer == u1_planner::Printer::U1)
        );
    }

    #[test]
    fn blocked_common_t4_scope_splits_into_reproducible_object_loadouts() {
        let mut analysis = minimal_analysis();
        let black = EffectiveMaterialColor {
            source_slots: vec![1],
            source_profile_ids: Vec::new(),
            material: Some("PLA".to_owned()),
            color: Some("#000000".to_owned()),
            roles: vec![SourceMaterialRole::Model],
        };
        let white = EffectiveMaterialColor {
            source_slots: vec![2],
            source_profile_ids: Vec::new(),
            material: Some("PLA".to_owned()),
            color: Some("#FFFFFF".to_owned()),
            roles: vec![SourceMaterialRole::Model],
        };
        analysis.effective_material_colors = vec![black.clone(), white.clone()];
        analysis.plates[0].effective_material_colors = vec![black.clone(), white.clone()];
        analysis.plates[0].effective_slots = vec![1, 2];
        analysis.plates[0].classification = ColorClassification::MultiColor;
        analysis.objects[0].effective_material_colors = vec![black];
        analysis.objects[0].effective_slots = vec![1];
        analysis.objects[0].object_extruder_slot = Some(1);
        let mut second_object = analysis.objects[0].clone();
        second_object.id = 11;
        second_object.name = Some("White object".to_owned());
        second_object.effective_material_colors = vec![white];
        second_object.effective_slots = vec![2];
        second_object.object_extruder_slot = Some(2);
        analysis.objects.push(second_object);
        let mut second_instance = analysis.plates[0].instances[0].clone();
        second_instance.object_id = 11;
        second_instance.instance_id = 2;
        analysis.plates[0].instances.push(second_instance);
        analysis.plates[0].object_count = 2;
        analysis.summary.object_count = 2;
        analysis.summary.instance_count = 2;
        analysis.summary.multi_color_object_count = 0;
        analysis.summary.mono_object_count = 2;

        let input = build_planning_input(&analysis, &PreliminaryPlanOptions::default()).unwrap();

        assert_eq!(input.scopes.len(), 2, "{:#?}", input.scopes);
        assert!(input.scopes.iter().all(|scope| scope.units.len() == 1));
        assert!(input.scopes.iter().all(|scope| {
            scope.requirements.iter().all(|requirement| {
                !matches!(
                    requirement.cmyx_candidate.recipe,
                    CmyxRecipe::ManualReview { .. }
                )
            })
        }));
        let result = plan(&input);
        assert!(result.errors.is_empty(), "{:#?}", result.errors);
        assert_eq!(result.jobs.len(), 2);
    }

    #[test]
    fn unknown_instance_bounds_remain_u1_pinned_when_a1_is_enabled() {
        let options = PreliminaryPlanOptions {
            a1_mini: A1MiniConfig {
                enabled: true,
                ..A1MiniConfig::default()
            },
            ..PreliminaryPlanOptions::default()
        };
        for unproven_bounds in [
            None,
            Some(AxisAlignedBounds {
                min: [0.0, 0.0, 0.0],
                max: [f64::INFINITY, 10.0, 10.0],
            }),
        ] {
            let mut analysis = minimal_analysis();
            analysis.plates[0].instances[0].printable_bounds = unproven_bounds;
            let input = build_planning_input(&analysis, &options).unwrap();
            let unit = &input.scopes[0].units[0];
            assert_eq!(unit.bounds, BoundsMm::from_size(0.0, 0.0, 0.0));
            assert_eq!(unit.printer_preference, PrinterPreference::U1);
        }
    }

    #[test]
    fn common_loadout_keeps_every_recipe_on_one_t4_spool() {
        let catalog = built_in_material_catalog().unwrap();
        let inventory = inventory_from_catalog(&catalog).unwrap();
        let matcher = CmyxMatcher::new(
            &catalog,
            &inventory,
            &[],
            Vec::new(),
            CmyxGeometryContext::default(),
        )
        .unwrap();
        let sources = vec![
            EffectiveMaterialColor {
                source_slots: vec![1],
                source_profile_ids: Vec::new(),
                material: Some("PLA".to_owned()),
                color: Some("#FFFFFF".to_owned()),
                roles: vec![SourceMaterialRole::Model],
            },
            EffectiveMaterialColor {
                source_slots: vec![2],
                source_profile_ids: Vec::new(),
                material: Some("PLA".to_owned()),
                color: Some("#8E9089".to_owned()),
                roles: vec![SourceMaterialRole::Model],
            },
        ];
        let selected_t4 = matcher.best_loadout_for_sources(&sources).unwrap();
        for source in sources {
            let color = parse_source_color(source.color.as_deref()).unwrap();
            let candidate = matcher
                .best_in_loadout(selected_t4, &Material::Pla, color, false)
                .unwrap();
            assert!(
                candidate.required_t4_spool_id.is_none()
                    || candidate.required_t4_spool_id.as_deref() == Some(selected_t4)
            );
        }
    }

    #[test]
    fn identical_colors_with_distinct_profiles_survive_scope_bridging() {
        let mut analysis = minimal_analysis();
        let basic = EffectiveMaterialColor {
            source_slots: vec![1],
            source_profile_ids: vec!["PLA Basic @U1".to_owned()],
            material: Some("PLA".to_owned()),
            color: Some("#808080".to_owned()),
            roles: vec![SourceMaterialRole::Model],
        };
        let matte = EffectiveMaterialColor {
            source_slots: vec![2],
            source_profile_ids: vec!["PLA Matte @U1".to_owned()],
            material: Some("PLA".to_owned()),
            color: Some("#808080".to_owned()),
            roles: vec![SourceMaterialRole::Model],
        };
        analysis.effective_material_colors = vec![basic.clone(), matte.clone()];
        analysis.plates[0].effective_material_colors = vec![basic.clone(), matte.clone()];
        analysis.plates[0].effective_slots = vec![1, 2];
        analysis.plates[0].classification = ColorClassification::MultiColor;
        analysis.objects[0].effective_material_colors = vec![basic, matte];
        analysis.objects[0].effective_slots = vec![1, 2];
        analysis.objects[0].classification = ColorClassification::MultiColor;

        let objects = analysis
            .objects
            .iter()
            .map(|object| (object.id, object))
            .collect::<BTreeMap<_, _>>();
        let merged = merged_instance_colors(
            &analysis.plates[0].instances,
            &objects,
            &analysis.plates[0].effective_material_colors,
        );
        assert_eq!(merged.len(), 2);

        let input = build_planning_input(&analysis, &PreliminaryPlanOptions::default()).unwrap();
        let scope = &input.scopes[0];
        assert_eq!(scope.requirements.len(), 2);
        assert_eq!(scope.units[0].requirement_ids.len(), 2);
        assert!(
            scope
                .requirements
                .iter()
                .any(|requirement| { requirement.source_profile_ids == vec!["PLA Basic @U1"] })
        );
        assert!(
            scope
                .requirements
                .iter()
                .any(|requirement| { requirement.source_profile_ids == vec!["PLA Matte @U1"] })
        );
    }

    #[test]
    fn unconfirmed_source_spools_do_not_enable_direct_printing() {
        let mut analysis = minimal_analysis();
        for source in &mut analysis.effective_material_colors {
            source.material = Some("PETG".to_owned());
            source.color = Some("#202020".to_owned());
        }
        for plate in &mut analysis.plates {
            for source in &mut plate.effective_material_colors {
                source.material = Some("PETG".to_owned());
                source.color = Some("#202020".to_owned());
            }
        }
        for object in &mut analysis.objects {
            for source in &mut object.effective_material_colors {
                source.material = Some("PETG".to_owned());
                source.color = Some("#202020".to_owned());
            }
        }

        let input = build_planning_input(&analysis, &PreliminaryPlanOptions::default()).unwrap();
        assert_eq!(input.scopes[0].strategy, ScopeStrategy::DirectSpools);
        let placeholder = input
            .inventory
            .iter()
            .find(|spool| spool.material == Material::Petg && spool.id == "source-petg-202020")
            .expect("source metadata should remain visible as an inventory placeholder");
        assert!(!placeholder.available);
        let result = plan(&input);
        assert!(result.jobs.is_empty());
        assert!(result.errors.iter().any(|error| {
            error.code == u1_planner::ErrorCode::RequestedDirectSpoolsUnavailable
        }));
        assert!(matches!(
            result.scope_options[0].direct_spools,
            u1_planner::DirectSpoolEligibility::Ineligible { .. }
        ));
    }

    #[test]
    fn petg_without_a_real_spool_exposes_and_schedules_only_double_approved_pla_fallback() {
        let mut analysis = minimal_analysis();
        for source in &mut analysis.effective_material_colors {
            source.material = Some("PETG".to_owned());
            source.color = Some("#202020".to_owned());
        }
        for plate in &mut analysis.plates {
            for source in &mut plate.effective_material_colors {
                source.material = Some("PETG".to_owned());
                source.color = Some("#202020".to_owned());
            }
        }
        for object in &mut analysis.objects {
            for source in &mut object.effective_material_colors {
                source.material = Some("PETG".to_owned());
                source.color = Some("#202020".to_owned());
            }
        }

        let initial = build_planning_input(&analysis, &PreliminaryPlanOptions::default()).unwrap();
        let scope = &initial.scopes[0];
        let requirement = &scope.requirements[0];
        let fallback = requirement
            .best_effort_cmyx_candidate
            .as_ref()
            .expect("PETG without a confirmed PETG spool must expose a PLA best-effort option");
        assert_eq!(fallback.target_material, Material::Pla);
        assert!(!fallback.candidate_id.is_empty());
        let candidate_id = fallback.candidate_id.clone();
        let requirement_id = requirement.id.clone();
        let scope_id = scope.id.clone();

        let approved_options = PreliminaryPlanOptions {
            scope_overrides: vec![ScopePlanningOverride {
                scope_id,
                strategy: Some(ScopeStrategy::CmyxFullSpectrum),
                direct_assignments: Vec::new(),
                approved_cmyx_fallbacks: vec![CmyxFallbackApproval {
                    requirement_id: requirement_id.clone(),
                    candidate_id: candidate_id.clone(),
                }],
                approved_material_substitutions: vec![MaterialSubstitutionApproval {
                    requirement_id,
                    candidate_id,
                    source_material: Material::Petg,
                    target_material: Material::Pla,
                    acknowledged: true,
                }],
            }],
            ..PreliminaryPlanOptions::default()
        };
        let approved_input = build_planning_input(&analysis, &approved_options).unwrap();
        let result = plan(&approved_input);
        assert!(result.errors.is_empty(), "{:#?}", result.errors);
        assert_eq!(result.jobs[0].printable_materials, vec![Material::Pla]);
        assert_eq!(
            result.jobs[0].color_mappings[0].source_material,
            Material::Petg
        );
        assert_eq!(
            result.jobs[0].color_mappings[0].actual_material,
            Some(Material::Pla)
        );
    }

    #[test]
    fn stale_fallback_approval_is_rejected_after_inventory_changes_the_candidate() {
        let mut analysis = minimal_analysis();
        for source in &mut analysis.effective_material_colors {
            source.material = Some("PETG".to_owned());
            source.color = Some("#202020".to_owned());
        }
        for plate in &mut analysis.plates {
            for source in &mut plate.effective_material_colors {
                source.material = Some("PETG".to_owned());
                source.color = Some("#202020".to_owned());
            }
        }
        for object in &mut analysis.objects {
            for source in &mut object.effective_material_colors {
                source.material = Some("PETG".to_owned());
                source.color = Some("#202020".to_owned());
            }
        }
        let initial = build_planning_input(&analysis, &PreliminaryPlanOptions::default()).unwrap();
        let requirement = &initial.scopes[0].requirements[0];
        let stale_candidate_id = requirement
            .best_effort_cmyx_candidate
            .as_ref()
            .expect("initial fallback must exist")
            .candidate_id
            .clone();
        let requirement_id = requirement.id.clone();
        let options = PreliminaryPlanOptions {
            confirmed_spools: vec![Spool {
                id: "new-petg-black".to_owned(),
                calibration_id: None,
                display_name: "New PETG Black".to_owned(),
                color_name: Some("Black".to_owned()),
                material: Material::Petg,
                nominal_color: RgbColor::new(32, 32, 32),
                measured_color: Some(RgbColor::new(32, 32, 32)),
                sku: None,
                profile_id: Some("PETG profile".to_owned()),
                available: true,
            }],
            scope_overrides: vec![ScopePlanningOverride {
                scope_id: initial.scopes[0].id.clone(),
                strategy: Some(ScopeStrategy::CmyxFullSpectrum),
                direct_assignments: Vec::new(),
                approved_cmyx_fallbacks: vec![CmyxFallbackApproval {
                    requirement_id,
                    candidate_id: stale_candidate_id,
                }],
                approved_material_substitutions: Vec::new(),
            }],
            ..PreliminaryPlanOptions::default()
        };

        let changed = build_planning_input(&analysis, &options).unwrap();
        let result = plan(&changed);
        assert!(
            result
                .errors
                .iter()
                .any(|error| { error.code == u1_planner::ErrorCode::InvalidCmyxFallbackApproval })
        );
    }

    #[test]
    fn fallback_fingerprint_uses_effective_identity_instead_of_alias_id() {
        let mut analysis = minimal_analysis();
        for source in &mut analysis.effective_material_colors {
            source.material = Some("PETG".to_owned());
            source.color = Some("#202020".to_owned());
            source.source_profile_ids = vec!["Shared PETG profile".to_owned()];
        }
        for plate in &mut analysis.plates {
            for source in &mut plate.effective_material_colors {
                source.material = Some("PETG".to_owned());
                source.color = Some("#202020".to_owned());
                source.source_profile_ids = vec!["Shared PETG profile".to_owned()];
            }
        }
        for object in &mut analysis.objects {
            for source in &mut object.effective_material_colors {
                source.material = Some("PETG".to_owned());
                source.color = Some("#202020".to_owned());
                source.source_profile_ids = vec!["Shared PETG profile".to_owned()];
            }
        }
        let mut input =
            build_planning_input(&analysis, &PreliminaryPlanOptions::default()).unwrap();
        let mut alias = input.scopes[0].requirements[0].clone();
        alias.id = "alias-requirement".to_owned();
        alias.source_slots = vec!["F99".to_owned()];
        alias
            .best_effort_cmyx_candidate
            .as_mut()
            .expect("fallback must exist")
            .candidate_id
            .clear();
        input.scopes[0].requirements[0]
            .best_effort_cmyx_candidate
            .as_mut()
            .expect("fallback must exist")
            .candidate_id
            .clear();
        input.scopes[0].requirements.push(alias);

        assign_best_effort_candidate_ids(&mut input.scopes);
        let first = input.scopes[0].requirements[0]
            .best_effort_cmyx_candidate
            .as_ref()
            .unwrap();
        let alias = input.scopes[0].requirements[1]
            .best_effort_cmyx_candidate
            .as_ref()
            .unwrap();
        assert_eq!(first.candidate_id, alias.candidate_id);

        let original_candidate_id = first.candidate_id.clone();
        let calibrated = input.scopes[0].requirements[0]
            .best_effort_cmyx_candidate
            .as_mut()
            .unwrap();
        calibrated.candidate.calibration_sample_id = Some("measured-fallback-r1".to_owned());
        calibrated.candidate_id.clear();
        assign_best_effort_candidate_ids(&mut input.scopes);
        assert_ne!(
            input.scopes[0].requirements[0]
                .best_effort_cmyx_candidate
                .as_ref()
                .unwrap()
                .candidate_id,
            original_candidate_id
        );
    }

    #[test]
    fn confirmed_petg_t4_keeps_cmy_loaded_for_an_exact_solid() {
        let mut analysis = minimal_analysis();
        for source in &mut analysis.effective_material_colors {
            source.material = Some("PETG".to_owned());
            source.color = Some("#202020".to_owned());
        }
        for plate in &mut analysis.plates {
            for source in &mut plate.effective_material_colors {
                source.material = Some("PETG".to_owned());
                source.color = Some("#202020".to_owned());
            }
        }
        for object in &mut analysis.objects {
            for source in &mut object.effective_material_colors {
                source.material = Some("PETG".to_owned());
                source.color = Some("#202020".to_owned());
            }
        }
        let options = PreliminaryPlanOptions {
            confirmed_spools: vec![Spool {
                id: "confirmed-petg-black".to_owned(),
                calibration_id: None,
                display_name: "Confirmed PETG Black".to_owned(),
                color_name: Some("Black".to_owned()),
                material: Material::Petg,
                nominal_color: RgbColor::new(32, 32, 32),
                measured_color: Some(RgbColor::new(32, 32, 32)),
                sku: Some("PETG-BLACK".to_owned()),
                profile_id: Some("PETG profile".to_owned()),
                available: true,
            }],
            ..PreliminaryPlanOptions::default()
        };

        let input = build_planning_input(&analysis, &options).unwrap();
        assert_eq!(input.scopes[0].strategy, ScopeStrategy::Auto);
        assert!(matches!(
            input.scopes[0].requirements[0].cmyx_candidate.recipe,
            CmyxRecipe::DedicatedT4
        ));
        assert_eq!(
            input.scopes[0].requirements[0]
                .cmyx_candidate
                .required_t4_spool_id
                .as_deref(),
            Some("confirmed-petg-black")
        );
        let result = plan(&input);
        assert!(result.errors.is_empty(), "{:#?}", result.errors);
        let PrinterLoadout::U1 { loadout } = &result.jobs[0].loadout else {
            panic!("confirmed PETG T4 remains a U1 CMY+T4 job");
        };
        assert_eq!(loadout.spool(Toolhead::T1), Some(CYAN_ID));
        assert_eq!(loadout.spool(Toolhead::T2), Some(MAGENTA_ID));
        assert_eq!(loadout.spool(Toolhead::T3), Some(YELLOW_ID));
        assert_eq!(loadout.spool(Toolhead::T4), Some("confirmed-petg-black"));
    }

    #[test]
    fn poor_nominal_cmyx_requires_manual_review_and_blocks_scheduling() {
        let candidate = RecipeMatch {
            recipe: MixRecipe::solid(1),
            predicted_srgb: SrgbColor::new(8, 171, 251),
            delta_e_00: 6.01,
            quality: MatchQuality::Poor,
            confidence: PredictionConfidence::Nominal,
            calibration_sample_id: None,
        };
        let guarded = planner_cmyx(candidate, GREY_ID.to_owned());
        assert!(matches!(guarded.recipe, CmyxRecipe::ManualReview { .. }));
        assert_eq!(guarded.delta_e00, Some(6.01));
        assert_eq!(guarded.confidence, ColorConfidence::Nominal);

        let mut input =
            build_planning_input(&minimal_analysis(), &PreliminaryPlanOptions::default()).unwrap();
        input.scopes[0].requirements[0].cmyx_candidate = guarded;
        let result = plan(&input);
        assert!(result.jobs.is_empty());
        assert!(
            result
                .errors
                .iter()
                .any(|error| { error.code == u1_planner::ErrorCode::CmyxRecipeUnavailable })
        );
    }

    #[test]
    fn nominal_full_spectrum_candidate_carries_the_adapter_process_contract() {
        let candidate = RecipeMatch {
            recipe: MixRecipe {
                mode: RecipeMode::Ratio,
                components: vec![
                    u1_color_engine::RecipeComponent { slot: 1, weight: 1 },
                    u1_color_engine::RecipeComponent { slot: 2, weight: 1 },
                ],
            },
            predicted_srgb: SrgbColor::new(80, 90, 180),
            delta_e_00: 2.0,
            quality: MatchQuality::Good,
            confidence: PredictionConfidence::Nominal,
            calibration_sample_id: None,
        };
        let planned = planner_cmyx(candidate, GREY_ID.to_owned());
        assert!(matches!(planned.recipe, CmyxRecipe::FullSpectrum { .. }));
        assert_eq!(
            planned.process_compatibility,
            Some(full_spectrum_process_compatibility())
        );

        let input =
            build_planning_input(&minimal_analysis(), &PreliminaryPlanOptions::default()).unwrap();
        assert_eq!(
            input.config.supported_full_spectrum_processes,
            vec![full_spectrum_process_compatibility()]
        );
    }

    #[test]
    fn exact_user_sample_calibrates_only_its_black_t4_recipe() {
        let target = "#2A6F51";
        let geometry_context = calibrated_geometry_context();
        let record = calibrated_t4_record(
            "black-chart-r1",
            "panchroma-basic-black",
            target,
            &geometry_context,
        );
        let options = PreliminaryPlanOptions {
            confirmed_calibration_samples: vec![record.clone()],
            cmyx_geometry_context: geometry_context,
            ..PreliminaryPlanOptions::default()
        };

        let input = build_planning_input(&analysis_with_colors(&[target]), &options).unwrap();
        let candidate = &input.scopes[0].requirements[0].cmyx_candidate;
        assert_eq!(candidate.confidence, ColorConfidence::Calibrated);
        assert_eq!(
            candidate.calibration_sample_id.as_deref(),
            Some("black-chart-r1")
        );
        assert_eq!(candidate.delta_e00, Some(0.0));
        assert_eq!(
            candidate.required_t4_spool_id.as_deref(),
            Some("panchroma-basic-black")
        );
        assert_eq!(
            candidate.recipe,
            CmyxRecipe::FullSpectrum {
                mode: FullSpectrumMode::Ratio,
                sequence: vec![Toolhead::T1, Toolhead::T1, Toolhead::T1, Toolhead::T4],
            }
        );
        assert_eq!(
            candidate.process_compatibility,
            Some(full_spectrum_process_compatibility())
        );
        assert!(candidate.warnings.is_empty());

        let result = plan(&input);
        assert!(result.errors.is_empty(), "{:#?}", result.errors);
        assert_eq!(
            result.jobs[0].color_mappings[0].confidence,
            ColorConfidence::Calibrated
        );
        assert_eq!(
            result.jobs[0].color_mappings[0]
                .cmyx_comparison
                .calibration_sample_id
                .as_deref(),
            Some("black-chart-r1")
        );

        let preparation = u1_orca_adapter::prepare_u1_full_spectrum_conversion(&input, &result)
            .expect("the exact measured plan must prepare for Full Spectrum output");
        let artifact = &preparation.artifacts[0];
        assert_eq!(
            artifact.recipe_calibration_sample_ids,
            vec!["black-chart-r1".to_owned()]
        );
        assert_eq!(
            artifact.assignments[0].calibration_sample_id.as_deref(),
            Some("black-chart-r1")
        );
        assert_eq!(
            artifact.recipe_table.targets[0]
                .calibration_sample_id
                .as_deref(),
            Some("black-chart-r1")
        );
        assert_eq!(
            artifact.recipe_table.definitions[0]
                .calibration_sample_id
                .as_deref(),
            Some("black-chart-r1")
        );
        let evidence = serde_json::to_value(artifact).unwrap();
        assert_eq!(evidence["recipeCalibrationSampleIds"][0], "black-chart-r1");

        let unrelated = build_planning_input(
            &analysis_with_colors(&[target]),
            &PreliminaryPlanOptions {
                confirmed_calibration_samples: vec![record],
                cmyx_geometry_context: CmyxGeometryContext {
                    orientation: SampleOrientation::Flat,
                    geometry_class: GeometryClass::Volumetric,
                },
                ..PreliminaryPlanOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            unrelated.scopes[0].requirements[0]
                .cmyx_candidate
                .calibration_sample_id,
            None
        );
    }

    #[test]
    fn black_t4_calibration_does_not_transfer_or_unlock_other_recipes() {
        let target = SrgbColor::from_hex("#2A6F51").unwrap();
        let geometry_context = calibrated_geometry_context();
        let record = calibrated_t4_record(
            "black-chart-r1",
            "panchroma-basic-black",
            &target.to_hex(),
            &geometry_context,
        );
        let catalog = built_in_material_catalog().unwrap();
        let inventory = inventory_from_catalog(&catalog).unwrap();
        let samples = UserCmyxCalibrationLibrary::from_records(vec![record.clone()])
            .unwrap()
            .engine_samples()
            .unwrap();

        let exact = CmyxMatcher::new(
            &catalog,
            &inventory,
            &[],
            samples.clone(),
            geometry_context.clone(),
        )
        .unwrap();
        let exact_match = exact
            .best_in_loadout("panchroma-basic-black", &Material::Pla, target, false)
            .unwrap();
        assert_eq!(exact_match.confidence, ColorConfidence::Calibrated);
        assert_eq!(
            exact_match.calibration_sample_id.as_deref(),
            Some("black-chart-r1")
        );

        let another_loadout = exact
            .best_in_loadout("panchroma-basic-white", &Material::Pla, target, false)
            .unwrap();
        assert_ne!(another_loadout.confidence, ColorConfidence::Calibrated);
        assert_eq!(another_loadout.calibration_sample_id, None);
        assert!(!mixed_recipe_uses_t4(&another_loadout.recipe));

        let another_geometry = CmyxMatcher::new(
            &catalog,
            &inventory,
            &[],
            samples,
            CmyxGeometryContext {
                orientation: SampleOrientation::Flat,
                ..geometry_context.clone()
            },
        )
        .unwrap()
        .best_in_loadout("panchroma-basic-black", &Material::Pla, target, false)
        .unwrap();
        assert_ne!(another_geometry.confidence, ColorConfidence::Calibrated);
        assert_eq!(another_geometry.calibration_sample_id, None);
        assert!(!mixed_recipe_uses_t4(&another_geometry.recipe));

        let mut another_process_record = record;
        another_process_record.context.process_fingerprint = "another-process".to_owned();
        let another_process_samples =
            UserCmyxCalibrationLibrary::from_records(vec![another_process_record])
                .unwrap()
                .engine_samples()
                .unwrap();
        let another_process = CmyxMatcher::new(
            &catalog,
            &inventory,
            &[],
            another_process_samples,
            geometry_context,
        )
        .unwrap()
        .best_in_loadout("panchroma-basic-black", &Material::Pla, target, false)
        .unwrap();
        assert_ne!(another_process.confidence, ColorConfidence::Calibrated);
        assert!(!mixed_recipe_uses_t4(&another_process.recipe));
    }

    #[test]
    fn physical_color_uses_the_persisted_batch_calibration_identity() {
        let mut spool = confirmed_spool("grey-spool", "#9199A4");
        spool.calibration_id =
            Some("spool-calibration:33333333-3333-4333-8333-333333333333".to_owned());

        let physical = physical_color_from_spool(4, &spool);

        assert_eq!(physical.slot, 4);
        assert_eq!(
            physical.calibration_id,
            "spool-calibration:33333333-3333-4333-8333-333333333333"
        );
        assert_eq!(spool.id, "grey-spool");
    }

    #[test]
    fn solid_only_t4_policy_accepts_only_the_exact_measured_candidate() {
        let recipe = MixRecipe {
            mode: RecipeMode::Ratio,
            components: vec![
                RecipeComponent { slot: 1, weight: 3 },
                RecipeComponent { slot: 4, weight: 1 },
            ],
        };
        let mut candidate = RecipeMatch {
            recipe,
            predicted_srgb: SrgbColor::new(42, 111, 81),
            delta_e_00: 0.0,
            quality: MatchQuality::Good,
            confidence: PredictionConfidence::Measured,
            calibration_sample_id: None,
        };
        assert!(!t4_recipe_allowed(
            &candidate,
            MixingPolicy::SolidOnlyUntilCalibrated
        ));
        candidate.calibration_sample_id = Some("exact-chart-r1".to_owned());
        assert!(t4_recipe_allowed(
            &candidate,
            MixingPolicy::SolidOnlyUntilCalibrated
        ));
        candidate.confidence = PredictionConfidence::Nominal;
        assert!(!t4_recipe_allowed(
            &candidate,
            MixingPolicy::SolidOnlyUntilCalibrated
        ));
    }

    #[test]
    fn four_confirmed_spools_unlock_direct_with_requested_toolheads() {
        let colors = ["#102030", "#405060", "#708090", "#A0B0C0"];
        let analysis = analysis_with_colors(&colors);
        let confirmed_spools = colors
            .iter()
            .enumerate()
            .map(|(index, color)| confirmed_spool(&format!("custom-{index}"), color))
            .collect::<Vec<_>>();
        let requested_toolheads = [Toolhead::T4, Toolhead::T2, Toolhead::T1, Toolhead::T3];
        let direct_assignments = requested_toolheads
            .iter()
            .enumerate()
            .map(|(index, toolhead)| DirectAssignmentRequest {
                requirement_id: format!("plate-1-requirement-{}", index + 1),
                spool_id: format!("custom-{index}"),
                toolhead: Some(*toolhead),
                allow_material_substitution: false,
            })
            .collect();
        let options = PreliminaryPlanOptions {
            confirmed_spools: confirmed_spools.clone(),
            scope_overrides: vec![ScopePlanningOverride {
                scope_id: "plate-1".to_owned(),
                strategy: Some(ScopeStrategy::DirectSpools),
                direct_assignments,
                approved_cmyx_fallbacks: Vec::new(),
                approved_material_substitutions: Vec::new(),
            }],
            ..PreliminaryPlanOptions::default()
        };

        let input = build_planning_input(&analysis, &options).unwrap();
        let mut reversed_options = options.clone();
        reversed_options.confirmed_spools.reverse();
        let reversed_input = build_planning_input(&analysis, &reversed_options).unwrap();
        assert_eq!(input.inventory, reversed_input.inventory);

        let result = plan(&input);
        assert!(result.errors.is_empty(), "{:#?}", result.errors);
        let u1_planner::DirectSpoolEligibility::Eligible { assignments } =
            &result.scope_options[0].direct_spools
        else {
            panic!("four confirmed spools should make the scope Direct-eligible");
        };
        let actual = assignments
            .iter()
            .map(|assignment| (assignment.spool_id.as_str(), assignment.toolhead))
            .collect::<BTreeMap<_, _>>();
        for (index, expected_toolhead) in requested_toolheads.iter().enumerate() {
            assert_eq!(
                actual.get(format!("custom-{index}").as_str()),
                Some(expected_toolhead)
            );
        }
        assert!(
            result
                .jobs
                .iter()
                .all(|job| job.strategy == u1_planner::ColorStrategy::DirectSpools)
        );
    }

    #[test]
    fn five_mono_object_colors_split_into_direct_eligible_palette_scopes() {
        let colors = ["#08ABFB", "#D93B90", "#F9ED3D", "#9199A4", "#080A0D"];
        let mut analysis = minimal_analysis();
        let effective_colors = colors
            .iter()
            .enumerate()
            .map(|(index, color)| EffectiveMaterialColor {
                source_slots: vec![(index + 1) as u16],
                source_profile_ids: Vec::new(),
                material: Some("PLA".to_owned()),
                color: Some((*color).to_owned()),
                roles: vec![SourceMaterialRole::Model],
            })
            .collect::<Vec<_>>();
        let template_object = analysis.objects[0].clone();
        let template_instance = analysis.plates[0].instances[0].clone();
        analysis.objects = effective_colors
            .iter()
            .enumerate()
            .map(|(index, color)| {
                let mut object = template_object.clone();
                object.id = 10 + index as u32;
                object.name = Some(format!("Mono object {}", index + 1));
                object.object_extruder_slot = Some((index + 1) as u16);
                object.effective_slots = vec![(index + 1) as u16];
                object.effective_material_colors = vec![color.clone()];
                object
            })
            .collect();
        analysis.plates[0].instances = analysis
            .objects
            .iter()
            .enumerate()
            .map(|(index, object)| {
                let mut instance = template_instance.clone();
                instance.object_id = object.id;
                instance.instance_id = (index + 1) as u32;
                instance
            })
            .collect();
        analysis.effective_material_colors = effective_colors.clone();
        analysis.plates[0].effective_material_colors = effective_colors;
        analysis.plates[0].effective_slots = (1..=5).collect();
        analysis.plates[0].object_count = 5;
        analysis.plates[0].classification = ColorClassification::MultiColor;
        analysis.summary.object_count = 5;
        analysis.summary.instance_count = 5;
        analysis.summary.mono_object_count = 5;
        analysis.summary.multi_color_object_count = 0;
        analysis.summary.declared_filament_count = 5;
        analysis.summary.used_filament_count = 5;

        let input = build_planning_input(&analysis, &PreliminaryPlanOptions::default()).unwrap();
        assert!(input.scopes.len() >= 2, "{:#?}", input.scopes);
        assert!(
            input
                .scopes
                .iter()
                .all(|scope| scope.requirements.len() <= 4)
        );
        assert_eq!(
            input
                .scopes
                .iter()
                .flat_map(|scope| scope.units.iter())
                .count(),
            5
        );

        let result = plan(&input);
        assert!(result.scope_options.iter().all(|scope| {
            scope.direct_pair_count <= 4
                && matches!(
                    &scope.direct_spools,
                    u1_planner::DirectSpoolEligibility::Eligible { .. }
                )
        }));
    }

    #[test]
    fn scope_override_is_isolated_from_other_scopes() {
        let mut analysis = minimal_analysis();
        let mut second_plate = analysis.plates[0].clone();
        second_plate.id = 2;
        second_plate.name = Some("Second plate".to_owned());
        second_plate.instances[0].instance_id = 2;
        analysis.plates.push(second_plate);
        analysis.summary.plate_count = 2;
        analysis.summary.instance_count = 2;
        analysis.objects[0].instance_count = 2;
        analysis.objects[0].plate_ids = vec![1, 2];

        let options = PreliminaryPlanOptions {
            confirmed_spools: vec![confirmed_spool("custom-cyan", "#08ABFB")],
            scope_overrides: vec![ScopePlanningOverride {
                scope_id: "plate-1".to_owned(),
                strategy: Some(ScopeStrategy::DirectSpools),
                direct_assignments: vec![DirectAssignmentRequest {
                    requirement_id: "plate-1-requirement-1".to_owned(),
                    spool_id: "custom-cyan".to_owned(),
                    toolhead: Some(Toolhead::T4),
                    allow_material_substitution: false,
                }],
                approved_cmyx_fallbacks: Vec::new(),
                approved_material_substitutions: Vec::new(),
            }],
            ..PreliminaryPlanOptions::default()
        };

        let input = build_planning_input(&analysis, &options).unwrap();
        let scopes = input
            .scopes
            .iter()
            .map(|scope| (scope.id.as_str(), scope))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(scopes["plate-1"].strategy, ScopeStrategy::DirectSpools);
        assert_eq!(scopes["plate-1"].direct_assignments.len(), 1);
        assert_eq!(scopes["plate-2"].strategy, ScopeStrategy::Auto);
        assert!(scopes["plate-2"].direct_assignments.is_empty());
    }

    #[test]
    fn alternative_joint_plates_are_excluded_until_selected() {
        let analysis = analysis_with_alternative_joint_plates();

        assert_eq!(
            alternative_plate_candidates(&analysis),
            vec![
                AlternativePlateCandidate {
                    id: 7,
                    name: "Updated Ball Joints".to_owned(),
                },
                AlternativePlateCandidate {
                    id: 9,
                    name: "Alternate Joints".to_owned(),
                },
            ]
        );

        let default_input =
            build_planning_input(&analysis, &PreliminaryPlanOptions::default()).unwrap();
        assert_eq!(
            default_input
                .scopes
                .iter()
                .map(|scope| scope.id.as_str())
                .collect::<Vec<_>>(),
            vec!["plate-1"]
        );

        let selected_input = build_planning_input(
            &analysis,
            &PreliminaryPlanOptions {
                included_alternative_plate_ids: vec![9],
                ..PreliminaryPlanOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            selected_input
                .scopes
                .iter()
                .map(|scope| scope.id.as_str())
                .collect::<Vec<_>>(),
            vec!["plate-1", "plate-9"]
        );
        assert_eq!(analysis.summary.plate_count, 3);
    }

    #[test]
    fn alternative_plate_selection_rejects_unknown_and_regular_plate_ids() {
        let analysis = analysis_with_alternative_joint_plates();
        for plate_id in [1, 404] {
            let options = PreliminaryPlanOptions {
                included_alternative_plate_ids: vec![plate_id],
                ..PreliminaryPlanOptions::default()
            };
            assert!(matches!(
                build_planning_input(&analysis, &options),
                Err(ApplicationError::InvalidAlternativePlateSelection {
                    plate_id: rejected
                }) if rejected == plate_id
            ));
        }
    }

    #[test]
    fn planning_options_validate_duplicate_spools_and_unknown_scopes() {
        let duplicate_options = PreliminaryPlanOptions {
            confirmed_spools: vec![
                confirmed_spool("duplicate", "#102030"),
                confirmed_spool("duplicate", "#405060"),
            ],
            ..PreliminaryPlanOptions::default()
        };
        assert!(matches!(
            build_planning_input(&minimal_analysis(), &duplicate_options),
            Err(ApplicationError::DuplicateSpoolId { spool_id, .. }) if spool_id == "duplicate"
        ));

        let duplicate_loaded_options = PreliminaryPlanOptions {
            current_toolheads: CurrentToolheadState {
                slots: [
                    ToolheadSlotState::Loaded("same-physical-spool".to_owned()),
                    ToolheadSlotState::Loaded("same-physical-spool".to_owned()),
                    ToolheadSlotState::Unknown,
                    ToolheadSlotState::Unknown,
                ],
            },
            ..PreliminaryPlanOptions::default()
        };
        assert!(matches!(
            build_planning_input(&minimal_analysis(), &duplicate_loaded_options),
            Err(ApplicationError::DuplicateLoadedSpool { spool_id })
                if spool_id == "same-physical-spool"
        ));

        let unknown_scope_options = PreliminaryPlanOptions {
            scope_overrides: vec![ScopePlanningOverride {
                scope_id: "missing-scope".to_owned(),
                strategy: Some(ScopeStrategy::DirectSpools),
                direct_assignments: Vec::new(),
                approved_cmyx_fallbacks: Vec::new(),
                approved_material_substitutions: Vec::new(),
            }],
            ..PreliminaryPlanOptions::default()
        };
        assert!(matches!(
            build_planning_input(&minimal_analysis(), &unknown_scope_options),
            Err(ApplicationError::UnknownScopeOverride { scope_id }) if scope_id == "missing-scope"
        ));
    }

    #[test]
    fn planning_options_are_serde_round_trip_ready() {
        let mut options = PreliminaryPlanOptions::default();
        options.a1_mini.enabled = true;
        options.a1_mini.route_fast_mono = false;
        options.a1_mini.current_spool_id = Some("custom".to_owned());
        options.confirmed_spools = vec![confirmed_spool("custom", "#123456")];
        options.cmyx_geometry_context = calibrated_geometry_context();
        options.confirmed_calibration_samples = vec![calibrated_t4_record(
            "black-chart-r1",
            "panchroma-basic-black",
            "#2A6F51",
            &options.cmyx_geometry_context,
        )];
        let encoded = serde_json::to_string(&options).unwrap();
        let decoded: PreliminaryPlanOptions = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, options);
        let input = build_planning_input(&minimal_analysis(), &decoded).unwrap();
        assert_eq!(input.config.a1_mini.enabled, options.a1_mini.enabled);
        assert_eq!(
            input.config.a1_mini.route_fast_mono,
            options.a1_mini.route_fast_mono
        );
        assert_eq!(
            input.config.a1_mini.build_volume,
            options.a1_mini.build_volume
        );
        assert_eq!(
            input.config.a1_mini.supported_materials,
            options.a1_mini.supported_materials
        );
        assert_eq!(
            input.config.a1_mini.current_spool_id,
            Some("custom".to_owned())
        );
        assert!(input.config.a1_mini.reserved_spool_ids.is_empty());
        assert_eq!(
            input.scopes[0].units[0].printer_preference,
            PrinterPreference::Auto
        );
        assert_eq!(
            serde_json::from_str::<PreliminaryPlanOptions>("{}").unwrap(),
            PreliminaryPlanOptions::default()
        );
    }

    fn confirmed_spool(id: &str, color: &str) -> Spool {
        Spool {
            id: id.to_owned(),
            calibration_id: None,
            display_name: format!("Confirmed {id}"),
            color_name: Some(color.to_owned()),
            material: Material::Pla,
            nominal_color: planner_color(SrgbColor::from_hex(color).unwrap()),
            measured_color: None,
            sku: Some(format!("sku-{id}")),
            profile_id: Some(format!("profile-{id}")),
            available: true,
        }
    }

    fn calibrated_geometry_context() -> CmyxGeometryContext {
        CmyxGeometryContext {
            orientation: SampleOrientation::Upright,
            geometry_class: GeometryClass::Volumetric,
        }
    }

    fn calibrated_t4_record(
        id: &str,
        t4_calibration_id: &str,
        measured_output_hex: &str,
        geometry_context: &CmyxGeometryContext,
    ) -> UserCmyxCalibrationRecord {
        let loadout =
            CmyxCalibrationLoadout::new(CYAN_ID, MAGENTA_ID, YELLOW_ID, t4_calibration_id);
        UserCmyxCalibrationRecord {
            id: id.to_owned(),
            context: full_spectrum_calibration_context(&loadout, geometry_context),
            loadout,
            recipe: MixRecipe {
                mode: RecipeMode::Ratio,
                components: vec![
                    RecipeComponent { slot: 1, weight: 3 },
                    RecipeComponent { slot: 4, weight: 1 },
                ],
            },
            measured_output_hex: measured_output_hex.to_owned(),
            provenance: CmyxMeasurementProvenance::verified(
                "2026-08-01T18:30:00Z",
                CmyxMeasurementMethod::InstrumentSrgb,
            ),
        }
    }

    fn mixed_recipe_uses_t4(recipe: &CmyxRecipe) -> bool {
        matches!(
            recipe,
            CmyxRecipe::FullSpectrum { sequence, .. } if sequence.contains(&Toolhead::T4)
        )
    }

    fn analysis_with_colors(colors: &[&str]) -> ProjectAnalysis {
        let mut analysis = minimal_analysis();
        let effective_colors = colors
            .iter()
            .enumerate()
            .map(|(index, color)| EffectiveMaterialColor {
                source_slots: vec![(index + 1) as u16],
                source_profile_ids: Vec::new(),
                material: Some("PLA".to_owned()),
                color: Some((*color).to_owned()),
                roles: vec![SourceMaterialRole::Model],
            })
            .collect::<Vec<_>>();
        let slots = (1..=colors.len() as u16).collect::<Vec<_>>();
        analysis.effective_material_colors = effective_colors.clone();
        analysis.plates[0].effective_material_colors = effective_colors.clone();
        analysis.plates[0].effective_slots = slots.clone();
        analysis.plates[0].classification = ColorClassification::MultiColor;
        analysis.objects[0].effective_material_colors = effective_colors;
        analysis.objects[0].effective_slots = slots;
        analysis.objects[0].classification = ColorClassification::MultiColor;
        analysis.summary.declared_filament_count = colors.len();
        analysis.summary.used_filament_count = colors.len();
        analysis.summary.mono_object_count = 0;
        analysis.summary.multi_color_object_count = 1;
        analysis
    }

    fn analysis_with_alternative_joint_plates() -> ProjectAnalysis {
        let mut analysis = minimal_analysis();
        analysis.plates[0].name = Some("Main Body".to_owned());

        let mut updated_joints = analysis.plates[0].clone();
        updated_joints.id = 7;
        updated_joints.name = Some("Updated Ball Joints".to_owned());
        updated_joints.instances[0].instance_id = 7;
        updated_joints.instances[0].source_build_item_index = Some(1);

        let mut alternate_joints = analysis.plates[0].clone();
        alternate_joints.id = 9;
        alternate_joints.name = Some("Alternate Joints".to_owned());
        alternate_joints.instances[0].instance_id = 9;
        alternate_joints.instances[0].source_build_item_index = Some(2);

        analysis.plates.extend([updated_joints, alternate_joints]);
        analysis.summary.plate_count = 3;
        analysis.summary.instance_count = 3;
        analysis.objects[0].instance_count = 3;
        analysis.objects[0].plate_ids = vec![1, 7, 9];
        analysis
    }

    fn minimal_analysis() -> ProjectAnalysis {
        let color = EffectiveMaterialColor {
            source_slots: vec![1],
            source_profile_ids: Vec::new(),
            material: Some("PLA".to_owned()),
            color: Some("#08ABFB".to_owned()),
            roles: vec![SourceMaterialRole::Model],
        };
        let bounds = AxisAlignedBounds {
            min: [5.0, 10.0, 15.0],
            max: [25.0, 40.0, 55.0],
        };
        ProjectAnalysis {
            input: InputIdentity {
                byte_size: 1,
                sha256: "00".repeat(32),
            },
            source: SourceInformation {
                application: SourceApplication::BambuStudio,
                application_name: Some("BambuStudio".to_owned()),
                application_version: None,
                title: None,
                dialect: ProjectDialect::BambuStudioProject,
                dialect_version: None,
                support: DialectSupport::Supported,
                has_bambu_or_orca_metadata: true,
                has_sliced_artifacts: false,
                sliced_artifact_entries: Vec::new(),
            },
            archive: ArchiveStatistics {
                entry_count: 1,
                archive_bytes: 1,
                total_compressed_bytes: 1,
                total_uncompressed_bytes: 1,
                largest_entry_uncompressed_bytes: 1,
                maximum_compression_ratio: 1.0,
            },
            printer: PrinterInformation::default(),
            process: ProcessInformation::default(),
            filaments: Vec::new(),
            effective_material_colors: vec![color.clone()],
            plates: vec![PlateAnalysis {
                id: 1,
                name: Some("Test plate".to_owned()),
                printable_bounds: Some(bounds),
                instances: vec![ObjectInstance {
                    object_id: 10,
                    instance_id: 1,
                    source_build_item_index: Some(0),
                    identify_id: None,
                    printable: true,
                    transform: None,
                    printable_bounds: Some(bounds),
                }],
                object_count: 1,
                part_count: 1,
                effective_slots: vec![1],
                effective_material_colors: vec![color.clone()],
                classification: ColorClassification::Mono,
            }],
            objects: vec![ObjectAnalysis {
                id: 10,
                source_model_path: None,
                source_object_id: None,
                name: Some("Test object".to_owned()),
                parts: Vec::new(),
                part_count: 1,
                printable_part_count: 1,
                instance_count: 1,
                plate_ids: vec![1],
                object_extruder_slot: Some(1),
                effective_slots: vec![1],
                effective_material_colors: vec![color],
                classification: ColorClassification::Mono,
                printable_bounds: Some(bounds),
            }],
            summary: AnalysisSummary {
                plate_count: 1,
                object_count: 1,
                instance_count: 1,
                part_count: 1,
                printable_part_count: 1,
                declared_filament_count: 1,
                used_filament_count: 1,
                mono_object_count: 1,
                multi_color_object_count: 0,
                unassigned_object_count: 0,
                bounded_printable_part_count: 1,
                bounded_object_count: 1,
                bounded_instance_count: 1,
                vertex_count: 0,
                triangle_count: 0,
            },
            warnings: Vec::new(),
        }
    }
}
