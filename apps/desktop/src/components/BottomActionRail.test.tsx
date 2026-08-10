// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { BottomActionRail } from "./BottomActionRail";

afterEach(cleanup);

describe("BottomActionRail writer diagnostics", () => {
  it("shows the exact required adapter, slicer version, hashes, and reason", () => {
    render(
      <BottomActionRail
        isBusy={false}
        isValidating={false}
        planAvailable
        exportAvailable={false}
        onExport={vi.fn()}
        onValidate={vi.fn()}
        validationAvailable
        a1MiniEnabled={false}
        a1MiniNeedsRecalculation={false}
        a1MiniRoutingFixed={false}
        planNeedsRecalculation={false}
        colorNeedsRecalculation={false}
        onA1MiniChange={vi.fn()}
        currentT4SpoolId=""
        t4Options={[]}
        onCurrentT4Change={vi.fn()}
        conversionAvailable={false}
        isCheckingConversion={false}
        conversionAdapters={[
          {
            target: "u1_full_spectrum",
            adapterId: "snapmaker-orca/2.3.5/u1-0.4-full-spectrum",
            slicer: "Snapmaker Orca",
            required: true,
            available: false,
            reason: "The installed profile pack does not match qualification evidence.",
            report: {
              applicationVersion: "2.3.5",
              executableSha256:
                "4c30e59cf582dcc4f12e43741fcab2f97045e972065d465481ed3760678d0fbe",
              profilePack: { version: "02.02.53.02" },
            },
          },
        ]}
      />,
    );

    fireEvent.click(screen.getByText("Writer adapter checks"));

    expect(
      screen.getByText("Snapmaker U1 · Full Spectrum"),
    ).toBeInTheDocument();
    expect(screen.getByText("Blocked")).toBeInTheDocument();
    expect(screen.getByText("Snapmaker Orca 2.3.5")).toBeInTheDocument();
    expect(screen.getByText("02.02.53.02")).toBeInTheDocument();
    expect(
      screen.getByText(
        "The installed profile pack does not match qualification evidence.",
      ),
    ).toBeInTheDocument();
    expect(screen.getByText("4c30e59cf582…8d0fbe")).toHaveAttribute(
      "title",
      "4c30e59cf582dcc4f12e43741fcab2f97045e972065d465481ed3760678d0fbe",
    );
  });
});
