// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { demoSpools } from "../data/mock-plan";
import type {
  CmyxCalibrationLibraryDocument,
  CmyxCalibrationMeasurementInput,
  PhysicalSpool,
} from "../types";
import { ColorCalibration } from "./ColorCalibration";

const emptyLibrary: CmyxCalibrationLibraryDocument = {
  schemaVersion: 2,
  planningGeometryContext: {
    orientation: "unknown",
    geometryClass: "unknown",
  },
  records: [],
};

const measuredLibrary: CmyxCalibrationLibraryDocument = {
  schemaVersion: 2,
  planningGeometryContext: {
    orientation: "flat",
    geometryClass: "calibration_swatch",
  },
  records: [
    {
      id: "grey-flat-ratio-01",
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
      measuredOutputHex: "#A4A8AA",
      provenance: {
        measuredAt: "2026-08-01T18:30:00.000Z",
        method: "instrument_srgb",
        instrumentReference: "Colorimeter C-17",
        operatorNotes: "Measured after a 24-hour cure.",
      },
      context: {
        loadoutFingerprint:
          "T1:panchroma-cyan|T2:panchroma-magenta|T3:panchroma-yellow|T4:matte-grey",
        printerProfile: {
          printerModel: "Snapmaker U1",
          printerVariant: "0.4 mm nozzle",
          profileId: "u1-full-spectrum-0.08",
          profileFingerprint: "profile-v1",
        },
        nozzleDiameterMicrons: 400,
        plateLayerHeightMicrons: 80,
        subdivisionPolicy: "subdivide_mix_layer",
        subdivisionFactor: 4,
        effectiveSublayerHeightMicrons: 20,
        processFingerprint: "process-v1",
        orientation: "flat",
        geometryClass: "calibration_swatch",
      },
    },
  ],
};

function renderCalibration(
  overrides: Partial<React.ComponentProps<typeof ColorCalibration>> = {},
) {
  const props: React.ComponentProps<typeof ColorCalibration> = {
    library: emptyLibrary,
    spools: demoSpools,
    isNative: false,
    isInventoryReady: true,
    isSaving: false,
    isGeneratingProject: false,
    onGenerateProject: vi.fn().mockResolvedValue(null),
    onSaveMeasurement: vi.fn().mockResolvedValue(true),
    onDeleteRecord: vi.fn().mockResolvedValue(true),
    onSetPlanningGeometry: vi.fn().mockResolvedValue(true),
    ...overrides,
  };
  render(<ColorCalibration {...props} />);
  return props;
}

function completeMeasurementEvidence(hex = "#A0A1A2") {
  fireEvent.change(screen.getByLabelText("Measurement date and time"), {
    target: { value: "2026-08-01T11:30:00" },
  });
  fireEvent.click(
    screen.getByRole("radio", { name: /Instrument sRGB/i }),
  );
  fireEvent.change(screen.getByLabelText("Measured sRGB HEX"), {
    target: { value: hex },
  });
  fireEvent.click(
    screen.getByRole("checkbox", { name: /I used a physical printed swatch/i }),
  );
}

afterEach(cleanup);

