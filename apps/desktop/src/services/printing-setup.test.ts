// @vitest-environment jsdom

import { describe, expect, it } from "vitest";

import {
  loadPrintingSetup,
  PRINTING_SETUP_STORAGE_KEY,
  savePrintingSetup,
  type PrintingSetup,
} from "./printing-setup";

const u1OnlySetup: PrintingSetup = {
  schemaVersion: 1,
  primaryPrinter: "u1",
  secondaryPrinter: null,
};

describe("printing setup persistence", () => {
  it("returns null when no setup has been saved", () => {
    window.localStorage.clear();
    expect(loadPrintingSetup(window.localStorage)).toBeNull();
  });

  it("round-trips U1-only and U1 plus A1 mini setups", () => {
    window.localStorage.clear();
    expect(savePrintingSetup(window.localStorage, u1OnlySetup)).toBe(true);
    expect(loadPrintingSetup(window.localStorage)).toEqual(u1OnlySetup);

    const twoPrinterSetup: PrintingSetup = {
      ...u1OnlySetup,
      secondaryPrinter: "a1-mini",
    };
    expect(savePrintingSetup(window.localStorage, twoPrinterSetup)).toBe(true);
    expect(loadPrintingSetup(window.localStorage)).toEqual(twoPrinterSetup);
  });

  it.each([
    "not-json",
    JSON.stringify({ ...u1OnlySetup, schemaVersion: 2 }),
    JSON.stringify({ ...u1OnlySetup, primaryPrinter: "a1-mini" }),
    JSON.stringify({ ...u1OnlySetup, secondaryPrinter: "u1" }),
    JSON.stringify({ ...u1OnlySetup, routingEnabled: true }),
  ])("rejects invalid stored data", (serialized) => {
    window.localStorage.setItem(PRINTING_SETUP_STORAGE_KEY, serialized);
    expect(loadPrintingSetup(window.localStorage)).toBeNull();
  });

  it("does not throw when storage is unavailable", () => {
    const throwingStorage = {
      getItem() {
        throw new Error("unavailable");
      },
      setItem() {
        throw new Error("unavailable");
      },
    } as unknown as Storage;

    expect(loadPrintingSetup(null)).toBeNull();
    expect(loadPrintingSetup(throwingStorage)).toBeNull();
    expect(savePrintingSetup(null, u1OnlySetup)).toBe(false);
    expect(savePrintingSetup(throwingStorage, u1OnlySetup)).toBe(false);
  });
});
