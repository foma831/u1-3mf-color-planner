//! Canonical, fail-closed views of one validated mixed-printer plan.
//!
//! Target adapters receive only the scopes, jobs, plates, batches, and source
//! units assigned to them. The authoritative full-plan fingerprint remains a
//! separate orchestration concern and is never reconstructed from UI data.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use u1_planner::{
    ColorStrategy, ErrorCode, PlanError, PlanningInput, PlanningResult, Printer, ScopeStrategy,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversionTarget {
    U1Direct,
    U1FullSpectrum,
    A1MiniMono,
}

impl ConversionTarget {
    #[must_use]
    pub fn matches(self, printer: Printer, strategy: ColorStrategy) -> bool {
        matches!(
            (self, printer, strategy),
            (
                Self::U1Direct,
                Printer::U1,
                ColorStrategy::DirectSpools | ColorStrategy::CmyxSolid
            ) | (
                Self::U1FullSpectrum,
                Printer::U1,
                ColorStrategy::CmyxFullSpectrum
            ) | (Self::A1MiniMono, Printer::A1Mini, ColorStrategy::A1Mono)
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConversionPlanSlice {
    pub target: ConversionTarget,
    pub input: PlanningInput,
    pub result: PlanningResult,
    pub source_unit_ids: Vec<String>,
}

/// Backend-derived evidence for one source unit that is intentionally absent
/// from an approved partial conversion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExcludedSourceUnit {
    /// Canonical planning scope that owned the omitted unit.
    pub scope_id: String,
    /// Unit ID inside `scope_id`; this is distinct from the immutable source
    /// unit identity used as the approval key.
    pub planning_unit_id: String,
    /// Numeric source plate identity when the canonical `plate-N` identity is
    /// available. Object-only source packages may legitimately have no plate.
    pub source_plate_id: Option<u32>,
    pub source_unit_id: String,
    pub reason: String,
    pub error_identity: String,
}

/// Exact, one-shot user approval for every source unit omitted by a partial
/// conversion. Callers must submit the canonical entries returned by
/// [`canonical_partial_conversion_exclusions`] without reconstructing them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PartialConversionApproval {
    pub excluded_source_units: Vec<ExcludedSourceUnit>,
}

/// A complete native-target partition plus durable exclusion evidence for the
/// outer conversion manifest.
#[derive(Clone, Debug, PartialEq)]
pub struct ConversionPlanPartition {
    pub slices: Vec<ConversionPlanSlice>,
    pub excluded_source_units: Vec<ExcludedSourceUnit>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConversionPlanSliceError {
    #[error("the canonical plan still contains blocking errors")]
    BlockingPlan,
    #[error("the selected target has no scheduled batches")]
    EmptyTarget,
    #[error("batch {batch_id:?} references unknown job {job_id:?}")]
    UnknownJob { batch_id: String, job_id: String },
    #[error("batch {batch_id:?} references unknown plate {plate_id:?}")]
    UnknownPlate { batch_id: String, plate_id: String },
    #[error("selected job {job_id:?} does not match the requested target")]
    TargetMismatch { job_id: String },
    #[error("selected plate {plate_id:?} references unknown unit {scope_id:?}/{unit_id:?}")]
    UnknownUnit {
        plate_id: String,
        scope_id: String,
        unit_id: String,
    },
    #[error("selected unit {scope_id:?}/{unit_id:?} is not placed exactly once")]
    UnitPlacementMismatch { scope_id: String, unit_id: String },
    #[error("selected source unit identity {source_unit_id:?} is duplicated")]
    DuplicateSourceUnit { source_unit_id: String },
    #[error("canonical {entity} {id:?} is covered {count} times; expected exactly once")]
    EntityCoverageMismatch {
        entity: &'static str,
        id: String,
        count: usize,
    },
    #[error("canonical batch {batch_id:?} does not map to a supported native conversion target")]
    UnsupportedTarget { batch_id: String },
    #[error("scheduled unit {scope_id:?}/{unit_id:?} is not present in the canonical input")]
    UnexpectedScheduledUnit { scope_id: String, unit_id: String },
    #[error("canonical source unit identity {source_unit_id:?} is not unique")]
    DuplicateCanonicalSourceUnit { source_unit_id: String },
    #[error(
        "canonical source unit {source_unit_id:?} has invalid source plate identity {source_plate_id:?}"
    )]
    InvalidCanonicalSourcePlateId {
        source_unit_id: String,
        source_plate_id: String,
    },
    #[error("plate {plate_id:?} is not owned by its declared job {job_id:?} in one batch")]
    PlateJobMismatch { plate_id: String, job_id: String },
    #[error("global hard error {code:?} blocks partial conversion")]
    GlobalPlanError { code: ErrorCode },
    #[error("hard error {code:?} cannot be resolved by excluding source units")]
    NonExcludablePlanError { code: ErrorCode },
    #[error("unit-scoped hard error {code:?} has no planning-unit identity")]
    UnitPlanErrorMissingUnit { code: ErrorCode },
    #[error("scope-scoped hard error {code:?} unexpectedly identifies planning unit {unit_id:?}")]
    ScopePlanErrorHasUnit { code: ErrorCode, unit_id: String },
    #[error(
        "scoped hard error {code:?} covers scheduled source unit {source_unit_id:?}; partial conversion is unsafe"
    )]
    ScopedPlanErrorCoversScheduledUnit {
        code: ErrorCode,
        source_unit_id: String,
    },
    #[error("hard error {error_identity} does not identify an omitted canonical source unit")]
    UnboundScopedPlanError { error_identity: String },
    #[error("omitted source unit {source_unit_id:?} has no applicable scope/unit hard error")]
    UnexplainedOmittedSourceUnit { source_unit_id: String },
    #[error("partial conversion requires explicit approval for {source_unit_ids:?}")]
    PartialConversionApprovalRequired { source_unit_ids: Vec<String> },
    #[error("partial-conversion approval duplicates source unit {source_unit_id:?}")]
    DuplicateApprovedExclusion { source_unit_id: String },
    #[error("partial-conversion approval references unknown source unit {source_unit_id:?}")]
    UnknownApprovedExclusion { source_unit_id: String },
    #[error("scheduled source unit {source_unit_id:?} cannot be excluded")]
    ScheduledUnitExclusion { source_unit_id: String },
    #[error("partial-conversion approval for source unit {source_unit_id:?} is stale or modified")]
    StaleApprovedExclusion { source_unit_id: String },
    #[error("partial-conversion approval is missing source unit {source_unit_id:?}")]
    MissingApprovedExclusion { source_unit_id: String },
    #[error("scoped hard error {code:?} has no backend-authored reason")]
    EmptyPlanErrorReason { code: ErrorCode },
    #[error("failed to serialize canonical partial-conversion evidence")]
    ExclusionEvidenceSerialization,
}

#[derive(Clone, Debug)]
struct CanonicalSourceUnit {
    scope_id: String,
    planning_unit_id: String,
    source_plate_identity: Option<String>,
    source_plate_id: Option<u32>,
    source_unit_id: String,
}

