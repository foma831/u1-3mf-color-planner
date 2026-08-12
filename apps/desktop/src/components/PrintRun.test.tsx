// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createDemoPlan } from "../data/mock-plan";
import type { BatchSegment, ProjectPlan } from "../types";
import {
  createEmptyPrintRunProgress,
  createPrintRunDescriptor,
  createPrintRunFingerprint,
  savePrintRunProgress,
  type PublishedPrintRunBundle,
  type StructuredT4Change,
} from "../utils/print-run";
import { PrintRun } from "./PrintRun";

type BatchFixture = BatchSegment & { t4Change?: StructuredT4Change | null };

class RemoveFailureStorage implements Storage {
  private values = new Map<string, string>();

  get length() {
    return this.values.size;
  }

  clear() {
    this.values.clear();
  }

  getItem(key: string) {
    return this.values.get(key) ?? null;
  }

  key(index: number) {
    return [...this.values.keys()][index] ?? null;
  }

  removeItem() {
    throw new Error("Storage removal failed");
  }

  setItem(key: string, value: string) {
    this.values.set(key, value);
  }
}

class RetriableSetFailureStorage implements Storage {
  private values = new Map<string, string>();
  allowWrites = false;

  get length() {
    return this.values.size;
  }

  clear() {
    this.values.clear();
  }

  getItem(key: string) {
    return this.values.get(key) ?? null;
  }

  key(index: number) {
    return [...this.values.keys()][index] ?? null;
  }

  removeItem(key: string) {
    this.values.delete(key);
  }

  setItem(key: string, value: string) {
    if (!this.allowWrites) throw new Error("Storage write failed");
    this.values.set(key, value);
  }
}

function executionPlan(): ProjectPlan {
  const plan = createDemoPlan();
  const [first, second, third, , , , a1] = plan.plates;
  plan.plates = [
    {
      ...first,
      order: 1,
      id: "target-grey-1",
      title: "Head components",
      source: "Source Plate 04",
    },
    {
      ...second,
      order: 2,
      id: "target-grey-2",
      title: "Torso components",
      source: "Source Plate 09",
    },
    {
      ...a1,
      order: 3,
      id: "target-a1",
      title: "A1 mono details",
      source: "Source Plate 12",
    },
    {
      ...third,
      order: 4,
      id: "target-black-1",
      title: "Black detail components",
      source: "Source Plate 05",
      loadoutLabel: "CMY + Black",
      t4Action: "Load Black",
    },
  ];
  plan.batches = [
    {
      id: "batch-grey",
      label: "CMY + Grey",
      detail: "CMY+X Full Spectrum",
      strategy: "cmyx",
      startOrder: 1,
      endOrder: 2,
      plateCount: 2,
      printer: "U1",
      setupActions: [],
    },
    {
      id: "batch-a1",
      label: "A1 Mono",
      detail: "Grey PLA",
      strategy: "a1-mono",
      startOrder: 3,
      endOrder: 3,
      plateCount: 1,
      printer: "A1 mini",
      setupActions: [],
    },
    {
      id: "batch-black",
      label: "CMY + Black",
      detail: "CMY+X Full Spectrum",
      strategy: "cmyx",
      startOrder: 4,
      endOrder: 4,
      plateCount: 1,
      printer: "U1",
      setupActions: [],
      t4Change: {
        fromSpoolId: "translucent-grey",
        fromSpoolName: "Translucent Grey",
        toSpoolId: "black",
        toSpoolName: "Black",
      },
    } as BatchFixture,
  ];
  plan.planReady = true;
  plan.blockingErrors = [];
  plan.omittedUnitCount = 0;
  return plan;
}

function partialExecutionPlan(): ProjectPlan {
  const plan = executionPlan();
  plan.planReady = false;
  plan.blockingErrors = [
    "Requirement 'plate-8-unit-2' has no schedulable physical loadout.",
  ];
  plan.omittedUnitCount = 1;
  plan.partialConversion = {
    available: true,
    reason: "One source unit can be isolated from the valid print jobs.",
    exclusions: [
      {
        scopeId: "plate-8",
        planningUnitId: "plate-8-unit-2",
        sourcePlateId: 8,
        sourceUnitId: "excluded-model-42",
        reason: "No schedulable physical loadout is available.",
        errorIdentity: "partial-error-v1:abc",
      },
    ],
  };
  return plan;
}

