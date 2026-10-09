import type { PrintingSetup } from "./printing-setup";

export const PLANNING_INTENT_SCHEMA_VERSION = 5 as const;
export const PLANNING_INTENT_STORAGE_KEY =
  "u1-planner.project-planning-intent.v5";
export const LEGACY_PLANNING_INTENT_V4_STORAGE_KEY =
  "u1-planner.project-planning-intent.v4";
export const LEGACY_PLANNING_INTENT_V3_STORAGE_KEY =
  "u1-planner.project-planning-intent.v3";
export const LEGACY_PLANNING_INTENT_V2_STORAGE_KEY =
  "u1-planner.project-planning-intent.v2";
export const LEGACY_PLANNING_INTENT_STORAGE_KEY =
  "u1-planner.project-planning-intent.v1";

export type DefaultPlanningStrategy = "auto" | "cmyx" | "direct";
export type DedicatedSupportUsage =
  | "interface-only"
  | "body-and-interface";

export interface PlanningIntent {
  defaultStrategy: DefaultPlanningStrategy;
  a1MiniEnabled: boolean;
  allowU1CrossSourceRepacking: boolean;
  dedicatedSupportSpoolId: string | null;
  dedicatedSupportUsage: DedicatedSupportUsage;
}

interface LegacyPlanningIntentValues {
  defaultStrategy: DefaultPlanningStrategy;
  a1MiniEnabled: boolean;
}

export interface LegacyPlanningIntentV1 extends LegacyPlanningIntentValues {
  schemaVersion: 1;
  currentT4SpoolId: string | null;
  currentA1SpoolId: string | null;
}

interface LegacyPlanningIntentV2 extends LegacyPlanningIntentValues {
  schemaVersion: 2;
}

interface LegacyPlanningIntentV3 extends LegacyPlanningIntentValues {
  schemaVersion: 3;
  allowU1CrossSourceRepacking: boolean;
}

interface LegacyPlanningIntentV4 extends LegacyPlanningIntentValues {
  schemaVersion: 4;
  allowU1CrossSourceRepacking: boolean;
  dedicatedSupportSpoolId: string | null;
}

interface StoredPlanningIntent extends PlanningIntent {
  schemaVersion: typeof PLANNING_INTENT_SCHEMA_VERSION;
}

export function createDefaultPlanningIntent(
  setup: PrintingSetup | null,
): PlanningIntent {
  return {
    defaultStrategy: "auto",
    a1MiniEnabled: setup?.secondaryPrinter === "a1-mini",
    allowU1CrossSourceRepacking: false,
    dedicatedSupportSpoolId: null,
    dedicatedSupportUsage: "interface-only",
  };
}

export function constrainPlanningIntentToEquipment(
  intent: PlanningIntent,
  setup: PrintingSetup | null,
): PlanningIntent {
  if (
    setup === null ||
    setup.secondaryPrinter === "a1-mini" ||
    !intent.a1MiniEnabled
  ) {
    return intent;
  }
  return {
    ...intent,
    a1MiniEnabled: false,
  };
}

export function loadLegacyPlanningIntent(
  storage: Storage | null,
): LegacyPlanningIntentV1 | null {
  if (!storage) return null;
  try {
    const serialized = storage.getItem(LEGACY_PLANNING_INTENT_STORAGE_KEY);
    if (serialized === null) return null;
    const parsed: unknown = JSON.parse(serialized);
    return isLegacyPlanningIntent(parsed)
      ? normalizeLegacyIntent(parsed)
      : null;
  } catch {
    return null;
  }
}

