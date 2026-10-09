import { useState } from "react";
import { ArrowRight, CheckCircle2, Plus, TriangleAlert } from "lucide-react";

import type {
  CmyxPaletteOption,
  ColorResolution,
  NewPhysicalSpoolInput,
  PhysicalSpool,
  SpoolMaterial,
} from "../types";
import { ColorSwatch } from "./ColorSwatch";
import { CmyxPalettePicker } from "./CmyxPalettePicker";
import { SpoolInventoryForm } from "./SpoolInventoryForm";

interface ColorResolutionPanelProps {
  resolutions: ColorResolution[];
  spools: PhysicalSpool[];
  onAcceptColor: (resolution: ColorResolution) => void;
  onSelectCmyxColor?: (
    resolution: ColorResolution,
    option: CmyxPaletteOption,
  ) => void;
  onAcceptMaterialSubstitution: (resolution: ColorResolution) => void;
  onAcceptAllSameMaterial: () => void;
  onChooseExistingSpool: (
    resolution: ColorResolution,
    spoolId: string,
    materialSubstitutionAcknowledged: boolean,
  ) => void;
  directUnavailableReason: (resolution: ColorResolution) => string | null;
  onAddSpool: (spool: NewPhysicalSpoolInput) => void;
}

function resolutionIsComplete(resolution: ColorResolution) {
  return (
    resolution.colorApproved &&
    (!resolution.requiresMaterialSubstitution || resolution.materialApproved)
  );
}

function isSpoolMaterial(material: string): material is SpoolMaterial {
  return material === "PLA" || material === "PETG" || material === "PVA";
}

function AlternativeColor({ resolution }: { resolution: ColorResolution }) {
  const alternativeHex = resolution.predictedHex ?? resolution.targetHex;

  return (
    <div
      className="color-resolution__comparison"
      aria-label={`Color comparison for ${resolution.scopeName}`}
    >
      <div className="color-resolution__color" aria-label="Source color">
        <span>Source</span>
        <span className="color-resolution__swatch-line">
          <ColorSwatch hex={resolution.sourceHex} size="large" />
          <span>
            <code>{resolution.sourceHex}</code>
            <small>{resolution.sourceMaterial}</small>
          </span>
        </span>
      </div>
      <ArrowRight aria-hidden="true" />
      <div className="color-resolution__color" aria-label="Selected CMY+X color">
        <span>Selected CMY+X color</span>
        <span className="color-resolution__swatch-line">
          <ColorSwatch hex={alternativeHex} size="large" />
          <span>
            <code>{alternativeHex}</code>
            <small>{resolution.targetMaterial}</small>
          </span>
        </span>
      </div>
    </div>
  );
}

