import { useRef, type ChangeEvent, type RefObject } from "react";
import {
  Box,
  CheckCircle2,
  FileSearch,
  Layers3,
  LoaderCircle,
  PackageOpen,
  Upload,
  X,
} from "lucide-react";

import type {
  AnalysisResult,
  ProjectSelection,
  ProjectSummary,
} from "../types";

interface ProjectHeaderProps {
  summary: ProjectSummary | null;
  analysisSource: AnalysisResult["source"] | null;
  pendingSelection: ProjectSelection | null;
  isAnalyzing: boolean;
  isCancellingAnalysis: boolean;
  isNative: boolean;
  browserFileInputRef?: RefObject<HTMLInputElement | null>;
  onBrowserFile: (file: File) => void;
  onChooseNativeFile: () => void;
  onAnalyze: () => void;
  onCancelAnalysis: () => void;
  analysisAvailable?: boolean;
  analysisBlockReason?: string;
}

export function ProjectHeader({
  summary,
  analysisSource,
  pendingSelection,
  isAnalyzing,
  isCancellingAnalysis,
  isNative,
  browserFileInputRef,
  onBrowserFile,
  onChooseNativeFile,
  onAnalyze,
  onCancelAnalysis,
  analysisAvailable = true,
  analysisBlockReason,
}: ProjectHeaderProps) {
  const internalBrowserFileInputRef = useRef<HTMLInputElement>(null);
  const activeBrowserFileInputRef =
    browserFileInputRef ?? internalBrowserFileInputRef;
  const status = isAnalyzing
    ? "Analyzing project…"
    : pendingSelection
      ? "Ready to analyze"
      : summary
        ? analysisSource === "browser-demo"
          ? "Demo analysis complete"
          : "Analysis complete"
        : "No project selected";

  const handleFileChange = (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    if (file) onBrowserFile(file);
  };

  return (
    <header
      className={`project-header ${
        summary
          ? "project-header--analyzed"
          : pendingSelection
            ? "project-header--pending"
            : "project-header--empty"
      }`}
      aria-busy={isAnalyzing}
    >
      <div className="project-identification">
        <div className="project-cube" aria-hidden="true">
          <Box />
        </div>
        <div className="project-copy">
          <p className="project-file-label">Project file</p>
          <h1>
            {pendingSelection?.fileName ??
              summary?.fileName ??
              "No project selected"}
          </h1>
          <p
            className={`analysis-status ${
              isAnalyzing
                ? "is-busy"
                : analysisSource === "browser-demo" && summary
                  ? "is-demo"
                  : !summary && !pendingSelection
                    ? "is-idle"
                    : ""
            }`}
          >
            {isAnalyzing ? (
              <LoaderCircle className="spin" aria-hidden="true" />
            ) : pendingSelection ? (
              <FileSearch aria-hidden="true" />
            ) : summary ? (
              <CheckCircle2 aria-hidden="true" />
            ) : (
              <Upload aria-hidden="true" />
            )}
            {status}
          </p>
          <p className="project-metadata">
            {pendingSelection
              ? "Selected .3mf · analysis has not started"
              : summary
                ? `${analysisSource === "browser-demo" ? "Browser demo · " : ""}${summary.sourceDialect}`
                : "Choose a .3mf project to begin."}
          </p>
          {summary ? (
            <details className="project-analysis-details">
              <summary>Analysis details</summary>
              <dl>
                <div>
                  <dt>Source</dt>
                  <dd>{summary.sourceApplication}</dd>
                </div>
                <div>
                  <dt>Project fingerprint</dt>
                  <dd>{summary.sourceHash}</dd>
                </div>
                <div>
                  <dt>File size</dt>
                  <dd>{formatBytes(summary.sourceByteSize)}</dd>
                </div>
                <div>
                  <dt>Instances with proven bounds</dt>
                  <dd>
                    {summary.boundedInstanceCount} / {summary.instanceCount}
                  </dd>
                </div>
                <div>
                  <dt>Printable parts</dt>
                  <dd>
                    {summary.printablePartCount} / {summary.partCount}
                  </dd>
                </div>
                <div>
                  <dt>Painted parts</dt>
                  <dd>{summary.paintedPartCount}</dd>
                </div>
                <div>
                  <dt>Unused filament slots</dt>
                  <dd>{summary.unusedFilamentCount}</dd>
                </div>
                <div>
                  <dt>Detected alternative plates</dt>
                  <dd>{summary.alternativePlateCount}</dd>
                </div>
              </dl>
            </details>
          ) : null}
        </div>
      </div>

      {summary ? (
        <div className="project-counts" aria-label="Source project summary">
          <div className="project-count">
            <Layers3 aria-hidden="true" />
            <strong>{summary.sourcePlateCount}</strong>
            <span>source plates</span>
          </div>
          <div className="project-count">
            <PackageOpen aria-hidden="true" />
            <strong>{summary.objectCount}</strong>
            <span>objects</span>
          </div>
          <div className="project-count">
            <span className="spool-count-icon" aria-hidden="true" />
            <strong>{summary.usedFilamentCount}</strong>
            <span>used filaments</span>
          </div>
        </div>
      ) : null}

      <div className="project-file-actions">
        {isNative ? (
          <button
            className={`button button--compact ${
              pendingSelection || summary
                ? "button--secondary"
                : "button--primary"
            }`}
            type="button"
            onClick={onChooseNativeFile}
            disabled={isAnalyzing}
          >
            <Upload aria-hidden="true" />
            {pendingSelection
              ? "Change 3MF"
              : summary
                ? "Open another 3MF"
                : "Open 3MF"}
          </button>
        ) : (
          <>
            <input
              ref={activeBrowserFileInputRef}
              hidden
              id="project-file-input"
              name="project-file"
              type="file"
              accept=".3mf,model/3mf"
              onChange={handleFileChange}
              disabled={isAnalyzing}
            />
            <button
              className={`button button--compact ${
                pendingSelection || summary
                  ? "button--secondary"
                  : "button--primary"
              }`}
              type="button"
              onClick={() => activeBrowserFileInputRef.current?.click()}
              disabled={isAnalyzing}
            >
              <Upload aria-hidden="true" />
              {pendingSelection
                ? "Change 3MF"
                : summary
                  ? "Open another 3MF"
                  : "Open 3MF"}
            </button>
          </>
        )}
        {isAnalyzing ? (
          <button
            className="button button--danger button--compact"
            type="button"
            onClick={onCancelAnalysis}
            disabled={isCancellingAnalysis}
          >
            <X aria-hidden="true" />
            {isCancellingAnalysis ? "Canceling…" : "Cancel Analysis"}
          </button>
        ) : pendingSelection ? (
          <span className="project-analysis-action">
            <button
              className="button button--dark button--compact"
              type="button"
              aria-disabled={!analysisAvailable || undefined}
              aria-describedby={
                !analysisAvailable && analysisBlockReason
                  ? "project-analysis-block-reason"
                  : undefined
              }
              onClick={(event) => {
                if (!analysisAvailable) {
                  event.preventDefault();
                  return;
                }
                onAnalyze();
              }}
            >
              <FileSearch aria-hidden="true" />
              Analyze Project
            </button>
            {!analysisAvailable && analysisBlockReason ? (
              <small id="project-analysis-block-reason">
                {analysisBlockReason}
              </small>
            ) : null}
          </span>
        ) : null}
      </div>
    </header>
  );
}

function formatBytes(bytes: number) {
  if (!Number.isFinite(bytes) || bytes < 0) return "Unknown";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unit = units[0];
  for (let index = 1; index < units.length && value >= 1024; index += 1) {
    value /= 1024;
    unit = units[index];
  }
  return `${value.toFixed(value >= 100 ? 0 : 1)} ${unit}`;
}
