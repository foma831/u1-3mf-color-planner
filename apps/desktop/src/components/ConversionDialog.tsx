import { useEffect, useId, useMemo, useRef, useState } from "react";
import {
  CheckCircle2,
  ExternalLink,
  FolderOpen,
  FolderOutput,
  Info,
  LoaderCircle,
  PlayCircle,
  RefreshCw,
  TriangleAlert,
  X,
} from "lucide-react";

import type {
  ActiveConversionOutputAction,
  ConversionProgress,
  ConversionResult,
  ExcludedSourceUnit,
  PreparedConversion,
  PreparedPhysicalSlot,
  PublishedConversionArtifact,
} from "../types";

interface ConversionDialogProps {
  prepared: PreparedConversion | null;
  destinationDirectory: string;
  isConverting: boolean;
  progress: ConversionProgress | null;
  result: ConversionResult | null;
  error: string;
  needsNewPreflight: boolean;
  activeOutputAction: ActiveConversionOutputAction | null;
  onConvert: (warningsAcknowledged: boolean) => void;
  onCancelConversion: () => void;
  onOpenOutput: (artifact: PublishedConversionArtifact) => void;
  onShowOutputInFinder: (artifact: PublishedConversionArtifact) => void;
  onRetryPreflight: () => void;
  onOpenPrintRun: () => void;
  onClose: () => void;
}

interface RoutedArtifact {
  adapterId: string;
  target: string;
  printer: string;
  strategy: string;
  slicer: string;
  batchId: string;
  fileName: string;
}

interface ArtifactGroup<T extends RoutedArtifact> {
  key: string;
  printer: string;
  slicer: string;
  artifacts: T[];
}

function groupArtifacts<T extends RoutedArtifact>(
  artifacts: readonly T[],
): ArtifactGroup<T>[] {
  const groups = new Map<string, ArtifactGroup<T>>();
  for (const artifact of artifacts) {
    const printer = artifact.printer || artifact.target || "Other printer";
    const slicer = artifact.slicer || "Configured slicer";
    const key = JSON.stringify([printer, slicer]);
    const group = groups.get(key);
    if (group) {
      group.artifacts.push(artifact);
    } else {
      groups.set(key, { key, printer, slicer, artifacts: [artifact] });
    }
  }
  return [...groups.values()];
}

function countLabel(count: number, singular: string, plural = `${singular}s`) {
  return `${count} ${count === 1 ? singular : plural}`;
}

function slotDetails(slot: PreparedPhysicalSlot) {
  return [slot.material, slot.profile, slot.color].filter(Boolean).join(" · ");
}

function artifactKey(artifact: RoutedArtifact) {
  return [
    artifact.adapterId,
    artifact.target,
    artifact.batchId,
    artifact.fileName,
  ].join(":");
}

function excludedUnitPlate(sourcePlateId: number | null) {
  return sourcePlateId === null
    ? "Source plate not identified"
    : `Source Plate ${String(sourcePlateId).padStart(2, "0")}`;
}

function ExcludedUnitsBlock({
  exclusions,
  headingId,
  result = false,
}: {
  exclusions: readonly ExcludedSourceUnit[];
  headingId: string;
  result?: boolean;
}) {
  if (exclusions.length === 0) return null;

  return (
    <section
      className="conversion-exclusions-block"
      aria-labelledby={headingId}
    >
      <div className="conversion-exclusions-block__heading">
        <TriangleAlert aria-hidden="true" />
        <div>
          <h3 id={headingId}>
            {countLabel(exclusions.length, "excluded source unit")}
          </h3>
          <p>
            {result
              ? "These units were not included in the published project files."
              : "These units will not be included in generated project files."}
          </p>
        </div>
      </div>
      <ul className="conversion-exclusions" role="list">
        {exclusions.map((exclusion) => (
          <li key={exclusion.errorIdentity}>
            <div>
              <strong>{exclusion.sourceUnitId}</strong>
              <span>{excludedUnitPlate(exclusion.sourcePlateId)}</span>
            </div>
            <p>{exclusion.reason}</p>
            <small>
              Scope {exclusion.scopeId} · Planning unit{" "}
              {exclusion.planningUnitId}
            </small>
          </li>
        ))}
      </ul>
    </section>
  );
}

