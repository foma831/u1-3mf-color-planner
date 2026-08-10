import {
  CheckCircle2,
  Download,
  ListChecks,
  LoaderCircle,
  LockKeyhole,
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
  a1MiniEnabled: boolean;
  a1MiniNeedsRecalculation: boolean;
  a1MiniRoutingFixed: boolean;
  planNeedsRecalculation: boolean;
  colorNeedsRecalculation: boolean;
  onA1MiniChange: (enabled: boolean) => void;
  currentT4SpoolId: string;
  t4Options: Array<{ id: string; label: string }>;
  onCurrentT4Change: (spoolId: string) => void;
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
  a1MiniEnabled,
  a1MiniNeedsRecalculation,
  a1MiniRoutingFixed,
  planNeedsRecalculation,
  colorNeedsRecalculation,
  onA1MiniChange,
  currentT4SpoolId,
  t4Options,
  onCurrentT4Change,
  conversionAvailable,
  conversionAdapters,
  isCheckingConversion,
  conversionLabel = "Approve & Convert",
  conversionBlockReason,
  onApprove,
}: BottomActionRailProps) {
  const recalculationNeeded =
    planNeedsRecalculation || colorNeedsRecalculation;
  return (
    <footer className="bottom-action-rail">
      <div className="bottom-plan-controls">
        <label className="current-t4-control">
          <span>Currently loaded T4</span>
          <select
            aria-label="Currently loaded T4 spool"
            value={currentT4SpoolId}
            disabled={isBusy || !planAvailable}
            onChange={(event) => onCurrentT4Change(event.target.value)}
          >
            <option value="">Unknown / not confirmed</option>
            {t4Options.map((option) => (
              <option key={option.id} value={option.id}>
                {option.label}
              </option>
            ))}
          </select>
        </label>
        <fieldset className="a1-mini-routing">
          <legend className="visually-hidden">Optional A1 mini routing</legend>
          <div className="a1-mini-routing__controls">
            <label className="a1-mini-toggle">
              <input
                type="checkbox"
                name="a1-mini-enabled"
                checked={a1MiniEnabled}
                disabled={isBusy || !planAvailable || a1MiniRoutingFixed}
                aria-describedby="a1-mini-routing-status"
                onChange={(event) => onA1MiniChange(event.target.checked)}
              />
              <span>Use Bambu Lab A1 mini for eligible mono parts</span>
            </label>
            {a1MiniNeedsRecalculation ? (
              <button
                className="button button--secondary a1-mini-recalculate"
                type="button"
                onClick={onValidate}
                disabled={isBusy || !validationAvailable}
              >
                {isValidating ? (
                  <LoaderCircle className="spin" aria-hidden="true" />
                ) : (
                  <RefreshCw aria-hidden="true" />
                )}
                {isValidating ? "Recalculating…" : "Recalculate plan"}
              </button>
            ) : null}
          </div>
          <p
            id="a1-mini-routing-status"
            className={
              a1MiniNeedsRecalculation
                ? "a1-mini-routing__status is-pending"
                : "a1-mini-routing__status"
            }
            aria-live="polite"
            aria-atomic="true"
          >
            {a1MiniRoutingFixed
              ? a1MiniEnabled
                ? "Browser demo routing is fixed to the included A1 mini preview. Open the desktop app to change printer assignments."
                : "Browser demo routing is fixed to the U1 preview. Open the desktop app to change printer assignments."
              : a1MiniNeedsRecalculation
              ? "Pending recalculation. Printer assignments still show the previous plan."
              : a1MiniEnabled
                ? "A1 mini routing is applied to this plan."
                : "Eligible mono parts stay on the U1."}
          </p>
        </fieldset>
        <p className="mapping-warning">
          <TriangleAlert aria-hidden="true" />
          Filament mapping must be verified in each target slicer before
          printing.
        </p>
      </div>
      <div className="bottom-actions">
        {isCheckingConversion || conversionAdapters.length > 0 ? (
          <details className="adapter-capabilities">
            <summary>
              {isCheckingConversion ? (
                <LoaderCircle className="spin" aria-hidden="true" />
              ) : conversionAdapters.every((adapter) => adapter.available) ? (
                <CheckCircle2 aria-hidden="true" />
              ) : (
                <TriangleAlert aria-hidden="true" />
              )}
              Writer adapter checks
            </summary>
            {isCheckingConversion ? (
              <p>Checking installed slicers, profiles, and qualification evidence…</p>
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
          className="button button--secondary button--large"
          type="button"
          onClick={onExport}
          disabled={isBusy || !exportAvailable}
        >
          <Download aria-hidden="true" />
          <span className="validation-button-copy">
            <span>Export JSON Plan</span>
            {planAvailable && !exportAvailable ? (
              <small>{exportBlockReason ?? "Validate pending edits first"}</small>
            ) : null}
          </span>
        </button>
        <button
          className="button button--dark button--large"
          type="button"
          aria-disabled={!validationAvailable}
          onClick={(event) => {
            if (!validationAvailable) {
              event.preventDefault();
              return;
            }
            onValidate();
          }}
          disabled={isBusy || !validationAvailable}
        >
          {isValidating ? (
            <LoaderCircle className="spin" aria-hidden="true" />
          ) : recalculationNeeded ? (
            <RefreshCw aria-hidden="true" />
          ) : (
            <ListChecks aria-hidden="true" />
          )}
          <span className="validation-button-copy">
            <span>
              {isValidating
                ? recalculationNeeded
                  ? "Recalculating…"
                  : "Validating…"
                : recalculationNeeded
                  ? "Recalculate Plan"
                  : "Validate Choices"}
            </span>
            {recalculationNeeded ? (
              <small>
                {colorNeedsRecalculation
                  ? "Apply approved color decisions"
                  : "Apply pending printer, spool, or plan changes"}
              </small>
            ) : null}
            {!validationAvailable ? <small>Analyze a project first</small> : null}
          </span>
        </button>
        <button
          className="button button--primary button--large button--conversion"
          type="button"
          aria-disabled={!conversionAvailable}
          onClick={(event) => {
            if (!conversionAvailable) {
              event.preventDefault();
              return;
            }
            onApprove?.();
          }}
          disabled={isBusy && conversionAvailable}
        >
          <LockKeyhole aria-hidden="true" />
          <span className="conversion-button-copy">
            <span>{conversionLabel}</span>
            {!conversionAvailable ? (
              <small>
                {conversionBlockReason ??
                  "Conversion is unavailable: no qualified writer adapter is installed."}
              </small>
            ) : null}
          </span>
        </button>
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
