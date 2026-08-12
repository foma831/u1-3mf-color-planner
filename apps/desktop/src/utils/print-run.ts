import type {
  BatchSegment,
  ConversionResult,
  PlatePlan,
  PreparedPhysicalSlot,
  ProjectPlan,
  PublishedConversionArtifact,
  T4Change,
} from "../types";

export const PRINT_RUN_STORAGE_PREFIX = "u1-planner:print-run:v2:";
export const PUBLISHED_PRINT_RUN_STORAGE_PREFIX =
  "u1-planner:published-print-run:v1:";
const LEGACY_PRINT_RUN_STORAGE_PREFIXES = ["u1-planner:print-run:v1:"];
export const UNKNOWN_T4_LABEL = "Unknown / current T4 spool";

export interface StructuredT4Change extends Partial<T4Change> {
  /** Compatibility aliases for older/demo view models. */
  fromLabel?: string | null;
  toLabel?: string | null;
  fromName?: string | null;
  toName?: string | null;
  from?: string | null;
  to?: string | null;
}

type BatchWithT4Change = BatchSegment & {
  t4Change?: StructuredT4Change | null;
};

export interface ResolvedT4Change {
  fromLabel: string;
  toLabel: string;
  fromSpoolId: string | null;
  toSpoolId: string | null;
  source: "structured" | "setup-actions";
}

export interface PrintRunPlateStep {
  id: string;
  kind: "plate";
  batchId: string | null;
  plate: PlatePlan;
  targetLabel: string;
}

export interface PrintRunFilamentStep {
  id: string;
  kind: "filament-change";
  batchId: string;
  beforePlateId: string;
  beforeTargetOrder: number;
  afterTargetOrder: number | null;
  fromLabel: string;
  toLabel: string;
  fromSpoolId: string | null;
  toSpoolId: string | null;
}

export interface PrintRunSetupStep {
  id: string;
  kind: "setup-checkpoint";
  batchId: string;
  phase: "before-batch" | "after-batch";
  printer: BatchSegment["printer"];
  actions: string[];
  beforePlateId: string | null;
  beforeTargetOrder: number | null;
  afterTargetOrder: number | null;
}

export interface PrintRunA1SpoolStep {
  id: string;
  kind: "a1-spool-checkpoint";
  batchId: string;
  printer: "A1 mini";
  spool: PreparedPhysicalSlot;
  actions: string[];
  beforePlateId: string;
  beforeTargetOrder: number;
  afterTargetOrder: number | null;
}

export type PrintRunCheckpointStep =
  PrintRunFilamentStep | PrintRunSetupStep | PrintRunA1SpoolStep;

export type PrintRunStep = PrintRunPlateStep | PrintRunCheckpointStep;

export interface PrintRunEvent {
  type: "run-started" | "plate-completed" | "checkpoint-confirmed";
  stepId: string | null;
  recordedAt: string;
  message: string;
}

export interface PrintRunProgress {
  version: 2;
  sourceHash: string;
  planFingerprint: string;
  status: "not-started" | "active" | "complete";
  currentStepIndex: number;
  completedPlateIds: string[];
  confirmedCheckpointIds: string[];
  events: PrintRunEvent[];
}

export type PrintRunPlan = Pick<
  ProjectPlan,
  | "summary"
  | "plates"
  | "batches"
  | "planReady"
  | "blockingErrors"
  | "omittedUnitCount"
  | "partialConversion"
  | "spools"
  | "plannedFinalLoadout"
  | "plannedFinalA1SpoolId"
>;

export interface PrintRunDescriptor {
  sourceHash: string;
  planFingerprint: string;
  storageKey: string;
  steps: PrintRunStep[];
}

/**
 * Binds a published native conversion result to the exact executable plan that
 * produced it. The backend fingerprint is retained for audit, while the local
 * execution fingerprint lets the UI reject stale files immediately after any
 * plan change.
 */
export interface PublishedPrintRunBundle {
  sourceHash: string;
  backendPlanFingerprint: string;
  executionPlanFingerprint: string;
  result: ConversionResult;
}

function nonemptyString(...values: unknown[]) {
  for (const value of values) {
    if (typeof value === "string" && value.trim()) return value.trim();
  }
  return null;
}

function displaySpool(name: string | null, id: string | null) {
  return name ?? id;
}

