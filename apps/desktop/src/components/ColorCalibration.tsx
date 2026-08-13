import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type FormEvent,
} from "react";
import {
  CheckCircle2,
  FileOutput,
  FlaskConical,
  LoaderCircle,
  Pipette,
  Trash2,
  TriangleAlert,
} from "lucide-react";

import type {
  CmyxCalibrationLibraryDocument,
  CmyxCalibrationMeasurementInput,
  CmyxCalibrationChartMode,
  CmyxCalibrationProjectInput,
  CmyxCalibrationProjectResult,
  CmyxCalibrationRecord,
  CmyxGeometryContext,
  CmyxMeasurementRecipeMode,
  CmyxRecipeComponent,
  CmyxVerifiedMeasurementMethod,
  PhysicalSpool,
} from "../types";
import { calibrationIdentityFor } from "../services/filament-library";
import { ColorSwatch } from "./ColorSwatch";

type GeometryPresetId =
  | "unknown"
  | "flat-swatch"
  | "upright-wall"
  | "flat-top"
  | "upright-volume";

interface GeometryPreset {
  id: GeometryPresetId;
  label: string;
  description: string;
  context: CmyxGeometryContext;
}

interface ColorCalibrationProps {
  library: CmyxCalibrationLibraryDocument;
  spools: PhysicalSpool[];
  isNative: boolean;
  isInventoryReady: boolean;
  isSaving: boolean;
  isGeneratingProject: boolean;
  onGenerateProject: (
    input: CmyxCalibrationProjectInput,
  ) => Promise<CmyxCalibrationProjectResult | null>;
  onSaveMeasurement: (
    measurement: CmyxCalibrationMeasurementInput,
  ) => Promise<boolean>;
  onDeleteRecord: (recordId: string) => Promise<boolean>;
  onSetPlanningGeometry: (
    geometryContext: CmyxGeometryContext,
  ) => Promise<boolean>;
}

const geometryPresets: readonly GeometryPreset[] = [
  {
    id: "unknown",
    label: "Unknown geometry",
    description: "Safe default: measured samples are not reused by planning.",
    context: { orientation: "unknown", geometryClass: "unknown" },
  },
  {
    id: "flat-swatch",
    label: "Flat calibration swatch",
    description: "A flat, purpose-built color coupon.",
    context: { orientation: "flat", geometryClass: "calibration_swatch" },
  },
  {
    id: "upright-wall",
    label: "Upright thin wall",
    description: "A vertical wall or shell coupon.",
    context: { orientation: "upright", geometryClass: "thin_wall" },
  },
  {
    id: "flat-top",
    label: "Flat top surface",
    description: "A broad horizontal top-surface coupon.",
    context: { orientation: "flat", geometryClass: "top_surface" },
  },
  {
    id: "upright-volume",
    label: "Upright volumetric coupon",
    description: "A solid three-dimensional coupon.",
    context: { orientation: "upright", geometryClass: "volumetric" },
  },
] as const;

const measurementPresets = geometryPresets.filter(
  (preset) => preset.id !== "unknown",
);

const modeOptions: ReadonlyArray<{
  id: CmyxMeasurementRecipeMode;
  label: string;
  description: string;
}> = [
  {
    id: "ratio",
    label: "Ratio",
    description: "Repeat two or three slots at relative layer weights.",
  },
  {
    id: "match",
    label: "Match",
    description: "Record a selected two- or three-slot color match.",
  },
  {
    id: "gradient",
    label: "Gradient",
    description: "Transition between exactly two slots in this order.",
  },
  {
    id: "cycle",
    label: "Cycle",
    description: "Cycle through two, three, or four slots in this order.",
  },
];

const measurementMethodOptions: ReadonlyArray<{
  id: CmyxVerifiedMeasurementMethod;
  label: string;
  description: string;
}> = [
  {
    id: "instrument_lab",
    label: "Instrument Lab → sRGB",
    description:
      "A colorimeter or spectrophotometer produced Lab data that was converted to this sRGB value.",
  },
  {
    id: "instrument_srgb",
    label: "Instrument sRGB",
    description:
      "A calibrated instrument or its trusted software produced this sRGB value directly.",
  },
  {
    id: "reliable_manual_srgb",
    label: "Reliable manual sRGB",
    description:
      "You are entering an sRGB result from a reliable external measurement workflow.",
  },
  {
    id: "visual_swatch",
    label: "Visual swatch comparison",
    description:
      "You compared the physical print under controlled light; this is lower-confidence evidence.",
  },
];

const chartModeOptions: ReadonlyArray<{
  id: CmyxCalibrationChartMode;
  label: string;
  description: string;
}> = [
  {
    id: "full",
    label: "Full · 26 swatches (recommended)",
    description:
      "All solids, 1:1 pairs, both 2:1 pair directions, and equal three-color mixes.",
  },
  {
    id: "quick",
    label: "Quick · 10 swatches",
    description: "Four solids and every 1:1 pair for a fast visual screening print.",
  },
];

const fixedRoleCandidates = {
  T1: ["panchroma-translucent-cyan", "panchroma-cyan"],
  T2: ["panchroma-translucent-magenta", "panchroma-magenta"],
  T3: ["panchroma-translucent-yellow", "panchroma-yellow"],
} as const;

function normalizedWords(spool: PhysicalSpool) {
  return `${spool.name} ${spool.colorName}`.toLocaleLowerCase();
}

