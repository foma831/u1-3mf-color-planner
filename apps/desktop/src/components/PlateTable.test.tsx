// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { createDemoPlan } from "../data/mock-plan";
import { PlanWarnings } from "./PlanWarnings";
import { PlateTable } from "./PlateTable";

describe("PlateTable warnings", () => {
  it("exposes the full plate warning text through a native disclosure", () => {
    const plan = createDemoPlan();
    const plate = {
      ...plan.plates[0],
      warnings: [
        "Confirm the T4 spool before printing.",
        "Review the generated purge tower in the slicer.",
      ],
    };

    render(
      <PlateTable
        plates={[plate]}
        spools={plan.spools}
        selectedPlateId={plate.id}
        onSelectPlate={vi.fn()}
        a1MiniEnabled={false}
        hasPendingPlanChanges={false}
      />,
    );

    const summary = screen.getByText("2 warnings").closest("summary");
    const disclosure = summary?.closest("details");

    expect(summary).not.toBeNull();
    expect(disclosure).not.toHaveAttribute("open");
    fireEvent.click(summary!);
    expect(disclosure).toHaveAttribute("open");
    expect(
      screen.getByText("Confirm the T4 spool before printing."),
    ).toBeVisible();
    expect(
      screen.getByText("Review the generated purge tower in the slicer."),
    ).toBeVisible();
  });

  it("uses singular warning text when a plate has one warning", () => {
    const plan = createDemoPlan();
    const plate = {
      ...plan.plates[0],
      warnings: ["Review this plate."],
    };

    render(
      <PlateTable
        plates={[plate]}
        spools={plan.spools}
        selectedPlateId=""
        onSelectPlate={vi.fn()}
        a1MiniEnabled={false}
        hasPendingPlanChanges={false}
      />,
    );

    expect(screen.getByText("1 warning").closest("summary")).not.toBeNull();
  });
});

describe("PlanWarnings", () => {
  it("renders global warning text in an accessible disclosure", () => {
    render(
      <PlanWarnings
        warnings={[
          "The selected inventory requires a manual review.",
          "Verify all generated projects before slicing.",
        ]}
      />,
    );

    const summary = screen.getByText("Project warnings").closest("summary");
    const disclosure = summary?.closest("details");

    expect(
      screen.getByRole("region", { name: "Project warnings" }),
    ).toBeInTheDocument();
    expect(disclosure).not.toHaveAttribute("open");
    fireEvent.click(summary!);
    expect(disclosure).toHaveAttribute("open");
    expect(
      screen.getByText("The selected inventory requires a manual review."),
    ).toBeVisible();
    expect(
      screen.getByText("Verify all generated projects before slicing."),
    ).toBeVisible();
  });

  it("renders nothing when there are no global warnings", () => {
    const { container } = render(<PlanWarnings warnings={[]} />);

    expect(container).toBeEmptyDOMElement();
  });
});
