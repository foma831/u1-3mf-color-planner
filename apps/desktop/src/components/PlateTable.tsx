import { useEffect, useRef, useState, type ReactNode } from "react";
import { CheckCircle2, ChevronRight, TriangleAlert } from "lucide-react";

import type { PhysicalSpool, PlatePlan, PrintStrategy } from "../types";
import {
  isDirectStrategyAvailable,
  plateDisplayDelta,
  plateLoadoutColors,
  strategyLabel,
} from "../utils/plan";
import { ColorSwatch } from "./ColorSwatch";
import { SourcePaletteSummary } from "./SourcePalette";

interface PlateTableProps {
  plates: PlatePlan[];
  spools: PhysicalSpool[];
  selectedPlateId: string;
  onSelectPlate: (plateId: string) => void;
  a1MiniEnabled: boolean;
  a1MiniConfigured?: boolean;
  hasPendingPlanChanges: boolean;
  onBulkStrategyChange: (
    scopeIds: string[],
    strategy: Extract<PrintStrategy, "cmyx" | "direct">,
  ) => void;
}

function Loadout({
  plate,
  spools,
}: {
  plate: PlatePlan;
  spools: PhysicalSpool[];
}) {
  const colors = plateLoadoutColors(plate, spools);
  const label =
    plate.strategy === "direct"
      ? `${colors.length} direct spools`
      : plate.loadoutLabel;

  return (
    <span className="loadout-cell">
      <span className="loadout-swatches" aria-hidden="true">
        {colors.map((color, index) => (
          <ColorSwatch hex={color} size="small" key={`${color}-${index}`} />
        ))}
      </span>
      <span className="loadout-label">{label}</span>
    </span>
  );
}

function Quality({
  plate,
  spools,
}: {
  plate: PlatePlan;
  spools: PhysicalSpool[];
}) {
  const delta = plateDisplayDelta(plate, spools);
  const status =
    delta === null
      ? "Review"
      : delta <= 2.5
        ? "Excellent"
        : delta <= 5
          ? "Very Good"
          : "Review";

  return (
    <span
      className={`quality quality--${status.toLowerCase().replace(" ", "-")}`}
    >
      {status === "Review" ? (
        <TriangleAlert aria-hidden="true" />
      ) : (
        <CheckCircle2 aria-hidden="true" />
      )}
      <span>
        {status}
        <small>ΔE00 {delta === null ? "Unavailable" : delta.toFixed(1)}</small>
      </span>
    </span>
  );
}

interface PrinterPlateTableProps {
  headingId: string;
  heading: string;
  plates: PlatePlan[];
  spools: PhysicalSpool[];
  selectedPlateId: string;
  onSelectPlate: (plateId: string) => void;
  emptyMessage: string;
  toolbar?: ReactNode;
  selectedScopeIds?: ReadonlySet<string>;
  onToggleScope?: (scopeId: string) => void;
  showDetailedColumns: boolean;
  boundaryNotice?: string;
}

