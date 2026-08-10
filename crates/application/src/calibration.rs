use std::collections::BTreeSet;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use u1_color_engine::{
    CalibrationContext, CalibrationSample, ColorBasis, GeometryClass, MixRecipe, PhysicalColor,
    RecipeError, SampleOrientation, SrgbColor, loadout_fingerprint, validate_calibration_sample,
};

pub const CMYX_CALIBRATION_LIBRARY_SCHEMA_VERSION: u32 = 2;
const MAX_CALIBRATION_RECORDS: usize = 2_048;
const MAX_ID_BYTES: usize = 128;
const MAX_INSTRUMENT_REFERENCE_BYTES: usize = 512;
const MAX_OPERATOR_NOTES_BYTES: usize = 2_048;

/// Exact spool or calibration-batch identities loaded in the four physical U1
/// toolheads when a sample was printed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CmyxCalibrationLoadout {
    pub t1_calibration_id: String,
    pub t2_calibration_id: String,
    pub t3_calibration_id: String,
    pub t4_calibration_id: String,
}

impl CmyxCalibrationLoadout {
    #[must_use]
    pub fn new(
        t1_calibration_id: impl Into<String>,
        t2_calibration_id: impl Into<String>,
        t3_calibration_id: impl Into<String>,
        t4_calibration_id: impl Into<String>,
    ) -> Self {
        Self {
            t1_calibration_id: t1_calibration_id.into(),
            t2_calibration_id: t2_calibration_id.into(),
            t3_calibration_id: t3_calibration_id.into(),
            t4_calibration_id: t4_calibration_id.into(),
        }
    }

    #[must_use]
    pub fn fingerprint(&self) -> String {
        loadout_fingerprint(&self.validation_loadout())
    }

    pub(crate) fn validation_loadout(&self) -> Vec<PhysicalColor> {
        self.ids()
            .into_iter()
            .enumerate()
            .map(|(index, calibration_id)| PhysicalColor {
                slot: (index + 1) as u8,
                calibration_id: calibration_id.to_owned(),
                name: calibration_id.to_owned(),
                // Color values do not participate in identity validation.
                srgb: SrgbColor::new(0, 0, 0),
                basis: ColorBasis::Nominal,
            })
            .collect()
    }

    fn normalized(mut self) -> Result<Self, CmyxCalibrationError> {
        for (index, id) in self.ids_mut().into_iter().enumerate() {
            *id = id.trim().to_owned();
            if id.is_empty() {
                return Err(CmyxCalibrationError::EmptyLoadoutIdentity {
                    toolhead: (index + 1) as u8,
                });
            }
            if id.len() > MAX_ID_BYTES {
                return Err(CmyxCalibrationError::LoadoutIdentityTooLong {
                    toolhead: (index + 1) as u8,
                    maximum: MAX_ID_BYTES,
                });
            }
        }
        let ids = self.ids().into_iter().collect::<BTreeSet<_>>();
        if ids.len() != 4 {
            return Err(CmyxCalibrationError::DuplicateLoadoutIdentity);
        }
        Ok(self)
    }

    fn ids(&self) -> [&str; 4] {
        [
            &self.t1_calibration_id,
            &self.t2_calibration_id,
            &self.t3_calibration_id,
            &self.t4_calibration_id,
        ]
    }

    fn ids_mut(&mut self) -> [&mut String; 4] {
        [
            &mut self.t1_calibration_id,
            &mut self.t2_calibration_id,
            &mut self.t3_calibration_id,
            &mut self.t4_calibration_id,
        ]
    }
}

/// Geometry identity selected for the print being planned. `Unknown` is the
/// safe default and deliberately prevents measured samples from being reused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CmyxGeometryContext {
    pub orientation: SampleOrientation,
    pub geometry_class: GeometryClass,
}

impl Default for CmyxGeometryContext {
    fn default() -> Self {
        Self {
            orientation: SampleOrientation::Unknown,
            geometry_class: GeometryClass::Unknown,
        }
    }
}

/// How the physical printed swatch was evaluated. The method is persisted with
/// the exact loadout/process/geometry identity so a nominal preview can never
/// acquire measured confidence merely by being copied into the library.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CmyxMeasurementMethod {
    /// Instrument Lab output converted to the stored sRGB value.
    InstrumentLab,
    /// Instrument-provided sRGB output.
    InstrumentSrgb,
    /// A reliable external measurement entered manually as sRGB.
    ReliableManualSrgb,
    /// A physical printed swatch confirmed by visual comparison.
    VisualSwatch,
    /// Schema-v1 data without sufficient measurement provenance.
    LegacyUnverified,
}

