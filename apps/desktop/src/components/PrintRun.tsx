import { useEffect, useMemo, useRef, useState, type Ref } from "react";
import {
  ArrowRight,
  CheckCircle2,
  Circle,
  ExternalLink,
  LockKeyhole,
  LoaderCircle,
  Play,
  Printer,
  RefreshCw,
  RotateCcw,
  TriangleAlert,
} from "lucide-react";

import type {
  ExcludedSourceUnit,
  PlatePlan,
  PublishedConversionArtifact,
} from "../types";
import {
  clearPrintRunProgress,
  createEmptyPrintRunProgress,
  createPrintRunDescriptor,
  isPublishedPrintRunBundleReady,
  isPrintRunPlanExecutable,
  loadPrintRunProgress,
  publishedArtifactsForPlan,
  savePrintRunProgress,
  targetPlateLabel,
  UNKNOWN_T4_LABEL,
  type PrintRunDescriptor,
  type PrintRunA1SpoolStep,
  type PrintRunEvent,
  type PrintRunCheckpointStep,
  type PrintRunFilamentStep,
  type PrintRunPlan,
  type PrintRunProgress,
  type PrintRunSetupStep,
  type PrintRunStep,
  type PublishedPrintRunBundle,
} from "../utils/print-run";
import "./PrintRun.css";

export interface PrintRunProps {
  plan: PrintRunPlan;
  /** False while any plan choice is pending backend validation. */
  isPlanValidated: boolean;
  /** The validated native files published for this exact plan. */
  publishedBundle?: PublishedPrintRunBundle | null;
  /** Defaults to window.localStorage. Pass null for an in-memory-only run. */
  storage?: Storage | null;
  /** Lets the host keep its plate inspector synchronized with this workflow. */
  onCurrentTargetChange?: (plateId: string) => void;
  onProgressChange?: (progress: PrintRunProgress) => void;
  /** Opens only a backend-registered artifact from the validated bundle. */
  onOpenArtifact?: (artifact: PublishedConversionArtifact) => Promise<void>;
  /** Injectable clock for deterministic tests. */
  now?: () => string;
}

interface KeyedProgress {
  storageKey: string;
  progress: PrintRunProgress;
}

function defaultStorage() {
  if (typeof window === "undefined") return null;
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

function formatTimestamp(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date);
}

function stepState(
  step: PrintRunStep,
  index: number,
  progress: PrintRunProgress,
) {
  if (progress.status === "complete" || index < progress.currentStepIndex) {
    return "complete" as const;
  }
  if (index === progress.currentStepIndex) {
    return progress.status === "not-started"
      ? ("ready" as const)
      : ("current" as const);
  }
  if (
    step.kind === "plate" &&
    progress.completedPlateIds.includes(step.plate.id)
  ) {
    return "complete" as const;
  }
  if (
    step.kind !== "plate" &&
    progress.confirmedCheckpointIds.includes(step.id)
  ) {
    return "complete" as const;
  }
  return "waiting" as const;
}

function stateLabel(state: ReturnType<typeof stepState>) {
  switch (state) {
    case "complete":
      return "Complete";
    case "current":
      return "Current action";
    case "ready":
      return "Ready to start";
    case "waiting":
      return "Waiting";
  }
}

function StepIcon({ state }: { state: ReturnType<typeof stepState> }) {
  if (state === "complete") return <CheckCircle2 aria-hidden="true" />;
  if (state === "waiting") return <LockKeyhole aria-hidden="true" />;
  return <Circle aria-hidden="true" />;
}

function checkpointTitle(step: PrintRunCheckpointStep) {
  if (step.kind === "filament-change") {
    return `T4 change before ${targetPlateLabel(step.beforeTargetOrder)}`;
  }
  if (step.kind === "a1-spool-checkpoint") {
    return `A1 spool setup before ${targetPlateLabel(step.beforeTargetOrder)}`;
  }
  if (step.phase === "before-batch" && step.beforeTargetOrder !== null) {
    return `Setup before ${targetPlateLabel(step.beforeTargetOrder)}`;
  }
  if (step.afterTargetOrder !== null) {
    return `Restore after ${targetPlateLabel(step.afterTargetOrder)}`;
  }
  return "Printer setup checkpoint";
}

