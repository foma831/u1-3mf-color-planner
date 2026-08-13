import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";

import type {
  CmyxCalibrationContext,
  CmyxCalibrationLibraryDocument,
  CmyxCalibrationLoadout,
  CmyxCalibrationMeasurementInput,
  CmyxMeasurementMethod,
  CmyxMeasurementProvenance,
  CmyxCalibrationProjectInput,
  CmyxCalibrationProjectResult,
  CmyxCalibrationProjectValidation,
  CmyxCalibrationRecord,
  CmyxGeometryClass,
  CmyxGeometryContext,
  CmyxMixRecipe,
  CmyxRecipeComponent,
  CmyxRecipeMode,
  CmyxSampleOrientation,
} from "../types";
import { isTauriRuntime } from "./project-analysis";

export const CMYX_CALIBRATION_SCHEMA_VERSION = 2 as const;
export const CMYX_CALIBRATION_STORAGE_KEY =
  "u1-planner.cmyx-calibration.v2";
export const CMYX_CALIBRATION_LEGACY_STORAGE_KEY =
  "u1-planner.cmyx-calibration.v1";

const MAX_INSTRUMENT_REFERENCE_LENGTH = 512;
const MAX_OPERATOR_NOTES_LENGTH = 2_048;

const defaultLibrary = (): CmyxCalibrationLibraryDocument => ({
  schemaVersion: CMYX_CALIBRATION_SCHEMA_VERSION,
  planningGeometryContext: {
    orientation: "unknown",
    geometryClass: "unknown",
  },
  records: [],
});

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isSha256(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-f]{64}$/i.test(value);
}

function stringArray(value: unknown): value is string[] {
  return (
    Array.isArray(value) &&
    value.length > 0 &&
    value.every((item) => typeof item === "string" && item.trim().length > 0)
  );
}

function calibrationChartSwatchCount(value: unknown) {
  if (value === "quick") return 10;
  if (value === "full") return 26;
  return null;
}

function normalizeCalibrationProjectValidation(
  value: unknown,
): CmyxCalibrationProjectValidation {
  if (
    !isRecord(value) ||
    typeof value.valid !== "boolean" ||
    (value.projectId !== null && typeof value.projectId !== "string") ||
    (value.manifestSha256 !== null && !isSha256(value.manifestSha256)) ||
    !Number.isSafeInteger(value.swatchCount) ||
    Number(value.swatchCount) < 0 ||
    !Array.isArray(value.issues) ||
    !value.issues.every((issue) => typeof issue === "string") ||
    !isRecord(value.fullSpectrum) ||
    typeof value.fullSpectrum.valid !== "boolean"
  ) {
    throw new Error(
      "The native calibration candidate returned an invalid validation report.",
    );
  }
  return value as unknown as CmyxCalibrationProjectValidation;
}

function normalizeCalibrationProjectResult(
  value: unknown,
): CmyxCalibrationProjectResult {
  const expectedSwatchCount = isRecord(value)
    ? calibrationChartSwatchCount(value.chartMode)
    : null;
  if (
    !isRecord(value) ||
    expectedSwatchCount === null ||
    !nonEmptyText(value.projectId) ||
    !nonEmptyText(value.path) ||
    !nonEmptyText(value.fileName) ||
    !Number.isSafeInteger(value.byteSize) ||
    Number(value.byteSize) <= 0 ||
    !isSha256(value.artifactSha256) ||
    !nonEmptyText(value.manifestPath) ||
    !isSha256(value.manifestSha256) ||
    value.swatchCount !== expectedSwatchCount ||
    value.productionQualified !== false ||
    !Array.isArray(value.warnings) ||
    !value.warnings.every((warning) => typeof warning === "string") ||
    !stringArray(value.nextSteps)
  ) {
    throw new Error(
      "The native calibration project generator returned invalid artifact data.",
    );
  }
  const validation = normalizeCalibrationProjectValidation(value.validation);
  if (
    !validation.valid ||
    validation.swatchCount !== expectedSwatchCount ||
    validation.projectId !== value.projectId ||
    validation.manifestSha256 !== value.manifestSha256
  ) {
    throw new Error(
      "The native calibration project report does not match its validated manifest.",
    );
  }
  return {
    ...(value as unknown as CmyxCalibrationProjectResult),
    projectId: String(value.projectId).trim(),
    path: String(value.path).trim(),
    fileName: String(value.fileName).trim(),
    manifestPath: String(value.manifestPath).trim(),
    validation,
  };
}