/// Audit metadata for one physical swatch measurement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CmyxMeasurementProvenance {
    /// RFC 3339 instant. It is absent only for safely migrated schema-v1 data.
    pub measured_at: Option<String>,
    pub method: CmyxMeasurementMethod,
    pub instrument_reference: Option<String>,
    pub operator_notes: Option<String>,
}

impl CmyxMeasurementProvenance {
    #[must_use]
    pub fn verified(measured_at: impl Into<String>, method: CmyxMeasurementMethod) -> Self {
        debug_assert_ne!(method, CmyxMeasurementMethod::LegacyUnverified);
        Self {
            measured_at: Some(measured_at.into()),
            method,
            instrument_reference: None,
            operator_notes: None,
        }
    }

    #[must_use]
    pub fn legacy_unverified() -> Self {
        Self {
            measured_at: None,
            method: CmyxMeasurementMethod::LegacyUnverified,
            instrument_reference: None,
            operator_notes: None,
        }
    }

    #[must_use]
    pub fn is_planning_qualified(&self) -> bool {
        self.method != CmyxMeasurementMethod::LegacyUnverified
    }

    fn normalized(mut self) -> Result<Self, CmyxCalibrationError> {
        self.instrument_reference = normalize_optional_provenance_text(
            self.instrument_reference,
            "instrument/reference",
            MAX_INSTRUMENT_REFERENCE_BYTES,
        )?;
        self.operator_notes = normalize_optional_provenance_text(
            self.operator_notes,
            "operator notes",
            MAX_OPERATOR_NOTES_BYTES,
        )?;

        if self.method == CmyxMeasurementMethod::LegacyUnverified {
            if self.measured_at.is_some() {
                return Err(CmyxCalibrationError::LegacyMeasurementHasTimestamp);
            }
            return Ok(self);
        }

        let measured_at = self
            .measured_at
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or(CmyxCalibrationError::MissingMeasurementTimestamp)?;
        let measured_at = DateTime::parse_from_rfc3339(measured_at).map_err(|_| {
            CmyxCalibrationError::InvalidMeasurementTimestamp {
                value: measured_at.to_owned(),
            }
        })?;
        self.measured_at = Some(
            measured_at
                .with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Millis, true),
        );
        Ok(self)
    }
}

fn normalize_optional_provenance_text(
    value: Option<String>,
    field: &'static str,
    maximum: usize,
) -> Result<Option<String>, CmyxCalibrationError> {
    let value = value.map(|value| value.trim().to_owned());
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.len() > maximum {
        return Err(CmyxCalibrationError::ProvenanceTextTooLong { field, maximum });
    }
    Ok(Some(value))
}

/// One user-confirmed color measurement. The measured output is persisted as
/// HEX for an explicit, portable interchange format; no nominal color is ever
/// promoted into this model automatically.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UserCmyxCalibrationRecord {
    pub id: String,
    pub loadout: CmyxCalibrationLoadout,
    pub context: CalibrationContext,
    pub recipe: MixRecipe,
    pub measured_output_hex: String,
    pub provenance: CmyxMeasurementProvenance,
}

impl UserCmyxCalibrationRecord {
    pub fn validate(&self) -> Result<(), CmyxCalibrationError> {
        self.clone().normalized().map(|_| ())
    }

    pub fn to_engine_sample(&self) -> Result<CalibrationSample, CmyxCalibrationError> {
        let normalized = self.clone().normalized()?;
        if !normalized.provenance.is_planning_qualified() {
            return Err(CmyxCalibrationError::UnverifiedLegacyMeasurement {
                record_id: normalized.id,
            });
        }
        normalized.engine_sample()
    }

    fn normalized(mut self) -> Result<Self, CmyxCalibrationError> {
        self.id = self.id.trim().to_owned();
        if self.id.is_empty() {
            return Err(CmyxCalibrationError::EmptyRecordId);
        }
        if self.id.len() > MAX_ID_BYTES {
            return Err(CmyxCalibrationError::RecordIdTooLong {
                maximum: MAX_ID_BYTES,
            });
        }
        self.loadout = self.loadout.normalized()?;
        let expected = self.loadout.fingerprint();
        if self.context.loadout_fingerprint != expected {
            return Err(CmyxCalibrationError::ContextLoadoutMismatch {
                expected,
                actual: self.context.loadout_fingerprint.clone(),
            });
        }
        let measured = SrgbColor::from_hex(&self.measured_output_hex).map_err(|_| {
            CmyxCalibrationError::InvalidMeasuredOutputHex {
                value: self.measured_output_hex.clone(),
            }
        })?;
        self.measured_output_hex = measured.to_hex();
        self.provenance = self.provenance.normalized()?;
        let sample = self.engine_sample()?;
        validate_calibration_sample(&sample, &self.loadout.validation_loadout()).map_err(
            |source| CmyxCalibrationError::InvalidSample {
                record_id: self.id.clone(),
                source,
            },
        )?;
        Ok(self)
    }

