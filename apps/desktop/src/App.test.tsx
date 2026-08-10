// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "./App";
import { DirectSpoolEditor } from "./components/DirectSpoolEditor";
import { PlateTable } from "./components/PlateTable";
import { createDemoPlan } from "./data/mock-plan";
import { FILAMENT_LIBRARY_STORAGE_KEY } from "./services/filament-library";
import type {
  ColorResolution,
  CmyxCalibrationLibraryDocument,
  ConversionCapability,
  ConversionResult,
  FilamentLibraryDocument,
  PhysicalSpool,
  PreparedConversion,
  ProjectPlan,
  ToolheadId,
} from "./types";
import {
  createPrintRunFingerprint,
  savePublishedPrintRunBundle,
  type PublishedPrintRunBundle,
} from "./utils/print-run";
import { deriveBatches } from "./utils/plan";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

const BROWSER_OUT_OF_STOCK_ERROR_FOR_TEST =
  "Browser demo data cannot be revalidated with out-of-stock spools. Use the native app for an authoritative plan.";

const removableUserSpool: PhysicalSpool = {
  id: "user-workshop-orange",
  calibrationIdentity:
    "spool-calibration:22222222-2222-4222-8222-222222222222",
  source: "user",
  name: "Workshop Orange",
  colorName: "Orange",
  hex: "#D97813",
  material: "PLA",
  profile: "Generic PLA",
  colorBasis: "Nominal",
  available: true,
};

const exactA1PreparedLoadout = [
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
];

function publishedPrintRunBundleFixture(
  plan: ProjectPlan,
  backendPlanFingerprint = "restart-plan-fingerprint",
): PublishedPrintRunBundle {
  const outputDirectory = "/tmp/output/restart-bundle";
  return {
    sourceHash: plan.summary.sourceHash,
    backendPlanFingerprint,
    executionPlanFingerprint: createPrintRunFingerprint(plan),
    result: {
      adapterId: "u1-planner/mixed-native",
      outputDirectory,
      manifestPath: `${outputDirectory}/manifest.json`,
      reportPath: `${outputDirectory}/conversion-report.html`,
      artifacts: plan.plates.map((plate) => {
        const fileName = `target-${String(plate.order).padStart(2, "0")}.3mf`;
        return {
          adapterId:
            plate.printer === "A1 mini"
              ? "bambu/a1-mini"
              : "snapmaker/u1-direct",
          target:
            plate.printer === "A1 mini" ? "a1_mini_mono" : "u1_direct",
          printer:
            plate.printer === "A1 mini"
              ? "Bambu Lab A1 mini"
              : "Snapmaker U1",
          strategy: plate.strategy,
          slicer:
            plate.printer === "A1 mini" ? "Bambu Studio" : "Snapmaker Orca",
          batchId: `batch-${plate.order}`,
          fileName,
          relativePath: `projects/${fileName}`,
          path: `${outputDirectory}/projects/${fileName}`,
          byteSize: 1024,
          sha256: `sha256-${plate.id}`,
          plateCount: 1,
          targetPlateIds: [plate.id],
          sourceUnitIds: plate.sourceUnitIds,
          loadout:
            plate.printer === "A1 mini" ? exactA1PreparedLoadout : [],
          setupActions: [],
          validationStatus: "Passed",
          adapterEvidence: { status: "passed" },
        };
      }),
      warnings: [],
      excludedSourceUnits: structuredClone(
        plan.partialConversion.exclusions,
      ),
      warningsAcknowledged: false,
    },
  };
}

async function analyzeBrowserDemo(fileName = "Withered_Foxy.3mf") {
  const input = document.querySelector<HTMLInputElement>("#project-file-input");
  expect(input).not.toBeNull();
  fireEvent.change(input!, {
    target: { files: [new File(["demo"], fileName, { type: "model/3mf" })] },
  });
  fireEvent.click(screen.getByRole("button", { name: "Analyze Project" }));
  await waitFor(
    () => expect(screen.getByText("Demo analysis complete")).toBeInTheDocument(),
    { timeout: 2_000 },
  );
}

async function renderAnalyzedApp() {
  render(<App />);
  await analyzeBrowserDemo();
}

function createA1RoutingOffNativePlan() {
  const plan = createDemoPlan();
  plan.plates = plan.plates.map((plate) =>
    plate.printer === "A1 mini"
      ? {
          ...plate,
          printer: "U1" as const,
          strategy: "cmyx" as const,
          directEligible: plate.logicalColorCount <= 4,
          directEligibilityReason: undefined,
        }
      : plate,
  );
  plan.batches = deriveBatches(plan.plates);
  plan.a1SpoolChangeCount = 0;
  return plan;
}

function createProjectWideDirectNativePlan() {
  const plan = createDemoPlan();
  const sourceMappings = plan.plates[0].mappings!;
  const mapping = (
    templateIndex: number,
    id: string,
    sourceMaterial: "PLA" | "PETG",
    sourceHex: string,
  ) => ({
    ...structuredClone(sourceMappings[templateIndex]),
    id,
    sourceMaterial,
    sourceHex,
    selectedSpoolId: "",
  });
  plan.plates = [
    {
      ...plan.plates[0],
      id: "target-plate-1",
      scopeId: "plate-1",
      title: "First scope — Plate 01",
      strategy: "cmyx",
      mappings: [
        mapping(0, "plate-1-red", "PETG", "#C72E2A"),
        mapping(1, "plate-1-grey", "PETG", "#ADB1B2"),
      ],
    },
    {
      ...plan.plates[1],
      id: "target-plate-2",
      scopeId: "plate-2",
      title: "Second scope — Plate 02",
      strategy: "cmyx",
      directEligible: true,
      directEligibilityReason: undefined,
      mappings: [
        mapping(0, "plate-2-red", "PETG", "#C72E2A"),
        mapping(1, "plate-2-grey", "PETG", "#ADB1B2"),
      ],
    },
  ];
  plan.scopeSelections = ["plate-1", "plate-2"].map((scopeId) => ({
    scopeId,
    strategy: "cmyx" as const,
    assignments: [],
    approvedColorFallbacks: [
      { requirementId: `${scopeId}-grey`, candidateId: "stale-cmyx" },
    ],
    materialSubstitutions: [
      {
        requirementId: `${scopeId}-grey`,
        candidateId: "stale-cmyx",
        sourceMaterial: "PETG",
        targetMaterial: "PLA" as const,
        acknowledged: true as const,
      },
    ],
  }));
  plan.unitPrinterSelections = plan.unitPrinterSelections.slice(0, 2);
  plan.projectDirectPalette = {
    available: true,
    effectivePairCount: 2,
    directPairCount: 2,
    maximumPairCount: 4,
    unavailableReason: null,
    mappings: [
      {
        id: "project-pair-01",
        physicalIdentityId: "project-physical-01",
        sourceMaterial: "PETG",
        sourceRole: "Cosmetic",
        sourceHex: "#C72E2A",
        sourceSlots: ["F8"],
        sourceProfileIds: ["profile-red"],
        usedBy: ["First scope", "Second scope"],
        references: ["plate-1", "plate-2"].map((scopeId) => ({
          scopeId,
          scopeName: scopeId === "plate-1" ? "First scope" : "Second scope",
          requirementIds: [`${scopeId}-red`],
        })),
        currentCmyxResults: [
          {
            scopeId: "plate-1",
            scopeName: "First scope",
            recipe: "Ratio T1·T2",
            predictedHex: "#C85A55",
            deltaE00: 4.2,
            confidence: "High",
          },
          {
            scopeId: "plate-2",
            scopeName: "Second scope",
            recipe: "Solid T4",
            predictedHex: "#B43E3A",
            deltaE00: 6.7,
            confidence: "Nominal",
          },
        ],
        selectedSpoolId: "",
        directToolhead: "T1",
        materialSubstitutionAcknowledged: false,
      },
      {
        id: "project-pair-02",
        physicalIdentityId: "project-physical-02",
        sourceMaterial: "PETG",
        sourceRole: "Functional",
        sourceHex: "#ADB1B2",
        sourceSlots: ["F6"],
        sourceProfileIds: ["profile-grey-petg"],
        usedBy: ["First scope", "Second scope"],
        references: ["plate-1", "plate-2"].map((scopeId) => ({
          scopeId,
          scopeName: scopeId === "plate-1" ? "First scope" : "Second scope",
          requirementIds: [`${scopeId}-grey`],
        })),
        currentCmyxResults: [
          {
            scopeId: "plate-1",
            scopeName: "First scope",
            recipe: "Solid T4 Grey",
            predictedHex: "#9CA9AB",
            deltaE00: 5.4,
            confidence: "Nominal",
          },
          {
            scopeId: "plate-2",
            scopeName: "Second scope",
            recipe: "Solid T4 Grey",
            predictedHex: "#9CA9AB",
            deltaE00: 5.4,
            confidence: "Nominal",
          },
        ],
        selectedSpoolId: "",
        directToolhead: "T2",
        materialSubstitutionAcknowledged: false,
      },
    ],
  };
  plan.batches = deriveBatches(plan.plates);
  plan.a1SpoolChangeCount = 0;
  plan.t4SwapCount = 0;
  return plan;
}

function createPerPlateRoleSharedNativePlan() {
  const plan = createProjectWideDirectNativePlan();
  const plate = plan.plates[0];
  const [redTemplate, greyTemplate] = plate.mappings!;
  const mapping = (
    template: typeof redTemplate,
    id: string,
    physicalIdentityId: string,
    sourceName: string,
    sourceMaterial: "PLA" | "PETG",
    sourceHex: string,
    directToolhead: ToolheadId,
    selectedSpoolId: string,
  ) => ({
    ...structuredClone(template),
    id,
    physicalIdentityId,
    sourceName,
    sourceMaterial,
    sourceHex,
    directToolhead,
    selectedSpoolId,
    materialSubstitutionAcknowledged: false,
  });
  plate.logicalColorCount = 4;
  plate.effectivePairCount = 5;
  plate.directPairCount = 4;
  plate.directEligible = true;
  plate.directEligibilityReason = undefined;
  plate.strategy = "cmyx";
  plate.mappings = [
    mapping(
      redTemplate,
      "plate-1-red-cosmetic",
      "plate-physical-01",
      "Red cosmetic",
      "PETG",
      "#C72E2A",
      "T1",
      "black-petg",
    ),
    mapping(
      redTemplate,
      "plate-1-red-support",
      "plate-physical-01",
      "Red support",
      "PETG",
      "#C72E2A",
      "T1",
      "black-petg",
    ),
    mapping(
      greyTemplate,
      "plate-1-cyan",
      "plate-physical-02",
      "Cyan detail",
      "PLA",
      "#08ABFB",
      "T2",
      "panchroma-cyan",
    ),
    mapping(
      greyTemplate,
      "plate-1-purple",
      "plate-physical-03",
      "Purple detail",
      "PLA",
      "#8951B8",
      "T3",
      "royal-purple",
    ),
    mapping(
      greyTemplate,
      "plate-1-sand",
      "plate-physical-04",
      "Sand detail",
      "PLA",
      "#D6BA87",
      "T4",
      "sandstone",
    ),
  ];
  plan.scopeSelections[0] = {
    scopeId: "plate-1",
    strategy: "cmyx",
    assignments: [],
    approvedColorFallbacks: [],
    materialSubstitutions: [],
  };
  return plan;
}

async function renderAnalyzedNativeApp(
  analyzePlan: ProjectPlan,
  replanPlans: ProjectPlan[] = [],
) {
  (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
  vi.mocked(open).mockResolvedValue("/tmp/Withered_Foxy.3mf");
  mockNativeInvoke({ analyzePlan, replanPlans });
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "Choose 3MF" }));
  await screen.findByRole("heading", { name: "Withered_Foxy.3mf" });
  fireEvent.click(screen.getByRole("button", { name: "Analyze Project" }));
  await screen.findByRole("table", { name: /U1 Plates in planned print order/i });
}

interface NativeInvokeFixtures {
  analyzePlan: ProjectPlan;
  analyzeProject?: () => Promise<ProjectPlan>;
  replanPlans?: ProjectPlan[];
  calibrationLibrary?: CmyxCalibrationLibraryDocument;
}

function mockNativeInvoke({
  analyzePlan,
  analyzeProject,
  replanPlans = [],
  calibrationLibrary = {
    schemaVersion: 2,
    planningGeometryContext: {
      orientation: "unknown",
      geometryClass: "unknown",
    },
    records: [],
  },
}: NativeInvokeFixtures) {
  const queuedReplans = [...replanPlans];
  let currentCalibrationLibrary = structuredClone(calibrationLibrary);
  const library: FilamentLibraryDocument = {
    schemaVersion: 2,
    spools: createDemoPlan().spools,
  };

  vi.mocked(invoke).mockImplementation(async (command, args) => {
    switch (command) {
      case "load_filament_library":
        return structuredClone(library);
      case "save_filament_library":
        return structuredClone(
          (args as { library: FilamentLibraryDocument }).library,
        );
      case "load_cmyx_calibration_library":
        return structuredClone(currentCalibrationLibrary);
      case "set_cmyx_calibration_geometry_context":
        currentCalibrationLibrary = {
          ...currentCalibrationLibrary,
          planningGeometryContext: structuredClone(
            (args as { geometryContext: CmyxCalibrationLibraryDocument["planningGeometryContext"] })
              .geometryContext,
          ),
        };
        return structuredClone(currentCalibrationLibrary);
      case "analyze_project":
        if (analyzeProject) return analyzeProject();
        return structuredClone(analyzePlan);
      case "cancel_analysis":
        return { accepted: true };
      case "replan_project": {
        const replanned = queuedReplans.shift();
        if (!replanned) {
          throw new Error("No native replan fixture remains.");
        }
        return structuredClone(replanned);
      }
      case "export_plan":
        return undefined;
      default:
        throw new Error(`Unexpected native command: ${command}`);
    }
  });
}

