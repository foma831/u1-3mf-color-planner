export type FullSpectrumReferenceMode = "cmy" | "cmyk";
export type FullSpectrumRecipeMode = "Solid" | "Ratio" | "Cycle";

export interface FullSpectrumComponent {
  symbol: "C" | "M" | "Y" | "K";
  toolhead: "T1" | "T2" | "T3" | "T4";
  name: string;
  hex: string;
}

export interface FullSpectrumReferenceRecipe {
  mode: FullSpectrumRecipeMode;
  components: FullSpectrumComponent[];
  weights: number[];
  label: string;
}

export interface FullSpectrumReferenceColor {
  hex: string;
  recipes: FullSpectrumReferenceRecipe[];
}

export const fullSpectrumComponents = {
  cmy: [
    { symbol: "C", toolhead: "T1", name: "Cyan", hex: "#08ABFB" },
    { symbol: "M", toolhead: "T2", name: "Magenta", hex: "#D93B90" },
    { symbol: "Y", toolhead: "T3", name: "Yellow", hex: "#F9ED3D" },
  ],
  cmyk: [
    { symbol: "C", toolhead: "T1", name: "Cyan", hex: "#08ABFB" },
    { symbol: "M", toolhead: "T2", name: "Magenta", hex: "#D93B90" },
    { symbol: "Y", toolhead: "T3", name: "Yellow", hex: "#F9ED3D" },
    { symbol: "K", toolhead: "T4", name: "Black", hex: "#080A0D" },
  ],
} satisfies Record<FullSpectrumReferenceMode, FullSpectrumComponent[]>;

const RATIO_DENOMINATOR = 8;

function parseHex(hex: string) {
  return [
    Number.parseInt(hex.slice(1, 3), 16),
    Number.parseInt(hex.slice(3, 5), 16),
    Number.parseInt(hex.slice(5, 7), 16),
  ];
}

function srgbChannelToLinear(channel: number) {
  const value = channel / 255;
  return value <= 0.04045
    ? value / 12.92
    : ((value + 0.055) / 1.055) ** 2.4;
}

function linearChannelToSrgb(channel: number) {
  const value =
    channel <= 0.0031308
      ? 12.92 * channel
      : 1.055 * channel ** (1 / 2.4) - 0.055;
  return Math.round(Math.min(1, Math.max(0, value)) * 255);
}

function formatHex(channels: number[]) {
  return `#${channels
    .map((channel) => channel.toString(16).padStart(2, "0"))
    .join("")}`.toUpperCase();
}

function predictColor(
  components: FullSpectrumComponent[],
  weights: number[],
) {
  const denominator = weights.reduce((total, weight) => total + weight, 0);
  const colors = components.map((component) => parseHex(component.hex));
  return formatHex(
    [0, 1, 2].map((channel) =>
      linearChannelToSrgb(
        colors.reduce(
          (total, color, index) =>
            total + srgbChannelToLinear(color[channel]) * weights[index],
          0,
        ) / denominator,
      ),
    ),
  );
}

function combinations<T>(values: T[], count: number) {
  const output: T[][] = [];
  const visit = (start: number, selected: T[]) => {
    if (selected.length === count) {
      output.push([...selected]);
      return;
    }
    for (let index = start; index < values.length; index += 1) {
      selected.push(values[index]);
      visit(index + 1, selected);
      selected.pop();
    }
  };
  visit(0, []);
  return output;
}

function positiveCompositions(total: number, count: number) {
  const output: number[][] = [];
  const visit = (remaining: number, remainingCount: number, values: number[]) => {
    if (remainingCount === 1) {
      if (remaining > 0) output.push([...values, remaining]);
      return;
    }
    const maximum = remaining - (remainingCount - 1);
    for (let value = 1; value <= maximum; value += 1) {
      visit(remaining - value, remainingCount - 1, [...values, value]);
    }
  };
  visit(total, count, []);
  return output;
}

function recipeLabel(
  mode: FullSpectrumRecipeMode,
  components: FullSpectrumComponent[],
  weights: number[],
) {
  if (mode === "Solid") {
    return `${components[0].toolhead} (${components[0].symbol})`;
  }
  const toolheads = components.map((component) => component.toolhead).join(":");
  const symbols = components.map((component) => component.symbol).join(":");
  if (mode === "Cycle") {
    return `${components.map((component) => component.toolhead).join(" → ")} (${components
      .map((component) => component.symbol)
      .join(" → ")})`;
  }
  return `${toolheads} = ${weights.join(":")} (${symbols})`;
}

function buildRecipe(
  mode: FullSpectrumRecipeMode,
  components: FullSpectrumComponent[],
  weights: number[],
): FullSpectrumReferenceRecipe {
  return {
    mode,
    components,
    weights,
    label: recipeLabel(mode, components, weights),
  };
}

function enumerateRecipes(mode: FullSpectrumReferenceMode) {
  const components = fullSpectrumComponents[mode];
  const recipes: FullSpectrumReferenceRecipe[] = components.map((component) =>
    buildRecipe("Solid", [component], [1]),
  );

  for (let count = 2; count <= Math.min(components.length, 3); count += 1) {
    for (const selected of combinations(components, count)) {
      for (const weights of positiveCompositions(RATIO_DENOMINATOR, count)) {
        recipes.push(buildRecipe("Ratio", selected, weights));
      }
    }
  }

  for (let count = 2; count <= Math.min(components.length, 4); count += 1) {
    for (const selected of combinations(components, count)) {
      recipes.push(buildRecipe("Cycle", selected, selected.map(() => 1)));
    }
  }
  return recipes;
}

const paletteCache = new Map<
  FullSpectrumReferenceMode,
  FullSpectrumReferenceColor[]
>();

export function getFullSpectrumReferencePalette(
  mode: FullSpectrumReferenceMode,
) {
  const cached = paletteCache.get(mode);
  if (cached) return cached;

  const colors = new Map<string, FullSpectrumReferenceRecipe[]>();
  for (const recipe of enumerateRecipes(mode)) {
    const hex = predictColor(recipe.components, recipe.weights);
    const equivalentRecipes = colors.get(hex) ?? [];
    equivalentRecipes.push(recipe);
    colors.set(hex, equivalentRecipes);
  }
  const palette = [...colors].map(([hex, recipes]) => ({ hex, recipes }));
  paletteCache.set(mode, palette);
  return palette;
}