function resolveStructuredT4Change(
  batch: BatchWithT4Change,
): ResolvedT4Change | null {
  const change = batch.t4Change;
  if (!change) return null;

  const fromSpoolId = nonemptyString(change.fromSpoolId);
  const toSpoolId = nonemptyString(change.toSpoolId);
  const fromName = nonemptyString(
    change.fromSpoolName,
    change.fromLabel,
    change.fromName,
    change.from,
  );
  const toName = nonemptyString(
    change.toSpoolName,
    change.toLabel,
    change.toName,
    change.to,
  );
  let toLabel = displaySpool(toName, toSpoolId);
  if (!toLabel) return null;
  let fromLabel = displaySpool(fromName, fromSpoolId) ?? UNKNOWN_T4_LABEL;
  if (fromSpoolId && toSpoolId && fromSpoolId === toSpoolId) return null;
  if (!fromSpoolId && !toSpoolId && fromLabel === toLabel) return null;

  if (
    fromLabel === toLabel &&
    fromSpoolId &&
    toSpoolId &&
    fromSpoolId !== toSpoolId
  ) {
    fromLabel = `${fromLabel} (${fromSpoolId})`;
    toLabel = `${toLabel} (${toSpoolId})`;
  }

  return {
    fromLabel,
    toLabel,
    fromSpoolId,
    toSpoolId,
    source: "structured",
  };
}

interface ParsedSetupAction {
  phase: "before-batch" | "after-batch" | null;
  toolhead: string | null;
  kind: "keep" | "unload" | "load" | "restore" | null;
  payload: string | null;
}

function parseSetupAction(action: string): ParsedSetupAction {
  const match = action.match(
    /^\s*(Before|After)\s+batch\s*:\s*([^—–-]+?)\s*(?:—|–|-)\s*(keep|unload|load|restore)\b\s*(.*?)\s*$/i,
  );
  if (!match) {
    const phase = action.match(/^\s*(Before|After)\s+batch\s*:/i)?.[1];
    return {
      phase:
        phase?.toLowerCase() === "after"
          ? "after-batch"
          : phase?.toLowerCase() === "before"
            ? "before-batch"
            : null,
      toolhead: null,
      kind: null,
      payload: null,
    };
  }
  return {
    phase: match[1].toLowerCase() === "after" ? "after-batch" : "before-batch",
    toolhead: nonemptyString(match[2]),
    kind: match[3].toLowerCase() as ParsedSetupAction["kind"],
    payload: nonemptyString(match[4]),
  };
}

function nonKeepSetupActions(
  batch: BatchSegment,
  phase: "before-batch" | "after-batch",
) {
  return batch.setupActions.filter((action) => {
    const parsed = parseSetupAction(action);
    const actionPhase = parsed.phase ?? "before-batch";
    return actionPhase === phase && parsed.kind !== "keep";
  });
}

function isPureT4Boundary(actions: string[]) {
  return (
    actions.length > 0 &&
    actions.every((action) => {
      const parsed = parseSetupAction(action);
      return (
        parsed.phase === "before-batch" &&
        parsed.toolhead?.toUpperCase() === "T4" &&
        (parsed.kind === "unload" || parsed.kind === "load")
      );
    })
  );
}

function resolveSetupActionT4Change(
  batch: BatchSegment,
): ResolvedT4Change | null {
  const fromLabel = batch.setupActions
    .map((action) => {
      const parsed = parseSetupAction(action);
      return parsed.phase === "before-batch" &&
        parsed.toolhead?.toUpperCase() === "T4" &&
        parsed.kind === "unload"
        ? parsed.payload
        : null;
    })
    .find((value): value is string => value !== null);
  const toLabel = batch.setupActions
    .map((action) => {
      const parsed = parseSetupAction(action);
      return parsed.phase === "before-batch" &&
        parsed.toolhead?.toUpperCase() === "T4" &&
        parsed.kind === "load"
        ? parsed.payload
        : null;
    })
    .find((value): value is string => value !== null);

  if (!fromLabel || !toLabel || fromLabel === toLabel) return null;
  return {
    fromLabel,
    toLabel,
    fromSpoolId: null,
    toSpoolId: null,
    source: "setup-actions",
  };
}

export function resolveBatchT4Change(
  batch: BatchSegment,
): ResolvedT4Change | null {
  return (
    resolveStructuredT4Change(batch as BatchWithT4Change) ??
    resolveSetupActionT4Change(batch)
  );
}

export function targetPlateLabel(order: number) {
  return `Target Plate ${String(order).padStart(2, "0")}`;
}

function containingBatch(plate: PlatePlan, batches: BatchSegment[]) {
  return (
    batches.find(
      (batch) =>
        batch.printer === plate.printer &&
        plate.order >= batch.startOrder &&
        plate.order <= batch.endOrder,
    ) ?? null
  );
}

