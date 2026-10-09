// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import type { ComponentProps } from "react";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createDemoPlan } from "../data/mock-plan";
import { ProjectSetupControl } from "./ProjectSetupControl";

const twoPrinterSetup = {
  schemaVersion: 1 as const,
  primaryPrinter: "u1" as const,
  secondaryPrinter: "a1-mini" as const,
};

function defaultProps(
  overrides: Partial<ComponentProps<typeof ProjectSetupControl>> = {},
): ComponentProps<typeof ProjectSetupControl> {
  return {
    intent: {
      defaultStrategy: "auto",
      a1MiniEnabled: true,
      allowU1CrossSourceRepacking: false,
      dedicatedSupportSpoolId: null,
      dedicatedSupportUsage: "interface-only",
    },
    loadout: {
      schemaVersion: 1,
      currentLoadout: [],
      currentA1SpoolId: null,
      updatedFrom: "manual",
    },
    equipmentSetup: twoPrinterSetup,
    availableSpools: createDemoPlan().spools,
    hasAnalysis: false,
    a1PlateCount: 0,
    isOpen: true,
    onOpenChange: vi.fn(),
    onConfirm: vi.fn(),
    onChangeEquipment: vi.fn(),
    ...overrides,
  };
}

afterEach(cleanup);

describe("ProjectSetupControl", () => {
  it("keeps printers and U1 printing methods visible in Step 2", () => {
    render(<ProjectSetupControl {...defaultProps()} />);

    expect(
      screen.getByRole("group", { name: "Printers for this project" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("group", { name: "U1 printing method" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("group", { name: "Filament loaded now" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("group", { name: "U1 plate layout" }),
    ).toBeInTheDocument();
    for (const toolhead of ["T1", "T2", "T3", "T4"]) {
      expect(screen.getByLabelText(`U1 ${toolhead}`)).toBeVisible();
    }
    expect(screen.getByLabelText("A1 mini external spool")).toBeVisible();
    expect(screen.getByRole("radio", { name: /Automatic/i })).toBeChecked();
    expect(
      screen.getByRole("radio", { name: /U1 \+ Bambu Lab A1 mini/i }),
    ).toBeChecked();
    expect(
      screen.getByRole("radio", { name: /Preserve source plates/i }),
    ).toBeChecked();
    expect(screen.getByText(/source 3MF is never modified/i)).toBeVisible();
    expect(screen.queryByText("Plan setup")).not.toBeInTheDocument();
  });

  it("submits a strict Direct plan without silently disabling A1", () => {
    const onConfirm = vi.fn();
    render(<ProjectSetupControl {...defaultProps({ onConfirm })} />);

    fireEvent.click(screen.getByRole("radio", { name: /Direct Spools only/i }));
    expect(
      screen.getByText(/Direct Spools only · U1 \+ A1 mini/i),
    ).toBeVisible();
    expect(
      screen.getByText(/Never fall back to CMY\+X silently/i),
    ).toBeVisible();
    fireEvent.click(
      screen.getByRole("button", { name: "Confirm project setup" }),
    );

    expect(onConfirm).toHaveBeenCalledWith(
      {
        defaultStrategy: "direct",
        a1MiniEnabled: true,
        allowU1CrossSourceRepacking: false,
        dedicatedSupportSpoolId: null,
        dedicatedSupportUsage: "interface-only",
      },
      {
        schemaVersion: 1,
        currentLoadout: [],
        currentA1SpoolId: null,
        updatedFrom: "manual",
      },
    );
    expect(screen.getByLabelText("U1 T1")).toBeVisible();
  });

  it("submits explicit compatible cross-source repacking", () => {
    const onConfirm = vi.fn();
    render(<ProjectSetupControl {...defaultProps({ onConfirm })} />);

    fireEvent.click(
      screen.getByRole("radio", {
        name: /Combine compatible source plates/i,
      }),
    );
    expect(
      screen.getByText(/strategy, physical loadout, material, and process/i),
    ).toBeVisible();
    fireEvent.click(
      screen.getByRole("button", { name: "Confirm project setup" }),
    );

    expect(onConfirm).toHaveBeenCalledWith(
      expect.objectContaining({ allowU1CrossSourceRepacking: true }),
      expect.any(Object),
    );
  });

  it("reserves an available PVA spool for support on T4", () => {
    const onConfirm = vi.fn();
    const pvaSpool = {
      id: "support-pva",
      calibrationIdentity: "support-pva",
      source: "user" as const,
      name: "Workshop PVA",
      colorName: "Natural",
      hex: "#F5F5E6",
      material: "PVA" as const,
      profile: "Snapmaker PVA @U1",
      colorBasis: "Nominal" as const,
      available: true,
    };
    render(
      <ProjectSetupControl
        {...defaultProps({
          onConfirm,
          availableSpools: [...createDemoPlan().spools, pvaSpool],
        })}
      />,
    );

    fireEvent.click(
      screen.getByRole("radio", { name: /Dedicated PVA on T4/i }),
    );
    expect(screen.getByLabelText("PVA spool")).toHaveValue("support-pva");
    expect(
      screen.getByRole("radio", { name: /Contact interface only/i }),
    ).toBeChecked();
    fireEvent.click(
      screen.getByRole("button", { name: "Confirm project setup" }),
    );

    expect(onConfirm).toHaveBeenCalledWith(
      expect.objectContaining({
        dedicatedSupportSpoolId: "support-pva",
        dedicatedSupportUsage: "interface-only",
      }),
      expect.any(Object),
    );
  });

  it("does not offer a PVA spool whose exact slicer profile is unknown", () => {
    const unknownPva = {
      id: "unknown-pva",
      calibrationIdentity: "unknown-pva",
      source: "user" as const,
      name: "Unknown PVA",
      colorName: "Natural",
      hex: "#F5F5E6",
      material: "PVA" as const,
      colorBasis: "Nominal" as const,
      available: true,
    };
    render(
      <ProjectSetupControl
        {...defaultProps({
          availableSpools: [...createDemoPlan().spools, unknownPva],
        })}
      />,
    );

    expect(
      screen.getByRole("radio", { name: /Dedicated PVA on T4/i }),
    ).toBeDisabled();
    expect(screen.getByText(/select its qualified profile/i)).toBeVisible();
  });

  it("keeps A1 routing unavailable for a saved U1-only equipment profile", () => {
    render(
      <ProjectSetupControl
        {...defaultProps({
          intent: {
            ...defaultProps().intent,
            a1MiniEnabled: false,
          },
          equipmentSetup: {
            schemaVersion: 1,
            primaryPrinter: "u1",
            secondaryPrinter: null,
          },
        })}
      />,
    );

    expect(
      screen.getByRole("radio", { name: /U1 \+ Bambu Lab A1 mini/i }),
    ).toBeDisabled();
    expect(screen.getByRole("button", { name: "Add A1 mini" })).toBeVisible();
    expect(
      screen.queryByLabelText("A1 mini external spool"),
    ).not.toBeInTheDocument();
  });

  it("submits the exact current physical loadout and prevents duplicate spools", () => {
    const onConfirm = vi.fn();
    render(<ProjectSetupControl {...defaultProps({ onConfirm })} />);

    fireEvent.change(screen.getByLabelText("U1 T1"), {
      target: { value: "panchroma-cyan" },
    });
    expect(
      screen
        .getByLabelText<HTMLSelectElement>("U1 T2")
        .querySelector('option[value="panchroma-cyan"]'),
    ).toBeDisabled();
    fireEvent.change(screen.getByLabelText("U1 T2"), {
      target: { value: "panchroma-magenta" },
    });
    fireEvent.change(screen.getByLabelText("U1 T3"), {
      target: { value: "panchroma-yellow" },
    });
    fireEvent.change(screen.getByLabelText("U1 T4"), {
      target: { value: "matte-grey" },
    });
    fireEvent.change(screen.getByLabelText("A1 mini external spool"), {
      target: { value: "burnt-orange" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Confirm project setup" }),
    );

    expect(onConfirm).toHaveBeenCalledWith(
      expect.objectContaining({ defaultStrategy: "auto" }),
      {
        schemaVersion: 1,
        currentLoadout: [
          { toolhead: "T1", spoolId: "panchroma-cyan" },
          { toolhead: "T2", spoolId: "panchroma-magenta" },
          { toolhead: "T3", spoolId: "panchroma-yellow" },
          { toolhead: "T4", spoolId: "matte-grey" },
        ],
        currentA1SpoolId: "burnt-orange",
        updatedFrom: "manual",
      },
    );
  });

  it("shows the selected filament color beside every native loadout selector", () => {
    render(<ProjectSetupControl {...defaultProps()} />);

    expect(screen.getAllByTitle("Unknown spool color")).toHaveLength(5);

    fireEvent.change(screen.getByLabelText("U1 T1"), {
      target: { value: "panchroma-cyan" },
    });
    fireEvent.change(screen.getByLabelText("A1 mini external spool"), {
      target: { value: "burnt-orange" },
    });

    expect(screen.getByTitle("Cyan · #08ABFB")).toBeVisible();
    expect(screen.getByTitle("Burnt Orange · #D97813")).toBeVisible();
    expect(screen.getAllByTitle("Unknown spool color")).toHaveLength(3);
  });

  it("reports requested A1 separately from a zero-plate planner result", () => {
    render(
      <ProjectSetupControl
        {...defaultProps({ hasAnalysis: true, isOpen: false, a1PlateCount: 0 })}
      />,
    );

    expect(
      screen.getByText("A1 mini requested · 0 eligible plates"),
    ).toBeVisible();
    expect(screen.getByText(/preference remains enabled/i)).toBeVisible();
    expect(screen.getByRole("button", { name: "Change setup" })).toHaveAttribute(
      "aria-controls",
      "project-setup-form",
    );
  });

  it("labels a fixed browser queue without changing a U1-only request", () => {
    render(
      <ProjectSetupControl
        {...defaultProps({
          intent: {
            ...defaultProps().intent,
            defaultStrategy: "direct",
            a1MiniEnabled: false,
          },
          hasAnalysis: true,
          isFixedPreview: true,
          isOpen: false,
          a1PlateCount: 2,
        })}
      />,
    );

    expect(
      screen.getByText("Direct Spools only", { exact: false }),
    ).toBeVisible();
    expect(
      screen.getByText("Fixed browser preview · 2 A1 mini plates"),
    ).toBeVisible();
    expect(screen.getByText(/fixed sample queue/i)).toBeVisible();
    expect(screen.getByRole("button", { name: "Change setup" })).toBeVisible();
  });

  it("shows a visible pending state and recalculation action after analysis", () => {
    render(
      <ProjectSetupControl
        {...defaultProps({ hasAnalysis: true, isStale: true })}
      />,
    );

    expect(screen.getByText("Changes pending")).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Apply & recalculate" }),
    ).toBeVisible();
  });
});
