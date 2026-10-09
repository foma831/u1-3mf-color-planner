import { ArrowRight, RotateCcw, TriangleAlert } from "lucide-react";

import type {
  DirectColorMapping,
  LoadedToolhead,
  NewPhysicalSpoolInput,
  PhysicalSpool,
  ToolheadId,
} from "../types";
import {
  directDelta,
  directQuality,
  getToolheadActions,
  inStockSpools,
  selectedSpoolForToolhead,
  toolheads,
} from "../utils/plan";
import { ColorSwatch } from "./ColorSwatch";
import { SpoolInventoryForm } from "./SpoolInventoryForm";

interface DirectSpoolEditorProps {
  plateId: string;
  mappings: DirectColorMapping[];
  spools: PhysicalSpool[];
  currentLoadout: LoadedToolhead[];
  restoreCmy: boolean;
  onRestoreChange: (restore: boolean) => void;
  onToolheadChange: (mappingId: string, toolhead: ToolheadId) => void;
  onSpoolChange: (mappingId: string, spoolId: string) => void;
  onMaterialSubstitutionChange: (mappingId: string, acknowledged: boolean) => void;
  onAddSpool: (spool: NewPhysicalSpoolInput) => void;
}

function MappingColors({
  mapping,
  spool,
}: {
  mapping: DirectColorMapping;
  spool?: PhysicalSpool;
}) {
  return (
    <div className="mapping-colors" aria-label={`Color comparison for ${mapping.sourceName}`}>
      <div className="mapping-color" aria-label="Source color">
        <span>Source</span>
        <span className="mapping-swatch-line">
          <ColorSwatch hex={mapping.sourceHex} size="large" />
          <code>{mapping.sourceHex}</code>
        </span>
      </div>
      <ArrowRight className="mapping-arrow" aria-hidden="true" />
      <div className="mapping-color" aria-label="Current CMY+X color">
        <span>Current CMY+X</span>
        {mapping.cmyPredictedHex ? (
          <span className="mapping-swatch-line">
            <ColorSwatch hex={mapping.cmyPredictedHex} size="large" />
            <code>{mapping.cmyPredictedHex}</code>
          </span>
        ) : (
          <strong className="mapping-unavailable">Unavailable</strong>
        )}
      </div>
      <ArrowRight className="mapping-arrow" aria-hidden="true" />
      <div className="mapping-color" aria-label="Actual spool color">
        <span>Actual spool</span>
        {spool ? (
          <span className="mapping-swatch-line">
            <ColorSwatch hex={spool.hex} size="large" />
            <code>{spool.hex}</code>
          </span>
        ) : (
          <strong className="mapping-unavailable">Unassigned</strong>
        )}
      </div>
    </div>
  );
}