export function ColorResolutionPanel({
  resolutions,
  spools,
  onAcceptColor,
  onSelectCmyxColor,
  onAcceptMaterialSubstitution,
  onAcceptAllSameMaterial,
  onChooseExistingSpool,
  directUnavailableReason,
  onAddSpool,
}: ColorResolutionPanelProps) {
  const [acknowledged, setAcknowledged] = useState<Set<string>>(new Set());
  const [openSpoolForms, setOpenSpoolForms] = useState<Set<string>>(new Set());
  const [selectedSpools, setSelectedSpools] = useState<Map<string, string>>(
    new Map(),
  );
  const [directMaterialAcknowledged, setDirectMaterialAcknowledged] = useState<
    Set<string>
  >(new Set());
  const availableSpools = spools.filter((spool) => spool.available);
  const pendingSameMaterial = resolutions.filter(
    (resolution) =>
      !resolution.requiresMaterialSubstitution && !resolution.colorApproved,
  );
  const pendingCount = resolutions.filter(
    (resolution) => !resolutionIsComplete(resolution),
  ).length;
  const byScope = new Map<string, ColorResolution[]>();
  for (const resolution of resolutions) {
    const group = byScope.get(resolution.scopeId) ?? [];
    group.push(resolution);
    byScope.set(resolution.scopeId, group);
  }

  if (resolutions.length === 0) return null;

  const setMaterialAcknowledged = (candidateId: string, checked: boolean) => {
    setAcknowledged((current) => {
      const next = new Set(current);
      if (checked) next.add(candidateId);
      else next.delete(candidateId);
      return next;
    });
  };

  const toggleSpoolForm = (candidateId: string) => {
    setOpenSpoolForms((current) => {
      const next = new Set(current);
      if (next.has(candidateId)) next.delete(candidateId);
      else next.add(candidateId);
      return next;
    });
  };

  const selectSpool = (candidateId: string, spoolId: string) => {
    setSelectedSpools((current) => {
      const next = new Map(current);
      next.set(candidateId, spoolId);
      return next;
    });
    setDirectMaterialAcknowledged((current) => {
      const next = new Set(current);
      next.delete(candidateId);
      return next;
    });
  };

  const setDirectMaterialRisk = (candidateId: string, checked: boolean) => {
    setDirectMaterialAcknowledged((current) => {
      const next = new Set(current);
      if (checked) next.add(candidateId);
      else next.delete(candidateId);
      return next;
    });
  };

  return (
    <section className="color-resolutions" aria-labelledby="color-resolutions-heading">
      <header className="color-resolutions__heading">
        <div>
          <span className="color-resolutions__icon">
            {pendingCount > 0 ? (
              <TriangleAlert aria-hidden="true" />
            ) : (
              <CheckCircle2 aria-hidden="true" />
            )}
          </span>
          <div>
            <h3 id="color-resolutions-heading">
              {pendingCount > 0 ? "Color decisions required" : "Color decisions accepted"}
            </h3>
            <p>
              Review the closest printable result before recalculating the plan.
              Source colors are never replaced silently.
            </p>
          </div>
        </div>
        {pendingSameMaterial.length > 0 ? (
          <button
            className="button button--secondary button--compact"
            type="button"
            onClick={onAcceptAllSameMaterial}
          >
            <CheckCircle2 aria-hidden="true" />
            Accept all color approximations ({pendingSameMaterial.length})
          </button>
        ) : null}
      </header>

      {[...byScope.entries()].map(([scopeId, scopeResolutions]) => (
        <section className="color-resolution-group" key={scopeId}>
          <div className="color-resolution-group__heading">
            <h4>{scopeResolutions[0].scopeName}</h4>
            <span>
              {scopeResolutions.length} {scopeResolutions.length === 1 ? "color" : "colors"}
            </span>
          </div>
          <div className="color-resolution-list">
            {scopeResolutions.map((resolution) => {
              const complete = resolutionIsComplete(resolution);
              const isAcknowledged = acknowledged.has(resolution.candidateId);
              const spoolFormOpen = openSpoolForms.has(resolution.candidateId);
              const acknowledgementId = `${resolution.candidateId}-material-risk`;
              const dedicatedSpoolMaterial = isSpoolMaterial(
                resolution.sourceMaterial,
              )
                ? resolution.sourceMaterial
                : null;
              const canAddDedicatedSpool =
                resolution.canAddDedicatedSpool && dedicatedSpoolMaterial !== null;
              const selectedSpoolId = selectedSpools.get(resolution.candidateId) ?? "";
              const selectedSpool = availableSpools.find(
                (spool) => spool.id === selectedSpoolId,
              );
              const changesDirectMaterial = Boolean(
                selectedSpool && selectedSpool.material !== resolution.sourceMaterial,
              );
              const directRiskAcknowledged = directMaterialAcknowledged.has(
                resolution.candidateId,
              );
              const unavailableReason = directUnavailableReason(resolution);
              const sortedSpools = [...availableSpools].sort((left, right) => {
                const leftMatches = left.material === resolution.sourceMaterial;
                const rightMatches = right.material === resolution.sourceMaterial;
                if (leftMatches !== rightMatches) return leftMatches ? -1 : 1;
                return left.colorName.localeCompare(right.colorName);
              });

              return (
                <article
                  className={`color-resolution-card ${complete ? "is-approved" : ""}`}
                  key={`${resolution.scopeId}-${resolution.requirementId}-${resolution.candidateId}`}
                >
                  <header className="color-resolution-card__heading">
                    <div>
                      <strong>Source color {resolution.sourceHex}</strong>
                      <span>{resolution.recommendation}</span>
                    </div>
                    <span className={`color-resolution-status ${complete ? "is-approved" : ""}`}>
                      {complete ? "Accepted" : "Needs approval"}
                    </span>
                  </header>

                  <AlternativeColor resolution={resolution} />

                  {onSelectCmyxColor ? (
                    <CmyxPalettePicker
                      resolution={resolution}
                      onSelect={onSelectCmyxColor}
                    />
                  ) : null}

                  <div className="color-resolution__facts">
                    <span>
                      ΔE00 <strong>{resolution.deltaE00?.toFixed(1) ?? "Unavailable"}</strong>
                    </span>
                    <span>
                      Recipe <strong>{resolution.recipe}</strong>
                    </span>
                    <span>
                      Required T4{" "}
                      <strong>
                        {resolution.requiredT4Name ?? "No T4 change"}
                      </strong>
                      {resolution.requiredT4Hex ? (
                        <ColorSwatch hex={resolution.requiredT4Hex} size="small" />
                      ) : null}
                    </span>
                    <span>
                      Confidence <strong>{resolution.confidence}</strong>
                    </span>
                  </div>

                  {resolution.requiresMaterialSubstitution && !resolution.materialApproved ? (
                    <div className="material-risk">
                      <TriangleAlert aria-hidden="true" />
                      <div>
                        <strong>
                          Material change: {resolution.sourceMaterial} → {resolution.targetMaterial}
                        </strong>
                        <p>
                          Strength, flexibility, heat resistance, and joint wear may change.
                          This is a mechanical substitution, not only a color approximation.
                        </p>
                        <label htmlFor={acknowledgementId}>
                          <input
                            id={acknowledgementId}
                            type="checkbox"
                            checked={isAcknowledged}
                            onChange={(event) =>
                              setMaterialAcknowledged(
                                resolution.candidateId,
                                event.target.checked,
                              )
                            }
                          />
                          I understand the mechanical-property risk.
                        </label>
                      </div>
                    </div>
                  ) : null}

                  <div className="color-resolution__actions">
                    {complete ? (
                      <span className="color-resolution__accepted">
                        <CheckCircle2 aria-hidden="true" />
                        This exact candidate is accepted
                      </span>
                    ) : resolution.requiresMaterialSubstitution ? (
                      <button
                        className="button button--primary button--compact"
                        type="button"
                        disabled={!isAcknowledged}
                        onClick={() => onAcceptMaterialSubstitution(resolution)}
                      >
                        Accept color and material change
                      </button>
                    ) : (
                      <button
                        className="button button--primary button--compact"
                        type="button"
                        onClick={() => onAcceptColor(resolution)}
                      >
                        Accept approximation
                      </button>
                    )}
                    {canAddDedicatedSpool ? (
                      <button
                        className="button button--secondary button--compact"
                        type="button"
                        aria-expanded={spoolFormOpen}
                        onClick={() => toggleSpoolForm(resolution.candidateId)}
                      >
                        <Plus aria-hidden="true" />
                        Choose / add spool
                      </button>
                    ) : null}
                  </div>

                  {spoolFormOpen && dedicatedSpoolMaterial ? (
                    <div className="color-resolution__spool-form">
                      <form
                        className="existing-spool-picker"
                        onSubmit={(event) => {
                          event.preventDefault();
                          if (
                            !selectedSpool ||
                            unavailableReason ||
                            (changesDirectMaterial && !directRiskAcknowledged)
                          ) {
                            return;
                          }
                          onChooseExistingSpool(
                            resolution,
                            selectedSpool.id,
                            changesDirectMaterial && directRiskAcknowledged,
                          );
                        }}
                      >
                        <fieldset disabled={Boolean(unavailableReason)}>
                          <legend>Choose an in-stock spool first</legend>
                          <p>
                            The selected physical spool will replace this source color.
                            The scope will switch to Direct Spools and T1–T4 setup actions
                            will be rebuilt after recalculation.
                          </p>
                          {unavailableReason ? (
                            <p className="existing-spool-picker__unavailable" role="status">
                              {unavailableReason}
                            </p>
                          ) : null}
                          {sortedSpools.length > 0 ? (
                            <div className="existing-spool-options">
                              {sortedSpools.map((spool) => {
                                const optionId = `resolution-${resolution.candidateId}-${spool.id}`;
                                return (
                                  <label
                                    className="existing-spool-option"
                                    htmlFor={optionId}
                                    key={spool.id}
                                  >
                                    <input
                                      id={optionId}
                                      name={`resolution-${resolution.candidateId}-spool`}
                                      type="radio"
                                      value={spool.id}
                                      required
                                      checked={selectedSpoolId === spool.id}
                                      onChange={() =>
                                        selectSpool(resolution.candidateId, spool.id)
                                      }
                                    />
                                    <ColorSwatch hex={spool.hex} size="medium" />
                                    <span>
                                      <strong>{spool.colorName}</strong>
                                      <small>
                                        {spool.material} · {spool.name}
                                      </small>
                                    </span>
                                    <code>{spool.hex}</code>
                                  </label>
                                );
                              })}
                            </div>
                          ) : (
                            <p>No in-stock spools are available. Add one below.</p>
                          )}
                        </fieldset>

                        {changesDirectMaterial && selectedSpool ? (
                          <div className="material-risk">
                            <TriangleAlert aria-hidden="true" />
                            <div>
                              <strong>
                                Material change: {resolution.sourceMaterial} →{" "}
                                {selectedSpool.material}
                              </strong>
                              <p>
                                This forced spool changes strength, flexibility, heat
                                resistance, and joint wear. It is not only a color change.
                              </p>
                              <label
                                htmlFor={`${resolution.candidateId}-direct-material-risk`}
                              >
                                <input
                                  id={`${resolution.candidateId}-direct-material-risk`}
                                  type="checkbox"
                                  required
                                  checked={directRiskAcknowledged}
                                  onChange={(event) =>
                                    setDirectMaterialRisk(
                                      resolution.candidateId,
                                      event.target.checked,
                                    )
                                  }
                                />
                                I understand the mechanical-property risk.
                              </label>
                            </div>
                          </div>
                        ) : null}

                        {sortedSpools.length > 0 ? (
                          <button
                            className="button button--primary button--compact"
                            type="submit"
                            disabled={Boolean(unavailableReason)}
                          >
                            Use selected spool
                          </button>
                        ) : null}
                      </form>

                      <SpoolInventoryForm
                        idPrefix={`resolution-${resolution.candidateId}`}
                        spoolCount={availableSpools.length}
                        heading="Need a different spool?"
                        description={`Add a ${resolution.sourceMaterial} spool to the library, then select it above and recalculate the plan.`}
                        summaryLabel="Add new spool to library"
                        initialValues={{
                          colorName: `Match ${resolution.sourceHex}`,
                          hex: resolution.sourceHex,
                          material: dedicatedSpoolMaterial,
                        }}
                        onAddSpool={onAddSpool}
                      />
                    </div>
                  ) : null}

                  <details className="color-resolution__technical">
                    <summary>Technical identity</summary>
                    <dl>
                      <div>
                        <dt>Requirement</dt>
                        <dd>{resolution.requirementId}</dd>
                      </div>
                      <div>
                        <dt>Candidate</dt>
                        <dd>{resolution.candidateId}</dd>
                      </div>
                      {resolution.requiredT4SpoolId ? (
                        <div>
                          <dt>T4 spool ID</dt>
                          <dd>{resolution.requiredT4SpoolId}</dd>
                        </div>
                      ) : null}
                    </dl>
                  </details>
                </article>
              );
            })}
          </div>
        </section>
      ))}
    </section>
  );
}