export function ConversionDialog({
  prepared,
  destinationDirectory,
  isConverting,
  progress,
  result,
  error,
  needsNewPreflight,
  activeOutputAction,
  onConvert,
  onCancelConversion,
  onOpenOutput,
  onShowOutputInFinder,
  onRetryPreflight,
  onOpenPrintRun,
  onClose,
}: ConversionDialogProps) {
  const instanceId = useId().replace(/:/g, "");
  const dialogRef = useRef<HTMLDialogElement>(null);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const titleRef = useRef<HTMLHeadingElement>(null);
  const warningAcknowledgementRef = useRef<HTMLInputElement>(null);
  const returnFocusRef = useRef<HTMLElement | null>(null);
  const fallbackDialogRef = useRef(false);
  const previousResultRef = useRef<ConversionResult | null>(null);
  const [warningsAcknowledged, setWarningsAcknowledged] = useState(false);
  const isOpen = Boolean(prepared || result || error);
  const warningEvidenceKey = useMemo(
    () =>
      prepared
        ? JSON.stringify([
            prepared.preparationToken,
            prepared.preparation.warnings,
            prepared.preparation.excludedSourceUnits,
          ])
        : "",
    [prepared],
  );

  useEffect(() => {
    setWarningsAcknowledged(false);
  }, [warningEvidenceKey]);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;

    if (isOpen && !dialog.open) {
      returnFocusRef.current =
        document.activeElement instanceof HTMLElement
          ? document.activeElement
          : null;
      if (typeof dialog.showModal === "function") {
        fallbackDialogRef.current = false;
        dialog.showModal();
      } else {
        // jsdom and older embedded webviews do not expose showModal().
        fallbackDialogRef.current = true;
        dialog.setAttribute("open", "");
        closeButtonRef.current?.focus();
      }
      return;
    }

    if (!isOpen && dialog.open) {
      if (typeof dialog.close === "function") dialog.close();
      else dialog.removeAttribute("open");
    }
    if (!isOpen && fallbackDialogRef.current) {
      returnFocusRef.current?.focus();
      returnFocusRef.current = null;
      fallbackDialogRef.current = false;
    }
  }, [isOpen]);

  useEffect(() => {
    if (
      result &&
      result !== previousResultRef.current &&
      dialogRef.current?.open
    ) {
      titleRef.current?.focus();
    }
    previousResultRef.current = result;
  }, [result]);

  const preparation = prepared?.preparation;
  const preparedGroups = groupArtifacts(preparation?.artifacts ?? []);
  const resultGroups = groupArtifacts(result?.artifacts ?? []);
  const titleId = `${instanceId}-conversion-dialog-title`;
  const filesHeadingId = `${instanceId}-conversion-files-heading`;
  const resultFilesHeadingId = `${instanceId}-conversion-result-files-heading`;
  const preparationExclusionsHeadingId = `${instanceId}-conversion-preparation-exclusions-heading`;
  const resultExclusionsHeadingId = `${instanceId}-conversion-result-exclusions-heading`;
  const warningAcknowledgementId = `${instanceId}-conversion-warning-acknowledgement`;
  const warningAcknowledgementLabelId = `${instanceId}-conversion-warning-acknowledgement-label`;
  const warningsRequireAcknowledgement =
    preparation !== undefined && preparation.warnings.length > 0;

  return (
    <dialog
      ref={dialogRef}
      className="conversion-dialog"
      aria-labelledby={titleId}
      onCancel={(event) => {
        if (isConverting) event.preventDefault();
      }}
      onClose={() => {
        if (!isConverting && isOpen) onClose();
      }}
    >
      <div className="conversion-dialog__header">
        <div className="conversion-dialog__title-icon" aria-hidden="true">
          {result ? <CheckCircle2 /> : <FolderOutput />}
        </div>
        <div>
          <p className="eyebrow">Native Project 3MF</p>
          <h2 id={titleId} ref={titleRef} tabIndex={-1}>
            {result ? "Conversion complete" : "Review conversion output"}
          </h2>
        </div>
        <button
          ref={closeButtonRef}
          className="icon-button"
          type="button"
          aria-label="Close conversion dialog"
          disabled={isConverting}
          onClick={onClose}
        >
          <X aria-hidden="true" />
        </button>
      </div>

      <div
        className="conversion-dialog__scroll-region"
        role="region"
        aria-label="Conversion details"
        tabIndex={0}
      >
        {preparation && !result ? (
          <div className="conversion-dialog__body">
            <dl className="conversion-summary">
              <div>
                <dt>Destination</dt>
                <dd>{destinationDirectory}</dd>
              </div>
              <div>
                <dt>Output folder</dt>
                <dd>{preparation.bundleDirectoryName}</dd>
              </div>
              <div>
                <dt>Adapter</dt>
                <dd>{preparation.adapterId}</dd>
              </div>
            </dl>

            <ExcludedUnitsBlock
              exclusions={preparation.excludedSourceUnits}
              headingId={preparationExclusionsHeadingId}
            />

            <section aria-labelledby={filesHeadingId}>
              <h3 id={filesHeadingId}>
                {countLabel(preparation.artifacts.length, "project file")}
              </h3>
              <div className="conversion-target-groups">
                {preparedGroups.map((group, groupIndex) => {
                  const groupHeadingId = `${instanceId}-prepared-target-${groupIndex}`;
                  return (
                    <section
                      className="conversion-target-group"
                      aria-labelledby={groupHeadingId}
                      key={group.key}
                    >
                      <header className="conversion-target-group__header">
                        <div>
                          <h4 id={groupHeadingId}>{group.printer}</h4>
                          <p>{group.slicer}</p>
                        </div>
                        <span>
                          {countLabel(group.artifacts.length, "file")}
                        </span>
                      </header>
                      <ul className="conversion-artifacts" role="list">
                        {group.artifacts.map((artifact) => (
                          <li key={artifactKey(artifact)}>
                            <article className="conversion-artifact">
                              <div>
                                <h5>{artifact.fileName}</h5>
                                <p>
                                  <strong>{artifact.strategy}</strong>
                                  {" · "}
                                  {countLabel(
                                    artifact.targetPlateIds.length,
                                    "plate",
                                  )}
                                  {" · "}
                                  {countLabel(
                                    artifact.sourceUnitIds.length,
                                    "printable unit",
                                  )}
                                </p>
                              </div>
                              <ol
                                className="conversion-loadout"
                                role="list"
                                aria-label={`${artifact.fileName} filament loadout for ${group.printer}`}
                              >
                                {artifact.loadout.map((slot) => (
                                  <li key={slot.toolhead}>
                                    <span
                                      className="conversion-loadout__swatch"
                                      style={{
                                        backgroundColor: slot.color ?? "#fff",
                                      }}
                                      aria-hidden="true"
                                    />
                                    <strong>{slot.toolhead}</strong>
                                    <span>{slot.spoolName ?? "Unused"}</span>
                                    <small>
                                      {slotDetails(slot) || "No spool assigned"}
                                    </small>
                                  </li>
                                ))}
                              </ol>
                              {artifact.setupActions.length > 0 ? (
                                <div className="conversion-setup-actions">
                                  <strong>Setup actions</strong>
                                  <ul>
                                    {artifact.setupActions.map(
                                      (action, actionIndex) => (
                                        <li key={`${actionIndex}:${action}`}>
                                          {action}
                                        </li>
                                      ),
                                    )}
                                  </ul>
                                </div>
                              ) : null}
                            </article>
                          </li>
                        ))}
                      </ul>
                    </section>
                  );
                })}
              </div>
            </section>

            <div className="conversion-notice">
              <TriangleAlert aria-hidden="true" />
              <div>
                <strong>Unsliced project files</strong>
                <p>
                  Open each file in the slicer shown for its printer, verify the
                  listed filament mapping, and slice before printing.
                </p>
              </div>
            </div>
            {preparation.warnings.length > 0 ? (
              <div className="conversion-warnings-block">
                <strong id={`${instanceId}-conversion-warnings`}>
                  Warnings
                </strong>
                <ul
                  className="conversion-warnings"
                  aria-labelledby={`${instanceId}-conversion-warnings`}
                >
                  {preparation.warnings.map((warning) => (
                    <li key={warning}>{warning}</li>
                  ))}
                </ul>
                <label className="conversion-warning-acknowledgement">
                  <input
                    ref={warningAcknowledgementRef}
                    id={warningAcknowledgementId}
                    type="checkbox"
                    checked={warningsAcknowledged}
                    disabled={isConverting}
                    onChange={(event) =>
                      setWarningsAcknowledged(event.target.checked)
                    }
                  />
                  <span id={warningAcknowledgementLabelId}>
                    I have reviewed these warnings and want to continue with
                    this conversion.
                  </span>
                </label>
              </div>
            ) : null}
          </div>
        ) : null}

        {result ? (
          <div className="conversion-dialog__body">
            <p className="conversion-result-path">{result.outputDirectory}</p>
            <p className="conversion-result-path">
              Human-readable report: <code>{result.reportPath}</code>
            </p>
            <div className="conversion-notice">
              <Info aria-hidden="true" />
              <div>
                <strong>Printer filament has not changed</strong>
                <p>
                  Conversion creates project files only. Complete the guided
                  Print Run before saving its final printer loadout for the next
                  project.
                </p>
              </div>
            </div>
            <ExcludedUnitsBlock
              exclusions={result.excludedSourceUnits}
              headingId={resultExclusionsHeadingId}
              result
            />
            <section aria-labelledby={resultFilesHeadingId}>
              <h3 id={resultFilesHeadingId}>
                {countLabel(result.artifacts.length, "converted project file")}
              </h3>
              <div className="conversion-target-groups">
                {resultGroups.map((group, groupIndex) => {
                  const groupHeadingId = `${instanceId}-result-target-${groupIndex}`;
                  return (
                    <section
                      className="conversion-target-group"
                      aria-labelledby={groupHeadingId}
                      key={group.key}
                    >
                      <header className="conversion-target-group__header">
                        <div>
                          <h4 id={groupHeadingId}>{group.printer}</h4>
                          <p>{group.slicer}</p>
                        </div>
                        <span>
                          {countLabel(group.artifacts.length, "file")}
                        </span>
                      </header>
                      <ul className="conversion-result-list" role="list">
                        {group.artifacts.map((artifact) => (
                          <li key={artifactKey(artifact)}>
                            <article className="conversion-result-artifact">
                              <header>
                                <div>
                                  <strong>{artifact.fileName}</strong>
                                  <small>
                                    {artifact.strategy} ·{" "}
                                    {countLabel(artifact.plateCount, "plate")} ·{" "}
                                    {(
                                      artifact.byteSize /
                                      (1024 * 1024)
                                    ).toFixed(1)}
                                    {" MB · "}
                                    {artifact.validationStatus} validation
                                  </small>
                                  <code>{artifact.relativePath}</code>
                                </div>
                                <code
                                  aria-label={`SHA-256 checksum for ${artifact.fileName}`}
                                >
                                  {artifact.sha256.slice(0, 16)}…
                                </code>
                              </header>
                              <ol
                                className="conversion-loadout"
                                role="list"
                                aria-label={`${artifact.fileName} published filament loadout for ${group.printer}`}
                              >
                                {artifact.loadout.map((slot) => (
                                  <li key={slot.toolhead}>
                                    <span
                                      className="conversion-loadout__swatch"
                                      style={{
                                        backgroundColor: slot.color ?? "#fff",
                                      }}
                                      aria-hidden="true"
                                    />
                                    <strong>{slot.toolhead}</strong>
                                    <span>{slot.spoolName ?? "Unused"}</span>
                                    <small>
                                      {slotDetails(slot) || "No spool assigned"}
                                    </small>
                                  </li>
                                ))}
                              </ol>
                              {artifact.setupActions.length > 0 ? (
                                <div className="conversion-setup-actions">
                                  <strong>Setup actions</strong>
                                  <ul>
                                    {artifact.setupActions.map(
                                      (action, actionIndex) => (
                                        <li key={`${actionIndex}:${action}`}>
                                          {action}
                                        </li>
                                      ),
                                    )}
                                  </ul>
                                </div>
                              ) : null}
                              <div className="conversion-result-actions">
                                <button
                                  className="button button--secondary"
                                  type="button"
                                  disabled={activeOutputAction !== null}
                                  onClick={() => onOpenOutput(artifact)}
                                >
                                  {activeOutputAction?.kind === "open" &&
                                  activeOutputAction.path === artifact.path ? (
                                    <LoaderCircle
                                      className="spin"
                                      aria-hidden="true"
                                    />
                                  ) : (
                                    <ExternalLink aria-hidden="true" />
                                  )}
                                  Open in {artifact.slicer}
                                </button>
                                <button
                                  className="button button--secondary"
                                  type="button"
                                  disabled={activeOutputAction !== null}
                                  onClick={() => onShowOutputInFinder(artifact)}
                                >
                                  {activeOutputAction?.kind === "reveal" &&
                                  activeOutputAction.path === artifact.path ? (
                                    <LoaderCircle
                                      className="spin"
                                      aria-hidden="true"
                                    />
                                  ) : (
                                    <FolderOpen aria-hidden="true" />
                                  )}
                                  Show in Finder
                                </button>
                              </div>
                            </article>
                          </li>
                        ))}
                      </ul>
                    </section>
                  );
                })}
              </div>
            </section>
            <p className="conversion-success-reminder">
              Bundle manifest and checksums were published. Open each project in
              its listed slicer and verify filament mapping before printing.
            </p>
            {result.warnings.length > 0 ? (
              <div className="conversion-warnings-block">
                <strong id={`${instanceId}-conversion-result-warnings`}>
                  Warnings
                </strong>
                <ul
                  className="conversion-warnings"
                  aria-labelledby={`${instanceId}-conversion-result-warnings`}
                >
                  {result.warnings.map((warning) => (
                    <li key={warning}>{warning}</li>
                  ))}
                </ul>
                <p className="conversion-warning-audit">
                  {result.warningsAcknowledged
                    ? "Warnings were explicitly acknowledged before conversion."
                    : "No warning acknowledgement was recorded."}
                </p>
              </div>
            ) : null}
          </div>
        ) : null}

        {progress && isConverting ? (
          <p
            className="conversion-progress"
            role="status"
            aria-live="polite"
            aria-atomic="true"
          >
            <LoaderCircle className="spin" aria-hidden="true" />
            {progress.message}
          </p>
        ) : null}
        {error ? (
          <div>
            <p className="conversion-error" role="alert">
              <TriangleAlert aria-hidden="true" />
              {error}
            </p>
            {needsNewPreflight ? (
              <p className="conversion-preflight-expired">
                This conversion attempt consumed its one-time preparation. Run a
                new preflight before trying again; the failed token cannot be
                reused.
              </p>
            ) : null}
          </div>
        ) : null}
      </div>

      <div className="conversion-dialog__actions">
        <button
          className="button button--secondary"
          type="button"
          onClick={isConverting ? onCancelConversion : onClose}
        >
          {isConverting ? "Stop conversion" : result ? "Close" : "Cancel"}
        </button>
        {!result && prepared ? (
          <button
            className="button button--primary"
            type="button"
            aria-disabled={
              (warningsRequireAcknowledgement && !warningsAcknowledged) ||
              undefined
            }
            aria-describedby={
              warningsRequireAcknowledgement && !warningsAcknowledged
                ? warningAcknowledgementLabelId
                : undefined
            }
            disabled={isConverting}
            onClick={(event) => {
              if (warningsRequireAcknowledgement && !warningsAcknowledged) {
                event.preventDefault();
                warningAcknowledgementRef.current?.focus();
                return;
              }
              onConvert(warningsAcknowledged);
            }}
          >
            {isConverting ? (
              <LoaderCircle className="spin" aria-hidden="true" />
            ) : (
              <FolderOutput aria-hidden="true" />
            )}
            {isConverting
              ? "Converting…"
              : warningsRequireAcknowledgement && !warningsAcknowledged
                ? "Review warnings to convert"
                : "Convert projects"}
          </button>
        ) : null}
        {!result && !prepared && error && needsNewPreflight ? (
          <button
            className="button button--primary"
            type="button"
            onClick={onRetryPreflight}
          >
            <RefreshCw aria-hidden="true" />
            Run preflight again
          </button>
        ) : null}
        {result ? (
          <button
            className="button button--primary"
            type="button"
            onClick={onOpenPrintRun}
          >
            <PlayCircle aria-hidden="true" />
            Open Print Run
          </button>
        ) : null}
      </div>
    </dialog>
  );
}