function nonEmptyText(value: unknown) {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function optionalText(value: unknown, maximum: number) {
  if (value === null || value === undefined) return null;
  if (typeof value !== "string") return undefined;
  const normalized = value.trim();
  if (!normalized) return null;
  return normalized.length <= maximum ? normalized : undefined;
}

function normalizeRfc3339(value: unknown) {
  if (typeof value !== "string") return null;
  const normalized = value.trim();
  const parts = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d+)?(Z|[+-](\d{2}):(\d{2}))$/i.exec(
    normalized,
  );
  if (!parts) return null;
  const [, yearText, monthText, dayText, hourText, minuteText, secondText, , offsetHourText, offsetMinuteText] = parts;
  const year = Number(yearText);
  const month = Number(monthText);
  const day = Number(dayText);
  const hour = Number(hourText);
  const minute = Number(minuteText);
  const second = Number(secondText);
  const leapYear = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const daysInMonth = [
    31,
    leapYear ? 29 : 28,
    31,
    30,
    31,
    30,
    31,
    31,
    30,
    31,
    30,
    31,
  ][month - 1] ?? 0;
  if (
    day < 1 ||
    day > daysInMonth ||
    hour > 23 ||
    minute > 59 ||
    second > 59 ||
    (offsetHourText !== undefined && Number(offsetHourText) > 23) ||
    (offsetMinuteText !== undefined && Number(offsetMinuteText) > 59)
  ) {
    return null;
  }
  const timestamp = Date.parse(normalized);
  return Number.isFinite(timestamp) ? new Date(timestamp).toISOString() : null;
}

function normalizeMeasurementMethod(
  value: unknown,
): CmyxMeasurementMethod | null {
  return value === "instrument_lab" ||
    value === "instrument_srgb" ||
    value === "reliable_manual_srgb" ||
    value === "visual_swatch" ||
    value === "legacy_unverified"
    ? value
    : null;
}

function normalizeProvenance(
  value: unknown,
): CmyxMeasurementProvenance | null {
  if (!isRecord(value)) return null;
  const method = normalizeMeasurementMethod(value.method);
  const instrumentReference = optionalText(
    value.instrumentReference,
    MAX_INSTRUMENT_REFERENCE_LENGTH,
  );
  const operatorNotes = optionalText(
    value.operatorNotes,
    MAX_OPERATOR_NOTES_LENGTH,
  );
  if (
    !method ||
    instrumentReference === undefined ||
    operatorNotes === undefined
  ) {
    return null;
  }
  if (method === "legacy_unverified") {
    return value.measuredAt === null
      ? { measuredAt: null, method, instrumentReference, operatorNotes }
      : null;
  }
  const measuredAt = normalizeRfc3339(value.measuredAt);
  return measuredAt
    ? { measuredAt, method, instrumentReference, operatorNotes }
    : null;
}

function normalizeCustomVariant(
  value: unknown,
  known: readonly string[],
): string | { custom: string } | null {
  if (typeof value === "string" && known.includes(value)) return value;
  if (!isRecord(value) || Object.keys(value).length !== 1) return null;
  const custom = nonEmptyText(value.custom);
  return custom ? { custom } : null;
}

function normalizeOrientation(value: unknown): CmyxSampleOrientation | null {
  return normalizeCustomVariant(
    value,
    ["upright", "flat", "angled", "unknown"] as const,
  ) as CmyxSampleOrientation | null;
}

function normalizeGeometryClass(value: unknown): CmyxGeometryClass | null {
  return normalizeCustomVariant(
    value,
    [
      "calibration_swatch",
      "thin_wall",
      "top_surface",
      "volumetric",
      "unknown",
    ] as const,
  ) as CmyxGeometryClass | null;
}

function isUnknownGeometry(context: CmyxGeometryContext) {
  return (
    context.orientation === "unknown" &&
    context.geometryClass === "unknown"
  );
}

function normalizeGeometryContext(value: unknown): CmyxGeometryContext | null {
  if (!isRecord(value)) return null;
  const orientation = normalizeOrientation(value.orientation);
  const geometryClass = normalizeGeometryClass(value.geometryClass);
  if (!orientation || !geometryClass) return null;

  const orientationUnknown = orientation === "unknown";
  const geometryUnknown = geometryClass === "unknown";
  if (orientationUnknown !== geometryUnknown) return null;
  return { orientation, geometryClass };
}

