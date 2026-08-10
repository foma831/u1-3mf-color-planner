// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import type { ComponentProps } from "react";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { PreparedConversion, PublishedConversionArtifact } from "../types";
import { ConversionDialog } from "./ConversionDialog";

const u1Slot = {
  toolhead: "T1",
  spoolId: "cyan",
  spoolName: "Panchroma Cyan",
  material: "PLA",
  color: "#00AEEF",
  profile: "Panchroma Translucent PLA",
  settingId: "cyan-setting",
  filamentId: "cyan-filament",
};

const excludedUnit = {
  scopeId: "plate-8",
  planningUnitId: "plate-8-unit-2",
  sourcePlateId: 8,
  sourceUnitId: "model-42",
  reason: "No schedulable physical loadout is available.",
  errorIdentity: "partial-error-v1:abc",
};

const prepared: PreparedConversion = {
  preparationToken: "preparation-token",
  conversionId: "conversion-id",
  preparation: {
    adapterId: "u1-planner/mixed-native",
    sourceSha256: "source-sha",
    planFingerprint: "plan-fingerprint",
    bundleDirectoryName: "fixture__converted",
    artifacts: [
      {
        adapterId: "snapmaker/u1-direct",
        target: "u1_direct",
        printer: "Snapmaker U1",
        strategy: "Direct Spools",
        slicer: "Snapmaker Orca",
        batchId: "direct-1",
        fileName: "fixture__u1_direct.3mf",
        targetPlateIds: ["plate-1"],
        sourceUnitIds: ["unit-1"],
        loadout: [u1Slot],
        setupActions: ["Load cyan in T1"],
        adapterEvidence: { status: "prepared" },
      },
      {
        adapterId: "snapmaker/u1-full-spectrum",
        target: "u1_full_spectrum",
        printer: "Snapmaker U1",
        strategy: "CMY+X Full Spectrum",
        slicer: "Snapmaker Orca",
        batchId: "full-1",
        fileName: "fixture__u1_full_spectrum.3mf",
        targetPlateIds: ["plate-2", "plate-3"],
        sourceUnitIds: ["unit-2", "unit-3"],
        loadout: [u1Slot],
        setupActions: [],
        adapterEvidence: { status: "prepared" },
      },
      {
        adapterId: "bambu/a1-mini",
        target: "a1_mini_mono",
        printer: "Bambu Lab A1 mini",
        strategy: "A1 Mono",
        slicer: "Bambu Studio",
        batchId: "a1-1",
        fileName: "fixture__a1_mini.3mf",
        targetPlateIds: ["plate-4"],
        sourceUnitIds: ["unit-4"],
        loadout: [
          {
            ...u1Slot,
            toolhead: "External spool",
            spoolId: "black",
            spoolName: "Black PLA",
            color: "#171717",
          },
        ],
        setupActions: [],
        adapterEvidence: { status: "prepared" },
      },
    ],
    warnings: ["Verify every generated project before slicing."],
    excludedSourceUnits: [],
  },
};

function publishedArtifact(
  overrides: Partial<PublishedConversionArtifact>,
): PublishedConversionArtifact {
  return {
    adapterId: "snapmaker/u1-direct",
    target: "u1_direct",
    printer: "Snapmaker U1",
    strategy: "Direct Spools",
    slicer: "Snapmaker Orca",
    batchId: "direct-1",
    fileName: "fixture__u1_direct.3mf",
    relativePath: "projects/fixture__u1_direct.3mf",
    path: "/output/projects/fixture__u1_direct.3mf",
    byteSize: 2 * 1024 * 1024,
    sha256: "0123456789abcdef0123456789abcdef",
    plateCount: 1,
    targetPlateIds: ["plate-1"],
    sourceUnitIds: ["unit-1"],
    loadout: [u1Slot],
    setupActions: ["Load cyan in T1"],
    validationStatus: "Passed",
    adapterEvidence: { status: "passed" },
    ...overrides,
  };
}

function defaultProps(): ComponentProps<typeof ConversionDialog> {
  return {
    prepared,
    destinationDirectory: "/output",
    isConverting: false,
    progress: null,
    result: null,
    error: "",
    needsNewPreflight: false,
    activeOutputAction: null,
    onConvert: vi.fn(),
    onCancelConversion: vi.fn(),
    onOpenOutput: vi.fn(),
    onShowOutputInFinder: vi.fn(),
    onRetryPreflight: vi.fn(),
    onClose: vi.fn(),
  };
}