export function createPrintRunSteps(
  plan: PrintRunPlan,
  bundle: PublishedPrintRunBundle | null = null,
): PrintRunStep[] {
  const plates = [...plan.plates].sort(
    (left, right) =>
      left.order - right.order || left.id.localeCompare(right.id),
  );
  const batches = [...plan.batches].sort(
    (left, right) =>
      left.startOrder - right.startOrder || left.id.localeCompare(right.id),
  );
  const batchByFirstPlate = new Map<string, BatchSegment>();
  const batchByLastPlate = new Map<string, BatchSegment>();
  const artifactsByPlate = publishedArtifactsForPlan(plan, bundle);
  for (const batch of batches) {
    batchByFirstPlate.set(`${batch.printer}:${batch.startOrder}`, batch);
    batchByLastPlate.set(`${batch.printer}:${batch.endOrder}`, batch);
  }

  const steps: PrintRunStep[] = [];
  let previousU1TargetOrder: number | null = null;
  for (const [plateIndex, plate] of plates.entries()) {
    const batch =
      batchByFirstPlate.get(`${plate.printer}:${plate.order}`) ??
      containingBatch(plate, batches);
    const startsBatch = batch?.startOrder === plate.order;
    const t4Change = startsBatch && batch ? resolveBatchT4Change(batch) : null;
    const beforeActions =
      startsBatch && batch ? nonKeepSetupActions(batch, "before-batch") : [];

    if (batch && startsBatch && batch.printer === "A1 mini") {
      const artifact = artifactsByPlate.get(plate.id);
      const spool = artifact ? exactA1SpoolForArtifact(artifact) : null;
      if (spool) {
        steps.push({
          id: [
            "a1-spool",
            batch.id,
            plate.id,
            spool.spoolId,
            spool.settingId,
            spool.filamentId,
          ].join(":"),
          kind: "a1-spool-checkpoint",
          batchId: batch.id,
          printer: "A1 mini",
          spool,
          actions: beforeActions,
          beforePlateId: plate.id,
          beforeTargetOrder: plate.order,
          afterTargetOrder: plates[plateIndex - 1]?.order ?? null,
        });
      }
    }

    if (
      batch &&
      t4Change &&
      batch.printer === "U1" &&
      (beforeActions.length === 0 || isPureT4Boundary(beforeActions))
    ) {
      steps.push({
        id: [
          "t4",
          batch.id,
          plate.id,
          t4Change.fromSpoolId ?? t4Change.fromLabel,
          t4Change.toSpoolId ?? t4Change.toLabel,
        ].join(":"),
        kind: "filament-change",
        batchId: batch.id,
        beforePlateId: plate.id,
        beforeTargetOrder: plate.order,
        afterTargetOrder: previousU1TargetOrder,
        fromLabel: t4Change.fromLabel,
        toLabel: t4Change.toLabel,
        fromSpoolId: t4Change.fromSpoolId,
        toSpoolId: t4Change.toSpoolId,
      });
    } else if (
      batch &&
      batch.printer !== "A1 mini" &&
      beforeActions.length > 0
    ) {
      steps.push({
        id: `setup:before:${batch.id}`,
        kind: "setup-checkpoint",
        batchId: batch.id,
        phase: "before-batch",
        printer: batch.printer,
        actions: beforeActions,
        beforePlateId: plate.id,
        beforeTargetOrder: plate.order,
        afterTargetOrder: plates[plateIndex - 1]?.order ?? null,
      });
    }

    steps.push({
      id: `plate:${plate.id}`,
      kind: "plate",
      batchId: batch?.id ?? null,
      plate,
      targetLabel: targetPlateLabel(plate.order),
    });
    if (plate.printer === "U1") previousU1TargetOrder = plate.order;

    const endingBatch =
      batchByLastPlate.get(`${plate.printer}:${plate.order}`) ?? batch;
    const endsBatch = endingBatch?.endOrder === plate.order;
    const afterActions =
      endsBatch && endingBatch
        ? nonKeepSetupActions(endingBatch, "after-batch")
        : [];
    if (endingBatch && afterActions.length > 0) {
      const nextPlate = plates[plateIndex + 1] ?? null;
      steps.push({
        id: `setup:after:${endingBatch.id}`,
        kind: "setup-checkpoint",
        batchId: endingBatch.id,
        phase: "after-batch",
        printer: endingBatch.printer,
        actions: afterActions,
        beforePlateId: nextPlate?.id ?? null,
        beforeTargetOrder: nextPlate?.order ?? null,
        afterTargetOrder: plate.order,
      });
    }
  }
  return steps;
}

