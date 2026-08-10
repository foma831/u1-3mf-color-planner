import { describe, expect, it } from "vitest";

import { createDemoPlan } from "../data/mock-plan";
import type { BatchSegment, ProjectPlan } from "../types";
import {
  PRINT_RUN_STORAGE_PREFIX,
  PUBLISHED_PRINT_RUN_STORAGE_PREFIX,
  createPrintRunDescriptor,
  createPrintRunFingerprint,
  createPrintRunSteps,
  createPublishedPrintRunStorageKey,
  loadUntrustedPublishedPrintRunBundle,
  loadPrintRunProgress,
  resolveBatchT4Change,
  savePublishedPrintRunBundle,
  savePrintRunProgress,
  type PublishedPrintRunBundle,
  type StructuredT4Change,
} from "./print-run";

type BatchFixture = BatchSegment & { t4Change?: StructuredT4Change | null };

class MemoryStorage implements Storage {
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

  removeItem(key: string) {
    this.values.delete(key);
  }

  setItem(key: string, value: string) {
    this.values.set(key, value);
  }
}

function executionPlan(): ProjectPlan {
  const plan = createDemoPlan();
  const [first, second, third, , , , a1] = plan.plates;
  plan.plates = [
    { ...first, order: 1, id: "target-grey-1", source: "Source Plate 04" },
    { ...second, order: 2, id: "target-grey-2", source: "Source Plate 09" },
    { ...a1, order: 3, id: "target-a1", source: "Source Plate 12" },
    {
      ...third,
      order: 4,
      id: "target-black-1",
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
      setupActions: [
        "Before batch: T4 — unload Wrong fallback",
        "Before batch: T4 — load Wrong fallback",
      ],
      t4Change: {
        fromSpoolId: "translucent-grey",
        fromSpoolName: "Translucent Grey",
        toSpoolId: "black",
        toSpoolName: "Black",
      },
    } as BatchFixture,
  ];
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
        "Before batch: T4 — keep White",
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
      artifacts: plan.plates.map((plate) => ({
        adapterId:
          plate.printer === "A1 mini"
            ? "bambu/a1-mini"
            : "snapmaker/u1-direct",
        target: plate.printer === "A1 mini" ? "a1_mini_mono" : "u1_direct",
        printer: plate.printer,
        strategy: plate.strategy,
        slicer: plate.printer === "A1 mini" ? "Bambu Studio" : "Snapmaker Orca",
        batchId: `batch-${plate.order}`,
        fileName: `target-${plate.order}.3mf`,
        relativePath: `projects/target-${plate.order}.3mf`,
        path: `/output/fixture__converted/projects/target-${plate.order}.3mf`,
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
      })),
      warnings: [],
      excludedSourceUnits: [],
      warningsAcknowledged: false,
    },
  };
}

