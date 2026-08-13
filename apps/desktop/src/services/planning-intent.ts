import type { PrintingSetup } from "./printing-setup";

export const PLANNING_INTENT_SCHEMA_VERSION = 3 as const;
export const PLANNING_INTENT_STORAGE_KEY =
  "u1-planner.project-planning-intent.v3";
export const LEGACY_PLANNING_INTENT_V2_STORAGE_KEY =
  "u1-planner.project-planning-intent.v2";
export const LEGACY_PLANNING_INTENT_STORAGE_KEY =
  "u1-planner.project-planning-intent.v1";

export type DefaultPlanningStrategy = "auto" | "cmyx" | "direct";

export interface PlanningIntent {
  defaultStrategy: DefaultPlanningStrategy;
  a1MiniEnabled: boolean;
  allowU1CrossSourceRepacking: boolean;
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
      };
    }
  } catch {
    return null;
  }

  const legacyV2 = loadLegacyPlanningIntentV2(storage);
  if (legacyV2 !== null) {
    return {
      defaultStrategy: legacyV2.defaultStrategy,
      a1MiniEnabled: legacyV2.a1MiniEnabled,
      allowU1CrossSourceRepacking: false,
    };
  }

  const legacy = loadLegacyPlanningIntent(storage);
  return legacy
    ? {
        defaultStrategy: legacy.defaultStrategy,
        a1MiniEnabled: legacy.a1MiniEnabled,
        allowU1CrossSourceRepacking: false,
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
      right?.allowU1CrossSourceRepacking
  );
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
    typeof value.allowU1CrossSourceRepacking === "boolean"
  );
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
] as const;

const planningIntentKeys = [
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