function publicationFingerprintPayload(bundle: PublishedPrintRunBundle | null) {
  if (!bundle) return null;
  return {
    sourceHash: bundle.sourceHash,
    backendPlanFingerprint: bundle.backendPlanFingerprint,
    executionPlanFingerprint: bundle.executionPlanFingerprint,
    outputDirectory: bundle.result.outputDirectory,
    manifestPath: bundle.result.manifestPath,
    reportPath: bundle.result.reportPath,
    excludedSourceUnits: bundle.result.excludedSourceUnits,
    warningsAcknowledged: bundle.result.warningsAcknowledged,
    warnings: bundle.result.warnings,
    artifacts: [...bundle.result.artifacts]
      .sort(
        (left, right) =>
          left.path.localeCompare(right.path) ||
          left.adapterId.localeCompare(right.adapterId),
      )
      .map((artifact) => ({
        adapterId: artifact.adapterId,
        target: artifact.target,
        printer: artifact.printer,
        slicer: artifact.slicer,
        batchId: artifact.batchId,
        fileName: artifact.fileName,
        path: artifact.path,
        relativePath: artifact.relativePath,
        byteSize: artifact.byteSize,
        sha256: artifact.sha256,
        validationStatus: artifact.validationStatus,
        targetPlateIds: [...artifact.targetPlateIds].sort(),
        sourceUnitIds: [...artifact.sourceUnitIds].sort(),
      })),
  };
}

function fingerprintPayload(
  plan: PrintRunPlan,
  bundle: PublishedPrintRunBundle | null = null,
) {
  return {
    sourceHash: plan.summary.sourceHash,
    planReady: plan.planReady,
    blockingErrors: plan.blockingErrors,
    omittedUnitCount: plan.omittedUnitCount,
    partialConversion: plan.partialConversion,
    plates: [...plan.plates]
      .sort(
        (left, right) =>
          left.order - right.order || left.id.localeCompare(right.id),
      )
      .map((plate) => ({
        id: plate.id,
        scopeId: plate.scopeId,
        sourceUnitIds: [...plate.sourceUnitIds].sort(),
        order: plate.order,
        printer: plate.printer,
        title: plate.title,
        source: plate.source,
        strategy: plate.strategy,
        objectCount: plate.objectCount,
        loadoutLabel: plate.loadoutLabel,
        loadoutColors: plate.loadoutColors,
        logicalColorCount: plate.logicalColorCount,
        effectivePairCount: plate.effectivePairCount ?? null,
        sourceColors:
          plate.sourceColors?.map((color) => ({
            sourceSlots: [...color.sourceSlots].sort(),
            sourceMaterial: color.sourceMaterial,
            sourceHex: color.sourceHex,
          })) ?? null,
        recipeSummary: plate.recipeSummary,
        colorQuality: plate.colorQuality,
        estimatedDeltaE00: plate.estimatedDeltaE00,
        material: plate.material,
        toolChanges: plate.toolChanges,
        t4Action: plate.t4Action,
        warnings: plate.warnings,
        isFastMono: plate.isFastMono,
        directEligible: plate.directEligible,
        directEligibilityReason: plate.directEligibilityReason ?? null,
        mappings:
          plate.mappings?.map((mapping) => ({
            id: mapping.id,
            sourceSlot: mapping.sourceSlot,
            sourceName: mapping.sourceName,
            sourceHex: mapping.sourceHex,
            sourceMaterial: mapping.sourceMaterial,
            cmyRecipe: mapping.cmyRecipe,
            cmyPredictedHex: mapping.cmyPredictedHex,
            cmyDeltaE00: mapping.cmyDeltaE00,
            cmyConfidence: mapping.cmyConfidence,
            directToolhead: mapping.directToolhead,
            selectedSpoolId: mapping.selectedSpoolId,
            materialSubstitutionAcknowledged:
              mapping.materialSubstitutionAcknowledged ?? false,
          })) ?? null,
      })),
    batches: [...plan.batches]
      .sort(
        (left, right) =>
          left.startOrder - right.startOrder || left.id.localeCompare(right.id),
      )
      .map((batch) => ({
        id: batch.id,
        label: batch.label,
        detail: batch.detail,
        strategy: batch.strategy,
        startOrder: batch.startOrder,
        endOrder: batch.endOrder,
        plateCount: batch.plateCount,
        printer: batch.printer,
        setupActions: batch.setupActions,
        t4Change: resolveBatchT4Change(batch),
      })),
    publication: publicationFingerprintPayload(bundle),
  };
}

function fnv1a64(value: string) {
  let hash = 0xcbf29ce484222325n;
  const prime = 0x100000001b3n;
  const mask = 0xffffffffffffffffn;
  for (let index = 0; index < value.length; index += 1) {
    hash ^= BigInt(value.charCodeAt(index));
    hash = (hash * prime) & mask;
  }
  return hash.toString(16).padStart(16, "0");
}

export function createPrintRunFingerprint(
  plan: PrintRunPlan,
  bundle: PublishedPrintRunBundle | null = null,
) {
  return `v2-${fnv1a64(JSON.stringify(fingerprintPayload(plan, bundle)))}`;
}

