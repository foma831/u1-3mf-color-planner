// @vitest-environment jsdom

import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { afterEach, describe, expect, it, vi } from "vitest";

import type {
  CmyxCalibrationLibraryDocument,
  CmyxCalibrationMeasurementInput,
} from "../types";
import {
  CMYX_CALIBRATION_LEGACY_STORAGE_KEY,
  CMYX_CALIBRATION_STORAGE_KEY,
  createRecommendedCmyxCalibrationProject,
  deleteCmyxCalibrationRecord,
  loadCmyxCalibrationLibrary,
  setCmyxCalibrationGeometryContext,
  upsertCmyxCalibrationMeasurement,
  validateCmyxCalibrationProject,
} from "./color-calibration";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn() }));

const artifactHash = "a".repeat(64);
const manifestHash = "b".repeat(64);
const generatedProject = {
  projectId: "grey-calibration-chart",
  chartMode: "full" as const,
  path: "/tmp/grey-calibration-chart.3mf",
  fileName: "grey-calibration-chart.3mf",
  byteSize: 12345,
  artifactSha256: artifactHash,
  manifestPath: "Metadata/u1_calibration_manifest.json",
  manifestSha256: manifestHash,
  swatchCount: 26,
  productionQualified: false,
  validation: {
    valid: true,
    projectId: "grey-calibration-chart",
    manifestSha256: manifestHash,
    swatchCount: 26,
    fullSpectrum: {
      adapterId: "snapmaker-orca/2.3.5/u1-0.4-full-spectrum",
      valid: true,
      physicalFilamentCount: 4,
      virtualFilamentCount: 26,
      usedFilamentIds: [1, 2, 3, 4],
      issues: [],
    },
    issues: [],
  },
  warnings: ["Qualification candidate only."],
  nextSteps: ["Open and slice it in Snapmaker Orca."],
};

const measurement: CmyxCalibrationMeasurementInput = {
  id: " grey-flat-01 ",
  loadout: {
    t1CalibrationId: "panchroma-cyan",
    t2CalibrationId: "panchroma-magenta",
    t3CalibrationId: "panchroma-yellow",
    t4CalibrationId: "matte-grey",
  },
  recipe: {
    mode: "ratio",
    components: [
      { slot: 1, weight: 2 },
      { slot: 4, weight: 6 },
    ],
  },
  measuredOutputHex: "#a4a8aa",
  geometryContext: {
    orientation: "flat",
    geometryClass: "calibration_swatch",
  },
  provenance: {
    measuredAt: "2026-08-01T18:30:00Z",
    method: "instrument_srgb",
    instrumentReference: "Colorimeter C-17",
    operatorNotes: "Controlled D65 lighting.",
  },
  physicalMeasurementConfirmed: true,
};

const nativeLibrary: CmyxCalibrationLibraryDocument = {
  schemaVersion: 2,
  planningGeometryContext: {
    orientation: "flat",
    geometryClass: "calibration_swatch",
  },
  records: [
    {
      id: "grey-flat-01",
      loadout: measurement.loadout,
      recipe: measurement.recipe,
      measuredOutputHex: "#A4A8AA",
      provenance: {
        measuredAt: "2026-08-01T18:30:00.000Z",
        method: "instrument_srgb",
        instrumentReference: "Colorimeter C-17",
        operatorNotes: "Controlled D65 lighting.",
      },
      context: {
        loadoutFingerprint:
          "T1:panchroma-cyan|T2:panchroma-magenta|T3:panchroma-yellow|T4:matte-grey",
        printerProfile: {
          printerModel: "Snapmaker U1",
          printerVariant: "0.4 mm nozzle",
          profileId: "u1-full-spectrum-0.08",
          profileFingerprint: "qualified-profile-v1",
        },
        nozzleDiameterMicrons: 400,
        plateLayerHeightMicrons: 80,
        subdivisionPolicy: "subdivide_mix_layer",
        subdivisionFactor: 4,
        effectiveSublayerHeightMicrons: 20,
        processFingerprint: "qualified-process-v1",
        orientation: "flat",
        geometryClass: "calibration_swatch",
      },
    },
  ],
};

afterEach(() => {
  vi.resetAllMocks();
  window.localStorage.clear();
  delete (window as Window & { __TAURI_INTERNALS__?: unknown })
    .__TAURI_INTERNALS__;
});