function checkpointSummary(step: PrintRunCheckpointStep) {
  if (step.kind === "filament-change") {
    return `${step.fromLabel} → ${step.toLabel}`;
  }
  if (step.kind === "a1-spool-checkpoint") {
    return `${step.spool.spoolName} · ${step.spool.material}`;
  }
  const actionLabel = step.actions.length === 1 ? "action" : "actions";
  return `${step.printer} · ${step.actions.length} required ${actionLabel}`;
}

function nextStepMessage(step: PrintRunStep | undefined) {
  if (!step) return "The print run is complete.";
  if (step.kind === "plate") {
    return `${step.targetLabel} is now the current target.`;
  }
  if (step.kind === "filament-change") {
    return `Filament change required before ${targetPlateLabel(step.beforeTargetOrder)}.`;
  }
  if (step.kind === "a1-spool-checkpoint") {
    return `A1 mini spool confirmation is required before ${targetPlateLabel(step.beforeTargetOrder)}.`;
  }
  if (step.phase === "before-batch" && step.beforeTargetOrder !== null) {
    return `Printer setup is required before ${targetPlateLabel(step.beforeTargetOrder)}.`;
  }
  if (step.afterTargetOrder !== null) {
    return `Restore actions are required after ${targetPlateLabel(step.afterTargetOrder)}.`;
  }
  return "A printer setup checkpoint is required.";
}

function PartialRunExclusions({
  exclusions,
  published,
}: {
  exclusions: readonly ExcludedSourceUnit[];
  published: boolean;
}) {
  if (exclusions.length === 0) return null;

  return (
    <section
      className="print-run-exclusions"
      aria-labelledby="print-run-exclusions-heading"
    >
      <TriangleAlert aria-hidden="true" />
      <div>
        <h3 id="print-run-exclusions-heading">
          Partial print run · {exclusions.length} source {exclusions.length === 1 ? "unit" : "units"} excluded
        </h3>
        <p>
          {published
            ? "The published files exclude these source units. They have no target-plate step and cannot be marked complete."
            : "These are the current canonical exclusions. Print Run stays locked until published files match them exactly."}
        </p>
        <ul role="list">
          {exclusions.map((exclusion) => (
            <li key={exclusion.errorIdentity}>
              <div>
                <strong>{exclusion.sourceUnitId}</strong>
                <span>
                  {exclusion.sourcePlateId === null
                    ? "Source plate not identified"
                    : `Source Plate ${String(exclusion.sourcePlateId).padStart(2, "0")}`}
                </span>
              </div>
              <p>{exclusion.reason}</p>
            </li>
          ))}
        </ul>
      </div>
    </section>
  );
}

function ProgressNavigation({
  descriptor,
  progress,
}: {
  descriptor: PrintRunDescriptor;
  progress: PrintRunProgress;
}) {
  return (
    <nav
      className="print-run-progress"
      aria-label="Print run progress"
      tabIndex={0}
    >
      <ol>
        {descriptor.steps.map((step, index) => {
          const state = stepState(step, index, progress);
          const isCurrent = index === progress.currentStepIndex;
          return (
            <li
              className={`print-run-progress__step print-run-progress__step--${state}`}
              key={step.id}
              aria-current={isCurrent ? "step" : undefined}
            >
              <StepIcon state={state} />
              <span className="print-run-progress__copy">
                <strong>
                  {step.kind === "plate"
                    ? step.targetLabel
                    : checkpointTitle(step)}
                </strong>
                <small>
                  {step.kind === "plate"
                    ? `${step.plate.printer} · Source: ${step.plate.source}`
                    : checkpointSummary(step)}
                </small>
              </span>
              <span className={`print-run-step-status print-run-step-status--${state}`}>
                {stateLabel(state)}
              </span>
            </li>
          );
        })}
      </ol>
    </nav>
  );
}

