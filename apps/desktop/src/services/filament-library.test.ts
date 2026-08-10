// @vitest-environment jsdom

import { invoke } from "@tauri-apps/api/core";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { FilamentLibraryDocument, PhysicalSpool } from "../types";
import {
  FILAMENT_LIBRARY_STORAGE_KEY,
  LEGACY_FILAMENT_LIBRARY_STORAGE_KEY,
  loadFilamentLibrary,
  mergeFilamentLibrary,
  saveFilamentLibrary,
} from "./filament-library";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const builtIn: PhysicalSpool = {
  id: "panchroma-cyan",
  calibrationIdentity: "panchroma-cyan",
  source: "built-in",
  name: "Panchroma Translucent Cyan",
  colorName: "Cyan",
  hex: "#08ABFB",
  material: "PLA",
  sku: "PM-CY-001",
  colorBasis: "Nominal",
  available: true,
};

const userSpool: PhysicalSpool = {
  id: "user-workshop-red",
  calibrationIdentity: "spool-calibration:11111111-1111-4111-8111-111111111111",
  source: "user",
  name: "Workshop Red",
  colorName: "Red",
  hex: "#C72E2A",
  material: "PETG",
  colorBasis: "Measured",
  available: false,
};

afterEach(() => {
  vi.resetAllMocks();
  window.localStorage.clear();
  delete (window as Window & { __TAURI_INTERNALS__?: unknown })
    .__TAURI_INTERNALS__;
});