function sourceStoragePrefix(sourceHash: string) {
  return `${PRINT_RUN_STORAGE_PREFIX}${encodeURIComponent(sourceHash)}:`;
}

function publishedBundleStoragePrefix(sourceHash: string) {
  return `${PUBLISHED_PRINT_RUN_STORAGE_PREFIX}${encodeURIComponent(sourceHash)}:`;
}

export function createPrintRunStorageKey(
  sourceHash: string,
  planFingerprint: string,
) {
  return `${sourceStoragePrefix(sourceHash)}${planFingerprint}`;
}

export function createPublishedPrintRunStorageKey(plan: PrintRunPlan) {
  return `${publishedBundleStoragePrefix(plan.summary.sourceHash)}${createPrintRunFingerprint(plan)}`;
}

export function createPrintRunDescriptor(
  plan: PrintRunPlan,
  bundle: PublishedPrintRunBundle | null = null,
): PrintRunDescriptor {
  const planFingerprint = createPrintRunFingerprint(plan, bundle);
  return {
    sourceHash: plan.summary.sourceHash,
    planFingerprint,
    storageKey: createPrintRunStorageKey(
      plan.summary.sourceHash,
      planFingerprint,
    ),
    steps: createPrintRunSteps(plan, bundle),
  };
}

function exactA1SpoolForArtifact(
  artifact: PublishedConversionArtifact,
): PreparedPhysicalSlot | null {
  if (artifact.loadout.length !== 1) return null;
  const spool = artifact.loadout[0];
  if (
    !spool ||
    !spool.toolhead.trim() ||
    !spool.spoolId?.trim() ||
    !spool.spoolName?.trim() ||
    !spool.material?.trim() ||
    !spool.profile.trim() ||
    !spool.settingId.trim() ||
    !spool.filamentId.trim()
  ) {
    return null;
  }
  return spool;
}

function sameA1SpoolIdentity(
  left: PreparedPhysicalSlot,
  right: PreparedPhysicalSlot,
) {
  return (
    left.toolhead === right.toolhead &&
    left.spoolId === right.spoolId &&
    left.spoolName === right.spoolName &&
    left.material === right.material &&
    left.color === right.color &&
    left.profile === right.profile &&
    left.settingId === right.settingId &&
    left.filamentId === right.filamentId
  );
}

function sameStringSet(left: readonly string[], right: readonly string[]) {
  if (
    left.length !== right.length ||
    new Set(left).size !== left.length ||
    new Set(right).size !== right.length
  ) {
    return false;
  }
  const sortedLeft = [...left].sort();
  const sortedRight = [...right].sort();
  return sortedLeft.every((value, index) => value === sortedRight[index]);
}

function sameCanonicalExclusions(
  left: ProjectPlan["partialConversion"]["exclusions"],
  right: ProjectPlan["partialConversion"]["exclusions"],
) {
  return (
    left.length === right.length &&
    left.every((expected, index) => {
      const actual = right[index];
      return (
        actual !== undefined &&
        expected.scopeId === actual.scopeId &&
        expected.planningUnitId === actual.planningUnitId &&
        expected.sourcePlateId === actual.sourcePlateId &&
        expected.sourceUnitId === actual.sourceUnitId &&
        expected.reason === actual.reason &&
        expected.errorIdentity === actual.errorIdentity
      );
    })
  );
}

function publicationExclusionsMatchPlan(
  plan: PrintRunPlan,
  bundle: PublishedPrintRunBundle,
) {
  if (
    bundle.sourceHash !== plan.summary.sourceHash ||
    bundle.executionPlanFingerprint !== createPrintRunFingerprint(plan)
  ) {
    return false;
  }

  const expected = plan.partialConversion.exclusions;
  const actual = bundle.result.excludedSourceUnits;
  if (plan.planReady) {
    return (
      plan.blockingErrors.length === 0 &&
      plan.omittedUnitCount === 0 &&
      expected.length === 0 &&
      actual.length === 0
    );
  }
  if (
    !plan.partialConversion.available ||
    plan.blockingErrors.length === 0 ||
    expected.length === 0 ||
    plan.omittedUnitCount !== expected.length ||
    !sameCanonicalExclusions(expected, actual)
  ) {
    return false;
  }

  const expectedSourceUnitIds = expected.map(
    (exclusion) => exclusion.sourceUnitId,
  );
  const expectedErrorIdentities = expected.map(
    (exclusion) => exclusion.errorIdentity,
  );
  if (
    new Set(expectedSourceUnitIds).size !== expectedSourceUnitIds.length ||
    new Set(expectedErrorIdentities).size !== expectedErrorIdentities.length
  ) {
    return false;
  }
  const scheduledSourceUnitIds = new Set(
    plan.plates.flatMap((plate) => plate.sourceUnitIds),
  );
  return expectedSourceUnitIds.every(
    (sourceUnitId) => !scheduledSourceUnitIds.has(sourceUnitId),
  );
}

