import { invoke } from "@tauri-apps/api/core";

import type {
  FilamentLibraryDocument,
  PhysicalSpool,
  SpoolMaterial,
} from "../types";
import { isTauriRuntime } from "./project-analysis";

export const FILAMENT_LIBRARY_SCHEMA_VERSION = 3 as const;
export const FILAMENT_LIBRARY_STORAGE_KEY =
  "u1-planner.filament-library.v3";
export const LEGACY_FILAMENT_LIBRARY_V2_STORAGE_KEY =
  "u1-planner.filament-library.v2";
export const LEGACY_FILAMENT_LIBRARY_STORAGE_KEY =
  "u1-planner.filament-library.v1";

const MAX_NOZZLE_TEMPERATURE_C = 500;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isMaterial(value: unknown): value is SpoolMaterial {
  return value === "PLA" || value === "PETG" || value === "PVA";
}

function optionalText(value: unknown) {
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function validStoredText(value: unknown, minimum: number, maximum: number) {
  if (typeof value !== "string") return false;
  const trimmed = value.trim();
  const bytes = new TextEncoder().encode(trimmed).byteLength;
  return (
    bytes >= minimum &&
    bytes <= maximum &&
    !/[\u0000-\u001F\u007F]/u.test(trimmed)
  );
}

function validOptionalStoredText(value: unknown, maximum: number) {
  return value === undefined || validStoredText(value, 0, maximum);
}

function optionalTemperature(value: unknown) {
  return typeof value === "number" && Number.isInteger(value)
    ? value
    : undefined;
}

export function calibrationIdentityFor(
  spool: Pick<PhysicalSpool, "id" | "calibrationIdentity">,
) {
  return optionalText(spool.calibrationIdentity) ?? spool.id.trim();
}

export function createCalibrationIdentity() {
  if (typeof globalThis.crypto?.randomUUID !== "function") {
    throw new Error(
      "This runtime cannot create a safe physical calibration identity.",
    );
  }
  return `spool-calibration:${globalThis.crypto.randomUUID()}`;
}

function sameCalibrationBatch(left: PhysicalSpool, right: PhysicalSpool) {
  return (
    optionalText(left.batchLot) === optionalText(right.batchLot) &&
    optionalText(left.calibrationReference) ===
      optionalText(right.calibrationReference)
  );
}

function withReconciledCalibrationIdentity(
  spool: PhysicalSpool,
  previous?: PhysicalSpool,
): PhysicalSpool {
  if (spool.source === "built-in") {
    return { ...spool, calibrationIdentity: spool.id };
  }
  if (previous && sameCalibrationBatch(spool, previous)) {
    return {
      ...spool,
      calibrationIdentity: calibrationIdentityFor(previous),
    };
  }
  return {
    ...spool,
    calibrationIdentity:
      previous || !optionalText(spool.calibrationIdentity)
        ? createCalibrationIdentity()
        : calibrationIdentityFor(spool),
  };
}

function normalizeSpool(
  value: unknown,
  sourceSchemaVersion: 1 | 2 | 3,
): PhysicalSpool | null {
  if (!isRecord(value)) return null;
  const minimumTemperature = optionalTemperature(value.minNozzleTemperatureC);
  const maximumTemperature = optionalTemperature(value.maxNozzleTemperatureC);
  if (
    typeof value.id !== "string" ||
    !/^[A-Za-z0-9_.:-]{1,128}$/.test(value.id.trim()) ||
    (value.source !== "built-in" && value.source !== "user") ||
    !validStoredText(value.name, 1, 256) ||
    !validStoredText(value.colorName, 1, 256) ||
    typeof value.hex !== "string" ||
    !/^#[0-9A-Fa-f]{6}$/.test(value.hex) ||
    !isMaterial(value.material) ||
    (value.colorBasis !== "Measured" && value.colorBasis !== "Nominal") ||
    typeof value.available !== "boolean" ||
    !validOptionalStoredText(value.sku, 512) ||
    !validOptionalStoredText(value.profile, 512) ||
    !validOptionalStoredText(value.vendor, 256) ||
    !validOptionalStoredText(value.productLine, 256) ||
    !validOptionalStoredText(value.opticalDescriptor, 512) ||
    !validOptionalStoredText(value.batchLot, 512) ||
    !validOptionalStoredText(value.calibrationReference, 512) ||
    !validOptionalStoredText(value.notes, 4096) ||
    (value.minNozzleTemperatureC !== undefined &&
      minimumTemperature === undefined) ||
    (value.maxNozzleTemperatureC !== undefined &&
      maximumTemperature === undefined) ||
    (minimumTemperature !== undefined &&
      (minimumTemperature < 0 ||
        minimumTemperature > MAX_NOZZLE_TEMPERATURE_C)) ||
    (maximumTemperature !== undefined &&
      (maximumTemperature < 0 ||
        maximumTemperature > MAX_NOZZLE_TEMPERATURE_C)) ||
    (minimumTemperature !== undefined &&
      maximumTemperature !== undefined &&
      minimumTemperature > maximumTemperature) ||
    (sourceSchemaVersion >= 2 &&
      !validStoredText(value.calibrationIdentity, 1, 128))
  ) {
    return null;
  }

  return {
    id: value.id.trim(),
    calibrationIdentity:
      sourceSchemaVersion === 1
        ? value.id.trim()
        : (value.calibrationIdentity as string).trim(),
    source: value.source,
    name: (value.name as string).trim(),
    colorName: (value.colorName as string).trim(),
    hex: value.hex.toUpperCase(),
    material: value.material,
    ...(optionalText(value.sku) ? { sku: optionalText(value.sku) } : {}),
    ...(optionalText(value.profile)
      ? { profile: optionalText(value.profile) }
      : {}),
    ...(optionalText(value.vendor) ? { vendor: optionalText(value.vendor) } : {}),
    ...(optionalText(value.productLine)
      ? { productLine: optionalText(value.productLine) }
      : {}),
    ...(optionalText(value.opticalDescriptor)
      ? { opticalDescriptor: optionalText(value.opticalDescriptor) }
      : {}),
    ...(minimumTemperature !== undefined
      ? { minNozzleTemperatureC: minimumTemperature }
      : {}),
    ...(maximumTemperature !== undefined
      ? { maxNozzleTemperatureC: maximumTemperature }
      : {}),
    ...(optionalText(value.batchLot)
      ? { batchLot: optionalText(value.batchLot) }
      : {}),
    ...(optionalText(value.calibrationReference)
      ? { calibrationReference: optionalText(value.calibrationReference) }
      : {}),
    ...(optionalText(value.notes) ? { notes: optionalText(value.notes) } : {}),
    colorBasis: value.colorBasis,
    available: value.available,
  };
}

function normalizeDocument(value: unknown): FilamentLibraryDocument | null {
  if (
    !isRecord(value) ||
    (value.schemaVersion !== 1 &&
      value.schemaVersion !== 2 &&
      value.schemaVersion !== FILAMENT_LIBRARY_SCHEMA_VERSION) ||
    !Array.isArray(value.spools)
  ) {
    return null;
  }

  const spools: PhysicalSpool[] = [];
  const spoolIds = new Set<string>();
  const calibrationIdentities = new Set<string>();
  for (const spool of value.spools) {
    const normalized = normalizeSpool(spool, value.schemaVersion);
    if (
      !normalized ||
      spoolIds.has(normalized.id) ||
      calibrationIdentities.has(calibrationIdentityFor(normalized))
    ) {
      return null;
    }
    spoolIds.add(normalized.id);
    calibrationIdentities.add(calibrationIdentityFor(normalized));
    spools.push(normalized);
  }
  return { schemaVersion: FILAMENT_LIBRARY_SCHEMA_VERSION, spools };
}

function fallbackDocument(spools: PhysicalSpool[]): FilamentLibraryDocument {
  return {
    schemaVersion: FILAMENT_LIBRARY_SCHEMA_VERSION,
    spools: spools.map((spool) => ({
      ...spool,
      calibrationIdentity: calibrationIdentityFor(spool),
    })),
  };
}

/**
 * Keeps the permanent built-in catalogue authoritative while applying its
 * saved availability flags and retaining saved user spools.
 */
export function mergeFilamentLibrary(
  defaults: PhysicalSpool[],
  saved: PhysicalSpool[],
) {
  if (defaults.length === 0) return saved.map((spool) => ({ ...spool }));

  const builtInDefaults = defaults.filter(
    (spool) => spool.source === "built-in",
  );
  const savedById = new Map(saved.map((spool) => [spool.id, spool]));
  const defaultIds = new Set(builtInDefaults.map((spool) => spool.id));
  const merged: PhysicalSpool[] = builtInDefaults.map((spool) => {
    const savedSpool = savedById.get(spool.id);
    if (!savedSpool) return { ...spool, calibrationIdentity: spool.id };
    return {
      ...spool,
      calibrationIdentity: spool.id,
      available: savedSpool.available,
    };
  });

  for (const spool of saved) {
    if (!defaultIds.has(spool.id) && spool.source === "user") {
      merged.push({ ...spool });
    }
  }
  return merged;
}

export async function loadFilamentLibrary(
  defaults: PhysicalSpool[] = [],
): Promise<FilamentLibraryDocument> {
  if (isTauriRuntime()) {
    const result = await invoke<FilamentLibraryDocument>(
      "load_filament_library",
    );
    const normalized = normalizeDocument(result);
    if (!normalized) {
      throw new Error("The native filament library returned invalid data.");
    }
    return normalized;
  }

  const currentSerialized = window.localStorage.getItem(
    FILAMENT_LIBRARY_STORAGE_KEY,
  );
  const legacyV2Serialized = window.localStorage.getItem(
    LEGACY_FILAMENT_LIBRARY_V2_STORAGE_KEY,
  );
  const legacySerialized = window.localStorage.getItem(
    LEGACY_FILAMENT_LIBRARY_STORAGE_KEY,
  );
  const serialized = currentSerialized ?? legacyV2Serialized ?? legacySerialized;
  if (serialized === null) return fallbackDocument(defaults);

  let storedValue: unknown;
  try {
    storedValue = JSON.parse(serialized);
  } catch {
    throw new Error(
      "The saved browser filament library is not valid JSON. The stored data was left unchanged.",
    );
  }
  const normalized = normalizeDocument(storedValue);
  if (!normalized) {
    throw new Error(
      "The saved browser filament library has an invalid schema. The stored data was left unchanged.",
    );
  }
  const migrated = {
    schemaVersion: FILAMENT_LIBRARY_SCHEMA_VERSION,
    spools: mergeFilamentLibrary(defaults, normalized.spools),
  };
  if (currentSerialized === null) {
    // Publish v3 before leaving the v1/v2 recovery copy in place.
    window.localStorage.setItem(
      FILAMENT_LIBRARY_STORAGE_KEY,
      JSON.stringify(migrated),
    );
  }
  return migrated;
}

export async function saveFilamentLibrary(
  spools: PhysicalSpool[],
): Promise<FilamentLibraryDocument> {
  let previousSpools: PhysicalSpool[] = [];
  if (!isTauriRuntime()) {
    const serialized = window.localStorage.getItem(
      FILAMENT_LIBRARY_STORAGE_KEY,
    );
    if (serialized !== null) {
      try {
        previousSpools = normalizeDocument(JSON.parse(serialized))?.spools ?? [];
      } catch {
        // The write below replaces only v3 after the caller has explicitly
        // supplied the complete in-memory catalogue.
      }
    }
  }
  const previousById = new Map(previousSpools.map((spool) => [spool.id, spool]));
  const reconciled = spools.map((spool) =>
    withReconciledCalibrationIdentity(spool, previousById.get(spool.id)),
  );
  const library = fallbackDocument(reconciled);
  if (isTauriRuntime()) {
    const result = await invoke<FilamentLibraryDocument>(
      "save_filament_library",
      { library },
    );
    const normalized = normalizeDocument(result);
    if (!normalized) {
      throw new Error("The native filament library returned invalid data.");
    }
    return normalized;
  }

  window.localStorage.setItem(
    FILAMENT_LIBRARY_STORAGE_KEY,
    JSON.stringify(library),
  );
  return library;
}