function directPlanWithRestore(): ProjectPlan {
  const plan = executionPlan();
  plan.plates = [
    {
      ...plan.plates[0],
      id: "target-direct",
      order: 1,
      strategy: "direct",
      title: "Direct color details",
      loadoutLabel: "Direct Spools",
    },
    {
      ...plan.plates[2],
      id: "target-later",
      order: 2,
    },
  ];
  plan.batches = [
    {
      id: "batch-direct",
      label: "Direct Spools",
      detail: "Full T1–T4 setup boundary",
      strategy: "direct",
      startOrder: 1,
      endOrder: 1,
      plateCount: 1,
      printer: "U1",
      setupActions: [
        "Before batch: T1 — unload Cyan",
        "Before batch: T1 — load Red",
        "Before batch: T2 — unload Magenta",
        "Before batch: T2 — load Green",
        "Before batch: T3 — unload Yellow",
        "Before batch: T3 — load Blue",
        "Before batch: T4 — unload Grey",
        "Before batch: T4 — load White",
        "After batch: T1 — restore Cyan (replace Red)",
        "After batch: T2 — restore Magenta (replace Green)",
        "After batch: T3 — restore Yellow (replace Blue)",
        "After batch: T4 — restore Grey (replace White)",
      ],
      t4Change: {
        fromSpoolId: "grey",
        fromSpoolName: "Grey",
        toSpoolId: "white",
        toSpoolName: "White",
      },
    } as BatchFixture,
    {
      id: "batch-later",
      label: "A1 Mono",
      detail: "Grey PLA",
      strategy: "a1-mono",
      startOrder: 2,
      endOrder: 2,
      plateCount: 1,
      printer: "A1 mini",
      setupActions: [],
    },
  ];
  return plan;
}

function renderPrintRun(
  plan = executionPlan(),
  props: Partial<React.ComponentProps<typeof PrintRun>> = {},
) {
  return render(
    <PrintRun
      plan={plan}
      isPlanValidated
      publishedBundle={publishedBundleFor(plan)}
      onOpenArtifact={async () => undefined}
      now={() => "2026-08-02T18:30:00.000Z"}
      {...props}
    />,
  );
}

function publishedBundleFor(plan: ProjectPlan): PublishedPrintRunBundle {
  return {
    sourceHash: plan.summary.sourceHash,
    backendPlanFingerprint: "backend-plan-fingerprint",
    executionPlanFingerprint: createPrintRunFingerprint(plan),
    result: {
      adapterId: "u1-planner/mixed-native",
      outputDirectory: "/output/fixture__converted",
      manifestPath: "/output/fixture__converted/manifest.json",
      reportPath: "/output/fixture__converted/conversion-report.html",
      artifacts: plan.plates.map((plate) => {
        const fileName = `target-${String(plate.order).padStart(2, "0")}.3mf`;
        return {
          adapterId:
            plate.printer === "A1 mini"
              ? "bambu/a1-mini"
              : "snapmaker/u1-direct",
          target: plate.printer === "A1 mini" ? "a1_mini_mono" : "u1_direct",
          printer: plate.printer,
          strategy: plate.strategy,
          slicer:
            plate.printer === "A1 mini" ? "Bambu Studio" : "Snapmaker Orca",
          batchId: `batch-${plate.order}`,
          fileName,
          relativePath: `projects/${fileName}`,
          path: `/output/fixture__converted/projects/${fileName}`,
          byteSize: 1024,
          sha256: `sha256-${plate.id}`,
          plateCount: 1,
          targetPlateIds: [plate.id],
          sourceUnitIds: plate.sourceUnitIds,
          loadout:
            plate.printer === "A1 mini"
              ? [
                  {
                    toolhead: "External spool",
                    spoolId: "creality-grey-petg",
                    spoolName: "Creality Grey PETG",
                    material: "PETG",
                    color: "#ADB1B2",
                    profile: "Creality PETG",
                    settingId: "GFSG99",
                    filamentId: "P123456",
                  },
                ]
              : [],
          setupActions: [],
          validationStatus: "Passed",
          adapterEvidence: { status: "passed" },
        };
      }),
      warnings: [],
      excludedSourceUnits: structuredClone(plan.partialConversion.exclusions),
      warningsAcknowledged: false,
    },
  };
}