function findFixedSpool(
  spools: PhysicalSpool[],
  toolhead: keyof typeof fixedRoleCandidates,
) {
  const ids = fixedRoleCandidates[toolhead];
  const byId = ids
    .map((id) => spools.find((spool) => spool.id === id))
    .find(Boolean);
  if (byId) return byId;
  const color = toolhead === "T1" ? "cyan" : toolhead === "T2" ? "magenta" : "yellow";
  return spools.find(
    (spool) =>
      spool.material === "PLA" &&
      new RegExp(`\\b${color}\\b`, "i").test(normalizedWords(spool)),
  );
}

function contextEquals(left: CmyxGeometryContext, right: CmyxGeometryContext) {
  return JSON.stringify(left) === JSON.stringify(right);
}

function presetForContext(context: CmyxGeometryContext) {
  return geometryPresets.find((preset) => contextEquals(preset.context, context));
}

function displayVariant(
  value: string | { custom: string },
) {
  if (typeof value === "object") return value.custom;
  return value
    .split("_")
    .map((part) => `${part.slice(0, 1).toUpperCase()}${part.slice(1)}`)
    .join(" ");
}

function contextLabel(context: CmyxGeometryContext) {
  return `${displayVariant(context.orientation)} · ${displayVariant(
    context.geometryClass,
  )}`;
}

function recordContext(record: CmyxCalibrationRecord): CmyxGeometryContext {
  return {
    orientation: record.context.orientation,
    geometryClass: record.context.geometryClass,
  };
}

function allowedComponentCounts(mode: CmyxMeasurementRecipeMode) {
  if (mode === "gradient") return [2];
  if (mode === "cycle") return [2, 3, 4];
  return [2, 3];
}

function makeComponents(count: number, existing: CmyxRecipeComponent[]) {
  const slotOrder: CmyxRecipeComponent["slot"][] = [1, 4, 2, 3];
  return Array.from({ length: count }, (_, index) =>
    existing[index] ?? { slot: slotOrder[index], weight: 1 },
  );
}

function loadoutEntries(record: CmyxCalibrationRecord) {
  return [
    ["T1", record.loadout.t1CalibrationId],
    ["T2", record.loadout.t2CalibrationId],
    ["T3", record.loadout.t3CalibrationId],
    ["T4", record.loadout.t4CalibrationId],
  ] as const;
}

function currentLocalDateTimeValue() {
  const now = new Date();
  const local = new Date(now.getTime() - now.getTimezoneOffset() * 60_000);
  return local.toISOString().slice(0, 19);
}

function measurementTimestamp(value: string) {
  const timestamp = new Date(value);
  return Number.isFinite(timestamp.getTime()) ? timestamp.toISOString() : null;
}

function measurementMethodLabel(record: CmyxCalibrationRecord) {
  if (record.provenance.method === "legacy_unverified") {
    return "Legacy record — unverified";
  }
  return (
    measurementMethodOptions.find(
      (option) => option.id === record.provenance.method,
    )?.label ?? displayVariant(record.provenance.method)
  );
}

function syncFormControlValidity(event: FormEvent<HTMLFormElement>) {
  const target = event.target;
  if (
    !(target instanceof HTMLInputElement) &&
    !(target instanceof HTMLSelectElement) &&
    !(target instanceof HTMLTextAreaElement)
  ) {
    return;
  }
  if (event.type === "input" && !target.hasAttribute("aria-invalid")) return;
  if (target.validity.valid) {
    target.removeAttribute("aria-invalid");
  } else {
    target.setAttribute("aria-invalid", "true");
  }
}