function CurrentPlate({
  plate,
  artifact,
  focusRef,
  onComplete,
  isOpening,
  openingDisabled,
  onOpen,
}: {
  plate: PlatePlan;
  artifact: PublishedConversionArtifact;
  focusRef: Ref<HTMLHeadingElement>;
  onComplete: () => void;
  isOpening: boolean;
  openingDisabled: boolean;
  onOpen: () => void;
}) {
  const label = targetPlateLabel(plate.order);
  return (
    <article className="print-run-current" aria-labelledby="current-target-heading">
      <div className="print-run-current__heading">
        <div>
          <span className="print-run-eyebrow">Current target</span>
          <h3
            className="print-run-action-heading"
            id="current-target-heading"
            ref={focusRef}
            tabIndex={-1}
          >
            {label}
          </h3>
          <p>{plate.title}</p>
        </div>
        <span className="print-run-printer">
          <Printer aria-hidden="true" />
          {plate.printer}
        </span>
      </div>

      <dl className="print-run-target-details">
        <div>
          <dt>Source model plate</dt>
          <dd>{plate.source}</dd>
        </div>
        <div>
          <dt>Physical loadout</dt>
          <dd>{plate.loadoutLabel}</dd>
        </div>
        <div>
          <dt>Material</dt>
          <dd>{plate.material}</dd>
        </div>
        <div>
          <dt>Strategy</dt>
          <dd>{plate.recipeSummary}</dd>
        </div>
      </dl>

      <div className="print-run-artifact">
        <strong>Published project file</strong>
        <span>{artifact.fileName}</span>
        <code>{artifact.path}</code>
        <small>
          Open in {artifact.slicer} · SHA-256 {artifact.sha256}
        </small>
        <button
          className="button button--compact"
          type="button"
          disabled={openingDisabled}
          onClick={onOpen}
        >
          {isOpening ? (
            <LoaderCircle className="is-spinning" aria-hidden="true" />
          ) : (
            <ExternalLink aria-hidden="true" />
          )}
          {isOpening ? `Opening in ${artifact.slicer}…` : `Open in ${artifact.slicer}`}
        </button>
      </div>

      <div className="print-run-instruction">
        <strong>Print this target plate now.</strong>
        <p>
          Launch the published file shown above on the {plate.printer}. This app
          does not start, pause, or monitor the printer.
        </p>
      </div>

      <button className="button button--primary" type="button" onClick={onComplete}>
        <CheckCircle2 aria-hidden="true" />
        Mark {label} complete
      </button>
      <p className="print-run-button-help">
        Use this only after the printer reports that the whole target plate has
        finished.
      </p>
    </article>
  );
}

function PublishedFiles({
  plan,
  bundle,
  artifactsByPlate,
  openingArtifactPath,
  canOpenArtifact,
  onOpenArtifact,
}: {
  plan: PrintRunPlan;
  bundle: PublishedPrintRunBundle;
  artifactsByPlate: Map<string, PublishedConversionArtifact>;
  openingArtifactPath: string | null;
  canOpenArtifact: boolean;
  onOpenArtifact: (artifact: PublishedConversionArtifact) => void;
}) {
  return (
    <section
      className="print-run-files"
      aria-labelledby="print-run-files-heading"
    >
      <div>
        <h3 id="print-run-files-heading">Published files for this run</h3>
        <p>
          Manifest: <code>{bundle.result.manifestPath}</code>
        </p>
        <p>
          Report: <code>{bundle.result.reportPath}</code>
        </p>
      </div>
      <ol>
        {[...plan.plates]
          .sort(
            (left, right) =>
              left.order - right.order || left.id.localeCompare(right.id),
          )
          .map((plate) => {
            const artifact = artifactsByPlate.get(plate.id);
            if (!artifact) return null;
            return (
              <li key={plate.id}>
                <span>
                  <strong>{targetPlateLabel(plate.order)}</strong>
                  <small>{plate.title}</small>
                </span>
                <span>
                  <strong>{artifact.fileName}</strong>
                  <code>{artifact.path}</code>
                </span>
                <button
                  className="button button--compact"
                  type="button"
                  disabled={openingArtifactPath !== null || !canOpenArtifact}
                  onClick={() => onOpenArtifact(artifact)}
                >
                  {openingArtifactPath === artifact.path ? (
                    <LoaderCircle className="is-spinning" aria-hidden="true" />
                  ) : (
                    <ExternalLink aria-hidden="true" />
                  )}
                  {openingArtifactPath === artifact.path
                    ? `Opening in ${artifact.slicer}…`
                    : `Open in ${artifact.slicer}`}
                </button>
              </li>
            );
          })}
      </ol>
    </section>
  );
}

