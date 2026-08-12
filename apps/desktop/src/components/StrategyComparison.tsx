import { CheckCircle2, Info, LoaderCircle, TriangleAlert } from "lucide-react";

import type { PhysicalSpool, PlatePlan, PrintStrategy } from "../types";
import {
  cmyAverageDelta,
  directPhysicalSpoolCount,
  directAverageDelta,
  isCmyxStrategy,
  isDirectStrategyAvailable,
} from "../utils/plan";

interface StrategyComparisonProps {
  plate: PlatePlan;
  spools: PhysicalSpool[];
  onChange: (strategy: Extract<PrintStrategy, "cmyx" | "direct">) => void;
  customDirectPalettesEnabled?: boolean;
  isPreparingCustomPalette?: boolean;
  customPaletteError?: string;
  onPrepareCustomPalette?: () => void;
  onOpenFilamentLibrary?: () => void;
}

export function StrategyComparison({
  plate,
  spools,
  onChange,
  customDirectPalettesEnabled = false,
  isPreparingCustomPalette = false,
  customPaletteError,
  onPrepareCustomPalette,
  onOpenFilamentLibrary,
}: StrategyComparisonProps) {
  const directAvailable = isDirectStrategyAvailable(plate);
  const effectivePairCount =
    plate.effectivePairCount ?? plate.logicalColorCount;
  const directPairCount = plate.directPairCount ?? effectivePairCount;
  const requiresPaletteReduction = directPairCount > 4;
  const directSpoolCount = directPhysicalSpoolCount(plate);
  const reducesPalette = directPairCount > directSpoolCount;
  const cmyDelta = plate.mappings
    ? cmyAverageDelta(plate.mappings)
    : plate.estimatedDeltaE00;
  const directDelta = directAverageDelta(plate.mappings, spools);
  const directRecommended =
    directAvailable &&
    directDelta !== null &&
    cmyDelta !== null &&
    directDelta < cmyDelta;
  const cmyxSelected = isCmyxStrategy(plate.strategy);
  const cmyxLabel =
    plate.strategy === "cmyx-solid"
      ? "CMY+X Solid (physical spools)"
      : "CMY+X Full Spectrum";

  return (
    <fieldset className="strategy-comparison">
      <legend>Strategy comparison</legend>
      <div className="strategy-options">
        <label className="strategy-option">
          <input
            type="radio"
            name={`strategy-${plate.id}`}
            value="cmyx"
            checked={cmyxSelected}
            onChange={() => onChange("cmyx")}
          />
          <span className="strategy-option__body">
            <span className="strategy-option__topline">
              <strong>{cmyxLabel}</strong>
              {cmyxSelected ? <CheckCircle2 aria-label="Selected" /> : null}
            </span>
            <span>Best for gradients and subtle mixed tones.</span>
            <span className="strategy-metrics">
              <b>
                ΔE00 {cmyDelta === null ? "Unavailable" : cmyDelta.toFixed(1)}
              </b>
              <span>Prime tower</span>
              <span>{plate.toolChanges} tool changes</span>
            </span>
          </span>
        </label>

        <label
          className={`strategy-option ${!directAvailable ? "is-disabled" : ""}`}
        >
          <input
            id={`strategy-${plate.id}-direct`}
            type="radio"
            name={`strategy-${plate.id}`}
            value="direct"
            checked={plate.strategy === "direct"}
            disabled={!directAvailable}
            onChange={() => onChange("direct")}
          />
          <span className="strategy-option__body">
            <span className="strategy-option__topline">
              <strong>
                {directPairCount > 4
                  ? "Custom 4-Spool Direct"
                  : "Direct Spools"}
              </strong>
              {plate.strategy === "direct" && directAvailable ? (
                <CheckCircle2 aria-label="Selected" />
              ) : plate.strategy === "direct" ? (
                <span className="setup-required-mark">Setup required</span>
              ) : directRecommended ? (
                <span className="recommended-mark">Recommended</span>
              ) : null}
            </span>
            <span>
              {directPairCount > 4
                ? `Reduce ${directPairCount} source identities to at most four in-stock physical spools.`
                : "Best for exact brand colors and specialty filaments."}
            </span>
            <span className="strategy-metrics">
              <b>
                {directAvailable
                  ? `ΔE00 ${directDelta === null ? "Unavailable" : directDelta.toFixed(1)}`
                  : requiresPaletteReduction && !customDirectPalettesEnabled
                    ? "Palette setup required"
                    : "Unavailable"}
              </b>
              <span>No prime tower</span>
              <span>Full spool setup</span>
            </span>
          </span>
        </label>
      </div>
      {!directAvailable ? (
        <div className="strategy-reason-group">
          <p className="strategy-reason">
            <TriangleAlert aria-hidden="true" />
            {plate.directEligibilityReason ??
              "Direct Spools is unavailable because this scope uses more than four mappings."}
          </p>
          {requiresPaletteReduction ? (
            <div className="strategy-recovery">
              <p
                className="strategy-reason-detail"
                id={`custom-palette-help-${plate.id}`}
              >
                {customDirectPalettesEnabled
                  ? "A four-spool palette is enabled, but the current inventory cannot satisfy this scope. Add or mark compatible material spools as available, then try again. Materials are never substituted automatically."
                  : `U1 can load four physical spools. Create a reviewable ${directPairCount}-to-4 proposal: every source identity and the source 3MF stay unchanged, while similar colors may intentionally share a physical spool. Materials are never substituted automatically.`}
              </p>
              {customPaletteError ? (
                <p className="strategy-recovery__error">
                  <TriangleAlert aria-hidden="true" />
                  {customPaletteError}
                </p>
              ) : null}
              <div className="strategy-recovery__actions">
                {onPrepareCustomPalette ? (
                  <button
                    className="button button--primary button--compact"
                    type="button"
                    aria-busy={isPreparingCustomPalette}
                    aria-describedby={`custom-palette-help-${plate.id}`}
                    disabled={isPreparingCustomPalette}
                    onClick={onPrepareCustomPalette}
                  >
                    {isPreparingCustomPalette ? (
                      <LoaderCircle className="spin" aria-hidden="true" />
                    ) : null}
                    {isPreparingCustomPalette
                      ? "Creating palette…"
                      : customDirectPalettesEnabled
                        ? "Try palette creation again"
                        : "Create 4-spool palette"}
                  </button>
                ) : (
                  <p className="strategy-recovery__desktop-note">
                    Create and validate this palette in the desktop app.
                  </p>
                )}
                {(customDirectPalettesEnabled || customPaletteError) &&
                onOpenFilamentLibrary ? (
                  <button
                    className="button button--secondary button--compact"
                    type="button"
                    onClick={onOpenFilamentLibrary}
                  >
                    Review Filament Library
                  </button>
                ) : null}
              </div>
              <p className="strategy-recovery__alternative">
                Prefer not to merge colors? CMY+X can reproduce the{" "}
                {effectivePairCount} semantic source{" "}
                {effectivePairCount === 1 ? "pair" : "pairs"} from four loaded
                filaments.
              </p>
            </div>
          ) : null}
        </div>
      ) : (
        <p className="strategy-note">
          <Info aria-hidden="true" />
          {reducesPalette ? (
            <>
              {effectivePairCount} semantic material-color-role{" "}
              {pluralPair(effectivePairCount)} use {directPairCount} source
              Direct {pluralIdentity(directPairCount)}, intentionally reduced to{" "}
              {directSpoolCount} physical{" "}
              {directSpoolCount === 1 ? "spool" : "spools"}.
            </>
          ) : (
            <>
              {effectivePairCount} semantic material-color-role{" "}
              {pluralPair(effectivePairCount)} use {directPairCount} physical
              Direct {pluralIdentity(directPairCount)}. Both strategies are
              valid.
            </>
          )}
        </p>
      )}
    </fieldset>
  );
}

function pluralPair(count: number) {
  return count === 1 ? "pair" : "pairs";
}

function pluralIdentity(count: number) {
  return count === 1 ? "identity" : "identities";
}