describe("filament library persistence", () => {
  it("uses browser storage and keeps canonical built-in details", async () => {
    window.localStorage.setItem(
      FILAMENT_LIBRARY_STORAGE_KEY,
      JSON.stringify({
        schemaVersion: 2,
        spools: [
          {
            ...builtIn,
            name: "Tampered built-in",
            available: false,
          },
          userSpool,
        ],
      }),
    );

    const loaded = await loadFilamentLibrary([builtIn]);

    expect(loaded).toEqual({
      schemaVersion: 2,
      spools: [{ ...builtIn, available: false }, userSpool],
    });
    expect(invoke).not.toHaveBeenCalled();
  });

  it("uses the supplied catalogue only when browser storage is absent", async () => {
    await expect(loadFilamentLibrary([builtIn])).resolves.toEqual({
      schemaVersion: 2,
      spools: [builtIn],
    });
  });

  it("migrates the legacy browser document to v2 without deleting its recovery copy", async () => {
    const legacyValue = JSON.stringify({
      schemaVersion: 1,
      spools: [
        {
          id: userSpool.id,
          source: userSpool.source,
          name: userSpool.name,
          colorName: userSpool.colorName,
          hex: userSpool.hex,
          material: userSpool.material,
          sku: "RED-01",
          profile: "Generic PETG",
          colorBasis: userSpool.colorBasis,
          available: false,
        },
      ],
    });
    window.localStorage.setItem(
      LEGACY_FILAMENT_LIBRARY_STORAGE_KEY,
      legacyValue,
    );

    const migrated = await loadFilamentLibrary([builtIn]);

    expect(migrated.schemaVersion).toBe(2);
    expect(migrated.spools).toContainEqual({
      ...userSpool,
      calibrationIdentity: userSpool.id,
      sku: "RED-01",
      profile: "Generic PETG",
    });
    expect(window.localStorage.getItem(LEGACY_FILAMENT_LIBRARY_STORAGE_KEY)).toBe(
      legacyValue,
    );
    expect(
      JSON.parse(
        window.localStorage.getItem(FILAMENT_LIBRARY_STORAGE_KEY) ?? "null",
      ),
    ).toEqual(migrated);
  });

  it("reports corrupt browser JSON without replacing the stored data", async () => {
    const storedValue = "not json";
    window.localStorage.setItem(FILAMENT_LIBRARY_STORAGE_KEY, storedValue);

    await expect(loadFilamentLibrary([builtIn])).rejects.toThrow(
      "The saved browser filament library is not valid JSON",
    );
    expect(window.localStorage.getItem(FILAMENT_LIBRARY_STORAGE_KEY)).toBe(
      storedValue,
    );
  });

  it("rejects the entire browser document when any spool is invalid", async () => {
    const storedValue = JSON.stringify({
      schemaVersion: 1,
      spools: [
        userSpool,
        {
          ...builtIn,
          id: "invalid-spool",
          hex: "not-a-color",
        },
      ],
    });
    window.localStorage.setItem(FILAMENT_LIBRARY_STORAGE_KEY, storedValue);

    await expect(loadFilamentLibrary([builtIn])).rejects.toThrow(
      "The saved browser filament library has an invalid schema",
    );
    expect(window.localStorage.getItem(FILAMENT_LIBRARY_STORAGE_KEY)).toBe(
      storedValue,
    );
  });

  it.each([
    ["missing", undefined],
    ["non-boolean", "yes"],
  ])(
    "rejects browser data with a %s availability value",
    async (_label, available) => {
      const spool = { ...userSpool } as Record<string, unknown>;
      if (available === undefined) {
        delete spool.available;
      } else {
        spool.available = available;
      }
      window.localStorage.setItem(
        FILAMENT_LIBRARY_STORAGE_KEY,
        JSON.stringify({ schemaVersion: 1, spools: [spool] }),
      );

      await expect(loadFilamentLibrary([builtIn])).rejects.toThrow(
        "The saved browser filament library has an invalid schema",
      );
    },
  );

  it("rejects duplicate spool ids instead of silently dropping an entry", async () => {
    window.localStorage.setItem(
      FILAMENT_LIBRARY_STORAGE_KEY,
      JSON.stringify({
        schemaVersion: 1,
        spools: [userSpool, { ...userSpool, name: "Duplicate Red" }],
      }),
    );

    await expect(loadFilamentLibrary([builtIn])).rejects.toThrow(
      "The saved browser filament library has an invalid schema",
    );
  });

  it("saves the full browser catalogue, including out-of-stock entries", async () => {
    const saved = await saveFilamentLibrary([builtIn, userSpool]);

    expect(saved.spools).toEqual([builtIn, userSpool]);
    expect(
      JSON.parse(
        window.localStorage.getItem(FILAMENT_LIBRARY_STORAGE_KEY) ?? "null",
      ),
    ).toEqual(saved);
  });

  it("rotates calibration identity only when lot or calibration reference changes", async () => {
    const initial = {
      ...userSpool,
      batchLot: "LOT-A",
      calibrationReference: "flat-v1",
      vendor: "Polymaker",
    };
    await saveFilamentLibrary([initial]);

    const displayOnly = await saveFilamentLibrary([
      { ...initial, vendor: "Updated vendor", calibrationIdentity: "forged" },
    ]);
    expect(displayOnly.spools[0]?.calibrationIdentity).toBe(
      userSpool.calibrationIdentity,
    );

    const lotChanged = await saveFilamentLibrary([
      { ...displayOnly.spools[0]!, batchLot: "LOT-B" },
    ]);
    expect(lotChanged.spools[0]?.calibrationIdentity).not.toBe(
      userSpool.calibrationIdentity,
    );
    expect(lotChanged.spools[0]?.calibrationIdentity).toMatch(
      /^spool-calibration:[0-9a-f-]{36}$/,
    );

    const referenceChanged = await saveFilamentLibrary([
      { ...lotChanged.spools[0]!, calibrationReference: "flat-v2" },
    ]);
    expect(referenceChanged.spools[0]?.calibrationIdentity).not.toBe(
      lotChanged.spools[0]?.calibrationIdentity,
    );
  });

  it("rejects invalid temperatures and duplicate calibration identities", async () => {
    window.localStorage.setItem(
      FILAMENT_LIBRARY_STORAGE_KEY,
      JSON.stringify({
        schemaVersion: 2,
        spools: [
          {
            ...userSpool,
            minNozzleTemperatureC: 260,
            maxNozzleTemperatureC: 230,
          },
        ],
      }),
    );
    await expect(loadFilamentLibrary()).rejects.toThrow("invalid schema");

    window.localStorage.setItem(
      FILAMENT_LIBRARY_STORAGE_KEY,
      JSON.stringify({
        schemaVersion: 2,
        spools: [
          userSpool,
          {
            ...userSpool,
            id: "user-second-red",
          },
        ],
      }),
    );
    await expect(loadFilamentLibrary()).rejects.toThrow("invalid schema");
  });

  it("uses the native load and replacement-save contracts in Tauri", async () => {
    (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ =
      {};
    const library: FilamentLibraryDocument = {
      schemaVersion: 2,
      spools: [builtIn, userSpool],
    };
    vi.mocked(invoke).mockResolvedValue(library);

    await expect(loadFilamentLibrary()).resolves.toEqual(library);
    await expect(saveFilamentLibrary(library.spools)).resolves.toEqual(library);

    expect(invoke).toHaveBeenNthCalledWith(1, "load_filament_library");
    expect(invoke).toHaveBeenNthCalledWith(2, "save_filament_library", {
      library,
    });
  });

  it("restores omitted built-ins but not deleted user spools", () => {
    expect(mergeFilamentLibrary([builtIn, userSpool], [])).toEqual([builtIn]);
    expect(mergeFilamentLibrary([builtIn], [])).toEqual([builtIn]);
  });
});
