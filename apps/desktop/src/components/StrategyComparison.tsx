import { CheckCircle2, Info, TriangleAlert } from "lucide-react";

import type { PhysicalSpool, PlatePlan, PrintStrategy } from "../types";
import {
  cmyAverageDelta,
  directAverageDelta,
  isCmyxStrategy,
  isDirectStrategyAvailable,
} from "../utils/plan";

interface StrategyComparisonProps {
  plate: PlatePlan;
  spools: PhysicalSpool[];
  onChange: (strategy: Extract<PrintStrategy, "cmyx" | "direct">) => void;
}

export function StrategyComparison({
  plate,
  spools,
  onChange,
}: StrategyComparisonProps) {
  const directAvailable = isDirectStrategyAvailable(plate);
  const effectivePairCount = plate.effectivePairCount ?? plate.logicalColorCount;
  const directPairCount = plate.directPairCount ?? effectivePairCount;
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
              {cmyxSelected ? (
                <CheckCircle2 aria-label="Selected" />
              ) : null}
            </span>
            <span>Best for gradients and subtle mixed tones.</span>
            <span className="strategy-metrics">
              <b>ΔE00 {cmyDelta === null ? "Unavailable" : cmyDelta.toFixed(1)}</b>
              <span>Prime tower</span>
              <span>{plate.toolChanges} tool changes</span>
            </span>
          </span>
        </label>

        <label className={`strategy-option ${!directAvailable ? "is-disabled" : ""}`}>
          <input
            type="radio"
            name={`strategy-${plate.id}`}
            value="direct"
            checked={plate.strategy === "direct"}
            disabled={!directAvailable}
            onChange={() => onChange("direct")}
          />
          <span className="strategy-option__body">
            <span className="strategy-option__topline">
              <strong>Direct Spools</strong>
              {plate.strategy === "direct" ? (
                <CheckCircle2 aria-label="Selected" />
              ) : directRecommended ? (
                <span className="recommended-mark">Recommended</span>
              ) : null}
            </span>
            <span>Best for exact brand colors and specialty filaments.</span>
            <span className="strategy-metrics">
              <b>
                {directAvailable
                  ? `ΔE00 ${directDelta === null ? "Unavailable" : directDelta.toFixed(1)}`
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
          {directPairCount > 4 ? (
            <p className="strategy-reason-detail">
              This limit applies to exact one-to-one Direct assignments. CMY+X can
              still reproduce the {effectivePairCount} semantic source pairs from
              four loaded filaments. Reducing {directPairCount} physical Direct
              identities to four would be a separate, lossy palette remap.
            </p>
          ) : null}
        </div>
      ) : (
        <p className="strategy-note">
          <Info aria-hidden="true" />
          {effectivePairCount} semantic material-color-role{" "}
          {pluralPair(effectivePairCount)} use {directPairCount} physical Direct{" "}
          {pluralIdentity(directPairCount)}. Both strategies are valid.
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
