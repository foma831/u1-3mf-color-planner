import { useEffect, useMemo, useState, type FormEvent } from "react";
import { ArrowRight, CheckCircle2, Palette, TriangleAlert } from "lucide-react";

import type {
  ProjectDirectPaletteChoice,
  ProjectPlan,
} from "../types";
import {
  inStockSpools,
  projectDirectPaletteToolheads,
} from "../utils/plan";
import { deltaE00 } from "../utils/color";
import { ColorSwatch } from "./ColorSwatch";

interface ProjectDirectPaletteProps {
  plan: ProjectPlan;
  isStale: boolean;
  onApply: (choices: ProjectDirectPaletteChoice[]) => void;
}

function draftSignature(plan: ProjectPlan) {
  return JSON.stringify(
    plan.projectDirectPalette.mappings.map((mapping) => [
      mapping.id,
      mapping.physicalIdentityId,
      mapping.selectedSpoolId,
      mapping.materialSubstitutionAcknowledged,
    ]),
  );
}

export function ProjectDirectPalette({
  plan,
  isStale,
  onApply,
}: ProjectDirectPaletteProps) {
  const { projectDirectPalette: palette } = plan;
  const signature = draftSignature(plan);
  const [selectedSpools, setSelectedSpools] = useState<Record<string, string>>(
    () =>
      Object.fromEntries(
        palette.mappings.map((mapping) => [mapping.id, mapping.selectedSpoolId]),
      ),
  );
  const [materialApprovals, setMaterialApprovals] = useState<
    Record<string, boolean>
  >(() =>
    Object.fromEntries(
      palette.mappings.map((mapping) => [
        mapping.id,
        mapping.materialSubstitutionAcknowledged,
      ]),
    ),
  );

  useEffect(() => {
    setSelectedSpools(
      Object.fromEntries(
        palette.mappings.map((mapping) => [mapping.id, mapping.selectedSpoolId]),
      ),
    );
    setMaterialApprovals(
      Object.fromEntries(
        palette.mappings.map((mapping) => [
          mapping.id,
          mapping.materialSubstitutionAcknowledged,
        ]),
      ),
    );
  }, [palette.mappings, signature]);

  const selectableSpools = useMemo(
    () =>
      inStockSpools(plan.spools).sort((left, right) =>
        left.colorName.localeCompare(right.colorName),
      ),
    [plan.spools],
  );
  const spoolById = useMemo(
    () => new Map(selectableSpools.map((spool) => [spool.id, spool])),
    [selectableSpools],
  );
  const physicalIdentityLabels = useMemo(() => {
    const ids = palette.mappings
      .map((mapping) => mapping.physicalIdentityId)
      .filter((id, index, values) => values.indexOf(id) === index);
    return new Map(ids.map((id, index) => [id, index + 1]));
  }, [palette.mappings]);
  const physicalIdentityUseCounts = useMemo(() => {
    const counts = new Map<string, number>();
    for (const mapping of palette.mappings) {
      counts.set(
        mapping.physicalIdentityId,
        (counts.get(mapping.physicalIdentityId) ?? 0) + 1,
      );
    }
    return counts;
  }, [palette.mappings]);
  const choices = palette.mappings.map((mapping) => ({
    mappingId: mapping.id,
    spoolId: selectedSpools[mapping.id] ?? "",
    materialSubstitutionAcknowledged:
      materialApprovals[mapping.id] ?? false,
  }));
  const toolheadBySpool = projectDirectPaletteToolheads(plan, choices);

  const applyPalette = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!event.currentTarget.reportValidity()) return;
    onApply(choices);
  };

  return (
    <section
      className={`project-direct-palette ${palette.available ? "" : "is-unavailable"}`}
      aria-labelledby="project-direct-palette-heading"
    >
      <header className="project-direct-palette__heading">
        <span className="project-direct-palette__icon" aria-hidden="true">
          {palette.available ? <Palette /> : <TriangleAlert />}
        </span>
        <div>
          <h3 id="project-direct-palette-heading">Project-wide Direct Spools</h3>
          <p>
            Replace every original effective material-color pair across all included
            source scopes with physical spools from your in-stock library.
          </p>
        </div>
        <strong className="project-direct-palette__count">
          {palette.effectivePairCount} semantic · {palette.directPairCount}/
          {palette.maximumPairCount} physical
        </strong>
      </header>

      {!palette.available ? (
        <div className="project-direct-palette__unavailable">
          <TriangleAlert aria-hidden="true" />
          <p>{palette.unavailableReason}</p>
        </div>
      ) : (
        <form onSubmit={applyPalette}>
          <fieldset>
            <legend>Map the complete project palette</legend>
            <p className="project-direct-palette__instructions">
              Choose one physical spool for each original semantic pair. Rows with
              the same declared material, color, and source profile stay linked to
              one physical identity and one toolhead, while material-risk approval
              remains separate for every source role. Intentional color changes are
              allowed.
            </p>
            {selectableSpools.length === 0 ? (
              <p className="project-direct-palette__empty" role="status">
                No in-stock spools are available. Mark a spool In stock in the
                Filament Library, then recalculate the project.
              </p>
            ) : null}
            <ol className="project-direct-palette__mappings" role="list">
              {palette.mappings.map((mapping) => {
                const selectId = `${mapping.id}-project-spool`;
                const riskId = `${mapping.id}-project-material-risk`;
                const selectedSpool = spoolById.get(
                  selectedSpools[mapping.id] ?? "",
                );
                const changesMaterial = Boolean(
                  selectedSpool &&
                    selectedSpool.material !== mapping.sourceMaterial,
                );
                const toolhead = selectedSpool
                  ? toolheadBySpool.get(selectedSpool.id)
                  : undefined;
                const cmyxGroups = [
                  ...mapping.currentCmyxResults
                    .reduce((groups, result) => {
                      const identity = JSON.stringify([
                        result.recipe,
                        result.predictedHex,
                        result.deltaE00,
                        result.confidence,
                      ]);
                      const existing = groups.get(identity);
                      if (existing) existing.scopeNames.push(result.scopeName);
                      else groups.set(identity, { result, scopeNames: [result.scopeName] });
                      return groups;
                    }, new Map<string, { result: (typeof mapping.currentCmyxResults)[number]; scopeNames: string[] }>())
                    .values(),
                ];
                const directDeltaE00 = selectedSpool
                  ? deltaE00(mapping.sourceHex, selectedSpool.hex)
                  : null;

                return (
                  <li key={mapping.id} className="project-direct-palette__mapping">
                    <div className="project-direct-palette__source">
                      <span>Original project pair</span>
                      <span className="mapping-swatch-line">
                        <ColorSwatch hex={mapping.sourceHex} size="large" />
                        <code>{mapping.sourceHex}</code>
                      </span>
                      <strong>
                        {mapping.sourceMaterial} · {mapping.sourceRole}
                      </strong>
                      <small>
                        {mapping.sourceSlots.length > 0
                          ? `Source ${mapping.sourceSlots.join(", ")}`
                          : "No source slot label"}
                        {mapping.sourceProfileIds.length > 0
                          ? ` · ${mapping.sourceProfileIds.join(", ")}`
                          : " · Unknown source profile"}
                      </small>
                      <small>Used by {mapping.usedBy.join(", ")}</small>
                      <small>
                        {physicalIdentityUseCounts.get(mapping.physicalIdentityId)! > 1
                          ? "Shared physical identity"
                          : "Physical identity"}{" "}
                        {physicalIdentityLabels.get(mapping.physicalIdentityId)}
                      </small>
                    </div>

                    <ArrowRight className="project-direct-palette__arrow" aria-hidden="true" />

                    <div className="project-direct-palette__cmyx">
                      <span>
                        Current CMY+X
                        {cmyxGroups.length > 1 ? (
                          <strong>Varies by scope</strong>
                        ) : null}
                      </span>
                      {cmyxGroups.length > 0 ? (
                        <ul role="list">
                          {cmyxGroups.map(({ result, scopeNames }) => (
                            <li
                              key={JSON.stringify([
                                result.recipe,
                                result.predictedHex,
                                result.deltaE00,
                                result.confidence,
                              ])}
                            >
                              {result.predictedHex ? (
                                <ColorSwatch hex={result.predictedHex} size="medium" />
                              ) : (
                                <span className="project-direct-palette__missing-swatch">
                                  No color
                                </span>
                              )}
                              <span>
                                <strong>{result.recipe}</strong>
                                <small>
                                  {result.predictedHex ?? "Predicted color unavailable"}
                                  {result.deltaE00 === null
                                    ? " · ΔE00 unavailable"
                                    : ` · ΔE00 ${result.deltaE00.toFixed(1)}`}
                                  {` · ${result.confidence}`}
                                </small>
                                <small>{scopeNames.join(", ")}</small>
                              </span>
                            </li>
                          ))}
                        </ul>
                      ) : (
                        <p className="project-direct-palette__pending">
                          CMY+X result unavailable
                        </p>
                      )}
                    </div>

                    <ArrowRight className="project-direct-palette__arrow" aria-hidden="true" />

                    <div className="project-direct-palette__target">
                      <label htmlFor={selectId}>Physical spool at print time</label>
                      <select
                        id={selectId}
                        name={`project-spool-${mapping.id}`}
                        required
                        value={selectedSpool?.id ?? ""}
                        onChange={(event) => {
                          const linkedMappingIds = palette.mappings
                            .filter(
                              (candidate) =>
                                candidate.physicalIdentityId ===
                                mapping.physicalIdentityId,
                            )
                            .map((candidate) => candidate.id);
                          setSelectedSpools((current) => ({
                            ...current,
                            ...Object.fromEntries(
                              linkedMappingIds.map((mappingId) => [
                                mappingId,
                                event.target.value,
                              ]),
                            ),
                          }));
                          setMaterialApprovals((current) => ({
                            ...current,
                            ...Object.fromEntries(
                              linkedMappingIds.map((mappingId) => [mappingId, false]),
                            ),
                          }));
                        }}
                      >
                        <option value="">Choose an in-stock physical spool</option>
                        {selectableSpools.map((spool) => (
                          <option key={spool.id} value={spool.id}>
                            {spool.colorName} · {spool.material} — {spool.name}
                          </option>
                        ))}
                      </select>
                      {selectedSpool ? (
                        <>
                          <div className="project-direct-palette__actual">
                            <ColorSwatch hex={selectedSpool.hex} size="large" />
                            <span>
                              <strong>{selectedSpool.colorName}</strong>
                              <code>{selectedSpool.hex}</code>
                              <small>{selectedSpool.colorBasis} color</small>
                            </span>
                            <span className="project-direct-palette__toolhead">
                              {toolhead ?? "Review T1–T4"}
                            </span>
                          </div>
                          <small className="project-direct-palette__direct-delta">
                            Preview Direct ΔE00 {directDeltaE00?.toFixed(1)}
                          </small>
                        </>
                      ) : (
                        <p className="project-direct-palette__pending">Unassigned</p>
                      )}
                    </div>

                    {changesMaterial && selectedSpool ? (
                      <div className="material-risk project-direct-palette__risk">
                        <TriangleAlert aria-hidden="true" />
                        <div>
                          <strong>
                            Material change: {mapping.sourceMaterial} →{" "}
                            {selectedSpool.material}
                          </strong>
                          <p>
                            This approval is specific to this original project pair and
                            does not approve any other material substitution.
                          </p>
                          <label htmlFor={riskId}>
                            <input
                              id={riskId}
                              type="checkbox"
                              required
                              checked={materialApprovals[mapping.id] ?? false}
                              onChange={(event) =>
                                setMaterialApprovals((current) => ({
                                  ...current,
                                  [mapping.id]: event.target.checked,
                                }))
                              }
                            />
                            I understand the mechanical-property risk for this pair.
                          </label>
                        </div>
                      </div>
                    ) : null}
                  </li>
                );
              })}
            </ol>
          </fieldset>
          <div className="project-direct-palette__actions">
            <p>
              {isStale
                ? "Project-wide assignments are pending backend recalculation."
                : "Applying changes planning intent only; Recalculate Plan performs authoritative validation."}
            </p>
            <button className="button button--secondary" type="submit">
              <CheckCircle2 aria-hidden="true" />
              Apply project-wide palette
            </button>
          </div>
        </form>
      )}
    </section>
  );
}
