// @vitest-environment jsdom

import { afterEach, describe, expect, it } from "vitest";

import { LEGACY_PLANNING_INTENT_STORAGE_KEY } from "./planning-intent";
import {
  constrainPrinterLoadoutToEquipment,
  createEmptyPrinterLoadout,
  loadPrinterLoadout,
  PRINTER_LOADOUT_STORAGE_KEY,
  printerLoadoutEquals,
  savePrinterLoadout,
  type PrinterLoadoutProfile,
} from "./printer-loadout";

const profile: PrinterLoadoutProfile = {
  schemaVersion: 1,
  currentLoadout: [
    { toolhead: "T1", spoolId: "cyan" },
    { toolhead: "T2", spoolId: "magenta" },
    { toolhead: "T3", spoolId: "yellow" },
    { toolhead: "T4", spoolId: "grey" },
  ],
  currentA1SpoolId: "white",
  updatedFrom: "manual",
};

const legacyIntent = {
  schemaVersion: 1,
  defaultStrategy: "direct",
  a1MiniEnabled: true,
  currentT4SpoolId: "  grey  ",
  currentA1SpoolId: "  white  ",
};

afterEach(() => window.localStorage.clear());

describe("printer loadout persistence", () => {
  it("creates an explicit unknown starting state", () => {
    expect(createEmptyPrinterLoadout()).toEqual({
      schemaVersion: 1,
      currentLoadout: [],
      currentA1SpoolId: null,
      updatedFrom: "manual",
    });
    expect(createEmptyPrinterLoadout("print-run").updatedFrom).toBe(
      "print-run",
    );
  });

  it("round-trips a valid profile in canonical T1-T4 order", () => {
    const unordered: PrinterLoadoutProfile = {
      ...profile,
      currentLoadout: [
        profile.currentLoadout[3],
        profile.currentLoadout[1],
        profile.currentLoadout[0],
        profile.currentLoadout[2],
      ],
      updatedFrom: "print-run",
    };

    expect(savePrinterLoadout(window.localStorage, unordered)).toBe(true);
    expect(loadPrinterLoadout(window.localStorage)).toEqual({
      ...profile,
      updatedFrom: "print-run",
    });
  });

  it("migrates T4 and A1 state from the legacy planning intent without mutating storage", () => {
    window.localStorage.setItem(
      LEGACY_PLANNING_INTENT_STORAGE_KEY,
      JSON.stringify(legacyIntent),
    );

    expect(loadPrinterLoadout(window.localStorage)).toEqual({
      schemaVersion: 1,
      currentLoadout: [{ toolhead: "T4", spoolId: "grey" }],
      currentA1SpoolId: "white",
      updatedFrom: "manual",
    });
    expect(window.localStorage.getItem(PRINTER_LOADOUT_STORAGE_KEY)).toBeNull();
  });

  it("migrates an explicitly unknown legacy loadout", () => {
    window.localStorage.setItem(
      LEGACY_PLANNING_INTENT_STORAGE_KEY,
      JSON.stringify({
        ...legacyIntent,
        currentT4SpoolId: null,
        currentA1SpoolId: null,
      }),
    );

    expect(loadPrinterLoadout(window.localStorage)).toEqual({
      schemaVersion: 1,
      currentLoadout: [],
      currentA1SpoolId: null,
      updatedFrom: "manual",
    });
  });

  it("does not resurrect a legacy loadout when a current record is present but corrupt", () => {
    window.localStorage.setItem(
      LEGACY_PLANNING_INTENT_STORAGE_KEY,
      JSON.stringify(legacyIntent),
    );
    window.localStorage.setItem(PRINTER_LOADOUT_STORAGE_KEY, "not-json");

    expect(loadPrinterLoadout(window.localStorage)).toBeNull();
  });

  it("rejects duplicate toolheads, U1 spools, and cross-printer spool identities", () => {
    const invalidProfiles = [
      {
        ...profile,
        currentLoadout: [
          { toolhead: "T1", spoolId: "cyan" },
          { toolhead: "T1", spoolId: "magenta" },
        ],
      },
      {
        ...profile,
        currentLoadout: [
          { toolhead: "T1", spoolId: "cyan" },
          { toolhead: "T2", spoolId: "cyan" },
        ],
      },
      {
        ...profile,
        currentA1SpoolId: "cyan",
      },
    ];

    for (const invalid of invalidProfiles) {
      window.localStorage.setItem(
        PRINTER_LOADOUT_STORAGE_KEY,
        JSON.stringify(invalid),
      );
      expect(loadPrinterLoadout(window.localStorage)).toBeNull();
      expect(
        savePrinterLoadout(
          window.localStorage,
          invalid as PrinterLoadoutProfile,
        ),
      ).toBe(false);
    }
  });

  it("rejects unknown fields, invalid slots, padded IDs, and unsupported sources", () => {
    const invalidProfiles = [
      { ...profile, extra: true },
      { ...profile, schemaVersion: 2 },
      { ...profile, updatedFrom: "analysis" },
      {
        ...profile,
        currentLoadout: [{ toolhead: "T5", spoolId: "cyan" }],
      },
      {
        ...profile,
        currentLoadout: [{ toolhead: "T1", spoolId: " cyan " }],
      },
      {
        ...profile,
        currentLoadout: [{ toolhead: "T1", spoolId: "cyan", extra: true }],
      },
      { ...profile, currentA1SpoolId: " " },
    ];

    for (const invalid of invalidProfiles) {
      window.localStorage.setItem(
        PRINTER_LOADOUT_STORAGE_KEY,
        JSON.stringify(invalid),
      );
      expect(loadPrinterLoadout(window.localStorage)).toBeNull();
    }
  });

  it("clears the A1 spool only when equipment explicitly excludes A1 mini", () => {
    expect(
      constrainPrinterLoadoutToEquipment(profile, {
        schemaVersion: 1,
        primaryPrinter: "u1",
        secondaryPrinter: null,
      }),
    ).toEqual({ ...profile, currentA1SpoolId: null });
    expect(
      constrainPrinterLoadoutToEquipment(profile, {
        schemaVersion: 1,
        primaryPrinter: "u1",
        secondaryPrinter: "a1-mini",
      }),
    ).toBe(profile);
    expect(constrainPrinterLoadoutToEquipment(profile, null)).toBe(profile);

    const u1Only = { ...profile, currentA1SpoolId: null };
    expect(
      constrainPrinterLoadoutToEquipment(u1Only, {
        schemaVersion: 1,
        primaryPrinter: "u1",
        secondaryPrinter: null,
      }),
    ).toBe(u1Only);
  });

  it("compares physical state independent of array order and update source", () => {
    expect(
      printerLoadoutEquals(profile, {
        ...profile,
        currentLoadout: [...profile.currentLoadout].reverse(),
        updatedFrom: "print-run",
      }),
    ).toBe(true);
    expect(
      printerLoadoutEquals(profile, {
        ...profile,
        currentA1SpoolId: "black",
      }),
    ).toBe(false);
    expect(
      printerLoadoutEquals(profile, {
        ...profile,
        currentLoadout: profile.currentLoadout.slice(1),
      }),
    ).toBe(false);
    expect(printerLoadoutEquals(null, null)).toBe(true);
    expect(printerLoadoutEquals(profile, null)).toBe(false);
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

    expect(loadPrinterLoadout(null)).toBeNull();
    expect(loadPrinterLoadout(throwingStorage)).toBeNull();
    expect(savePrinterLoadout(null, profile)).toBe(false);
    expect(savePrinterLoadout(throwingStorage, profile)).toBe(false);
  });
});