function nativeCommandCalls(command: string) {
  return vi
    .mocked(invoke)
    .mock.calls.filter(([calledCommand]) => calledCommand === command);
}

const colorResolutionFixtures: ColorResolution[] = [
  {
    scopeId: "plate-1",
    scopeName: "Head",
    requirementId: "plate-1-requirement-2",
    candidateId: "head-grey-cmy-grey-v1",
    sourceMaterial: "PLA",
    sourceHex: "#8E9089",
    targetMaterial: "PLA",
    targetHex: "#8E9089",
    predictedHex: "#9199A4",
    recipe: "Solid T4 Grey",
    deltaE00: 10.1,
    confidence: "Nominal",
    requiredT4SpoolId: "panchroma-translucent-grey",
    requiredT4Name: "Panchroma Translucent Grey",
    requiredT4Hex: "#9199A4",
    colorApproved: false,
    materialApproved: false,
    requiresMaterialSubstitution: false,
    canAddDedicatedSpool: true,
    recommendation: "Closest available CMY + Grey result; visible difference expected.",
  },
  {
    scopeId: "plate-6",
    scopeName: "Frame AMS",
    requirementId: "plate-6-requirement-1",
    candidateId: "frame-petg-to-pla-grey-v1",
    sourceMaterial: "PETG",
    sourceHex: "#ADB1B2",
    targetMaterial: "PLA",
    targetHex: "#ADB1B2",
    predictedHex: "#A8ADB0",
    recipe: "CMY + Grey",
    deltaE00: 2.2,
    confidence: "Nominal",
    requiredT4SpoolId: "panchroma-translucent-grey",
    requiredT4Name: "Panchroma Translucent Grey",
    requiredT4Hex: "#9199A4",
    colorApproved: false,
    materialApproved: false,
    requiresMaterialSubstitution: true,
    canAddDedicatedSpool: true,
    recommendation: "Add PETG Grey or explicitly accept PLA for this frame part.",
  },
];

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((promiseResolve, promiseReject) => {
    resolve = promiseResolve;
    reject = promiseReject;
  });
  return { promise, resolve, reject };
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  window.localStorage.clear();
  delete (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
});