    fn engine_sample(&self) -> Result<CalibrationSample, CmyxCalibrationError> {
        let measured_srgb = SrgbColor::from_hex(&self.measured_output_hex).map_err(|_| {
            CmyxCalibrationError::InvalidMeasuredOutputHex {
                value: self.measured_output_hex.clone(),
            }
        })?;
        Ok(CalibrationSample {
            context: self.context.clone(),
            recipe: self.recipe.clone(),
            measured_srgb,
            sample_id: self.id.clone(),
        })
    }
}

/// Serializable CRUD container for persistent user measurements.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UserCmyxCalibrationLibrary {
    pub schema_version: u32,
    pub records: Vec<UserCmyxCalibrationRecord>,
}

impl Default for UserCmyxCalibrationLibrary {
    fn default() -> Self {
        Self {
            schema_version: CMYX_CALIBRATION_LIBRARY_SCHEMA_VERSION,
            records: Vec::new(),
        }
    }
}

impl UserCmyxCalibrationLibrary {
    pub fn from_records(
        records: Vec<UserCmyxCalibrationRecord>,
    ) -> Result<Self, CmyxCalibrationError> {
        Self {
            schema_version: CMYX_CALIBRATION_LIBRARY_SCHEMA_VERSION,
            records,
        }
        .normalized()
    }