describe("print run planning", () => {
  it("uses structured native T4 data before the setup-action fallback", () => {
    const batch = executionPlan().batches[2];

    expect(resolveBatchT4Change(batch)).toEqual({
      fromLabel: "Translucent Grey",
      toLabel: "Black",
      fromSpoolId: "translucent-grey",
      toSpoolId: "black",
      source: "structured",
    });
  });

  it("parses the legacy setup action pair without matching load inside unload", () => {
    const batch: BatchSegment = {
      id: "legacy",
      label: "CMY + Black",
      detail: "CMY+X Full Spectrum",
      strategy: "cmyx",
      startOrder: 3,
      endOrder: 4,
      plateCount: 2,
      printer: "U1",
      setupActions: [
        "Before batch: T4 — unload Panchroma Translucent Grey",
        "Before batch: T4 — load PolyLite Black",
      ],
    };

    expect(resolveBatchT4Change(batch)).toEqual({
      fromLabel: "Panchroma Translucent Grey",
      toLabel: "PolyLite Black",
      fromSpoolId: null,
      toSpoolId: null,
      source: "setup-actions",
    });
  });

  it("creates an initial checkpoint when the current T4 state is unknown", () => {
    const batch: BatchFixture = {
      id: "initial-black",
      label: "CMY + Black",
      detail: "CMY+X Full Spectrum",
      strategy: "cmyx",
      startOrder: 1,
      endOrder: 1,
      plateCount: 1,
      printer: "U1",
      setupActions: ["Before batch: T4 — load Black"],
      t4Change: {
        fromSpoolId: null,
        fromSpoolName: null,
        toSpoolId: "black",
        toSpoolName: "Black",
      },
    };

    expect(resolveBatchT4Change(batch)).toEqual({
      fromLabel: "Unknown / current T4 spool",
      toLabel: "Black",
      fromSpoolId: null,
      toSpoolId: "black",
      source: "structured",
    });
  });

  it("places a blocking checkpoint before the new U1 batch and refers to the previous U1 plate", () => {
    const steps = createPrintRunSteps(executionPlan());
    const checkpoint = steps.find((step) => step.kind === "filament-change");

    expect(steps.map((step) => step.kind)).toEqual([
      "plate",
      "plate",
      "plate",
      "filament-change",
      "plate",
    ]);
    expect(checkpoint).toMatchObject({
      beforeTargetOrder: 4,
      afterTargetOrder: 2,
      beforePlateId: "target-black-1",
      fromLabel: "Translucent Grey",
      toLabel: "Black",
    });
  });

  it("blocks every Direct T1–T4 change before the batch and every restore action after it", () => {
    const steps = createPrintRunSteps(directPlanWithRestore());

    expect(steps.map((step) => step.kind)).toEqual([
      "setup-checkpoint",
      "plate",
      "setup-checkpoint",
      "plate",
    ]);
    expect(steps).not.toContainEqual(
      expect.objectContaining({ kind: "filament-change" }),
    );
    expect(steps[0]).toMatchObject({
      phase: "before-batch",
      beforeTargetOrder: 1,
      actions: [
        "Before batch: T1 — unload Cyan",
        "Before batch: T1 — load Red",
        "Before batch: T2 — unload Magenta",
        "Before batch: T2 — load Green",
        "Before batch: T3 — unload Yellow",
        "Before batch: T3 — load Blue",
        "Before batch: T4 — unload Grey",
        "Before batch: T4 — load White",
      ],
    });
    expect(steps[2]).toMatchObject({
      phase: "after-batch",
      afterTargetOrder: 1,
      beforeTargetOrder: 2,
      actions: [
        "After batch: T1 — restore Cyan (replace Red)",
        "After batch: T2 — restore Magenta (replace Green)",
        "After batch: T3 — restore Yellow (replace Blue)",
        "After batch: T4 — restore Grey (replace White)",
      ],
    });
  });

  it("creates a deterministic fingerprint that changes with execution-critical plan data", () => {
    const plan = executionPlan();
    const first = createPrintRunFingerprint(plan);
    expect(createPrintRunFingerprint(executionPlan())).toBe(first);

    const changed = executionPlan();
    changed.plates[3].loadoutLabel = "CMY + White";
    expect(createPrintRunFingerprint(changed)).not.toBe(first);

    const changedExclusionEvidence = executionPlan();
    changedExclusionEvidence.planReady = false;
    changedExclusionEvidence.blockingErrors = ["One unit is excluded."];
    changedExclusionEvidence.omittedUnitCount = 1;
    changedExclusionEvidence.partialConversion = {
      available: true,
      reason: "One valid exclusion.",
      exclusions: [
        {
          scopeId: "plate-9",
          planningUnitId: "plate-9-unit-1",
          sourcePlateId: 9,
          sourceUnitId: "excluded-source-unit",
          reason: "No schedulable loadout.",
          errorIdentity: "partial-error-v1:fixture",
        },
      ],
    };
    expect(createPrintRunFingerprint(changedExclusionEvidence)).not.toBe(first);
  });

  it("stores progress by source hash and fingerprint and removes stale progress for that source", () => {
    const storage = new MemoryStorage();
    const firstPlan = executionPlan();
    const firstDescriptor = createPrintRunDescriptor(firstPlan);
    const firstProgress = {
      ...loadPrintRunProgress(storage, firstDescriptor),
      status: "active" as const,
    };
    expect(savePrintRunProgress(storage, firstDescriptor, firstProgress)).toBe(true);
    expect(firstDescriptor.storageKey).toContain(PRINT_RUN_STORAGE_PREFIX);

    const changedPlan = executionPlan();
    changedPlan.plates[0].material = "PETG → PETG";
    const changedDescriptor = createPrintRunDescriptor(changedPlan);
    const changedProgress = loadPrintRunProgress(storage, changedDescriptor);

    expect(changedDescriptor.storageKey).not.toBe(firstDescriptor.storageKey);
    expect(changedProgress.status).toBe("not-started");
    expect(storage.getItem(firstDescriptor.storageKey)).toBeNull();
  });

  it("persists confirmed setup and restore checkpoints only for the exact v2 descriptor", () => {
    const storage = new MemoryStorage();
    const descriptor = createPrintRunDescriptor(directPlanWithRestore());
    const beforeCheckpoint = descriptor.steps[0];
    const restoreCheckpoint = descriptor.steps[2];
    const progress = {
      ...loadPrintRunProgress(storage, descriptor),
      status: "active" as const,
      currentStepIndex: 3,
      completedPlateIds: ["target-direct"],
      confirmedCheckpointIds: [beforeCheckpoint.id, restoreCheckpoint.id],
    };

    expect(savePrintRunProgress(storage, descriptor, progress)).toBe(true);
    expect(loadPrintRunProgress(storage, descriptor)).toEqual(progress);
    expect(descriptor.planFingerprint).toMatch(/^v2-/);
    expect(descriptor.storageKey).toContain(":v2:");
  });

  it("derives an A1 spool checkpoint from the exact validated artifact loadout", () => {
    const plan = executionPlan();
    const steps = createPrintRunSteps(plan, publishedBundleFor(plan));
    const checkpoint = steps.find(
      (step) => step.kind === "a1-spool-checkpoint",
    );

    expect(checkpoint).toMatchObject({
      beforePlateId: "target-a1",
      beforeTargetOrder: 3,
      spool: {
        spoolId: "creality-grey-petg",
        spoolName: "Creality Grey PETG",
        material: "PETG",
        profile: "Creality PETG",
      },
    });
  });

  it("stores an exact publication receipt by source and plan but loads it only as untrusted recovery data", () => {
    const storage = new MemoryStorage();
    const plan = executionPlan();
    const bundle = publishedBundleFor(plan);

    expect(savePublishedPrintRunBundle(storage, plan, bundle)).toBe(true);
    expect(createPublishedPrintRunStorageKey(plan)).toContain(
      PUBLISHED_PRINT_RUN_STORAGE_PREFIX,
    );
    expect(loadUntrustedPublishedPrintRunBundle(storage, plan)).toEqual(bundle);

    const changed = executionPlan();
    changed.plates[0].material = "PETG";
    expect(loadUntrustedPublishedPrintRunBundle(storage, changed)).toBeNull();
  });
});
