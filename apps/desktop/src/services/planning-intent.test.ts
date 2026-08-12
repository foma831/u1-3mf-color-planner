// @vitest-environment jsdom

import { afterEach, describe, expect, it } from "vitest";

import type { PrintingSetup } from "./printing-setup";
import {
  constrainPlanningIntentToEquipment,
  createDefaultPlanningIntent,
  LEGACY_PLANNING_INTENT_STORAGE_KEY,
  loadLegacyPlanningIntent,
  loadPlanningIntent,
  PLANNING_INTENT_STORAGE_KEY,
  planningIntentEquals,
  savePlanningIntent,
  type PlanningIntent,
} from "./planning-intent";

const twoPrinterSetup: PrintingSetup = {
  schemaVersion: 1,
  primaryPrinter: "u1",
  secondaryPrinter: "a1-mini",
};

const intent: PlanningIntent = {
  defaultStrategy: "direct",
  a1MiniEnabled: true,
};

const legacyIntent = {
  schemaVersion: 1,
  defaultStrategy: "cmyx",
  a1MiniEnabled: true,
  currentT4SpoolId: "grey",
  currentA1SpoolId: "white",
};

afterEach(() => window.localStorage.clear());

describe("planning intent persistence", () => {
  it("defaults A1 routing from available equipment without selecting a strategy silently", () => {
    expect(createDefaultPlanningIntent(twoPrinterSetup)).toEqual({
      defaultStrategy: "auto",
      a1MiniEnabled: true,
    });
    expect(createDefaultPlanningIntent(null)).toEqual({
      defaultStrategy: "auto",
      a1MiniEnabled: false,
    });
  });

  it("cannot restore A1 routing into a saved U1-only equipment profile", () => {
    expect(
      constrainPlanningIntentToEquipment(intent, {
        schemaVersion: 1,
        primaryPrinter: "u1",
        secondaryPrinter: null,
      }),
    ).toEqual({
      ...intent,
      a1MiniEnabled: false,
    });
    expect(constrainPlanningIntentToEquipment(intent, twoPrinterSetup)).toBe(
      intent,
    );
    expect(
      constrainPlanningIntentToEquipment(
        { ...intent, a1MiniEnabled: false },
        { schemaVersion: 1, primaryPrinter: "u1", secondaryPrinter: null },
      ).a1MiniEnabled,
    ).toBe(false);
  });

  it("round-trips a strict version-two intent", () => {
    expect(savePlanningIntent(window.localStorage, intent)).toBe(true);
    expect(loadPlanningIntent(window.localStorage)).toEqual(intent);
    expect(
      JSON.parse(
        window.localStorage.getItem(PLANNING_INTENT_STORAGE_KEY) ?? "null",
      ),
    ).toEqual({ schemaVersion: 2, ...intent });
  });

  it("refuses to save runtime records with legacy or unknown fields", () => {
    expect(
      savePlanningIntent(window.localStorage, {
        ...intent,
        currentT4SpoolId: "grey",
      } as PlanningIntent),
    ).toBe(false);
    expect(window.localStorage.getItem(PLANNING_INTENT_STORAGE_KEY)).toBeNull();
  });

  it("migrates strategy and routing from a valid legacy version-one record", () => {
    window.localStorage.setItem(
      LEGACY_PLANNING_INTENT_STORAGE_KEY,
      JSON.stringify(legacyIntent),
    );

    expect(loadPlanningIntent(window.localStorage)).toEqual({
      defaultStrategy: "cmyx",
      a1MiniEnabled: true,
    });
    expect(loadLegacyPlanningIntent(window.localStorage)).toEqual(legacyIntent);
    expect(window.localStorage.getItem(PLANNING_INTENT_STORAGE_KEY)).toBeNull();
  });

  it("normalizes padded legacy spool identifiers for the loadout migration", () => {
    window.localStorage.setItem(
      LEGACY_PLANNING_INTENT_STORAGE_KEY,
      JSON.stringify({
        ...legacyIntent,
        currentT4SpoolId: "  grey  ",
        currentA1SpoolId: "  white  ",
      }),
    );

    expect(loadLegacyPlanningIntent(window.localStorage)).toMatchObject({
      currentT4SpoolId: "grey",
      currentA1SpoolId: "white",
    });
  });

  it("does not resurrect legacy intent when a current record is present but corrupt", () => {
    window.localStorage.setItem(
      LEGACY_PLANNING_INTENT_STORAGE_KEY,
      JSON.stringify(legacyIntent),
    );
    window.localStorage.setItem(PLANNING_INTENT_STORAGE_KEY, "not-json");

    expect(loadPlanningIntent(window.localStorage)).toBeNull();
  });

  it("rejects corrupt, unknown, and incomplete current records", () => {
    for (const value of [
      "not-json",
      JSON.stringify({ schemaVersion: 1, ...intent }),
      JSON.stringify({ schemaVersion: 2, ...intent, extra: true }),
      JSON.stringify({
        schemaVersion: 2,
        ...intent,
        defaultStrategy: "sometimes",
      }),
      JSON.stringify({ schemaVersion: 2, defaultStrategy: "auto" }),
    ]) {
      window.localStorage.setItem(PLANNING_INTENT_STORAGE_KEY, value);
      expect(loadPlanningIntent(window.localStorage)).toBeNull();
    }
  });

  it("rejects legacy records with unknown or incomplete fields", () => {
    for (const value of [
      JSON.stringify({ ...legacyIntent, extra: true }),
      JSON.stringify({ ...legacyIntent, schemaVersion: 2 }),
      JSON.stringify({ ...legacyIntent, currentT4SpoolId: "" }),
      JSON.stringify({ ...legacyIntent, currentA1SpoolId: 7 }),
    ]) {
      window.localStorage.setItem(LEGACY_PLANNING_INTENT_STORAGE_KEY, value);
      expect(loadLegacyPlanningIntent(window.localStorage)).toBeNull();
      expect(loadPlanningIntent(window.localStorage)).toBeNull();
    }
  });

  it("never throws when storage is unavailable", () => {
    const throwingStorage = {
      getItem() {
        throw new Error("blocked");
      },
      setItem() {
        throw new Error("blocked");
      },
    } as unknown as Storage;

    expect(loadPlanningIntent(null)).toBeNull();
    expect(loadPlanningIntent(throwingStorage)).toBeNull();
    expect(loadLegacyPlanningIntent(throwingStorage)).toBeNull();
    expect(savePlanningIntent(null, intent)).toBe(false);
    expect(savePlanningIntent(throwingStorage, intent)).toBe(false);
  });

  it("compares only the applied and draft project choices", () => {
    expect(planningIntentEquals(intent, { ...intent })).toBe(true);
    expect(
      planningIntentEquals(intent, { ...intent, defaultStrategy: "cmyx" }),
    ).toBe(false);
    expect(
      planningIntentEquals(intent, { ...intent, a1MiniEnabled: false }),
    ).toBe(false);
    expect(planningIntentEquals(intent, null)).toBe(false);
    expect(planningIntentEquals(null, null)).toBe(true);
  });
});
