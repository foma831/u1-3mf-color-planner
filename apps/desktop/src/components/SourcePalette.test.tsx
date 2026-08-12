// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createDemoPlan } from "../data/mock-plan";
import type { PlatePlan, SourceColor } from "../types";
import { SourcePaletteDetails, SourcePaletteSummary } from "./SourcePalette";
import { StrategyComparison } from "./StrategyComparison";

afterEach(cleanup);

const headColors: SourceColor[] = [
  ["F3", "#000000"],
  ["F5", "#8E9089"],
  ["F7", "#9D2235"],
  ["F1", "#C12E1F"],
  ["F2", "#F7E6DE"],
  ["F8", "#FEC600"],
  ["F4", "#FFFFFF"],
].map(([slot, hex]) => ({
  sourceSlots: [slot],
  sourceMaterial: "PLA",
  sourceHex: hex,
}));

function demoPlate(overrides: Partial<PlatePlan>): PlatePlan {
  return { ...createDemoPlan().plates[0], ...overrides };
}

describe("source palette presentation", () => {
  it("shows every original Head color even when Direct Spools is unavailable", () => {
    const plate = demoPlate({
      sourceColors: headColors,
      logicalColorCount: 7,
      effectivePairCount: 7,
      directEligible: false,
      mappings: undefined,
    });
    const { container } = render(<SourcePaletteSummary plate={plate} />);

    expect(screen.getByText("7 source colors")).toBeInTheDocument();
    expect(
      screen.getByText("7 semantic pairs · 7 Direct identities"),
    ).toBeInTheDocument();
    expect(container.querySelectorAll(".color-swatch")).toHaveLength(7);
  });

  it("explains why an original mono source may still remain on U1", () => {
    const plate = demoPlate({
      printer: "U1",
      sourceColors: [
        {
          sourceSlots: ["F6"],
          sourceMaterial: "PETG",
          sourceHex: "#ADB1B2",
        },
      ],
      logicalColorCount: 1,
      effectivePairCount: 1,
      isFastMono: false,
    });

    render(<SourcePaletteDetails plate={plate} />);

    expect(screen.getByText("F6")).toBeInTheDocument();
    expect(screen.getByText("#ADB1B2")).toBeInTheDocument();
    expect(
      screen.getByText(
        /assign the same compatible spool to every source mapping/i,
      ),
    ).toBeInTheDocument();
  });

  it("distinguishes the Direct one-to-one limit from CMY+X physical spools", () => {
    const onPrepareCustomPalette = vi.fn();
    const plate = demoPlate({
      sourceColors: headColors,
      logicalColorCount: 7,
      effectivePairCount: 7,
      directEligible: false,
      directEligibilityReason:
        "Direct Spools is unavailable because this scope uses 7 effective material-color pairs; the maximum is 4.",
      mappings: undefined,
    });

    render(
      <StrategyComparison
        plate={plate}
        spools={createDemoPlan().spools}
        onChange={vi.fn()}
        onPrepareCustomPalette={onPrepareCustomPalette}
      />,
    );

    expect(screen.getByText(/Prefer not to merge colors/i)).toHaveTextContent(
      /CMY\+X can reproduce the 7 semantic source pairs/i,
    );
    const createPalette = screen.getByRole("button", {
      name: "Create 4-spool palette",
    });
    expect(createPalette).toHaveAccessibleDescription(
      /every source identity and the source 3MF stay unchanged/i,
    );
    fireEvent.click(createPalette);
    expect(onPrepareCustomPalette).toHaveBeenCalledOnce();
  });

  it("keeps palette creation single-action while a replan is running", () => {
    const plate = demoPlate({
      logicalColorCount: 7,
      effectivePairCount: 7,
      directPairCount: 7,
      directEligible: false,
      mappings: undefined,
    });

    render(
      <StrategyComparison
        plate={plate}
        spools={createDemoPlan().spools}
        onChange={vi.fn()}
        isPreparingCustomPalette
        onPrepareCustomPalette={vi.fn()}
      />,
    );

    const createPalette = screen.getByRole("button", {
      name: "Creating palette…",
    });
    expect(createPalette).toBeDisabled();
    expect(createPalette).toHaveAttribute("aria-busy", "true");
  });

  it("reports the actual pair count when Direct is available", () => {
    const source = createDemoPlan().plates[0];
    const plate = demoPlate({
      directEligible: true,
      effectivePairCount: 1,
      logicalColorCount: 1,
      mappings: source.mappings?.slice(0, 1),
    });

    render(
      <StrategyComparison
        plate={plate}
        spools={createDemoPlan().spools}
        onChange={vi.fn()}
      />,
    );

    expect(
      screen.getByText(
        "1 semantic material-color-role pair use 1 physical Direct identity. Both strategies are valid.",
      ),
    ).toBeInTheDocument();
  });

  it("shows role-separated semantics sharing fewer physical Direct identities", () => {
    const plate = demoPlate({
      sourceColors: [
        ...headColors.slice(0, 4),
        { ...headColors[0], sourceSlots: ["support-F3"] },
      ],
      logicalColorCount: 4,
      effectivePairCount: 5,
      directPairCount: 4,
      directEligible: true,
    });

    render(<SourcePaletteDetails plate={plate} />);

    expect(
      screen.getByText("4 colors · 5 semantic pairs · 4 Direct identities"),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        /Role-separated source pairs .* share one physical spool/i,
      ),
    ).toBeInTheDocument();
  });
});
