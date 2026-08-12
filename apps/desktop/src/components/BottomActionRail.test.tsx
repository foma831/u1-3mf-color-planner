// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import type { ComponentProps } from "react";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { BottomActionRail } from "./BottomActionRail";

function defaultProps(
  overrides: Partial<ComponentProps<typeof BottomActionRail>> = {},
): ComponentProps<typeof BottomActionRail> {
  return {
    isBusy: false,
    isValidating: false,
    planAvailable: true,
    exportAvailable: false,
    onExport: vi.fn(),
    onValidate: vi.fn(),
    validationAvailable: true,
    planNeedsRecalculation: false,
    colorNeedsRecalculation: false,
    conversionStageAvailable: false,
    conversionAvailable: false,
    conversionAdapters: [],
    isCheckingConversion: false,
    conversionLabel: "Review conversion…",
    onApprove: vi.fn(),
    ...overrides,
  };
}

afterEach(cleanup);

describe("BottomActionRail", () => {
  it("keeps project configuration out of the footer", () => {
    render(<BottomActionRail {...defaultProps()} />);

    expect(screen.queryByText("Plan setup")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Currently loaded T4 spool")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Validate plan" })).toBeVisible();
  });

  it("shows one contextual primary action", () => {
    const { rerender } = render(
      <BottomActionRail {...defaultProps({ planNeedsRecalculation: true })} />,
    );

    expect(
      screen.getByRole("button", { name: "Recalculate plan" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Review conversion…" }),
    ).not.toBeInTheDocument();

    rerender(
      <BottomActionRail
        {...defaultProps({
          exportAvailable: true,
          conversionStageAvailable: true,
          conversionAvailable: true,
        })}
      />,
    );
    expect(
      screen.queryByRole("button", { name: "Validate plan" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Review conversion…" }),
    ).toBeInTheDocument();
  });

  it("keeps an unavailable conversion action focusable and explains why", () => {
    const onApprove = vi.fn();
    render(
      <BottomActionRail
        {...defaultProps({
          exportAvailable: true,
          conversionStageAvailable: true,
          conversionAvailable: false,
          conversionBlockReason: "Install the qualified writer adapter first",
          onApprove,
        })}
      />,
    );

    const action = screen.getByRole("button", { name: "Review conversion…" });
    expect(action).not.toBeDisabled();
    expect(action).toHaveAttribute("aria-disabled", "true");
    expect(action).toHaveAccessibleDescription(
      "Install the qualified writer adapter first",
    );
    action.focus();
    expect(action).toHaveFocus();
    fireEvent.click(action);
    expect(onApprove).not.toHaveBeenCalled();
  });

  it("uses a true disabled state only while work is in progress", () => {
    render(
      <BottomActionRail
        {...defaultProps({
          isBusy: true,
          exportAvailable: true,
          conversionStageAvailable: true,
          conversionAvailable: true,
        })}
      />,
    );

    expect(
      screen.getByRole("button", { name: "Review conversion…" }),
    ).toBeDisabled();
  });

  it("shows writer evidence inside More actions", () => {
    render(
      <BottomActionRail
        {...defaultProps({
          conversionAdapters: [
            {
              target: "u1_full_spectrum",
              adapterId: "snapmaker-orca/2.3.5/u1-0.4-full-spectrum",
              slicer: "Snapmaker Orca",
              required: true,
              available: false,
              reason:
                "The installed profile pack does not match qualification evidence.",
              report: {
                applicationVersion: "2.3.5",
                executableSha256:
                  "4c30e59cf582dcc4f12e43741fcab2f97045e972065d465481ed3760678d0fbe",
                profilePack: { version: "02.02.53.02" },
              },
            },
          ],
        })}
      />,
    );

    fireEvent.click(screen.getByText("More actions"));
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