function A1SpoolCheckpoint({
  step,
  focusRef,
  onConfirm,
}: {
  step: PrintRunA1SpoolStep;
  focusRef: Ref<HTMLHeadingElement>;
  onConfirm: () => void;
}) {
  const beforeLabel = targetPlateLabel(step.beforeTargetOrder);
  const spoolName = step.spool.spoolName!;
  return (
    <article
      className="print-run-checkpoint"
      aria-labelledby="a1-spool-checkpoint-heading"
    >
      <div className="print-run-checkpoint__heading">
        <TriangleAlert aria-hidden="true" />
        <div>
          <span className="print-run-eyebrow">Blocking operator checkpoint</span>
          <h3
            className="print-run-action-heading"
            id="a1-spool-checkpoint-heading"
            ref={focusRef}
            tabIndex={-1}
          >
            Load the exact A1 mini spool before {beforeLabel}
          </h3>
        </div>
      </div>

      <p>
        This identity comes from the validated published project. Confirm it
        before opening the first plate in this A1 mini batch.
      </p>
      <dl className="print-run-target-details">
        <div>
          <dt>Physical spool</dt>
          <dd>{spoolName}</dd>
        </div>
        <div>
          <dt>Spool ID</dt>
          <dd><code>{step.spool.spoolId}</code></dd>
        </div>
        <div>
          <dt>Material</dt>
          <dd>{step.spool.material}</dd>
        </div>
        <div>
          <dt>External slot</dt>
          <dd>{step.spool.toolhead}</dd>
        </div>
        <div>
          <dt>Filament profile</dt>
          <dd>{step.spool.profile}</dd>
        </div>
        {step.spool.color ? (
          <div>
            <dt>Published color</dt>
            <dd>{step.spool.color}</dd>
          </div>
        ) : null}
      </dl>

      {step.actions.length > 0 ? (
        <ol className="print-run-change-actions" aria-label="Published A1 setup actions">
          {step.actions.map((action, index) => (
            <li key={`${step.id}:action:${index}`}>
              <span>{index + 1}</span>
              <span className="print-run-change-actions__instruction">
                {action}
              </span>
            </li>
          ))}
        </ol>
      ) : null}

      <p className="print-run-checkpoint__gate">
        <LockKeyhole aria-hidden="true" />
        {beforeLabel} remains blocked until this exact physical spool and
        filament profile are confirmed.
      </p>
      <button className="button button--dark" type="button" onClick={onConfirm}>
        <RefreshCw aria-hidden="true" />
        Confirm {spoolName} is loaded on A1 mini
      </button>
      <p className="print-run-button-help">
        This records your confirmation. It does not control or verify the printer.
      </p>
    </article>
  );
}

function FilamentCheckpoint({
  step,
  focusRef,
  onConfirm,
}: {
  step: PrintRunFilamentStep;
  focusRef: Ref<HTMLHeadingElement>;
  onConfirm: () => void;
}) {
  const beforeLabel = targetPlateLabel(step.beforeTargetOrder);
  const boundaryInstruction =
    step.afterTargetOrder === null
      ? `Make this change before launching ${beforeLabel}.`
      : `On the U1, make this change after ${targetPlateLabel(step.afterTargetOrder)} has finished and before launching ${beforeLabel}.`;

  return (
    <article
      className="print-run-checkpoint"
      aria-labelledby="filament-checkpoint-heading"
    >
      <div className="print-run-checkpoint__heading">
        <TriangleAlert aria-hidden="true" />
        <div>
          <span className="print-run-eyebrow">Blocking operator checkpoint</span>
          <h3
            className="print-run-action-heading"
            id="filament-checkpoint-heading"
            ref={focusRef}
            tabIndex={-1}
          >
            Filament change required before {beforeLabel}
          </h3>
        </div>
      </div>

      <p>{boundaryInstruction}</p>
      <ol className="print-run-change-actions">
        <li>
          <span>1</span>
          {step.fromLabel === UNKNOWN_T4_LABEL ? (
            <>Check T4. If a spool is loaded, unload it.</>
          ) : (
            <>
              Unload <strong>{step.fromLabel}</strong> from T4.
            </>
          )}
        </li>
        <li>
          <span>2</span>
          Load <strong>{step.toLabel}</strong> into T4 and finish the printer's load
          procedure.
        </li>
        <li>
          <span>3</span>
          Verify the T4 material and color mapping on the printer.
        </li>
      </ol>

      <p className="print-run-checkpoint__gate">
        <LockKeyhole aria-hidden="true" />
        {beforeLabel} remains blocked until you confirm the loaded T4 spool.
      </p>
      <button className="button button--dark" type="button" onClick={onConfirm}>
        <RefreshCw aria-hidden="true" />
        Confirm T4 {step.toLabel} is loaded
      </button>
      <p className="print-run-button-help">
        This records your confirmation. It does not control or verify the printer.
      </p>
    </article>
  );
}

