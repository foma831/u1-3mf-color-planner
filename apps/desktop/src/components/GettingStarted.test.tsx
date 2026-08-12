// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { GettingStarted } from "./GettingStarted";

afterEach(cleanup);

describe("GettingStarted", () => {
  it("guides a first-time user to confirm physical inventory", () => {
    const onOpenFilamentLibrary = vi.fn();
    render(
      <GettingStarted
        availableSpoolCount={0}
        isInventoryLoading={false}
        isProjectSetupConfirmed={false}
        projectSetup={<div>Visible project setup</div>}
        isAnalyzing={false}
        onOpenFilamentLibrary={onOpenFilamentLibrary}
        onOpenProject={vi.fn()}
        onAnalyzeProject={vi.fn()}
      />,
    );

    expect(
      screen.getByRole("heading", { name: "Create a print-ready plan" }),
    ).toBeInTheDocument();
    expect(screen.getByText(/physically available now/i)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Confirm inventory" }));
    expect(onOpenFilamentLibrary).toHaveBeenCalledOnce();
    expect(
      screen.queryByRole("button", { name: "Open 3MF from Step 3" }),
    ).not.toBeInTheDocument();
  });

  it("makes Step 2 current and keeps the project setup visible after inventory", () => {
    render(
      <GettingStarted
        availableSpoolCount={4}
        isInventoryLoading={false}
        isProjectSetupConfirmed={false}
        projectSetup={<div>Visible project setup</div>}
        isAnalyzing={false}
        onOpenFilamentLibrary={vi.fn()}
        onOpenProject={vi.fn()}
        onAnalyzeProject={vi.fn()}
      />,
    );

    expect(
      screen
        .getByRole("heading", { name: "Choose how to print this project" })
        .closest("li"),
    ).toHaveAttribute("aria-current", "step");
    expect(screen.getByText("Visible project setup")).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "Open 3MF from Step 3" }),
    ).not.toBeInTheDocument();
  });

  it("makes Step 3 current and opens the project picker after setup", () => {
    const onOpenProject = vi.fn();
    render(
      <GettingStarted
        availableSpoolCount={4}
        isInventoryLoading={false}
        isProjectSetupConfirmed
        projectSetup={<div>Confirmed project setup</div>}
        isAnalyzing={false}
        onOpenFilamentLibrary={vi.fn()}
        onOpenProject={onOpenProject}
        onAnalyzeProject={vi.fn()}
      />,
    );

    expect(
      screen.getByRole("heading", { name: "Open and analyze a 3MF" })
        .closest("li"),
    ).toHaveAttribute("aria-current", "step");
    fireEvent.click(
      screen.getByRole("button", { name: "Open 3MF from Step 3" }),
    );
    expect(onOpenProject).toHaveBeenCalledOnce();
  });

  it("offers analysis inside Step 3 after a project is selected", () => {
    const onAnalyzeProject = vi.fn();
    render(
      <GettingStarted
        availableSpoolCount={4}
        isInventoryLoading={false}
        isProjectSetupConfirmed
        projectSetup={<div>Confirmed project setup</div>}
        pendingFileName="fixture.3mf"
        isAnalyzing={false}
        onOpenFilamentLibrary={vi.fn()}
        onOpenProject={vi.fn()}
        onAnalyzeProject={onAnalyzeProject}
      />,
    );

    expect(
      screen.getByText(/4 physical spools are marked available/i),
    ).toBeInTheDocument();
    expect(screen.getByText(/fixture\.3mf is ready/i)).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "Analyze Project from Step 3" }),
    );
    expect(onAnalyzeProject).toHaveBeenCalledOnce();
  });
});