afterEach(cleanup);

describe("ConversionDialog", () => {
  it("groups mixed preparation artifacts by printer and slicer", () => {
    const props = defaultProps();
    render(<ConversionDialog {...props} />);

    const detailsRegion = screen.getByRole("region", {
      name: "Conversion details",
    });
    const actions = screen.getByRole("button", {
      name: "Cancel",
    }).parentElement;
    expect(detailsRegion).toHaveAttribute("tabindex", "0");
    expect(actions).toHaveClass("conversion-dialog__actions");
    expect(detailsRegion.nextElementSibling).toBe(actions);

    expect(
      screen.getByRole("heading", { name: "3 project files" }),
    ).toBeInTheDocument();

    const u1Group = screen
      .getByRole("heading", { name: "Snapmaker U1" })
      .closest("section");
    const a1Group = screen
      .getByRole("heading", { name: "Bambu Lab A1 mini" })
      .closest("section");
    expect(u1Group).not.toBeNull();
    expect(a1Group).not.toBeNull();
    expect(within(u1Group!).getByText("Snapmaker Orca")).toBeInTheDocument();
    expect(within(u1Group!).getAllByRole("article")).toHaveLength(2);
    expect(within(a1Group!).getByText("Bambu Studio")).toBeInTheDocument();
    expect(within(a1Group!).getAllByRole("article")).toHaveLength(1);

    expect(
      screen.getByRole("list", {
        name: "fixture__a1_mini.3mf filament loadout for Bambu Lab A1 mini",
      }),
    ).toHaveTextContent(
      "External spoolBlack PLAPLA · Panchroma Translucent PLA · #171717",
    );
    expect(screen.getByText("Load cyan in T1")).toBeInTheDocument();
    expect(
      screen.getByText(/Open each file in the slicer shown/i),
    ).toBeInTheDocument();

    expect(
      screen.getByRole("button", { name: "Review warnings to convert" }),
    ).toBeDisabled();
    fireEvent.click(
      screen.getByRole("checkbox", {
        name: /I have reviewed these warnings and want to continue/i,
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Convert projects" }));
    expect(props.onConvert).toHaveBeenCalledWith(true);
  });

  it("keeps progress inside the modal and exposes explicit cancellation", () => {
    const props = {
      ...defaultProps(),
      isConverting: true,
      progress: {
        conversionId: "conversion-id",
        stage: "writing",
        message: "Writing mixed project bundle…",
      },
    };
    render(<ConversionDialog {...props} />);

    expect(screen.getByRole("status")).toHaveTextContent(
      "Writing mixed project bundle…",
    );
    expect(
      screen.getByRole("button", { name: "Close conversion dialog" }),
    ).toBeDisabled();
    expect(screen.getByRole("button", { name: "Converting…" })).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: "Stop conversion" }));
    expect(props.onCancelConversion).toHaveBeenCalledOnce();

    const cancelEvent = new Event("cancel", { cancelable: true });
    fireEvent(screen.getByRole("dialog"), cancelEvent);
    expect(cancelEvent.defaultPrevented).toBe(true);
  });

  it("resets warning approval when preflight evidence changes", () => {
    const props = defaultProps();
    const { rerender } = render(<ConversionDialog {...props} />);
    const acknowledgement = screen.getByRole("checkbox", {
      name: /I have reviewed these warnings and want to continue/i,
    });
    fireEvent.click(acknowledgement);
    expect(acknowledgement).toBeChecked();

    rerender(
      <ConversionDialog
        {...props}
        prepared={{
          ...prepared,
          preparationToken: "replacement-preparation-token",
        }}
      />,
    );

    expect(
      screen.getByRole("checkbox", {
        name: /I have reviewed these warnings and want to continue/i,
      }),
    ).not.toBeChecked();
    expect(
      screen.getByRole("button", { name: "Review warnings to convert" }),
    ).toBeDisabled();
  });

  it("shows preflight exclusions without requiring a warning acknowledgement", () => {
    const props = defaultProps();
    render(
      <ConversionDialog
        {...props}
        prepared={{
          ...prepared,
          preparation: {
            ...prepared.preparation,
            warnings: [],
            excludedSourceUnits: [excludedUnit],
          },
        }}
      />,
    );

    expect(
      screen.getByRole("heading", { name: "1 excluded source unit" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        "These units will not be included in generated project files.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("checkbox", {
        name: /I have reviewed these warnings and want to continue/i,
      }),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Convert projects" }));
    expect(props.onConvert).toHaveBeenCalledWith(false);
  });

  it("offers a fresh preflight after a one-time preparation is consumed", () => {
    const props = {
      ...defaultProps(),
      prepared: null,
      error: "Conversion failed. The writer stopped after preparation.",
      needsNewPreflight: true,
    };
    render(<ConversionDialog {...props} />);

    expect(screen.getByRole("alert")).toHaveTextContent("Conversion failed");
    expect(
      screen.getByText(/consumed its one-time preparation/i),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Convert projects" }),
    ).not.toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", { name: "Run preflight again" }),
    );
    expect(props.onRetryPreflight).toHaveBeenCalledOnce();
  });

  it("groups published artifacts and focuses the completion heading", () => {
    const result = {
      adapterId: "u1-planner/mixed-native",
      outputDirectory: "/output/fixture__converted",
      manifestPath: "/output/fixture__converted/manifest.json",
      reportPath: "/output/fixture__converted/conversion-report.html",
      artifacts: [
        publishedArtifact({}),
        publishedArtifact({
          adapterId: "bambu/a1-mini",
          target: "a1_mini_mono",
          printer: "Bambu Lab A1 mini",
          strategy: "A1 Mono",
          slicer: "Bambu Studio",
          batchId: "a1-1",
          fileName: "fixture__a1_mini.3mf",
          relativePath: "projects/fixture__a1_mini.3mf",
          path: "/output/projects/fixture__a1_mini.3mf",
        }),
      ],
      warnings: ["Review the Bambu Studio plate order."],
      excludedSourceUnits: [excludedUnit],
      warningsAcknowledged: true,
    };
    render(
      <ConversionDialog {...defaultProps()} prepared={null} result={result} />,
    );

    const heading = screen.getByRole("heading", {
      name: "Conversion complete",
    });
    expect(heading).toHaveFocus();
    expect(
      screen.getByRole("heading", { name: "2 converted project files" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Snapmaker Orca")).toBeInTheDocument();
    expect(screen.getByText("Bambu Studio")).toBeInTheDocument();
    expect(
      screen.getByText("projects/fixture__a1_mini.3mf"),
    ).toBeInTheDocument();
    expect(screen.getAllByText(/Passed validation/)).toHaveLength(2);
    expect(
      screen.getByText("Review the Bambu Studio plate order."),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "1 excluded source unit" }),
    ).toBeInTheDocument();
    expect(screen.getByText("model-42")).toBeInTheDocument();
    expect(
      screen.getByText(
        "These units were not included in the published project files.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        "Warnings were explicitly acknowledged before conversion.",
      ),
    ).toBeInTheDocument();

    const bambuArtifact = result.artifacts[1];
    const actionProps = defaultProps();
    cleanup();
    render(
      <ConversionDialog {...actionProps} prepared={null} result={result} />,
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Open in Bambu Studio" }),
    );
    expect(actionProps.onOpenOutput).toHaveBeenCalledWith(bambuArtifact);
    fireEvent.click(
      screen.getAllByRole("button", { name: "Show in Finder" })[1],
    );
    expect(actionProps.onShowOutputInFinder).toHaveBeenCalledWith(
      bambuArtifact,
    );
  });

  it("restores focus when the dialog fallback closes", () => {
    const trigger = document.createElement("button");
    trigger.textContent = "Open conversion";
    document.body.append(trigger);
    trigger.focus();

    const props = defaultProps();
    const { rerender } = render(<ConversionDialog {...props} />);
    expect(
      screen.getByRole("button", { name: "Close conversion dialog" }),
    ).toHaveFocus();

    rerender(
      <ConversionDialog {...props} prepared={null} result={null} error="" />,
    );
    expect(trigger).toHaveFocus();
    trigger.remove();
  });
});