function SetupCheckpoint({
  step,
  focusRef,
  onConfirm,
}: {
  step: PrintRunSetupStep;
  focusRef: Ref<HTMLHeadingElement>;
  onConfirm: () => void;
}) {
  const isBeforeBatch = step.phase === "before-batch";
  const heading = isBeforeBatch
    ? step.beforeTargetOrder === null
      ? "Printer setup required"
      : `Printer setup required before ${targetPlateLabel(step.beforeTargetOrder)}`
    : step.afterTargetOrder === null
      ? "Printer restore required"
      : `Restore required after ${targetPlateLabel(step.afterTargetOrder)}`;
  const timing = isBeforeBatch
    ? step.beforeTargetOrder === null
      ? "Complete every listed action before starting this batch."
      : `Complete every listed action before launching ${targetPlateLabel(step.beforeTargetOrder)}.`
    : step.beforeTargetOrder === null
      ? "Complete every listed restore action before ending this print run."
      : `Complete every listed restore action before launching ${targetPlateLabel(step.beforeTargetOrder)}.`;
  const buttonLabel = isBeforeBatch
    ? "Confirm printer setup is complete"
    : "Confirm restore is complete";

  return (
    <article
      className="print-run-checkpoint"
      aria-labelledby="setup-checkpoint-heading"
    >
      <div className="print-run-checkpoint__heading">
        <TriangleAlert aria-hidden="true" />
        <div>
          <span className="print-run-eyebrow">Blocking operator checkpoint</span>
          <h3
            className="print-run-action-heading"
            id="setup-checkpoint-heading"
            ref={focusRef}
            tabIndex={-1}
          >
            {heading}
          </h3>
        </div>
      </div>

      <p>{timing}</p>
      <ol className="print-run-change-actions">
        {step.actions.map((action, index) => (
          <li key={`${step.id}:${index}`}>
            <span>{index + 1}</span>
            <span className="print-run-change-actions__instruction">
              {action}
            </span>
          </li>
        ))}
      </ol>

      <p className="print-run-checkpoint__gate">
        <LockKeyhole aria-hidden="true" />
        {isBeforeBatch && step.beforeTargetOrder !== null
          ? `${targetPlateLabel(step.beforeTargetOrder)} remains blocked until you confirm the complete ${step.printer} setup.`
          : "The print run remains blocked until you confirm every listed action."}
      </p>
      <button className="button button--dark" type="button" onClick={onConfirm}>
        <RefreshCw aria-hidden="true" />
        {buttonLabel}
      </button>
      <p className="print-run-button-help">
        This records your confirmation. It does not control or verify the printer.
      </p>
    </article>
  );
}

function OperatorLog({ events }: { events: PrintRunEvent[] }) {
  if (events.length === 0) return null;
  return (
    <details className="print-run-log">
      <summary>Operator log · {events.length} recorded actions</summary>
      <ol>
        {events.map((event, index) => (
          <li key={`${event.type}-${event.recordedAt}-${index}`}>
            <span>{event.message}</span>
            <time dateTime={event.recordedAt}>{formatTimestamp(event.recordedAt)}</time>
          </li>
        ))}
      </ol>
    </details>
  );
}

