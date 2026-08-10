// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { WorkflowRail } from "./WorkflowRail";

afterEach(cleanup);

describe("WorkflowRail", () => {
  it("starts at Project before analysis", () => {
    render(
      <WorkflowRail
        hasAnalysis={false}
        hasPendingPlanChanges={false}
        planReady={false}
      />,
    );

    expect(screen.getByText("Project").closest("li")).toHaveAttribute(
      "aria-current",
      "step",
    );
    expect(screen.getByText("Materials").closest("li")).toHaveClass(
      "workflow-step--upcoming",
    );
  });

  it("returns to Color Strategy while edits need recalculation", () => {
    render(
      <WorkflowRail
        hasAnalysis
        hasPendingPlanChanges
        planReady={false}
      />,
    );

    expect(screen.getByText("Color Strategy").closest("li")).toHaveAttribute(
      "aria-current",
      "step",
    );
    expect(screen.getByText("Project").closest("li")).toHaveClass(
      "workflow-step--complete",
    );
  });

  it("moves to Validate only when the authoritative plan is ready", () => {
    render(
      <WorkflowRail
        hasAnalysis
        hasPendingPlanChanges={false}
        planReady
      />,
    );

    expect(screen.getByText("Validate").closest("li")).toHaveAttribute(
      "aria-current",
      "step",
    );
    expect(screen.getByText("Print Plan").closest("li")).toHaveClass(
      "workflow-step--complete",
    );
  });
});
