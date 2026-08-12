import type { LoadedToolhead, ToolheadId } from "../types";
import {
  loadLegacyPlanningIntent,
  type LegacyPlanningIntentV1,
} from "./planning-intent";
import type { PrintingSetup } from "./printing-setup";

export const PRINTER_LOADOUT_SCHEMA_VERSION = 1 as const;
export const PRINTER_LOADOUT_STORAGE_KEY = "u1-planner.printer-loadout.v1";

export type PrinterLoadoutUpdateSource = "manual" | "print-run";

export interface PrinterLoadoutProfile {
  schemaVersion: typeof PRINTER_LOADOUT_SCHEMA_VERSION;
  currentLoadout: LoadedToolhead[];
  currentA1SpoolId: string | null;
  updatedFrom: PrinterLoadoutUpdateSource;
}

export function createEmptyPrinterLoadout(
  updatedFrom: PrinterLoadoutUpdateSource = "manual",
): PrinterLoadoutProfile {
  return {
    schemaVersion: PRINTER_LOADOUT_SCHEMA_VERSION,
    currentLoadout: [],
    currentA1SpoolId: null,
    updatedFrom,
  };
}

export function loadPrinterLoadout(
  storage: Storage | null,
): PrinterLoadoutProfile | null {
  if (!storage) return null;
  try {
    const serialized = storage.getItem(PRINTER_LOADOUT_STORAGE_KEY);
    if (serialized !== null) {
      const parsed: unknown = JSON.parse(serialized);
      return isPrinterLoadoutProfile(parsed)
        ? normalizePrinterLoadout(parsed)
        : null;
    }
  } catch {
    return null;
  }

  const legacy = loadLegacyPlanningIntent(storage);
  if (!legacy) return null;
  const migrated = migrateLegacyPlanningIntentLoadout(legacy);
  return isPrinterLoadoutProfile(migrated) ? migrated : null;
}

export function savePrinterLoadout(
  storage: Storage | null,
  profile: PrinterLoadoutProfile,
): boolean {
  if (!storage || !isPrinterLoadoutProfile(profile)) return false;
  try {
    storage.setItem(
      PRINTER_LOADOUT_STORAGE_KEY,
      JSON.stringify(normalizePrinterLoadout(profile)),
    );
    return true;
  } catch {
    return false;
  }
}

export function printerLoadoutEquals(
  left: PrinterLoadoutProfile | null,
  right: PrinterLoadoutProfile | null,
) {
  if (left === null || right === null) return left === right;
  if (left.currentA1SpoolId !== right.currentA1SpoolId) return false;
  if (left.currentLoadout.length !== right.currentLoadout.length) return false;

  const rightByToolhead = new Map(
    right.currentLoadout.map((entry) => [entry.toolhead, entry.spoolId]),
  );
  return left.currentLoadout.every(
    (entry) => rightByToolhead.get(entry.toolhead) === entry.spoolId,
  );
}

export function constrainPrinterLoadoutToEquipment(
  profile: PrinterLoadoutProfile,
  setup: PrintingSetup | null,
): PrinterLoadoutProfile {
  if (
    setup === null ||
    setup.secondaryPrinter === "a1-mini" ||
    profile.currentA1SpoolId === null
  ) {
    return profile;
  }
  return {
    ...profile,
    currentA1SpoolId: null,
  };
}

function migrateLegacyPlanningIntentLoadout(
  legacy: LegacyPlanningIntentV1,
): PrinterLoadoutProfile {
  return {
    schemaVersion: PRINTER_LOADOUT_SCHEMA_VERSION,
    currentLoadout: legacy.currentT4SpoolId
      ? [{ toolhead: "T4", spoolId: legacy.currentT4SpoolId }]
      : [],
    currentA1SpoolId: legacy.currentA1SpoolId,
    updatedFrom: "manual",
  };
}

function isPrinterLoadoutProfile(
  value: unknown,
): value is PrinterLoadoutProfile {
  if (!isRecord(value) || !hasExactKeys(value, profileKeys)) return false;
  if (
    value.schemaVersion !== PRINTER_LOADOUT_SCHEMA_VERSION ||
    !Array.isArray(value.currentLoadout) ||
    !isOptionalSpoolId(value.currentA1SpoolId) ||
    (value.updatedFrom !== "manual" && value.updatedFrom !== "print-run")
  ) {
    return false;
  }

  const seenToolheads = new Set<ToolheadId>();
  const seenSpools = new Set<string>();
  for (const entry of value.currentLoadout) {
    if (!isLoadedToolhead(entry)) return false;
    if (seenToolheads.has(entry.toolhead) || seenSpools.has(entry.spoolId)) {
      return false;
    }
    seenToolheads.add(entry.toolhead);
    seenSpools.add(entry.spoolId);
  }
  return (
    value.currentA1SpoolId === null || !seenSpools.has(value.currentA1SpoolId)
  );
}

function isLoadedToolhead(value: unknown): value is LoadedToolhead {
  return (
    isRecord(value) &&
    hasExactKeys(value, loadedToolheadKeys) &&
    isToolheadId(value.toolhead) &&
    isSpoolId(value.spoolId)
  );
}

function isToolheadId(value: unknown): value is ToolheadId {
  return (
    typeof value === "string" &&
    (["T1", "T2", "T3", "T4"] as const).includes(value as ToolheadId)
  );
}

function isOptionalSpoolId(value: unknown): value is string | null {
  return value === null || isSpoolId(value);
}

function isSpoolId(value: unknown): value is string {
  return typeof value === "string" && value !== "" && value.trim() === value;
}

function normalizePrinterLoadout(
  profile: PrinterLoadoutProfile,
): PrinterLoadoutProfile {
  return {
    ...profile,
    currentLoadout: profile.currentLoadout
      .map((entry) => ({ ...entry }))
      .sort(
        (left, right) =>
          toolheadOrder[left.toolhead] - toolheadOrder[right.toolhead],
      ),
  };
}

function hasExactKeys(
  value: Record<string, unknown>,
  expected: readonly string[],
) {
  const keys = Object.keys(value);
  return (
    keys.length === expected.length &&
    keys.every((key) => expected.includes(key))
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

const profileKeys = [
  "schemaVersion",
  "currentLoadout",
  "currentA1SpoolId",
  "updatedFrom",
] as const;

const loadedToolheadKeys = ["toolhead", "spoolId"] as const;

const toolheadOrder: Record<ToolheadId, number> = {
  T1: 0,
  T2: 1,
  T3: 2,
  T4: 3,
};