function normalizeLoadout(value: unknown): CmyxCalibrationLoadout | null {
  if (!isRecord(value)) return null;
  const t1CalibrationId = nonEmptyText(value.t1CalibrationId);
  const t2CalibrationId = nonEmptyText(value.t2CalibrationId);
  const t3CalibrationId = nonEmptyText(value.t3CalibrationId);
  const t4CalibrationId = nonEmptyText(value.t4CalibrationId);
  if (
    !t1CalibrationId ||
    !t2CalibrationId ||
    !t3CalibrationId ||
    !t4CalibrationId ||
    [t1CalibrationId, t2CalibrationId, t3CalibrationId, t4CalibrationId].some(
      (identity) => identity.length > 128,
    ) ||
    new Set([
      t1CalibrationId,
      t2CalibrationId,
      t3CalibrationId,
      t4CalibrationId,
    ]).size !== 4
  ) {
    return null;
  }
  return {
    t1CalibrationId,
    t2CalibrationId,
    t3CalibrationId,
    t4CalibrationId,
  };
}

function normalizeRecipeMode(value: unknown): CmyxRecipeMode | null {
  return value === "solid" ||
    value === "cycle" ||
    value === "ratio" ||
    value === "match" ||
    value === "gradient"
    ? value
    : null;
}

function validComponentCount(mode: CmyxRecipeMode, count: number) {
  if (mode === "solid") return count === 1;
  if (mode === "gradient") return count === 2;
  if (mode === "ratio" || mode === "match") return count >= 2 && count <= 3;
  return count >= 2 && count <= 4;
}

function normalizeRecipeComponent(value: unknown): CmyxRecipeComponent | null {
  if (
    !isRecord(value) ||
    !Number.isInteger(value.slot) ||
    !Number.isInteger(value.weight) ||
    Number(value.slot) < 1 ||
    Number(value.slot) > 4 ||
    Number(value.weight) < 1 ||
    Number(value.weight) > 255
  ) {
    return null;
  }
  return {
    slot: Number(value.slot) as CmyxRecipeComponent["slot"],
    weight: Number(value.weight),
  };
}

function normalizeRecipe(value: unknown): CmyxMixRecipe | null {
  if (!isRecord(value) || !Array.isArray(value.components)) return null;
  const mode = normalizeRecipeMode(value.mode);
  const components = value.components.map(normalizeRecipeComponent);
  if (
    !mode ||
    components.some((component) => component === null) ||
    !validComponentCount(mode, components.length)
  ) {
    return null;
  }
  return { mode, components: components as CmyxRecipeComponent[] };
}

function normalizeStringVariant(value: unknown) {
  if (typeof value === "string" && value.trim()) return value;
  if (!isRecord(value) || Object.keys(value).length !== 1) return null;
  const [key] = Object.keys(value);
  const label = nonEmptyText(value[key]);
  return label ? { [key]: label } : null;
}

function positiveInteger(value: unknown, allowZero = false) {
  return Number.isInteger(value) && Number(value) >= (allowZero ? 0 : 1)
    ? Number(value)
    : null;
}

function normalizeCalibrationContext(
  value: unknown,
): CmyxCalibrationContext | null {
  if (!isRecord(value) || !isRecord(value.printerProfile)) return null;
  const loadoutFingerprint = nonEmptyText(value.loadoutFingerprint);
  const printerModel = nonEmptyText(value.printerProfile.printerModel);
  const printerVariant = nonEmptyText(value.printerProfile.printerVariant);
  const profileId = nonEmptyText(value.printerProfile.profileId);
  const profileFingerprint = nonEmptyText(
    value.printerProfile.profileFingerprint,
  );
  const nozzleDiameterMicrons = positiveInteger(value.nozzleDiameterMicrons);
  const plateLayerHeightMicrons = positiveInteger(
    value.plateLayerHeightMicrons,
  );
  const subdivisionPolicy = normalizeStringVariant(value.subdivisionPolicy);
  const subdivisionFactor = positiveInteger(value.subdivisionFactor, true);
  const effectiveSublayerHeightMicrons = positiveInteger(
    value.effectiveSublayerHeightMicrons,
  );
  const processFingerprint = nonEmptyText(value.processFingerprint);
  const orientation = normalizeOrientation(value.orientation);
  const geometryClass = normalizeGeometryClass(value.geometryClass);
  if (
    !loadoutFingerprint ||
    !printerModel ||
    !printerVariant ||
    !profileId ||
    !profileFingerprint ||
    nozzleDiameterMicrons === null ||
    plateLayerHeightMicrons === null ||
    !subdivisionPolicy ||
    subdivisionFactor === null ||
    effectiveSublayerHeightMicrons === null ||
    !processFingerprint ||
    !orientation ||
    !geometryClass
  ) {
    return null;
  }
  return {
    loadoutFingerprint,
    printerProfile: {
      printerModel,
      printerVariant,
      profileId,
      profileFingerprint,
    },
    nozzleDiameterMicrons,
    plateLayerHeightMicrons,
    subdivisionPolicy,
    subdivisionFactor,
    effectiveSublayerHeightMicrons,
    processFingerprint,
    orientation,
    geometryClass,
  };
}

