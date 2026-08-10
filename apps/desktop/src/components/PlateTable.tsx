import { CheckCircle2, ChevronRight, TriangleAlert } from "lucide-react";

import type { PhysicalSpool, PlatePlan } from "../types";
import {
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
  hasPendingPlanChanges: boolean;
}

function Loadout({ plate, spools }: { plate: PlatePlan; spools: PhysicalSpool[] }) {
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

function Quality({ plate, spools }: { plate: PlatePlan; spools: PhysicalSpool[] }) {
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
    <span className={`quality quality--${status.toLowerCase().replace(" ", "-")}`}>
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
}

function PrinterPlateTable({
  headingId,
  heading,
  plates,
  spools,
  selectedPlateId,
  onSelectPlate,
  emptyMessage,
}: PrinterPlateTableProps) {
  return (
    <section className="printer-plate-group" aria-labelledby={headingId}>
      <div className="printer-plate-heading">
        <h3 id={headingId}>{heading}</h3>
        <span>{plates.length} planned</span>
      </div>
      {plates.length > 0 ? (
        <div
          className="table-scroll"
          tabIndex={0}
          aria-label={`Scrollable ${heading} table`}
        >
          <table>
            <caption>
              {heading} in planned print order, including printer, strategy, loadout,
              original source palette, color quality, setup action, and warnings.
            </caption>
            <thead>
              <tr>
                <th scope="col">Order</th>
                <th scope="col">Printer</th>
                <th scope="col">Target Plate</th>
                <th scope="col">Source Palette</th>
                <th scope="col">Strategy</th>
                <th scope="col">Loadout</th>
                <th scope="col">Objects</th>
                <th scope="col">Color Quality</th>
                <th scope="col">T4 / Setup Action</th>
                <th scope="col">Warnings</th>
              </tr>
            </thead>
            <tbody>
              {plates.map((plate) => {
                const selected = selectedPlateId === plate.id;
                return (
                  <tr className={selected ? "is-selected" : undefined} key={plate.id}>
                    <th scope="row" className="order-cell">
                      {selected ? <ChevronRight aria-hidden="true" /> : null}
                      {plate.order}
                    </th>
                    <td>
                      <span
                        className={`printer-label printer-label--${plate.printer === "U1" ? "u1" : "a1"}`}
                      >
                        {plate.printer}
                      </span>
                    </td>
                    <td>
                      <button
                        className="plate-select"
                        type="button"
                        aria-pressed={selected}
                        onClick={() => onSelectPlate(plate.id)}
                      >
                        <strong>{plate.title}</strong>
                        <small>{plate.source}</small>
                      </button>
                    </td>
                    <td>
                      <SourcePaletteSummary plate={plate} />
                    </td>
                    <td>
                      <span className={`strategy-text strategy-text--${plate.strategy}`}>
                        {strategyLabel[plate.strategy]}
                      </span>
                    </td>
                    <td>
                      <Loadout plate={plate} spools={spools} />
                    </td>
                    <td>{plate.objectCount}</td>
                    <td>
                      <Quality plate={plate} spools={spools} />
                    </td>
                    <td>
                      <span className="setup-action">
                        {plate.strategy === "direct" ? "Full setup" : plate.t4Action}
                      </span>
                    </td>
                    <td>
                      {plate.warnings.length > 0 ? (
                        <details className="warning-disclosure warning-disclosure--plate">
                          <summary>
                            <TriangleAlert aria-hidden="true" />
                            <span>
                              {plate.warnings.length}{" "}
                              {plate.warnings.length === 1 ? "warning" : "warnings"}
                            </span>
                          </summary>
                          <ul>
                            {plate.warnings.map((warning, index) => (
                              <li key={`${plate.id}-warning-${index}`}>{warning}</li>
                            ))}
                          </ul>
                        </details>
                      ) : (
                        <span aria-label="No warnings">—</span>
                      )}
                    </td>
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
  hasPendingPlanChanges,
}: PlateTableProps) {
  const u1Plates = plates.filter((plate) => plate.printer === "U1");
  const a1MiniPlates = plates.filter((plate) => plate.printer === "A1 mini");
  const a1EmptyMessage = hasPendingPlanChanges
    ? "Routing changed. Recalculate the plan to update A1 mini assignments."
    : a1MiniEnabled
      ? "No eligible mono plates were assigned to the A1 mini."
      : "A1 mini routing is off. Enable it and recalculate the plan to move eligible mono parts here.";

  return (
    <section className="plate-table-section" aria-labelledby="target-plates-heading">
      <div className="section-heading-row plate-table-heading">
        <div>
          <h2 id="target-plates-heading">Printer plate queues</h2>
          <p>
            {hasPendingPlanChanges
              ? "Pending edits are not reflected in these queues yet. Recalculate the plan to update assignments."
              : "Select a row to inspect its color strategy and physical loadout."}
          </p>
        </div>
        <span>{plates.length} total</span>
      </div>
      <div className="printer-plate-groups">
        <PrinterPlateTable
          headingId="u1-plates-heading"
          heading="U1 Plates"
          plates={u1Plates}
          spools={spools}
          selectedPlateId={selectedPlateId}
          onSelectPlate={onSelectPlate}
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
        />
      </div>
    </section>
  );
}
