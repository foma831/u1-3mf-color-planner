// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createDemoPlan } from "../data/mock-plan";
import { PlanWarnings } from "./PlanWarnings";
import { PlateTable } from "./PlateTable";

afterEach(() => {
  cleanup();
});

describe("PlateTable warnings", () => {
  it("separates printable targets from blocked resolution rows", () => {
    const plan = createDemoPlan();
    const printable = Array.from({ length: 5 }, (_, index) => ({
      ...plan.plates[0],
      id: `printable-${index + 1}`,
      scopeId: `printable-scope-${index + 1}`,
      title: `Printable plate ${index + 1}`,
      order: index + 1,
      printer: "U1" as const,
      planningStatus: "printable" as const,
    }));
    const blocked = {
      ...plan.plates[0],
      id: "blocked-frame",
      scopeId: "blocked-frame-scope",
      title: "Frame — omitted units",
      order: 6,
      printer: "U1" as const,
      planningStatus: "blocked" as const,
    };

    render(
      <PlateTable
        plates={[...printable, blocked]}
        spools={plan.spools}
        selectedPlateId=""
        onSelectPlate={vi.fn()}
        a1MiniEnabled={false}
        hasPendingPlanChanges={false}
        onBulkStrategyChange={vi.fn()}
      />,
    );

    expect(screen.getByText("5 printable · 1 blocked")).toBeInTheDocument();
    expect(
      screen.getByText("5 printable · 1 blocked total"),
    ).toBeInTheDocument();
    expect(screen.queryByText("6 planned")).not.toBeInTheDocument();
    expect(
      screen.getByText(
        "This plan preserves the source project's U1 plate boundaries. Objects from different source plates are not combined.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        /Blocked rows show omitted source units that must be resolved; they are not printable target plates\./,
      ),
    ).toBeInTheDocument();
  });

  it("uses compact columns by default and reveals expert detail on request", () => {
    const plan = createDemoPlan();
    render(
      <PlateTable
        plates={[plan.plates[0]]}
        spools={plan.spools}
        selectedPlateId=""
        onSelectPlate={vi.fn()}
        a1MiniEnabled={false}
        hasPendingPlanChanges={false}
        onBulkStrategyChange={vi.fn()}
      />,
    );

    expect(
      screen.queryByRole("columnheader", { name: "Source Palette" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("columnheader", { name: "Status" }),
    ).toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", { name: "Show detailed columns" }),
    );
    expect(
      screen.getByRole("columnheader", { name: "Source Palette" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Use compact columns" }),
    ).toHaveAttribute("aria-pressed", "true");
  });

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
        onBulkStrategyChange={vi.fn()}
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
        onBulkStrategyChange={vi.fn()}
      />,
    );

    expect(screen.getByText("1 warning").closest("summary")).not.toBeNull();
  });
});

describe("PlateTable bulk U1 strategy", () => {
  it("applies Direct Spools to every selected linked source scope", () => {
    const plan = createDemoPlan();
    const first = {
      ...plan.plates[0],
      id: "bulk-plate-1",
      scopeId: "bulk-scope-1",
      printer: "U1" as const,
      directEligible: true,
      directPairCount: 2,
    };
    const second = {
      ...first,
      id: "bulk-plate-2",
      scopeId: "bulk-scope-2",
      title: "Bulk target plate 2",
    };
    const onBulkStrategyChange = vi.fn();

    render(
      <PlateTable
        plates={[first, second]}
        spools={plan.spools}
        selectedPlateId=""
        onSelectPlate={vi.fn()}
        a1MiniEnabled={false}
        hasPendingPlanChanges={false}
        onBulkStrategyChange={onBulkStrategyChange}
      />,
    );

    fireEvent.click(screen.getByText("Bulk strategy changes"));
    fireEvent.click(screen.getByLabelText("Select all U1 plates"));
    fireEvent.click(
      screen.getByRole("button", { name: "Set selected to Direct Spools" }),
    );

    expect(onBulkStrategyChange).toHaveBeenCalledWith(
      ["bulk-scope-1", "bulk-scope-2"],
      "direct",
    );
    expect(
      screen.getByLabelText(`Select ${first.title} for bulk strategy change`),
    ).toBeChecked();
    expect(
      screen.getByLabelText(`Select ${second.title} for bulk strategy change`),
    ).toBeChecked();
  });

  it("links split target plates from the same source scope", () => {
    const plan = createDemoPlan();
    const first = {
      ...plan.plates[0],
      id: "split-plate-1",
      scopeId: "shared-scope",
      printer: "U1" as const,
      directEligible: true,
      directPairCount: 2,
    };
    const second = {
      ...first,
      id: "split-plate-2",
      title: "Split target plate 2",
    };
    const onBulkStrategyChange = vi.fn();

    render(
      <PlateTable
        plates={[first, second]}
        spools={plan.spools}
        selectedPlateId=""
        onSelectPlate={vi.fn()}
        a1MiniEnabled={false}
        hasPendingPlanChanges={false}
        onBulkStrategyChange={onBulkStrategyChange}
      />,
    );

    fireEvent.click(screen.getByText("Bulk strategy changes"));
    fireEvent.click(
      screen.getByLabelText(`Select ${first.title} for bulk strategy change`),
    );

    expect(
      screen.getByLabelText(`Select ${first.title} for bulk strategy change`),
    ).toBeChecked();
    expect(
      screen.getByLabelText(
        "Select Split target plate 2 for bulk strategy change",
      ),
    ).toBeChecked();
    fireEvent.click(
      screen.getByRole("button", { name: "Set selected to Direct Spools" }),
    );
    expect(onBulkStrategyChange).toHaveBeenCalledWith(
      ["shared-scope"],
      "direct",
    );
  });

  it("blocks a bulk Direct action while a selected scope is unavailable", () => {
    const plan = createDemoPlan();
    const plate = {
      ...plan.plates[0],
      printer: "U1" as const,
      directEligible: false,
      directEligibilityReason: "Too many physical identities",
    };

    render(
      <PlateTable
        plates={[plate]}
        spools={plan.spools}
        selectedPlateId=""
        onSelectPlate={vi.fn()}
        a1MiniEnabled={false}
        hasPendingPlanChanges={false}
        onBulkStrategyChange={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByText("Bulk strategy changes"));
    fireEvent.click(screen.getByLabelText("Select all U1 plates"));

    expect(
      screen.getByRole("button", { name: "Set selected to Direct Spools" }),
    ).toBeDisabled();
    expect(
      screen.getByText(
        /Open an unavailable plate and choose Create 4-spool palette/i,
      ),
    ).toBeInTheDocument();
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