    pub fn validate(&self) -> Result<(), CmyxCalibrationError> {
        self.clone().normalized().map(|_| ())
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&UserCmyxCalibrationRecord> {
        let id = id.trim();
        self.records.iter().find(|record| record.id == id)
    }

    pub fn upsert(
        &mut self,
        record: UserCmyxCalibrationRecord,
    ) -> Result<(), CmyxCalibrationError> {
        *self = self.clone().normalized()?;
        let record = record.normalized()?;
        if let Some(conflict) = self.records.iter().find(|existing| {
            existing.id != record.id
                && existing.provenance.is_planning_qualified()
                && record.provenance.is_planning_qualified()
                && existing.context == record.context
                && existing.recipe == record.recipe
        }) {
            return Err(CmyxCalibrationError::DuplicateQualifiedRecipe {
                first_id: conflict.id.clone(),
                second_id: record.id,
            });
        }
        if let Some(existing) = self
            .records
            .iter_mut()
            .find(|existing| existing.id == record.id)
        {
            *existing = record;
        } else {
            self.records.push(record);
        }
        self.records.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> Option<UserCmyxCalibrationRecord> {
        let id = id.trim();
        let index = self.records.iter().position(|record| record.id == id)?;
        Some(self.records.remove(index))
    }

    pub fn engine_samples(&self) -> Result<Vec<CalibrationSample>, CmyxCalibrationError> {
        let normalized = self.clone().normalized()?;
        normalized
            .records
            .iter()
            .filter(|record| record.provenance.is_planning_qualified())
            .map(UserCmyxCalibrationRecord::engine_sample)
            .collect()
    }

    fn normalized(mut self) -> Result<Self, CmyxCalibrationError> {
        if self.schema_version != CMYX_CALIBRATION_LIBRARY_SCHEMA_VERSION {
            return Err(CmyxCalibrationError::UnsupportedSchemaVersion {
                actual: self.schema_version,
                expected: CMYX_CALIBRATION_LIBRARY_SCHEMA_VERSION,
            });
        }
        if self.records.len() > MAX_CALIBRATION_RECORDS {
            return Err(CmyxCalibrationError::TooManyRecords {
                actual: self.records.len(),
                maximum: MAX_CALIBRATION_RECORDS,
            });
        }
        self.records = self
            .records
            .into_iter()
            .map(UserCmyxCalibrationRecord::normalized)
            .collect::<Result<Vec<_>, _>>()?;
        self.records.sort_by(|left, right| left.id.cmp(&right.id));
        for pair in self.records.windows(2) {
            if pair[0].id == pair[1].id {
                return Err(CmyxCalibrationError::DuplicateRecordId {
                    record_id: pair[0].id.clone(),
                });
            }
        }
        for (index, record) in self.records.iter().enumerate() {
            if let Some(conflict) = self.records[..index].iter().find(|existing| {
                existing.provenance.is_planning_qualified()
                    && record.provenance.is_planning_qualified()
                    && existing.context == record.context
                    && existing.recipe == record.recipe
            }) {
                return Err(CmyxCalibrationError::DuplicateQualifiedRecipe {
                    first_id: conflict.id.clone(),
                    second_id: record.id.clone(),
                });
            }
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CmyxCalibrationError {
    #[error("unsupported CMY+X calibration schema version {actual}; expected {expected}")]
    UnsupportedSchemaVersion { actual: u32, expected: u32 },
    #[error("CMY+X calibration library contains {actual} records; the maximum is {maximum}")]
    TooManyRecords { actual: usize, maximum: usize },
    #[error("CMY+X calibration record ID must not be empty")]
    EmptyRecordId,
    #[error("CMY+X calibration record ID exceeds the {maximum}-byte limit")]
    RecordIdTooLong { maximum: usize },
    #[error("T{toolhead} calibration identity must not be empty")]
    EmptyLoadoutIdentity { toolhead: u8 },
    #[error("T{toolhead} calibration identity exceeds the {maximum}-byte limit")]
    LoadoutIdentityTooLong { toolhead: u8, maximum: usize },
    #[error("one physical calibration identity cannot occupy multiple U1 toolheads")]
    DuplicateLoadoutIdentity,
    #[error("calibration context uses loadout {actual:?}; expected {expected:?}")]
    ContextLoadoutMismatch { expected: String, actual: String },
    #[error("measured output {value:?} is not a six-digit HEX color")]
    InvalidMeasuredOutputHex { value: String },
    #[error("measurement date and time is required for verified CMY+X calibration")]
    MissingMeasurementTimestamp,
    #[error("measurement date and time {value:?} is not a valid RFC 3339 instant")]
    InvalidMeasurementTimestamp { value: String },
    #[error("legacy unverified calibration must not claim a measurement timestamp")]
    LegacyMeasurementHasTimestamp,
    #[error("calibration {field} exceeds the {maximum}-byte limit")]
    ProvenanceTextTooLong { field: &'static str, maximum: usize },
    #[error(
        "calibration record {record_id:?} has legacy unverified provenance and cannot be used for measured prediction"
    )]
    UnverifiedLegacyMeasurement { record_id: String },
    #[error("calibration record {record_id:?} is invalid: {source}")]
    InvalidSample {
        record_id: String,
        source: RecipeError,
    },
    #[error("CMY+X calibration record ID {record_id:?} is duplicated")]
    DuplicateRecordId { record_id: String },
    #[error(
        "calibration records {first_id:?} and {second_id:?} qualify the same context and recipe"
    )]
    DuplicateQualifiedRecipe { first_id: String, second_id: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use u1_color_engine::{
        LayerSubdivisionPolicy, PrinterProfileIdentity, RecipeComponent, RecipeMode,
    };

    fn loadout() -> CmyxCalibrationLoadout {
        CmyxCalibrationLoadout::new("cyan-a", "magenta-a", "yellow-a", "black-a")
    }

    fn context(loadout: &CmyxCalibrationLoadout) -> CalibrationContext {
        CalibrationContext {
            loadout_fingerprint: loadout.fingerprint(),
            printer_profile: PrinterProfileIdentity {
                printer_model: "Snapmaker U1".to_owned(),
                printer_variant: "0.4 mm nozzle".to_owned(),
                profile_id: "full-spectrum".to_owned(),
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

    fn record(id: &str) -> UserCmyxCalibrationRecord {
        let loadout = loadout();
        UserCmyxCalibrationRecord {
            id: id.to_owned(),
            context: context(&loadout),
            loadout,
            recipe: MixRecipe {
                mode: RecipeMode::Ratio,
                components: vec![
                    RecipeComponent { slot: 1, weight: 3 },
                    RecipeComponent { slot: 4, weight: 1 },
                ],
            },
            measured_output_hex: "#1a2b3c".to_owned(),
            provenance: CmyxMeasurementProvenance::verified(
                "2026-08-01T12:34:56Z",
                CmyxMeasurementMethod::InstrumentSrgb,
            ),
        }
    }

    #[test]
    fn library_crud_normalizes_and_round_trips_records() {
        let mut library = UserCmyxCalibrationLibrary::default();
        library.upsert(record(" sample-b ")).unwrap();
        let mut updated = record("sample-b");
        updated.measured_output_hex = "#ABCDEF".to_owned();
        library.upsert(updated).unwrap();
        assert_eq!(library.records.len(), 1);
        assert_eq!(
            library.get("sample-b").unwrap().measured_output_hex,
            "#ABCDEF"
        );

        let encoded = serde_json::to_string(&library).unwrap();
        let decoded: UserCmyxCalibrationLibrary = serde_json::from_str(&encoded).unwrap();
        decoded.validate().unwrap();
        assert_eq!(decoded, library);
        assert_eq!(decoded.engine_samples().unwrap().len(), 1);
        assert_eq!(
            library.remove(" sample-b ").unwrap().id,
            "sample-b".to_owned()
        );
        assert!(library.records.is_empty());
    }

    #[test]
    fn record_validation_rejects_fabricated_or_ambiguous_identity() {
        let mut invalid = record("sample");
        invalid.measured_output_hex = "not-a-color".to_owned();
        assert!(matches!(
            invalid.validate(),
            Err(CmyxCalibrationError::InvalidMeasuredOutputHex { .. })
        ));

        let mut invalid = record("sample");
        invalid.context.geometry_class = GeometryClass::Unknown;
        assert!(matches!(
            invalid.validate(),
            Err(CmyxCalibrationError::InvalidSample {
                source: RecipeError::UnderspecifiedCalibrationSample,
                ..
            })
        ));

        let mut invalid = record("sample");
        invalid.loadout.t4_calibration_id = "cyan-a".to_owned();
        assert_eq!(
            invalid.validate(),
            Err(CmyxCalibrationError::DuplicateLoadoutIdentity)
        );

        let mut invalid = record("sample");
        invalid.context.loadout_fingerprint = "T4:another-spool".to_owned();
        assert!(matches!(
            invalid.validate(),
            Err(CmyxCalibrationError::ContextLoadoutMismatch { .. })
        ));
    }

    #[test]
    fn duplicate_qualified_recipe_is_rejected_instead_of_becoming_order_dependent() {
        let mut second = record("sample-b");
        second.measured_output_hex = "#FFFFFF".to_owned();
        let error =
            UserCmyxCalibrationLibrary::from_records(vec![record("sample-a"), second]).unwrap_err();
        assert!(matches!(
            error,
            CmyxCalibrationError::DuplicateQualifiedRecipe { .. }
        ));
    }

    #[test]
    fn provenance_requires_a_real_timestamp_and_normalizes_optional_text() {
        let mut missing = record("missing-date");
        missing.provenance.measured_at = None;
        assert_eq!(
            missing.validate(),
            Err(CmyxCalibrationError::MissingMeasurementTimestamp)
        );

        let mut invalid = record("invalid-date");
        invalid.provenance.measured_at = Some("yesterday".to_owned());
        assert!(matches!(
            invalid.validate(),
            Err(CmyxCalibrationError::InvalidMeasurementTimestamp { .. })
        ));

        let mut impossible = record("impossible-date");
        impossible.provenance.measured_at = Some("2026-02-30T12:00:00Z".to_owned());
        assert!(matches!(
            impossible.validate(),
            Err(CmyxCalibrationError::InvalidMeasurementTimestamp { .. })
        ));

        let mut normalized = record("normalized");
        normalized.provenance.measured_at = Some("2026-08-01T05:34:56-07:00".to_owned());
        normalized.provenance.instrument_reference = Some("  Colorimeter #7  ".to_owned());
        let mut library = UserCmyxCalibrationLibrary::default();
        library.upsert(normalized).unwrap();
        assert_eq!(
            library.records[0].provenance.measured_at.as_deref(),
            Some("2026-08-01T12:34:56.000Z")
        );
        assert_eq!(
            library.records[0]
                .provenance
                .instrument_reference
                .as_deref(),
            Some("Colorimeter #7")
        );
    }

    #[test]
    fn migrated_v1_measurement_stays_valid_but_never_gets_measured_confidence() {
        let mut legacy = record("legacy");
        legacy.provenance = CmyxMeasurementProvenance::legacy_unverified();
        legacy.validate().unwrap();
        assert!(matches!(
            legacy.to_engine_sample(),
            Err(CmyxCalibrationError::UnverifiedLegacyMeasurement { .. })
        ));
        let library = UserCmyxCalibrationLibrary::from_records(vec![legacy]).unwrap();
        assert!(library.engine_samples().unwrap().is_empty());
    }
}