export function ColorCalibration({
  library,
  spools,
  isNative,
  isInventoryReady,
  isSaving,
  isGeneratingProject,
  onGenerateProject,
  onSaveMeasurement,
  onDeleteRecord,
  onSetPlanningGeometry,
}: ColorCalibrationProps) {
  const id = useId().replace(/:/g, "");
  const fixedSpools = useMemo(
    () => ({
      T1: findFixedSpool(spools, "T1"),
      T2: findFixedSpool(spools, "T2"),
      T3: findFixedSpool(spools, "T3"),
    }),
    [spools],
  );
  const fixedIds = useMemo(
    () =>
      new Set(
        Object.values(fixedSpools)
          .filter((spool): spool is PhysicalSpool => Boolean(spool))
          .map((spool) => spool.id),
      ),
    [fixedSpools],
  );
  const t4Options = useMemo(
    () =>
      spools
        .filter(
          (spool) =>
            spool.material === "PLA" && spool.available && !fixedIds.has(spool.id),
        )
        .sort((left, right) => left.name.localeCompare(right.name)),
    [fixedIds, spools],
  );
  const fixedLoadoutReady = Object.values(fixedSpools).every(
    (spool) => spool?.available,
  );

  const initialPlanningPreset =
    presetForContext(library.planningGeometryContext)?.id ?? "custom";
  const [planningPresetId, setPlanningPresetId] = useState(initialPlanningPreset);
  const [recordId, setRecordId] = useState("");
  const [t4SpoolId, setT4SpoolId] = useState(t4Options[0]?.id ?? "");
  const [measurementPresetId, setMeasurementPresetId] =
    useState<GeometryPresetId>("flat-swatch");
  const [mode, setMode] = useState<CmyxMeasurementRecipeMode>("ratio");
  const [components, setComponents] = useState<CmyxRecipeComponent[]>([
    { slot: 1, weight: 1 },
    { slot: 4, weight: 1 },
  ]);
  const [measuredHex, setMeasuredHex] = useState("");
  const [measurementDateTime, setMeasurementDateTime] = useState(
    currentLocalDateTimeValue,
  );
  const [measurementMethod, setMeasurementMethod] =
    useState<CmyxVerifiedMeasurementMethod | "">("");
  const [instrumentReference, setInstrumentReference] = useState("");
  const [operatorNotes, setOperatorNotes] = useState("");
  const [physicalMeasurementConfirmed, setPhysicalMeasurementConfirmed] =
    useState(false);
  const [projectId, setProjectId] = useState("cmyx-calibration-chart");
  const [chartMode, setChartMode] =
    useState<CmyxCalibrationChartMode>("full");
  const [projectError, setProjectError] = useState("");
  const [projectStatus, setProjectStatus] = useState("");
  const [generatedProject, setGeneratedProject] =
    useState<CmyxCalibrationProjectResult | null>(null);
  const [formError, setFormError] = useState("");
  const [formStatus, setFormStatus] = useState("");
  const [geometryError, setGeometryError] = useState("");
  const [geometryStatus, setGeometryStatus] = useState("");
  const [deleteCandidateId, setDeleteCandidateId] = useState<string | null>(null);
  const [deleteError, setDeleteError] = useState("");
  const deleteConfirmRef = useRef<HTMLButtonElement | null>(null);

  useEffect(() => {
    setPlanningPresetId(
      presetForContext(library.planningGeometryContext)?.id ?? "custom",
    );
  }, [library.planningGeometryContext]);

  useEffect(() => {
    if (!t4Options.some((spool) => spool.id === t4SpoolId)) {
      setT4SpoolId(t4Options[0]?.id ?? "");
    }
  }, [t4Options, t4SpoolId]);

  useEffect(() => {
    if (deleteCandidateId) deleteConfirmRef.current?.focus();
  }, [deleteCandidateId]);

  const planningPreset = geometryPresets.find(
    (preset) => preset.id === planningPresetId,
  );
  const activePlanningPreset = presetForContext(
    library.planningGeometryContext,
  );
  const planningUnknown = activePlanningPreset?.id === "unknown";
  const selectedT4Spool = t4Options.find((spool) => spool.id === t4SpoolId);

  const generateProject = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setProjectError("");
    setProjectStatus("");
    setGeneratedProject(null);
    const form = event.currentTarget;
    if (!form.checkValidity()) {
      form.reportValidity();
      setProjectError("Enter a portable project ID before choosing an output file.");
      return;
    }
    if (!isNative) {
      setProjectError(
        "Calibration 3MF generation is unavailable in browser demo mode.",
      );
      return;
    }
    if (!isInventoryReady) {
      setProjectError(
        "Wait for the native Filament Library to finish loading before generating a chart.",
      );
      return;
    }
    if (
      !fixedLoadoutReady ||
      !fixedSpools.T1 ||
      !fixedSpools.T2 ||
      !fixedSpools.T3 ||
      !selectedT4Spool
    ) {
      setProjectError(
        "Choose four distinct in-stock PLA spools for the fixed CMY plus T4 loadout.",
      );
      return;
    }
    try {
      const result = await onGenerateProject({
        projectId: projectId.trim(),
        chartMode,
        spoolIds: [
          fixedSpools.T1.id,
          fixedSpools.T2.id,
          fixedSpools.T3.id,
          selectedT4Spool.id,
        ],
      });
      if (!result) {
        setProjectStatus("Output selection was canceled; no file was created.");
        return;
      }
      setGeneratedProject(result);
      setProjectStatus(
        `${result.fileName} was generated and passed native candidate validation.`,
      );
    } catch (error) {
      setProjectError(error instanceof Error ? error.message : String(error));
    }
  };

  const changeMode = (nextMode: CmyxMeasurementRecipeMode) => {
    const counts = allowedComponentCounts(nextMode);
    const nextCount = counts.includes(components.length)
      ? components.length
      : counts[0];
    setMode(nextMode);
    setComponents((current) => makeComponents(nextCount, current));
    setFormError("");
    setFormStatus("");
  };

  const savePlanningGeometry = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setGeometryError("");
    setGeometryStatus("");
    if (!planningPreset) {
      setGeometryError("Choose one of the qualified planning geometry presets.");
      return;
    }
    const saved = await onSetPlanningGeometry(planningPreset.context);
    if (saved) {
      setGeometryStatus(
        planningPreset.id === "unknown"
          ? "Unknown geometry saved. Measured-sample reuse is disabled."
          : `${planningPreset.label} saved for the next plan. Recalculate any analyzed plan.`,
      );
    } else {
      setGeometryError("Planning geometry was not saved. Review the storage error above.");
    }
  };

  const saveMeasurement = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setFormError("");
    setFormStatus("");
    const form = event.currentTarget;
    if (!form.checkValidity()) {
      form.reportValidity();
      setFormError("Complete every required field in the measurement form.");
      return;
    }
    if (!fixedLoadoutReady) {
      setFormError("The fixed Cyan, Magenta, and Yellow PLA spools must all be in stock.");
      return;
    }
    if (isNative && !isInventoryReady) {
      setFormError(
        "Wait for the native Filament Library to finish loading before saving a measurement.",
      );
      return;
    }
    const t4 = t4Options.find((spool) => spool.id === t4SpoolId);
    const geometry = measurementPresets.find(
      (preset) => preset.id === measurementPresetId,
    );
    if (!fixedSpools.T1 || !fixedSpools.T2 || !fixedSpools.T3 || !t4) {
      setFormError("Choose an available PLA spool for every T1–T4 position.");
      return;
    }
    if (!geometry) {
      setFormError("Choose a qualified coupon geometry.");
      return;
    }
    if (new Set(components.map((component) => component.slot)).size !== components.length) {
      setFormError("Each recipe component must use a different T1–T4 slot.");
      return;
    }
    const measuredAt = measurementTimestamp(measurementDateTime);
    if (!measuredAt || !measurementMethod || !physicalMeasurementConfirmed) {
      setFormError(
        "Enter the physical measurement date, choose its method, and confirm that the value came from a printed swatch.",
      );
      return;
    }

    const saved = await onSaveMeasurement({
      id: recordId.trim(),
      loadout: {
        t1CalibrationId: calibrationIdentityFor(fixedSpools.T1),
        t2CalibrationId: calibrationIdentityFor(fixedSpools.T2),
        t3CalibrationId: calibrationIdentityFor(fixedSpools.T3),
        t4CalibrationId: calibrationIdentityFor(t4),
      },
      recipe: { mode, components },
      measuredOutputHex: measuredHex.toUpperCase(),
      geometryContext: geometry.context,
      provenance: {
        measuredAt,
        method: measurementMethod,
        instrumentReference: instrumentReference.trim() || null,
        operatorNotes: operatorNotes.trim() || null,
      },
      physicalMeasurementConfirmed: true,
    });
    if (saved) {
      setFormStatus(
        `${recordId.trim()} saved as a measured CMY+X sample. Recalculate any analyzed plan.`,
      );
      setRecordId("");
      setPhysicalMeasurementConfirmed(false);
    } else {
      setFormError("The measurement was not saved. Review the storage error above.");
    }
  };

  const confirmDelete = async () => {
    if (!deleteCandidateId) return;
    setDeleteError("");
    const deleted = await onDeleteRecord(deleteCandidateId);
    if (deleted) {
      setDeleteCandidateId(null);
    } else {
      setDeleteError("The calibration record was not deleted. Review the storage error above.");
    }
  };

  return (
    <section className="color-calibration" aria-labelledby={`${id}-heading`}>
      <header className="color-calibration__header">
        <span className="color-calibration__icon" aria-hidden="true">
          <Pipette />
        </span>
        <div>
          <h2 id={`${id}-heading`}>CMY+X Color Calibration</h2>
          <p>
            Record measurements for one exact T1–T4 spool loadout, U1 Full
            Spectrum process, and printed coupon geometry.
          </p>
        </div>
      </header>

      <div className="calibration-notices">
        <div className="calibration-notice calibration-notice--important">
          <FlaskConical aria-hidden="true" />
          <p>
            <strong>Chart generation and physical measurement are separate qualification steps.</strong>{" "}
            The native chart remains a non-production qualification candidate.
            Print it with the exact T1–T4 loadout, then record measured six-digit
            sRGB HEX values; nominal library colors are not measured output.
          </p>
        </div>
        {!isNative ? (
          <div className="calibration-notice calibration-notice--demo">
            <TriangleAlert aria-hidden="true" />
            <p>
              Browser demo records are saved only in this browser profile. They
              are not injected into native U1 planning, and browser mode never
              creates or pretends to create a calibration 3MF. Use the desktop
              app for native chart generation and persistent calibration.
            </p>
          </div>
        ) : null}
      </div>

      <section className="calibration-panel" aria-labelledby={`${id}-geometry-heading`}>
        <div className="calibration-panel__heading">
          <div>
            <h3 id={`${id}-geometry-heading`}>Active planning geometry</h3>
            <p>
              Measurements are reused only when loadout, native process, orientation,
              and geometry class all match the next plan.
            </p>
          </div>
          <span className={`calibration-reuse-status ${planningUnknown ? "is-disabled" : "is-enabled"}`}>
            {planningUnknown ? "Reuse disabled" : "Geometry qualified"}
          </span>
        </div>
        <form className="calibration-geometry-form" onSubmit={savePlanningGeometry}>
          <div className="calibration-field">
            <label htmlFor={`${id}-planning-geometry`}>Planning geometry preset</label>
            <select
              id={`${id}-planning-geometry`}
              value={planningPresetId}
              onChange={(event) => {
                setPlanningPresetId(event.target.value);
                setGeometryError("");
                setGeometryStatus("");
              }}
            >
              {planningPresetId === "custom" ? (
                <option value="custom" disabled>
                  Existing custom geometry
                </option>
              ) : null}
              {geometryPresets.map((preset) => (
                <option key={preset.id} value={preset.id}>
                  {preset.label}
                </option>
              ))}
            </select>
            <small>
              <strong>Currently stored:</strong>{" "}
              {activePlanningPreset?.label ??
                contextLabel(library.planningGeometryContext)}. {" "}
              {planningPreset?.description ?? "Choose a qualified preset to replace it."}
            </small>
          </div>
          <button className="button button--primary" type="submit" disabled={isSaving}>
            {isSaving ? <LoaderCircle className="spin" aria-hidden="true" /> : null}
            Save planning geometry
          </button>
        </form>
        {planningUnknown ? (
          <p className="calibration-warning">
            <TriangleAlert aria-hidden="true" />
            Unknown is deliberately safe: planning will not reuse any measured
            sample until a specific geometry preset is saved.
          </p>
        ) : null}
        {geometryError ? <p className="calibration-error" role="alert">{geometryError}</p> : null}
        {geometryStatus ? <p className="calibration-success" role="status"><CheckCircle2 aria-hidden="true" />{geometryStatus}</p> : null}
      </section>

      <section className="calibration-panel" aria-labelledby={`${id}-loadout-heading`}>
        <div className="calibration-panel__heading">
          <div>
            <h3 id={`${id}-loadout-heading`}>Physical calibration loadout</h3>
            <p>
              T1–T3 are fixed CMY. Choose the available PLA spool physically loaded
              in T4. Chart generation and every later measurement use these exact IDs.
            </p>
          </div>
        </div>

        <div className="calibration-loadout" aria-label="Fixed CMY toolheads">
          {(["T1", "T2", "T3"] as const).map((toolhead) => {
            const spool = fixedSpools[toolhead];
            return (
              <div className="calibration-spool-card" key={toolhead}>
                <span className="calibration-spool-card__slot">{toolhead}</span>
                {spool ? <ColorSwatch hex={spool.hex} size="large" /> : null}
                <div>
                  <strong>{spool?.name ?? "Required CMY spool not found"}</strong>
                  <span>{spool ? `${spool.colorName} · PLA` : "Add it to Filament Library"}</span>
                  <small>{spool ? `${spool.id} · ${spool.available ? "In stock" : "Out of stock"}` : "Unavailable"}</small>
                </div>
              </div>
            );
          })}
        </div>

        {!isInventoryReady ? (
          <p className="calibration-warning">
            <TriangleAlert aria-hidden="true" />
            Filament Library is not ready. Native generation stays disabled
            until the authoritative inventory has loaded.
          </p>
        ) : !fixedLoadoutReady ? (
          <p className="calibration-warning">
            <TriangleAlert aria-hidden="true" />
            Calibration requires the fixed Cyan, Magenta, and Yellow PLA
            spools to exist and be marked in stock in Filament Library.
          </p>
        ) : null}

        <div className="calibration-field calibration-t4-selector">
          <label htmlFor={`${id}-t4-spool`}>T4 available PLA spool</label>
          <select
            id={`${id}-t4-spool`}
            required
            value={t4SpoolId}
            onChange={(event) => {
              setT4SpoolId(event.target.value);
              setGeneratedProject(null);
              setProjectError("");
              setProjectStatus("");
            }}
          >
            <option value="" disabled>
              {t4Options.length ? "Choose T4 spool" : "No available PLA T4 spool"}
            </option>
            {t4Options.map((spool) => (
              <option key={spool.id} value={spool.id}>
                {spool.colorName} · {spool.name} · {spool.hex}
              </option>
            ))}
          </select>
          <small>The selected spool ID and resolved physical profile are embedded in the chart manifest.</small>
          {selectedT4Spool ? (
            <span className="calibration-selected-t4">
              <ColorSwatch hex={selectedT4Spool.hex} size="small" />
              <span>
                <strong>T4: {selectedT4Spool.id}</strong>
                <small>
                  Library swatch {selectedT4Spool.hex} · {selectedT4Spool.colorBasis} · In stock
                </small>
              </span>
            </span>
          ) : null}
        </div>
      </section>

      <section className="calibration-panel" aria-labelledby={`${id}-project-heading`}>
        <div className="calibration-panel__heading">
          <div>
            <h3 id={`${id}-project-heading`}>Generate a CMY+X calibration chart</h3>
            <p>
              Creates a new, numbered 3MF qualification candidate and validates
              its embedded manifest and Full Spectrum contract before reporting it.
            </p>
          </div>
          <span className="calibration-reuse-status is-disabled">Candidate only</span>
        </div>
        <form className="calibration-project-form" onSubmit={generateProject}>
          <fieldset className="calibration-mode-fieldset calibration-chart-mode">
            <legend>Chart detail</legend>
            <div className="calibration-mode-options">
              {chartModeOptions.map((option) => (
                <label key={option.id}>
                  <input
                    type="radio"
                    name={`${id}-chart-mode`}
                    value={option.id}
                    checked={chartMode === option.id}
                    onChange={() => {
                      setChartMode(option.id);
                      setGeneratedProject(null);
                      setProjectError("");
                      setProjectStatus("");
                    }}
                  />
                  <span>{option.label}</span>
                  <small>{option.description}</small>
                </label>
              ))}
            </div>
          </fieldset>
          <div className="calibration-field">
            <label htmlFor={`${id}-project-id`}>Calibration project ID</label>
            <input
              id={`${id}-project-id`}
              name="calibrationProjectId"
              type="text"
              required
              minLength={1}
              maxLength={64}
              pattern="[A-Za-z0-9][A-Za-z0-9_-]{0,63}"
              value={projectId}
              onChange={(event) => {
                setProjectId(event.target.value);
                setGeneratedProject(null);
                setProjectError("");
                setProjectStatus("");
              }}
              aria-describedby={`${id}-project-id-hint`}
            />
            <small id={`${id}-project-id-hint`}>
              1–64 ASCII letters or digits; internal hyphen and underscore are allowed.
            </small>
          </div>
          <ol className="calibration-project-loadout" aria-label="Chart physical loadout">
            {([fixedSpools.T1, fixedSpools.T2, fixedSpools.T3, selectedT4Spool] as const).map(
              (spool, index) => (
                <li key={`T${index + 1}`}>
                  <span>T{index + 1}</span>
                  {spool ? <ColorSwatch hex={spool.hex} size="small" /> : null}
                  <strong>{spool?.id ?? "Unavailable"}</strong>
                </li>
              ),
            )}
          </ol>
          <div className="calibration-form-actions">
            <button
              className="button button--primary"
              type="submit"
              disabled={isGeneratingProject || !isNative || !isInventoryReady}
            >
              {isGeneratingProject ? (
                <LoaderCircle className="spin" aria-hidden="true" />
              ) : (
                <FileOutput aria-hidden="true" />
              )}
              {isGeneratingProject ? "Generating and validating…" : "Choose output and generate 3MF"}
            </button>
            <p>
              {isNative && isInventoryReady
                ? "Existing destinations are rejected; the generator never overwrites a file."
                : isNative
                  ? "Unavailable until the authoritative Filament Library has loaded."
                  : "Unavailable in browser demo mode; no local file will be created."}
            </p>
          </div>
        </form>
        {projectError ? <p className="calibration-error" role="alert">{projectError}</p> : null}
        {projectStatus ? <p className="calibration-success" role="status"><CheckCircle2 aria-hidden="true" />{projectStatus}</p> : null}
        {generatedProject ? (
          <article className="calibration-project-result" aria-labelledby={`${id}-project-result-heading`}>
            <header>
              <CheckCircle2 aria-hidden="true" />
              <div>
                <h4 id={`${id}-project-result-heading`}>Validated qualification candidate</h4>
                <p>
                  {generatedProject.fileName} · {generatedProject.chartMode === "full" ? "Full" : "Quick"} ·{" "}
                  {generatedProject.swatchCount} numbered swatches
                </p>
              </div>
              <strong>Native validation passed</strong>
            </header>
            <dl>
              <div><dt>Output path</dt><dd><code>{generatedProject.path}</code></dd></div>
              <div><dt>Artifact SHA-256</dt><dd><code>{generatedProject.artifactSha256}</code></dd></div>
              <div><dt>Manifest</dt><dd><code>{generatedProject.manifestPath}</code></dd></div>
              <div><dt>Manifest SHA-256</dt><dd><code>{generatedProject.manifestSha256}</code></dd></div>
            </dl>
            <p className="calibration-project-result__boundary">
              <TriangleAlert aria-hidden="true" />
              <span>
                <strong><code>productionQualified=false</code></strong>{" "}
                This file is unsliced and cannot qualify production color until
                the GUI round trip, physical print, and measurements are complete.
              </span>
            </p>
            <ul className="calibration-project-warnings" aria-label="Candidate warnings">
              {generatedProject.warnings.map((warning) => <li key={warning}>{warning}</li>)}
            </ul>
            <div className="calibration-project-next-steps">
              <h5>Required next steps</h5>
              <ol>
                {generatedProject.nextSteps.map((step) => <li key={step}>{step}</li>)}
              </ol>
            </div>
          </article>
        ) : null}
      </section>

      <section className="calibration-panel" aria-labelledby={`${id}-measurement-heading`}>
        <div className="calibration-panel__heading">
          <div>
            <h3 id={`${id}-measurement-heading`}>Record a printed swatch measurement</h3>
            <p>
              After printing, copy one manifest recipe and the measured six-digit
              sRGB HEX value. The selected physical loadout above remains authoritative.
            </p>
          </div>
        </div>

        <form
          className="calibration-measurement-form"
          onSubmit={saveMeasurement}
          onInvalid={syncFormControlValidity}
          onInput={syncFormControlValidity}
        >
          <div className="calibration-form-grid calibration-form-grid--measurement">
            <div className="calibration-field">
              <label htmlFor={`${id}-record-id`}>Record ID</label>
              <input
                id={`${id}-record-id`}
                type="text"
                required
                maxLength={128}
                value={recordId}
                onChange={(event) => setRecordId(event.target.value)}
                placeholder="grey-flat-ratio-01"
              />
              <small>A unique lab or coupon identifier; saving the same ID replaces it.</small>
            </div>
            <div className="calibration-field">
              <label htmlFor={`${id}-coupon-geometry`}>Printed coupon geometry</label>
              <select
                id={`${id}-coupon-geometry`}
                required
                value={measurementPresetId}
                onChange={(event) => setMeasurementPresetId(event.target.value as GeometryPresetId)}
              >
                {measurementPresets.map((preset) => (
                  <option key={preset.id} value={preset.id}>
                    {preset.label}
                  </option>
                ))}
              </select>
              <small>{measurementPresets.find((preset) => preset.id === measurementPresetId)?.description}</small>
            </div>
          </div>

          <fieldset className="calibration-mode-fieldset">
            <legend>Measurement basis and method</legend>
            <p id={`${id}-measurement-method-hint`}>
              Required. Choose how the physical printed swatch was evaluated.
              A nominal preview is not a measurement.
            </p>
            <div
              className="calibration-mode-options"
              aria-describedby={`${id}-measurement-method-hint`}
            >
              {measurementMethodOptions.map((option) => (
                <label key={option.id}>
                  <input
                    type="radio"
                    name={`${id}-measurement-method`}
                    value={option.id}
                    required
                    checked={measurementMethod === option.id}
                    onChange={() => setMeasurementMethod(option.id)}
                  />
                  <span>
                    <strong>{option.label}</strong>
                    <small>{option.description}</small>
                  </span>
                </label>
              ))}
            </div>
          </fieldset>

          <div className="calibration-form-grid calibration-form-grid--measurement">
            <div className="calibration-field">
              <label htmlFor={`${id}-measurement-date-time`}>
                Measurement date and time
              </label>
              <input
                id={`${id}-measurement-date-time`}
                name="measurementDateTime"
                type="datetime-local"
                step={1}
                required
                value={measurementDateTime}
                onChange={(event) => setMeasurementDateTime(event.target.value)}
                aria-describedby={`${id}-measurement-date-time-hint`}
              />
              <small id={`${id}-measurement-date-time-hint`}>
                Stored as an RFC 3339 instant with your current time-zone offset.
              </small>
            </div>
            <div className="calibration-field">
              <label htmlFor={`${id}-instrument-reference`}>
                Instrument or reference (optional)
              </label>
              <input
                id={`${id}-instrument-reference`}
                name="instrumentReference"
                type="text"
                maxLength={512}
                value={instrumentReference}
                onChange={(event) => setInstrumentReference(event.target.value)}
                placeholder="Instrument model/serial, profile, or reference card"
              />
            </div>
          </div>

          <div className="calibration-field">
            <label htmlFor={`${id}-operator-notes`}>Operator notes (optional)</label>
            <textarea
              id={`${id}-operator-notes`}
              name="operatorNotes"
              maxLength={2048}
              rows={3}
              value={operatorNotes}
              onChange={(event) => setOperatorNotes(event.target.value)}
              placeholder="Lighting, conversion workflow, coupon condition, or other audit notes"
            />
          </div>

          <fieldset className="calibration-mode-fieldset">
            <legend>Recipe mode</legend>
            <p id={`${id}-mode-hint`}>Choose the mode used to print this exact coupon.</p>
            <div className="calibration-mode-options" aria-describedby={`${id}-mode-hint`}>
              {modeOptions.map((option) => (
                <label key={option.id}>
                  <input
                    type="radio"
                    name={`${id}-recipe-mode`}
                    value={option.id}
                    checked={mode === option.id}
                    onChange={() => changeMode(option.id)}
                  />
                  <span><strong>{option.label}</strong><small>{option.description}</small></span>
                </label>
              ))}
            </div>
          </fieldset>

          <div className="calibration-recipe-builder">
            <div className="calibration-field calibration-field--count">
              <label htmlFor={`${id}-component-count`}>Recipe components</label>
              <select
                id={`${id}-component-count`}
                value={components.length}
                onChange={(event) =>
                  setComponents((current) => makeComponents(Number(event.target.value), current))
                }
              >
                {allowedComponentCounts(mode).map((count) => (
                  <option key={count} value={count}>{count}</option>
                ))}
              </select>
              <small>Order below is significant and matches serialized layer order.</small>
            </div>
            <ol className="calibration-component-list">
              {components.map((component, index) => (
                <li key={index}>
                  <span className="calibration-component-list__order">{index + 1}</span>
                  <div className="calibration-field">
                    <label htmlFor={`${id}-component-${index}-slot`}>Component {index + 1} slot</label>
                    <select
                      id={`${id}-component-${index}-slot`}
                      value={component.slot}
                      onChange={(event) =>
                        setComponents((current) => current.map((item, itemIndex) =>
                          itemIndex === index
                            ? { ...item, slot: Number(event.target.value) as CmyxRecipeComponent["slot"] }
                            : item,
                        ))
                      }
                    >
                      {[1, 2, 3, 4].map((slot) => <option key={slot} value={slot}>T{slot}</option>)}
                    </select>
                  </div>
                  <div className="calibration-field">
                    <label htmlFor={`${id}-component-${index}-weight`}>Relative weight</label>
                    <input
                      id={`${id}-component-${index}-weight`}
                      type="number"
                      required
                      min={1}
                      max={255}
                      step={1}
                      value={component.weight}
                      onChange={(event) =>
                        setComponents((current) => current.map((item, itemIndex) =>
                          itemIndex === index
                            ? { ...item, weight: Number(event.target.value) }
                            : item,
                        ))
                      }
                    />
                  </div>
                </li>
              ))}
            </ol>
          </div>

          <div className="calibration-measured-color">
            <div className="calibration-field calibration-field--color">
              <label htmlFor={`${id}-measured-color-picker`}>Measured output color</label>
              <input
                id={`${id}-measured-color-picker`}
                name="measuredColorPicker"
                type="color"
                value={/^#[0-9A-Fa-f]{6}$/.test(measuredHex) ? measuredHex : "#808080"}
                onChange={(event) => setMeasuredHex(event.target.value.toUpperCase())}
              />
            </div>
            <div className="calibration-field">
              <label htmlFor={`${id}-measured-hex`}>Measured sRGB HEX</label>
              <input
                id={`${id}-measured-hex`}
                name="measuredOutputHex"
                type="text"
                required
                pattern="#[0-9A-Fa-f]{6}"
                maxLength={7}
                value={measuredHex}
                onChange={(event) => setMeasuredHex(event.target.value)}
                placeholder="#808080"
                aria-describedby={`${id}-hex-hint`}
              />
              <small id={`${id}-hex-hint`}>
                Exactly six hexadecimal digits including #. This field starts
                empty so the nominal preview cannot be saved accidentally.
              </small>
            </div>
          </div>

          <label className="calibration-measurement-confirmation">
            <input
              type="checkbox"
              name="physicalMeasurementConfirmed"
              required
              checked={physicalMeasurementConfirmed}
              onChange={(event) =>
                setPhysicalMeasurementConfirmed(event.target.checked)
              }
            />
            <span>
              <strong>I used a physical printed swatch.</strong>
              <small>
                I confirm that the entered sRGB value is measurement or visual
                comparison evidence, not a nominal library color or preview.
              </small>
            </span>
          </label>

          {formError ? <p className="calibration-error" role="alert">{formError}</p> : null}
          {formStatus ? <p className="calibration-success" role="status"><CheckCircle2 aria-hidden="true" />{formStatus}</p> : null}
          <div className="calibration-form-actions">
            <button className="button button--primary" type="submit" disabled={isSaving}>
              {isSaving ? <LoaderCircle className="spin" aria-hidden="true" /> : null}
              Save measured sample
            </button>
            <p>Native mode derives process context; this form never supplies or edits it.</p>
          </div>
        </form>
      </section>

      <section className="calibration-panel" aria-labelledby={`${id}-records-heading`}>
        <div className="calibration-panel__heading">
          <div>
            <h3 id={`${id}-records-heading`}>Measured sample records</h3>
            <p>{library.records.length} {library.records.length === 1 ? "record" : "records"} stored.</p>
          </div>
        </div>
        {library.records.length === 0 ? (
          <p className="calibration-empty">No measured CMY+X samples have been recorded.</p>
        ) : (
          <ul className="calibration-record-list">
            {library.records.map((record) => {
              const isDeleteCandidate = deleteCandidateId === record.id;
              return (
                <li className="calibration-record" key={record.id}>
                  <div className="calibration-record__header">
                    <div className="calibration-record__measured">
                      <ColorSwatch hex={record.measuredOutputHex} size="large" />
                      <div><strong>{record.id}</strong><code>{record.measuredOutputHex}</code></div>
                    </div>
                    {!isDeleteCandidate ? (
                      <button
                        className="button button--compact"
                        type="button"
                        aria-label={`Delete calibration record ${record.id}`}
                        disabled={isSaving}
                        onClick={() => {
                          setDeleteError("");
                          setDeleteCandidateId(record.id);
                        }}
                      >
                        <Trash2 aria-hidden="true" /> Delete
                      </button>
                    ) : null}
                  </div>
                  <dl className="calibration-record__details">
                    <div><dt>Exact loadout</dt><dd>{loadoutEntries(record).map(([slot, spoolId]) => `${slot}: ${spoolId}`).join(" · ")}</dd></div>
                    <div><dt>Recipe</dt><dd><strong>{displayVariant(record.recipe.mode)}</strong> · {record.recipe.components.map((component) => `T${component.slot} × ${component.weight}`).join(" → ")}</dd></div>
                    <div><dt>Geometry</dt><dd>{contextLabel(recordContext(record))}</dd></div>
                    <div><dt>Qualified process</dt><dd>{record.context.printerProfile.printerModel} · {(record.context.nozzleDiameterMicrons / 1000).toFixed(1)} mm nozzle · {(record.context.plateLayerHeightMicrons / 1000).toFixed(2)} mm plate layer · {record.context.subdivisionFactor}× subdivision</dd></div>
                    <div>
                      <dt>Measurement evidence</dt>
                      <dd>
                        <strong>{measurementMethodLabel(record)}</strong>
                        {record.provenance.measuredAt ? (
                          <> · <time dateTime={record.provenance.measuredAt}>{record.provenance.measuredAt}</time></>
                        ) : (
                          " · No verified measurement date"
                        )}
                      </dd>
                    </div>
                    {record.provenance.instrumentReference ? (
                      <div><dt>Instrument/reference</dt><dd>{record.provenance.instrumentReference}</dd></div>
                    ) : null}
                    {record.provenance.operatorNotes ? (
                      <div><dt>Operator notes</dt><dd>{record.provenance.operatorNotes}</dd></div>
                    ) : null}
                  </dl>
                  {record.provenance.method === "legacy_unverified" ? (
                    <p className="calibration-warning">
                      <TriangleAlert aria-hidden="true" />
                      Migrated from schema v1 without measurement date or method.
                      This record is retained for recovery but excluded from color planning.
                    </p>
                  ) : null}
                  {isDeleteCandidate ? (
                    <div className="calibration-delete-confirmation" role="group" aria-label={`Delete ${record.id}`}>
                      <p><strong>Delete this measured sample?</strong> Planning will stop using it after recalculation.</p>
                      {deleteError ? <p className="calibration-error" role="alert">{deleteError}</p> : null}
                      <div>
                        <button
                          ref={deleteConfirmRef}
                          className="button button--danger"
                          type="button"
                          disabled={isSaving}
                          onClick={() => void confirmDelete()}
                        >
                          {isSaving ? <LoaderCircle className="spin" aria-hidden="true" /> : <Trash2 aria-hidden="true" />}
                          Confirm delete
                        </button>
                        <button
                          className="button"
                          type="button"
                          disabled={isSaving}
                          onClick={() => {
                            setDeleteCandidateId(null);
                            setDeleteError("");
                          }}
                        >
                          Cancel
                        </button>
                      </div>
                    </div>
                  ) : null}
                </li>
              );
            })}
          </ul>
        )}
      </section>
    </section>
  );
}
