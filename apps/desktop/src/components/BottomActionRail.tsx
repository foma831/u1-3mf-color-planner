import { useId } from "react";
import {
  CheckCircle2,
  Download,
  Ellipsis,
  FolderOutput,
  ListChecks,
  LoaderCircle,
  RefreshCw,
  TriangleAlert,
} from "lucide-react";

import type { ConversionAdapterCapability } from "../types";

interface BottomActionRailProps {
  isBusy: boolean;
  isValidating: boolean;
  planAvailable: boolean;
  exportAvailable: boolean;
  exportBlockReason?: string;
  onExport: () => void;
  onValidate: () => void;
  validationAvailable: boolean;
  planNeedsRecalculation: boolean;
  colorNeedsRecalculation: boolean;
  conversionStageAvailable: boolean;
  conversionAvailable: boolean;
  conversionAdapters: ConversionAdapterCapability[];
  isCheckingConversion: boolean;
  conversionLabel?: string;
  conversionBlockReason?: string;
  onApprove?: () => void;
}

export function BottomActionRail({
  isBusy,
  isValidating,
  planAvailable,
  exportAvailable,
  exportBlockReason,
  onExport,
  onValidate,
  validationAvailable,
  planNeedsRecalculation,
  colorNeedsRecalculation,
  conversionStageAvailable,
  conversionAvailable,
  conversionAdapters,
  isCheckingConversion,
  conversionLabel = "Approve & Convert",
  conversionBlockReason,
  onApprove,
}: BottomActionRailProps) {
  const instanceId = useId().replace(/:/g, "");
  const recalculationNeeded = planNeedsRecalculation || colorNeedsRecalculation;
  const showValidationAction = recalculationNeeded || !conversionStageAvailable;
  const validationLabelId = `${instanceId}-validation-label`;
  const validationReasonId = `${instanceId}-validation-reason`;
  const conversionLabelId = `${instanceId}-conversion-label`;
  const conversionReasonId = `${instanceId}-conversion-reason`;
  const exportLabelId = `${instanceId}-export-label`;
  const exportReasonId = `${instanceId}-export-reason`;
  return (
    <footer className="bottom-action-rail">
      <div className="bottom-actions">
        <details className="more-actions">
          <summary className="button button--secondary">
            <Ellipsis aria-hidden="true" />
            More actions
          </summary>
          <div className="more-actions__panel">
            {isCheckingConversion || conversionAdapters.length > 0 ? (
              <details className="adapter-capabilities">
                <summary>
                  {isCheckingConversion ? (
                    <LoaderCircle className="spin" aria-hidden="true" />
                  ) : conversionAdapters.every(
                      (adapter) => adapter.available,
                    ) ? (
                    <CheckCircle2 aria-hidden="true" />
                  ) : (
                    <TriangleAlert aria-hidden="true" />
                  )}
                  Writer adapter checks
                </summary>
                {isCheckingConversion ? (
                  <p>
                    Checking installed slicers, profiles, and qualification
                    evidence…
                  </p>
                ) : (
                  <ul>
                    {conversionAdapters.map((adapter) => {
                      const report = adapterReportSummary(adapter.report);
                      return (
                        <li key={`${adapter.target}-${adapter.adapterId}`}>
                          <div>
                            <strong>{adapterTargetName(adapter.target)}</strong>
                            <span
                              className={
                                adapter.available
                                  ? "adapter-capabilities__status is-ready"
                                  : "adapter-capabilities__status is-blocked"
                              }
                            >
                              {adapter.available ? "Ready" : "Blocked"}
                            </span>
                          </div>
                          <p>{adapter.reason}</p>
                          <dl>
                            <div>
                              <dt>Slicer</dt>
                              <dd>
                                {adapter.slicer}
                                {report.version ? ` ${report.version}` : ""}
                              </dd>
                            </div>
                            <div>
                              <dt>Adapter</dt>
                              <dd>{adapter.adapterId}</dd>
                            </div>
                            {report.executableHash ? (
                              <div>
                                <dt>Executable</dt>
                                <dd title={report.executableHash}>
                                  {shortHash(report.executableHash)}
                                </dd>
                              </div>
                            ) : null}
                            {report.profileVersion ? (
                              <div>
                                <dt>Profiles</dt>
                                <dd>{report.profileVersion}</dd>
                              </div>
                            ) : null}
                          </dl>
                        </li>
                      );
                    })}
                  </ul>
                )}
              </details>
            ) : null}
            <button
              className="button button--secondary"
              type="button"
              aria-labelledby={exportLabelId}
              aria-disabled={!exportAvailable || undefined}
              aria-describedby={!exportAvailable ? exportReasonId : undefined}
              onClick={(event) => {
                if (!exportAvailable) {
                  event.preventDefault();
                  return;
                }
                onExport();
              }}
              disabled={isBusy}
            >
              <Download aria-hidden="true" />
              <span className="validation-button-copy">
                <span id={exportLabelId}>Export JSON plan</span>
                {planAvailable && !exportAvailable ? (
                  <small id={exportReasonId}>
                    {exportBlockReason ?? "Validate pending edits first"}
                  </small>
                ) : null}
              </span>
            </button>
          </div>
        </details>

        {showValidationAction ? (
          <button
            className="button button--primary button--large"
            type="button"
            aria-labelledby={validationLabelId}
            aria-describedby={
              !validationAvailable ? validationReasonId : undefined
            }
            aria-disabled={!validationAvailable || undefined}
            onClick={(event) => {
              if (!validationAvailable) {
                event.preventDefault();
                return;
              }
              onValidate();
            }}
            disabled={isBusy}
          >
            {isValidating ? (
              <LoaderCircle className="spin" aria-hidden="true" />
            ) : recalculationNeeded ? (
              <RefreshCw aria-hidden="true" />
            ) : (
              <ListChecks aria-hidden="true" />
            )}
            <span className="validation-button-copy">
              <span id={validationLabelId}>
                {isValidating
                  ? recalculationNeeded
                    ? "Recalculating…"
                    : "Validating…"
                  : recalculationNeeded
                    ? "Recalculate plan"
                    : "Validate plan"}
              </span>
              {recalculationNeeded ? (
                <small>
                  {colorNeedsRecalculation
                    ? "Apply approved color decisions"
                    : "Apply pending printer, spool, or plan changes"}
                </small>
              ) : null}
              {!validationAvailable ? (
                <small id={validationReasonId}>
                  Resolve the library or source-file requirement first
                </small>
              ) : null}
            </span>
          </button>
        ) : (
          <button
            className="button button--primary button--large button--conversion"
            type="button"
            aria-labelledby={conversionLabelId}
            aria-describedby={
              !conversionAvailable ? conversionReasonId : undefined
            }
            aria-disabled={!conversionAvailable || undefined}
            onClick={(event) => {
              if (!conversionAvailable) {
                event.preventDefault();
                return;
              }
              onApprove?.();
            }}
            disabled={isBusy}
          >
            <FolderOutput aria-hidden="true" />
            <span className="conversion-button-copy">
              <span id={conversionLabelId}>{conversionLabel}</span>
              {!conversionAvailable ? (
                <small id={conversionReasonId}>
                  {conversionBlockReason ??
                    "Conversion is unavailable: no qualified writer adapter is installed."}
                </small>
              ) : null}
            </span>
          </button>
        )}
      </div>
    </footer>
  );
}

function adapterTargetName(target: string) {
  switch (target) {
    case "u1_direct":
      return "Snapmaker U1 · Direct Spools";
    case "u1_full_spectrum":
      return "Snapmaker U1 · Full Spectrum";
    case "a1_mini_mono":
      return "Bambu Lab A1 mini · Mono";
    default:
      return target;
  }
}

function adapterReportSummary(report: unknown): {
  version: string | null;
  executableHash: string | null;
  profileVersion: string | null;
} {
  if (!report || typeof report !== "object") {
    return { version: null, executableHash: null, profileVersion: null };
  }
  const record = report as Record<string, unknown>;
  const profilePack =
    record.profilePack && typeof record.profilePack === "object"
      ? (record.profilePack as Record<string, unknown>)
      : null;
  return {
    version:
      typeof record.applicationVersion === "string"
        ? record.applicationVersion
        : null,
    executableHash:
      typeof record.executableSha256 === "string"
        ? record.executableSha256
        : null,
    profileVersion:
      profilePack && typeof profilePack.version === "string"
        ? profilePack.version
        : null,
  };
}

function shortHash(hash: string) {
  return hash.length > 18 ? `${hash.slice(0, 12)}…${hash.slice(-6)}` : hash;
}