export function loadPlanningIntent(
  storage: Storage | null,
): PlanningIntent | null {
  if (!storage) return null;
  try {
    const serialized = storage.getItem(PLANNING_INTENT_STORAGE_KEY);
    if (serialized !== null) {
      const parsed: unknown = JSON.parse(serialized);
      if (!isStoredPlanningIntent(parsed)) return null;
      return {
        defaultStrategy: parsed.defaultStrategy,
        a1MiniEnabled: parsed.a1MiniEnabled,
        allowU1CrossSourceRepacking: parsed.allowU1CrossSourceRepacking,
        dedicatedSupportSpoolId: normalizeOptionalSpoolId(
          parsed.dedicatedSupportSpoolId,
        ),
        dedicatedSupportUsage: parsed.dedicatedSupportUsage,
      };
    }
  } catch {
    return null;
  }

  const legacyV4 = loadLegacyPlanningIntentV4(storage);
  if (legacyV4 !== null) {
    return {
      defaultStrategy: legacyV4.defaultStrategy,
      a1MiniEnabled: legacyV4.a1MiniEnabled,
      allowU1CrossSourceRepacking: legacyV4.allowU1CrossSourceRepacking,
      dedicatedSupportSpoolId: normalizeOptionalSpoolId(
        legacyV4.dedicatedSupportSpoolId,
      ),
      dedicatedSupportUsage: "interface-only",
    };
  }

  const legacyV3 = loadLegacyPlanningIntentV3(storage);
  if (legacyV3 !== null) {
    return {
      defaultStrategy: legacyV3.defaultStrategy,
      a1MiniEnabled: legacyV3.a1MiniEnabled,
      allowU1CrossSourceRepacking: legacyV3.allowU1CrossSourceRepacking,
      dedicatedSupportSpoolId: null,
      dedicatedSupportUsage: "interface-only",
    };
  }

  const legacyV2 = loadLegacyPlanningIntentV2(storage);
  if (legacyV2 !== null) {
    return {
      defaultStrategy: legacyV2.defaultStrategy,
      a1MiniEnabled: legacyV2.a1MiniEnabled,
      allowU1CrossSourceRepacking: false,
      dedicatedSupportSpoolId: null,
      dedicatedSupportUsage: "interface-only",
    };
  }

  const legacy = loadLegacyPlanningIntent(storage);
  return legacy
    ? {
        defaultStrategy: legacy.defaultStrategy,
        a1MiniEnabled: legacy.a1MiniEnabled,
        allowU1CrossSourceRepacking: false,
        dedicatedSupportSpoolId: null,
        dedicatedSupportUsage: "interface-only",
      }
    : null;
}

export function savePlanningIntent(
  storage: Storage | null,
  intent: PlanningIntent,
): boolean {
  if (!storage || !isPlanningIntent(intent)) return false;
  try {
    const stored: StoredPlanningIntent = {
      schemaVersion: PLANNING_INTENT_SCHEMA_VERSION,
      ...intent,
    };
    storage.setItem(PLANNING_INTENT_STORAGE_KEY, JSON.stringify(stored));
    return true;
  } catch {
    return false;
  }
}

export function planningIntentEquals(
  left: PlanningIntent | null,
  right: PlanningIntent | null,
) {
  return (
    left?.defaultStrategy === right?.defaultStrategy &&
    left?.a1MiniEnabled === right?.a1MiniEnabled &&
    left?.allowU1CrossSourceRepacking ===
      right?.allowU1CrossSourceRepacking &&
    left?.dedicatedSupportSpoolId === right?.dedicatedSupportSpoolId &&
    left?.dedicatedSupportUsage === right?.dedicatedSupportUsage
  );
}

function loadLegacyPlanningIntentV4(
  storage: Storage,
): LegacyPlanningIntentV4 | null {
  try {
    const serialized = storage.getItem(LEGACY_PLANNING_INTENT_V4_STORAGE_KEY);
    if (serialized === null) return null;
    const parsed: unknown = JSON.parse(serialized);
    if (!isRecord(parsed) || !hasExactKeys(parsed, legacyV4IntentKeys)) {
      return null;
    }
    return parsed.schemaVersion === 4 &&
      hasLegacyPlanningIntentValues(parsed) &&
      typeof parsed.allowU1CrossSourceRepacking === "boolean" &&
      isOptionalSpoolId(parsed.dedicatedSupportSpoolId)
      ? (parsed as unknown as LegacyPlanningIntentV4)
      : null;
  } catch {
    return null;
  }
}