function normalizeRecord(value: unknown): CmyxCalibrationRecord | null {
  if (!isRecord(value)) return null;
  const id = nonEmptyText(value.id);
  const loadout = normalizeLoadout(value.loadout);
  const context = normalizeCalibrationContext(value.context);
  const recipe = normalizeRecipe(value.recipe);
  const provenance = normalizeProvenance(value.provenance);
  const measuredOutputHex =
    typeof value.measuredOutputHex === "string" &&
    /^#[0-9A-Fa-f]{6}$/.test(value.measuredOutputHex)
      ? value.measuredOutputHex.toUpperCase()
      : null;
  const contextKnown =
    context?.orientation !== "unknown" && context?.geometryClass !== "unknown";
  const expectedFingerprint = loadout
    ? [
        `T1:${loadout.t1CalibrationId}`,
        `T2:${loadout.t2CalibrationId}`,
        `T3:${loadout.t3CalibrationId}`,
        `T4:${loadout.t4CalibrationId}`,
      ].join("|")
    : null;
  if (
    !id ||
    id.length > 128 ||
    !loadout ||
    !context ||
    !contextKnown ||
    context.loadoutFingerprint !== expectedFingerprint ||
    !recipe ||
    !measuredOutputHex ||
    !provenance
  ) {
    return null;
  }
  return { id, loadout, context, recipe, measuredOutputHex, provenance };
}

function normalizeLibrary(
  value: unknown,
): CmyxCalibrationLibraryDocument | null {
  if (
    !isRecord(value) ||
    value.schemaVersion !== CMYX_CALIBRATION_SCHEMA_VERSION ||
    !Array.isArray(value.records)
  ) {
    return null;
  }
  const planningGeometryContext = normalizeGeometryContext(
    value.planningGeometryContext,
  );
  const records = value.records.map(normalizeRecord);
  if (
    !planningGeometryContext ||
    records.some((record) => record === null)
  ) {
    return null;
  }
  const normalizedRecords = records as CmyxCalibrationRecord[];
  if (new Set(normalizedRecords.map((record) => record.id)).size !== records.length) {
    return null;
  }
  const qualifiedRecipeKeys = normalizedRecords.map((record) =>
    JSON.stringify([record.context, record.recipe]),
  );
  if (new Set(qualifiedRecipeKeys).size !== qualifiedRecipeKeys.length) {
    return null;
  }
  return {
    schemaVersion: CMYX_CALIBRATION_SCHEMA_VERSION,
    planningGeometryContext,
    records: normalizedRecords,
  };
}

function migrateLegacyLibrary(
  value: unknown,
): CmyxCalibrationLibraryDocument | null {
  if (
    !isRecord(value) ||
    value.schemaVersion !== 1 ||
    !Array.isArray(value.records)
  ) {
    return null;
  }
  const migratedRecords = value.records.map((record) =>
    isRecord(record)
      ? {
          ...record,
          provenance: {
            measuredAt: null,
            method: "legacy_unverified",
            instrumentReference: null,
            operatorNotes: null,
          },
        }
      : record,
  );
  return normalizeLibrary({
    schemaVersion: CMYX_CALIBRATION_SCHEMA_VERSION,
    planningGeometryContext: value.planningGeometryContext,
    records: migratedRecords,
  });
}

function requireLibrary(value: unknown, source: "native" | "browser") {
  const normalized = normalizeLibrary(value);
  if (!normalized) {
    throw new Error(
      source === "native"
        ? "The native CMY+X calibration library returned invalid data."
        : "The saved browser CMY+X calibration library has an invalid schema. The stored data was left unchanged.",
    );
  }
  return normalized;
}