describe("ColorCalibration", () => {
  it("explains qualification boundaries and shows an explicit Unknown reuse state", () => {
    renderCalibration();

    expect(
      screen.getByRole("heading", { name: "CMY+X Color Calibration" }),
    ).toBeInTheDocument();
    expect(screen.getByText(/chart generation and physical measurement are separate/i)).toBeInTheDocument();
    expect(screen.getByText(/nominal library colors are not measured output/i)).toBeInTheDocument();
    expect(screen.getByText(/saved only in this browser profile/i)).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Choose output and generate 3MF" }),
    ).toBeDisabled();
    expect(screen.getByText(/browser mode never creates or pretends to create/i)).toBeInTheDocument();
    expect(screen.getByText("Reuse disabled")).toBeInTheDocument();
    expect(screen.getByText(/planning will not reuse any measured sample/i)).toBeInTheDocument();
    expect(screen.getByText("Panchroma Translucent Cyan")).toBeInTheDocument();
    expect(screen.getByText("Panchroma Translucent Magenta")).toBeInTheDocument();
    expect(screen.getByText("Panchroma Translucent Yellow")).toBeInTheDocument();
    expect(
      within(screen.getByLabelText("T4 available PLA spool")).getByRole(
        "option",
        { name: /Grey.*#9199A4/i },
      ),
    ).toBeInTheDocument();
  });

  it("generates the recommended 26-swatch native candidate and exposes both hashes and qualification boundary", async () => {
    const artifactSha256 = "a".repeat(64);
    const manifestSha256 = "b".repeat(64);
    const onGenerateProject = vi.fn().mockResolvedValue({
      projectId: "cmyx-calibration-chart",
      chartMode: "full",
      path: "/tmp/cmyx-calibration-chart.3mf",
      fileName: "cmyx-calibration-chart.3mf",
      byteSize: 12000,
      artifactSha256,
      manifestPath: "Metadata/u1_calibration_manifest.json",
      manifestSha256,
      swatchCount: 26,
      productionQualified: false,
      validation: {
        valid: true,
        projectId: "cmyx-calibration-chart",
        manifestSha256,
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
      nextSteps: ["Open, slice, save, close, reopen, and reslice."],
    });
    renderCalibration({ isNative: true, onGenerateProject });

    fireEvent.change(screen.getByLabelText("T4 available PLA spool"), {
      target: { value: "matte-grey" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Choose output and generate 3MF" }),
    );

    await waitFor(() =>
      expect(onGenerateProject).toHaveBeenCalledWith({
        projectId: "cmyx-calibration-chart",
        chartMode: "full",
        spoolIds: [
          "panchroma-cyan",
          "panchroma-magenta",
          "panchroma-yellow",
          "matte-grey",
        ],
      }),
    );
    expect(await screen.findByText("Validated qualification candidate")).toBeInTheDocument();
    expect(screen.getByText("Native validation passed")).toBeInTheDocument();
    expect(screen.getByText(artifactSha256)).toBeInTheDocument();
    expect(screen.getByText(manifestSha256)).toBeInTheDocument();
    expect(screen.getByText("productionQualified=false")).toBeInTheDocument();
    expect(screen.getByText(/Open, slice, save, close, reopen/i)).toBeInTheDocument();
  });

  it("offers a Quick 10-swatch chart as an explicit accessible alternative", async () => {
    const onGenerateProject = vi.fn().mockResolvedValue(null);
    renderCalibration({ isNative: true, onGenerateProject });

    expect(
      screen.getByRole("radio", { name: /Full · 26 swatches/i }),
    ).toBeChecked();
    fireEvent.click(
      screen.getByRole("radio", { name: /Quick · 10 swatches/i }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Choose output and generate 3MF" }),
    );

    await waitFor(() =>
      expect(onGenerateProject).toHaveBeenCalledWith(
        expect.objectContaining({ chartMode: "quick" }),
      ),
    );
  });

  it("keeps native chart generation disabled until authoritative inventory loads", () => {
    renderCalibration({ isNative: true, isInventoryReady: false });

    expect(
      screen.getByRole("button", { name: "Choose output and generate 3MF" }),
    ).toBeDisabled();
    expect(screen.getByText(/authoritative inventory has loaded/i)).toBeInTheDocument();
  });

  it("constrains component counts for each recipe mode", () => {
    renderCalibration();
    const count = screen.getByLabelText("Recipe components");

    expect(within(count).getAllByRole("option").map((option) => option.textContent)).toEqual([
      "2",
      "3",
    ]);

    fireEvent.click(screen.getByRole("radio", { name: /Gradient/i }));
    expect(within(count).getAllByRole("option")).toHaveLength(1);
    expect(count).toHaveValue("2");

    fireEvent.click(screen.getByRole("radio", { name: /Cycle/i }));
    expect(within(count).getAllByRole("option")).toHaveLength(3);
    fireEvent.change(count, { target: { value: "4" } });
    expect(screen.getAllByLabelText(/Component \d slot/)).toHaveLength(4);
    expect(screen.getAllByLabelText("Relative weight")).toHaveLength(4);
  });

  it("submits only measurement input for the exact loadout and recipe", async () => {
    const onSaveMeasurement = vi.fn().mockResolvedValue(true);
    renderCalibration({ onSaveMeasurement });

    fireEvent.change(screen.getByLabelText("Record ID"), {
      target: { value: "grey-coupon-02" },
    });
    completeMeasurementEvidence();
    fireEvent.click(screen.getByRole("button", { name: "Save measured sample" }));

    await waitFor(() => expect(onSaveMeasurement).toHaveBeenCalledTimes(1));
    const submitted = onSaveMeasurement.mock.calls[0][0] as CmyxCalibrationMeasurementInput &
      Record<string, unknown>;
    expect(submitted).toMatchObject({
      id: "grey-coupon-02",
      loadout: {
        t1CalibrationId: "panchroma-cyan",
        t2CalibrationId: "panchroma-magenta",
        t3CalibrationId: "panchroma-yellow",
      },
      recipe: {
        mode: "ratio",
        components: [
          { slot: 1, weight: 1 },
          { slot: 4, weight: 1 },
        ],
      },
      measuredOutputHex: "#A0A1A2",
      geometryContext: {
        orientation: "flat",
        geometryClass: "calibration_swatch",
      },
      provenance: {
        measuredAt: expect.any(String),
        method: "instrument_srgb",
        instrumentReference: null,
        operatorNotes: null,
      },
      physicalMeasurementConfirmed: true,
    });
    expect(submitted).not.toHaveProperty("context");
    expect(screen.getByText(/saved as a measured CMY\+X sample/i)).toBeInTheDocument();
  });

  it("binds a new measurement to physical batch calibration identities", async () => {
    const spools = demoSpools.map((spool) => ({
      ...spool,
      calibrationIdentity: `batch:${spool.id}`,
    }));
    const onSaveMeasurement = vi.fn().mockResolvedValue(true);
    renderCalibration({ spools, onSaveMeasurement });

    fireEvent.change(screen.getByLabelText("Record ID"), {
      target: { value: "batch-bound-grey" },
    });
    completeMeasurementEvidence();
    fireEvent.click(screen.getByRole("button", { name: "Save measured sample" }));

    await waitFor(() => expect(onSaveMeasurement).toHaveBeenCalledTimes(1));
    expect(onSaveMeasurement).toHaveBeenCalledWith(
      expect.objectContaining({
        loadout: {
          t1CalibrationId: "batch:panchroma-cyan",
          t2CalibrationId: "batch:panchroma-magenta",
          t3CalibrationId: "batch:panchroma-yellow",
          t4CalibrationId: "batch:graphite",
        },
      }),
    );
  });

  it("reports duplicate slots before calling persistence", async () => {
    const onSaveMeasurement = vi.fn().mockResolvedValue(true);
    renderCalibration({ onSaveMeasurement });
    fireEvent.change(screen.getByLabelText("Record ID"), {
      target: { value: "duplicate-slots" },
    });
    fireEvent.change(screen.getByLabelText("Component 2 slot"), {
      target: { value: "1" },
    });
    completeMeasurementEvidence();
    fireEvent.click(screen.getByRole("button", { name: "Save measured sample" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Each recipe component must use a different",
    );
    expect(onSaveMeasurement).not.toHaveBeenCalled();
  });

  it("persists planning geometry and exposes the Unknown safety choice", async () => {
    const onSetPlanningGeometry = vi.fn().mockResolvedValue(true);
    renderCalibration({ onSetPlanningGeometry });
    fireEvent.change(screen.getByLabelText("Planning geometry preset"), {
      target: { value: "upright-wall" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save planning geometry" }));

    await waitFor(() =>
      expect(onSetPlanningGeometry).toHaveBeenCalledWith({
        orientation: "upright",
        geometryClass: "thin_wall",
      }),
    );
    expect(screen.getByRole("status")).toHaveTextContent(
      "Upright thin wall saved",
    );
  });

  it("lists measured swatches, loadout, recipe, geometry, and confirms deletion", async () => {
    const onDeleteRecord = vi.fn().mockResolvedValue(true);
    renderCalibration({ library: measuredLibrary, onDeleteRecord, isNative: true });

    expect(screen.getByText("grey-flat-ratio-01")).toBeInTheDocument();
    expect(screen.getByText("#A4A8AA")).toBeInTheDocument();
    expect(screen.getByText(/T1: panchroma-cyan.*T4: matte-grey/i)).toBeInTheDocument();
    expect(screen.getByText(/T1 × 2.*T4 × 6/i)).toBeInTheDocument();
    expect(screen.getAllByText("Flat · Calibration Swatch")).not.toHaveLength(0);
    expect(screen.getByText(/0.4 mm nozzle.*0.08 mm plate layer.*4× subdivision/i)).toBeInTheDocument();
    expect(screen.getAllByText("Instrument sRGB")).not.toHaveLength(0);
    expect(screen.getByText("2026-08-01T18:30:00.000Z")).toBeInTheDocument();
    expect(screen.getByText("Colorimeter C-17")).toBeInTheDocument();
    expect(screen.getByText("Measured after a 24-hour cure.")).toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", {
        name: "Delete calibration record grey-flat-ratio-01",
      }),
    );
    expect(screen.getByText("Delete this measured sample?")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Confirm delete" })).toHaveFocus();
    fireEvent.click(screen.getByRole("button", { name: "Confirm delete" }));
    await waitFor(() =>
      expect(onDeleteRecord).toHaveBeenCalledWith("grey-flat-ratio-01"),
    );
  });

  it("labels migrated v1 records as unverified and excluded from planning", () => {
    const legacyLibrary: CmyxCalibrationLibraryDocument = {
      ...measuredLibrary,
      records: measuredLibrary.records.map((record) => ({
        ...record,
        provenance: {
          measuredAt: null,
          method: "legacy_unverified" as const,
          instrumentReference: null,
          operatorNotes: null,
        },
      })),
    };
    renderCalibration({ library: legacyLibrary, isNative: true });

    expect(screen.getByText("Legacy record — unverified")).toBeInTheDocument();
    expect(screen.getByText(/No verified measurement date/i)).toBeInTheDocument();
    expect(screen.getByText(/excluded from color planning/i)).toBeInTheDocument();
  });

  it("blocks unavailable fixed CMY loadouts with text, not color alone", async () => {
    const spools: PhysicalSpool[] = demoSpools.map((spool) =>
      spool.id === "panchroma-cyan" ? { ...spool, available: false } : spool,
    );
    const onSaveMeasurement = vi.fn().mockResolvedValue(true);
    renderCalibration({ spools, onSaveMeasurement });

    expect(screen.getByText(/panchroma-cyan · Out of stock/i)).toBeInTheDocument();
    expect(screen.getByText(/exist and be marked in stock/i)).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Record ID"), {
      target: { value: "blocked-coupon" },
    });
    completeMeasurementEvidence();
    fireEvent.click(screen.getByRole("button", { name: "Save measured sample" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "must all be in stock",
    );
    expect(onSaveMeasurement).not.toHaveBeenCalled();
  });

  it("keeps nominal preview values out of measured records until physical evidence is explicit", async () => {
    const onSaveMeasurement = vi.fn().mockResolvedValue(true);
    renderCalibration({ onSaveMeasurement });
    fireEvent.change(screen.getByLabelText("Record ID"), {
      target: { value: "nominal-preview" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save measured sample" }));

    expect(onSaveMeasurement).not.toHaveBeenCalled();
    expect(screen.getByLabelText("Measured sRGB HEX")).toBeInvalid();
    expect(
      screen.getByRole("checkbox", { name: /I used a physical printed swatch/i }),
    ).not.toBeChecked();
  });
});