describe("Print Plan workflow", () => {
  it("starts without a false-ready plan and gates plan actions", () => {
    render(<App />);

    expect(screen.getByRole("heading", { name: "No project selected" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "No analyzed print plan" })).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Export JSON Plan" })).toBeDisabled();
    expect(
      screen.getByRole("button", {
        name: /Validate Choices.*Analyze a project first/i,
      }),
    ).toBeDisabled();
    expect(
      screen.getByRole("checkbox", {
        name: "Use Bambu Lab A1 mini for eligible mono parts",
      }),
    ).toBeDisabled();
  });

  it("opens the browser calibration workflow without implying native persistence", async () => {
    render(<App />);

    fireEvent.click(
      within(
        screen.getByRole("navigation", { name: "Application views" }),
      ).getByRole("button", { name: "Color Calibration" }),
    );

    expect(
      await screen.findByRole("heading", { name: "CMY+X Color Calibration" }),
    ).toBeInTheDocument();
    expect(document.title).toBe("Color Calibration | U1 3MF Color Planner");
    expect(
      screen.getByText(
        /Saved only in this browser demo profile; not used by native planning/i,
      ),
    ).toBeInTheDocument();
    expect(screen.getByText("Reuse disabled")).toBeInTheDocument();
  });

  it("shows a recoverable calibration loading error without replacing stored data", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const filamentLibrary: FilamentLibraryDocument = {
      schemaVersion: 2,
      spools: createDemoPlan().spools,
    };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "load_filament_library") {
        return structuredClone(filamentLibrary);
      }
      if (command === "load_cmyx_calibration_library") {
        throw new Error("fixture calibration storage unavailable");
      }
      throw new Error(`Unexpected native command: ${command}`);
    });

    render(<App />);
    fireEvent.click(
      screen.getByRole("button", { name: "Color Calibration" }),
    );

    expect(
      await screen.findByRole("heading", {
        name: "Color calibration unavailable",
      }),
    ).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent(
      "fixture calibration storage unavailable",
    );
    expect(
      screen.getByRole("button", { name: "Retry loading" }),
    ).toBeEnabled();
  });

  it("invalidates an analyzed native plan after planning geometry changes", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const nativePlan = createDemoPlan();
    vi.mocked(open).mockResolvedValue("/tmp/Withered_Foxy.3mf");
    mockNativeInvoke({ analyzePlan: nativePlan });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Choose 3MF" }));
    await waitFor(() =>
      expect(
        screen.getByRole("heading", { name: "Withered_Foxy.3mf" }),
      ).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Analyze Project" }));
    await waitFor(() =>
      expect(
        screen.getByRole("table", { name: /U1 Plates in planned print order/i }),
      ).toBeInTheDocument(),
    );

    fireEvent.click(screen.getByRole("button", { name: "Color Calibration" }));
    await screen.findByRole("heading", { name: "CMY+X Color Calibration" });
    fireEvent.change(screen.getByLabelText("Planning geometry preset"), {
      target: { value: "flat-swatch" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Save planning geometry" }),
    );
    await waitFor(() =>
      expect(
        nativeCommandCalls("set_cmyx_calibration_geometry_context"),
      ).toHaveLength(1),
    );
    expect(
      nativeCommandCalls("set_cmyx_calibration_geometry_context")[0],
    ).toEqual([
      "set_cmyx_calibration_geometry_context",
      {
        geometryContext: {
          orientation: "flat",
          geometryClass: "calibration_swatch",
        },
      },
    ]);

    fireEvent.click(screen.getByRole("button", { name: "Print Plan" }));
    expect(
      screen.getByRole("button", {
        name: /Recalculate Plan.*Apply approved color decisions/i,
      }),
    ).toBeEnabled();
    expect(
      screen.getByRole("button", {
        name: /Export JSON Plan.*Validate pending edits first/i,
      }),
    ).toBeDisabled();
  });

  it("cancels native analysis without surfacing a late worker error", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const pendingAnalysis = deferred<ProjectPlan>();
    vi.mocked(open).mockResolvedValue("/tmp/Withered_Foxy.3mf");
    mockNativeInvoke({
      analyzePlan: createDemoPlan(),
      analyzeProject: () => pendingAnalysis.promise,
    });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Choose 3MF" }));
    await screen.findByRole("heading", { name: "Withered_Foxy.3mf" });
    fireEvent.click(screen.getByRole("button", { name: "Analyze Project" }));
    await waitFor(() =>
      expect(nativeCommandCalls("analyze_project")).toHaveLength(1),
    );

    fireEvent.click(screen.getByRole("button", { name: "Cancel Analysis" }));
    await waitFor(() =>
      expect(nativeCommandCalls("cancel_analysis")).toHaveLength(1),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Analyze Project" }),
      ).toBeEnabled(),
    );

    await act(async () => {
      pendingAnalysis.reject(new Error("late cancelled worker failure"));
      await pendingAnalysis.promise.catch(() => undefined);
    });

    expect(
      screen.getByText(
        "Analysis canceled. The selected 3MF is ready to analyze again.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.queryByText(/Analysis could not be completed/i),
    ).not.toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  it("keeps a newer native analysis when a cancelled promise resolves late", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const firstAnalysis = deferred<ProjectPlan>();
    const freshPlan = createDemoPlan();
    freshPlan.summary.fileName = "Fresh_Result.3mf";
    const stalePlan = createDemoPlan();
    stalePlan.summary.fileName = "Stale_Result.3mf";
    let analysisCall = 0;
    vi.mocked(open).mockResolvedValue("/tmp/Withered_Foxy.3mf");
    mockNativeInvoke({
      analyzePlan: freshPlan,
      analyzeProject: () => {
        analysisCall += 1;
        return analysisCall === 1
          ? firstAnalysis.promise
          : Promise.resolve(structuredClone(freshPlan));
      },
    });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Choose 3MF" }));
    await screen.findByRole("heading", { name: "Withered_Foxy.3mf" });
    fireEvent.click(screen.getByRole("button", { name: "Analyze Project" }));
    await waitFor(() =>
      expect(nativeCommandCalls("analyze_project")).toHaveLength(1),
    );
    fireEvent.click(screen.getByRole("button", { name: "Cancel Analysis" }));
    const analyzeAgain = await screen.findByRole("button", {
      name: "Analyze Project",
    });
    fireEvent.click(analyzeAgain);

    await screen.findByRole("heading", { name: "Fresh_Result.3mf" });
    expect(nativeCommandCalls("analyze_project")).toHaveLength(2);
    await act(async () => {
      firstAnalysis.resolve(stalePlan);
      await firstAnalysis.promise;
    });

    expect(
      screen.getByRole("heading", { name: "Fresh_Result.3mf" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("heading", { name: "Stale_Result.3mf" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText(/Analysis could not be completed/i),
    ).not.toBeInTheDocument();
  });

  it("keeps filament editing unavailable until the persistent library loads", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const library: FilamentLibraryDocument = {
      schemaVersion: 2,
      spools: createDemoPlan().spools,
    };
    let resolveLibrary!: (value: FilamentLibraryDocument) => void;
    const pendingLibrary = new Promise<FilamentLibraryDocument>((resolve) => {
      resolveLibrary = resolve;
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "load_filament_library") return pendingLibrary;
      throw new Error(`Unexpected native command: ${command}`);
    });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Filament Library" }));

    expect(
      screen.getByRole("heading", { name: "Loading filament library" }),
    ).toBeInTheDocument();
    expect(screen.queryByText("Add physical spool")).not.toBeInTheDocument();

    await act(async () => resolveLibrary(structuredClone(library)));
    await waitFor(() =>
      expect(
        screen.getByRole("heading", { name: "Filament Library" }),
      ).toBeInTheDocument(),
    );
  });

  it("keeps a failed filament library load unavailable without claiming it is saved", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    vi.mocked(invoke).mockRejectedValue(new Error("Library JSON is corrupt."));

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Filament Library" }));

    await waitFor(() =>
      expect(
        screen.getByRole("heading", { name: "Filament library unavailable" }),
      ).toBeInTheDocument(),
    );
    expect(
      screen.getByText(
        "Filament library is unavailable. Resolve the loading error before editing or planning.",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText("Add physical spool")).not.toBeInTheDocument();
    expect(
      screen.queryByText("Saved in this app's local data directory."),
    ).not.toBeInTheDocument();
  });

  it("reports an unsaved filament library instead of a false saved status", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const library: FilamentLibraryDocument = {
      schemaVersion: 2,
      spools: createDemoPlan().spools,
    };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "load_filament_library") return structuredClone(library);
      if (command === "save_filament_library") {
        throw new Error("Disk is read-only.");
      }
      throw new Error(`Unexpected native command: ${command}`);
    });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Filament Library" }));
    await waitFor(() =>
      expect(
        screen.getByRole("heading", { name: "Filament Library" }),
      ).toBeInTheDocument(),
    );
    fireEvent.click(
      screen.getByRole("button", {
        name: "Mark out of stock: PolyLite Graphite",
      }),
    );

    await waitFor(() =>
      expect(
        screen.getByText(
          "Changes are not saved. Resolve the saving error, then retry the change.",
        ),
      ).toBeInTheDocument(),
    );
    expect(
      screen.queryByText("Saved in this app's local data directory."),
    ).not.toBeInTheDocument();
  });

  it("keeps a user spool visible when its persistent deletion fails", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const library: FilamentLibraryDocument = {
      schemaVersion: 2,
      spools: [...createDemoPlan().spools, removableUserSpool],
    };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "load_filament_library") return structuredClone(library);
      if (command === "save_filament_library") {
        throw new Error("Disk is read-only.");
      }
      throw new Error(`Unexpected native command: ${command}`);
    });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Filament Library" }));
    await waitFor(() =>
      expect(screen.getByText("Workshop Orange")).toBeInTheDocument(),
    );

    fireEvent.click(
      screen.getByRole("button", { name: "Delete Workshop Orange" }),
    );
    fireEvent.click(
      screen.getByRole("button", {
        name: "Confirm delete Workshop Orange",
      }),
    );

    await waitFor(() =>
      expect(
        screen.getByText(/Filament library could not be saved.*Disk is read-only/i),
      ).toBeInTheDocument(),
    );
    expect(screen.getAllByText("Workshop Orange").length).toBeGreaterThan(0);
    expect(
      screen.getByText(
        "The spool was not deleted. Review the storage error and try again.",
      ),
    ).toHaveAttribute("role", "alert");
  });

  it("removes a user spool only after the replacement library is saved", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const library: FilamentLibraryDocument = {
      schemaVersion: 2,
      spools: [...createDemoPlan().spools, removableUserSpool],
    };
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "load_filament_library") return structuredClone(library);
      if (command === "save_filament_library") {
        return structuredClone(
          (args as { library: FilamentLibraryDocument }).library,
        );
      }
      throw new Error(`Unexpected native command: ${command}`);
    });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Filament Library" }));
    await waitFor(() =>
      expect(screen.getByText("Workshop Orange")).toBeInTheDocument(),
    );

    fireEvent.click(
      screen.getByRole("button", { name: "Delete Workshop Orange" }),
    );
    fireEvent.click(
      screen.getByRole("button", {
        name: "Confirm delete Workshop Orange",
      }),
    );

    await waitFor(() =>
      expect(screen.queryByText("Workshop Orange")).not.toBeInTheDocument(),
    );
    const saveCalls = nativeCommandCalls("save_filament_library");
    expect(saveCalls).toHaveLength(1);
    const saved = (
      saveCalls[0][1] as { library: FilamentLibraryDocument }
    ).library;
    expect(saved.spools).not.toContainEqual(
      expect.objectContaining({ id: removableUserSpool.id }),
    );
  });

  it("opens the filament library before analysis and applies stock status to the planner", async () => {
    render(<App />);

    const appNavigation = screen.getByRole("navigation", {
      name: "Application views",
    });
    const printPlanNavigation = within(appNavigation).getByRole("button", {
      name: "Print Plan",
    });
    const libraryNavigation = within(appNavigation).getByRole("button", {
      name: "Filament Library",
    });
    const printRunNavigation = within(appNavigation).getByRole("button", {
      name: "Print Run",
    });

    expect(libraryNavigation).toBeEnabled();
    expect(printRunNavigation).toBeDisabled();
    fireEvent.click(libraryNavigation);
    await waitFor(() =>
      expect(
        screen.getByRole("heading", { name: "Filament Library" }),
      ).toBeInTheDocument(),
    );

    fireEvent.click(printPlanNavigation);
    await analyzeBrowserDemo();
    expect(screen.getByRole("button", { name: "Export JSON Plan" })).toBeEnabled();

    fireEvent.click(libraryNavigation);
    fireEvent.click(
      screen.getByRole("button", {
        name: "Mark out of stock: PolyLite Graphite",
      }),
    );

    await waitFor(() => {
      const serialized = window.localStorage.getItem(
        FILAMENT_LIBRARY_STORAGE_KEY,
      );
      expect(serialized).not.toBeNull();
      const saved = JSON.parse(serialized!) as FilamentLibraryDocument;
      expect(
        saved.spools.find((spool) => spool.id === "graphite")?.available,
      ).toBe(false);
    });

    fireEvent.click(printPlanNavigation);
    for (const spoolSelect of screen.getAllByLabelText("Selected spool")) {
      expect(
        within(spoolSelect).queryByRole("option", {
          name: "Graphite · PLA — PolyLite Graphite",
        }),
      ).not.toBeInTheDocument();
    }
    const currentT4 = screen.getByLabelText("Currently loaded T4 spool");
    expect(currentT4).toHaveValue("matte-grey");
    expect(
      within(currentT4).queryByRole("option", { name: "Graphite · PLA" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", {
        name: /Export JSON Plan.*Resolve blocking choices first/i,
      }),
    ).toBeDisabled();
    expect(printRunNavigation).toBeDisabled();

    fireEvent.click(
      screen.getByRole("button", { name: /Recalculate Plan/i }),
    );
    await waitFor(() =>
      expect(
        screen.getByText(/Choices validated locally using demo data/i),
      ).toBeInTheDocument(),
    );
    expect(screen.getByText(BROWSER_OUT_OF_STOCK_ERROR_FOR_TEST)).toBeInTheDocument();
    expect(
      screen.getByRole("button", {
        name: /Export JSON Plan.*Resolve blocking choices first/i,
      }),
    ).toBeDisabled();
  });

  it("keeps a validated native print run locked until conversion publishes its files", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const nativePlan = createDemoPlan();
    vi.mocked(open).mockResolvedValue("/tmp/Withered_Foxy.3mf");
    mockNativeInvoke({
      analyzePlan: nativePlan,
      replanPlans: [nativePlan],
    });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Choose 3MF" }));
    await waitFor(() =>
      expect(
        screen.getByRole("heading", { name: "Withered_Foxy.3mf" }),
      ).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Analyze Project" }));
    await waitFor(() =>
      expect(
        screen.getByRole("table", { name: /U1 Plates in planned print order/i }),
      ).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Validate Choices" }));
    await waitFor(() =>
      expect(screen.getByRole("status")).toHaveTextContent(
        "Choices validated. 8 target plates replanned.",
      ),
    );

    const appNavigation = screen.getByRole("navigation", {
      name: "Application views",
    });
    const printRunNavigation = within(appNavigation).getByRole("button", {
      name: "Print Run",
    });
    expect(printRunNavigation).toBeDisabled();
    expect(printRunNavigation).toHaveAccessibleDescription(
      /successfully convert a native print plan/i,
    );
    expect(
      screen.queryByRole("button", { name: "Start print run" }),
    ).not.toBeInTheDocument();
  });

  it("does not expose a T4 print workflow before its exact files are published", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const nativePlan = createDemoPlan();
    nativePlan.plates = nativePlan.plates.slice(0, 2);
    nativePlan.batches = [
      {
        id: "grey-batch",
        label: "CMY + Grey",
        detail: "CMY+X Full Spectrum",
        strategy: "cmyx",
        startOrder: 1,
        endOrder: 1,
        plateCount: 1,
        printer: "U1",
        setupActions: [],
      },
      {
        id: "black-batch",
        label: "CMY + Black",
        detail: "CMY+X Full Spectrum",
        strategy: "cmyx",
        startOrder: 2,
        endOrder: 2,
        plateCount: 1,
        printer: "U1",
        setupActions: [],
        t4Change: {
          fromSpoolId: "matte-grey",
          fromSpoolName: "PolyLite Matte Grey",
          toSpoolId: "black-petg",
          toSpoolName: "PolyLite PETG Black",
        },
      },
    ];
    vi.mocked(open).mockResolvedValue("/tmp/Withered_Foxy.3mf");
    mockNativeInvoke({ analyzePlan: nativePlan });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Choose 3MF" }));
    await waitFor(() =>
      expect(
        screen.getByRole("heading", { name: "Withered_Foxy.3mf" }),
      ).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Analyze Project" }));
    await waitFor(() =>
      expect(
        screen.getByRole("table", { name: /U1 Plates in planned print order/i }),
      ).toBeInTheDocument(),
    );

    const printRunNavigation = within(
      screen.getByRole("navigation", { name: "Application views" }),
    ).getByRole("button", { name: "Print Run" });
    expect(printRunNavigation).toBeDisabled();
    expect(
      screen.queryByRole("heading", {
        name: "Filament change required before Target Plate 02",
      }),
    ).not.toBeInTheDocument();
  });

  it("renders the semantic plan and keeps A1 mini rows strictly mono", async () => {
    await renderAnalyzedApp();

    expect(screen.getByRole("heading", { name: "Withered_Foxy.3mf" })).toBeInTheDocument();
    expect(screen.getByText("Browser demo · Bambu Studio 3MF · sha256:8f3a…91c2")).toBeInTheDocument();
    expect(
      screen.getByRole("table", {
        name: /U1 Plates in planned print order/i,
      }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("table", {
        name: /A1 mini Plates in planned print order/i,
      }),
    ).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "U1 Plates" })).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "A1 mini Plates" }),
    ).toBeInTheDocument();

    const standRow = screen
      .getByRole("button", { name: /Stand — Plate 07/i })
      .closest("tr");
    expect(standRow).not.toBeNull();
    expect(within(standRow!).getByText("A1 Mono")).toBeInTheDocument();
    expect(within(standRow!).queryByText("CMY+X Full Spectrum")).not.toBeInTheDocument();
  });

  it("explains empty U1 and A1 mini plate queues", () => {
    const plan = createDemoPlan();

    render(
      <PlateTable
        plates={[]}
        spools={plan.spools}
        selectedPlateId=""
        onSelectPlate={vi.fn()}
        a1MiniEnabled={true}
        hasPendingPlanChanges={false}
      />,
    );

    expect(
      screen.getByText("No plates are assigned to the Snapmaker U1 in this plan."),
    ).toBeInTheDocument();
    expect(
      screen.getByText("No eligible mono plates were assigned to the A1 mini."),
    ).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  it("shows detected alternative joint plates as excluded checkboxes", async () => {
    await renderAnalyzedApp();

    expect(
      screen.getByRole("heading", { name: "Alternative source plates" }),
    ).toBeInTheDocument();
    for (const name of [
      "Updated Ball Joints",
      "Alternate Joints",
      "Hinge Joints",
      "Ankle Joints",
    ]) {
      expect(
        screen.getByRole("checkbox", { name: new RegExp(name, "i") }),
      ).not.toBeChecked();
    }
  });

  it("shows the exact whole-project Direct Spools limit when more than four pairs exist", async () => {
    await renderAnalyzedApp();

    expect(
      screen.getByRole("heading", { name: "Project-wide Direct Spools" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText("10 semantic · 10/4 physical"),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        "Project-wide Direct Spools is unavailable because 10 semantic material-color-role pairs require 10 unique physical Direct identities; the maximum is 4.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Apply project-wide palette" }),
    ).not.toBeInTheDocument();
  });

  it("applies one <=4-pair Direct palette across every scope in the native replan DTO", async () => {
    const nativePlan = createProjectWideDirectNativePlan();
    await renderAnalyzedNativeApp(nativePlan, [structuredClone(nativePlan)]);

    expect(
      screen.getByText("2 semantic · 2/4 physical"),
    ).toBeInTheDocument();
    const projectPaletteBeforeEditing = screen
      .getByRole("heading", { name: "Project-wide Direct Spools" })
      .closest("section");
    expect(projectPaletteBeforeEditing).not.toBeNull();
    expect(
      within(projectPaletteBeforeEditing!).getAllByText("Current CMY+X"),
    ).toHaveLength(2);
    expect(
      within(projectPaletteBeforeEditing!).getByText("Varies by scope"),
    ).toBeInTheDocument();
    expect(
      within(projectPaletteBeforeEditing!).getByText("Ratio T1·T2"),
    ).toBeInTheDocument();
    expect(projectPaletteBeforeEditing).toHaveTextContent(
      "#C85A55 · ΔE00 4.2 · High",
    );
    const spoolSelects = screen.getAllByLabelText("Physical spool at print time");
    expect(spoolSelects).toHaveLength(2);
    for (const select of spoolSelects) {
      fireEvent.change(select, { target: { value: "signal-red" } });
    }
    expect(screen.getAllByText(/Preview Direct ΔE00/)).toHaveLength(2);
    expect(within(projectPaletteBeforeEditing!).getAllByText("Nominal color")).toHaveLength(2);

    const materialRisks = screen.getAllByRole("checkbox", {
      name: /mechanical-property risk for this pair/i,
    });
    expect(materialRisks).toHaveLength(2);
    expect(materialRisks[0]).not.toBeChecked();
    expect(materialRisks[1]).not.toBeChecked();
    fireEvent.click(materialRisks[0]);
    expect(materialRisks[0]).toBeChecked();
    expect(materialRisks[1]).not.toBeChecked();
    fireEvent.click(materialRisks[1]);
    fireEvent.click(
      screen.getByRole("button", { name: "Apply project-wide palette" }),
    );

    expect(screen.getByRole("status")).toHaveTextContent(
      "Project-wide Direct Spools assigned 2 semantic pairs across 2 physical identities to 1 physical spool",
    );
    const projectPalette = screen
      .getByRole("heading", { name: "Project-wide Direct Spools" })
      .closest("section");
    expect(projectPalette).not.toBeNull();
    expect(
      projectPalette!.querySelectorAll(".project-direct-palette__toolhead"),
    ).toHaveLength(2);
    expect(
      [...projectPalette!.querySelectorAll(".project-direct-palette__toolhead")].map(
        (element) => element.textContent,
      ),
    ).toEqual(["T1", "T1"]);

    fireEvent.click(screen.getByRole("button", { name: /Recalculate Plan/i }));
    await waitFor(() => expect(nativeCommandCalls("replan_project")).toHaveLength(1));
    const request = (
      nativeCommandCalls("replan_project")[0][1] as {
        request: { scopeOverrides: ProjectPlan["scopeSelections"] };
      }
    ).request;
    expect(request.scopeOverrides).toEqual([
      {
        scopeId: "plate-1",
        strategy: "direct",
        assignments: [
          {
            requirementId: "plate-1-red",
            spoolId: "signal-red",
            toolhead: "T1",
            allowMaterialSubstitution: true,
          },
          {
            requirementId: "plate-1-grey",
            spoolId: "signal-red",
            toolhead: "T1",
            allowMaterialSubstitution: true,
          },
        ],
        approvedColorFallbacks: [],
        materialSubstitutions: [],
      },
      {
        scopeId: "plate-2",
        strategy: "direct",
        assignments: [
          {
            requirementId: "plate-2-red",
            spoolId: "signal-red",
            toolhead: "T1",
            allowMaterialSubstitution: true,
          },
          {
            requirementId: "plate-2-grey",
            spoolId: "signal-red",
            toolhead: "T1",
            allowMaterialSubstitution: true,
          },
        ],
        approvedColorFallbacks: [],
        materialSubstitutions: [],
      },
    ]);
  });

  it("keeps five role rows while linking them to four physical Direct identities", async () => {
    const nativePlan = createProjectWideDirectNativePlan();
    const [red, grey] = nativePlan.projectDirectPalette.mappings;
    const withIdentity = (
      mapping: typeof red,
      id: string,
      physicalIdentityId: string,
      sourceRole: string,
      sourceHex: string,
      profile: string,
      requirementSuffix: string,
    ) => ({
      ...structuredClone(mapping),
      id,
      physicalIdentityId,
      sourceMaterial: "PLA",
      sourceRole,
      sourceHex,
      sourceProfileIds: [profile],
      selectedSpoolId: "",
      references: mapping.references.map((reference) => ({
        ...reference,
        requirementIds: [`${reference.scopeId}-${requirementSuffix}`],
      })),
    });
    nativePlan.projectDirectPalette = {
      available: true,
      effectivePairCount: 5,
      directPairCount: 4,
      maximumPairCount: 4,
      unavailableReason: null,
      mappings: [
        withIdentity(
          red,
          "role-pair-01",
          "physical-01",
          "Cosmetic",
          "#C72E2A",
          "profile-shared-red",
          "red-cosmetic",
        ),
        withIdentity(
          red,
          "role-pair-02",
          "physical-01",
          "Support",
          "#C72E2A",
          "profile-shared-red",
          "red-support",
        ),
        withIdentity(
          grey,
          "role-pair-03",
          "physical-02",
          "Functional",
          "#ADB1B2",
          "profile-grey",
          "grey",
        ),
        withIdentity(
          red,
          "role-pair-04",
          "physical-03",
          "Cosmetic",
          "#2A9650",
          "profile-green",
          "green",
        ),
        withIdentity(
          red,
          "role-pair-05",
          "physical-04",
          "Cosmetic",
          "#1E46B4",
          "profile-blue",
          "blue",
        ),
      ],
    };
    nativePlan.projectDirectPalette.mappings[0].sourceMaterial = "PETG";
    nativePlan.projectDirectPalette.mappings[1].sourceMaterial = "PETG";
    await renderAnalyzedNativeApp(nativePlan, [structuredClone(nativePlan)]);

    expect(
      screen.getByText("5 semantic · 4/4 physical"),
    ).toBeInTheDocument();
    const spoolSelects = screen.getAllByLabelText("Physical spool at print time");
    expect(spoolSelects).toHaveLength(5);
    fireEvent.change(spoolSelects[0], { target: { value: "signal-red" } });
    expect(spoolSelects[1]).toHaveValue("signal-red");
    const materialRisks = screen.getAllByRole("checkbox", {
      name: /mechanical-property risk for this pair/i,
    });
    expect(materialRisks).toHaveLength(2);
    fireEvent.click(materialRisks[0]);
    expect(materialRisks[0]).toBeChecked();
    expect(materialRisks[1]).not.toBeChecked();
    fireEvent.click(materialRisks[1]);
    for (const select of spoolSelects.slice(2)) {
      fireEvent.change(select, { target: { value: "signal-red" } });
    }
    fireEvent.click(
      screen.getByRole("button", { name: "Apply project-wide palette" }),
    );
    expect(screen.getByRole("status")).toHaveTextContent(
      "5 semantic pairs across 4 physical identities to 1 physical spool",
    );

    fireEvent.click(screen.getByRole("button", { name: /Recalculate Plan/i }));
    await waitFor(() => expect(nativeCommandCalls("replan_project")).toHaveLength(1));
    const request = (
      nativeCommandCalls("replan_project")[0][1] as {
        request: { scopeOverrides: ProjectPlan["scopeSelections"] };
      }
    ).request;
    for (const scope of request.scopeOverrides) {
      expect(scope.assignments).toHaveLength(5);
      const cosmetic = scope.assignments.find((assignment) =>
        assignment.requirementId.endsWith("red-cosmetic"),
      );
      const support = scope.assignments.find((assignment) =>
        assignment.requirementId.endsWith("red-support"),
      );
      expect(cosmetic).toMatchObject({ spoolId: "signal-red", toolhead: "T1" });
      expect(support).toMatchObject({ spoolId: "signal-red", toolhead: "T1" });
    }
  });

  it("selects rows and switches an eligible U1 plate to Direct Spools", async () => {
    await renderAnalyzedApp();

    const directRadio = screen.getByRole("radio", { name: /Direct Spools/i });
    fireEvent.click(directRadio);

    expect(directRadio).toBeChecked();
    const headRow = screen
      .getByRole("button", { name: /Head — Plate 01/i })
      .closest("tr");
    expect(headRow).not.toBeNull();
    expect(within(headRow!).getByText("Direct Spools")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /Stand — Plate 07/i }));
    expect(screen.getByRole("heading", { name: "Stand — Plate 07" })).toBeInTheDocument();
    expect(screen.getByText("A1 Mono", { selector: ".locked-strategy strong" })).toBeInTheDocument();
    expect(screen.queryByRole("radio", { name: /CMY\+X Full Spectrum/i })).not.toBeInTheDocument();
  });

  it("edits five semantic rows through four linked per-plate physical identities", async () => {
    const nativePlan = createPerPlateRoleSharedNativePlan();
    await renderAnalyzedNativeApp(nativePlan, [structuredClone(nativePlan)]);

    const directRadio = screen.getByRole("radio", { name: /Direct Spools/i });
    expect(directRadio).toBeEnabled();
    fireEvent.click(directRadio);
    expect(directRadio).toBeChecked();
    expect(
      screen.getByRole("heading", { name: "Direct spool mapping" }),
    ).toBeInTheDocument();
    expect(screen.getAllByText(/^Shared physical identity 1/)).toHaveLength(2);

    const spoolSelects = screen.getAllByLabelText("Selected spool");
    const toolheadSelects = screen.getAllByLabelText("Direct toolhead");
    expect(spoolSelects).toHaveLength(5);
    expect(toolheadSelects).toHaveLength(5);
    fireEvent.change(spoolSelects[1], { target: { value: "signal-red" } });
    expect(spoolSelects[0]).toHaveValue("signal-red");
    expect(spoolSelects[1]).toHaveValue("signal-red");
    expect(toolheadSelects[0]).toHaveValue("T1");
    expect(toolheadSelects[1]).toHaveValue("T1");

    const materialRisks = screen.getAllByRole("checkbox", {
      name: "I understand the mechanical-property risk.",
    });
    expect(materialRisks).toHaveLength(2);
    expect(materialRisks[0]).not.toBeChecked();
    expect(materialRisks[1]).not.toBeChecked();
    fireEvent.click(materialRisks[0]);
    expect(materialRisks[0]).toBeChecked();
    expect(materialRisks[1]).not.toBeChecked();

    fireEvent.change(toolheadSelects[1], { target: { value: "T4" } });
    expect(toolheadSelects[0]).toHaveValue("T4");
    expect(toolheadSelects[1]).toHaveValue("T4");
    expect(toolheadSelects[4]).toHaveValue("T1");
    fireEvent.click(materialRisks[1]);

    fireEvent.click(screen.getByRole("button", { name: /Recalculate Plan/i }));
    await waitFor(() => expect(nativeCommandCalls("replan_project")).toHaveLength(1));
    const request = (
      nativeCommandCalls("replan_project")[0][1] as {
        request: { scopeOverrides: ProjectPlan["scopeSelections"] };
      }
    ).request;
    const plateOverride = request.scopeOverrides.find(
      (scope) => scope.scopeId === "plate-1",
    );
    expect(plateOverride?.strategy).toBe("direct");
    expect(plateOverride?.assignments).toHaveLength(5);
    expect(
      plateOverride?.assignments.find(
        (assignment) => assignment.requirementId === "plate-1-red-cosmetic",
      ),
    ).toMatchObject({
      spoolId: "signal-red",
      toolhead: "T4",
      allowMaterialSubstitution: true,
    });
    expect(
      plateOverride?.assignments.find(
        (assignment) => assignment.requirementId === "plate-1-red-support",
      ),
    ).toMatchObject({
      spoolId: "signal-red",
      toolhead: "T4",
      allowMaterialSubstitution: true,
    });
  });

  it("edits direct spool assignments and swaps occupied toolheads", async () => {
    await renderAnalyzedApp();

    const toolheadSelects = screen.getAllByLabelText("Direct toolhead");
    expect(toolheadSelects).toHaveLength(4);
    expect(toolheadSelects[0]).toHaveValue("T1");
    expect(toolheadSelects[3]).toHaveValue("T4");

    fireEvent.change(toolheadSelects[0], { target: { value: "T4" } });
    expect(toolheadSelects[0]).toHaveValue("T4");
    expect(toolheadSelects[3]).toHaveValue("T1");

    const spoolSelects = screen.getAllByLabelText("Selected spool");
    fireEvent.change(spoolSelects[0], { target: { value: "signal-red" } });
    expect(spoolSelects[0]).toHaveValue("signal-red");
    expect(screen.getAllByText("#C72E2A").length).toBeGreaterThan(0);
  });

  it("combines multiple source colors into one physical spool without clearing the first", async () => {
    await renderAnalyzedApp();

    const spoolSelects = screen.getAllByLabelText("Selected spool");
    const firstSpoolId = (spoolSelects[0] as HTMLSelectElement).value;
    expect(firstSpoolId).not.toBe("");

    fireEvent.change(spoolSelects[1], { target: { value: firstSpoolId } });

    expect(spoolSelects[0]).toHaveValue(firstSpoolId);
    expect(spoolSelects[1]).toHaveValue(firstSpoolId);
    const toolheadSelects = screen.getAllByLabelText("Direct toolhead");
    expect(toolheadSelects[1]).toHaveValue(
      (toolheadSelects[0] as HTMLSelectElement).value,
    );
    expect(
      screen.getByRole("heading", { name: "Combined source colors" }),
    ).toBeInTheDocument();
    expect(screen.getByText(/2 physical identities →/i)).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(
      "2 physical Direct identities will use",
    );
    expect(
      screen.getByRole("button", {
        name: /Recalculate Plan.*Apply pending printer, spool, or plan changes/i,
      }),
    ).toBeEnabled();
    expect(
      screen.getByText(/Pending edits are not reflected in these queues yet/i),
    ).toBeInTheDocument();
  });

  it("moves a split physical identity back to a free toolhead before replanning", async () => {
    const nativePlan = createDemoPlan();
    nativePlan.plates[0].strategy = "direct";
    nativePlan.scopeSelections[0].strategy = "direct";
    await renderAnalyzedNativeApp(nativePlan, [structuredClone(nativePlan)]);

    const spoolSelects = screen.getAllByLabelText("Selected spool");
    const toolheadSelects = screen.getAllByLabelText("Direct toolhead");
    const firstSpoolId = (spoolSelects[0] as HTMLSelectElement).value;
    expect(toolheadSelects[0]).toHaveValue("T1");
    expect(toolheadSelects[1]).toHaveValue("T2");

    fireEvent.change(spoolSelects[1], { target: { value: firstSpoolId } });
    expect(toolheadSelects[0]).toHaveValue("T1");
    expect(toolheadSelects[1]).toHaveValue("T1");

    fireEvent.change(spoolSelects[1], { target: { value: "" } });
    expect(spoolSelects[1]).toHaveValue("");
    expect(toolheadSelects[1]).toHaveValue("T1");
    fireEvent.change(toolheadSelects[1], { target: { value: "T3" } });
    expect(toolheadSelects[1]).toHaveValue("T3");
    expect(toolheadSelects[2]).toHaveValue("T3");

    fireEvent.change(spoolSelects[1], { target: { value: "signal-red" } });
    expect(spoolSelects[0]).toHaveValue(firstSpoolId);
    expect(spoolSelects[1]).toHaveValue("signal-red");
    expect(toolheadSelects[0]).toHaveValue("T1");
    expect(toolheadSelects[1]).toHaveValue("T2");
    expect(toolheadSelects[2]).toHaveValue("T3");

    fireEvent.click(screen.getByRole("button", { name: /Recalculate Plan/i }));
    await waitFor(() => expect(nativeCommandCalls("replan_project")).toHaveLength(1));
    const request = (
      nativeCommandCalls("replan_project")[0][1] as {
        request: { scopeOverrides: ProjectPlan["scopeSelections"] };
      }
    ).request;
    const plateOverride = request.scopeOverrides.find(
      (scope) => scope.scopeId === "plate-1",
    );
    expect(
      plateOverride?.assignments.find(
        (assignment) => assignment.requirementId === "head-cyan",
      ),
    ).toMatchObject({ spoolId: firstSpoolId, toolhead: "T1" });
    expect(
      plateOverride?.assignments.find(
        (assignment) => assignment.requirementId === "head-purple",
      ),
    ).toMatchObject({ spoolId: "signal-red", toolhead: "T2" });
  });

  it("keeps a cleared spool assignment unassigned and marks it for review", async () => {
    await renderAnalyzedApp();

    const spoolSelect = screen.getAllByLabelText("Selected spool")[0];
    const mappingCard = spoolSelect.closest("article");
    expect(mappingCard).not.toBeNull();

    fireEvent.change(spoolSelect, { target: { value: "" } });

    expect(spoolSelect).toHaveValue("");
    expect(within(mappingCard!).getByText("Review")).toBeInTheDocument();
    const actualColor = within(mappingCard!).getByLabelText("Actual spool color");
    expect(within(actualColor).getByText("Unassigned")).toBeInTheDocument();
    expect(actualColor.querySelector(".color-swatch")).toBeNull();
    expect(mappingCard).toHaveTextContent(/Direct ΔE00\s*Unavailable/);
    expect(screen.getByText("Review Unassigned")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(
      "Spool assignment cleared for the linked physical identity. Direct mapping requires review.",
    );
  });

  it("adds a user spool with synchronized color inputs and assigns it", async () => {
    await renderAnalyzedApp();

    fireEvent.click(screen.getByText("Add physical spool"));
    const colorPicker = screen.getByLabelText("Color");
    const hexInput = screen.getByLabelText(/HEX color/i);
    expect(colorPicker).toHaveAttribute("type", "color");

    fireEvent.change(colorPicker, { target: { value: "#c72e2a" } });
    expect(hexInput).toHaveValue("#C72E2A");
    fireEvent.change(hexInput, { target: { value: "#B72E2A" } });
    expect(colorPicker).toHaveValue("#b72e2a");

    fireEvent.change(screen.getByLabelText(/Spool name/i), {
      target: { value: "Workshop Signal Red" },
    });
    fireEvent.change(screen.getByLabelText(/Color name/i), {
      target: { value: "Workshop Red" },
    });
    fireEvent.click(screen.getByRole("radio", { name: /PETG/i }));
    fireEvent.change(screen.getByLabelText("SKU (optional)"), {
      target: { value: "SHOP-RD-01" },
    });
    fireEvent.change(screen.getByLabelText("Profile (optional)"), {
      target: { value: "0.20 mm Workshop PETG" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add spool" }));

    expect(screen.getByText("11 available")).toBeInTheDocument();
    await waitFor(() =>
      expect(screen.getByRole("status")).toHaveTextContent(
        "Workshop Red added to the filament library. Recalculate the plan to use it.",
      ),
    );

    const spoolSelect = screen.getAllByLabelText("Selected spool")[0];
    expect(
      within(spoolSelect).getByRole("option", {
        name: "Workshop Red · PETG — Workshop Signal Red",
      }),
    ).toHaveValue("user-workshop-signal-red");

    fireEvent.change(spoolSelect, {
      target: { value: "user-workshop-signal-red" },
    });
    const mappingCard = spoolSelect.closest("article");
    expect(mappingCard).not.toBeNull();
    expect(within(mappingCard!).getByText("Material mismatch")).toBeInTheDocument();
    expect(mappingCard).toHaveTextContent("#B72E2A");
    expect(mappingCard).toHaveTextContent("SHOP-RD-01");
    expect(mappingCard).toHaveTextContent("0.20 mm Workshop PETG");
  });

  it("shows unavailable CMY data without inventing a predicted swatch", () => {
    const plan = createDemoPlan();
    const mapping = {
      ...plan.plates[0].mappings![0],
      cmyPredictedHex: null,
      cmyDeltaE00: null,
    };

    render(
      <DirectSpoolEditor
        plateId="nullable-cmy"
        mappings={[mapping]}
        spools={plan.spools}
        currentLoadout={plan.currentLoadout}
        restoreCmy={false}
        onRestoreChange={vi.fn()}
        onToolheadChange={vi.fn()}
        onSpoolChange={vi.fn()}
        onMaterialSubstitutionChange={vi.fn()}
        onAddSpool={vi.fn()}
      />,
    );

    const cmyColor = screen.getByLabelText("Current CMY+X color");
    expect(within(cmyColor).getByText("Unavailable")).toBeInTheDocument();
    expect(cmyColor.querySelector(".color-swatch")).toBeNull();
    expect(screen.getByText(/CMY\+X ΔE00 Unavailable/)).toBeInTheDocument();
  });

  it("shows complete keep, unload, load, and optional restore actions", async () => {
    await renderAnalyzedApp();

    expect(screen.getByText("Keep Cyan")).toBeInTheDocument();
    expect(screen.getByText("Unload Magenta")).toBeInTheDocument();
    expect(screen.getByText("Load Royal Purple")).toBeInTheDocument();
    expect(screen.getByText("Restore Magenta")).toBeInTheDocument();

    const restoreToggle = screen.getByRole("checkbox", {
      name: /Restore CMY setup after this job/i,
    });
    fireEvent.click(restoreToggle);

    expect(restoreToggle).not.toBeChecked();
    expect(screen.queryByText("Restore Magenta")).not.toBeInTheDocument();
    expect(screen.getByText("Unload Magenta")).toBeInTheDocument();
    expect(screen.getByText("Load Royal Purple")).toBeInTheDocument();
  });

  it("selects a browser file and completes the explicit demo analysis fallback", async () => {
    render(<App />);

    const input = document.querySelector<HTMLInputElement>("#project-file-input");
    expect(input).not.toBeNull();
    const file = new File(["demo"], "color-study.3mf", { type: "model/3mf" });
    fireEvent.change(input!, { target: { files: [file] } });

    expect(screen.getByRole("heading", { name: "color-study.3mf" })).toBeInTheDocument();
    const analyzeButton = screen.getByRole("button", { name: "Analyze Project" });
    expect(analyzeButton).toBeEnabled();
    fireEvent.click(analyzeButton);

    expect(
      screen.getByRole("button", { name: "Cancel Analysis" }),
    ).toBeEnabled();
    await waitFor(
      () =>
        expect(
          screen.getByText(/Analysis complete for color-study\.3mf.*browser demo data/i),
        ).toBeInTheDocument(),
      { timeout: 2_000 },
    );
  });

  it("keeps conversion unavailable before the print plan is validated", () => {
    render(<App />);

    const convertButton = screen.getByRole("button", {
      name: /Approve & Convert.*Validate the current plan first/i,
    });
    expect(convertButton).toHaveAttribute("aria-disabled", "true");
    expect(convertButton).toBeEnabled();

    convertButton.focus();
    expect(convertButton).toHaveFocus();
    fireEvent.click(convertButton);

    expect(screen.queryByText(/Plan approved/i)).not.toBeInTheDocument();
    expect(screen.getByRole("status")).toBeEmptyDOMElement();
  });

  it("keeps a persisted publication locked until backend revalidation, then recovers and opens its registered artifact", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const nativePlan = createDemoPlan();
    nativePlan.planReady = true;
    nativePlan.blockingErrors = [];
    nativePlan.omittedUnitCount = 0;
    nativePlan.partialConversion = {
      available: false,
      exclusions: [],
      reason: "No partial conversion is needed.",
    };
    const bundle = publishedPrintRunBundleFixture(nativePlan);
    expect(
      savePublishedPrintRunBundle(window.localStorage, nativePlan, bundle),
    ).toBe(true);
    const capability: ConversionCapability = {
      available: true,
      reason: "All required writer adapters are ready.",
      planFingerprint: bundle.backendPlanFingerprint,
      sourceDialectSupport: "Supported",
      experimentalDialectApprovalRequired: false,
      experimentalDialectFingerprint: null,
      adapters: [],
    };
    const library: FilamentLibraryDocument = {
      schemaVersion: 2,
      spools: nativePlan.spools,
    };
    let resolveRevalidation!: (result: ConversionResult) => void;
    const pendingRevalidation = new Promise<ConversionResult>((resolve) => {
      resolveRevalidation = resolve;
    });

    vi.mocked(open).mockResolvedValueOnce("/tmp/Withered_Foxy.3mf");
    vi.mocked(invoke).mockImplementation(async (command) => {
      switch (command) {
        case "load_filament_library":
          return structuredClone(library);
        case "analyze_project":
          return structuredClone(nativePlan);
        case "inspect_conversion_capabilities":
          return structuredClone(capability);
        case "revalidate_published_conversion":
          return pendingRevalidation;
        case "open_output_in_slicer":
          return undefined;
        default:
          throw new Error(`Unexpected native command: ${command}`);
      }
    });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Choose 3MF" }));
    await screen.findByRole("heading", { name: "Withered_Foxy.3mf" });
    fireEvent.click(screen.getByRole("button", { name: "Analyze Project" }));

    await waitFor(() =>
      expect(nativeCommandCalls("revalidate_published_conversion")).toEqual([
        [
          "revalidate_published_conversion",
          {
            sourcePath: "/tmp/Withered_Foxy.3mf",
            sourceSha256: nativePlan.summary.sourceHash,
            planFingerprint: bundle.backendPlanFingerprint,
            outputDirectory: bundle.result.outputDirectory,
            experimentalDialectApproval: null,
          },
        ],
      ]),
    );
    expect(
      screen.getByText(/Revalidating saved Print Run files/i),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Print Run" })).toBeDisabled();

    await act(async () => {
      resolveRevalidation(structuredClone(bundle.result));
      await pendingRevalidation;
    });

    const printRunButton = screen.getByRole("button", { name: "Print Run" });
    await waitFor(() => expect(printRunButton).toBeEnabled());
    fireEvent.click(printRunButton);
    const openButton = screen.getAllByRole("button", {
      name: "Open in Snapmaker Orca",
    })[0];
    fireEvent.click(openButton);
    await waitFor(() =>
      expect(nativeCommandCalls("open_output_in_slicer")).toEqual([
        [
          "open_output_in_slicer",
          {
            path: bundle.result.artifacts[0].path,
            adapterId: bundle.result.artifacts[0].adapterId,
          },
        ],
      ]),
    );
  });

  it("uses the mixed conversion DTO and requires fresh preflight after a consumed failure", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const nativePlan = createDemoPlan();
    const library: FilamentLibraryDocument = {
      schemaVersion: 2,
      spools: nativePlan.spools,
    };
    const capability: ConversionCapability = {
      available: true,
      reason: "All required writer adapters are ready.",
      planFingerprint: "mixed-plan-fingerprint",
      sourceDialectSupport: "Experimental",
      experimentalDialectApprovalRequired: true,
      experimentalDialectFingerprint: "sha256:experimental-source-fixture",
      adapters: [
        {
          target: "u1_direct",
          adapterId: "snapmaker/u1-direct",
          slicer: "Snapmaker Orca",
          required: true,
          available: true,
          reason: "Ready.",
          report: { status: "qualified" },
        },
        {
          target: "a1_mini_mono",
          adapterId: "bambu/a1-mini",
          slicer: "Bambu Studio",
          required: true,
          available: true,
          reason: "Ready.",
          report: { status: "qualified" },
        },
      ],
    };
    const preparedConversion: PreparedConversion = {
      preparationToken: "mixed-preparation-token",
      conversionId: "mixed-conversion-id",
      preparation: {
        adapterId: "u1-planner/mixed-native",
        sourceSha256: nativePlan.summary.sourceHash,
        planFingerprint: "mixed-plan-fingerprint",
        bundleDirectoryName: "Withered_Foxy__converted",
        artifacts: [
          {
            adapterId: "snapmaker/u1-direct",
            target: "u1_direct",
            printer: "Snapmaker U1",
            strategy: "Direct Spools",
            slicer: "Snapmaker Orca",
            batchId: "u1-batch",
            fileName: "Withered_Foxy__u1.3mf",
            targetPlateIds: nativePlan.plates
              .filter((plate) => plate.printer === "U1")
              .map((plate) => plate.id),
            sourceUnitIds: nativePlan.plates
              .filter((plate) => plate.printer === "U1")
              .flatMap((plate) => plate.sourceUnitIds),
            loadout: [],
            setupActions: [],
            adapterEvidence: { status: "prepared" },
          },
          {
            adapterId: "bambu/a1-mini",
            target: "a1_mini_mono",
            printer: "Bambu Lab A1 mini",
            strategy: "A1 Mono",
            slicer: "Bambu Studio",
            batchId: "a1-batch",
            fileName: "Withered_Foxy__a1.3mf",
            targetPlateIds: nativePlan.plates
              .filter((plate) => plate.printer === "A1 mini")
              .map((plate) => plate.id),
            sourceUnitIds: nativePlan.plates
              .filter((plate) => plate.printer === "A1 mini")
              .flatMap((plate) => plate.sourceUnitIds),
            loadout: exactA1PreparedLoadout,
            setupActions: [],
            adapterEvidence: { status: "prepared" },
          },
        ],
        warnings: [],
        excludedSourceUnits: [],
      },
    };
    const conversionResult: ConversionResult = {
      adapterId: "u1-planner/mixed-native",
      outputDirectory: "/tmp/output/Withered_Foxy__converted",
      manifestPath: "/tmp/output/Withered_Foxy__converted/manifest.json",
      reportPath: "/tmp/output/Withered_Foxy__converted/conversion-report.html",
      artifacts: preparedConversion.preparation.artifacts.map((artifact) => ({
        adapterId: artifact.adapterId,
        target: artifact.target,
        printer: artifact.printer,
        strategy: artifact.strategy,
        slicer: artifact.slicer,
        batchId: artifact.batchId,
        fileName: artifact.fileName,
        relativePath: `projects/${artifact.fileName}`,
        path: `/tmp/output/Withered_Foxy__converted/projects/${artifact.fileName}`,
        byteSize: 1024,
        sha256: `${artifact.batchId}-sha256`,
        plateCount: artifact.targetPlateIds.length,
        targetPlateIds: artifact.targetPlateIds,
        sourceUnitIds: artifact.sourceUnitIds,
        loadout: artifact.loadout,
        setupActions: artifact.setupActions,
        validationStatus: "Passed",
        adapterEvidence: { status: "passed" },
      })),
      warnings: [],
      excludedSourceUnits: [],
      warningsAcknowledged: false,
    };

    vi.mocked(open)
      .mockResolvedValueOnce("/tmp/Withered_Foxy.3mf")
      .mockResolvedValueOnce("/tmp/output")
      .mockResolvedValueOnce("/tmp/output");
    let preparationCount = 0;
    let conversionCount = 0;
    vi.mocked(invoke).mockImplementation(async (command) => {
      switch (command) {
        case "load_filament_library":
          return structuredClone(library);
        case "analyze_project":
          return structuredClone(nativePlan);
        case "inspect_conversion_capabilities":
          return structuredClone(capability);
        case "prepare_conversion": {
          preparationCount += 1;
          return structuredClone({
            ...preparedConversion,
            preparationToken: `mixed-preparation-token-${preparationCount}`,
            conversionId: `mixed-conversion-id-${preparationCount}`,
          });
        }
        case "convert_project":
          conversionCount += 1;
          if (conversionCount === 1) {
            throw new Error("fixture writer failure after token consumption");
          }
          return structuredClone(conversionResult);
        default:
          throw new Error(`Unexpected native command: ${command}`);
      }
    });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Choose 3MF" }));
    await waitFor(() =>
      expect(
        screen.getByRole("heading", { name: "Withered_Foxy.3mf" }),
      ).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Analyze Project" }));
    await waitFor(() =>
      expect(nativeCommandCalls("inspect_conversion_capabilities")).toHaveLength(1),
    );

    const experimentalApproval = await screen.findByRole("checkbox", {
      name: /I understand this source dialect is Experimental/i,
    });
    const approveButton = screen.getByText("Approve & Convert").closest("button")!;
    expect(approveButton).toHaveAttribute("aria-disabled", "true");
    fireEvent.click(experimentalApproval);
    await waitFor(() =>
      expect(approveButton).toHaveAttribute("aria-disabled", "false"),
    );
    fireEvent.click(approveButton);

    await waitFor(() =>
      expect(
        screen.getByRole("heading", { name: "2 project files" }),
      ).toBeInTheDocument(),
    );
    expect(screen.getByRole("heading", { name: "Snapmaker U1" })).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "Bambu Lab A1 mini" }),
    ).toBeInTheDocument();
    expect(nativeCommandCalls("prepare_conversion")).toEqual([
      [
        "prepare_conversion",
        {
          sourcePath: "/tmp/Withered_Foxy.3mf",
          sourceSha256: nativePlan.summary.sourceHash,
          planFingerprint: "mixed-plan-fingerprint",
          partialConversionApproval: null,
          experimentalDialectApproval: {
            sourceFingerprint: "sha256:experimental-source-fixture",
          },
        },
      ],
    ]);

    fireEvent.click(screen.getByRole("button", { name: "Convert projects" }));
    await waitFor(() =>
      expect(
        screen.getByRole("alert"),
      ).toHaveTextContent("fixture writer failure after token consumption"),
    );
    expect(
      screen.queryByRole("button", { name: "Convert projects" }),
    ).not.toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "Run preflight again" }),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("heading", { name: "2 project files" }),
      ).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Convert projects" }));
    await waitFor(() =>
      expect(
        screen.getByRole("heading", { name: "Conversion complete" }),
      ).toBeInTheDocument(),
    );
    expect(nativeCommandCalls("convert_project")).toEqual([
      [
        "convert_project",
        {
          preparationToken: "mixed-preparation-token-1",
          destinationDirectory: "/tmp/output",
          acknowledgedWarnings: [],
        },
      ],
      [
        "convert_project",
        {
          preparationToken: "mixed-preparation-token-2",
          destinationDirectory: "/tmp/output",
          acknowledgedWarnings: [],
        },
      ],
    ]);
    expect(screen.getByText("projects/Withered_Foxy__u1.3mf")).toBeInTheDocument();
    expect(screen.getByText("projects/Withered_Foxy__a1.3mf")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    const printRunTab = screen.getByRole("button", { name: "Print Run" });
    expect(printRunTab).toBeEnabled();
    fireEvent.click(printRunTab);
    expect(
      screen.getByRole("heading", { name: "Published files for this run" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText("/tmp/output/Withered_Foxy__converted/manifest.json"),
    ).toBeInTheDocument();
    expect(
      screen.getAllByText("Withered_Foxy__u1.3mf").length,
    ).toBeGreaterThan(0);
  });

  it("passes exact backend exclusions only after explicit partial-conversion approval", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const nativePlan = createDemoPlan();
    const exclusions = [
      {
        scopeId: "plate-8",
        planningUnitId: "plate-8-unit-2",
        sourcePlateId: 8,
        sourceUnitId: "model-42",
        reason: "No schedulable physical loadout is available.",
        errorIdentity: "partial-error-v1:abc",
      },
    ];
    nativePlan.planReady = false;
    nativePlan.omittedUnitCount = 1;
    nativePlan.blockingErrors = [
      "Requirement 'plate-8-unit-2' has no schedulable physical loadout.",
    ];
    nativePlan.partialConversion = {
      available: true,
      exclusions,
      reason: "One source unit can be isolated from the valid print jobs.",
    };
    const library: FilamentLibraryDocument = {
      schemaVersion: 2,
      spools: nativePlan.spools,
    };
    const capability: ConversionCapability = {
      available: true,
      reason: "All required writer adapters are ready.",
      planFingerprint: "partial-plan-fingerprint",
      sourceDialectSupport: "Supported",
      experimentalDialectApprovalRequired: false,
      experimentalDialectFingerprint: null,
      adapters: [],
    };
    const partialU1Plates = nativePlan.plates.filter(
      (plate) => plate.printer === "U1",
    );
    const partialA1Plates = nativePlan.plates.filter(
      (plate) => plate.printer === "A1 mini",
    );
    const preparedConversion: PreparedConversion = {
      preparationToken: "partial-preparation-token",
      conversionId: "partial-conversion-id",
      preparation: {
        adapterId: "u1-planner/mixed-native",
        sourceSha256: nativePlan.summary.sourceHash,
        planFingerprint: "partial-plan-fingerprint",
        bundleDirectoryName: "Withered_Foxy__valid-jobs",
        artifacts: [
          ...(partialU1Plates.length > 0
            ? [{
            adapterId: "snapmaker/u1-direct",
            target: "u1_direct",
            printer: "Snapmaker U1",
            strategy: "Direct Spools",
            slicer: "Snapmaker Orca",
            batchId: "valid-u1-batch",
            fileName: "Withered_Foxy__valid-jobs.3mf",
            targetPlateIds: partialU1Plates.map((plate) => plate.id),
            sourceUnitIds: partialU1Plates.flatMap(
              (plate) => plate.sourceUnitIds,
            ),
            loadout: [],
            setupActions: [],
            adapterEvidence: { status: "prepared" },
          }]
            : []),
          ...(partialA1Plates.length > 0
            ? [{
                adapterId: "bambu/a1-mini",
                target: "a1_mini_mono",
                printer: "Bambu Lab A1 mini",
                strategy: "A1 Mono",
                slicer: "Bambu Studio",
                batchId: "valid-a1-batch",
                fileName: "Withered_Foxy__valid-a1-jobs.3mf",
                targetPlateIds: partialA1Plates.map((plate) => plate.id),
                sourceUnitIds: partialA1Plates.flatMap(
                  (plate) => plate.sourceUnitIds,
                ),
                loadout: exactA1PreparedLoadout,
                setupActions: [],
                adapterEvidence: { status: "prepared" },
              }]
            : []),
        ],
        warnings: [],
        excludedSourceUnits: exclusions,
      },
    };
    const conversionResult: ConversionResult = {
      adapterId: "u1-planner/mixed-native",
      outputDirectory: "/tmp/output/Withered_Foxy__valid-jobs",
      manifestPath: "/tmp/output/Withered_Foxy__valid-jobs/manifest.json",
      reportPath: "/tmp/output/Withered_Foxy__valid-jobs/conversion-report.html",
      artifacts: preparedConversion.preparation.artifacts.map((artifact) => ({
        ...artifact,
        relativePath: `projects/${artifact.fileName}`,
        path: `/tmp/output/Withered_Foxy__valid-jobs/projects/${artifact.fileName}`,
        byteSize: 1024,
        sha256: "partial-artifact-sha256",
        plateCount: artifact.targetPlateIds.length,
        validationStatus: "Passed",
      })),
      warnings: [],
      excludedSourceUnits: exclusions,
      warningsAcknowledged: false,
    };

    vi.mocked(open)
      .mockResolvedValueOnce("/tmp/Withered_Foxy.3mf")
      .mockResolvedValueOnce("/tmp/output");
    vi.mocked(invoke).mockImplementation(async (command) => {
      switch (command) {
        case "load_filament_library":
          return structuredClone(library);
        case "analyze_project":
          return structuredClone(nativePlan);
        case "inspect_conversion_capabilities":
          return structuredClone(capability);
        case "prepare_conversion":
          return structuredClone(preparedConversion);
        case "convert_project":
          return structuredClone(conversionResult);
        default:
          throw new Error(`Unexpected native command: ${command}`);
      }
    });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Choose 3MF" }));
    await waitFor(() =>
      expect(
        screen.getByRole("heading", { name: "Withered_Foxy.3mf" }),
      ).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Analyze Project" }));

    const approval = await screen.findByRole("checkbox", {
      name: /I understand that these source units will be excluded/i,
    });
    expect(screen.getByText("model-42")).toBeInTheDocument();
    const convertValidJobs = screen.getByRole("button", {
      name: /Convert Valid Jobs.*Review and acknowledge every excluded source unit first/i,
    });
    expect(convertValidJobs).toHaveAttribute("aria-disabled", "true");

    await waitFor(() =>
      expect(
        screen.getByRole("checkbox", {
          name: /I understand that these source units will be excluded/i,
        }),
      ).toBeEnabled(),
    );
    const currentApproval = screen.getByRole("checkbox", {
      name: /I understand that these source units will be excluded/i,
    });
    fireEvent.click(currentApproval);
    expect(currentApproval).toBeChecked();
    await waitFor(
      () => {
        const button = screen.getByText("Convert Valid Jobs").closest("button");
        expect(button).toHaveAttribute("aria-disabled", "false");
      },
      { timeout: 2_000 },
    );
    fireEvent.click(screen.getByText("Convert Valid Jobs").closest("button")!);

    const dialog = await screen.findByRole("dialog");
    expect(
      within(dialog).getByRole("heading", { name: "1 excluded source unit" }),
    ).toBeInTheDocument();
    expect(within(dialog).getByText("model-42")).toBeInTheDocument();
    expect(nativeCommandCalls("prepare_conversion")).toEqual([
      [
        "prepare_conversion",
        {
          sourcePath: "/tmp/Withered_Foxy.3mf",
          sourceSha256: nativePlan.summary.sourceHash,
          planFingerprint: "partial-plan-fingerprint",
          partialConversionApproval: {
            excludedSourceUnits: exclusions,
          },
          experimentalDialectApproval: null,
        },
      ],
    ]);

    fireEvent.click(
      within(dialog).getByRole("button", { name: "Convert projects" }),
    );
    await screen.findByRole("heading", { name: "Conversion complete" });
    expect(nativeCommandCalls("convert_project")).toEqual([
      [
        "convert_project",
        {
          preparationToken: "partial-preparation-token",
          destinationDirectory: "/tmp/output",
          acknowledgedWarnings: [],
        },
      ],
    ]);
    fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    const printRunButton = screen.getByRole("button", { name: "Print Run" });
    expect(printRunButton).toBeEnabled();
    fireEvent.click(printRunButton);
    expect(
      screen.getByRole("heading", {
        name: "Partial print run · 1 source unit excluded",
      }),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start print run" })).toBeEnabled();

    fireEvent.click(screen.getByRole("button", { name: "Print Plan" }));
    fireEvent.click(
      screen.getByRole("checkbox", { name: /Updated Ball Joints/i }),
    );
    expect(
      screen.getByRole("checkbox", {
        name: /I understand that these source units will be excluded/i,
      }),
    ).not.toBeChecked();
    expect(screen.getByRole("button", { name: "Print Run" })).toBeDisabled();
  });

  it("clears partial approval when an identical replan receives new backend evidence", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const nativePlan = createDemoPlan();
    nativePlan.planReady = false;
    nativePlan.blockingErrors = ["One isolated source unit cannot be scheduled."];
    nativePlan.omittedUnitCount = 1;
    nativePlan.partialConversion = {
      available: true,
      reason: "One source unit can be isolated from the valid print jobs.",
      exclusions: [
        {
          scopeId: "plate-8",
          planningUnitId: "plate-8-unit-2",
          sourcePlateId: 8,
          sourceUnitId: "model-42",
          reason: "No schedulable physical loadout is available.",
          errorIdentity: "partial-error-v1:abc",
        },
      ],
    };
    const library: FilamentLibraryDocument = {
      schemaVersion: 2,
      spools: nativePlan.spools,
    };
    let capabilityRevision = 0;

    vi.mocked(open).mockResolvedValue("/tmp/Withered_Foxy.3mf");
    vi.mocked(invoke).mockImplementation(async (command) => {
      switch (command) {
        case "load_filament_library":
          return structuredClone(library);
        case "analyze_project":
        case "replan_project":
          return structuredClone(nativePlan);
        case "inspect_conversion_capabilities":
          capabilityRevision += 1;
          return {
            available: true,
            reason: "All required writer adapters are ready.",
            planFingerprint: `backend-plan-fingerprint-${capabilityRevision}`,
            sourceDialectSupport: "Supported",
            experimentalDialectApprovalRequired: false,
            experimentalDialectFingerprint: null,
            adapters: [],
          } satisfies ConversionCapability;
        default:
          throw new Error(`Unexpected native command: ${command}`);
      }
    });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Choose 3MF" }));
    await screen.findByRole("heading", { name: "Withered_Foxy.3mf" });
    fireEvent.click(screen.getByRole("button", { name: "Analyze Project" }));
    const approval = await screen.findByRole("checkbox", {
      name: /I understand that these source units will be excluded/i,
    });
    await waitFor(() =>
      expect(nativeCommandCalls("inspect_conversion_capabilities")).toHaveLength(1),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", {
          name: /Convert Valid Jobs.*Review and acknowledge every excluded source unit first/i,
        }),
      ).toBeInTheDocument(),
    );
    fireEvent.click(approval);
    expect(approval).toBeChecked();

    fireEvent.click(screen.getByRole("button", { name: "Validate Choices" }));
    await waitFor(() => expect(nativeCommandCalls("replan_project")).toHaveLength(1));
    await waitFor(() =>
      expect(nativeCommandCalls("inspect_conversion_capabilities")).toHaveLength(2),
    );
    expect(capabilityRevision).toBe(2);
    expect(
      screen.getByRole("checkbox", {
        name: /I understand that these source units will be excluded/i,
      }),
    ).not.toBeChecked();
    expect(
      screen.getByRole("button", {
        name: /Convert Valid Jobs.*Review and acknowledge every excluded source unit first/i,
      }),
    ).toHaveAttribute("aria-disabled", "true");
  });

  it("validates choices explicitly with the local browser demo", async () => {
    await renderAnalyzedApp();

    const validateButton = screen.getByRole("button", { name: "Validate Choices" });
    expect(validateButton).toBeEnabled();
    fireEvent.click(validateButton);

    expect(screen.getByRole("button", { name: "Validating…" })).toBeDisabled();
    await waitFor(() =>
      expect(screen.getByRole("status")).toHaveTextContent(
        "Choices validated locally using demo data. All Direct mappings are assigned.",
      ),
    );
  });

  it("keeps browser demo A1 mini routing aligned with its fixed A1 queue", async () => {
    await renderAnalyzedApp();

    const a1MiniToggle = screen.getByRole("checkbox", {
      name: "Use Bambu Lab A1 mini for eligible mono parts",
    });
    expect(a1MiniToggle).toBeChecked();
    expect(a1MiniToggle).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Recalculate plan" })).not.toBeInTheDocument();
    expect(
      screen.getByText(
        /Browser demo routing is fixed to the included A1 mini preview/i,
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("table", { name: /A1 mini Plates in planned print order/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("table", { name: /U1 Plates in planned print order/i }),
    ).toBeInTheDocument();
  });

  it("keeps native A1 mini routing editable when the analyzed plan starts on U1", async () => {
    const nativePlan = createA1RoutingOffNativePlan();
    const replanned = createDemoPlan();
    await renderAnalyzedNativeApp(nativePlan, [replanned]);

    const a1MiniToggle = screen.getByRole("checkbox", {
      name: "Use Bambu Lab A1 mini for eligible mono parts",
    });
    expect(a1MiniToggle).not.toBeChecked();
    expect(a1MiniToggle).toBeEnabled();
    fireEvent.click(a1MiniToggle);
    expect(a1MiniToggle).toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: "Recalculate plan" }));

    await waitFor(() =>
      expect(nativeCommandCalls("replan_project")).toHaveLength(1),
    );
    expect(screen.getByText("A1 mini routing is applied to this plan.")).toBeInTheDocument();
  });

  it("persists a mono plate printer choice by stable source unit IDs", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const nativePlan = createDemoPlan();
    const replanned = structuredClone(nativePlan);
    vi.mocked(open).mockResolvedValue("/tmp/Withered_Foxy.3mf");
    mockNativeInvoke({ analyzePlan: nativePlan, replanPlans: [replanned] });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Choose 3MF" }));
    await waitFor(() =>
      expect(screen.getByRole("heading", { name: "Withered_Foxy.3mf" })).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Analyze Project" }));
    await waitFor(() =>
      expect(screen.getAllByText("Analysis complete")).not.toHaveLength(0),
    );

    fireEvent.click(
      screen.getByRole("checkbox", {
        name: "Use Bambu Lab A1 mini for eligible mono parts",
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: /Stand — Plate 07/i }));
    expect(
      screen.getByRole("group", { name: "Prepare this plate for" }),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("radio", { name: /Snapmaker U1/i }));
    expect(screen.getByRole("status")).toHaveTextContent(
      "Stand — Plate 07 is set to Snapmaker U1",
    );

    fireEvent.click(screen.getByRole("button", { name: "Recalculate plan" }));
    await waitFor(() => expect(nativeCommandCalls("replan_project")).toHaveLength(1));
    expect(nativeCommandCalls("replan_project")[0]).toEqual([
      "replan_project",
      expect.objectContaining({
        request: expect.objectContaining({
          a1MiniEnabled: true,
          unitPrinterOverrides: expect.arrayContaining([
            {
              sourceUnitId: "source-unit-7",
              preference: "u1",
            },
          ]),
        }),
      }),
    ]);
  });

  it("forces an existing in-stock spool onto a source color before offering creation", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const nativePlan = createDemoPlan();
    nativePlan.colorResolutions = [structuredClone(colorResolutionFixtures[1])];
    nativePlan.blockingErrors = ["One source color requires a decision."];
    nativePlan.omittedUnitCount = 1;
    nativePlan.planReady = false;
    const replanned = structuredClone(nativePlan);
    replanned.colorResolutions = [];
    replanned.blockingErrors = [];
    replanned.omittedUnitCount = 0;
    replanned.planReady = true;
    vi.mocked(open).mockResolvedValue("/tmp/Withered_Foxy.3mf");
    mockNativeInvoke({ analyzePlan: nativePlan, replanPlans: [replanned] });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Choose 3MF" }));
    await waitFor(() =>
      expect(screen.getByRole("heading", { name: "Withered_Foxy.3mf" })).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Analyze Project" }));
    await waitFor(() =>
      expect(screen.getByText("Source color #ADB1B2")).toBeInTheDocument(),
    );

    const card = screen.getByText("Source color #ADB1B2").closest("article");
    expect(card).not.toBeNull();
    fireEvent.click(
      within(card!).getByRole("button", { name: "Choose / add spool" }),
    );
    expect(within(card!).getByText("Choose an in-stock spool first")).toBeInTheDocument();
    fireEvent.click(
      within(card!).getByRole("radio", {
        name: /Black.*PolyLite PETG Black/i,
      }),
    );
    fireEvent.click(
      within(card!).getByRole("button", { name: "Use selected spool" }),
    );
    expect(screen.getByRole("status")).toHaveTextContent(
      "Black will replace #ADB1B2 using Direct Spools",
    );

    fireEvent.click(screen.getByRole("button", { name: /Recalculate Plan/i }));
    await waitFor(() => expect(nativeCommandCalls("replan_project")).toHaveLength(1));
    expect(nativeCommandCalls("replan_project")[0]).toEqual([
      "replan_project",
      expect.objectContaining({
        request: expect.objectContaining({
          scopeOverrides: expect.arrayContaining([
            expect.objectContaining({
              scopeId: "plate-6",
              strategy: "direct",
              assignments: expect.arrayContaining([
                expect.objectContaining({
                  requirementId: "plate-6-requirement-1",
                  spoolId: "black-petg",
                  allowMaterialSubstitution: false,
                }),
              ]),
            }),
          ]),
        }),
      }),
    ]);
  });

  it("clears A1 mini pending state when the draft returns to the applied off setting", async () => {
    await renderAnalyzedNativeApp(createA1RoutingOffNativePlan());

    const a1MiniToggle = screen.getByRole("checkbox", {
      name: "Use Bambu Lab A1 mini for eligible mono parts",
    });
    expect(screen.getByRole("button", { name: "Export JSON Plan" })).toBeEnabled();

    fireEvent.click(a1MiniToggle);
    expect(
      screen.getByRole("button", {
        name: /Export JSON Plan.*Validate pending edits first/i,
      }),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "Recalculate plan" }),
    ).toBeInTheDocument();

    fireEvent.click(a1MiniToggle);

    expect(a1MiniToggle).not.toBeChecked();
    expect(
      screen.queryByRole("button", { name: "Recalculate plan" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Export JSON Plan" })).toBeEnabled();
    expect(screen.getByText("Eligible mono parts stay on the U1.")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(
      "A1 mini routing restored to the applied setting. No recalculation is required.",
    );
  });

  it("compares A1 mini draft changes with the latest applied on setting", async () => {
    await renderAnalyzedNativeApp(createA1RoutingOffNativePlan(), [createDemoPlan()]);

    const a1MiniToggle = screen.getByRole("checkbox", {
      name: "Use Bambu Lab A1 mini for eligible mono parts",
    });
    fireEvent.click(a1MiniToggle);
    fireEvent.click(screen.getByRole("button", { name: "Recalculate plan" }));
    await waitFor(() =>
      expect(screen.getByText("A1 mini routing is applied to this plan.")).toBeInTheDocument(),
    );
    expect(screen.getByRole("button", { name: "Export JSON Plan" })).toBeEnabled();

    fireEvent.click(a1MiniToggle);
    expect(a1MiniToggle).not.toBeChecked();
    expect(screen.getByText(/Pending recalculation/i)).toBeInTheDocument();
    expect(
      screen.getByRole("button", {
        name: /Export JSON Plan.*Validate pending edits first/i,
      }),
    ).toBeDisabled();

    fireEvent.click(a1MiniToggle);

    expect(a1MiniToggle).toBeChecked();
    expect(
      screen.queryByRole("button", { name: "Recalculate plan" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Export JSON Plan" })).toBeEnabled();
    expect(screen.getByText("A1 mini routing is applied to this plan.")).toBeInTheDocument();
  });

  it("keeps fixed T1–T3 spools out of the current T4 choices", async () => {
    await renderAnalyzedApp();

    const currentT4 = screen.getByLabelText("Currently loaded T4 spool");
    expect(
      within(currentT4).queryByRole("option", { name: "Cyan · PLA" }),
    ).not.toBeInTheDocument();
    expect(
      within(currentT4).queryByRole("option", { name: "Magenta · PLA" }),
    ).not.toBeInTheDocument();
    expect(
      within(currentT4).queryByRole("option", { name: "Yellow · PLA" }),
    ).not.toBeInTheDocument();
    expect(
      within(currentT4).getByRole("option", { name: "Grey · PLA" }),
    ).toHaveValue("matte-grey");
  });

  it("disables export while edited choices are pending validation", async () => {
    await renderAnalyzedApp();

    expect(screen.getByRole("button", { name: "Export JSON Plan" })).toBeEnabled();
    fireEvent.click(screen.getByRole("radio", { name: /Direct Spools/i }));

    expect(
      screen.getByRole("button", { name: /Export JSON Plan.*Validate pending edits first/i }),
    ).toBeDisabled();
    expect(screen.getByRole("status")).toHaveTextContent(
      "Changes are pending validation.",
    );

    fireEvent.click(
      screen.getByRole("button", {
        name: /Recalculate Plan.*Apply pending printer, spool, or plan changes/i,
      }),
    );
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Export JSON Plan" })).toBeEnabled(),
    );
  });

  it("exports the browser plan as JSON", async () => {
    const originalCreateObjectUrl = URL.createObjectURL;
    const originalRevokeObjectUrl = URL.revokeObjectURL;
    const createObjectUrl = vi.fn<(object: Blob) => string>(
      () => "blob:u1-print-plan",
    );
    const revokeObjectUrl = vi.fn();
    Object.assign(URL, {
      createObjectURL: createObjectUrl,
      revokeObjectURL: revokeObjectUrl,
    });
    let downloadedFileName = "";
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (
      this: HTMLAnchorElement,
    ) {
      downloadedFileName = this.download;
    });

    try {
      await renderAnalyzedApp();
      fireEvent.click(screen.getByRole("button", { name: "Export JSON Plan" }));

      expect(createObjectUrl).toHaveBeenCalledOnce();
      const exportedBlob = createObjectUrl.mock.calls.at(0)?.at(0);
      expect(exportedBlob).toBeInstanceOf(Blob);
      expect(exportedBlob?.type).toBe("application/json");
      const exportedText = await new Promise<string>((resolve, reject) => {
        const reader = new FileReader();
        reader.addEventListener("load", () => resolve(String(reader.result)));
        reader.addEventListener("error", () => reject(reader.error));
        reader.readAsText(exportedBlob!);
      });
      expect(JSON.parse(exportedText)).toMatchObject({
        provenance: "browser-demo",
        validated: false,
      });
      expect(downloadedFileName).toBe("Withered_Foxy-print-plan.json");
      expect(revokeObjectUrl).toHaveBeenCalledWith("blob:u1-print-plan");
      expect(screen.getByRole("status")).toHaveTextContent(
        "Print plan exported as Withered_Foxy-print-plan.json.",
      );
    } finally {
      Object.assign(URL, {
        createObjectURL: originalCreateObjectUrl,
        revokeObjectURL: originalRevokeObjectUrl,
      });
    }
  });

  it("saves the native plan through the Tauri export command", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const nativePlan = createDemoPlan();
    nativePlan.batches = [
      { ...nativePlan.batches[0], id: "native-authoritative", label: "Native batch" },
    ];
    nativePlan.t4SwapCount = 7;
    nativePlan.a1SpoolChangeCount = 4;
    vi.mocked(open).mockResolvedValue("/tmp/Withered_Foxy.3mf");
    vi.mocked(save).mockResolvedValue("/tmp/Withered_Foxy-print-plan.json");
    mockNativeInvoke({ analyzePlan: nativePlan });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Choose 3MF" }));
    await waitFor(() =>
      expect(screen.getByRole("heading", { name: "Withered_Foxy.3mf" })).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Analyze Project" }));
    await waitFor(() =>
      expect(
        screen.getByRole("table", { name: /U1 Plates in planned print order/i }),
      ).toBeInTheDocument(),
    );
    expect(screen.getByText("Native batch")).toBeInTheDocument();
    const summary = screen.getByRole("region", { name: "Print plan summary" });
    expect(within(summary).getByText("7")).toBeInTheDocument();
    expect(within(summary).getByText("4")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Export JSON Plan" }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("export_plan", {
        destinationPath: "/tmp/Withered_Foxy-print-plan.json",
        contents: expect.any(String),
        sourcePath: "/tmp/Withered_Foxy.3mf",
        sourceHash: nativePlan.summary.sourceHash,
      }),
    );
    expect(save).toHaveBeenCalledWith({
      defaultPath: "Withered_Foxy-print-plan.json",
      filters: [{ name: "Print plan JSON", extensions: ["json"] }],
    });
    expect(screen.getByRole("status")).toHaveTextContent(
      "Print plan exported as Withered_Foxy-print-plan.json.",
    );
  });

  it("replans native projects with the selected alternative plate IDs", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const nativePlan = createDemoPlan();
    const replanned = createDemoPlan();
    replanned.alternativePlates = replanned.alternativePlates.map((plate) =>
      plate.id === 7 ? { ...plate, included: true } : plate,
    );
    vi.mocked(open).mockResolvedValue("/tmp/Withered_Foxy.3mf");
    mockNativeInvoke({ analyzePlan: nativePlan, replanPlans: [replanned] });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Choose 3MF" }));
    await waitFor(() =>
      expect(screen.getByRole("heading", { name: "Withered_Foxy.3mf" })).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Analyze Project" }));
    await waitFor(() =>
      expect(
        screen.getByRole("table", { name: /U1 Plates in planned print order/i }),
      ).toBeInTheDocument(),
    );

    const alternative = screen.getByRole("checkbox", {
      name: /Updated Ball Joints/i,
    });
    expect(alternative).not.toBeChecked();
    fireEvent.click(alternative);
    expect(alternative).toBeChecked();
    expect(
      screen.getByRole("button", {
        name: /Export JSON Plan.*Validate pending edits first/i,
      }),
    ).toBeDisabled();

    fireEvent.click(
      screen.getByRole("button", {
        name: /Recalculate Plan.*Apply pending printer, spool, or plan changes/i,
      }),
    );
    await waitFor(() =>
      expect(nativeCommandCalls("replan_project")).toContainEqual([
        "replan_project",
        {
        sourcePath: "/tmp/Withered_Foxy.3mf",
        request: expect.objectContaining({
          includedAlternativePlateIds: [7],
        }),
        },
      ]),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("checkbox", { name: /Updated Ball Joints/i }),
      ).toBeChecked(),
    );
  });

  it("shows safe color fallbacks and submits exact approvals on recalculation", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const nativePlan = createDemoPlan();
    nativePlan.colorResolutions = structuredClone(colorResolutionFixtures);
    nativePlan.blockingErrors = [
      "Two source colors require an explicit fallback decision.",
    ];
    nativePlan.omittedUnitCount = 2;
    nativePlan.planReady = false;
    nativePlan.plates = nativePlan.plates.map((plate) =>
      plate.scopeId === "plate-6" ? { ...plate, strategy: "direct" } : plate,
    );
    const replanned = structuredClone(nativePlan);
    replanned.colorResolutions = replanned.colorResolutions.map((resolution) => ({
      ...resolution,
      colorApproved: true,
      materialApproved: resolution.requiresMaterialSubstitution,
    }));
    replanned.blockingErrors = [];
    replanned.omittedUnitCount = 0;
    replanned.planReady = true;

    vi.mocked(open).mockResolvedValue("/tmp/Withered_Foxy.3mf");
    mockNativeInvoke({ analyzePlan: nativePlan, replanPlans: [replanned] });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Choose 3MF" }));
    await waitFor(() =>
      expect(screen.getByRole("heading", { name: "Withered_Foxy.3mf" })).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Analyze Project" }));
    await waitFor(() =>
      expect(
        screen.getByRole("heading", { name: "Color decisions required" }),
      ).toBeInTheDocument(),
    );

    const headCard = screen.getByText("Source color #8E9089").closest("article");
    const frameCard = screen.getByText("Source color #ADB1B2").closest("article");
    expect(headCard).not.toBeNull();
    expect(frameCard).not.toBeNull();
    expect(within(headCard!).getByLabelText("Source color")).toHaveTextContent(
      "#8E9089PLA",
    );
    expect(
      within(headCard!).getByLabelText("Nearest CMY+X alternative"),
    ).toHaveTextContent("#9199A4PLA");
    expect(headCard).toHaveTextContent("ΔE00 10.1");
    expect(headCard).toHaveTextContent("Required T4 Panchroma Translucent Grey");

    fireEvent.click(
      within(headCard!).getByRole("button", { name: "Choose / add spool" }),
    );
    fireEvent.click(within(headCard!).getByText("Add new spool to library"));
    expect(within(headCard!).getByLabelText(/HEX color/i)).toHaveValue("#8E9089");
    expect(within(headCard!).getByRole("radio", { name: "PLA" })).toBeChecked();

    fireEvent.click(
      screen.getByRole("button", {
        name: "Accept all color approximations (1)",
      }),
    );
    expect(within(headCard!).getByText("Accepted")).toBeInTheDocument();
    expect(within(frameCard!).getByText("Needs approval")).toBeInTheDocument();

    const materialButton = within(frameCard!).getByRole("button", {
      name: "Accept color and material change",
    });
    expect(materialButton).toBeDisabled();
    expect(frameCard).toHaveTextContent("Material change: PETG → PLA");
    fireEvent.click(
      within(frameCard!).getByRole("checkbox", {
        name: "I understand the mechanical-property risk.",
      }),
    );
    expect(materialButton).toBeEnabled();
    fireEvent.click(materialButton);

    const recalculate = screen.getByRole("button", {
      name: /Recalculate Plan.*Apply approved color decisions/i,
    });
    fireEvent.click(recalculate);
    await waitFor(() =>
      expect(nativeCommandCalls("replan_project")).toHaveLength(1),
    );
    expect(nativeCommandCalls("replan_project")[0]).toEqual([
      "replan_project",
      {
        sourcePath: "/tmp/Withered_Foxy.3mf",
        request: expect.objectContaining({
          scopeOverrides: expect.arrayContaining([
            expect.objectContaining({
              scopeId: "plate-1",
              strategy: "cmyx",
              approvedColorFallbacks: [
                {
                  requirementId: "plate-1-requirement-2",
                  candidateId: "head-grey-cmy-grey-v1",
                },
              ],
            }),
            expect.objectContaining({
              scopeId: "plate-6",
              strategy: "cmyx",
              approvedColorFallbacks: [
                {
                  requirementId: "plate-6-requirement-1",
                  candidateId: "frame-petg-to-pla-grey-v1",
                },
              ],
              materialSubstitutions: [
                {
                  requirementId: "plate-6-requirement-1",
                  candidateId: "frame-petg-to-pla-grey-v1",
                  sourceMaterial: "PETG",
                  targetMaterial: "PLA",
                  acknowledged: true,
                },
              ],
            }),
          ]),
        }),
      },
    ]);
  });

  it("clears accepted fallback candidates when a spool changes inventory", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const nativePlan = createDemoPlan();
    nativePlan.colorResolutions = structuredClone(colorResolutionFixtures);
    const replanned = structuredClone(nativePlan);
    vi.mocked(open).mockResolvedValue("/tmp/Withered_Foxy.3mf");
    mockNativeInvoke({ analyzePlan: nativePlan, replanPlans: [replanned] });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Choose 3MF" }));
    await waitFor(() =>
      expect(screen.getByRole("heading", { name: "Withered_Foxy.3mf" })).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Analyze Project" }));
    await waitFor(() =>
      expect(
        screen.getByRole("heading", { name: "Color decisions required" }),
      ).toBeInTheDocument(),
    );

    const headCard = screen.getByText("Source color #8E9089").closest("article");
    expect(headCard).not.toBeNull();
    fireEvent.click(
      within(headCard!).getByRole("button", { name: "Accept approximation" }),
    );
    expect(within(headCard!).getByText("Accepted")).toBeInTheDocument();

    fireEvent.click(
      within(headCard!).getByRole("button", { name: "Choose / add spool" }),
    );
    fireEvent.change(within(headCard!).getByLabelText(/Spool name/i), {
      target: { value: "Dedicated Head Grey" },
    });
    fireEvent.click(within(headCard!).getByRole("button", { name: "Add spool" }));

    expect(within(headCard!).getByText("Needs approval")).toBeInTheDocument();
    await waitFor(() =>
      expect(screen.getByRole("status")).toHaveTextContent(
        "added to the filament library",
      ),
    );

    fireEvent.click(
      screen.getByRole("button", {
        name: /Recalculate Plan.*Apply approved color decisions/i,
      }),
    );
    await waitFor(() =>
      expect(nativeCommandCalls("replan_project")).toHaveLength(1),
    );
    expect(nativeCommandCalls("replan_project")[0]).toEqual([
      "replan_project",
      {
        sourcePath: "/tmp/Withered_Foxy.3mf",
        request: expect.objectContaining({
          confirmedSpools: expect.arrayContaining([
            expect.objectContaining({
              name: "Dedicated Head Grey",
              colorName: "Match #8E9089",
              hex: "#8E9089",
            }),
          ]),
          scopeOverrides: expect.arrayContaining([
            expect.objectContaining({
              scopeId: "plate-1",
              approvedColorFallbacks: [],
              materialSubstitutions: [],
            }),
          ]),
        }),
      },
    ]);
  });

  it("keeps a Direct scope selection through repeated native A1 mini replans", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
    const nativePlan = createDemoPlan();
    nativePlan.scopeSelections = nativePlan.scopeSelections.map((selection) =>
      selection.scopeId === "plate-8"
        ? {
            scopeId: "plate-8",
            strategy: "direct",
            assignments: [
              {
                requirementId: "nameplate-black",
                spoolId: "black-petg",
                toolhead: "T4",
              },
            ],
            approvedColorFallbacks: [],
            materialSubstitutions: [],
          }
        : selection,
    );
    const replanned = structuredClone(nativePlan);
    vi.mocked(open).mockResolvedValue("/tmp/Withered_Foxy.3mf");
    mockNativeInvoke({
      analyzePlan: nativePlan,
      replanPlans: [replanned, replanned],
    });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Choose 3MF" }));
    await waitFor(() =>
      expect(screen.getByRole("heading", { name: "Withered_Foxy.3mf" })).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Analyze Project" }));
    await waitFor(() =>
      expect(
        screen.getByRole("table", { name: /U1 Plates in planned print order/i }),
      ).toBeInTheDocument(),
    );

    fireEvent.click(
      screen.getByRole("checkbox", {
        name: "Use Bambu Lab A1 mini for eligible mono parts",
      }),
    );
    fireEvent.click(
      screen.getByRole("button", {
        name: /Recalculate Plan.*Apply pending printer, spool, or plan changes/i,
      }),
    );
    await waitFor(() =>
      expect(nativeCommandCalls("replan_project")).toHaveLength(1),
    );
    fireEvent.click(screen.getByRole("button", { name: "Validate Choices" }));
    await waitFor(() =>
      expect(nativeCommandCalls("replan_project")).toHaveLength(2),
    );

    const expectedSelection = {
      scopeId: "plate-8",
      strategy: "direct",
      assignments: [
        {
          requirementId: "nameplate-black",
          spoolId: "black-petg",
          toolhead: "T4",
        },
      ],
      approvedColorFallbacks: [],
      materialSubstitutions: [],
    };
    for (const call of nativeCommandCalls("replan_project")) {
      expect(call).toEqual([
        "replan_project",
        {
          sourcePath: "/tmp/Withered_Foxy.3mf",
          request: expect.objectContaining({
            a1MiniEnabled: true,
            scopeOverrides: expect.arrayContaining([expectedSelection]),
          }),
        },
      ]);
    }
  });
});