function saveCompletedRun(plan: ProjectPlan) {
  const descriptor = createPrintRunDescriptor(plan, publishedBundleFor(plan));
  const progress = {
    ...createEmptyPrintRunProgress(descriptor),
    status: "complete" as const,
    currentStepIndex: descriptor.steps.length,
    completedPlateIds: descriptor.steps.flatMap((step) =>
      step.kind === "plate" ? [step.plate.id] : [],
    ),
    confirmedCheckpointIds: descriptor.steps.flatMap((step) =>
      step.kind === "plate" ? [] : [step.id],
    ),
  };
  expect(savePrintRunProgress(window.localStorage, descriptor, progress)).toBe(
    true,
  );
}

afterEach(() => {
  cleanup();
  window.localStorage.clear();
  vi.restoreAllMocks();
});

describe("PrintRun", () => {
  it("offers the recorded final loadout only after completion and saves it explicitly", () => {
    const plan = executionPlan();
    const onSaveFinalLoadout = vi.fn();
    saveCompletedRun(plan);

    renderPrintRun(plan, { onSaveFinalLoadout });

    expect(onSaveFinalLoadout).not.toHaveBeenCalled();
    const preview = screen.getByRole("region", {
      name: "Final printer loadout",
    });
    expect(
      within(preview).getByText("Snapmaker U1 · T1").closest("div"),
    ).toHaveTextContent("Panchroma Translucent CyanCyan · PLA");
    expect(
      within(preview).getByText("Snapmaker U1 · T4").closest("div"),
    ).toHaveTextContent("No loaded spool recorded");
    expect(
      within(preview).getByText("Bambu Lab A1 mini").closest("div"),
    ).toHaveTextContent("PolyLite PETG BlackBlack · PETG");

    fireEvent.click(
      within(preview).getByRole("button", {
        name: "Use as current printer loadout",
      }),
    );
    expect(onSaveFinalLoadout).toHaveBeenCalledOnce();
    expect(onSaveFinalLoadout).toHaveBeenCalledWith({
      currentLoadout: plan.plannedFinalLoadout,
      currentA1SpoolId: plan.plannedFinalA1SpoolId,
    });
  });

  it("keeps the saved final-loadout confirmation visible and prevents duplicate saves", () => {
    const plan = executionPlan();
    const onSaveFinalLoadout = vi.fn();
    saveCompletedRun(plan);

    renderPrintRun(plan, {
      onSaveFinalLoadout,
      finalLoadoutSaved: true,
    });

    expect(
      screen.getByText(
        "This loadout will be used to optimize the next project.",
      ),
    ).toBeVisible();
    const savedButton = screen.getByRole("button", {
      name: "Current printer loadout saved",
    });
    expect(savedButton).toHaveAttribute("aria-disabled", "true");
    fireEvent.click(savedButton);
    expect(onSaveFinalLoadout).not.toHaveBeenCalled();
  });

  it("blocks the next target plate at a T4 boundary until the exact load is confirmed", () => {
    const onCurrentTargetChange = vi.fn();
    renderPrintRun(executionPlan(), { onCurrentTargetChange });
    expect(
      screen.queryByRole("region", { name: "Final printer loadout" }),
    ).not.toBeInTheDocument();

    const progress = screen.getByRole("navigation", {
      name: "Print run progress",
    });
    expect(within(progress).getAllByRole("listitem")).toHaveLength(6);
    expect(within(progress).getByText("Target Plate 01")).toBeInTheDocument();
    expect(
      within(progress).getByText("U1 · Source: Source Plate 04"),
    ).toBeInTheDocument();
    expect(within(progress).getByText("Ready to start")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Start print run" }));
    expect(
      screen.getByRole("heading", { name: "Target Plate 01" }),
    ).toBeInTheDocument();
    const current = screen.getByRole("article", { name: "Target Plate 01" });
    expect(within(current).getByText("Source model plate")).toBeInTheDocument();
    expect(within(current).getByText("Source Plate 04")).toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", { name: "Mark Target Plate 01 complete" }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Mark Target Plate 02 complete" }),
    );
    expect(
      screen.getByRole("heading", {
        name: "Load the exact A1 mini spool before Target Plate 03",
      }),
    ).toBeInTheDocument();
    expect(screen.getByText("Creality Grey PETG")).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", {
        name: "Confirm Creality Grey PETG is loaded on A1 mini",
      }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Mark Target Plate 03 complete" }),
    );

    expect(
      screen.getByRole("heading", {
        name: "Filament change required before Target Plate 04",
      }),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/after Target Plate 02 has finished/i),
    ).toBeInTheDocument();
    expect(screen.getByText(/Unload/).closest("li")).toHaveTextContent(
      "Unload Translucent Grey from T4.",
    );
    expect(screen.getByText(/Load/).closest("li")).toHaveTextContent(
      "Load Black into T4",
    );
    expect(
      screen.queryByRole("button", { name: "Mark Target Plate 04 complete" }),
    ).not.toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", { name: "Confirm T4 Black is loaded" }),
    );
    expect(
      screen.getByRole("heading", { name: "Target Plate 04" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Mark Target Plate 04 complete" }),
    ).toBeEnabled();
    expect(onCurrentTargetChange).toHaveBeenLastCalledWith("target-black-1");

    fireEvent.click(
      screen.getByRole("button", { name: "Mark Target Plate 04 complete" }),
    );
    expect(
      screen.getByRole("heading", { name: "Print run complete" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("region", { name: "Final printer loadout" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText("4 of 4 target plates complete"),
    ).toBeInTheDocument();
  });

  it("moves focus to the new current action after every operator transition", () => {
    renderPrintRun();

    fireEvent.click(screen.getByRole("button", { name: "Start print run" }));
    expect(
      screen.getByRole("heading", { name: "Target Plate 01" }),
    ).toHaveFocus();

    fireEvent.click(
      screen.getByRole("button", { name: "Mark Target Plate 01 complete" }),
    );
    expect(
      screen.getByRole("heading", { name: "Target Plate 02" }),
    ).toHaveFocus();

    fireEvent.click(
      screen.getByRole("button", { name: "Mark Target Plate 02 complete" }),
    );
    expect(
      screen.getByRole("heading", {
        name: "Load the exact A1 mini spool before Target Plate 03",
      }),
    ).toHaveFocus();

    fireEvent.click(
      screen.getByRole("button", {
        name: "Confirm Creality Grey PETG is loaded on A1 mini",
      }),
    );
    expect(
      screen.getByRole("heading", { name: "Target Plate 03" }),
    ).toHaveFocus();

    fireEvent.click(
      screen.getByRole("button", { name: "Mark Target Plate 03 complete" }),
    );
    expect(
      screen.getByRole("heading", {
        name: "Filament change required before Target Plate 04",
      }),
    ).toHaveFocus();

    fireEvent.click(
      screen.getByRole("button", { name: "Confirm T4 Black is loaded" }),
    );
    expect(
      screen.getByRole("heading", { name: "Target Plate 04" }),
    ).toHaveFocus();

    fireEvent.click(
      screen.getByRole("button", { name: "Mark Target Plate 04 complete" }),
    );
    expect(
      screen.getByRole("heading", { name: "Print run complete" }),
    ).toHaveFocus();

    fireEvent.click(screen.getByRole("button", { name: "Reset print run" }));
    expect(
      screen.getByRole("heading", { name: "Ready to begin" }),
    ).toHaveFocus();
  });

  it("does not move focus on initial render, resume, or plan changes", () => {
    const sentinelRender = render(
      <button type="button">Outside Print Run</button>,
    );
    const sentinel = screen.getByRole("button", { name: "Outside Print Run" });
    sentinel.focus();

    const fresh = renderPrintRun(executionPlan(), { storage: null });
    expect(sentinel).toHaveFocus();
    fresh.unmount();

    const plan = executionPlan();
    const descriptor = createPrintRunDescriptor(plan, publishedBundleFor(plan));
    const savedProgress = {
      ...createEmptyPrintRunProgress(descriptor),
      status: "active" as const,
      currentStepIndex: 1,
      completedPlateIds: ["target-grey-1"],
    };
    expect(
      savePrintRunProgress(window.localStorage, descriptor, savedProgress),
    ).toBe(true);
    sentinel.focus();

    const resumed = renderPrintRun(plan);
    expect(
      screen.getByRole("heading", { name: "Target Plate 02" }),
    ).toBeInTheDocument();
    expect(sentinel).toHaveFocus();

    const changed = executionPlan();
    changed.plates[1].title = "Changed torso components";
    resumed.rerender(
      <PrintRun
        plan={changed}
        isPlanValidated
        publishedBundle={publishedBundleFor(changed)}
        now={() => "2026-08-02T18:30:00.000Z"}
      />,
    );
    expect(sentinel).toHaveFocus();
    sentinelRender.unmount();
  });

  it("persists the blocking checkpoint and resumes only for the exact plan", () => {
    const plan = executionPlan();
    const firstRender = renderPrintRun(plan);

    fireEvent.click(screen.getByRole("button", { name: "Start print run" }));
    for (const order of ["01", "02"]) {
      fireEvent.click(
        screen.getByRole("button", {
          name: `Mark Target Plate ${order} complete`,
        }),
      );
    }
    fireEvent.click(
      screen.getByRole("button", {
        name: "Confirm Creality Grey PETG is loaded on A1 mini",
      }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Mark Target Plate 03 complete" }),
    );
    expect(window.localStorage.length).toBe(1);
    firstRender.unmount();

    const resumed = renderPrintRun(executionPlan());
    expect(
      screen.getByRole("heading", {
        name: "Filament change required before Target Plate 04",
      }),
    ).toBeInTheDocument();
    expect(
      screen.getByText("3 of 4 target plates complete"),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/Operator log · 5 recorded actions/),
    ).toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", { name: "Confirm T4 Black is loaded" }),
    );
    resumed.unmount();
    renderPrintRun(executionPlan());
    expect(
      screen.getByRole("heading", { name: "Target Plate 04" }),
    ).toBeInTheDocument();
  });

  it("requires full Direct setup before its plate and all restore actions immediately after it", () => {
    renderPrintRun(directPlanWithRestore());

    const progress = screen.getByRole("navigation", {
      name: "Print run progress",
    });
    expect(within(progress).getAllByRole("listitem")).toHaveLength(5);
    expect(
      within(progress).getByText("Setup before Target Plate 01"),
    ).toBeInTheDocument();
    expect(
      within(progress).getByText("Restore after Target Plate 01"),
    ).toBeInTheDocument();
    expect(
      within(progress).queryByText(/T4 change before/i),
    ).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Start print run" }));
    expect(
      screen.getByRole("heading", {
        name: "Printer setup required before Target Plate 01",
      }),
    ).toHaveFocus();
    expect(screen.getByText("Before batch: T1 — load Red")).toBeInTheDocument();
    expect(
      screen.getByText("Before batch: T4 — load White"),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Mark Target Plate 01 complete" }),
    ).not.toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", {
        name: "Confirm printer setup is complete",
      }),
    );
    expect(
      screen.getByRole("heading", { name: "Target Plate 01" }),
    ).toHaveFocus();
    fireEvent.click(
      screen.getByRole("button", { name: "Mark Target Plate 01 complete" }),
    );

    expect(
      screen.getByRole("heading", {
        name: "Restore required after Target Plate 01",
      }),
    ).toHaveFocus();
    expect(
      screen.getByText("After batch: T1 — restore Cyan (replace Red)"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("After batch: T4 — restore Grey (replace White)"),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Mark Target Plate 02 complete" }),
    ).not.toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", { name: "Confirm restore is complete" }),
    );
    expect(
      screen.getByRole("heading", {
        name: "Load the exact A1 mini spool before Target Plate 02",
      }),
    ).toHaveFocus();
    fireEvent.click(
      screen.getByRole("button", {
        name: "Confirm Creality Grey PETG is loaded on A1 mini",
      }),
    );
    expect(
      screen.getByRole("heading", { name: "Target Plate 02" }),
    ).toHaveFocus();
  });

  it("requires a confirmed first-batch load when the current T4 spool is unknown", () => {
    const plan = executionPlan();
    plan.batches[0].t4Change = {
      fromSpoolId: null,
      fromSpoolName: null,
      toSpoolId: "translucent-grey",
      toSpoolName: "Translucent Grey",
    };
    renderPrintRun(plan);

    fireEvent.click(screen.getByRole("button", { name: "Start print run" }));

    expect(
      screen.getByRole("heading", {
        name: "Filament change required before Target Plate 01",
      }),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Check T4. If a spool is loaded, unload it."),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Mark Target Plate 01 complete" }),
    ).not.toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", {
        name: "Confirm T4 Translucent Grey is loaded",
      }),
    );
    expect(
      screen.getByRole("heading", { name: "Target Plate 01" }),
    ).toBeInTheDocument();
  });

  it("invalidates stale progress when execution-critical plan data changes", () => {
    const plan = executionPlan();
    const firstRender = renderPrintRun(plan);
    fireEvent.click(screen.getByRole("button", { name: "Start print run" }));
    fireEvent.click(
      screen.getByRole("button", { name: "Mark Target Plate 01 complete" }),
    );
    expect(window.localStorage.length).toBe(1);
    firstRender.unmount();

    const changed = executionPlan();
    changed.plates[0].loadoutLabel = "CMY + White";
    renderPrintRun(changed);

    expect(
      screen.getByRole("heading", { name: "Ready to begin" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText("0 of 4 target plates complete"),
    ).toBeInTheDocument();
    expect(window.localStorage.length).toBe(0);
  });

  it("resets saved operator progress without changing the validated plan", () => {
    renderPrintRun();
    fireEvent.click(screen.getByRole("button", { name: "Start print run" }));
    fireEvent.click(
      screen.getByRole("button", { name: "Mark Target Plate 01 complete" }),
    );

    fireEvent.click(screen.getByRole("button", { name: "Reset print run" }));

    expect(
      screen.getByRole("heading", { name: "Ready to begin" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText("0 of 4 target plates complete"),
    ).toBeInTheDocument();
    expect(window.localStorage.length).toBe(0);
  });

  it("warns when reset cannot clear saved progress", () => {
    const storage = new RemoveFailureStorage();
    renderPrintRun(executionPlan(), { storage });
    fireEvent.click(screen.getByRole("button", { name: "Start print run" }));
    fireEvent.click(
      screen.getByRole("button", { name: "Mark Target Plate 01 complete" }),
    );

    fireEvent.click(screen.getByRole("button", { name: "Reset print run" }));

    expect(
      screen.getByRole("heading", { name: "Target Plate 02" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/saved progress could not be cleared/i, {
        selector: ".print-run-storage-warning",
      }),
    ).toBeInTheDocument();
    expect(
      screen.getByText("1 of 4 target plates complete"),
    ).toBeInTheDocument();
    expect(
      screen.queryByText(/Saved progress was cleared/i),
    ).not.toBeInTheDocument();
  });

  it("does not advance when local progress persistence fails and lets the operator retry", () => {
    const storage = new RetriableSetFailureStorage();
    const onProgressChange = vi.fn();
    renderPrintRun(executionPlan(), { storage, onProgressChange });

    fireEvent.click(screen.getByRole("button", { name: "Start print run" }));

    expect(
      screen.getByRole("heading", { name: "Ready to begin" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent(
      /action was not recorded and the workflow did not advance/i,
    );
    expect(onProgressChange).not.toHaveBeenCalled();

    storage.allowWrites = true;
    fireEvent.click(screen.getByRole("button", { name: "Start print run" }));

    expect(
      screen.getByRole("heading", { name: "Target Plate 01" }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(onProgressChange).toHaveBeenCalledTimes(1);
  });

  it("opens only validated published artifacts through the host callback and reports failures", async () => {
    const onOpenArtifact = vi
      .fn<
        (
          artifact: PublishedPrintRunBundle["result"]["artifacts"][number],
        ) => Promise<void>
      >()
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(new Error("Backend registry rejected the file"));
    renderPrintRun(executionPlan(), { onOpenArtifact });

    const publishedOpenButtons = screen.getAllByRole("button", {
      name: "Open in Snapmaker Orca",
    });
    fireEvent.click(publishedOpenButtons[0]);
    await waitFor(() => expect(onOpenArtifact).toHaveBeenCalledTimes(1));
    expect(onOpenArtifact.mock.calls[0][0].path).toContain("target-01.3mf");

    fireEvent.click(screen.getByRole("button", { name: "Start print run" }));
    const currentTarget = screen.getByRole("article", {
      name: "Target Plate 01",
    });
    fireEvent.click(
      within(currentTarget).getByRole("button", {
        name: "Open in Snapmaker Orca",
      }),
    );
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent(
        /Backend registry rejected the file/i,
      ),
    );
    expect(onOpenArtifact).toHaveBeenCalledTimes(2);
  });

  it("locks an A1 run when the published artifact does not identify one exact physical spool", () => {
    const plan = executionPlan();
    const bundle = publishedBundleFor(plan);
    const a1Artifact = bundle.result.artifacts.find(
      (artifact) => artifact.target === "a1_mini_mono",
    )!;
    a1Artifact.loadout = [];

    renderPrintRun(plan, { publishedBundle: bundle });

    expect(
      screen.getByRole("heading", { name: "Print Run is locked" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Start print run" }),
    ).not.toBeInTheDocument();
  });

  it("keeps Print Run locked while choices are pending validation", () => {
    renderPrintRun(executionPlan(), { isPlanValidated: false });

    expect(
      screen.getByRole("heading", { name: "Print Run is locked" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/Recalculate and validate pending plan changes/i),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Start print run" }),
    ).not.toBeInTheDocument();
  });

  it("keeps a validated native plan locked until its files are published", () => {
    renderPrintRun(executionPlan(), { publishedBundle: null });

    expect(
      screen.getByRole("heading", { name: "Print Run is locked" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/Convert this native plan successfully/i),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Start print run" }),
    ).not.toBeInTheDocument();
  });

  it("runs only published valid jobs when canonical partial exclusions match exactly", () => {
    const plan = partialExecutionPlan();
    renderPrintRun(plan);

    expect(
      screen.getByRole("heading", {
        name: "Partial print run · 1 source unit excluded",
      }),
    ).toBeInTheDocument();
    expect(screen.getByText("excluded-model-42")).toBeInTheDocument();
    expect(
      screen.getByText(
        /They have no target-plate step and cannot be marked complete/i,
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Start print run" }),
    ).toBeEnabled();
    expect(
      screen.queryByRole("button", { name: /excluded-model-42/i }),
    ).not.toBeInTheDocument();
    expect(
      within(
        screen.getByRole("navigation", { name: "Print run progress" }),
      ).queryByText("excluded-model-42"),
    ).not.toBeInTheDocument();
  });

  it("locks a partial run when exclusion evidence is mismatched or stale", () => {
    const plan = partialExecutionPlan();
    const mismatchedBundle = publishedBundleFor(plan);
    mismatchedBundle.result.excludedSourceUnits[0] = {
      ...mismatchedBundle.result.excludedSourceUnits[0],
      reason: "A reconstructed frontend reason must not be accepted.",
    };
    const first = renderPrintRun(plan, {
      publishedBundle: mismatchedBundle,
    });

    expect(
      screen.getByRole("heading", { name: "Print Run is locked" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Start print run" }),
    ).not.toBeInTheDocument();
    first.unmount();

    const staleBundle = publishedBundleFor(plan);
    const changedPlan = partialExecutionPlan();
    changedPlan.partialConversion.exclusions[0] = {
      ...changedPlan.partialConversion.exclusions[0],
      errorIdentity: "partial-error-v1:replacement",
    };
    renderPrintRun(changedPlan, { publishedBundle: staleBundle });

    expect(
      screen.getByRole("heading", { name: "Print Run is locked" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/exclusion evidence do not match this exact plan/i),
    ).toBeInTheDocument();
  });

  it("rejects a published partial artifact that silently omits a scheduled source unit", () => {
    const plan = partialExecutionPlan();
    const bundle = publishedBundleFor(plan);
    expect(bundle.result.artifacts[0].sourceUnitIds.length).toBeGreaterThan(0);
    bundle.result.artifacts[0].sourceUnitIds = [];
    renderPrintRun(plan, { publishedBundle: bundle });

    expect(
      screen.getByRole("heading", { name: "Print Run is locked" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/source-unit coverage.*do not match this exact plan/i),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Start print run" }),
    ).not.toBeInTheDocument();
  });

  it("states that actions happen between whole plate jobs", () => {
    renderPrintRun();
    expect(
      screen.getByText(/There are no mid-print filament-change instructions/i),
    ).toBeInTheDocument();
  });
});
