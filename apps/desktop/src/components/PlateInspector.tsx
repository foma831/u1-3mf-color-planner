import { useEffect, useRef } from "react";
import { LockKeyhole, Printer, X } from "lucide-react";

import type {
  CmyxPaletteOption,
  ColorResolution,
  NewPhysicalSpoolInput,
  PlatePlan,
  PrinterPreference,
  PrintStrategy,
  ProjectPlan,
  ToolheadId,
} from "../types";
import { isDirectStrategyAvailable, strategyLabel } from "../utils/plan";
import { DirectSpoolEditor } from "./DirectSpoolEditor";
import { SourcePaletteDetails } from "./SourcePalette";
import { StrategyComparison } from "./StrategyComparison";

interface PlateInspectorProps {
  plan: ProjectPlan;
  plate: PlatePlan;
  isOpen: boolean;
  onClose: () => void;
  restoreCmy: boolean;
  onStrategyChange: (
    strategy: Extract<PrintStrategy, "cmyx" | "direct">,
  ) => void;
  onRestoreChange: (restore: boolean) => void;
  onToolheadChange: (mappingId: string, toolhead: ToolheadId) => void;
  onSpoolChange: (mappingId: string, spoolId: string) => void;
  onMaterialSubstitutionChange: (
    mappingId: string,
    acknowledged: boolean,
  ) => void;
  onSelectCmyxColor?: (
    resolution: ColorResolution,
    option: CmyxPaletteOption,
  ) => void;
  a1MiniEnabled: boolean;
  onPrinterPreferenceChange: (preference: PrinterPreference) => void;
  onAddSpool: (spool: NewPhysicalSpoolInput) => void;
  customDirectPalettesEnabled: boolean;
  isPreparingCustomPalette: boolean;
  customPaletteError?: string;
  onPrepareCustomPalette?: () => void;
  onOpenFilamentLibrary: () => void;
}

function PrinterChoice({
  plan,
  plate,
  onChange,
}: {
  plan: ProjectPlan;
  plate: PlatePlan;
  onChange: (preference: PrinterPreference) => void;
}) {
  const sourceUnitIds = new Set(plate.sourceUnitIds);
  const preferences = new Set(
    plan.unitPrinterSelections
      .filter((selection) => sourceUnitIds.has(selection.sourceUnitId))
      .map((selection) => selection.preference),
  );
  const selected = preferences.size === 1 ? [...preferences][0] : null;
  const a1ProvisionallyAvailable = plate.isFastMono;

  return (
    <section
      className="printer-choice"
      aria-labelledby={`printer-choice-${plate.id}`}
    >
      <fieldset>
        <legend id={`printer-choice-${plate.id}`}>
          Prepare this plate for
        </legend>
        <p>
          The choice applies to all {plate.sourceUnitIds.length} source{" "}
          {plate.sourceUnitIds.length === 1 ? "unit" : "units"} in this row.
        </p>
        {selected === null ? (
          <span className="printer-choice__mixed" role="status">
            Current preference: Mixed
          </span>
        ) : null}
        <div className="printer-choice__options">
          {(
            [
              [
                "auto",
                "Auto",
                "Use A1 mini when validation allows it; otherwise keep U1.",
              ],
              [
                "u1",
                "Snapmaker U1",
                "Always keep these source units in the U1 queue.",
              ],
              [
                "a1-mini",
                "Bambu Lab A1 mini",
                "Require an A1 mini single-spool output.",
              ],
            ] as const
          ).map(([value, label, description]) => {
            const optionId = `${plate.id}-printer-${value}`;
            const disabled = value === "a1-mini" && !a1ProvisionallyAvailable;
            return (
              <label
                className={`printer-choice__option ${disabled ? "is-disabled" : ""}`}
                htmlFor={optionId}
                key={value}
              >
                <input
                  id={optionId}
                  name={`${plate.id}-printer-preference`}
                  type="radio"
                  value={value}
                  checked={selected === value}
                  disabled={disabled}
                  onChange={() => onChange(value)}
                />
                <span>
                  <strong>{label}</strong>
                  <small>{description}</small>
                </span>
              </label>
            );
          })}
        </div>
        {!a1ProvisionallyAvailable ? (
          <p className="printer-choice__notice">
            A1 mini is unavailable because this result does not use exactly one
            physical spool. Select Direct Spools, assign the same compatible
            spool to every source mapping you want to merge, then recalculate.
          </p>
        ) : (
          <p className="printer-choice__notice">
            A1 mini routing is provisional until 180 × 180 mm packing and its
            own 0.4 mm printer profile are validated.
          </p>
        )}
      </fieldset>
    </section>
  );
}