export function publishedArtifactsForPlan(
  plan: PrintRunPlan,
  bundle: PublishedPrintRunBundle | null,
): Map<string, PublishedConversionArtifact> {
  const artifactsByPlate = new Map<string, PublishedConversionArtifact>();
  if (
    !bundle ||
    bundle.sourceHash !== plan.summary.sourceHash ||
    !bundle.backendPlanFingerprint.trim() ||
    bundle.executionPlanFingerprint !== createPrintRunFingerprint(plan) ||
    !bundle.result.outputDirectory.trim() ||
    !bundle.result.manifestPath.trim() ||
    bundle.result.artifacts.length === 0
  ) {
    return artifactsByPlate;
  }

  if (!publicationExclusionsMatchPlan(plan, bundle)) {
    return artifactsByPlate;
  }

  const plannedPlateIds = new Set(plan.plates.map((plate) => plate.id));
  const plannedSourceUnitIds = plan.plates.flatMap(
    (plate) => plate.sourceUnitIds,
  );
  if (new Set(plannedSourceUnitIds).size !== plannedSourceUnitIds.length) {
    return artifactsByPlate;
  }
  const publishedSourceUnitIds = new Set<string>();
  for (const artifact of bundle.result.artifacts) {
    if (
      artifact.validationStatus.toLowerCase() !== "passed" ||
      !artifact.fileName.trim() ||
      !artifact.path.trim() ||
      !artifact.sha256.trim() ||
      artifact.targetPlateIds.length === 0
    ) {
      return new Map();
    }
    const expectedArtifactSourceUnitIds = artifact.targetPlateIds.flatMap(
      (plateId) =>
        plan.plates.find((plate) => plate.id === plateId)?.sourceUnitIds ?? [],
    );
    if (
      !sameStringSet(artifact.sourceUnitIds, expectedArtifactSourceUnitIds) ||
      artifact.sourceUnitIds.some((sourceUnitId) =>
        publishedSourceUnitIds.has(sourceUnitId),
      )
    ) {
      return new Map();
    }
    for (const sourceUnitId of artifact.sourceUnitIds) {
      publishedSourceUnitIds.add(sourceUnitId);
    }
    for (const plateId of artifact.targetPlateIds) {
      const plannedPlate = plan.plates.find((plate) => plate.id === plateId);
      if (
        !plannedPlateIds.has(plateId) ||
        !plannedPlate ||
        artifactsByPlate.has(plateId) ||
        (plannedPlate.printer === "A1 mini" &&
          (!artifact.target.toLowerCase().includes("a1") ||
            exactA1SpoolForArtifact(artifact) === null))
      ) {
        return new Map();
      }
      artifactsByPlate.set(plateId, artifact);
    }
  }

  if (
    artifactsByPlate.size !== plan.plates.length ||
    plan.plates.some((plate) => !artifactsByPlate.has(plate.id)) ||
    !sameStringSet([...publishedSourceUnitIds], plannedSourceUnitIds)
  ) {
    return new Map();
  }

  for (const batch of plan.batches.filter(
    (candidate) => candidate.printer === "A1 mini",
  )) {
    const batchPlates = plan.plates.filter(
      (plate) =>
        plate.printer === "A1 mini" &&
        plate.order >= batch.startOrder &&
        plate.order <= batch.endOrder,
    );
    if (batchPlates.length === 0) return new Map();
    const firstArtifact = artifactsByPlate.get(batchPlates[0].id);
    const expectedSpool = firstArtifact
      ? exactA1SpoolForArtifact(firstArtifact)
      : null;
    if (
      !expectedSpool ||
      batchPlates.some((plate) => {
        const artifact = artifactsByPlate.get(plate.id);
        const spool = artifact ? exactA1SpoolForArtifact(artifact) : null;
        return !spool || !sameA1SpoolIdentity(expectedSpool, spool);
      })
    ) {
      return new Map();
    }
  }
  return artifactsByPlate;
}

export function isPublishedPrintRunBundleReady(
  plan: PrintRunPlan,
  bundle: PublishedPrintRunBundle | null,
) {
  return (
    plan.plates.length > 0 &&
    publishedArtifactsForPlan(plan, bundle).size === plan.plates.length
  );
}

