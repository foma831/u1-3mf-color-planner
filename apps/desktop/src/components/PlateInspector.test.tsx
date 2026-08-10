// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createDemoPlan } from "../data/mock-plan";
import { PlateInspector } from "./PlateInspector";

afterEach(cleanup);

function renderInspector(isFastMono: boolean) {
  const plan = createDemoPlan();
  const plate = {
    ...plan.plates[0],
    logicalColorCount: 3,
    isFastMono,
  };
  plan.plates = [plate, ...plan.plates.slice(1)];
  const onPrinterPreferenceChange = vi.fn();

  render(
    <PlateInspector
      plan={plan}
      plate={plate}
      restoreCmy={false}
      onStrategyChange={vi.fn()}
      onRestoreChange={vi.fn()}
      onToolheadChange={vi.fn()}
      onSpoolChange={vi.fn()}
      onMaterialSubstitutionChange={vi.fn()}
      a1MiniEnabled
      onPrinterPreferenceChange={onPrinterPreferenceChange}
      onAddSpool={vi.fn()}
    />,
  );
  return onPrinterPreferenceChange;
}

describe("PlateInspector A1 mini routing", () => {
  it("allows a multi-source-color plate that resolves to one physical spool", () => {
    const onPrinterPreferenceChange = renderInspector(true);

    expect(
      screen.getByText("Prepare this plate for", { selector: "legend" }),
    ).toBeInTheDocument();
    const a1Option = screen.getByRole("radio", {
      name: /Bambu Lab A1 mini/i,
    });
    expect(a1Option).toBeEnabled();

    fireEvent.click(a1Option);
    expect(onPrinterPreferenceChange).toHaveBeenCalledWith("a1-mini");
  });

  it("shows why recalculation is required before a multi-color plate becomes eligible", () => {
    renderInspector(false);

    expect(
      screen.getByRole("radio", { name: /Bambu Lab A1 mini/i }),
    ).toBeDisabled();
    expect(
      screen.getByText(/assign the same compatible spool.*then recalculate/i),
    ).toBeInTheDocument();
  });
});