export function DirectSpoolEditor({
  plateId,
  mappings,
  spools,
  currentLoadout,
  restoreCmy,
  onRestoreChange,
  onToolheadChange,
  onSpoolChange,
  onMaterialSubstitutionChange,
  onAddSpool,
}: DirectSpoolEditorProps) {
  const spoolById = new Map(spools.map((spool) => [spool.id, spool]));
  const selectableSpools = inStockSpools(spools);
  const selectableSpoolById = new Map(
    selectableSpools.map((spool) => [spool.id, spool]),
  );
  const currentByToolhead = new Map(
    currentLoadout.map((entry) => [entry.toolhead, spoolById.get(entry.spoolId)]),
  );
  const physicalIdentityIds = mappings
    .map((mapping) => mapping.physicalIdentityId)
    .filter((id, index, values) => values.indexOf(id) === index);
  const physicalIdentityLabels = new Map(
    physicalIdentityIds.map((id, index) => [id, index + 1]),
  );
  const selectedPhysicalSpoolCount = new Set(
    mappings
      .map((mapping) => mapping.selectedSpoolId)
      .filter((spoolId) => spoolId.length > 0),
  ).size;
  const physicalIdentityUseCounts = new Map<string, number>();
  for (const mapping of mappings) {
    physicalIdentityUseCounts.set(
      mapping.physicalIdentityId,
      (physicalIdentityUseCounts.get(mapping.physicalIdentityId) ?? 0) + 1,
    );
  }
  const mergedSpoolGroups = selectableSpools.flatMap((spool) => {
    const sharedMappings = mappings.filter(
      (mapping) => mapping.selectedSpoolId === spool.id,
    );
    const sharedPhysicalIdentities = new Set(
      sharedMappings.map((mapping) => mapping.physicalIdentityId),
    );
    return sharedPhysicalIdentities.size > 1
      ? [
          {
            spool,
            mappings: sharedMappings,
            physicalIdentityCount: sharedPhysicalIdentities.size,
          },
        ]
      : [];
  });

  return (
    <section className="direct-editor" aria-labelledby={`direct-editor-${plateId}`}>
      <div className="inspector-section-heading">
        <div>
          <h3 id={`direct-editor-${plateId}`}>Direct spool mapping</h3>
          <p>Source, current CMY+X result, and the physical spool used at print time.</p>
        </div>
      </div>

      <SpoolInventoryForm
        idPrefix={plateId}
        spoolCount={selectableSpools.length}
        onAddSpool={onAddSpool}
      />

      {physicalIdentityIds.length > 4 ? (
        <section className="mapping-merge-notice" aria-labelledby={`${plateId}-custom-palette-summary`}>
          <h4 id={`${plateId}-custom-palette-summary`}>Custom four-spool palette</h4>
          <p>
            This plate has {physicalIdentityIds.length} source physical identities.
            They are currently mapped to {selectedPhysicalSpoolCount} in-stock physical{" "}
            {selectedPhysicalSpoolCount === 1 ? "spool" : "spools"}. You may assign the
            same spool to several source colors, but the visible differences between
            those colors will be intentionally removed.
          </p>
        </section>
      ) : null}

      {mergedSpoolGroups.length > 0 ? (
        <section
          className="mapping-merge-notice"
          aria-labelledby={`${plateId}-merged-source-colors`}
        >
          <h4 id={`${plateId}-merged-source-colors`}>Combined source colors</h4>
          <ul>
            {mergedSpoolGroups.map(
              ({ spool, mappings: sharedMappings, physicalIdentityCount }) => (
                <li key={spool.id}>
                  <ColorSwatch hex={spool.hex} size="medium" />
                  <span>
                    <strong>
                      {physicalIdentityCount} physical identities →{" "}
                      {spool.colorName}
                    </strong>
                    <small>
                      One physical spool on {sharedMappings[0].directToolhead}. The
                      differences between those physical palette choices will be
                      intentionally removed.
                    </small>
                  </span>
                </li>
              ),
            )}
          </ul>
          <p>
            Recalculate the plan to validate the reduced loadout. If every source
            identity uses one compatible spool, the plate can also become eligible for
            A1 Mono routing.
          </p>
        </section>
      ) : null}

      <div className="mapping-list">
        {mappings.map((mapping) => {
          const spool = mapping.selectedSpoolId
            ? selectableSpoolById.get(mapping.selectedSpoolId)
            : undefined;
          const delta = directDelta(mapping, spools);
          const quality = directQuality(mapping, spools);
          const toolheadId = `${plateId}-${mapping.id}-toolhead`;
          const spoolId = `${plateId}-${mapping.id}-spool`;
          const materialRiskId = `${plateId}-${mapping.id}-material-risk`;
          const changesMaterial = Boolean(
            spool && spool.material !== mapping.sourceMaterial,
          );

          return (
            <article className="mapping-card" key={mapping.id}>
              <header className="mapping-card__header">
                <div>
                  <strong>{mapping.sourceName}</strong>
                  <span>
                    {mapping.dedicatedSupport
                      ? "Dedicated support"
                      : physicalIdentityUseCounts.get(mapping.physicalIdentityId)! >
                          1
                        ? "Shared physical identity"
                        : "Physical identity"}
                    {mapping.dedicatedSupport ? "" : " "}
                    {mapping.dedicatedSupport
                      ? null
                      : physicalIdentityLabels.get(mapping.physicalIdentityId)}
                    {mapping.dedicatedSupport ? "" : " · "}
                    {mapping.sourceSlot} · {mapping.sourceMaterial} · {mapping.usedBy}
                  </span>
                </div>
                <span className={`mapping-status mapping-status--${quality.toLowerCase().replace(" ", "-")}`}>
                  {quality}
                </span>
              </header>

              <MappingColors mapping={mapping} spool={spool} />

              <div className="mapping-recipe">
                <span>
                  Current recipe <strong>{mapping.cmyRecipe}</strong>
                </span>
                <span>
                  CMY+X ΔE00 {mapping.cmyDeltaE00 === null
                    ? "Unavailable"
                    : mapping.cmyDeltaE00.toFixed(1)}
                </span>
              </div>

              <div className="mapping-controls">
                <div className="field">
                  <label htmlFor={toolheadId}>Direct toolhead</label>
                  <select
                    id={toolheadId}
                    name={`toolhead-${mapping.id}`}
                    value={mapping.directToolhead}
                    disabled={mapping.dedicatedSupport}
                    onChange={(event) =>
                      onToolheadChange(mapping.id, event.target.value as ToolheadId)
                    }
                  >
                    {toolheads.map((toolhead) => (
                      <option key={toolhead} value={toolhead}>
                        {toolhead}
                      </option>
                    ))}
                  </select>
                </div>
                <div className="field field--spool">
                  <label htmlFor={spoolId}>Selected spool</label>
                  <div className="spool-select-control">
                    <span
                      className={`spool-select-control__swatch${spool ? "" : " is-empty"}`}
                      role="img"
                      aria-label={
                        spool
                          ? `Selected spool color ${spool.colorName} ${spool.hex}`
                          : "No spool color selected"
                      }
                      title={spool ? `${spool.colorName} · ${spool.hex}` : "Unassigned"}
                    >
                      {spool ? <ColorSwatch hex={spool.hex} size="small" /> : null}
                    </span>
                    <select
                      id={spoolId}
                      name={`spool-${mapping.id}`}
                      value={spool ? mapping.selectedSpoolId : ""}
                      disabled={mapping.dedicatedSupport}
                      onChange={(event) =>
                        onSpoolChange(mapping.id, event.target.value)
                      }
                    >
                      <option value="">Unassigned — choose a physical spool</option>
                      {selectableSpools.map((candidate) => (
                        <option key={candidate.id} value={candidate.id}>
                          {candidate.colorName} · {candidate.material} — {candidate.name}
                        </option>
                      ))}
                    </select>
                  </div>
                </div>
              </div>

              {changesMaterial && spool ? (
                <div className="material-risk">
                  <TriangleAlert aria-hidden="true" />
                  <div>
                    <strong>
                      Material change: {mapping.sourceMaterial} → {spool.material}
                    </strong>
                    <p>
                      This forced spool changes mechanical properties, not only color.
                      Confirm the risk before recalculating the plan.
                    </p>
                    <label htmlFor={materialRiskId}>
                      <input
                        id={materialRiskId}
                        type="checkbox"
                        checked={mapping.materialSubstitutionAcknowledged ?? false}
                        onChange={(event) =>
                          onMaterialSubstitutionChange(
                            mapping.id,
                            event.target.checked,
                          )
                        }
                      />
                      I understand the mechanical-property risk.
                    </label>
                  </div>
                </div>
              ) : null}

              <p className="mapping-result">
                Direct ΔE00
                <strong>{delta === null ? "Unavailable" : delta.toFixed(1)}</strong>
                {spool ? (
                  <>
                    <span aria-hidden="true">·</span>
                    {spool.material}
                    <span aria-hidden="true">·</span>
                    {spool.colorBasis} color
                    {spool.sku ? (
                      <span className="mapping-spool-detail">
                        <span aria-hidden="true">·</span>
                        SKU {spool.sku}
                      </span>
                    ) : null}
                    {spool.profile ? (
                      <span className="mapping-spool-detail">
                        <span aria-hidden="true">·</span>
                        Profile {spool.profile}
                      </span>
                    ) : null}
                  </>
                ) : (
                  <>
                    <span aria-hidden="true">·</span>
                    <strong className="mapping-unavailable">Unassigned</strong>
                  </>
                )}
              </p>
            </article>
          );
        })}
      </div>

      <section className="toolhead-setup" aria-labelledby={`toolhead-setup-${plateId}`}>
        <div className="inspector-section-heading">
          <div>
            <h3 id={`toolhead-setup-${plateId}`}>T1–T4 setup actions</h3>
            <p>Current loadout compared with the selected Direct Spools job.</p>
          </div>
        </div>
        <ol className="toolhead-list">
          {toolheads.map((toolhead) => {
            const current = currentByToolhead.get(toolhead);
            const selectedMapping = mappings.find(
              (mapping) => mapping.directToolhead === toolhead,
            );
            const selectedId = selectedMapping?.selectedSpoolId;
            const selected = selectedId
              ? selectableSpoolById.get(selectedId)
              : undefined;
            const actions = getToolheadActions(
              spools,
              currentLoadout,
              mappings,
              toolhead,
              restoreCmy,
            );

            return (
              <li key={toolhead}>
                <strong className="toolhead-name">{toolhead}</strong>
                <div className="toolhead-spools">
                  <span>
                    <small>Currently loaded</small>
                    <span className="toolhead-spool-value">
                      {current ? <ColorSwatch hex={current.hex} size="small" /> : null}
                      {current?.colorName ?? "Unknown"}
                    </span>
                  </span>
                  <ArrowRight aria-hidden="true" />
                  <span>
                    <small>Selected for job</small>
                    <span className="toolhead-spool-value">
                      {selected ? <ColorSwatch hex={selected.hex} size="small" /> : null}
                      {selected?.colorName ??
                        (selectedMapping ? "Unassigned" : "Unused")}
                    </span>
                  </span>
                </div>
                <div className="toolhead-actions" aria-label={`${toolhead} actions`}>
                  {actions.map((action, index) => (
                    <span
                      className={`action-chip action-chip--${action.kind.toLowerCase()}`}
                      key={`${action.kind}-${action.spoolName}-${index}`}
                    >
                      {action.kind} {action.spoolName}
                    </span>
                  ))}
                </div>
              </li>
            );
          })}
        </ol>
        <label className="restore-toggle">
          <input
            type="checkbox"
            name="restore-cmy"
            checked={restoreCmy}
            onChange={(event) => onRestoreChange(event.target.checked)}
          />
          <span>
            <strong>Restore CMY setup after this job</strong>
            <small>Add the return loadout to the production plan.</small>
          </span>
          <RotateCcw aria-hidden="true" />
        </label>
      </section>
    </section>
  );
}
