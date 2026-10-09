import { describe, expect, it } from "vitest";

import {
  fullSpectrumComponents,
  getFullSpectrumReferencePalette,
} from "./cmyx-reference";

describe("Full Spectrum reference palette", () => {
  it("matches the engine CMY search grid after visible-color deduplication", () => {
    const palette = getFullSpectrumReferencePalette("cmy");

    expect(palette).toHaveLength(46);
    expect(palette.map((color) => color.hex)).toContain("#08ABFB");
    expect(
      palette.every((color) =>
        color.recipes.every((recipe) =>
          recipe.components.every((component) => component.symbol !== "K"),
        ),
      ),
    ).toBe(true);
  });

  it("matches the theoretical four-component CMYK search grid", () => {
    const palette = getFullSpectrumReferencePalette("cmyk");

    expect(palette).toHaveLength(135);
    expect(palette.map((color) => color.hex)).toContain("#080A0D");
    expect(
      palette.some((color) =>
        color.recipes.some((recipe) =>
          recipe.components.some((component) => component.symbol === "K"),
        ),
      ),
    ).toBe(true);
  });

  it("keeps equivalent ratio and cycle recipes on one visible color", () => {
    const cyan = fullSpectrumComponents.cmy[0];
    const magenta = fullSpectrumComponents.cmy[1];
    const equivalent = getFullSpectrumReferencePalette("cmy").find((color) =>
      color.recipes.some(
        (recipe) =>
          recipe.mode === "Ratio" &&
          recipe.components[0] === cyan &&
          recipe.components[1] === magenta &&
          recipe.weights.join(":") === "4:4",
      ),
    );

    expect(equivalent?.recipes.map((recipe) => recipe.mode)).toEqual([
      "Ratio",
      "Cycle",
    ]);
  });
});
