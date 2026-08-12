// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { PrintingSetup } from "../services/printing-setup";
import { PrintingSetupControl } from "./PrintingSetupControl";

afterEach(cleanup);

describe("PrintingSetupControl", () => {
  it("saves a U1-only setup when no optional printer is selected", () => {
    const onSave = vi.fn();
    render(
      <PrintingSetupControl
        setup={null}
        isOpen
        onOpenChange={vi.fn()}
        onSave={onSave}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", { name: "Save printing setup" }),
    );

    expect(onSave).toHaveBeenCalledWith({
      schemaVersion: 1,
      primaryPrinter: "u1",
      secondaryPrinter: null,
    });
  });

  it("starts with the U1 and saves an optional A1 mini", () => {
    const onOpenChange = vi.fn();
    const onSave = vi.fn();
    render(
      <PrintingSetupControl
        setup={null}
        isOpen
        onOpenChange={onOpenChange}
        onSave={onSave}
      />,
    );

    expect(screen.getByText("Primary printer · Always included")).toBeVisible();
    expect(
      screen.queryByRole("radio", { name: /Direct Spools|CMY/i }),
    ).not.toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("checkbox", { name: /Bambu Lab A1 mini/i }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Save printing setup" }),
    );

    expect(onSave).toHaveBeenCalledWith({
      schemaVersion: 1,
      primaryPrinter: "u1",
      secondaryPrinter: "a1-mini",
    });
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("summarizes a saved two-printer setup without choosing routing", () => {
    const setup: PrintingSetup = {
      schemaVersion: 1,
      primaryPrinter: "u1",
      secondaryPrinter: "a1-mini",
    };
    render(
      <PrintingSetupControl
        setup={setup}
        isOpen={false}
        onOpenChange={vi.fn()}
        onSave={vi.fn()}
      />,
    );

    expect(screen.getByText("2 printers available")).toBeInTheDocument();
    expect(
      screen.queryByRole("radio", { name: /Direct Spools|CMY/i }),
    ).not.toBeInTheDocument();
    expect(screen.getByText("Saved")).toBeInTheDocument();
  });

  it("does not claim permanent persistence when storage is unavailable", () => {
    const setup: PrintingSetup = {
      schemaVersion: 1,
      primaryPrinter: "u1",
      secondaryPrinter: null,
    };
    render(
      <PrintingSetupControl
        setup={setup}
        persistenceStatus="session"
        isOpen={false}
        onOpenChange={vi.fn()}
        onSave={vi.fn()}
      />,
    );

    expect(screen.getByText("Available this session")).toBeInTheDocument();
    expect(screen.queryByText("Saved")).not.toBeInTheDocument();
  });
});
