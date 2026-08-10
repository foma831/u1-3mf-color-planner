//! Color conversion and comparison primitives used by spool planning.
//!
//! The engine intentionally separates nominal catalog colors from measured
//! print colors. CIEDE2000 is a ranking aid, not a print-fidelity guarantee.

use serde::{Deserialize, Serialize};
use std::f64::consts::PI;
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ColorError {
    #[error("expected a six-digit sRGB color such as #08ABFB")]
    InvalidHex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrgbColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl SrgbColor {
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    pub fn from_hex(value: &str) -> Result<Self, ColorError> {
        let digits = value.strip_prefix('#').unwrap_or(value);
        if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ColorError::InvalidHex);
        }

        let red = u8::from_str_radix(&digits[0..2], 16).map_err(|_| ColorError::InvalidHex)?;
        let green = u8::from_str_radix(&digits[2..4], 16).map_err(|_| ColorError::InvalidHex)?;
        let blue = u8::from_str_radix(&digits[4..6], 16).map_err(|_| ColorError::InvalidHex)?;
        Ok(Self::new(red, green, blue))
    }

    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.red, self.green, self.blue)
    }

    pub fn to_lab(self) -> LabColor {
        let linearize = |channel: u8| {
            let value = f64::from(channel) / 255.0;
            if value <= 0.040_45 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };

        let red = linearize(self.red);
        let green = linearize(self.green);
        let blue = linearize(self.blue);

        // IEC 61966-2-1 sRGB to CIE XYZ, D65 reference white.
        let x = red * 0.412_456_4 + green * 0.357_576_1 + blue * 0.180_437_5;
        let y = red * 0.212_672_9 + green * 0.715_152_2 + blue * 0.072_175_0;
        let z = red * 0.019_333_9 + green * 0.119_192_0 + blue * 0.950_304_1;

        let transform = |ratio: f64| {
            const EPSILON: f64 = 216.0 / 24_389.0;
            const KAPPA: f64 = 24_389.0 / 27.0;
            if ratio > EPSILON {
                ratio.cbrt()
            } else {
                (KAPPA * ratio + 16.0) / 116.0
            }
        };

        let fx = transform(x / 0.950_47);
        let fy = transform(y);
        let fz = transform(z / 1.088_83);

        LabColor {
            lightness: 116.0 * fy - 16.0,
            a: 500.0 * (fx - fy),
            b: 200.0 * (fy - fz),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LabColor {
    pub lightness: f64,
    pub a: f64,
    pub b: f64,
}

impl LabColor {
    pub const fn new(lightness: f64, a: f64, b: f64) -> Self {
        Self { lightness, a, b }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchQuality {
    Good,
    Review,
    Poor,
}

pub fn classify_delta_e(delta_e: f64) -> MatchQuality {
    if delta_e <= 3.0 {
        MatchQuality::Good
    } else if delta_e <= 6.0 {
        MatchQuality::Review
    } else {
        MatchQuality::Poor
    }
}

/// Calculates CIEDE2000 with all weighting factors set to one.
pub fn delta_e_2000(first: LabColor, second: LabColor) -> f64 {
    let average_lightness = (first.lightness + second.lightness) / 2.0;
    let chroma_first = first.a.hypot(first.b);
    let chroma_second = second.a.hypot(second.b);
    let average_chroma = (chroma_first + chroma_second) / 2.0;
    let average_chroma_seventh = average_chroma.powi(7);
    let g =
        0.5 * (1.0 - (average_chroma_seventh / (average_chroma_seventh + 25_f64.powi(7))).sqrt());

    let a_first_prime = (1.0 + g) * first.a;
    let a_second_prime = (1.0 + g) * second.a;
    let chroma_first_prime = a_first_prime.hypot(first.b);
    let chroma_second_prime = a_second_prime.hypot(second.b);
    let average_chroma_prime = (chroma_first_prime + chroma_second_prime) / 2.0;

    let hue_angle = |b: f64, a: f64| {
        let degrees = b.atan2(a).to_degrees();
        if degrees < 0.0 {
            degrees + 360.0
        } else {
            degrees
        }
    };
    let hue_first_prime = if chroma_first_prime == 0.0 {
        0.0
    } else {
        hue_angle(first.b, a_first_prime)
    };
    let hue_second_prime = if chroma_second_prime == 0.0 {
        0.0
    } else {
        hue_angle(second.b, a_second_prime)
    };

    let delta_lightness_prime = second.lightness - first.lightness;
    let delta_chroma_prime = chroma_second_prime - chroma_first_prime;
    let hue_difference = if chroma_first_prime * chroma_second_prime == 0.0 {
        0.0
    } else if (hue_second_prime - hue_first_prime).abs() <= 180.0 {
        hue_second_prime - hue_first_prime
    } else if hue_second_prime <= hue_first_prime {
        hue_second_prime - hue_first_prime + 360.0
    } else {
        hue_second_prime - hue_first_prime - 360.0
    };
    let delta_hue_prime = 2.0
        * (chroma_first_prime * chroma_second_prime).sqrt()
        * ((hue_difference / 2.0) * PI / 180.0).sin();

    let average_hue_prime = if chroma_first_prime * chroma_second_prime == 0.0 {
        hue_first_prime + hue_second_prime
    } else if (hue_first_prime - hue_second_prime).abs() <= 180.0 {
        (hue_first_prime + hue_second_prime) / 2.0
    } else if hue_first_prime + hue_second_prime < 360.0 {
        (hue_first_prime + hue_second_prime + 360.0) / 2.0
    } else {
        (hue_first_prime + hue_second_prime - 360.0) / 2.0
    };

    let t = 1.0 - 0.17 * ((average_hue_prime - 30.0) * PI / 180.0).cos()
        + 0.24 * ((2.0 * average_hue_prime) * PI / 180.0).cos()
        + 0.32 * ((3.0 * average_hue_prime + 6.0) * PI / 180.0).cos()
        - 0.20 * ((4.0 * average_hue_prime - 63.0) * PI / 180.0).cos();
    let delta_theta = 30.0 * (-((average_hue_prime - 275.0) / 25.0).powi(2)).exp();
    let rotation = -2.0
        * (average_chroma_prime.powi(7) / (average_chroma_prime.powi(7) + 25_f64.powi(7))).sqrt()
        * (2.0 * delta_theta * PI / 180.0).sin();
    let lightness_scale = 1.0
        + 0.015 * (average_lightness - 50.0).powi(2)
            / (20.0 + (average_lightness - 50.0).powi(2)).sqrt();
    let chroma_scale = 1.0 + 0.045 * average_chroma_prime;
    let hue_scale = 1.0 + 0.015 * average_chroma_prime * t;

    let lightness_term = delta_lightness_prime / lightness_scale;
    let chroma_term = delta_chroma_prime / chroma_scale;
    let hue_term = delta_hue_prime / hue_scale;

    (lightness_term.powi(2)
        + chroma_term.powi(2)
        + hue_term.powi(2)
        + rotation * chroma_term * hue_term)
        .sqrt()
}

pub fn delta_e_for_srgb(first: SrgbColor, second: SrgbColor) -> f64 {
    delta_e_2000(first.to_lab(), second.to_lab())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorBasis {
    Measured,
    Nominal,
}

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("the built-in material catalog is invalid: {0}")]
    InvalidBuiltInCatalog(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterialCatalog {
    #[serde(rename = "$schema")]
    pub schema: String,
    pub schema_version: u32,
    pub catalog_id: String,
    pub source: CatalogSource,
    pub disclaimer: String,
    pub spools: Vec<CatalogSpool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSource {
    pub name: String,
    pub url: String,
    pub retrieved_on: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CatalogMaterial {
    #[serde(rename = "PLA")]
    Pla,
    #[serde(rename = "PETG")]
    Petg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultSpoolRole {
    FixedCyan,
    FixedMagenta,
    FixedYellow,
    DefaultT4,
    Optional,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MixingPolicy {
    FixedComponent,
    FullSpectrum,
    SolidOnlyUntilCalibrated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSpool {
    pub id: String,
    pub manufacturer: String,
    pub product: String,
    pub color_name: String,
    pub hex: String,
    pub material: CatalogMaterial,
    pub color_basis: ColorBasis,
    pub default_role: DefaultSpoolRole,
    pub mixing_policy: MixingPolicy,
    pub source_url: String,
}

pub fn built_in_material_catalog() -> Result<MaterialCatalog, CatalogError> {
    const JSON: &str = include_str!("../../../assets/material-catalog.v1.json");
    serde_json::from_str(JSON).map_err(CatalogError::from)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhysicalColor {
    /// One-based physical U1 slot number.
    pub slot: u8,
    /// Stable spool or calibration-batch identifier. Recipes calibrated for a
    /// different physical loadout must never be reused by slot number alone.
    pub calibration_id: String,
    pub name: String,
    pub srgb: SrgbColor,
    pub basis: ColorBasis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecipeMode {
    Solid,
    Cycle,
    Ratio,
    Match,
    Gradient,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RecipeComponent {
    /// One-based physical U1 slot number.
    pub slot: u8,
    /// Relative layer count used by the recipe.
    pub weight: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MixRecipe {
    pub mode: RecipeMode,
    /// Component order is significant and matches the serialized layer order.
    pub components: Vec<RecipeComponent>,
}

impl MixRecipe {
    pub fn solid(slot: u8) -> Self {
        Self {
            mode: RecipeMode::Solid,
            components: vec![RecipeComponent { slot, weight: 1 }],
        }
    }
}

/// Identity of the exact printer and slicer profile used to create a color
/// calibration. Human-readable profile names are not sufficient because a
/// profile can be edited without being renamed, so callers must also provide a
/// stable content or version fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrinterProfileIdentity {
    pub printer_model: String,
    pub printer_variant: String,
    pub profile_id: String,
    pub profile_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerSubdivisionPolicy {
    Disabled,
    SubdivideMixLayer,
    AdapterDefined(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleOrientation {
    Upright,
    Flat,
    Angled,
    Unknown,
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeometryClass {
    CalibrationSwatch,
    ThinWall,
    TopSurface,
    Volumetric,
    Unknown,
    Custom(String),
}

/// Complete process identity for one color measurement or prediction.
///
/// Dimensions use integer micrometres so equality is stable across JSON
/// round-trips. `process_fingerprint` must identify every remaining setting
/// that can materially affect the result, including nozzle temperatures, flow,
/// wall/infill policy and adapter-specific Full Spectrum settings. A measured
/// sample is reusable only when this value, and every other field, matches
/// exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationContext {
    pub loadout_fingerprint: String,
    pub printer_profile: PrinterProfileIdentity,
    pub nozzle_diameter_microns: u32,
    pub plate_layer_height_microns: u32,
    pub subdivision_policy: LayerSubdivisionPolicy,
    pub subdivision_factor: u8,
    pub effective_sublayer_height_microns: u32,
    pub process_fingerprint: String,
    pub orientation: SampleOrientation,
    pub geometry_class: GeometryClass,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalibrationSample {
    pub context: CalibrationContext,
    pub recipe: MixRecipe,
    pub measured_srgb: SrgbColor,
    pub sample_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PredictionConfidence {
    Measured,
    Nominal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecipeMatch {
    pub recipe: MixRecipe,
    pub predicted_srgb: SrgbColor,
    pub delta_e_00: f64,
    pub quality: MatchQuality,
    pub confidence: PredictionConfidence,
    pub calibration_sample_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecipeSearchSettings {
    /// Total relative layers distributed between Ratio/Match components.
    pub ratio_denominator: u8,
    pub include_cycle: bool,
    pub max_results: usize,
}

impl Default for RecipeSearchSettings {
    fn default() -> Self {
        Self {
            ratio_denominator: 8,
            include_cycle: true,
            max_results: 12,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RecipeError {
    #[error("physical U1 slots must be numbered 1 through 4")]
    InvalidSlot,
    #[error("a recipe must contain at least one component with a non-zero weight")]
    EmptyRecipe,
    #[error("recipe components must have a non-zero relative layer weight")]
    ZeroWeight,
    #[error("{mode:?} recipe requires {expected}; received {actual} components")]
    InvalidComponentCount {
        mode: RecipeMode,
        expected: &'static str,
        actual: usize,
    },
    #[error("recipe references physical slot T{0}, which is not in the loadout")]
    MissingSlot(u8),
    #[error("ratio denominator must be between 2 and 32")]
    InvalidRatioDenominator,
    #[error("calibration context loadout does not match the supplied physical colors")]
    ContextLoadoutMismatch,
    #[error("invalid calibration context: {0}")]
    InvalidCalibrationContext(&'static str),
    #[error("calibration sample ID must not be empty")]
    EmptyCalibrationSampleId,
    #[error("a measured calibration sample requires a known orientation and geometry class")]
    UnderspecifiedCalibrationSample,
}

/// Validates one measured sample against the exact physical loadout it
/// qualifies. This does not infer or repair any measurement metadata.
///
/// Callers persisting user measurements should run this validation before the
/// sample is accepted. Prediction deliberately remains tolerant of unrelated
/// samples so a record for another qualified context cannot break a search.
pub fn validate_calibration_sample(
    sample: &CalibrationSample,
    loadout: &[PhysicalColor],
) -> Result<(), RecipeError> {
    if sample.sample_id.trim().is_empty() {
        return Err(RecipeError::EmptyCalibrationSampleId);
    }
    if !context_is_measurement_specific(&sample.context) {
        return Err(RecipeError::UnderspecifiedCalibrationSample);
    }
    validate_recipe(&sample.recipe)?;
    for component in &sample.recipe.components {
        if !loadout.iter().any(|color| color.slot == component.slot) {
            return Err(RecipeError::MissingSlot(component.slot));
        }
    }
    let fingerprint = loadout_fingerprint(loadout);
    validate_calibration_context(&sample.context, &fingerprint)
}

/// Predicts a recipe using an exact calibration sample whenever possible.
///
/// The nominal fallback averages linear-light sRGB values. It is intentionally
/// marked `Nominal`: translucent, alternating-layer prints are not additive
/// displays and must be calibrated before the prediction is treated as print
/// color evidence.
pub fn predict_recipe(
    recipe: &MixRecipe,
    loadout: &[PhysicalColor],
    context: &CalibrationContext,
    calibration: &[CalibrationSample],
) -> Result<(SrgbColor, PredictionConfidence, Option<String>), RecipeError> {
    validate_recipe(recipe)?;
    for component in &recipe.components {
        if !loadout.iter().any(|color| color.slot == component.slot) {
            return Err(RecipeError::MissingSlot(component.slot));
        }
    }
    let fingerprint = loadout_fingerprint(loadout);
    validate_calibration_context(context, &fingerprint)?;
    let measured_sample = if context_is_measurement_specific(context) {
        calibration
            .iter()
            .find(|sample| sample.context == *context && sample.recipe == *recipe)
    } else {
        None
    };
    if let Some(sample) = measured_sample {
        return Ok((
            sample.measured_srgb,
            PredictionConfidence::Measured,
            Some(sample.sample_id.clone()),
        ));
    }

    let mut total_weight = 0_u32;
    let mut linear_red = 0.0;
    let mut linear_green = 0.0;
    let mut linear_blue = 0.0;
    for component in &recipe.components {
        let color = loadout
            .iter()
            .find(|color| color.slot == component.slot)
            .ok_or(RecipeError::MissingSlot(component.slot))?;
        let weight = f64::from(component.weight);
        total_weight += u32::from(component.weight);
        linear_red += srgb_channel_to_linear(color.srgb.red) * weight;
        linear_green += srgb_channel_to_linear(color.srgb.green) * weight;
        linear_blue += srgb_channel_to_linear(color.srgb.blue) * weight;
    }
    let denominator = f64::from(total_weight);
    Ok((
        SrgbColor::new(
            linear_channel_to_srgb(linear_red / denominator),
            linear_channel_to_srgb(linear_green / denominator),
            linear_channel_to_srgb(linear_blue / denominator),
        ),
        PredictionConfidence::Nominal,
        None,
    ))
}

/// Searches solids, two/three-component Ratio recipes and equal-weight Cycle
/// recipes. Results are deterministic and measured candidates win equal-ΔE
/// ties. Gradient candidates require geometry context and are generated by the
/// planner rather than this color-only search.
pub fn search_recipes(
    target: SrgbColor,
    loadout: &[PhysicalColor],
    context: &CalibrationContext,
    calibration: &[CalibrationSample],
    settings: RecipeSearchSettings,
) -> Result<Vec<RecipeMatch>, RecipeError> {
    if !(2..=32).contains(&settings.ratio_denominator) {
        return Err(RecipeError::InvalidRatioDenominator);
    }
    if loadout.iter().any(|color| !(1..=4).contains(&color.slot)) {
        return Err(RecipeError::InvalidSlot);
    }
    let fingerprint = loadout_fingerprint(loadout);
    validate_calibration_context(context, &fingerprint)?;

    let mut recipes = loadout
        .iter()
        .map(|color| MixRecipe::solid(color.slot))
        .collect::<Vec<_>>();
    let slots = loadout.iter().map(|color| color.slot).collect::<Vec<_>>();

    for component_count in 2..=slots.len().min(3) {
        for selected_slots in combinations(&slots, component_count) {
            for weights in positive_compositions(settings.ratio_denominator, component_count) {
                recipes.push(MixRecipe {
                    mode: RecipeMode::Ratio,
                    components: selected_slots
                        .iter()
                        .zip(weights)
                        .map(|(slot, weight)| RecipeComponent {
                            slot: *slot,
                            weight,
                        })
                        .collect(),
                });
            }
        }
    }

    if settings.include_cycle {
        for component_count in 2..=slots.len().min(4) {
            for selected_slots in combinations(&slots, component_count) {
                recipes.push(MixRecipe {
                    mode: RecipeMode::Cycle,
                    components: selected_slots
                        .into_iter()
                        .map(|slot| RecipeComponent { slot, weight: 1 })
                        .collect(),
                });
            }
        }
    }

    // Include measured recipes even when their weight grid is outside the
    // default search settings; calibration is stronger evidence than nominal
    // enumeration.
    if context_is_measurement_specific(context) {
        for sample in calibration
            .iter()
            .filter(|sample| sample.context == *context)
        {
            if !recipes.contains(&sample.recipe) {
                recipes.push(sample.recipe.clone());
            }
        }
    }

    let mut matches = Vec::with_capacity(recipes.len());
    for recipe in recipes {
        let (predicted_srgb, confidence, calibration_sample_id) =
            predict_recipe(&recipe, loadout, context, calibration)?;
        let delta_e_00 = delta_e_for_srgb(target, predicted_srgb);
        matches.push(RecipeMatch {
            recipe,
            predicted_srgb,
            delta_e_00,
            quality: classify_delta_e(delta_e_00),
            confidence,
            calibration_sample_id,
        });
    }
    matches.sort_by(|first, second| {
        first
            .delta_e_00
            .total_cmp(&second.delta_e_00)
            .then_with(|| match (first.confidence, second.confidence) {
                (PredictionConfidence::Measured, PredictionConfidence::Nominal) => {
                    std::cmp::Ordering::Less
                }
                (PredictionConfidence::Nominal, PredictionConfidence::Measured) => {
                    std::cmp::Ordering::Greater
                }
                _ => std::cmp::Ordering::Equal,
            })
            .then_with(|| recipe_sort_key(&first.recipe).cmp(&recipe_sort_key(&second.recipe)))
    });
    matches.truncate(settings.max_results);
    Ok(matches)
}

fn validate_calibration_context(
    context: &CalibrationContext,
    loadout_fingerprint: &str,
) -> Result<(), RecipeError> {
    if context.loadout_fingerprint != loadout_fingerprint {
        return Err(RecipeError::ContextLoadoutMismatch);
    }
    if context.printer_profile.printer_model.trim().is_empty()
        || context.printer_profile.printer_variant.trim().is_empty()
        || context.printer_profile.profile_id.trim().is_empty()
        || context
            .printer_profile
            .profile_fingerprint
            .trim()
            .is_empty()
    {
        return Err(RecipeError::InvalidCalibrationContext(
            "printer/profile identity must be complete",
        ));
    }
    if context.nozzle_diameter_microns == 0
        || context.plate_layer_height_microns == 0
        || context.subdivision_factor == 0
        || context.effective_sublayer_height_microns == 0
    {
        return Err(RecipeError::InvalidCalibrationContext(
            "nozzle and layer dimensions plus subdivision factor must be non-zero",
        ));
    }
    if context.process_fingerprint.trim().is_empty() {
        return Err(RecipeError::InvalidCalibrationContext(
            "process fingerprint must not be empty",
        ));
    }
    if u64::from(context.effective_sublayer_height_microns) * u64::from(context.subdivision_factor)
        != u64::from(context.plate_layer_height_microns)
    {
        return Err(RecipeError::InvalidCalibrationContext(
            "effective sublayer height multiplied by subdivision factor must equal plate layer height",
        ));
    }
    if context.subdivision_policy == LayerSubdivisionPolicy::Disabled
        && context.subdivision_factor != 1
    {
        return Err(RecipeError::InvalidCalibrationContext(
            "disabled subdivision requires factor one",
        ));
    }
    if matches!(
        &context.subdivision_policy,
        LayerSubdivisionPolicy::AdapterDefined(value) if value.trim().is_empty()
    ) || matches!(
        &context.orientation,
        SampleOrientation::Custom(value) if value.trim().is_empty()
    ) || matches!(
        &context.geometry_class,
        GeometryClass::Custom(value) if value.trim().is_empty()
    ) {
        return Err(RecipeError::InvalidCalibrationContext(
            "custom context identities must not be empty",
        ));
    }
    Ok(())
}

fn context_is_measurement_specific(context: &CalibrationContext) -> bool {
    !matches!(context.orientation, SampleOrientation::Unknown)
        && !matches!(context.geometry_class, GeometryClass::Unknown)
}

/// Produces the stable physical-loadout key used to scope calibration samples.
/// Slot order is normalized, so caller collection order does not change the
/// identity while moving a spool to another toolhead does.
pub fn loadout_fingerprint(loadout: &[PhysicalColor]) -> String {
    let mut slots = loadout
        .iter()
        .map(|color| (color.slot, color.calibration_id.as_str()))
        .collect::<Vec<_>>();
    slots.sort_unstable_by_key(|(slot, _)| *slot);
    slots
        .into_iter()
        .map(|(slot, id)| format!("T{slot}:{id}"))
        .collect::<Vec<_>>()
        .join("|")
}

fn validate_recipe(recipe: &MixRecipe) -> Result<(), RecipeError> {
    if recipe.components.is_empty() {
        return Err(RecipeError::EmptyRecipe);
    }
    if recipe
        .components
        .iter()
        .any(|component| component.weight == 0)
    {
        return Err(RecipeError::ZeroWeight);
    }
    if recipe
        .components
        .iter()
        .any(|component| !(1..=4).contains(&component.slot))
    {
        return Err(RecipeError::InvalidSlot);
    }
    let actual = recipe.components.len();
    let expected = match recipe.mode {
        RecipeMode::Solid if actual == 1 => None,
        RecipeMode::Gradient if actual == 2 => None,
        RecipeMode::Ratio | RecipeMode::Match if (2..=3).contains(&actual) => None,
        RecipeMode::Cycle if (2..=4).contains(&actual) => None,
        RecipeMode::Solid => Some("exactly one component"),
        RecipeMode::Gradient => Some("exactly two components"),
        RecipeMode::Ratio | RecipeMode::Match => Some("two or three components"),
        RecipeMode::Cycle => Some("two through four components"),
    };
    if let Some(expected) = expected {
        return Err(RecipeError::InvalidComponentCount {
            mode: recipe.mode,
            expected,
            actual,
        });
    }
    Ok(())
}

fn srgb_channel_to_linear(channel: u8) -> f64 {
    let value = f64::from(channel) / 255.0;
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_channel_to_srgb(channel: f64) -> u8 {
    let value = if channel <= 0.003_130_8 {
        12.92 * channel
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    };
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn combinations<T: Copy>(values: &[T], count: usize) -> Vec<Vec<T>> {
    fn visit<T: Copy>(
        values: &[T],
        count: usize,
        start: usize,
        current: &mut Vec<T>,
        output: &mut Vec<Vec<T>>,
    ) {
        if current.len() == count {
            output.push(current.clone());
            return;
        }
        for index in start..values.len() {
            current.push(values[index]);
            visit(values, count, index + 1, current, output);
            current.pop();
        }
    }

    let mut output = Vec::new();
    visit(values, count, 0, &mut Vec::new(), &mut output);
    output
}

fn positive_compositions(total: u8, count: usize) -> Vec<Vec<u8>> {
    fn visit(total: u8, count: usize, current: &mut Vec<u8>, output: &mut Vec<Vec<u8>>) {
        if count == 1 {
            if total > 0 {
                current.push(total);
                output.push(current.clone());
                current.pop();
            }
            return;
        }
        for value in 1..=total.saturating_sub((count - 1) as u8) {
            current.push(value);
            visit(total - value, count - 1, current, output);
            current.pop();
        }
    }

    let mut output = Vec::new();
    visit(total, count, &mut Vec::new(), &mut output);
    output
}

fn recipe_sort_key(recipe: &MixRecipe) -> (u8, Vec<(u8, u8)>) {
    let mode = match recipe.mode {
        RecipeMode::Solid => 0,
        RecipeMode::Ratio => 1,
        RecipeMode::Cycle => 2,
        RecipeMode::Match => 3,
        RecipeMode::Gradient => 4,
    };
    (
        mode,
        recipe
            .components
            .iter()
            .map(|component| (component.slot, component.weight))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calibration_context(loadout: &[PhysicalColor]) -> CalibrationContext {
        CalibrationContext {
            loadout_fingerprint: loadout_fingerprint(loadout),
            printer_profile: PrinterProfileIdentity {
                printer_model: "Snapmaker U1".to_owned(),
                printer_variant: "0.4 mm nozzle".to_owned(),
                profile_id: "Snapmaker PLA Full Spectrum @U1 0.4 nozzle".to_owned(),
                profile_fingerprint: "profile-sha256-a".to_owned(),
            },
            nozzle_diameter_microns: 400,
            plate_layer_height_microns: 80,
            subdivision_policy: LayerSubdivisionPolicy::SubdivideMixLayer,
            subdivision_factor: 4,
            effective_sublayer_height_microns: 20,
            process_fingerprint: "process-sha256-a".to_owned(),
            orientation: SampleOrientation::Upright,
            geometry_class: GeometryClass::CalibrationSwatch,
        }
    }

    #[test]
    fn parses_and_formats_catalog_hex() {
        let cyan = SrgbColor::from_hex("#08abfb").expect("valid color");
        assert_eq!(cyan, SrgbColor::new(8, 171, 251));
        assert_eq!(cyan.to_hex(), "#08ABFB");
        assert_eq!(
            SrgbColor::from_hex("#123").unwrap_err(),
            ColorError::InvalidHex
        );
    }

    #[test]
    fn converts_reference_srgb_values_to_lab() {
        let white = SrgbColor::new(255, 255, 255).to_lab();
        assert!((white.lightness - 100.0).abs() < 0.001);
        assert!(white.a.abs() < 0.001);
        assert!(white.b.abs() < 0.001);

        let black = SrgbColor::new(0, 0, 0).to_lab();
        assert!(black.lightness.abs() < 0.001);
    }

    #[test]
    fn matches_sharma_ciede2000_reference_pairs() {
        let pairs = [
            (
                LabColor::new(50.0, 2.6772, -79.7751),
                LabColor::new(50.0, 0.0, -82.7485),
                2.0425,
            ),
            (
                LabColor::new(50.0, 3.1571, -77.2803),
                LabColor::new(50.0, 0.0, -82.7485),
                2.8615,
            ),
            (
                LabColor::new(50.0, 2.8361, -74.0200),
                LabColor::new(50.0, 0.0, -82.7485),
                3.4412,
            ),
            (
                LabColor::new(50.0, -1.3802, -84.2814),
                LabColor::new(50.0, 0.0, -82.7485),
                1.0000,
            ),
        ];

        for (first, second, expected) in pairs {
            let actual = delta_e_2000(first, second);
            assert!(
                (actual - expected).abs() < 0.0001,
                "expected {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn classifies_configured_quality_bands() {
        assert_eq!(classify_delta_e(3.0), MatchQuality::Good);
        assert_eq!(classify_delta_e(3.01), MatchQuality::Review);
        assert_eq!(classify_delta_e(6.0), MatchQuality::Review);
        assert_eq!(classify_delta_e(6.01), MatchQuality::Poor);
    }

    #[test]
    fn exact_calibration_overrides_nominal_prediction() {
        let loadout = vec![
            PhysicalColor {
                slot: 1,
                calibration_id: "cyan-batch-a".to_owned(),
                name: "Cyan".to_owned(),
                srgb: SrgbColor::new(0, 180, 220),
                basis: ColorBasis::Measured,
            },
            PhysicalColor {
                slot: 2,
                calibration_id: "magenta-batch-a".to_owned(),
                name: "Magenta".to_owned(),
                srgb: SrgbColor::new(210, 0, 120),
                basis: ColorBasis::Measured,
            },
        ];
        let recipe = MixRecipe {
            mode: RecipeMode::Ratio,
            components: vec![
                RecipeComponent { slot: 1, weight: 1 },
                RecipeComponent { slot: 2, weight: 1 },
            ],
        };
        let context = calibration_context(&loadout);
        let calibration = vec![CalibrationSample {
            context: context.clone(),
            recipe: recipe.clone(),
            measured_srgb: SrgbColor::new(78, 61, 160),
            sample_id: "chart-2026-07-r1".to_owned(),
        }];

        let prediction = predict_recipe(&recipe, &loadout, &context, &calibration).unwrap();
        assert_eq!(prediction.0, SrgbColor::new(78, 61, 160));
        assert_eq!(prediction.1, PredictionConfidence::Measured);
        assert_eq!(prediction.2.as_deref(), Some("chart-2026-07-r1"));
    }

    #[test]
    fn persisted_calibration_sample_requires_exact_specific_identity() {
        let loadout = vec![PhysicalColor {
            slot: 1,
            calibration_id: "cyan-batch-a".to_owned(),
            name: "Cyan".to_owned(),
            srgb: SrgbColor::new(0, 180, 220),
            basis: ColorBasis::Measured,
        }];
        let mut sample = CalibrationSample {
            context: calibration_context(&loadout),
            recipe: MixRecipe::solid(1),
            measured_srgb: SrgbColor::new(4, 170, 211),
            sample_id: "chart-2026-08-r1".to_owned(),
        };

        validate_calibration_sample(&sample, &loadout).unwrap();

        sample.sample_id = "  ".to_owned();
        assert_eq!(
            validate_calibration_sample(&sample, &loadout),
            Err(RecipeError::EmptyCalibrationSampleId)
        );
        sample.sample_id = "chart-2026-08-r1".to_owned();
        sample.context.geometry_class = GeometryClass::Unknown;
        assert_eq!(
            validate_calibration_sample(&sample, &loadout),
            Err(RecipeError::UnderspecifiedCalibrationSample)
        );
        sample.context = calibration_context(&loadout);
        let other_loadout = vec![PhysicalColor {
            calibration_id: "cyan-batch-b".to_owned(),
            ..loadout[0].clone()
        }];
        assert_eq!(
            validate_calibration_sample(&sample, &other_loadout),
            Err(RecipeError::ContextLoadoutMismatch)
        );
    }

    #[test]
    fn calibration_requires_complete_context_equality() {
        let loadout = vec![PhysicalColor {
            slot: 1,
            calibration_id: "cyan-batch-a".to_owned(),
            name: "Cyan".to_owned(),
            srgb: SrgbColor::new(0, 180, 220),
            basis: ColorBasis::Measured,
        }];
        let recipe = MixRecipe::solid(1);
        let context = calibration_context(&loadout);
        let calibration = vec![CalibrationSample {
            context: context.clone(),
            recipe: recipe.clone(),
            measured_srgb: SrgbColor::new(4, 170, 211),
            sample_id: "exact-context".to_owned(),
        }];

        let exact = predict_recipe(&recipe, &loadout, &context, &calibration).unwrap();
        assert_eq!(exact.1, PredictionConfidence::Measured);

        let mut mismatches = Vec::new();
        let mut changed = context.clone();
        changed.printer_profile.profile_fingerprint = "profile-sha256-b".to_owned();
        mismatches.push(changed);
        let mut changed = context.clone();
        changed.nozzle_diameter_microns = 600;
        mismatches.push(changed);
        let mut changed = context.clone();
        changed.plate_layer_height_microns = 120;
        changed.subdivision_factor = 6;
        mismatches.push(changed);
        let mut changed = context.clone();
        changed.process_fingerprint = "process-sha256-b".to_owned();
        mismatches.push(changed);
        let mut changed = context.clone();
        changed.orientation = SampleOrientation::Flat;
        mismatches.push(changed);
        let mut changed = context.clone();
        changed.geometry_class = GeometryClass::ThinWall;
        mismatches.push(changed);

        for mismatch in mismatches {
            let prediction = predict_recipe(&recipe, &loadout, &mismatch, &calibration).unwrap();
            assert_eq!(prediction.1, PredictionConfidence::Nominal);
            assert_eq!(prediction.2, None);
        }
    }

    #[test]
    fn calibration_context_round_trips_without_losing_identity() {
        let loadout = vec![PhysicalColor {
            slot: 1,
            calibration_id: "cyan-batch-a".to_owned(),
            name: "Cyan".to_owned(),
            srgb: SrgbColor::new(0, 180, 220),
            basis: ColorBasis::Measured,
        }];
        let context = calibration_context(&loadout);
        let json = serde_json::to_string(&context).unwrap();
        let decoded: CalibrationContext = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, context);
    }

    #[test]
    fn unknown_geometry_never_promotes_preview_to_measured_confidence() {
        let loadout = vec![PhysicalColor {
            slot: 1,
            calibration_id: "cyan-batch-a".to_owned(),
            name: "Cyan".to_owned(),
            srgb: SrgbColor::new(0, 180, 220),
            basis: ColorBasis::Measured,
        }];
        let recipe = MixRecipe::solid(1);
        let mut context = calibration_context(&loadout);
        context.orientation = SampleOrientation::Unknown;
        context.geometry_class = GeometryClass::Unknown;
        let calibration = vec![CalibrationSample {
            context: context.clone(),
            recipe: recipe.clone(),
            measured_srgb: SrgbColor::new(4, 170, 211),
            sample_id: "underspecified-sample".to_owned(),
        }];

        let prediction = predict_recipe(&recipe, &loadout, &context, &calibration).unwrap();
        assert_eq!(prediction.1, PredictionConfidence::Nominal);
        assert_eq!(prediction.2, None);
    }

    #[test]
    fn calibration_from_another_loadout_is_ignored() {
        let loadout = vec![PhysicalColor {
            slot: 1,
            calibration_id: "cyan-batch-a".to_owned(),
            name: "Cyan".to_owned(),
            srgb: SrgbColor::new(0, 180, 220),
            basis: ColorBasis::Measured,
        }];
        let recipe = MixRecipe::solid(1);
        let context = calibration_context(&loadout);
        let mut other_context = context.clone();
        other_context.loadout_fingerprint = "T1:cyan-batch-b".to_owned();
        let calibration = vec![CalibrationSample {
            context: other_context,
            recipe: recipe.clone(),
            measured_srgb: SrgbColor::new(4, 170, 211),
            sample_id: "different-loadout".to_owned(),
        }];

        let prediction = predict_recipe(&recipe, &loadout, &context, &calibration).unwrap();
        assert_eq!(prediction.1, PredictionConfidence::Nominal);
        assert_eq!(prediction.2, None);
    }

    #[test]
    fn search_is_deterministic_and_returns_best_color_first() {
        let loadout = vec![
            PhysicalColor {
                slot: 1,
                calibration_id: "red".to_owned(),
                name: "Red".to_owned(),
                srgb: SrgbColor::new(255, 0, 0),
                basis: ColorBasis::Nominal,
            },
            PhysicalColor {
                slot: 2,
                calibration_id: "blue".to_owned(),
                name: "Blue".to_owned(),
                srgb: SrgbColor::new(0, 0, 255),
                basis: ColorBasis::Nominal,
            },
        ];
        let settings = RecipeSearchSettings {
            ratio_denominator: 4,
            include_cycle: true,
            max_results: 6,
        };

        let context = calibration_context(&loadout);
        let first =
            search_recipes(SrgbColor::new(255, 0, 0), &loadout, &context, &[], settings).unwrap();
        let second =
            search_recipes(SrgbColor::new(255, 0, 0), &loadout, &context, &[], settings).unwrap();
        assert_eq!(first, second);
        assert_eq!(first[0].recipe, MixRecipe::solid(1));
        assert!(first[0].delta_e_00 < 0.0001);
    }

    #[test]
    fn enforces_documented_mode_component_limits() {
        let invalid_ratio = MixRecipe {
            mode: RecipeMode::Ratio,
            components: vec![RecipeComponent { slot: 1, weight: 1 }],
        };
        assert!(matches!(
            validate_recipe(&invalid_ratio),
            Err(RecipeError::InvalidComponentCount {
                mode: RecipeMode::Ratio,
                actual: 1,
                ..
            })
        ));

        let invalid_cycle = MixRecipe {
            mode: RecipeMode::Cycle,
            components: vec![
                RecipeComponent { slot: 1, weight: 1 },
                RecipeComponent { slot: 2, weight: 0 },
            ],
        };
        assert_eq!(
            validate_recipe(&invalid_cycle),
            Err(RecipeError::ZeroWeight)
        );
    }

    #[test]
    fn built_in_catalog_has_fixed_cmy_and_optional_t4_spools() {
        let catalog = built_in_material_catalog().unwrap();
        assert_eq!(catalog.schema_version, 1);
        assert_eq!(catalog.spools.len(), 6);
        assert!(catalog.spools.iter().any(|spool| {
            spool.default_role == DefaultSpoolRole::FixedCyan && spool.hex == "#08ABFB"
        }));
        assert!(catalog.spools.iter().any(|spool| {
            spool.default_role == DefaultSpoolRole::Optional
                && spool.color_name == "Black"
                && spool.mixing_policy == MixingPolicy::SolidOnlyUntilCalibrated
        }));
    }
}