function A1MonoInspector({ plate }: { plate: PlatePlan }) {
  return (
    <section className="a1-strategy" aria-labelledby="a1-strategy-heading">
      <h3 id="a1-strategy-heading">Color strategy</h3>
      <div className="locked-strategy">
        <span className="locked-strategy__icon">
          <Printer aria-hidden="true" />
        </span>
        <span>
          <strong>{strategyLabel["a1-mono"]}</strong>
          <small>{plate.loadoutLabel} · one physical spool</small>
        </span>
        <LockKeyhole aria-label="Locked strategy" />
      </div>
      <p>
        A1 mini jobs are single-spool output. Full Spectrum recipes and U1
        toolhead mappings never apply to this plate.
      </p>
    </section>
  );
}

export function PlateInspector({
  plan,
  plate,
  isOpen,
  onClose,
  restoreCmy,
  onStrategyChange,
  onRestoreChange,
  onToolheadChange,
  onSpoolChange,
  onMaterialSubstitutionChange,
  onSelectCmyxColor,
  a1MiniEnabled,
  onPrinterPreferenceChange,
  onAddSpool,
  customDirectPalettesEnabled,
  isPreparingCustomPalette,
  customPaletteError,
  onPrepareCustomPalette,
  onOpenFilamentLibrary,
}: PlateInspectorProps) {
  const headingRef = useRef<HTMLHeadingElement | null>(null);

  useEffect(() => {
    if (isOpen) headingRef.current?.focus();
  }, [isOpen, plate.id]);

  return (
    <aside
      className={`plate-inspector ${isOpen ? "is-open" : ""}`}
      aria-labelledby="selected-plate-heading"
    >
      <header className="inspector-header">
        <p>Selected Plate</p>
        <h2 ref={headingRef} id="selected-plate-heading" tabIndex={-1}>
          {plate.title}
        </h2>
        <span>
          Order {plate.order} of {plan.plates.length} · {plate.printer}
        </span>
        <button
          className="icon-button inspector-close"
          type="button"
          aria-label="Close plate details"
          onClick={onClose}
        >
          <X aria-hidden="true" />
        </button>
      </header>

      <SourcePaletteDetails
        plate={plate}
        resolutions={plan.colorResolutions}
        onSelectCmyxColor={onSelectCmyxColor}
      />

      {a1MiniEnabled && plate.sourceUnitIds.length > 0 ? (
        <PrinterChoice
          plan={plan}
          plate={plate}
          onChange={onPrinterPreferenceChange}
        />
      ) : null}

      {plate.printer === "A1 mini" ? (
        <A1MonoInspector plate={plate} />
      ) : (
        <>
          <StrategyComparison
            plate={plate}
            spools={plan.spools}
            onChange={onStrategyChange}
            customDirectPalettesEnabled={customDirectPalettesEnabled}
            isPreparingCustomPalette={isPreparingCustomPalette}
            customPaletteError={customPaletteError}
            onPrepareCustomPalette={onPrepareCustomPalette}
            onOpenFilamentLibrary={onOpenFilamentLibrary}
          />
          {isDirectStrategyAvailable(plate) && plate.mappings ? (
            <DirectSpoolEditor
              plateId={plate.id}
              mappings={plate.mappings}
              spools={plan.spools}
              currentLoadout={plan.currentLoadout}
              restoreCmy={restoreCmy}
              onRestoreChange={onRestoreChange}
              onToolheadChange={onToolheadChange}
              onSpoolChange={onSpoolChange}
              onMaterialSubstitutionChange={onMaterialSubstitutionChange}
              onAddSpool={onAddSpool}
            />
          ) : null}
        </>
      )}
    </aside>
  );
}
