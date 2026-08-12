// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import type { BatchSegment } from "../types";
import { BatchTimeline } from "./BatchTimeline";

afterEach(cleanup);

describe("BatchTimeline", () => {
  it("keeps the fixed-width timeline inside a keyboard-scrollable region", () => {
    const batches: BatchSegment[] = [
      {
        id: "batch-1",
        label: "CMY + Grey",
        detail: "CMY+X Full Spectrum",
        strategy: "cmyx",
        startOrder: 1,
        endOrder: 2,
        plateCount: 2,
        printer: "U1",
        setupActions: ["Load Grey in T4"],
      },
      {
        id: "batch-2",
        label: "Direct Spools",
        detail: "Full T1–T4 setup boundary",
        strategy: "direct",
        startOrder: 3,
        endOrder: 3,
        plateCount: 1,
        printer: "U1",
        setupActions: [],
      },
    ];

    render(<BatchTimeline batches={batches} totalPlates={3} />);

    const scrollRegion = screen.getByLabelText(
      "Scrollable print batch timeline",
    );
    expect(scrollRegion).toHaveAttribute("tabindex", "0");
    expect(
      screen.getByRole("list", { name: "Print batches" }),
    ).toBeInTheDocument();
    expect(screen.getByText("After Plate 02")).toBeInTheDocument();
  });
});
