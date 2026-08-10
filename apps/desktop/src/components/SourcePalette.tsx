import { Info } from "lucide-react";

import type { PlatePlan, SourceColor } from "../types";
import { ColorSwatch } from "./ColorSwatch";

function fallbackSourceColors(plate: PlatePlan): SourceColor[] {
  if (plate.sourceColors?.length) return plate.sourceColors;
  if (!plate.mappings) return [];
  return plate.mappings.map((mapping) => ({
    sourceSlots: mapping.sourceSlot
      .split(",")
      .map((slot) => slot.trim())
      .filter(Boolean),
    sourceMaterial: mapping.sourceMaterial,
    sourceHex: mapping.sourceHex,
  }));
}

function plural(count: number, singular: string, pluralForm = `${singular}s`) {
  return count === 1 ? singular : pluralForm;
}

function paletteCounts(plate: PlatePlan, colors: SourceColor[]) {
  const visualColorCount = colors.length
    ? new Set(colors.map((color) => color.sourceHex.toUpperCase())).size
    : plate.logicalColorCount;
  return {
    visualColorCount,
    effectivePairCount: plate.effectivePairCount ?? plate.logicalColorCount,
    directPairCount:
      plate.directPairCount ?? plate.effectivePairCount ?? plate.logicalColorCount,
  };
}

export function SourcePaletteSummary({ plate }: { plate: PlatePlan }) {
  const colors = fallbackSourceColors(plate);
  const { visualColorCount, effectivePairCount, directPairCount } = paletteCounts(
    plate,
    colors,
  );
  const description = colors.length
    ? colors
        .map(
          (color) =>
            `${color.sourceMaterial} ${color.sourceHex}, ${
              color.sourceSlots.join(", ") || "source slot unknown"
            }`,
        )
        .join("; ")
    : "Source swatches unavailable";

  return (
    <span className="source-palette-summary">
      <span className="source-palette-summary__swatches" aria-hidden="true">
        {colors.map((color, index) => (
          <ColorSwatch
            hex={color.sourceHex}
            size="small"
            key={`${color.sourceMaterial}-${color.sourceHex}-${index}`}
          />
        ))}
      </span>
      <span className="source-palette-summary__label">
        <strong>
          {visualColorCount} source {plural(visualColorCount, "color")}
        </strong>
        <small>
          {effectivePairCount} semantic {plural(effectivePairCount, "pair")} ·{" "}
          {directPairCount} Direct {plural(directPairCount, "identity", "identities")}
        </small>
      </span>
      <span className="visually-hidden">Source palette: {description}.</span>
    </span>
  );
}

export function SourcePaletteDetails({ plate }: { plate: PlatePlan }) {
  const colors = fallbackSourceColors(plate);
  const { visualColorCount, effectivePairCount, directPairCount } = paletteCounts(
    plate,
    colors,
  );
  const headingId = `source-palette-${plate.id}`;
  const sourceIsMono = visualColorCount === 1;

  return (
    <section className="source-palette-details" aria-labelledby={headingId}>
      <div className="source-palette-details__heading">
        <div>
          <h3 id={headingId}>Original source palette</h3>
          <p>Detected before CMY+X approximation or physical spool assignment.</p>
        </div>
        <span>
          {visualColorCount} {plural(visualColorCount, "color")} · {effectivePairCount}{" "}
          semantic {plural(effectivePairCount, "pair")} · {directPairCount} Direct{" "}
          {plural(directPairCount, "identity", "identities")}
        </span>
      </div>

      {colors.length ? (
        <ul className="source-palette-details__list">
          {colors.map((color, index) => (
            <li key={`${color.sourceMaterial}-${color.sourceHex}-${index}`}>
              <ColorSwatch hex={color.sourceHex} size="large" />
              <span>
                <strong>{color.sourceSlots.join(", ") || "Unknown source slot"}</strong>
                <small>{color.sourceMaterial}</small>
              </span>
              <code>{color.sourceHex}</code>
            </li>
          ))}
        </ul>
      ) : (
        <p className="source-palette-details__empty">
          Source swatches are unavailable for this compatibility plan.
        </p>
      )}

      {effectivePairCount !== visualColorCount ? (
        <p className="source-palette-details__note">
          <Info aria-hidden="true" />
          Identical visible colors remain separate semantic pairs when their
          material, source profile, or role differs.
        </p>
      ) : null}

      {directPairCount !== effectivePairCount ? (
        <p className="source-palette-details__note">
          <Info aria-hidden="true" />
          Role-separated source pairs with the same material, color, and declared
          profile share one physical spool and toolhead in Direct Spools mode.
        </p>
      ) : null}

      {plate.printer === "U1" && sourceIsMono ? (
        <p className="source-palette-details__note source-palette-details__note--a1">
          <Info aria-hidden="true" />
          {plate.isFastMono
            ? "The final recipe uses one physical spool. If A1 mini routing is enabled but this job remains on U1, check its geometry, material support, and whether a separate A1 spool identity is available."
            : "This result does not yet resolve to one physical spool. For A1 mini, select Direct Spools, assign the same compatible spool to every source mapping you want to merge, then recalculate."}
        </p>
      ) : null}
    </section>
  );
}