function loadLegacyPlanningIntentV2(
  storage: Storage,
): LegacyPlanningIntentV2 | null {
  try {
    const serialized = storage.getItem(LEGACY_PLANNING_INTENT_V2_STORAGE_KEY);
    if (serialized === null) return null;
    const parsed: unknown = JSON.parse(serialized);
    return isLegacyPlanningIntentV2(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

function loadLegacyPlanningIntentV3(
  storage: Storage,
): LegacyPlanningIntentV3 | null {
  try {
    const serialized = storage.getItem(LEGACY_PLANNING_INTENT_V3_STORAGE_KEY);
    if (serialized === null) return null;
    const parsed: unknown = JSON.parse(serialized);
    if (!isRecord(parsed) || !hasExactKeys(parsed, legacyV3IntentKeys)) {
      return null;
    }
    return parsed.schemaVersion === 3 &&
      hasLegacyPlanningIntentValues(parsed) &&
      typeof parsed.allowU1CrossSourceRepacking === "boolean"
      ? (parsed as unknown as LegacyPlanningIntentV3)
      : null;
  } catch {
    return null;
  }
}

function isStoredPlanningIntent(value: unknown): value is StoredPlanningIntent {
  if (!isRecord(value) || !hasExactKeys(value, storedIntentKeys)) return false;
  return (
    value.schemaVersion === PLANNING_INTENT_SCHEMA_VERSION &&
    hasPlanningIntentValues(value)
  );
}

function isLegacyPlanningIntent(
  value: unknown,
): value is LegacyPlanningIntentV1 {
  if (!isRecord(value) || !hasExactKeys(value, legacyIntentKeys)) return false;
  return (
    value.schemaVersion === 1 &&
    hasLegacyPlanningIntentValues(value) &&
    isOptionalSpoolId(value.currentT4SpoolId) &&
    isOptionalSpoolId(value.currentA1SpoolId)
  );
}

function isLegacyPlanningIntentV2(
  value: unknown,
): value is LegacyPlanningIntentV2 {
  if (!isRecord(value) || !hasExactKeys(value, legacyV2IntentKeys)) {
    return false;
  }
  return value.schemaVersion === 2 && hasLegacyPlanningIntentValues(value);
}

function normalizeLegacyIntent(
  value: LegacyPlanningIntentV1,
): LegacyPlanningIntentV1 {
  return {
    ...value,
    currentT4SpoolId: normalizeOptionalSpoolId(value.currentT4SpoolId),
    currentA1SpoolId: normalizeOptionalSpoolId(value.currentA1SpoolId),
  };
}

function isPlanningIntent(value: unknown): value is PlanningIntent {
  return (
    isRecord(value) &&
    hasExactKeys(value, planningIntentKeys) &&
    hasPlanningIntentValues(value)
  );
}

function hasPlanningIntentValues(
  value: Record<string, unknown>,
): value is Record<string, unknown> & PlanningIntent {
  return (
    hasLegacyPlanningIntentValues(value) &&
    typeof value.allowU1CrossSourceRepacking === "boolean" &&
    isOptionalSpoolId(value.dedicatedSupportSpoolId) &&
    isDedicatedSupportUsage(value.dedicatedSupportUsage)
  );
}

function isDedicatedSupportUsage(
  value: unknown,
): value is DedicatedSupportUsage {
  return value === "interface-only" || value === "body-and-interface";
}

function hasLegacyPlanningIntentValues(
  value: Record<string, unknown>,
): value is Record<string, unknown> & LegacyPlanningIntentValues {
  return (
    ["auto", "cmyx", "direct"].includes(String(value.defaultStrategy)) &&
    typeof value.a1MiniEnabled === "boolean"
  );
}

function isOptionalSpoolId(value: unknown): value is string | null {
  return value === null || (typeof value === "string" && value.trim() !== "");
}

function normalizeOptionalSpoolId(value: string | null) {
  return value === null ? null : value.trim();
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

const storedIntentKeys = [
  "schemaVersion",
  "defaultStrategy",
  "a1MiniEnabled",
  "allowU1CrossSourceRepacking",
  "dedicatedSupportSpoolId",
  "dedicatedSupportUsage",
] as const;

const planningIntentKeys = [
  "defaultStrategy",
  "a1MiniEnabled",
  "allowU1CrossSourceRepacking",
  "dedicatedSupportSpoolId",
  "dedicatedSupportUsage",
] as const;

const legacyV4IntentKeys = [
  "schemaVersion",
  "defaultStrategy",
  "a1MiniEnabled",
  "allowU1CrossSourceRepacking",
  "dedicatedSupportSpoolId",
] as const;

const legacyV3IntentKeys = [
  "schemaVersion",
  "defaultStrategy",
  "a1MiniEnabled",
  "allowU1CrossSourceRepacking",
] as const;

const legacyV2IntentKeys = [
  "schemaVersion",
  "defaultStrategy",
  "a1MiniEnabled",
] as const;

const legacyIntentKeys = [
  "schemaVersion",
  "defaultStrategy",
  "a1MiniEnabled",
  "currentT4SpoolId",
  "currentA1SpoolId",
] as const;
