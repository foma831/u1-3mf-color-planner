export const PRINTING_SETUP_SCHEMA_VERSION = 1 as const;
export const PRINTING_SETUP_STORAGE_KEY = "u1-planner.printing-setup.v1";

export interface PrintingSetup {
  schemaVersion: typeof PRINTING_SETUP_SCHEMA_VERSION;
  primaryPrinter: "u1";
  secondaryPrinter: "a1-mini" | null;
}

function isPrintingSetup(value: unknown): value is PrintingSetup {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return false;
  }
  const candidate = value as Record<string, unknown>;
  const keys = Object.keys(candidate);
  return (
    keys.length === 3 &&
    keys.every((key) =>
      ["schemaVersion", "primaryPrinter", "secondaryPrinter"].includes(key),
    ) &&
    candidate.schemaVersion === PRINTING_SETUP_SCHEMA_VERSION &&
    candidate.primaryPrinter === "u1" &&
    (candidate.secondaryPrinter === null ||
      candidate.secondaryPrinter === "a1-mini")
  );
}

export function loadPrintingSetup(
  storage: Storage | null,
): PrintingSetup | null {
  if (!storage) return null;
  try {
    const serialized = storage.getItem(PRINTING_SETUP_STORAGE_KEY);
    if (!serialized) return null;
    const parsed: unknown = JSON.parse(serialized);
    return isPrintingSetup(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

export function savePrintingSetup(
  storage: Storage | null,
  setup: PrintingSetup,
): boolean {
  if (!storage || !isPrintingSetup(setup)) return false;
  try {
    storage.setItem(PRINTING_SETUP_STORAGE_KEY, JSON.stringify(setup));
    return true;
  } catch {
    return false;
  }
}