function normalizeMeasurement(
  value: CmyxCalibrationMeasurementInput,
): CmyxCalibrationMeasurementInput {
  const id = nonEmptyText(value.id);
  const loadout = normalizeLoadout(value.loadout);
  const recipe = normalizeRecipe(value.recipe);
  const geometryContext = normalizeGeometryContext(value.geometryContext);
  const provenance = normalizeProvenance(value.provenance);
  const measuredOutputHex = /^#[0-9A-Fa-f]{6}$/.test(
    value.measuredOutputHex,
  )
    ? value.measuredOutputHex.toUpperCase()
    : null;
  if (
    !id ||
    id.length > 128 ||
    !loadout ||
    !recipe ||
    !geometryContext ||
    isUnknownGeometry(geometryContext) ||
    !measuredOutputHex ||
    !provenance ||
    provenance.method === "legacy_unverified" ||
    value.physicalMeasurementConfirmed !== true
  ) {
    throw new Error(
      "The CMY+X measurement is incomplete, lacks physical measurement provenance, or violates the recipe and geometry constraints.",
    );
  }
  return {
    id,
    loadout,
    recipe,
    measuredOutputHex,
    geometryContext,
    provenance: {
      measuredAt: provenance.measuredAt as string,
      method: provenance.method,
      instrumentReference: provenance.instrumentReference,
      operatorNotes: provenance.operatorNotes,
    },
    physicalMeasurementConfirmed: true,
  };
}

function browserContext(
  loadout: CmyxCalibrationLoadout,
  geometry: CmyxGeometryContext,
): CmyxCalibrationContext {
  return {
    loadoutFingerprint: [
      `T1:${loadout.t1CalibrationId}`,
      `T2:${loadout.t2CalibrationId}`,
      `T3:${loadout.t3CalibrationId}`,
      `T4:${loadout.t4CalibrationId}`,
    ].join("|"),
    printerProfile: {
      printerModel: "Browser demo only",
      printerVariant: "No native printer profile",
      profileId: "browser-demo",
      profileFingerprint: "browser-demo-not-for-native-planning",
    },
    nozzleDiameterMicrons: 400,
    plateLayerHeightMicrons: 80,
    subdivisionPolicy: "subdivide_mix_layer",
    subdivisionFactor: 4,
    effectiveSublayerHeightMicrons: 20,
    processFingerprint: "browser-demo-not-for-native-planning",
    orientation: geometry.orientation,
    geometryClass: geometry.geometryClass,
  };
}

function readBrowserLibrary() {
  const serialized = window.localStorage.getItem(CMYX_CALIBRATION_STORAGE_KEY);
  if (serialized === null) {
    const legacySerialized = window.localStorage.getItem(
      CMYX_CALIBRATION_LEGACY_STORAGE_KEY,
    );
    if (legacySerialized === null) return defaultLibrary();
    let legacyValue: unknown;
    try {
      legacyValue = JSON.parse(legacySerialized);
    } catch {
      throw new Error(
        "The saved browser CMY+X calibration library v1 is not valid JSON. The stored data was left unchanged.",
      );
    }
    const migrated = migrateLegacyLibrary(legacyValue);
    if (!migrated) {
      throw new Error(
        "The saved browser CMY+X calibration library v1 has an invalid schema. The stored data was left unchanged.",
      );
    }
    return saveBrowserLibrary(migrated);
  }
  let value: unknown;
  try {
    value = JSON.parse(serialized);
  } catch {
    throw new Error(
      "The saved browser CMY+X calibration library is not valid JSON. The stored data was left unchanged.",
    );
  }
  return requireLibrary(value, "browser");
}

function saveBrowserLibrary(library: CmyxCalibrationLibraryDocument) {
  window.localStorage.setItem(
    CMYX_CALIBRATION_STORAGE_KEY,
    JSON.stringify(library),
  );
  return library;
}

export async function loadCmyxCalibrationLibrary() {
  if (isTauriRuntime()) {
    return requireLibrary(
      await invoke("load_cmyx_calibration_library"),
      "native",
    );
  }
  return readBrowserLibrary();
}