function PrinterPlateTable({
  headingId,
  heading,
  plates,
  spools,
  selectedPlateId,
  onSelectPlate,
  emptyMessage,
  toolbar,
  selectedScopeIds,
  onToggleScope,
  showDetailedColumns,
  boundaryNotice,
}: PrinterPlateTableProps) {
  const bulkSelectionEnabled = Boolean(selectedScopeIds && onToggleScope);
  const printableCount = plates.filter(
    (plate) => plate.planningStatus === "printable",
  ).length;
  const blockedCount = plates.length - printableCount;
  const queueSummary =
    blockedCount > 0
      ? `${printableCount} printable · ${blockedCount} blocked`
      : `${printableCount} printable`;
  return (
    <section className="printer-plate-group" aria-labelledby={headingId}>
      <div className="printer-plate-heading">
        <h3 id={headingId}>{heading}</h3>
        <span>{queueSummary}</span>
      </div>
      {boundaryNotice ? (
        <p className="printer-plate-boundary-note">{boundaryNotice}</p>
      ) : null}
      {toolbar}
      {plates.length > 0 ? (
        <div
          className="table-scroll"
          tabIndex={0}
          aria-label={`Scrollable ${heading} table`}
        >
          <table
            className={`plate-queue-table ${
              showDetailedColumns ? "plate-queue-table--detailed" : ""
            }`}
          >
            <caption>
              {heading} in planned print order. The default view includes plate,
              printer, strategy, loadout, and status. Detailed columns add the
              source palette, objects, setup action, and optional bulk
              selection.
              {blockedCount > 0
                ? " Blocked rows show omitted source units that must be resolved; they are not printable target plates."
                : ""}
            </caption>
            <thead>
              <tr>
                {bulkSelectionEnabled ? <th scope="col">Bulk</th> : null}
                <th scope="col">Plate</th>
                <th scope="col">Printer</th>
                <th scope="col">Strategy</th>
                <th scope="col">Loadout</th>
                <th scope="col">Status</th>
                {showDetailedColumns ? (
                  <>
                    <th scope="col">Source Palette</th>
                    <th scope="col">Objects</th>
                    <th scope="col">T4 / Setup Action</th>
                  </>
                ) : null}
              </tr>
            </thead>
            <tbody>
              {plates.map((plate) => {
                const selected = selectedPlateId === plate.id;
                return (
                  <tr
                    className={selected ? "is-selected" : undefined}
                    key={plate.id}
                  >
                    {bulkSelectionEnabled ? (
                      <td className="bulk-selection-cell">
                        <input
                          type="checkbox"
                          aria-label={`Select ${plate.title} for bulk strategy change`}
                          checked={
                            selectedScopeIds?.has(plate.scopeId) ?? false
                          }
                          onChange={() => onToggleScope?.(plate.scopeId)}
                        />
                      </td>
                    ) : null}
                    <th scope="row">
                      <button
                        id={`plate-select-${plate.id}`}
                        className="plate-select"
                        type="button"
                        aria-pressed={selected}
                        onClick={() => onSelectPlate(plate.id)}
                      >
                        <span className="plate-select__title">
                          {selected ? (
                            <ChevronRight aria-hidden="true" />
                          ) : null}
                          <strong>{plate.title}</strong>
                        </span>
                        <small>
                          Plate {plate.order} · {plate.source}
                        </small>
                      </button>
                    </th>
                    <td>
                      <span
                        className={`printer-label printer-label--${plate.printer === "U1" ? "u1" : "a1"}`}
                      >
                        {plate.printer}
                      </span>
                    </td>
                    <td>
                      <span
                        className={`strategy-text strategy-text--${plate.strategy}`}
                      >
                        {strategyLabel[plate.strategy]}
                      </span>
                    </td>
                    <td>
                      <Loadout plate={plate} spools={spools} />
                    </td>
                    <td className="plate-status-cell">
                      <Quality plate={plate} spools={spools} />
                      {plate.warnings.length > 0 ? (
                        <details className="warning-disclosure warning-disclosure--plate">
                          <summary>
                            <TriangleAlert aria-hidden="true" />
                            <span>
                              {plate.warnings.length}{" "}
                              {plate.warnings.length === 1
                                ? "warning"
                                : "warnings"}
                            </span>
                          </summary>
                          <ul>
                            {plate.warnings.map((warning, index) => (
                              <li key={`${plate.id}-warning-${index}`}>
                                {warning}
                              </li>
                            ))}
                          </ul>
                        </details>
                      ) : (
                        <small className="plate-status-cell__ready">
                          No warnings
                        </small>
                      )}
                    </td>
                    {showDetailedColumns ? (
                      <>
                        <td>
                          <SourcePaletteSummary plate={plate} />
                        </td>
                        <td>{plate.objectCount}</td>
                        <td>
                          <span className="setup-action">
                            {plate.strategy === "direct"
                              ? "Full setup"
                              : plate.t4Action}
                          </span>
                        </td>
                      </>
                    ) : null}
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      ) : (
        <p className="printer-plate-empty">{emptyMessage}</p>
      )}
    </section>
  );
}

export function PlateTable({
  plates,
  spools,
  selectedPlateId,
  onSelectPlate,
  a1MiniEnabled,
  a1MiniConfigured = true,
  hasPendingPlanChanges,
  onBulkStrategyChange,
}: PlateTableProps) {
  const u1Plates = plates.filter((plate) => plate.printer === "U1");
  const a1MiniPlates = plates.filter((plate) => plate.printer === "A1 mini");
  const printablePlateCount = plates.filter(
    (plate) => plate.planningStatus === "printable",
  ).length;
  const blockedPlateCount = plates.length - printablePlateCount;
  const u1ScopeIds = u1Plates
    .map((plate) => plate.scopeId)
    .filter((scopeId, index, values) => values.indexOf(scopeId) === index);
  const u1ScopeSignature = u1ScopeIds.join("\u0000");
  const [selectedU1ScopeIds, setSelectedU1ScopeIds] = useState<string[]>([]);
  const [showDetailedColumns, setShowDetailedColumns] = useState(false);
  const selectAllRef = useRef<HTMLInputElement>(null);
  const bulkDetailsRef = useRef<HTMLDetailsElement>(null);

  useEffect(() => {
    const visibleScopeIds = new Set(u1ScopeIds);
    setSelectedU1ScopeIds((current) =>
      current.filter((scopeId) => visibleScopeIds.has(scopeId)),
    );
  }, [u1ScopeSignature]);

  const selectedScopeSet = new Set(selectedU1ScopeIds);
  const selectedPlateCount = u1Plates.filter((plate) =>
    selectedScopeSet.has(plate.scopeId),
  ).length;
  const allU1Selected =
    u1ScopeIds.length > 0 && selectedU1ScopeIds.length === u1ScopeIds.length;
  const directEligibleScopeIds = u1ScopeIds.filter((scopeId) =>
    u1Plates
      .filter((plate) => plate.scopeId === scopeId)
      .every(isDirectStrategyAvailable),
  );
  const directBlockedScopeIds = selectedU1ScopeIds.filter(
    (scopeId) => !directEligibleScopeIds.includes(scopeId),
  );

  useEffect(() => {
    if (!selectAllRef.current) return;
    selectAllRef.current.indeterminate =
      selectedU1ScopeIds.length > 0 && !allU1Selected;
  }, [allU1Selected, selectedU1ScopeIds.length]);

  const toggleScope = (scopeId: string) => {
    setSelectedU1ScopeIds((current) =>
      current.includes(scopeId)
        ? current.filter((candidate) => candidate !== scopeId)
        : [...current, scopeId],
    );
  };

  const bulkStatus =
    selectedU1ScopeIds.length === 0
      ? "Select one or more U1 plates. Plates from the same source scope stay linked."
      : directBlockedScopeIds.length > 0
        ? `${selectedU1ScopeIds.length} source scopes selected (${selectedPlateCount} plates). ${directBlockedScopeIds.length} selected scope${directBlockedScopeIds.length === 1 ? " is" : "s are"} not currently available for Direct Spools.`
        : `${selectedU1ScopeIds.length} source scopes selected (${selectedPlateCount} plates). All selected scopes can use Direct Spools.`;
  const a1EmptyMessage = hasPendingPlanChanges
    ? "Routing changed. Recalculate the plan to update A1 mini assignments."
    : !a1MiniConfigured
      ? "A1 mini is not in the saved printing setup. Add it there before evaluating optional routing."
      : a1MiniEnabled
        ? "No eligible mono plates were assigned to the A1 mini."
        : "A1 mini routing is off. Enable it and recalculate the plan to move eligible mono parts here.";

  const bulkToolbar =
    u1Plates.length > 0 ? (
      <section
        className="bulk-strategy-toolbar"
        aria-labelledby="bulk-strategy-heading"
      >
        <div className="bulk-strategy-toolbar__heading">
          <div>
            <h4 id="bulk-strategy-heading">Bulk U1 strategy</h4>
            <p>
              Select all U1 plates or only the rows you want, then apply one
              strategy to their linked source scopes.
            </p>
          </div>
          <span>{selectedPlateCount} selected</span>
        </div>
        <div className="bulk-strategy-toolbar__controls">
          <label>
            <input
              ref={selectAllRef}
              type="checkbox"
              checked={allU1Selected}
              onChange={(event) =>
                setSelectedU1ScopeIds(event.target.checked ? u1ScopeIds : [])
              }
            />
            Select all U1 plates
          </label>
          <button
            className="button button--secondary button--compact"
            type="button"
            disabled={directEligibleScopeIds.length === 0}
            onClick={() => setSelectedU1ScopeIds(directEligibleScopeIds)}
          >
            Select Direct-compatible
          </button>
          <button
            className="button button--secondary button--compact"
            type="button"
            disabled={selectedU1ScopeIds.length === 0}
            onClick={() => setSelectedU1ScopeIds([])}
          >
            Clear
          </button>
        </div>
        <div className="bulk-strategy-toolbar__actions">
          <button
            className="button button--dark button--compact"
            type="button"
            disabled={
              selectedU1ScopeIds.length === 0 ||
              directBlockedScopeIds.length > 0
            }
            onClick={() => onBulkStrategyChange(selectedU1ScopeIds, "direct")}
          >
            Set selected to Direct Spools
          </button>
          <button
            className="button button--secondary button--compact"
            type="button"
            disabled={selectedU1ScopeIds.length === 0}
            onClick={() => onBulkStrategyChange(selectedU1ScopeIds, "cmyx")}
          >
            Set selected to CMY+X
          </button>
        </div>
        <p
          className="bulk-strategy-toolbar__status"
          aria-live="polite"
          aria-atomic="true"
        >
          {bulkStatus}
          {directBlockedScopeIds.length > 0
            ? " Open an unavailable plate and choose Create 4-spool palette, or deselect that scope."
            : ""}
        </p>
      </section>
    ) : null;

  const bulkToolbarDisclosure = bulkToolbar ? (
    <details ref={bulkDetailsRef} className="bulk-strategy-disclosure">
      <summary onClick={() => setShowDetailedColumns(true)}>
        <span>Bulk strategy changes</span>
        <small>Advanced</small>
      </summary>
      {bulkToolbar}
    </details>
  ) : null;

  return (
    <section
      className="plate-table-section"
      aria-labelledby="target-plates-heading"
    >
      <div className="section-heading-row plate-table-heading">
        <div>
          <h2 id="target-plates-heading">Printer plate queues</h2>
          <p>
            {hasPendingPlanChanges
              ? "Pending edits are not reflected in these queues yet. Recalculate the plan to update assignments."
              : "Select a row to inspect its color strategy and physical loadout."}
          </p>
        </div>
        <div className="plate-table-view-options">
          <button
            id="plate-table-detail-toggle"
            className="button button--secondary button--compact"
            type="button"
            aria-pressed={showDetailedColumns}
            onClick={() => {
              const next = !showDetailedColumns;
              setShowDetailedColumns(next);
              if (!next && bulkDetailsRef.current) {
                bulkDetailsRef.current.open = false;
              }
            }}
          >
            {showDetailedColumns
              ? "Use compact columns"
              : "Show detailed columns"}
          </button>
          <span>
            {blockedPlateCount > 0
              ? `${printablePlateCount} printable · ${blockedPlateCount} blocked total`
              : `${printablePlateCount} printable total`}
          </span>
        </div>
      </div>
      <div className="printer-plate-groups">
        <PrinterPlateTable
          headingId="u1-plates-heading"
          heading="U1 Plates"
          plates={u1Plates}
          spools={spools}
          selectedPlateId={selectedPlateId}
          onSelectPlate={onSelectPlate}
          toolbar={bulkToolbarDisclosure}
          selectedScopeIds={showDetailedColumns ? selectedScopeSet : undefined}
          onToggleScope={showDetailedColumns ? toggleScope : undefined}
          showDetailedColumns={showDetailedColumns}
          boundaryNotice={
            u1Plates.filter(
              (plate) => plate.planningStatus === "printable",
            ).length > 1
              ? "This plan preserves the source project's U1 plate boundaries. Objects from different source plates are not combined."
              : undefined
          }
          emptyMessage="No plates are assigned to the Snapmaker U1 in this plan."
        />
        <PrinterPlateTable
          headingId="a1-mini-plates-heading"
          heading="A1 mini Plates"
          plates={a1MiniPlates}
          spools={spools}
          selectedPlateId={selectedPlateId}
          onSelectPlate={onSelectPlate}
          emptyMessage={a1EmptyMessage}
          showDetailedColumns={showDetailedColumns}
        />
      </div>
    </section>
  );
}