impl CanonicalSourceUnit {
    fn scoped_ref(&self) -> u1_planner::ScopedUnitRef {
        u1_planner::ScopedUnitRef {
            scope_id: self.scope_id.clone(),
            unit_id: self.planning_unit_id.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
struct CanonicalExclusionError {
    code: ErrorCode,
    scope_id: String,
    planning_unit_id: Option<String>,
    message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExclusionIdentityDocument<'a> {
    schema: &'static str,
    scope_id: &'a str,
    planning_unit_id: &'a str,
    source_plate_identity: Option<&'a str>,
    source_plate_id: Option<u32>,
    source_unit_id: &'a str,
    errors: &'a [CanonicalExclusionError],
}

struct PartialConversionCandidate {
    partition: ConversionPlanPartition,
    scheduled_source_unit_ids: BTreeSet<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExcludableErrorShape {
    ScopeOnly,
    UnitOnly,
    ScopeOrUnit,
}

/// Slices every supported native target and verifies that the combined output
/// is an exact partition of the canonical plan.
///
/// This is the strict full-conversion orchestration entry point. Calling one
/// target slicer directly is appropriate for an adapter test, but a mixed
/// conversion must use this function (or the explicit partial-approval entry
/// point) so an orphaned or doubly-routed unit cannot disappear silently.
pub fn slice_all_conversion_targets(
    input: &PlanningInput,
    result: &PlanningResult,
) -> Result<Vec<ConversionPlanSlice>, ConversionPlanSliceError> {
    if result.has_hard_errors() {
        return Err(ConversionPlanSliceError::BlockingPlan);
    }
    if result.batches.is_empty() {
        return Err(ConversionPlanSliceError::EmptyTarget);
    }
    validate_batch_reference_closure(result)?;

    let mut slices = Vec::new();
    for target in [
        ConversionTarget::U1Direct,
        ConversionTarget::U1FullSpectrum,
        ConversionTarget::A1MiniMono,
    ] {
        match slice_conversion_plan(input, result, target) {
            Ok(slice) => slices.push(slice),
            Err(ConversionPlanSliceError::EmptyTarget) => {}
            Err(error) => return Err(error),
        }
    }

    validate_exact_partition(input, result, &slices)?;
    Ok(slices)
}

/// Returns the backend-authored exclusion records that must be shown and
/// approved before converting the valid subset of a plan with hard errors.
///
/// This function validates the entire surviving native-target partition. It
/// never turns a global, structural, unbound, or scheduled-unit error into an
/// exclusion candidate.
pub fn canonical_partial_conversion_exclusions(
    input: &PlanningInput,
    result: &PlanningResult,
) -> Result<Vec<ExcludedSourceUnit>, ConversionPlanSliceError> {
    if !result.has_hard_errors() {
        slice_all_conversion_targets(input, result)?;
        return Ok(Vec::new());
    }

    Ok(build_partial_conversion_candidate(input, result)?
        .partition
        .excluded_source_units)
}

/// Slices a complete plan normally, or a plan's valid subset after exact,
/// explicit approval of every backend-derived exclusion.
///
/// Approval is compared as a set, while duplicates are rejected. Every field
/// of every exclusion record is authoritative, so a stale reason, identity,
/// scope, unit, or source-plate value fails closed.
pub fn slice_all_conversion_targets_with_partial_approval(
    input: &PlanningInput,
    result: &PlanningResult,
    approval: Option<&PartialConversionApproval>,
) -> Result<ConversionPlanPartition, ConversionPlanSliceError> {
    if !result.has_hard_errors() {
        let slices = slice_all_conversion_targets(input, result)?;
        if approval.is_none_or(|value| value.excluded_source_units.is_empty()) {
            return Ok(ConversionPlanPartition {
                slices,
                excluded_source_units: Vec::new(),
            });
        }

        let canonical_units = canonical_source_units(input)?;
        let scheduled_source_unit_ids = canonical_units
            .iter()
            .map(|unit| unit.source_unit_id.clone())
            .collect();
        validate_partial_conversion_approval(approval, &[], &scheduled_source_unit_ids)?;
        return Ok(ConversionPlanPartition {
            slices,
            excluded_source_units: Vec::new(),
        });
    }

    let candidate = build_partial_conversion_candidate(input, result)?;
    validate_partial_conversion_approval(
        approval,
        &candidate.partition.excluded_source_units,
        &candidate.scheduled_source_unit_ids,
    )?;
    Ok(candidate.partition)
}

fn build_partial_conversion_candidate(
    input: &PlanningInput,
    result: &PlanningResult,
) -> Result<PartialConversionCandidate, ConversionPlanSliceError> {
    let canonical_units = canonical_source_units(input)?;
    let canonical_by_ref = canonical_units
        .iter()
        .map(|unit| (unit.scoped_ref(), unit))
        .collect::<BTreeMap<_, _>>();

    let mut scheduled_refs = BTreeSet::new();
    let mut scheduled_source_unit_ids = BTreeSet::new();
    for unit_ref in result.jobs.iter().flat_map(|job| &job.units) {
        let Some(unit) = canonical_by_ref.get(unit_ref) else {
            return Err(ConversionPlanSliceError::UnexpectedScheduledUnit {
                scope_id: unit_ref.scope_id.clone(),
                unit_id: unit_ref.unit_id.clone(),
            });
        };
        scheduled_refs.insert(unit_ref.clone());
        scheduled_source_unit_ids.insert(unit.source_unit_id.clone());
    }

    let mut errors_by_unit =
        BTreeMap::<u1_planner::ScopedUnitRef, Vec<CanonicalExclusionError>>::new();
    for error in &result.errors {
        let Some(scope_id) = error.scope_id.as_deref() else {
            return Err(ConversionPlanSliceError::GlobalPlanError { code: error.code });
        };
        let Some(error_shape) = excludable_error_shape(error.code) else {
            return Err(ConversionPlanSliceError::NonExcludablePlanError { code: error.code });
        };
        match (error_shape, error.unit_id.as_deref()) {
            (ExcludableErrorShape::UnitOnly, None) => {
                return Err(ConversionPlanSliceError::UnitPlanErrorMissingUnit {
                    code: error.code,
                });
            }
            (ExcludableErrorShape::ScopeOnly, Some(unit_id)) => {
                return Err(ConversionPlanSliceError::ScopePlanErrorHasUnit {
                    code: error.code,
                    unit_id: unit_id.to_owned(),
                });
            }
            _ => {}
        }
        let message = error.message.trim();
        if message.is_empty() {
            return Err(ConversionPlanSliceError::EmptyPlanErrorReason { code: error.code });
        }
        let canonical_error = CanonicalExclusionError {
            code: error.code,
            scope_id: scope_id.to_owned(),
            planning_unit_id: error.unit_id.clone(),
            message: message.to_owned(),
        };

        if let Some(unit_id) = error.unit_id.as_deref() {
            let unit_ref = u1_planner::ScopedUnitRef {
                scope_id: scope_id.to_owned(),
                unit_id: unit_id.to_owned(),
            };
            let Some(unit) = canonical_by_ref.get(&unit_ref) else {
                return Err(ConversionPlanSliceError::UnboundScopedPlanError {
                    error_identity: standalone_error_identity(error)?,
                });
            };
            if scheduled_refs.contains(&unit_ref) {
                return Err(
                    ConversionPlanSliceError::ScopedPlanErrorCoversScheduledUnit {
                        code: error.code,
                        source_unit_id: unit.source_unit_id.clone(),
                    },
                );
            }
            errors_by_unit
                .entry(unit_ref)
                .or_default()
                .push(canonical_error);
            continue;
        }

        let scope_units = canonical_units
            .iter()
            .filter(|unit| unit.scope_id == scope_id)
            .collect::<Vec<_>>();
        if scope_units.is_empty() {
            return Err(ConversionPlanSliceError::UnboundScopedPlanError {
                error_identity: standalone_error_identity(error)?,
            });
        }
        if let Some(unit) = scope_units
            .iter()
            .find(|unit| scheduled_refs.contains(&unit.scoped_ref()))
        {
            return Err(
                ConversionPlanSliceError::ScopedPlanErrorCoversScheduledUnit {
                    code: error.code,
                    source_unit_id: unit.source_unit_id.clone(),
                },
            );
        }
        for unit in scope_units {
            errors_by_unit
                .entry(unit.scoped_ref())
                .or_default()
                .push(canonical_error.clone());
        }
    }

    let mut excluded_source_units = Vec::new();
    for unit in canonical_units
        .iter()
        .filter(|unit| !scheduled_refs.contains(&unit.scoped_ref()))
    {
        let Some(mut applicable_errors) = errors_by_unit.remove(&unit.scoped_ref()) else {
            return Err(ConversionPlanSliceError::UnexplainedOmittedSourceUnit {
                source_unit_id: unit.source_unit_id.clone(),
            });
        };
        applicable_errors.sort();
        applicable_errors.dedup();
        excluded_source_units.push(exclusion_evidence(unit, &applicable_errors)?);
    }
    excluded_source_units.sort_by(|left, right| {
        left.source_unit_id
            .cmp(&right.source_unit_id)
            .then_with(|| left.scope_id.cmp(&right.scope_id))
            .then_with(|| left.planning_unit_id.cmp(&right.planning_unit_id))
    });

    if scheduled_refs.is_empty() {
        return Err(ConversionPlanSliceError::EmptyTarget);
    }

    let excluded_ids = excluded_source_units
        .iter()
        .map(|unit| unit.source_unit_id.as_str())
        .collect::<BTreeSet<_>>();
    let partial_input = input_without_excluded_source_units(input, &excluded_ids);
    let mut partial_result = result.clone();
    partial_result.errors.clear();
    let slices = slice_all_conversion_targets(&partial_input, &partial_result)?;

    Ok(PartialConversionCandidate {
        partition: ConversionPlanPartition {
            slices,
            excluded_source_units,
        },
        scheduled_source_unit_ids,
    })
}

fn canonical_source_units(
    input: &PlanningInput,
) -> Result<Vec<CanonicalSourceUnit>, ConversionPlanSliceError> {
    let mut scope_counts = BTreeMap::<&str, usize>::new();
    for scope in &input.scopes {
        *scope_counts.entry(scope.id.as_str()).or_default() += 1;
    }
    if let Some((scope_id, count)) = scope_counts.iter().find(|(_, count)| **count != 1) {
        return Err(ConversionPlanSliceError::EntityCoverageMismatch {
            entity: "input scope",
            id: (*scope_id).to_owned(),
            count: *count,
        });
    }

    let mut unit_counts = BTreeMap::<u1_planner::ScopedUnitRef, usize>::new();
    let mut source_unit_ids = BTreeSet::new();
    let mut units = Vec::new();
    for scope in &input.scopes {
        for unit in &scope.units {
            let unit_ref = u1_planner::ScopedUnitRef {
                scope_id: scope.id.clone(),
                unit_id: unit.id.clone(),
            };
            *unit_counts.entry(unit_ref).or_default() += 1;
            if !source_unit_ids.insert(unit.source_unit_id.clone()) {
                return Err(ConversionPlanSliceError::DuplicateCanonicalSourceUnit {
                    source_unit_id: unit.source_unit_id.clone(),
                });
            }
            units.push(CanonicalSourceUnit {
                scope_id: scope.id.clone(),
                planning_unit_id: unit.id.clone(),
                source_plate_identity: unit.source_plate_id.clone(),
                source_plate_id: normalized_source_plate_id(
                    unit.source_plate_id.as_deref(),
                    &unit.source_unit_id,
                )?,
                source_unit_id: unit.source_unit_id.clone(),
            });
        }
    }
    if let Some((unit_ref, count)) = unit_counts.iter().find(|(_, count)| **count != 1) {
        return Err(ConversionPlanSliceError::EntityCoverageMismatch {
            entity: "input unit",
            id: format!("{}/{}", unit_ref.scope_id, unit_ref.unit_id),
            count: *count,
        });
    }
    Ok(units)
}

fn normalized_source_plate_id(
    value: Option<&str>,
    source_unit_id: &str,
) -> Result<Option<u32>, ConversionPlanSliceError> {
    let Some(value) = value else {
        return Ok(None);
    };
    value
        .strip_prefix("plate-")
        .unwrap_or(value)
        .parse()
        .map(Some)
        .map_err(
            |_| ConversionPlanSliceError::InvalidCanonicalSourcePlateId {
                source_unit_id: source_unit_id.to_owned(),
                source_plate_id: value.to_owned(),
            },
        )
}

fn excludable_error_shape(code: ErrorCode) -> Option<ExcludableErrorShape> {
    match code {
        ErrorCode::DuplicateScopeId
        | ErrorCode::DuplicateUnitId
        | ErrorCode::DuplicateSpoolId
        | ErrorCode::InvalidBuildVolume => None,
        ErrorCode::DuplicateRequirementId
        | ErrorCode::RequestedDirectSpoolsUnavailable
        | ErrorCode::InvalidCmyxFallbackApproval => Some(ExcludableErrorShape::ScopeOnly),
        ErrorCode::UnknownRequirement
        | ErrorCode::UnknownSpool
        | ErrorCode::InvalidBounds
        | ErrorCode::RequestedA1Unavailable
        | ErrorCode::CmyxRecipeUnavailable
        | ErrorCode::MaterialSubstitutionApprovalRequired
        | ErrorCode::FullSpectrumProcessMismatch
        | ErrorCode::MultipleT4SpoolsInUnit
        | ErrorCode::MaterialMismatch
        | ErrorCode::MixedPrintableMaterials
        | ErrorCode::UnitOutOfBounds
        | ErrorCode::PackingFailed => Some(ExcludableErrorShape::UnitOnly),
        ErrorCode::InvalidCmyxRecipe => Some(ExcludableErrorShape::ScopeOrUnit),
    }
}

fn exclusion_evidence(
    unit: &CanonicalSourceUnit,
    errors: &[CanonicalExclusionError],
) -> Result<ExcludedSourceUnit, ConversionPlanSliceError> {
    let reason = errors
        .iter()
        .map(|error| error.message.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(" | ");
    let identity_document = ExclusionIdentityDocument {
        schema: "u1.partial-conversion-exclusion.v1",
        scope_id: &unit.scope_id,
        planning_unit_id: &unit.planning_unit_id,
        source_plate_identity: unit.source_plate_identity.as_deref(),
        source_plate_id: unit.source_plate_id,
        source_unit_id: &unit.source_unit_id,
        errors,
    };
    Ok(ExcludedSourceUnit {
        scope_id: unit.scope_id.clone(),
        planning_unit_id: unit.planning_unit_id.clone(),
        source_plate_id: unit.source_plate_id,
        source_unit_id: unit.source_unit_id.clone(),
        reason,
        error_identity: sha256_json_identity(&identity_document)?,
    })
}

fn standalone_error_identity(error: &PlanError) -> Result<String, ConversionPlanSliceError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Identity<'a> {
        schema: &'static str,
        code: ErrorCode,
        scope_id: Option<&'a str>,
        planning_unit_id: Option<&'a str>,
        message: &'a str,
    }

    sha256_json_identity(&Identity {
        schema: "u1.partial-conversion-plan-error.v1",
        code: error.code,
        scope_id: error.scope_id.as_deref(),
        planning_unit_id: error.unit_id.as_deref(),
        message: error.message.trim(),
    })
}

fn sha256_json_identity(value: &impl Serialize) -> Result<String, ConversionPlanSliceError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| ConversionPlanSliceError::ExclusionEvidenceSerialization)?;
    let digest = Sha256::digest(bytes);
    Ok(format!("sha256:{digest:x}"))
}

fn input_without_excluded_source_units(
    input: &PlanningInput,
    excluded_source_unit_ids: &BTreeSet<&str>,
) -> PlanningInput {
    let mut partial = input.clone();
    for scope in &mut partial.scopes {
        scope
            .units
            .retain(|unit| !excluded_source_unit_ids.contains(unit.source_unit_id.as_str()));
        let requirement_ids = scope
            .units
            .iter()
            .flat_map(|unit| unit.requirement_ids.iter().cloned())
            .collect::<BTreeSet<_>>();
        scope
            .requirements
            .retain(|requirement| requirement_ids.contains(&requirement.id));
        scope
            .direct_assignments
            .retain(|assignment| requirement_ids.contains(&assignment.requirement_id));
        scope
            .approved_cmyx_fallbacks
            .retain(|approval| requirement_ids.contains(&approval.requirement_id));
        scope
            .approved_material_substitutions
            .retain(|approval| requirement_ids.contains(&approval.requirement_id));
    }
    partial.scopes.retain(|scope| !scope.units.is_empty());
    partial
}

fn validate_partial_conversion_approval(
    approval: Option<&PartialConversionApproval>,
    expected: &[ExcludedSourceUnit],
    scheduled_source_unit_ids: &BTreeSet<String>,
) -> Result<(), ConversionPlanSliceError> {
    let Some(approval) = approval else {
        if expected.is_empty() {
            return Ok(());
        }
        return Err(
            ConversionPlanSliceError::PartialConversionApprovalRequired {
                source_unit_ids: expected
                    .iter()
                    .map(|unit| unit.source_unit_id.clone())
                    .collect(),
            },
        );
    };

    let expected_by_source_id = expected
        .iter()
        .map(|unit| (unit.source_unit_id.as_str(), unit))
        .collect::<BTreeMap<_, _>>();
    let mut submitted_source_ids = BTreeSet::new();
    for submitted in &approval.excluded_source_units {
        if !submitted_source_ids.insert(submitted.source_unit_id.as_str()) {
            return Err(ConversionPlanSliceError::DuplicateApprovedExclusion {
                source_unit_id: submitted.source_unit_id.clone(),
            });
        }
    }
    for submitted in &approval.excluded_source_units {
        if let Some(canonical) = expected_by_source_id.get(submitted.source_unit_id.as_str()) {
            if submitted != *canonical {
                return Err(ConversionPlanSliceError::StaleApprovedExclusion {
                    source_unit_id: submitted.source_unit_id.clone(),
                });
            }
            continue;
        }
        if scheduled_source_unit_ids.contains(&submitted.source_unit_id) {
            return Err(ConversionPlanSliceError::ScheduledUnitExclusion {
                source_unit_id: submitted.source_unit_id.clone(),
            });
        }
        return Err(ConversionPlanSliceError::UnknownApprovedExclusion {
            source_unit_id: submitted.source_unit_id.clone(),
        });
    }
    for canonical in expected {
        if !submitted_source_ids.contains(canonical.source_unit_id.as_str()) {
            return Err(ConversionPlanSliceError::MissingApprovedExclusion {
                source_unit_id: canonical.source_unit_id.clone(),
            });
        }
    }
    Ok(())
}

fn validate_exact_partition(
    input: &PlanningInput,
    result: &PlanningResult,
    slices: &[ConversionPlanSlice],
) -> Result<(), ConversionPlanSliceError> {
    let target_for_batch = |batch: &u1_planner::PlannedBatch| {
        [
            ConversionTarget::U1Direct,
            ConversionTarget::U1FullSpectrum,
            ConversionTarget::A1MiniMono,
        ]
        .into_iter()
        .find(|target| target.matches(batch.printer, batch.strategy))
    };
    for batch in &result.batches {
        if target_for_batch(batch).is_none() {
            return Err(ConversionPlanSliceError::UnsupportedTarget {
                batch_id: batch.id.clone(),
            });
        }
    }

    let sliced_batches = slices
        .iter()
        .flat_map(|slice| slice.result.batches.iter().map(|batch| batch.id.as_str()))
        .collect::<Vec<_>>();
    let sliced_jobs = slices
        .iter()
        .flat_map(|slice| slice.result.jobs.iter().map(|job| job.id.as_str()))
        .collect::<Vec<_>>();
    let sliced_plates = slices
        .iter()
        .flat_map(|slice| slice.result.plates.iter().map(|plate| plate.id.as_str()))
        .collect::<Vec<_>>();
    ensure_entity_partition(
        "batch",
        result.batches.iter().map(|batch| batch.id.as_str()),
        &sliced_batches,
    )?;
    ensure_entity_partition(
        "job",
        result.jobs.iter().map(|job| job.id.as_str()),
        &sliced_jobs,
    )?;
    ensure_entity_partition(
        "plate",
        result.plates.iter().map(|plate| plate.id.as_str()),
        &sliced_plates,
    )?;

    let mut canonical_units = BTreeMap::<u1_planner::ScopedUnitRef, usize>::new();
    let mut canonical_source_units = BTreeSet::new();
    for scope in &input.scopes {
        for unit in &scope.units {
            *canonical_units
                .entry(u1_planner::ScopedUnitRef {
                    scope_id: scope.id.clone(),
                    unit_id: unit.id.clone(),
                })
                .or_default() += 1;
            if !canonical_source_units.insert(unit.source_unit_id.clone()) {
                return Err(ConversionPlanSliceError::DuplicateCanonicalSourceUnit {
                    source_unit_id: unit.source_unit_id.clone(),
                });
            }
        }
    }
    for (unit, count) in &canonical_units {
        if *count != 1 {
            return Err(ConversionPlanSliceError::EntityCoverageMismatch {
                entity: "input unit",
                id: format!("{}/{}", unit.scope_id, unit.unit_id),
                count: *count,
            });
        }
    }

    let mut scheduled_units = BTreeMap::<u1_planner::ScopedUnitRef, usize>::new();
    for slice in slices {
        for job in &slice.result.jobs {
            for unit in &job.units {
                *scheduled_units.entry(unit.clone()).or_default() += 1;
            }
        }
    }
    for unit in scheduled_units.keys() {
        if !canonical_units.contains_key(unit) {
            return Err(ConversionPlanSliceError::UnexpectedScheduledUnit {
                scope_id: unit.scope_id.clone(),
                unit_id: unit.unit_id.clone(),
            });
        }
    }
    for unit in canonical_units.keys() {
        let count = scheduled_units.get(unit).copied().unwrap_or_default();
        if count != 1 {
            return Err(ConversionPlanSliceError::EntityCoverageMismatch {
                entity: "scheduled unit",
                id: format!("{}/{}", unit.scope_id, unit.unit_id),
                count,
            });
        }
    }
    Ok(())
}

fn validate_batch_reference_closure(
    result: &PlanningResult,
) -> Result<(), ConversionPlanSliceError> {
    let mut job_batch_counts = BTreeMap::<&str, usize>::new();
    let mut plate_batch_counts = BTreeMap::<&str, usize>::new();
    for batch in &result.batches {
        for job_id in &batch.job_ids {
            *job_batch_counts.entry(job_id).or_default() += 1;
        }
        for plate_id in &batch.plate_ids {
            *plate_batch_counts.entry(plate_id).or_default() += 1;
        }
    }
    for job in &result.jobs {
        require_one("batch reference for job", &job.id, &job_batch_counts)?;
    }
    for plate in &result.plates {
        require_one("batch reference for plate", &plate.id, &plate_batch_counts)?;
        let owning_batches = result
            .batches
            .iter()
            .filter(|batch| batch.plate_ids.contains(&plate.id))
            .collect::<Vec<_>>();
        if owning_batches.len() != 1 || !owning_batches[0].job_ids.contains(&plate.job_id) {
            return Err(ConversionPlanSliceError::PlateJobMismatch {
                plate_id: plate.id.clone(),
                job_id: plate.job_id.clone(),
            });
        }
    }
    Ok(())
}

fn ensure_entity_partition<'a>(
    entity: &'static str,
    canonical_ids: impl Iterator<Item = &'a str>,
    sliced_ids: &[&str],
) -> Result<(), ConversionPlanSliceError> {
    let mut canonical_counts = BTreeMap::<&str, usize>::new();
    for id in canonical_ids {
        *canonical_counts.entry(id).or_default() += 1;
    }
    for (id, canonical_count) in &canonical_counts {
        let count = sliced_ids
            .iter()
            .filter(|candidate| **candidate == *id)
            .count();
        if *canonical_count != 1 || count != 1 {
            return Err(ConversionPlanSliceError::EntityCoverageMismatch {
                entity,
                id: (*id).to_owned(),
                count,
            });
        }
    }
    for id in sliced_ids {
        if !canonical_counts.contains_key(id) {
            return Err(ConversionPlanSliceError::EntityCoverageMismatch {
                entity,
                id: (*id).to_owned(),
                count: 1,
            });
        }
    }
    Ok(())
}