export async function upsertCmyxCalibrationMeasurement(
  measurement: CmyxCalibrationMeasurementInput,
) {
  const normalized = normalizeMeasurement(measurement);
  if (isTauriRuntime()) {
    return requireLibrary(
      await invoke("upsert_cmyx_calibration_measurement", {
        measurement: normalized,
      }),
      "native",
    );
  }

  const library = readBrowserLibrary();
  const record: CmyxCalibrationRecord = {
    id: normalized.id,
    loadout: normalized.loadout,
    recipe: normalized.recipe,
    measuredOutputHex: normalized.measuredOutputHex,
    provenance: normalized.provenance,
    context: browserContext(normalized.loadout, normalized.geometryContext),
  };
  const conflictingRecord = library.records.find(
    (existing) =>
      existing.id !== record.id &&
      JSON.stringify(existing.context) === JSON.stringify(record.context) &&
      JSON.stringify(existing.recipe) === JSON.stringify(record.recipe),
  );
  if (conflictingRecord) {
    throw new Error(
      `Calibration record ${conflictingRecord.id} already qualifies this exact process and recipe. Reuse that ID to replace it.`,
    );
  }
  return saveBrowserLibrary({
    ...library,
    records: library.records.some((existing) => existing.id === record.id)
      ? library.records.map((existing) =>
          existing.id === record.id ? record : existing,
        )
      : [...library.records, record],
  });
}

export async function deleteCmyxCalibrationRecord(recordId: string) {
  const normalizedId = nonEmptyText(recordId);
  if (!normalizedId) throw new Error("Calibration record ID is required.");
  if (isTauriRuntime()) {
    return requireLibrary(
      await invoke("delete_cmyx_calibration_record", {
        recordId: normalizedId,
      }),
      "native",
    );
  }
  const library = readBrowserLibrary();
  if (!library.records.some((record) => record.id === normalizedId)) {
    throw new Error(`Calibration record ${normalizedId} does not exist.`);
  }
  return saveBrowserLibrary({
    ...library,
    records: library.records.filter((record) => record.id !== normalizedId),
  });
}

export async function setCmyxCalibrationGeometryContext(
  geometryContext: CmyxGeometryContext,
) {
  const normalized = normalizeGeometryContext(geometryContext);
  if (!normalized) throw new Error("Choose a valid planning geometry preset.");
  if (isTauriRuntime()) {
    return requireLibrary(
      await invoke("set_cmyx_calibration_geometry_context", {
        geometryContext: normalized,
      }),
      "native",
    );
  }
  const library = readBrowserLibrary();
  return saveBrowserLibrary({
    ...library,
    planningGeometryContext: normalized,
  });
}

function normalizeCalibrationProjectInput(
  input: CmyxCalibrationProjectInput,
): CmyxCalibrationProjectInput {
  const projectId = nonEmptyText(input.projectId);
  if (
    !projectId ||
    calibrationChartSwatchCount(input.chartMode) === null ||
    projectId.length > 64 ||
    !/^[A-Za-z0-9](?:[A-Za-z0-9_-]{0,63})$/.test(projectId) ||
    input.spoolIds.length !== 4 ||
    input.spoolIds.some((spoolId) => !nonEmptyText(spoolId)) ||
    new Set(input.spoolIds).size !== 4
  ) {
    throw new Error(
      "Calibration project requires a portable project ID and four distinct physical spool identities.",
    );
  }
  return {
    projectId,
    chartMode: input.chartMode,
    spoolIds: input.spoolIds.map((spoolId) => spoolId.trim()) as [
      string,
      string,
      string,
      string,
    ],
  };
}

export async function createRecommendedCmyxCalibrationProject(
  input: CmyxCalibrationProjectInput,
) {
  if (!isTauriRuntime()) {
    throw new Error(
      "Calibration 3MF generation is available only in the native desktop app. Browser demo mode never fabricates local project files.",
    );
  }
  const normalized = normalizeCalibrationProjectInput(input);
  const destinationPath = await save({
    title: "Save CMY+X calibration qualification candidate",
    defaultPath: `${normalized.projectId}.3mf`,
    filters: [{ name: "3MF calibration project", extensions: ["3mf"] }],
  });
  if (typeof destinationPath !== "string") return null;

  const result = normalizeCalibrationProjectResult(
    await invoke("build_cmyx_calibration_project", {
      request: {
        ...normalized,
        destinationPath,
      },
    }),
  );
  if (result.chartMode !== normalized.chartMode) {
    throw new Error(
      "The native calibration project mode does not match the requested chart mode.",
    );
  }
  return result;
}

export async function validateCmyxCalibrationProject(path: string) {
  if (!isTauriRuntime()) {
    throw new Error(
      "Calibration 3MF validation is available only in the native desktop app.",
    );
  }
  const normalizedPath = nonEmptyText(path);
  if (!normalizedPath) throw new Error("Calibration project path is required.");
  return normalizeCalibrationProjectValidation(
    await invoke("validate_cmyx_calibration_project", {
      path: normalizedPath,
    }),
  );
}