export function createEmptyPrintRunProgress(
  descriptor: PrintRunDescriptor,
): PrintRunProgress {
  return {
    version: 2,
    sourceHash: descriptor.sourceHash,
    planFingerprint: descriptor.planFingerprint,
    status: "not-started",
    currentStepIndex: 0,
    completedPlateIds: [],
    confirmedCheckpointIds: [],
    events: [],
  };
}

function isStringArray(value: unknown): value is string[] {
  return (
    Array.isArray(value) && value.every((item) => typeof item === "string")
  );
}

function isPrintRunEvent(value: unknown): value is PrintRunEvent {
  if (!value || typeof value !== "object") return false;
  const event = value as Partial<PrintRunEvent>;
  return (
    (event.type === "run-started" ||
      event.type === "plate-completed" ||
      event.type === "checkpoint-confirmed") &&
    (event.stepId === null || typeof event.stepId === "string") &&
    typeof event.recordedAt === "string" &&
    typeof event.message === "string"
  );
}

function isValidProgress(
  value: unknown,
  descriptor: PrintRunDescriptor,
): value is PrintRunProgress {
  if (!value || typeof value !== "object") return false;
  const progress = value as Partial<PrintRunProgress>;
  const plateStepIds = new Set(
    descriptor.steps
      .filter((step): step is PrintRunPlateStep => step.kind === "plate")
      .map((step) => step.plate.id),
  );
  const checkpointIds = new Set(
    descriptor.steps
      .filter((step): step is PrintRunCheckpointStep => step.kind !== "plate")
      .map((step) => step.id),
  );
  const validIndex =
    Number.isInteger(progress.currentStepIndex) &&
    (progress.currentStepIndex ?? -1) >= 0 &&
    (progress.currentStepIndex ?? descriptor.steps.length + 1) <=
      descriptor.steps.length;

  if (!validIndex) return false;
  const currentStepIndex = progress.currentStepIndex as number;
  const expectedCompletedPlateIds = descriptor.steps
    .slice(0, currentStepIndex)
    .filter((step): step is PrintRunPlateStep => step.kind === "plate")
    .map((step) => step.plate.id);
  const expectedConfirmedCheckpointIds = descriptor.steps
    .slice(0, currentStepIndex)
    .filter((step): step is PrintRunCheckpointStep => step.kind !== "plate")
    .map((step) => step.id);
  const hasExactIds = (actual: string[], expected: string[]) =>
    actual.length === expected.length &&
    new Set(actual).size === actual.length &&
    expected.every((id) => actual.includes(id));

  return (
    progress.version === 2 &&
    progress.sourceHash === descriptor.sourceHash &&
    progress.planFingerprint === descriptor.planFingerprint &&
    (progress.status === "not-started" ||
      progress.status === "active" ||
      progress.status === "complete") &&
    isStringArray(progress.completedPlateIds) &&
    progress.completedPlateIds.every((id) => plateStepIds.has(id)) &&
    isStringArray(progress.confirmedCheckpointIds) &&
    progress.confirmedCheckpointIds.every((id) => checkpointIds.has(id)) &&
    Array.isArray(progress.events) &&
    progress.events.every(isPrintRunEvent) &&
    hasExactIds(progress.completedPlateIds, expectedCompletedPlateIds) &&
    hasExactIds(
      progress.confirmedCheckpointIds,
      expectedConfirmedCheckpointIds,
    ) &&
    (progress.status !== "not-started" || currentStepIndex === 0) &&
    (progress.status !== "active" ||
      currentStepIndex < descriptor.steps.length) &&
    (progress.status !== "complete" ||
      currentStepIndex === descriptor.steps.length)
  );
}

function removeStaleProgress(
  storage: Storage,
  sourceHash: string,
  currentStorageKey: string,
) {
  const prefix = sourceStoragePrefix(sourceHash);
  const staleKeys: string[] = [];
  for (let index = 0; index < storage.length; index += 1) {
    const key = storage.key(index);
    if (key?.startsWith(prefix) && key !== currentStorageKey) {
      staleKeys.push(key);
    }
  }
  for (const key of staleKeys) storage.removeItem(key);
}

function removeLegacyProgress(storage: Storage, sourceHash: string) {
  const prefixes = LEGACY_PRINT_RUN_STORAGE_PREFIXES.map(
    (prefix) => `${prefix}${encodeURIComponent(sourceHash)}:`,
  );
  const legacyKeys: string[] = [];
  for (let index = 0; index < storage.length; index += 1) {
    const key = storage.key(index);
    if (key && prefixes.some((prefix) => key.startsWith(prefix))) {
      legacyKeys.push(key);
    }
  }
  for (const key of legacyKeys) storage.removeItem(key);
}

