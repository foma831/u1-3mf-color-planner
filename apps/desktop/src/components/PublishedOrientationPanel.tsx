import { useEffect, useId, useMemo, useState } from "react";
import { LoaderCircle, Rotate3D, Sparkles } from "lucide-react";

import {
  listPublishedOrientationPlates,
  optimizePublishedPlates,
} from "../services/project-analysis";
import type {
  AdhesionMode,
  DetectedAdhesionPolicy,
  OrientationPlateOption,
  PublishedConversionArtifact,
  PublishedOrientationResult,
} from "../types";

interface PublishedOrientationPanelProps {
  artifacts: PublishedConversionArtifact[];
  detectedAdhesion?: DetectedAdhesionPolicy;
  onResultsChange?: (results: PublishedOrientationResult[]) => void;
}

interface ArtifactSelection {
  artifact: PublishedConversionArtifact;
  plates: OrientationPlateOption[];
  selectedPlateIds: number[];
}

interface SkippedOptimization {
  artifact: PublishedConversionArtifact;
  reason: string;
}

function percentage(value: number) {
  return `${(value * 100).toFixed(1)}%`;
}

function skippedOptimizationReason(message: string) {
  if (message.includes("no support-aware orientation layout fit")) {
    return "No safe support-aware layout fits this printer bed within the bounded search. The original published file remains selected for printing.";
  }
  return `The optimized copy was not created. The original published file remains selected for printing. ${message}`;
}

const adhesionOptions: Array<{
  value: AdhesionMode;
  label: string;
  description: string;
}> = [
  {
    value: "standard",
    label: "Standard",
    description: "Keep Auto Brim and reserve its complete 18 mm envelope.",
  },
  {
    value: "reliable",
    label: "Reliable (recommended)",
    description:
      "Use a risk-sized outer brim and slow the first three layers without a raft.",
  },
  {
    value: "maximum",
    label: "Maximum",
    description:
      "Add a two-layer raft, an 18 mm outer brim, and four slow starting layers.",
  },
];