describe("CMY+X calibration persistence", () => {
  it("starts browser demo storage with Unknown planning geometry", async () => {
    await expect(loadCmyxCalibrationLibrary()).resolves.toEqual({
      schemaVersion: 2,
      planningGeometryContext: {
        orientation: "unknown",
        geometryClass: "unknown",
      },
      records: [],
    });
    expect(invoke).not.toHaveBeenCalled();
  });

  it("round-trips browser measurements, geometry, and deletion without claiming native context", async () => {
    const savedMeasurement = await upsertCmyxCalibrationMeasurement(measurement);
    expect(savedMeasurement.records[0]).toMatchObject({
      id: "grey-flat-01",
      measuredOutputHex: "#A4A8AA",
      context: {
        processFingerprint: "browser-demo-not-for-native-planning",
        printerProfile: { printerModel: "Browser demo only" },
      },
      provenance: {
        measuredAt: "2026-08-01T18:30:00.000Z",
        method: "instrument_srgb",
      },
    });

    const withGeometry = await setCmyxCalibrationGeometryContext({
      orientation: "upright",
      geometryClass: "thin_wall",
    });
    expect(withGeometry.planningGeometryContext).toEqual({
      orientation: "upright",
      geometryClass: "thin_wall",
    });

    const deleted = await deleteCmyxCalibrationRecord("grey-flat-01");
    expect(deleted.records).toEqual([]);
    expect(
      JSON.parse(
        window.localStorage.getItem(CMYX_CALIBRATION_STORAGE_KEY) ?? "null",
      ),
    ).toEqual(deleted);
  });

  it("rejects corrupt browser JSON and leaves it untouched", async () => {
    window.localStorage.setItem(CMYX_CALIBRATION_STORAGE_KEY, "not json");

    await expect(loadCmyxCalibrationLibrary()).rejects.toThrow(
      "not valid JSON",
    );
    expect(window.localStorage.getItem(CMYX_CALIBRATION_STORAGE_KEY)).toBe(
      "not json",
    );
  });

  it("migrates browser schema v1 as legacy unverified without deleting recovery data", async () => {
    const legacy = {
      schemaVersion: 1,
      planningGeometryContext: nativeLibrary.planningGeometryContext,
      records: nativeLibrary.records.map(({ provenance: _provenance, ...record }) => record),
    };
    const serialized = JSON.stringify(legacy);
    window.localStorage.setItem(CMYX_CALIBRATION_LEGACY_STORAGE_KEY, serialized);

    const migrated = await loadCmyxCalibrationLibrary();
    expect(migrated.schemaVersion).toBe(2);
    expect(migrated.records[0].provenance).toEqual({
      measuredAt: null,
      method: "legacy_unverified",
      instrumentReference: null,
      operatorNotes: null,
    });
    expect(window.localStorage.getItem(CMYX_CALIBRATION_LEGACY_STORAGE_KEY)).toBe(serialized);
    expect(window.localStorage.getItem(CMYX_CALIBRATION_STORAGE_KEY)).not.toBeNull();
  });

  it("rejects mixed known and Unknown planning geometry", async () => {
    await expect(
      setCmyxCalibrationGeometryContext({
        orientation: "unknown",
        geometryClass: "thin_wall",
      }),
    ).rejects.toThrow("valid planning geometry preset");
  });

  it("rejects Unknown measurement geometry and invalid recipe counts", async () => {
    await expect(
      upsertCmyxCalibrationMeasurement({
        ...measurement,
        geometryContext: {
          orientation: "unknown",
          geometryClass: "unknown",
        },
      }),
    ).rejects.toThrow("measurement is incomplete");
    await expect(
      upsertCmyxCalibrationMeasurement({
        ...measurement,
        recipe: {
          mode: "gradient",
          components: [
            { slot: 1, weight: 1 },
            { slot: 2, weight: 1 },
            { slot: 3, weight: 1 },
          ],
        },
      }),
    ).rejects.toThrow("measurement is incomplete");
  });

  it("rejects missing measurement provenance and nominal-preview acknowledgments", async () => {
    await expect(
      upsertCmyxCalibrationMeasurement({
        ...measurement,
        provenance: {
          ...measurement.provenance,
          measuredAt: "not-a-date",
        },
      }),
    ).rejects.toThrow("lacks physical measurement provenance");
    await expect(
      upsertCmyxCalibrationMeasurement({
        ...measurement,
        provenance: {
          ...measurement.provenance,
          measuredAt: "2026-02-30T12:00:00Z",
        },
      }),
    ).rejects.toThrow("lacks physical measurement provenance");
    await expect(
      upsertCmyxCalibrationMeasurement({
        ...measurement,
        physicalMeasurementConfirmed: false,
      } as unknown as CmyxCalibrationMeasurementInput),
    ).rejects.toThrow("lacks physical measurement provenance");
  });

  it("uses exact native command arguments and never supplies measurement context", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ =
      {};
    vi.mocked(invoke).mockResolvedValue(nativeLibrary);

    await expect(loadCmyxCalibrationLibrary()).resolves.toEqual(nativeLibrary);
    await expect(
      upsertCmyxCalibrationMeasurement(measurement),
    ).resolves.toEqual(nativeLibrary);
    await expect(
      setCmyxCalibrationGeometryContext({
        orientation: "flat",
        geometryClass: "top_surface",
      }),
    ).resolves.toEqual(nativeLibrary);
    await expect(deleteCmyxCalibrationRecord("grey-flat-01")).resolves.toEqual(
      nativeLibrary,
    );

    expect(invoke).toHaveBeenNthCalledWith(1, "load_cmyx_calibration_library");
    expect(invoke).toHaveBeenNthCalledWith(
      2,
      "upsert_cmyx_calibration_measurement",
      {
        measurement: {
          ...measurement,
          id: "grey-flat-01",
          measuredOutputHex: "#A4A8AA",
          provenance: {
            ...measurement.provenance,
            measuredAt: "2026-08-01T18:30:00.000Z",
          },
          physicalMeasurementConfirmed: true,
        },
      },
    );
    const sentMeasurement = vi.mocked(invoke).mock.calls[1][1] as {
      measurement: Record<string, unknown>;
    };
    expect(sentMeasurement.measurement).not.toHaveProperty("context");
    expect(invoke).toHaveBeenNthCalledWith(
      3,
      "set_cmyx_calibration_geometry_context",
      {
        geometryContext: {
          orientation: "flat",
          geometryClass: "top_surface",
        },
      },
    );
    expect(invoke).toHaveBeenNthCalledWith(
      4,
      "delete_cmyx_calibration_record",
      { recordId: "grey-flat-01" },
    );
  });

  it("rejects invalid native response data", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ =
      {};
    vi.mocked(invoke).mockResolvedValue({
      ...nativeLibrary,
      planningGeometryContext: {
        orientation: "unknown",
        geometryClass: "thin_wall",
      },
    });

    await expect(loadCmyxCalibrationLibrary()).rejects.toThrow(
      "native CMY+X calibration library returned invalid data",
    );
  });

  it("never fabricates a calibration project in browser demo mode", async () => {
    await expect(
      createRecommendedCmyxCalibrationProject({
        projectId: "grey-calibration-chart",
        chartMode: "full",
        spoolIds: ["cyan", "magenta", "yellow", "grey"],
      }),
    ).rejects.toThrow("only in the native desktop app");
    expect(save).not.toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("chooses a new 3MF path and sends the exact T1-T4 loadout to native generation", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ =
      {};
    vi.mocked(save).mockResolvedValue("/tmp/grey-calibration-chart.3mf");
    vi.mocked(invoke).mockResolvedValue(generatedProject);
    const spoolIds: [string, string, string, string] = [
      "panchroma-translucent-cyan",
      "panchroma-translucent-magenta",
      "panchroma-translucent-yellow",
      "panchroma-translucent-grey",
    ];

    await expect(
      createRecommendedCmyxCalibrationProject({
        projectId: " grey-calibration-chart ",
        chartMode: "full",
        spoolIds,
      }),
    ).resolves.toEqual(generatedProject);
    expect(save).toHaveBeenCalledWith({
      title: "Save CMY+X calibration qualification candidate",
      defaultPath: "grey-calibration-chart.3mf",
      filters: [{ name: "3MF calibration project", extensions: ["3mf"] }],
    });
    expect(invoke).toHaveBeenCalledWith("build_cmyx_calibration_project", {
      request: {
        projectId: "grey-calibration-chart",
        chartMode: "full",
        spoolIds,
        destinationPath: "/tmp/grey-calibration-chart.3mf",
      },
    });
  });

  it("treats save cancellation as no mutation and rejects forged qualification claims", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ =
      {};
    vi.mocked(save).mockResolvedValue(null);
    const input = {
      projectId: "grey-calibration-chart",
      chartMode: "quick" as const,
      spoolIds: ["cyan", "magenta", "yellow", "grey"] as [
        string,
        string,
        string,
        string,
      ],
    };
    await expect(createRecommendedCmyxCalibrationProject(input)).resolves.toBeNull();
    expect(invoke).not.toHaveBeenCalled();

    vi.mocked(save).mockResolvedValue("/tmp/grey-calibration-chart.3mf");
    vi.mocked(invoke).mockResolvedValue({
      ...generatedProject,
      productionQualified: true,
    });
    await expect(createRecommendedCmyxCalibrationProject(input)).rejects.toThrow(
      "invalid artifact data",
    );

    vi.mocked(invoke).mockResolvedValue(generatedProject);
    await expect(createRecommendedCmyxCalibrationProject(input)).rejects.toThrow(
      "does not match the requested chart mode",
    );
  });

  it("validates an existing native calibration candidate without browser fallback", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ =
      {};
    vi.mocked(invoke).mockResolvedValue(generatedProject.validation);

    await expect(
      validateCmyxCalibrationProject(" /tmp/grey-calibration-chart.3mf "),
    ).resolves.toEqual(generatedProject.validation);
    expect(invoke).toHaveBeenCalledWith("validate_cmyx_calibration_project", {
      path: "/tmp/grey-calibration-chart.3mf",
    });
  });

  it("preserves a structurally valid negative validation report and its issues", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ =
      {};
    const rejected = {
      ...generatedProject.validation,
      valid: false,
      fullSpectrum: {
        ...generatedProject.validation.fullSpectrum,
        valid: false,
      },
      issues: ["Manifest checksum mismatch."],
    };
    vi.mocked(invoke).mockResolvedValue(rejected);

    await expect(validateCmyxCalibrationProject("/tmp/bad.3mf")).resolves.toEqual(
      rejected,
    );
  });
});
