// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { PlanWarnings } from "./PlanWarnings";

afterEach(cleanup);

describe("PlanWarnings", () => {
  it("discloses every global warning as readable text", () => {
    render(
      <PlanWarnings
        warnings={[
          "A shared spool needs operator confirmation.",
          "Verify the final slicer mapping.",
        ]}
      />,
    );

    const disclosure = screen
      .getByText("Project warnings")
      .closest("details");
    expect(disclosure).not.toBeNull();
    expect(disclosure).not.toHaveAttribute("open");

    fireEvent.click(screen.getByText("Project warnings"));
    expect(disclosure).toHaveAttribute("open");
    expect(
      screen.getByText("A shared spool needs operator confirmation."),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Verify the final slicer mapping."),
    ).toBeInTheDocument();
  });

  it("renders nothing when there are no global warnings", () => {
    const { container } = render(<PlanWarnings warnings={[]} />);
    expect(container).toBeEmptyDOMElement();
  });
});
