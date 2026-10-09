import { useId } from "react";
import { Check } from "lucide-react";

import type { CmyxPaletteOption, ColorResolution } from "../types";
import { ColorSwatch } from "./ColorSwatch";

interface CmyxPalettePickerProps {
  resolution: ColorResolution;
  onSelect: (resolution: ColorResolution, option: CmyxPaletteOption) => void;
  compact?: boolean;
}

function paletteSortKey(option: CmyxPaletteOption) {
  const hex = option.predictedHex ?? option.targetHex;
  const red = Number.parseInt(hex.slice(1, 3), 16) / 255;
  const green = Number.parseInt(hex.slice(3, 5), 16) / 255;
  const blue = Number.parseInt(hex.slice(5, 7), 16) / 255;
  const maximum = Math.max(red, green, blue);
  const minimum = Math.min(red, green, blue);
  const delta = maximum - minimum;
  const lightness = (maximum + minimum) / 2;
  const saturation = delta === 0 ? 0 : delta / (1 - Math.abs(2 * lightness - 1));
  let hue = 0;
  if (delta !== 0) {
    if (maximum === red) hue = ((green - blue) / delta) % 6;
    else if (maximum === green) hue = (blue - red) / delta + 2;
    else hue = (red - green) / delta + 4;
    hue = (hue * 60 + 360) % 360;
  }
  return [saturation < 0.08 ? 1 : 0, hue, lightness, hex] as const;
}

function comparePaletteOptions(left: CmyxPaletteOption, right: CmyxPaletteOption) {
  const leftKey = paletteSortKey(left);
  const rightKey = paletteSortKey(right);
  for (let index = 0; index < leftKey.length; index += 1) {
    const comparison = leftKey[index] < rightKey[index] ? -1 : leftKey[index] > rightKey[index] ? 1 : 0;
    if (comparison !== 0) return comparison;
  }
  return left.candidateId.localeCompare(right.candidateId);
}

export function CmyxPalettePicker({
  resolution,
  onSelect,
  compact = false,
}: CmyxPalettePickerProps) {
  const pickerId = useId().replaceAll(":", "");
  const options = [...(resolution.paletteOptions ?? [])].sort(
    comparePaletteOptions,
  );
  if (options.length < 2) return null;
  const selectedHex = resolution.predictedHex ?? resolution.targetHex;

  return (
    <details className={`cmyx-palette-picker ${compact ? "is-compact" : ""}`}>
      <summary>
        <ColorSwatch hex={selectedHex} size="small" />
        <span>Change CMY color</span>
        <code>{selectedHex}</code>
      </summary>
      <fieldset>
        <legend>Printable CMY palette</legend>
        <p>
          Every swatch is a real recipe for the current loadout. Selecting a
          same-material CMY recipe accepts that exact choice immediately.
        </p>
        <div className="cmyx-palette-picker__grid">
          {options.map((option) => {
            const hex = option.predictedHex ?? option.targetHex;
            const selected = option.candidateId === resolution.candidateId;
            const optionId = `cmyx-${pickerId}-${option.candidateId}`;
            const delta = option.deltaE00?.toFixed(1) ?? "unavailable";
            return (
              <label
                className={selected ? "is-selected" : undefined}
                htmlFor={optionId}
                key={option.candidateId}
                title={`${hex} · ${option.recipe} · ΔE00 ${delta}`}
              >
                <input
                  id={optionId}
                  name={`cmyx-${pickerId}`}
                  type="radio"
                  value={option.candidateId}
                  checked={selected}
                  onChange={() => onSelect(resolution, option)}
                />
                <ColorSwatch hex={hex} size="large" />
                {selected ? <Check aria-hidden="true" /> : null}
                <span className="visually-hidden">
                  {hex}, {option.recipe}, color difference {delta}
                </span>
              </label>
            );
          })}
        </div>
        <div className="cmyx-palette-picker__selection" aria-live="polite">
          <ColorSwatch hex={selectedHex} size="medium" />
          <span>
            <strong>{selectedHex}</strong>
            <small>
              {resolution.recipe} · ΔE00 {resolution.deltaE00?.toFixed(1) ?? "unavailable"}
            </small>
          </span>
        </div>
      </fieldset>
    </details>
  );
}