export function PublishedOrientationPanel({
  artifacts,
  detectedAdhesion,
  onResultsChange,
}: PublishedOrientationPanelProps) {
  const instanceId = useId().replace(/:/g, "");
  const [enabled, setEnabled] = useState(false);
  const restoredAdhesionMode =
    detectedAdhesion?.mode === "standard" ||
    detectedAdhesion?.mode === "reliable" ||
    detectedAdhesion?.mode === "maximum"
      ? detectedAdhesion.mode
      : "reliable";
  const [adhesionMode, setAdhesionMode] =
    useState<AdhesionMode>(restoredAdhesionMode);
  const [selections, setSelections] = useState<ArtifactSelection[]>([]);
  const [results, setResults] = useState<PublishedOrientationResult[]>([]);
  const [skipped, setSkipped] = useState<SkippedOptimization[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [isOptimizing, setIsOptimizing] = useState(false);
  const [error, setError] = useState("");
  const artifactIdentity = useMemo(
    () => artifacts.map((artifact) => `${artifact.path}:${artifact.sha256}`).join("|"),
    [artifacts],
  );
  const selectedCount = selections.reduce(
    (total, selection) => total + selection.selectedPlateIds.length,
    0,
  );

  useEffect(() => {
    setEnabled(false);
    setAdhesionMode(restoredAdhesionMode);
    setSelections([]);
    setResults([]);
    setSkipped([]);
    setError("");
    onResultsChange?.([]);
  }, [artifactIdentity, onResultsChange, restoredAdhesionMode]);

  const enable = async () => {
    setEnabled(true);
    setIsLoading(true);
    setError("");
    try {
      const inspected = await Promise.all(
        artifacts.map(async (artifact) => {
          const plates = await listPublishedOrientationPlates(artifact);
          return {
            artifact,
            plates,
            selectedPlateIds: plates.map((plate) => plate.id),
          };
        }),
      );
      setSelections(inspected.filter((selection) => selection.plates.length > 0));
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : String(reason);
      setError(`Generated plates could not be inspected. ${message}`);
    } finally {
      setIsLoading(false);
    }
  };

  const optimize = async () => {
    if (selectedCount === 0) return;
    setIsOptimizing(true);
    setError("");
    setSkipped([]);
    const completed: PublishedOrientationResult[] = [];
    const notOptimized: SkippedOptimization[] = [];
    for (const selection of selections) {
      if (selection.selectedPlateIds.length === 0) continue;
      try {
        const result = await optimizePublishedPlates(
          selection.artifact,
          selection.selectedPlateIds,
          adhesionMode,
        );
        if (result) completed.push(result);
      } catch (reason) {
        const message = reason instanceof Error ? reason.message : String(reason);
        notOptimized.push({
          artifact: selection.artifact,
          reason: skippedOptimizationReason(message),
        });
      }
    }
    const completedBySource = new Map(
      completed.map((result) => [result.sourcePath, result]),
    );
    const merged = [
      ...results.filter((result) => !completedBySource.has(result.sourcePath)),
      ...completed,
    ];
    setResults(merged);
    setSkipped(notOptimized);
    onResultsChange?.(merged);
    setIsOptimizing(false);
  };

  return (
    <section
      className="published-orientation"
      aria-labelledby={`${instanceId}-heading`}
    >
      <header className="published-orientation__heading">
        <Rotate3D aria-hidden="true" />
        <div>
          <h3 id={`${instanceId}-heading`}>Optimize generated plates</h3>
          <p>
            Work from the verified project files above and create separate
            support-optimized copies. Published files and bundle checksums stay
            unchanged. The selected adhesion mode also controls the exact
            first-layer envelope used while packing every model.
          </p>
        </div>
      </header>

      <label className="published-orientation__toggle">
        <input
          type="checkbox"
          checked={enabled}
          disabled={isLoading || isOptimizing}
          onChange={(event) => {
            if (event.target.checked) void enable();
            else {
              setEnabled(false);
              setSelections([]);
              setSkipped([]);
              setError("");
            }
          }}
        />
        <span>Enable optional support-aware optimization after conversion</span>
      </label>

      {isLoading ? (
        <p className="published-orientation__status" role="status">
          <LoaderCircle className="spin" aria-hidden="true" />
          Inspecting generated plates…
        </p>
      ) : null}

      {enabled && !isLoading ? (
        <div className="published-orientation__files">
          <fieldset
            className="published-orientation__adhesion"
            disabled={isOptimizing}
          >
            <legend>Bed adhesion</legend>
            {detectedAdhesion?.mode === "custom" ? (
              <p className="published-orientation__profile-warning" role="status">
                The source uses a modified U1 Planner adhesion profile
                {detectedAdhesion.profileName
                  ? ` (${detectedAdhesion.profileName})`
                  : ""}
                . Reliable is selected as a safe default; optimizing will
                normalize the copy to the selected versioned profile.
              </p>
            ) : detectedAdhesion?.mode !== undefined &&
              detectedAdhesion.mode !== "none" ? (
              <p className="published-orientation__profile-note" role="status">
                Restored {detectedAdhesion.mode} adhesion from the source 3MF.
              </p>
            ) : null}
            {adhesionOptions.map((option) => {
              const descriptionId = `${instanceId}-${option.value}-description`;
              return (
                <label key={option.value}>
                  <input
                    type="radio"
                    name={`${instanceId}-adhesion`}
                    value={option.value}
                    checked={adhesionMode === option.value}
                    aria-describedby={descriptionId}
                    onChange={() => {
                      setAdhesionMode(option.value);
                      setResults([]);
                      setSkipped([]);
                      onResultsChange?.([]);
                    }}
                  />
                  <span>
                    <strong>{option.label}</strong>
                    <small id={descriptionId}>{option.description}</small>
                  </span>
                </label>
              );
            })}
            <p className="published-orientation__profile-note">
              U1 copies use a versioned U1 Planner project profile. Installed
              Snapmaker system presets are never overwritten. When supports
              are enabled, the copy also uses Tree Hybrid with model-origin
              branches, a denser interface, reinforced tree walls, and
              conservative support speeds.
            </p>
          </fieldset>
          {selections.map((selection, selectionIndex) => (
            <fieldset key={selection.artifact.path} disabled={isOptimizing}>
              <legend>{selection.artifact.fileName}</legend>
              {selection.plates.map((plate) => {
                const checked = selection.selectedPlateIds.includes(plate.id);
                return (
                  <label key={plate.id}>
                    <input
                      type="checkbox"
                      checked={checked}
                      onChange={(event) => {
                        setSelections((current) =>
                          current.map((candidate, candidateIndex) => {
                            if (candidateIndex !== selectionIndex) return candidate;
                            return {
                              ...candidate,
                              selectedPlateIds: event.target.checked
                                ? [...candidate.selectedPlateIds, plate.id].sort(
                                    (left, right) => left - right,
                                  )
                                : candidate.selectedPlateIds.filter(
                                    (plateId) => plateId !== plate.id,
                                  ),
                            };
                          }),
                        );
                      }}
                    />
                    <span>
                      {plate.name} · {plate.printableInstanceCount}{" "}
                      {plate.printableInstanceCount === 1
                        ? "instance"
                        : "instances"}
                    </span>
                  </label>
                );
              })}
            </fieldset>
          ))}
          <button
            className="button button--primary"
            type="button"
            disabled={selectedCount === 0 || isOptimizing}
            onClick={() => void optimize()}
          >
            {isOptimizing ? (
              <LoaderCircle className="spin" aria-hidden="true" />
            ) : (
              <Sparkles aria-hidden="true" />
            )}
            {isOptimizing
              ? "Optimizing generated copies…"
              : `Optimize ${selectedCount} selected ${selectedCount === 1 ? "plate" : "plates"}`}
          </button>
        </div>
      ) : null}

      {results.length > 0 ? (
        <div className="published-orientation__results" role="status">
          <strong>Optimized copies created</strong>
          <ul>
            {results.map((result) => {
              const totalSource = result.reports.reduce(
                (total, report) => total + report.source_score,
                0,
              );
              const totalSelected = result.reports.reduce(
                (total, report) => total + report.selected_score,
                0,
              );
              const improvement =
                totalSource > 0 ? 1 - totalSelected / totalSource : 0;
              const maximumRisk = result.reports
                .map((report) => report.maximum_adhesion_risk)
                .sort((left, right) => right.score - left.score)[0];
              return (
                <li key={result.destinationPath}>
                  <code>{result.destinationPath}</code>
                  <span>Combined score improvement {percentage(improvement)}</span>
                  <span>
                    Adhesion: {result.reports[0]?.adhesion_mode ?? adhesionMode} ·
                    maximum risk {maximumRisk?.level ?? "unknown"} (
                    {maximumRisk ? maximumRisk.score.toFixed(0) : "?"}/100)
                  </span>
                </li>
              );
            })}
          </ul>
        </div>
      ) : null}

      {skipped.length > 0 ? (
        <div className="published-orientation__skipped" role="status">
          <strong>
            {results.length > 0
              ? "Other generated files kept their original layout"
              : "No optimized copies were created"}
          </strong>
          <ul>
            {skipped.map(({ artifact: skippedArtifact, reason }) => (
              <li key={skippedArtifact.path}>
                <code>{skippedArtifact.path}</code>
                <span>{reason}</span>
              </li>
            ))}
          </ul>
        </div>
      ) : null}

      {error ? (
        <p className="published-orientation__error" role="alert">
          {error}
        </p>
      ) : null}
    </section>
  );
}