export function PrintRun({
  plan,
  isPlanValidated,
  publishedBundle = null,
  storage,
  onCurrentTargetChange,
  onProgressChange,
  onOpenArtifact,
  now = () => new Date().toISOString(),
}: PrintRunProps) {
  const descriptor = useMemo(
    () => createPrintRunDescriptor(plan, publishedBundle),
    [plan, publishedBundle],
  );
  const artifactsByPlate = useMemo(
    () => publishedArtifactsForPlan(plan, publishedBundle),
    [plan, publishedBundle],
  );
  const progressStorage = storage === undefined ? defaultStorage() : storage;
  const requiresPersistentProgress = storage !== null;
  const [keyedProgress, setKeyedProgress] = useState<KeyedProgress>(() => ({
    storageKey: descriptor.storageKey,
    progress: loadPrintRunProgress(progressStorage, descriptor),
  }));
  const [liveMessage, setLiveMessage] = useState("");
  const [storageWarning, setStorageWarning] = useState("");
  const [openingArtifactPath, setOpeningArtifactPath] = useState<string | null>(null);
  const [artifactOpenError, setArtifactOpenError] = useState("");
  const [focusRequest, setFocusRequest] = useState(0);
  const actionHeadingRef = useRef<HTMLHeadingElement>(null);
  const progress =
    keyedProgress.storageKey === descriptor.storageKey
      ? keyedProgress.progress
      : createEmptyPrintRunProgress(descriptor);
  const bundleReady = isPublishedPrintRunBundleReady(plan, publishedBundle);
  const planExecutable = isPrintRunPlanExecutable(plan, publishedBundle);
  const executable = isPlanValidated && planExecutable && bundleReady;
  const currentStep = descriptor.steps[progress.currentStepIndex] ?? null;
  const completedPlateCount = progress.completedPlateIds.length;

  useEffect(() => {
    setKeyedProgress((current) => {
      if (current.storageKey === descriptor.storageKey) return current;
      return {
        storageKey: descriptor.storageKey,
        progress: loadPrintRunProgress(progressStorage, descriptor),
      };
    });
    setLiveMessage("");
    setStorageWarning("");
    setOpeningArtifactPath(null);
    setArtifactOpenError("");
  }, [descriptor, progressStorage]);

  useEffect(() => {
    if (focusRequest === 0) return;
    actionHeadingRef.current?.focus();
  }, [focusRequest]);

  const currentTargetPlateId =
    currentStep?.kind === "plate"
      ? currentStep.plate.id
      : currentStep?.beforePlateId ?? null;
  useEffect(() => {
    if (
      executable &&
      progress.status !== "not-started" &&
      currentTargetPlateId
    ) {
      onCurrentTargetChange?.(currentTargetPlateId);
    }
  }, [
    currentTargetPlateId,
    executable,
    onCurrentTargetChange,
    progress.status,
  ]);

  const commitProgress = (next: PrintRunProgress, message: string) => {
    const saved = savePrintRunProgress(progressStorage, descriptor, next);
    if (requiresPersistentProgress && !saved) {
      const warning =
        "Progress could not be saved locally. The action was not recorded and the workflow did not advance. Resolve local storage, then retry this action.";
      setStorageWarning(warning);
      setLiveMessage(warning);
      return false;
    }
    setKeyedProgress({ storageKey: descriptor.storageKey, progress: next });
    setStorageWarning("");
    setLiveMessage(message);
    onProgressChange?.(next);
    setFocusRequest((request) => request + 1);
    return true;
  };

  const openArtifact = async (artifact: PublishedConversionArtifact) => {
    if (openingArtifactPath || !onOpenArtifact) return;
    const registered = [...artifactsByPlate.values()].some(
      (candidate) =>
        candidate.path === artifact.path &&
        candidate.adapterId === artifact.adapterId &&
        candidate.sha256 === artifact.sha256,
    );
    if (!registered) {
      const message =
        "This file is not part of the validated publication for the current plan and cannot be opened.";
      setArtifactOpenError(message);
      setLiveMessage(message);
      return;
    }
    setOpeningArtifactPath(artifact.path);
    setArtifactOpenError("");
    setLiveMessage(`Opening ${artifact.fileName} in ${artifact.slicer}…`);
    try {
      await onOpenArtifact(artifact);
      setLiveMessage(`${artifact.fileName} was opened in ${artifact.slicer}.`);
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      const message = `Could not open ${artifact.fileName} in ${artifact.slicer}. ${detail}`;
      setArtifactOpenError(message);
      setLiveMessage(message);
    } finally {
      setOpeningArtifactPath(null);
    }
  };

  const startRun = () => {
    if (!executable || descriptor.steps.length === 0) return;
    const recordedAt = now();
    const firstStep = descriptor.steps[0];
    const firstAction = nextStepMessage(firstStep);
    commitProgress(
      {
        ...progress,
        status: "active",
        currentStepIndex: 0,
        events: [
          ...progress.events,
          {
            type: "run-started",
            stepId: null,
            recordedAt,
            message: "Print run started",
          },
        ],
      },
      `Print run started. ${firstAction}`,
    );
  };

  const completePlate = () => {
    if (progress.status !== "active" || currentStep?.kind !== "plate") return;
    const nextIndex = progress.currentStepIndex + 1;
    const isComplete = nextIndex >= descriptor.steps.length;
    const nextStep = descriptor.steps[nextIndex];
    const message = `${currentStep.targetLabel} marked complete.`;
    commitProgress(
      {
        ...progress,
        status: isComplete ? "complete" : "active",
        currentStepIndex: nextIndex,
        completedPlateIds: [
          ...new Set([...progress.completedPlateIds, currentStep.plate.id]),
        ],
        events: [
          ...progress.events,
          {
            type: "plate-completed",
            stepId: currentStep.id,
            recordedAt: now(),
            message,
          },
        ],
      },
      isComplete
        ? `${message} The print run is complete.`
        : `${message} ${nextStepMessage(nextStep)}`,
    );
  };

  const confirmCheckpoint = () => {
    if (progress.status !== "active" || !currentStep || currentStep.kind === "plate") {
      return;
    }
    const nextIndex = progress.currentStepIndex + 1;
    const isComplete = nextIndex >= descriptor.steps.length;
    const nextStep = descriptor.steps[nextIndex];
    const message =
      currentStep.kind === "filament-change"
        ? `T4 ${currentStep.toLabel} load confirmed before ${targetPlateLabel(currentStep.beforeTargetOrder)}.`
        : currentStep.kind === "a1-spool-checkpoint"
          ? `A1 mini spool ${currentStep.spool.spoolName} confirmed before ${targetPlateLabel(currentStep.beforeTargetOrder)}.`
        : currentStep.phase === "before-batch"
          ? `Printer setup confirmed${currentStep.beforeTargetOrder === null ? "." : ` before ${targetPlateLabel(currentStep.beforeTargetOrder)}.`}`
          : `Restore actions confirmed${currentStep.afterTargetOrder === null ? "." : ` after ${targetPlateLabel(currentStep.afterTargetOrder)}.`}`;
    commitProgress(
      {
        ...progress,
        status: isComplete ? "complete" : "active",
        currentStepIndex: nextIndex,
        confirmedCheckpointIds: [
          ...new Set([...progress.confirmedCheckpointIds, currentStep.id]),
        ],
        events: [
          ...progress.events,
          {
            type: "checkpoint-confirmed",
            stepId: currentStep.id,
            recordedAt: now(),
            message,
          },
        ],
      },
      isComplete ? `${message} The print run is complete.` : `${message} ${nextStepMessage(nextStep)}`,
    );
  };

  const resetRun = () => {
    const cleared = clearPrintRunProgress(progressStorage, descriptor);
    const next = createEmptyPrintRunProgress(descriptor);
    if (requiresPersistentProgress && !cleared) {
      const warning =
        "Saved progress could not be cleared. The workflow was not reset. Resolve local storage, then retry Reset print run.";
      setStorageWarning(warning);
      setLiveMessage(warning);
      return;
    } else {
      setKeyedProgress({ storageKey: descriptor.storageKey, progress: next });
      setStorageWarning("");
      setLiveMessage(
        requiresPersistentProgress
          ? "Print run progress reset. Saved progress was cleared."
          : "Print run progress reset for this session. No plate is marked complete.",
      );
    }
    onProgressChange?.(next);
    setFocusRequest((request) => request + 1);
  };

  return (
    <section className="print-run" aria-labelledby="print-run-heading">
      <div className="print-run__heading">
        <div>
          <span className="print-run-eyebrow">Operator workflow</span>
          <h2 id="print-run-heading">Print Run</h2>
          <p>
            Record completed target plates and required spool changes between print
            jobs.
          </p>
        </div>
        <span className="print-run__count">
          {completedPlateCount} of {plan.plates.length} target plates complete
        </span>
      </div>

      <PartialRunExclusions
        exclusions={plan.partialConversion.exclusions}
        published={executable}
      />

      {!executable ? (
        <div className="print-run-locked">
          <LockKeyhole aria-hidden="true" />
          <div>
            <h3>Print Run is locked</h3>
            <p>
              {!isPlanValidated
                ? "Recalculate and validate pending plan changes before starting or resuming."
                : !publishedBundle
                    ? "Convert this native plan successfully before starting. Print Run uses only validated, published project files."
                    : !bundleReady
                      ? "The published files, source-unit coverage, or exclusion evidence do not match this exact plan. Run conversion again before starting."
                      : !planExecutable
                        ? "The current plan is not a complete plan or an exactly approved partial plan. Review its exclusions and convert again."
                        : "The last published files do not match this exact plan. Run conversion again before starting."}
            </p>
          </div>
        </div>
      ) : (
        <>
          {publishedBundle ? (
            <PublishedFiles
              plan={plan}
              bundle={publishedBundle}
              artifactsByPlate={artifactsByPlate}
              openingArtifactPath={openingArtifactPath}
              canOpenArtifact={Boolean(onOpenArtifact)}
              onOpenArtifact={(artifact) => void openArtifact(artifact)}
            />
          ) : null}
          <ProgressNavigation descriptor={descriptor} progress={progress} />

          {progress.status === "not-started" ? (
            <article className="print-run-start" aria-labelledby="print-run-start-heading">
              <Play aria-hidden="true" />
              <div>
                <h3
                  className="print-run-action-heading"
                  id="print-run-start-heading"
                  ref={actionHeadingRef}
                  tabIndex={-1}
                >
                  Ready to begin
                </h3>
                <p>
                  Start only when these published project files are the ones loaded
                  in their listed slicers. Progress is saved for this source file,
                  exact plan, manifest, and artifact checksums.
                </p>
                <button
                  className="button button--primary"
                  type="button"
                  onClick={startRun}
                >
                  <Play aria-hidden="true" />
                  Start print run
                </button>
              </div>
            </article>
          ) : progress.status === "complete" ? (
            <article className="print-run-complete" aria-labelledby="print-run-complete-heading">
              <CheckCircle2 aria-hidden="true" />
              <div>
                <h3
                  className="print-run-action-heading"
                  id="print-run-complete-heading"
                  ref={actionHeadingRef}
                  tabIndex={-1}
                >
                  Print run complete
                </h3>
                <p>All {plan.plates.length} target plates are recorded as complete.</p>
              </div>
            </article>
          ) : currentStep?.kind === "filament-change" ? (
            <FilamentCheckpoint
              step={currentStep}
              focusRef={actionHeadingRef}
              onConfirm={confirmCheckpoint}
            />
          ) : currentStep?.kind === "a1-spool-checkpoint" ? (
            <A1SpoolCheckpoint
              step={currentStep}
              focusRef={actionHeadingRef}
              onConfirm={confirmCheckpoint}
            />
          ) : currentStep?.kind === "setup-checkpoint" ? (
            <SetupCheckpoint
              step={currentStep}
              focusRef={actionHeadingRef}
              onConfirm={confirmCheckpoint}
            />
          ) : currentStep?.kind === "plate" ? (
            artifactsByPlate.get(currentStep.plate.id) ? (
              <CurrentPlate
                plate={currentStep.plate}
                artifact={artifactsByPlate.get(currentStep.plate.id)!}
                focusRef={actionHeadingRef}
                onComplete={completePlate}
                isOpening={openingArtifactPath === artifactsByPlate.get(currentStep.plate.id)!.path}
                openingDisabled={openingArtifactPath !== null || !onOpenArtifact}
                onOpen={() => void openArtifact(artifactsByPlate.get(currentStep.plate.id)!)}
              />
            ) : null
          ) : null}

          <div className="print-run-footer">
            <p>
              <ArrowRight aria-hidden="true" />
              All actions happen between complete plate jobs. There are no mid-print
              filament-change instructions.
            </p>
            {progress.status !== "not-started" ? (
              <button
                className="button button--compact"
                type="button"
                onClick={resetRun}
              >
                <RotateCcw aria-hidden="true" />
                Reset print run
              </button>
            ) : null}
          </div>

          <OperatorLog events={progress.events} />
        </>
      )}

      {storageWarning ? (
        <p className="print-run-storage-warning" role="alert">
          <TriangleAlert aria-hidden="true" />
          {storageWarning}
        </p>
      ) : null}
      {artifactOpenError ? (
        <p className="print-run-storage-warning" role="alert">
          <TriangleAlert aria-hidden="true" />
          {artifactOpenError}
        </p>
      ) : null}
      <p className="visually-hidden" role="status" aria-live="polite" aria-atomic="true">
        {liveMessage}
      </p>
    </section>
  );
}