fn require_one(
    entity: &'static str,
    id: &str,
    counts: &BTreeMap<&str, usize>,
) -> Result<(), ConversionPlanSliceError> {
    let count = counts.get(id).copied().unwrap_or_default();
    if count == 1 {
        return Ok(());
    }
    Err(ConversionPlanSliceError::EntityCoverageMismatch {
        entity,
        id: id.to_owned(),
        count,
    })
}

pub fn slice_conversion_plan(
    input: &PlanningInput,
    result: &PlanningResult,
    target: ConversionTarget,
) -> Result<ConversionPlanSlice, ConversionPlanSliceError> {
    if result.has_hard_errors() {
        return Err(ConversionPlanSliceError::BlockingPlan);
    }

    let selected_batches = result
        .batches
        .iter()
        .filter(|batch| target.matches(batch.printer, batch.strategy))
        .cloned()
        .collect::<Vec<_>>();
    if selected_batches.is_empty() {
        return Err(ConversionPlanSliceError::EmptyTarget);
    }

    let selected_job_ids = selected_batches
        .iter()
        .flat_map(|batch| batch.job_ids.iter().cloned())
        .collect::<BTreeSet<_>>();
    let selected_plate_ids = selected_batches
        .iter()
        .flat_map(|batch| batch.plate_ids.iter().cloned())
        .collect::<BTreeSet<_>>();

    for batch in &selected_batches {
        for job_id in &batch.job_ids {
            if !result.jobs.iter().any(|job| job.id == *job_id) {
                return Err(ConversionPlanSliceError::UnknownJob {
                    batch_id: batch.id.clone(),
                    job_id: job_id.clone(),
                });
            }
        }
        for plate_id in &batch.plate_ids {
            if !result.plates.iter().any(|plate| plate.id == *plate_id) {
                return Err(ConversionPlanSliceError::UnknownPlate {
                    batch_id: batch.id.clone(),
                    plate_id: plate_id.clone(),
                });
            }
        }
    }

    let jobs = result
        .jobs
        .iter()
        .filter(|job| selected_job_ids.contains(&job.id))
        .map(|job| {
            if !target.matches(job.printer, job.strategy) {
                return Err(ConversionPlanSliceError::TargetMismatch {
                    job_id: job.id.clone(),
                });
            }
            Ok(job.clone())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let plates = result
        .plates
        .iter()
        .filter(|plate| selected_plate_ids.contains(&plate.id))
        .cloned()
        .collect::<Vec<_>>();

    let selected_refs = jobs
        .iter()
        .flat_map(|job| job.units.iter().cloned())
        .collect::<BTreeSet<_>>();
    let mut placement_counts = std::collections::BTreeMap::new();
    let mut positioned_counts = std::collections::BTreeMap::new();
    for plate in &plates {
        for unit in &plate.units {
            if !selected_refs.contains(unit) {
                return Err(ConversionPlanSliceError::UnitPlacementMismatch {
                    scope_id: unit.scope_id.clone(),
                    unit_id: unit.unit_id.clone(),
                });
            }
            *placement_counts.entry(unit.clone()).or_insert(0_usize) += 1;
        }
        for placement in &plate.placements {
            if !selected_refs.contains(&placement.unit) || !plate.units.contains(&placement.unit) {
                return Err(ConversionPlanSliceError::UnitPlacementMismatch {
                    scope_id: placement.unit.scope_id.clone(),
                    unit_id: placement.unit.unit_id.clone(),
                });
            }
            *positioned_counts
                .entry(placement.unit.clone())
                .or_insert(0_usize) += 1;
        }
    }
    for unit in &selected_refs {
        if placement_counts.get(unit).copied() != Some(1)
            || positioned_counts.get(unit).copied() != Some(1)
        {
            return Err(ConversionPlanSliceError::UnitPlacementMismatch {
                scope_id: unit.scope_id.clone(),
                unit_id: unit.unit_id.clone(),
            });
        }
    }

    let selected_scope_ids = selected_refs
        .iter()
        .map(|unit| unit.scope_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut source_unit_ids = BTreeSet::new();
    let mut scopes = Vec::new();
    for scope in &input.scopes {
        if !selected_scope_ids.contains(scope.id.as_str()) {
            continue;
        }
        let mut selected = scope.clone();
        selected.units.retain(|unit| {
            selected_refs.contains(&u1_planner::ScopedUnitRef {
                scope_id: scope.id.clone(),
                unit_id: unit.id.clone(),
            })
        });
        for unit in &selected.units {
            if !source_unit_ids.insert(unit.source_unit_id.clone()) {
                return Err(ConversionPlanSliceError::DuplicateSourceUnit {
                    source_unit_id: unit.source_unit_id.clone(),
                });
            }
        }
        let requirement_ids = selected
            .units
            .iter()
            .flat_map(|unit| unit.requirement_ids.iter().cloned())
            .collect::<BTreeSet<_>>();
        selected
            .requirements
            .retain(|requirement| requirement_ids.contains(&requirement.id));
        selected
            .direct_assignments
            .retain(|assignment| requirement_ids.contains(&assignment.requirement_id));
        selected
            .approved_cmyx_fallbacks
            .retain(|approval| requirement_ids.contains(&approval.requirement_id));
        selected
            .approved_material_substitutions
            .retain(|approval| requirement_ids.contains(&approval.requirement_id));
        if selected.units.is_empty() {
            continue;
        }
        // Preserve the original strategy where possible. A1 jobs can originate
        // from either Direct or CMY+X mono candidates, and their writer relies
        // on the planned A1 job rather than this source-scope preference.
        if target == ConversionTarget::U1Direct {
            let contains_cmyx_solid = jobs.iter().any(|job| {
                job.strategy == ColorStrategy::CmyxSolid
                    && job.units.iter().any(|unit| unit.scope_id == selected.id)
            });
            selected.strategy = if contains_cmyx_solid {
                ScopeStrategy::CmyxFullSpectrum
            } else {
                ScopeStrategy::DirectSpools
            };
        } else if target == ConversionTarget::U1FullSpectrum {
            selected.strategy = ScopeStrategy::CmyxFullSpectrum;
        }
        scopes.push(selected);
    }

    for plate in &plates {
        for unit in &plate.units {
            let exists = scopes.iter().any(|scope| {
                scope.id == unit.scope_id && scope.units.iter().any(|item| item.id == unit.unit_id)
            });
            if !exists {
                return Err(ConversionPlanSliceError::UnknownUnit {
                    plate_id: plate.id.clone(),
                    scope_id: unit.scope_id.clone(),
                    unit_id: unit.unit_id.clone(),
                });
            }
        }
    }

    let scope_ids = scopes
        .iter()
        .map(|scope| scope.id.as_str())
        .collect::<BTreeSet<_>>();
    let scope_options = result
        .scope_options
        .iter()
        .filter(|option| scope_ids.contains(option.scope_id.as_str()))
        .cloned()
        .collect();
    let warnings = result
        .warnings
        .iter()
        .filter(|warning| match (&warning.scope_id, &warning.unit_id) {
            (Some(scope_id), Some(unit_id)) => selected_refs.contains(&u1_planner::ScopedUnitRef {
                scope_id: scope_id.clone(),
                unit_id: unit_id.clone(),
            }),
            (Some(scope_id), None) => scope_ids.contains(scope_id.as_str()),
            (None, Some(unit_id)) => selected_refs.iter().any(|unit| unit.unit_id == *unit_id),
            (None, None) => true,
        })
        .cloned()
        .collect();

    let sliced_input = PlanningInput {
        scopes,
        inventory: input.inventory.clone(),
        current_toolheads: input.current_toolheads.clone(),
        config: input.config.clone(),
    };
    let sliced_result = PlanningResult {
        scope_options,
        jobs,
        plates,
        t4_swap_count: selected_batches
            .iter()
            .filter(|batch| batch.t4_swap_before)
            .count() as u32,
        a1_spool_change_count: selected_a1_spool_changes(&selected_batches),
        batches: selected_batches,
        final_toolheads: result.final_toolheads.clone(),
        warnings,
        errors: Vec::new(),
    };

    Ok(ConversionPlanSlice {
        target,
        input: sliced_input,
        result: sliced_result,
        source_unit_ids: source_unit_ids.into_iter().collect(),
    })
}

fn selected_a1_spool_changes(batches: &[u1_planner::PlannedBatch]) -> u32 {
    batches
        .iter()
        .filter(|batch| batch.printer == Printer::A1Mini)
        .filter(|batch| {
            batch.setup_actions.iter().any(|action| {
                action.toolhead.is_none() && action.kind == u1_planner::SetupActionKind::Load
            })
        })
        .count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use u1_planner::{
        CmySetup, ColorStrategy, CurrentToolheadState, Estimate, PackingStatus, PlannedBatch,
        PlannedJob, PlannedPlacement, PlannedPlate, PlannerConfig, PlanningInput, PlanningResult,
        PrintScope, PrintableUnit, Printer, PrinterLoadout, ScopeStrategy, ScopedUnitRef,
        ToolheadSlotState, U1Loadout,
    };

    fn fixture() -> (PlanningInput, PlanningResult) {
        let direct_ref = ScopedUnitRef {
            scope_id: "scope-direct".into(),
            unit_id: "direct-unit".into(),
        };
        let a1_ref = ScopedUnitRef {
            scope_id: "scope-a1".into(),
            unit_id: "a1-unit".into(),
        };
        let scope = |id: &str, unit_id: &str, source_unit_id: &str| PrintScope {
            id: id.into(),
            display_name: id.into(),
            requirements: Vec::new(),
            units: vec![PrintableUnit {
                id: unit_id.into(),
                source_unit_id: source_unit_id.into(),
                source_object_id: 1,
                source_instance_id: 0,
                source_model_path: None,
                display_name: unit_id.into(),
                source_plate_id: Some("plate-1".into()),
                requirement_ids: Vec::new(),
                bounds: u1_planner::BoundsMm::from_size(10.0, 10.0, 10.0),
                source_layer_height_mm: Some(0.2),
                printer_preference: u1_planner::PrinterPreference::Auto,
            }],
            strategy: ScopeStrategy::Auto,
            direct_assignments: Vec::new(),
            approved_cmyx_fallbacks: Vec::new(),
            approved_material_substitutions: Vec::new(),
        };
        let u1_loadout = PrinterLoadout::U1 {
            loadout: U1Loadout {
                slots: [Some("black".into()), None, None, None],
            },
        };
        let job = |id: &str,
                   unit: ScopedUnitRef,
                   printer: Printer,
                   strategy: ColorStrategy,
                   loadout: PrinterLoadout| PlannedJob {
            id: id.into(),
            scope_ids: vec![unit.scope_id.clone()],
            units: vec![unit],
            printer,
            strategy,
            loadout,
            printable_materials: Vec::new(),
            fast_mono: true,
            full_spectrum_process: None,
            color_mappings: Vec::new(),
            estimated_tool_changes: Estimate::RequiresSlicing,
        };
        let plate = |id: &str, job_id: &str, unit: ScopedUnitRef, printer: Printer| PlannedPlate {
            id: id.into(),
            job_id: job_id.into(),
            printer,
            units: vec![unit.clone()],
            placements: vec![PlannedPlacement {
                unit,
                target_min_x_mm: 5.0,
                target_min_y_mm: 5.0,
            }],
            prime_tower: None,
            packing_status: PackingStatus::PackedAabb,
            individual_bounds_validated: true,
            full_spectrum_process: None,
            estimated_print_time_seconds: Estimate::RequiresSlicing,
            estimated_material_grams: Estimate::RequiresSlicing,
        };
        let input = PlanningInput {
            scopes: vec![
                scope("scope-direct", "direct-unit", "source-direct"),
                scope("scope-a1", "a1-unit", "source-a1"),
            ],
            inventory: Vec::new(),
            current_toolheads: CurrentToolheadState {
                slots: std::array::from_fn(|_| ToolheadSlotState::Empty),
            },
            config: PlannerConfig::with_cmy_setup(CmySetup {
                cyan_spool_id: "cyan".into(),
                magenta_spool_id: "magenta".into(),
                yellow_spool_id: "yellow".into(),
                default_t4_spool_id: None,
            }),
        };
        let result = PlanningResult {
            scope_options: Vec::new(),
            jobs: vec![
                job(
                    "job-direct",
                    direct_ref.clone(),
                    Printer::U1,
                    ColorStrategy::DirectSpools,
                    u1_loadout.clone(),
                ),
                job(
                    "job-a1",
                    a1_ref.clone(),
                    Printer::A1Mini,
                    ColorStrategy::A1Mono,
                    PrinterLoadout::A1Mini {
                        spool_id: "a1-black".into(),
                    },
                ),
            ],
            plates: vec![
                plate("plate-direct", "job-direct", direct_ref, Printer::U1),
                plate("plate-a1", "job-a1", a1_ref, Printer::A1Mini),
            ],
            batches: vec![
                PlannedBatch {
                    id: "batch-direct".into(),
                    printer: Printer::U1,
                    strategy: ColorStrategy::DirectSpools,
                    loadout: u1_loadout,
                    job_ids: vec!["job-direct".into()],
                    plate_ids: vec!["plate-direct".into()],
                    setup_actions: Vec::new(),
                    t4_swap_before: false,
                },
                PlannedBatch {
                    id: "batch-a1".into(),
                    printer: Printer::A1Mini,
                    strategy: ColorStrategy::A1Mono,
                    loadout: PrinterLoadout::A1Mini {
                        spool_id: "a1-black".into(),
                    },
                    job_ids: vec!["job-a1".into()],
                    plate_ids: vec!["plate-a1".into()],
                    setup_actions: Vec::new(),
                    t4_swap_before: false,
                },
            ],
            t4_swap_count: 0,
            a1_spool_change_count: 0,
            final_toolheads: CurrentToolheadState::default(),
            warnings: Vec::new(),
            errors: Vec::new(),
        };
        (input, result)
    }

    fn add_cmyx_solid_fixture(input: &mut PlanningInput, result: &mut PlanningResult) {
        let solid_ref = ScopedUnitRef {
            scope_id: "scope-solid".into(),
            unit_id: "solid-unit".into(),
        };
        let mut solid_scope = input.scopes[0].clone();
        solid_scope.id = solid_ref.scope_id.clone();
        solid_scope.display_name = "CMY+X solid".into();
        solid_scope.units[0].id = solid_ref.unit_id.clone();
        solid_scope.units[0].source_unit_id = "source-solid".into();
        input.scopes.push(solid_scope);

        let mut solid_job = result.jobs[0].clone();
        solid_job.id = "job-solid".into();
        solid_job.scope_ids = vec![solid_ref.scope_id.clone()];
        solid_job.units = vec![solid_ref.clone()];
        solid_job.strategy = ColorStrategy::CmyxSolid;
        solid_job.fast_mono = false;
        result.jobs.push(solid_job);

        let mut solid_plate = result.plates[0].clone();
        solid_plate.id = "plate-solid".into();
        solid_plate.job_id = "job-solid".into();
        solid_plate.units = vec![solid_ref.clone()];
        solid_plate.placements[0].unit = solid_ref;
        result.plates.push(solid_plate);

        let mut solid_batch = result.batches[0].clone();
        solid_batch.id = "batch-solid".into();
        solid_batch.strategy = ColorStrategy::CmyxSolid;
        solid_batch.job_ids = vec!["job-solid".into()];
        solid_batch.plate_ids = vec!["plate-solid".into()];
        result.batches.push(solid_batch);
    }

    fn add_omitted_scope(
        input: &mut PlanningInput,
        scope_id: &str,
        units: &[(&str, &str, Option<u32>)],
    ) {
        let mut scope = input.scopes[0].clone();
        scope.id = scope_id.into();
        scope.display_name = scope_id.into();
        scope.units = units
            .iter()
            .enumerate()
            .map(|(index, (unit_id, source_unit_id, source_plate_id))| {
                let mut unit = input.scopes[0].units[0].clone();
                unit.id = (*unit_id).into();
                unit.source_unit_id = (*source_unit_id).into();
                unit.source_object_id = 100 + index as u32;
                unit.display_name = (*unit_id).into();
                unit.source_plate_id = source_plate_id.map(|id| format!("plate-{id}"));
                unit
            })
            .collect();
        input.scopes.push(scope);
    }

    fn plan_error(
        code: ErrorCode,
        scope_id: Option<&str>,
        unit_id: Option<&str>,
        message: &str,
    ) -> PlanError {
        PlanError {
            code,
            scope_id: scope_id.map(str::to_owned),
            unit_id: unit_id.map(str::to_owned),
            message: message.into(),
        }
    }

    fn approval_for(exclusions: Vec<ExcludedSourceUnit>) -> PartialConversionApproval {
        PartialConversionApproval {
            excluded_source_units: exclusions,
        }
    }

    fn broken_unit_fixture() -> (PlanningInput, PlanningResult) {
        let (mut input, mut result) = fixture();
        add_omitted_scope(
            &mut input,
            "scope-broken",
            &[("broken-unit", "source-broken", Some(42))],
        );
        result.errors.push(plan_error(
            ErrorCode::InvalidBounds,
            Some("scope-broken"),
            Some("broken-unit"),
            "Unit bounds are invalid.",
        ));
        (input, result)
    }

    #[test]
    fn slices_one_target_without_leaking_other_units() {
        let (input, result) = fixture();
        let sliced = slice_conversion_plan(&input, &result, ConversionTarget::A1MiniMono)
            .expect("valid A1 slice");
        assert_eq!(sliced.input.scopes.len(), 1);
        assert_eq!(sliced.result.jobs.len(), 1);
        assert_eq!(sliced.result.plates.len(), 1);
        assert_eq!(sliced.result.batches.len(), 1);
        assert_eq!(sliced.source_unit_ids, ["source-a1"]);
    }

    #[test]
    fn rejects_a_selected_unit_that_is_not_placed_exactly_once() {
        let (input, mut result) = fixture();
        result.plates[0].units.clear();
        let error = slice_conversion_plan(&input, &result, ConversionTarget::U1Direct)
            .expect_err("missing placement must fail closed");
        assert!(matches!(
            error,
            ConversionPlanSliceError::UnitPlacementMismatch { .. }
        ));
    }

    #[test]
    fn rejects_an_extra_placement_outside_the_selected_job() {
        let (input, mut result) = fixture();
        result.plates[0].placements.push(PlannedPlacement {
            unit: ScopedUnitRef {
                scope_id: "scope-extra".into(),
                unit_id: "unit-extra".into(),
            },
            target_min_x_mm: 15.0,
            target_min_y_mm: 15.0,
        });

        let error = slice_conversion_plan(&input, &result, ConversionTarget::U1Direct)
            .expect_err("an extra placement must fail closed");

        assert_eq!(
            error,
            ConversionPlanSliceError::UnitPlacementMismatch {
                scope_id: "scope-extra".into(),
                unit_id: "unit-extra".into(),
            }
        );
    }

    #[test]
    fn all_targets_form_an_exact_partition() {
        let (input, result) = fixture();
        let slices = slice_all_conversion_targets(&input, &result)
            .expect("the canonical mixed plan is fully covered");
        assert_eq!(slices.len(), 2);
        assert_eq!(slices[0].target, ConversionTarget::U1Direct);
        assert_eq!(slices[1].target, ConversionTarget::A1MiniMono);
    }

    #[test]
    fn cmyx_solid_is_partitioned_into_u1_direct_without_losing_scope_intent() {
        let (mut input, mut result) = fixture();
        add_cmyx_solid_fixture(&mut input, &mut result);

        let slices = slice_all_conversion_targets(&input, &result)
            .expect("CMY+X solid jobs use the direct native conversion target");
        assert_eq!(slices.len(), 2);
        let direct = slices
            .iter()
            .find(|slice| slice.target == ConversionTarget::U1Direct)
            .expect("U1 direct slice");
        assert_eq!(direct.result.jobs.len(), 2);
        assert_eq!(direct.result.plates.len(), 2);
        assert_eq!(direct.result.batches.len(), 2);
        assert_eq!(
            direct
                .result
                .jobs
                .iter()
                .map(|job| job.strategy)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([ColorStrategy::DirectSpools, ColorStrategy::CmyxSolid])
        );
        assert_eq!(
            direct
                .input
                .scopes
                .iter()
                .map(|scope| (scope.id.as_str(), scope.strategy))
                .collect::<BTreeMap<_, _>>(),
            BTreeMap::from([
                ("scope-direct", ScopeStrategy::DirectSpools),
                ("scope-solid", ScopeStrategy::CmyxFullSpectrum),
            ])
        );
        assert_eq!(
            direct.source_unit_ids,
            ["source-direct".to_owned(), "source-solid".to_owned()]
        );
    }

    #[test]
    fn all_targets_reject_an_orphaned_job() {
        let (input, mut result) = fixture();
        let mut orphan = result.jobs[0].clone();
        orphan.id = "job-orphan".into();
        result.jobs.push(orphan);
        let error = slice_all_conversion_targets(&input, &result)
            .expect_err("an orphaned canonical job must fail closed");
        assert_eq!(
            error,
            ConversionPlanSliceError::EntityCoverageMismatch {
                entity: "batch reference for job",
                id: "job-orphan".into(),
                count: 0,
            }
        );
    }

    #[test]
    fn all_targets_reject_a_unit_missing_from_the_schedule() {
        let (mut input, result) = fixture();
        input.scopes.push(PrintScope {
            id: "scope-missing".into(),
            display_name: "Missing".into(),
            requirements: Vec::new(),
            units: vec![PrintableUnit {
                id: "unit-missing".into(),
                source_unit_id: "source-missing".into(),
                source_object_id: 3,
                source_instance_id: 0,
                source_model_path: None,
                display_name: "Missing".into(),
                source_plate_id: None,
                requirement_ids: Vec::new(),
                bounds: u1_planner::BoundsMm::from_size(1.0, 1.0, 1.0),
                source_layer_height_mm: Some(0.2),
                printer_preference: u1_planner::PrinterPreference::Auto,
            }],
            strategy: ScopeStrategy::Auto,
            direct_assignments: Vec::new(),
            approved_cmyx_fallbacks: Vec::new(),
            approved_material_substitutions: Vec::new(),
        });
        let error = slice_all_conversion_targets(&input, &result)
            .expect_err("an omitted source unit must fail closed");
        assert_eq!(
            error,
            ConversionPlanSliceError::EntityCoverageMismatch {
                entity: "scheduled unit",
                id: "scope-missing/unit-missing".into(),
                count: 0,
            }
        );
    }

    #[test]
    fn all_targets_reject_a_job_referenced_by_two_batches() {
        let (input, mut result) = fixture();
        result.batches[1].job_ids.push("job-direct".into());
        let error = slice_all_conversion_targets(&input, &result)
            .expect_err("a job cannot be routed through two physical batches");
        assert_eq!(
            error,
            ConversionPlanSliceError::EntityCoverageMismatch {
                entity: "batch reference for job",
                id: "job-direct".into(),
                count: 2,
            }
        );
    }

    #[test]
    fn partial_api_preserves_the_complete_error_free_path() {
        let (input, result) = fixture();
        let strict = slice_all_conversion_targets(&input, &result).expect("strict partition");

        let partition = slice_all_conversion_targets_with_partial_approval(&input, &result, None)
            .expect("approval is unnecessary for a complete plan");

        assert_eq!(partition.slices, strict);
        assert!(partition.excluded_source_units.is_empty());
        assert_eq!(
            canonical_partial_conversion_exclusions(&input, &result)
                .expect("complete plan remains valid"),
            Vec::<ExcludedSourceUnit>::new()
        );
    }

    #[test]
    fn derives_durable_canonical_evidence_for_an_omitted_unit() {
        let (input, result) = broken_unit_fixture();

        let exclusions = canonical_partial_conversion_exclusions(&input, &result)
            .expect("one unit-level hard error is excludable");

        assert_eq!(exclusions.len(), 1);
        let exclusion = &exclusions[0];
        assert_eq!(exclusion.scope_id, "scope-broken");
        assert_eq!(exclusion.planning_unit_id, "broken-unit");
        assert_eq!(exclusion.source_plate_id, Some(42));
        assert_eq!(exclusion.source_unit_id, "source-broken");
        assert_eq!(exclusion.reason, "Unit bounds are invalid.");
        assert_eq!(
            exclusion.error_identity,
            "sha256:dd70b77283d4dde155f8d532aafc074f537cf280b8fdeba257dbc02731baf847"
        );

        let json = serde_json::to_value(exclusion).expect("serializable evidence");
        assert_eq!(json["scopeId"], "scope-broken");
        assert_eq!(json["planningUnitId"], "broken-unit");
        assert_eq!(json["sourcePlateId"], 42);
        assert_eq!(json["sourceUnitId"], "source-broken");
        assert!(json.get("scope_id").is_none());
    }

    #[test]
    fn exact_partial_approval_returns_valid_mixed_target_slices_and_evidence() {
        let (input, result) = broken_unit_fixture();
        let exclusions =
            canonical_partial_conversion_exclusions(&input, &result).expect("canonical exclusions");
        let approval = approval_for(exclusions.clone());

        let partition =
            slice_all_conversion_targets_with_partial_approval(&input, &result, Some(&approval))
                .expect("exact approval permits the valid subset");

        assert_eq!(partition.excluded_source_units, exclusions);
        assert_eq!(partition.slices.len(), 2);
        assert_eq!(partition.slices[0].target, ConversionTarget::U1Direct);
        assert_eq!(partition.slices[1].target, ConversionTarget::A1MiniMono);
        assert!(
            partition
                .slices
                .iter()
                .all(|slice| slice.result.errors.is_empty())
        );
        assert_eq!(
            partition
                .slices
                .iter()
                .flat_map(|slice| slice.source_unit_ids.iter().map(String::as_str))
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["source-a1", "source-direct"])
        );
    }

    #[test]
    fn partial_conversion_requires_explicit_approval_of_every_exclusion() {
        let (input, result) = broken_unit_fixture();

        let error = slice_all_conversion_targets_with_partial_approval(&input, &result, None)
            .expect_err("implicit omission must fail closed");

        assert_eq!(
            error,
            ConversionPlanSliceError::PartialConversionApprovalRequired {
                source_unit_ids: vec!["source-broken".into()],
            }
        );

        let empty = PartialConversionApproval::default();
        let error =
            slice_all_conversion_targets_with_partial_approval(&input, &result, Some(&empty))
                .expect_err("an explicit but incomplete approval must fail closed");
        assert_eq!(
            error,
            ConversionPlanSliceError::MissingApprovedExclusion {
                source_unit_id: "source-broken".into(),
            }
        );
    }

    #[test]
    fn global_and_structural_errors_cannot_be_approved_as_exclusions() {
        let (input, mut result) = fixture();
        result.errors.push(plan_error(
            ErrorCode::InvalidBuildVolume,
            None,
            None,
            "Invalid build volume.",
        ));
        assert_eq!(
            canonical_partial_conversion_exclusions(&input, &result)
                .expect_err("global errors block every conversion"),
            ConversionPlanSliceError::GlobalPlanError {
                code: ErrorCode::InvalidBuildVolume,
            }
        );

        let (input, mut result) = broken_unit_fixture();
        result.errors[0].code = ErrorCode::DuplicateUnitId;
        assert_eq!(
            canonical_partial_conversion_exclusions(&input, &result)
                .expect_err("structurally ambiguous IDs are not excludable"),
            ConversionPlanSliceError::NonExcludablePlanError {
                code: ErrorCode::DuplicateUnitId,
            }
        );
    }

    #[test]
    fn hard_error_scope_shape_must_match_the_backend_contract() {
        let (input, mut result) = broken_unit_fixture();
        result.errors[0].unit_id = None;
        assert_eq!(
            canonical_partial_conversion_exclusions(&input, &result)
                .expect_err("InvalidBounds must identify one exact unit"),
            ConversionPlanSliceError::UnitPlanErrorMissingUnit {
                code: ErrorCode::InvalidBounds,
            }
        );

        let (input, mut result) = broken_unit_fixture();
        result.errors[0].code = ErrorCode::RequestedDirectSpoolsUnavailable;
        assert_eq!(
            canonical_partial_conversion_exclusions(&input, &result)
                .expect_err("the direct-loadout error must cover the entire scope"),
            ConversionPlanSliceError::ScopePlanErrorHasUnit {
                code: ErrorCode::RequestedDirectSpoolsUnavailable,
                unit_id: "broken-unit".into(),
            }
        );
    }

    #[test]
    fn every_omitted_unit_must_have_an_applicable_hard_error() {
        let (mut input, mut result) = fixture();
        add_omitted_scope(
            &mut input,
            "scope-broken",
            &[
                ("broken-one", "source-broken-one", Some(10)),
                ("broken-two", "source-broken-two", Some(10)),
            ],
        );
        result.errors.push(plan_error(
            ErrorCode::InvalidBounds,
            Some("scope-broken"),
            Some("broken-one"),
            "First unit is invalid.",
        ));

        let error = canonical_partial_conversion_exclusions(&input, &result)
            .expect_err("the second omission is unexplained");

        assert_eq!(
            error,
            ConversionPlanSliceError::UnexplainedOmittedSourceUnit {
                source_unit_id: "source-broken-two".into(),
            }
        );
    }

    #[test]
    fn a_scoped_error_cannot_cover_a_scheduled_unit() {
        let (input, mut result) = fixture();
        result.errors.push(plan_error(
            ErrorCode::InvalidBounds,
            Some("scope-direct"),
            Some("direct-unit"),
            "Contradictory scheduled-unit error.",
        ));

        let error = canonical_partial_conversion_exclusions(&input, &result)
            .expect_err("scheduled errored units are unsafe");

        assert_eq!(
            error,
            ConversionPlanSliceError::ScopedPlanErrorCoversScheduledUnit {
                code: ErrorCode::InvalidBounds,
                source_unit_id: "source-direct".into(),
            }
        );
    }

    #[test]
    fn a_scope_error_excludes_all_units_or_fails_closed_on_a_scheduled_sibling() {
        let (mut input, mut result) = fixture();
        add_omitted_scope(
            &mut input,
            "scope-broken",
            &[
                ("broken-one", "source-broken-one", Some(7)),
                ("broken-two", "source-broken-two", Some(8)),
            ],
        );
        result.errors.push(plan_error(
            ErrorCode::RequestedDirectSpoolsUnavailable,
            Some("scope-broken"),
            None,
            "The requested loadout is unavailable.",
        ));

        let exclusions = canonical_partial_conversion_exclusions(&input, &result)
            .expect("the entire omitted scope is excludable");
        assert_eq!(
            exclusions
                .iter()
                .map(|unit| (
                    unit.source_unit_id.as_str(),
                    unit.source_plate_id,
                    unit.reason.as_str(),
                ))
                .collect::<Vec<_>>(),
            vec![
                (
                    "source-broken-one",
                    Some(7),
                    "The requested loadout is unavailable."
                ),
                (
                    "source-broken-two",
                    Some(8),
                    "The requested loadout is unavailable."
                ),
            ]
        );
        assert_ne!(exclusions[0].error_identity, exclusions[1].error_identity);

        let (input, mut result) = fixture();
        result.errors.push(plan_error(
            ErrorCode::RequestedDirectSpoolsUnavailable,
            Some("scope-direct"),
            None,
            "The scope is invalid.",
        ));
        assert_eq!(
            canonical_partial_conversion_exclusions(&input, &result)
                .expect_err("a scope error cannot coexist with a scheduled sibling"),
            ConversionPlanSliceError::ScopedPlanErrorCoversScheduledUnit {
                code: ErrorCode::RequestedDirectSpoolsUnavailable,
                source_unit_id: "source-direct".into(),
            }
        );
    }

    #[test]
    fn unknown_scoped_error_identity_is_rejected() {
        let (input, mut result) = broken_unit_fixture();
        result.errors[0].scope_id = Some("unknown-scope".into());

        let error = canonical_partial_conversion_exclusions(&input, &result)
            .expect_err("an error must bind to a canonical unit");

        let ConversionPlanSliceError::UnboundScopedPlanError { error_identity } = error else {
            panic!("unexpected error: {error:?}");
        };
        assert!(error_identity.starts_with("sha256:"));
        assert_eq!(error_identity.len(), "sha256:".len() + 64);
    }

    #[test]
    fn duplicate_unknown_and_scheduled_approval_entries_are_rejected() {
        let (input, result) = broken_unit_fixture();
        let exclusion = canonical_partial_conversion_exclusions(&input, &result)
            .expect("canonical exclusion")
            .remove(0);

        let duplicate = approval_for(vec![exclusion.clone(), exclusion.clone()]);
        assert_eq!(
            slice_all_conversion_targets_with_partial_approval(&input, &result, Some(&duplicate))
                .expect_err("duplicate approval entries are ambiguous"),
            ConversionPlanSliceError::DuplicateApprovedExclusion {
                source_unit_id: "source-broken".into(),
            }
        );

        let mut unknown_entry = exclusion.clone();
        unknown_entry.source_unit_id = "source-unknown".into();
        let unknown = approval_for(vec![exclusion.clone(), unknown_entry]);
        assert_eq!(
            slice_all_conversion_targets_with_partial_approval(&input, &result, Some(&unknown))
                .expect_err("unknown exclusions are not backend candidates"),
            ConversionPlanSliceError::UnknownApprovedExclusion {
                source_unit_id: "source-unknown".into(),
            }
        );

        let mut scheduled_entry = exclusion.clone();
        scheduled_entry.source_unit_id = "source-direct".into();
        let scheduled = approval_for(vec![exclusion, scheduled_entry]);
        assert_eq!(
            slice_all_conversion_targets_with_partial_approval(&input, &result, Some(&scheduled))
                .expect_err("scheduled units cannot be excluded"),
            ConversionPlanSliceError::ScheduledUnitExclusion {
                source_unit_id: "source-direct".into(),
            }
        );
    }

    #[test]
    fn every_backend_evidence_field_is_authoritative() {
        let (input, result) = broken_unit_fixture();
        let exclusion = canonical_partial_conversion_exclusions(&input, &result)
            .expect("canonical exclusion")
            .remove(0);
        let mut modified = Vec::new();

        let mut value = exclusion.clone();
        value.scope_id = "different-scope".into();
        modified.push(value);
        let mut value = exclusion.clone();
        value.planning_unit_id = "different-unit".into();
        modified.push(value);
        let mut value = exclusion.clone();
        value.source_plate_id = Some(99);
        modified.push(value);
        let mut value = exclusion.clone();
        value.reason = "Modified reason.".into();
        modified.push(value);
        let mut value = exclusion;
        value.error_identity = format!("sha256:{}", "0".repeat(64));
        modified.push(value);

        for stale in modified {
            let source_unit_id = stale.source_unit_id.clone();
            let approval = approval_for(vec![stale]);
            assert_eq!(
                slice_all_conversion_targets_with_partial_approval(
                    &input,
                    &result,
                    Some(&approval)
                )
                .expect_err("modified backend evidence must fail closed"),
                ConversionPlanSliceError::StaleApprovedExclusion { source_unit_id }
            );
        }
    }

    #[test]
    fn reordered_exact_approval_is_accepted_but_missing_entries_are_not() {
        let (mut input, mut result) = fixture();
        add_omitted_scope(
            &mut input,
            "scope-broken",
            &[
                ("broken-one", "source-broken-one", Some(7)),
                ("broken-two", "source-broken-two", Some(8)),
            ],
        );
        result.errors.push(plan_error(
            ErrorCode::RequestedDirectSpoolsUnavailable,
            Some("scope-broken"),
            None,
            "The scope cannot be planned.",
        ));
        let canonical = canonical_partial_conversion_exclusions(&input, &result)
            .expect("two canonical exclusions");

        let missing = approval_for(vec![canonical[1].clone()]);
        assert_eq!(
            slice_all_conversion_targets_with_partial_approval(&input, &result, Some(&missing))
                .expect_err("one implicit omission remains"),
            ConversionPlanSliceError::MissingApprovedExclusion {
                source_unit_id: "source-broken-one".into(),
            }
        );

        let mut reversed = canonical.clone();
        reversed.reverse();
        let partition = slice_all_conversion_targets_with_partial_approval(
            &input,
            &result,
            Some(&approval_for(reversed)),
        )
        .expect("approval order is not semantically significant");
        assert_eq!(partition.excluded_source_units, canonical);
    }

    #[test]
    fn evidence_is_stable_across_error_order_and_contains_all_reasons() {
        let (input, mut result) = broken_unit_fixture();
        result.errors.push(plan_error(
            ErrorCode::UnknownRequirement,
            Some("scope-broken"),
            Some("broken-unit"),
            "Requirement is missing.",
        ));
        let forward = canonical_partial_conversion_exclusions(&input, &result)
            .expect("canonical multi-error evidence");
        result.errors.reverse();
        let reverse = canonical_partial_conversion_exclusions(&input, &result)
            .expect("error order does not affect evidence");

        assert_eq!(forward, reverse);
        assert_eq!(
            forward[0].reason,
            "Requirement is missing. | Unit bounds are invalid."
        );
    }

    #[test]
    fn an_old_approval_is_stale_when_the_backend_error_changes() {
        let (input, mut result) = broken_unit_fixture();
        let old = canonical_partial_conversion_exclusions(&input, &result).expect("old evidence");
        result.errors[0].message = "Unit bounds changed after replanning.".into();

        let error = slice_all_conversion_targets_with_partial_approval(
            &input,
            &result,
            Some(&approval_for(old)),
        )
        .expect_err("an approval is bound to the exact backend error set");

        assert_eq!(
            error,
            ConversionPlanSliceError::StaleApprovedExclusion {
                source_unit_id: "source-broken".into(),
            }
        );
    }

    #[test]
    fn malformed_or_unknown_approval_json_is_rejected() {
        let (input, result) = broken_unit_fixture();
        let evidence =
            canonical_partial_conversion_exclusions(&input, &result).expect("canonical evidence");
        let approval = approval_for(evidence);
        let json = serde_json::to_string(&approval).expect("approval JSON");
        assert!(json.contains("excludedSourceUnits"));
        assert!(json.contains("planningUnitId"));
        assert!(json.contains("sourcePlateId"));
        assert!(serde_json::from_str::<PartialConversionApproval>(&json).is_ok());

        assert!(
            serde_json::from_str::<PartialConversionApproval>(
                r#"{"excludedSourceUnits":[],"unexpected":true}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<PartialConversionApproval>(
                r#"{"excludedSourceUnits":[{"scopeId":"s","planningUnitId":"u","sourcePlateId":1,"sourceUnitId":"source","reason":"reason","errorIdentity":"identity","unexpected":true}]}"#
            )
            .is_err()
        );
    }

    #[test]
    fn duplicate_canonical_source_identity_fails_before_approval() {
        let (mut input, mut result) = fixture();
        add_omitted_scope(
            &mut input,
            "scope-broken",
            &[("broken-unit", "source-direct", Some(3))],
        );
        result.errors.push(plan_error(
            ErrorCode::InvalidBounds,
            Some("scope-broken"),
            Some("broken-unit"),
            "Broken unit.",
        ));

        assert_eq!(
            canonical_partial_conversion_exclusions(&input, &result)
                .expect_err("source identity must remain globally unique"),
            ConversionPlanSliceError::DuplicateCanonicalSourceUnit {
                source_unit_id: "source-direct".into(),
            }
        );
    }

    #[test]
    fn duplicate_canonical_scope_and_planning_unit_identities_fail_closed() {
        let (mut input, result) = broken_unit_fixture();
        let duplicate_scope = input.scopes.last().expect("broken scope").clone();
        input.scopes.push(duplicate_scope);
        assert_eq!(
            canonical_partial_conversion_exclusions(&input, &result)
                .expect_err("duplicate scope IDs are structurally ambiguous"),
            ConversionPlanSliceError::EntityCoverageMismatch {
                entity: "input scope",
                id: "scope-broken".into(),
                count: 2,
            }
        );

        let (mut input, result) = broken_unit_fixture();
        let mut duplicate_unit = input.scopes.last().expect("broken scope").units[0].clone();
        duplicate_unit.source_unit_id = "source-broken-copy".into();
        input
            .scopes
            .last_mut()
            .expect("broken scope")
            .units
            .push(duplicate_unit);
        assert_eq!(
            canonical_partial_conversion_exclusions(&input, &result)
                .expect_err("duplicate scoped planning unit IDs are ambiguous"),
            ConversionPlanSliceError::EntityCoverageMismatch {
                entity: "input unit",
                id: "scope-broken/broken-unit".into(),
                count: 2,
            }
        );
    }

    #[test]
    fn partial_path_still_validates_the_surviving_batch_closure() {
        let (input, mut result) = broken_unit_fixture();
        let mut orphan = result.jobs[0].clone();
        orphan.id = "job-orphan".into();
        result.jobs.push(orphan);

        let error = canonical_partial_conversion_exclusions(&input, &result)
            .expect_err("partial approval cannot weaken structural validation");

        assert_eq!(
            error,
            ConversionPlanSliceError::EntityCoverageMismatch {
                entity: "batch reference for job",
                id: "job-orphan".into(),
                count: 0,
            }
        );
    }

    #[test]
    fn excluding_every_source_unit_does_not_create_an_empty_conversion() {
        let (input, mut result) = fixture();
        result.jobs.clear();
        result.plates.clear();
        result.batches.clear();
        result.errors.extend([
            plan_error(
                ErrorCode::InvalidBounds,
                Some("scope-direct"),
                Some("direct-unit"),
                "Direct unit is invalid.",
            ),
            plan_error(
                ErrorCode::InvalidBounds,
                Some("scope-a1"),
                Some("a1-unit"),
                "A1 unit is invalid.",
            ),
        ]);

        assert_eq!(
            canonical_partial_conversion_exclusions(&input, &result)
                .expect_err("there must be at least one valid target"),
            ConversionPlanSliceError::EmptyTarget
        );
    }

    #[test]
    fn nonempty_batches_with_no_scheduled_source_units_are_not_valid_jobs() {
        let (input, mut result) = fixture();
        for job in &mut result.jobs {
            job.units.clear();
        }
        for plate in &mut result.plates {
            plate.units.clear();
            plate.placements.clear();
        }
        result.errors.extend([
            plan_error(
                ErrorCode::InvalidBounds,
                Some("scope-direct"),
                Some("direct-unit"),
                "Direct unit is invalid.",
            ),
            plan_error(
                ErrorCode::InvalidBounds,
                Some("scope-a1"),
                Some("a1-unit"),
                "A1 unit is invalid.",
            ),
        ]);

        assert!(!result.batches.is_empty());
        assert_eq!(
            canonical_partial_conversion_exclusions(&input, &result)
                .expect_err("an empty job shell is not a valid conversion target"),
            ConversionPlanSliceError::EmptyTarget
        );
    }

    #[test]
    fn malformed_source_plate_identity_fails_closed_and_raw_identity_is_bound() {
        let (mut input, result) = broken_unit_fixture();
        input.scopes.last_mut().expect("broken scope").units[0].source_plate_id =
            Some("not-a-plate".into());
        assert_eq!(
            canonical_partial_conversion_exclusions(&input, &result)
                .expect_err("malformed source provenance cannot be discarded"),
            ConversionPlanSliceError::InvalidCanonicalSourcePlateId {
                source_unit_id: "source-broken".into(),
                source_plate_id: "not-a-plate".into(),
            }
        );

        let (mut input, result) = broken_unit_fixture();
        let prefixed = canonical_partial_conversion_exclusions(&input, &result)
            .expect("prefixed source plate identity");
        input.scopes.last_mut().expect("broken scope").units[0].source_plate_id = Some("42".into());
        let numeric = canonical_partial_conversion_exclusions(&input, &result)
            .expect("numeric source plate identity");
        assert_eq!(prefixed[0].source_plate_id, numeric[0].source_plate_id);
        assert_ne!(prefixed[0].error_identity, numeric[0].error_identity);
    }

    #[test]
    fn complete_plan_rejects_an_attempt_to_exclude_a_scheduled_unit() {
        let (input, result) = fixture();
        let approval = approval_for(vec![ExcludedSourceUnit {
            scope_id: "scope-direct".into(),
            planning_unit_id: "direct-unit".into(),
            source_plate_id: Some(1),
            source_unit_id: "source-direct".into(),
            reason: "Not a backend exclusion.".into(),
            error_identity: format!("sha256:{}", "0".repeat(64)),
        }]);

        assert_eq!(
            slice_all_conversion_targets_with_partial_approval(&input, &result, Some(&approval))
                .expect_err("full plans do not accept ad-hoc exclusions"),
            ConversionPlanSliceError::ScheduledUnitExclusion {
                source_unit_id: "source-direct".into(),
            }
        );
    }

    #[test]
    fn empty_backend_error_reason_is_not_approvable() {
        let (input, mut result) = broken_unit_fixture();
        result.errors[0].message = "   ".into();

        assert_eq!(
            canonical_partial_conversion_exclusions(&input, &result)
                .expect_err("manifest evidence requires a backend reason"),
            ConversionPlanSliceError::EmptyPlanErrorReason {
                code: ErrorCode::InvalidBounds,
            }
        );
    }
}