export function loadPrintRunProgress(
  storage: Storage | null,
  descriptor: PrintRunDescriptor,
) {
  const empty = createEmptyPrintRunProgress(descriptor);
  if (!storage) return empty;
  try {
    removeLegacyProgress(storage, descriptor.sourceHash);
    removeStaleProgress(storage, descriptor.sourceHash, descriptor.storageKey);
    const serialized = storage.getItem(descriptor.storageKey);
    if (!serialized) return empty;
    const parsed: unknown = JSON.parse(serialized);
    if (!isValidProgress(parsed, descriptor)) {
      storage.removeItem(descriptor.storageKey);
      return empty;
    }
    return parsed;
  } catch {
    try {
      storage.removeItem(descriptor.storageKey);
    } catch {
      // Storage can be unavailable in private/restricted browser contexts.
    }
    return empty;
  }
}

export function savePrintRunProgress(
  storage: Storage | null,
  descriptor: PrintRunDescriptor,
  progress: PrintRunProgress,
) {
  if (!storage) return false;
  try {
    storage.setItem(descriptor.storageKey, JSON.stringify(progress));
    return true;
  } catch {
    return false;
  }
}

export function clearPrintRunProgress(
  storage: Storage | null,
  descriptor: PrintRunDescriptor,
) {
  if (!storage) return false;
  try {
    storage.removeItem(descriptor.storageKey);
    return true;
  } catch {
    return false;
  }
}

function looksLikePersistedPublishedBundle(
  value: unknown,
): value is PublishedPrintRunBundle {
  if (!value || typeof value !== "object") return false;
  const bundle = value as Partial<PublishedPrintRunBundle>;
  if (
    typeof bundle.sourceHash !== "string" ||
    typeof bundle.backendPlanFingerprint !== "string" ||
    typeof bundle.executionPlanFingerprint !== "string" ||
    !bundle.result ||
    typeof bundle.result !== "object"
  ) {
    return false;
  }
  const result = bundle.result as Partial<ConversionResult>;
  return (
    typeof result.adapterId === "string" &&
    typeof result.outputDirectory === "string" &&
    typeof result.manifestPath === "string" &&
    typeof result.reportPath === "string" &&
    Array.isArray(result.artifacts) &&
    Array.isArray(result.warnings) &&
    Array.isArray(result.excludedSourceUnits) &&
    typeof result.warningsAcknowledged === "boolean"
  );
}

/**
 * Stores the exact publication receipt for restart recovery. The serialized
 * value is untrusted on the next launch and must never be supplied directly to
 * PrintRun. A native backend revalidation must reconstruct the authoritative
 * ConversionResult first.
 */
export function savePublishedPrintRunBundle(
  storage: Storage | null,
  plan: PrintRunPlan,
  bundle: PublishedPrintRunBundle,
) {
  if (
    !storage ||
    bundle.sourceHash !== plan.summary.sourceHash ||
    bundle.executionPlanFingerprint !== createPrintRunFingerprint(plan) ||
    !isPublishedPrintRunBundleReady(plan, bundle)
  ) {
    return false;
  }
  try {
    storage.setItem(
      createPublishedPrintRunStorageKey(plan),
      JSON.stringify(bundle),
    );
    return true;
  } catch {
    return false;
  }
}

/**
 * Loads only syntactically and plan-bound receipt data. The return value is
 * still untrusted: callers may use its outputDirectory as a recovery hint, but
 * must not expose its artifacts or enable Print Run before backend validation.
 */
export function loadUntrustedPublishedPrintRunBundle(
  storage: Storage | null,
  plan: PrintRunPlan,
): PublishedPrintRunBundle | null {
  if (!storage) return null;
  try {
    const serialized = storage.getItem(createPublishedPrintRunStorageKey(plan));
    if (!serialized) return null;
    const parsed: unknown = JSON.parse(serialized);
    if (
      !looksLikePersistedPublishedBundle(parsed) ||
      parsed.sourceHash !== plan.summary.sourceHash ||
      parsed.executionPlanFingerprint !== createPrintRunFingerprint(plan) ||
      !parsed.backendPlanFingerprint.trim() ||
      !parsed.result.outputDirectory.trim()
    ) {
      return null;
    }
    return parsed;
  } catch {
    return null;
  }
}

export function isPrintRunPlanExecutable(
  plan: PrintRunPlan,
  bundle: PublishedPrintRunBundle | null = null,
) {
  if (plan.plates.length === 0) return false;
  if (plan.planReady) {
    return (
      plan.blockingErrors.length === 0 &&
      plan.omittedUnitCount === 0 &&
      plan.partialConversion.exclusions.length === 0 &&
      (!bundle || bundle.result.excludedSourceUnits.length === 0)
    );
  }
  return bundle !== null && publicationExclusionsMatchPlan(plan, bundle);
}
