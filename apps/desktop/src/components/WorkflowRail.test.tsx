// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { WorkflowRail } from "./WorkflowRail";

afterEach(cleanup);

const baseProps = {
  inventoryReady: true,
  projectSetupReady: true,
  hasAnalysis: false,
  needsAttention: false,
  validatedPlanAvailable: false,
  printRunAvailable: false,
};

describe("WorkflowRail", () => {
  it("starts with inventory before physical spools are confirmed", () => {
    render(<WorkflowRail {...baseProps} inventoryReady={false} />);

    expect(screen.getByText("Inventory").closest("li")).toHaveAttribute(
      "aria-current",
      "step",
    );
    expect(screen.getByText("Setup").closest("li")).toHaveClass(
      "workflow-step--upcoming",
    );
  });

  it("makes Project setup Step 2 after inventory", () => {
    render(<WorkflowRail {...baseProps} projectSetupReady={false} />);

    expect(screen.getByText("Inventory").closest("li")).toHaveClass(
      "workflow-step--complete",
    );
    expect(screen.getByText("Setup").closest("li")).toHaveAttribute(
      "aria-current",
      "step",
    );
    expect(screen.getByText("Analyze").closest("li")).toHaveClass(
      "workflow-step--upcoming",
    );
  });

  it("moves to analysis after initial setup is ready", () => {
    render(<WorkflowRail {...baseProps} />);

    expect(screen.getByText("Setup").closest("li")).toHaveClass(
      "workflow-step--complete",
    );
    expect(screen.getByText("Analyze").closest("li")).toHaveAttribute(
      "aria-current",
      "step",
    );
  });

  it("returns to Resolve while choices need attention", () => {
    render(<WorkflowRail {...baseProps} hasAnalysis needsAttention />);

    expect(screen.getByText("Resolve").closest("li")).toHaveAttribute(
      "aria-current",
      "step",
    );
    expect(screen.getByText("Analyze").closest("li")).toHaveClass(
      "workflow-step--complete",
    );
  });

  it("moves from conversion to printing after publication", () => {
    const { rerender } = render(
      <WorkflowRail {...baseProps} hasAnalysis validatedPlanAvailable />,
    );
    expect(screen.getByText("Convert").closest("li")).toHaveAttribute(
      "aria-current",
      "step",
    );

    rerender(
      <WorkflowRail
        {...baseProps}
        hasAnalysis
        validatedPlanAvailable
        printRunAvailable
      />,
    );
    expect(screen.getByText("Print").closest("li")).toHaveAttribute(
      "aria-current",
      "step",
    );
  });
});
