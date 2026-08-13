import { describe, expect, it } from "vitest";

import { createDemoPlan } from "../data/mock-plan";
import { deltaE00 } from "./color";
import {
  buildReplanRequest,
  cmyAverageDelta,
  createUniqueSpoolId,
  deriveBatches,
  derivePlanStats,
  directAverageDelta,
  directDelta,
  directQuality,
  getToolheadActions,
  inStockSpools,
  invalidateColorApprovals,
  isDirectStrategyAvailable,
  strategyLabel,
} from "./plan";
import { qualityForDelta } from "./color";

describe("plan helpers", () => {
  it("uses the authoritative physical Direct count instead of semantic row count", () => {
    const plate = structuredClone(createDemoPlan().plates[0]);
    plate.directEligible = true;
    plate.directPairCount = 4;
    plate.mappings = [
      ...plate.mappings!,
      {
        ...plate.mappings![0],
        id: "head-cyan-support",
        sourceName: "Clear Cyan Support",
      },
    ];

    expect(plate.mappings).toHaveLength(5);
    expect(isDirectStrategyAvailable(plate)).toBe(true);

    delete plate.directPairCount;
    expect(isDirectStrategyAvailable(plate)).toBe(false);
  });

  it("accepts seven source identities when the authoritative Direct loadout uses four spools", () => {
    const plate = structuredClone(createDemoPlan().plates[0]);
    const templates = plate.mappings!;
    plate.directEligible = true;
    plate.directPairCount = 7;
    plate.mappings = Array.from({ length: 7 }, (_, index) => ({
      ...structuredClone(templates[index % templates.length]),
      id: `source-${index + 1}`,
      inheritanceKey: `source-key-${index + 1}`,
      physicalIdentityId: `source-identity-${index + 1}`,
      selectedSpoolId: templates[index % 4].selectedSpoolId,
    }));

    expect(isDirectStrategyAvailable(plate)).toBe(true);
  });

  it("retains draft Direct mappings on CMY plates while custom palettes are enabled", () => {
    const plan = createDemoPlan();
    plan.plates[0].strategy = "cmyx";

    const disabled = buildReplanRequest(plan, false, false, false);
    const enabled = buildReplanRequest(plan, false, false, true);
    const disabledScope = disabled.scopeOverrides.find(
      (scope) => scope.scopeId === plan.plates[0].scopeId,
    );
    const enabledScope = enabled.scopeOverrides.find(
      (scope) => scope.scopeId === plan.plates[0].scopeId,
    );

    expect(disabled.allowDirectPaletteReduction).toBe(false);
    expect(disabledScope?.assignments).toHaveLength(0);
    expect(enabled.allowDirectPaletteReduction).toBe(true);
    expect(enabledScope?.strategy).toBe("cmyx");
    expect(enabledScope?.assignments).toHaveLength(
      plan.plates[0].mappings!.length,
    );
  });

  it("routes merged-plate Direct assignments back to their owning source scopes", () => {
    const plan = createDemoPlan();
    const firstMapping = structuredClone(plan.plates[0].mappings![0]);
    const secondMapping = {
      ...structuredClone(plan.plates[3].mappings![0]),
      id: "plate-4-source-cyan",
      scopeId: "plate-4",
    };
    plan.plates = [
      {
        ...plan.plates[0],
        scopeIds: ["plate-1", "plate-4"],
        strategy: "direct",
        mappings: [firstMapping, secondMapping],
      },
    ];

    const request = buildReplanRequest(
      plan,
      false,
      false,
      false,
      "direct",
      true,
    );

    expect(request.allowU1CrossSourceRepacking).toBe(true);
    expect(
      request.scopeOverrides.find((scope) => scope.scopeId === "plate-1")
        ?.assignments,
    ).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ requirementId: firstMapping.id }),
      ]),
    );
    expect(
      request.scopeOverrides.find((scope) => scope.scopeId === "plate-4")
        ?.assignments,
    ).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ requirementId: secondMapping.id }),
      ]),
    );
  });

  it("uses the planner Direct Spool ΔE00 quality boundaries", () => {
    expect(qualityForDelta(0)).toBe("Exact");
    expect(qualityForDelta(0.001)).toBe("Close");
    expect(qualityForDelta(3)).toBe("Close");
    expect(qualityForDelta(3.001)).toBe("Review");
    expect(qualityForDelta(6)).toBe("Review");
    expect(qualityForDelta(6.001)).toBe("Poor");
  });
  it("keeps A1 mini jobs in a separate A1 Mono batch", () => {
    const plan = createDemoPlan();
    const batches = deriveBatches(plan.plates);
    const a1Batches = batches.filter((batch) => batch.printer === "A1 mini");

    expect(a1Batches).toHaveLength(2);
    expect(a1Batches.every((batch) => batch.strategy === "a1-mono")).toBe(true);
    expect(strategyLabel[a1Batches[0].strategy]).toBe("A1 Mono");
  });

  it("uses authoritative planner batches and swap counts for summary metrics", () => {
    const plan = createDemoPlan();
    plan.batches = [{ ...plan.batches[0], id: "authoritative-only" }];
    plan.t4SwapCount = 7;
    plan.a1SpoolChangeCount = 4;
    plan.globalWarnings = ["Global planning warning"];
    const expectedWarningCount =
      1 +
      plan.plates.reduce(
        (count, plate) => count + plate.warnings.length,
        0,
      );

    expect(derivePlanStats(plan)).toMatchObject({
      batchCount: 1,
      t4Swaps: 7,
      a1SpoolChanges: 4,
      warnings: expectedWarningCount,
    });
  });

  it("builds keep and reversible replacement actions from the physical loadout", () => {
    const plan = createDemoPlan();
    const mappings = plan.plates[0].mappings!;

    expect(
      getToolheadActions(plan.spools, plan.currentLoadout, mappings, "T1", true),
    ).toEqual([{ kind: "Keep", spoolName: "Cyan" }]);
    expect(
      getToolheadActions(plan.spools, plan.currentLoadout, mappings, "T2", true),
    ).toEqual([
      { kind: "Unload", spoolName: "Magenta" },
      { kind: "Load", spoolName: "Royal Purple" },
      { kind: "Restore", spoolName: "Magenta" },
    ]);
  });

  it("computes a zero CIEDE2000 distance for identical colors", () => {
    expect(deltaE00("#08ABFB", "#08ABFB")).toBeCloseTo(0, 6);
  });

  it("keeps an unassigned Direct mapping explicit and pending review", () => {
    const plan = createDemoPlan();
    const mappings = plan.plates[0].mappings!.map((mapping, index) =>
      index === 0 ? { ...mapping, selectedSpoolId: "" } : mapping,
    );

    expect(directDelta(mappings[0], plan.spools)).toBeNull();
    expect(directQuality(mappings[0], plan.spools)).toBe("Review");
    expect(directAverageDelta(mappings, plan.spools)).toBeNull();
    expect(
      getToolheadActions(plan.spools, plan.currentLoadout, mappings, "T1", true),
    ).toEqual([{ kind: "Review", spoolName: "Unassigned" }]);
  });

  it("keeps out-of-stock spools in the catalogue but out of planning choices", () => {
    const plan = createDemoPlan();
    const unavailableIds = new Set(["panchroma-cyan", "royal-purple"]);
    plan.spools = plan.spools.map((spool) =>
      unavailableIds.has(spool.id) ? { ...spool, available: false } : spool,
    );
    plan.plates[0].strategy = "direct";
    const purpleMapping = plan.plates[0].mappings![1];

    expect(inStockSpools(plan.spools).some((spool) => spool.id === "royal-purple"))
      .toBe(false);
    expect(directDelta(purpleMapping, plan.spools)).toBeNull();
    expect(directQuality(purpleMapping, plan.spools)).toBe("Review");
    expect(
      getToolheadActions(
        plan.spools,
        plan.currentLoadout,
        plan.plates[0].mappings!,
        "T2",
        false,
      ),
    ).toEqual([{ kind: "Review", spoolName: "Out of stock" }]);

    const request = buildReplanRequest(plan, false);
    const headOverride = request.scopeOverrides.find(
      (override) => override.scopeId === "plate-1",
    );
    expect(
      headOverride?.assignments.some(
        (assignment) => assignment.spoolId === "royal-purple",
      ),
    ).toBe(false);
    expect(
      request.currentLoadout.some(
        (loaded) => loaded.spoolId === "panchroma-cyan",
      ),
    ).toBe(false);
  });

  it("ignores unavailable CMY deltas without treating them as exact matches", () => {
    const mappings = createDemoPlan().plates[0].mappings!;
    const partlyUnavailable = mappings.map((mapping, index) =>
      index === 0 ? { ...mapping, cmyDeltaE00: null } : mapping,
    );

    expect(cmyAverageDelta(partlyUnavailable)).toBeCloseTo((6.8 + 5.1 + 9.2) / 3);
    expect(
      cmyAverageDelta(
        mappings.map((mapping) => ({ ...mapping, cmyDeltaE00: null })),
      ),
    ).toBeNull();
  });

  it("creates stable unique IDs for user inventory and preserves native scope keys", () => {
    expect(createUniqueSpoolId("Workshop Red", [])).toBe("user-workshop-red");
    expect(
      createUniqueSpoolId("Workshop Red", [
        "user-workshop-red",
        "user-workshop-red-2",
      ]),
    ).toBe("user-workshop-red-3");

    const plan = createDemoPlan();
    expect(plan.spools.every((spool) => spool.source === "built-in")).toBe(true);
    expect(plan.plates.map((plate) => plate.scopeId)).toEqual([
      "plate-1",
      "plate-2",
      "plate-3",
      "plate-4",
      "plate-5",
      "plate-6",
      "plate-7",
      "plate-8",
    ]);
  });

  it("builds a native replan request from user inventory and nonempty Direct assignments", () => {
    const plan = createDemoPlan();
    plan.spools.push({
      id: "user-workshop-red",
      source: "user",
      name: "Workshop Red",
      colorName: "Red",
      hex: "#B72E2A",
      material: "PETG",
      sku: "SHOP-RD-01",
      profile: "0.20 mm Workshop PETG",
      colorBasis: "Nominal",
      available: true,
    });
    plan.plates[0].strategy = "direct";
    plan.plates[0].mappings![0].selectedSpoolId = "";
    plan.plates.push({
      ...plan.plates[0],
      id: "plate-01-second-job",
      order: 99,
      mappings: plan.plates[0].mappings?.map((mapping) => ({ ...mapping })),
    });

    const request = buildReplanRequest(plan, false);

    expect(request.confirmedSpools).toEqual([
      {
        id: "user-workshop-red",
        name: "Workshop Red",
        colorName: "Red",
        hex: "#B72E2A",
        material: "PETG",
        sku: "SHOP-RD-01",
        profile: "0.20 mm Workshop PETG",
        colorBasis: "Nominal",
        available: true,
      },
    ]);
    expect(request.scopeOverrides).toHaveLength(7);
    expect(
      request.scopeOverrides.filter((override) => override.scopeId === "plate-1"),
    ).toHaveLength(1);
    expect(request.scopeOverrides.some((override) => override.scopeId === "plate-7")).toBe(
      false,
    );
    expect(
      request.scopeOverrides.find((override) => override.scopeId === "plate-8"),
    ).toMatchObject({ strategy: "cmyx", assignments: [] });
    expect(
      request.scopeOverrides.find((override) => override.scopeId === "plate-1"),
    ).toMatchObject({
      strategy: "direct",
      assignments: [
        { requirementId: "head-purple", spoolId: "royal-purple", toolhead: "T2" },
        { requirementId: "head-sand", spoolId: "sandstone", toolhead: "T3" },
        { requirementId: "head-grey", spoolId: "graphite", toolhead: "T4" },
      ],
    });
    expect(request.restoreCmyAfterDirect).toBe(false);
    expect(request.a1MiniEnabled).toBe(false);
    expect(request.currentA1SpoolId).toBeNull();
    expect(request.includedAlternativePlateIds).toEqual([]);
    expect(buildReplanRequest(plan, true, true).a1MiniEnabled).toBe(true);
  });

  it("preserves Direct intent and assignments when the scheduler routes a scope to A1 mini", () => {
    const replanned = createDemoPlan();
    replanned.scopeSelections = replanned.scopeSelections.map((selection) =>
      selection.scopeId === "plate-8"
        ? {
            scopeId: "plate-8",
            strategy: "direct",
            assignments: [
              {
                requirementId: "nameplate-black",
                spoolId: "black-petg",
                toolhead: "T4",
              },
            ],
            approvedColorFallbacks: [],
            materialSubstitutions: [],
          }
        : selection,
    );
    const a1Plate = replanned.plates.find((plate) => plate.scopeId === "plate-8");
    expect(a1Plate).toMatchObject({ printer: "A1 mini", strategy: "a1-mono" });
    expect(a1Plate?.mappings).toBeUndefined();

    const firstRequest = buildReplanRequest(replanned, true, true);
    const repeatedRequest = buildReplanRequest(replanned, true, true);

    for (const request of [firstRequest, repeatedRequest]) {
      expect(
        request.scopeOverrides.find((selection) => selection.scopeId === "plate-8"),
      ).toEqual({
        scopeId: "plate-8",
        strategy: "direct",
        assignments: [
          {
            requirementId: "nameplate-black",
            spoolId: "black-petg",
            toolhead: "T4",
          },
        ],
        approvedColorFallbacks: [],
        materialSubstitutions: [],
      });
    }
  });

  it("does not resurrect a Direct assignment cleared on a visible U1 scope", () => {
    const plan = createDemoPlan();
    plan.scopeSelections = plan.scopeSelections.map((selection) =>
      selection.scopeId === "plate-1"
        ? {
            scopeId: "plate-1",
            strategy: "direct",
            assignments: plan.plates[0].mappings!.map((mapping) => ({
              requirementId: mapping.id,
              spoolId: mapping.selectedSpoolId,
              toolhead: mapping.directToolhead,
            })),
            approvedColorFallbacks: [],
            materialSubstitutions: [],
          }
        : selection,
    );
    plan.plates[0].strategy = "direct";
    plan.plates[0].mappings![0].selectedSpoolId = "";

    const selection = buildReplanRequest(plan, true).scopeOverrides.find(
      (candidate) => candidate.scopeId === "plate-1",
    );

    expect(selection?.assignments).not.toContainEqual(
      expect.objectContaining({ requirementId: "head-cyan" }),
    );
  });

  it("sends selected alternatives and drops stale overrides for excluded plates", () => {
    const plan = createDemoPlan();
    plan.alternativePlates = plan.alternativePlates.map((plate) =>
      plate.id === 10 ? { ...plate, included: true } : plate,
    );
    plan.plates.push({
      ...plan.plates[0],
      id: "stale-alternative-job",
      scopeId: "plate-7-palette-01",
      order: 99,
    });

    const request = buildReplanRequest(plan, true);

    expect(request.includedAlternativePlateIds).toEqual([10]);
    expect(
      request.scopeOverrides.some(
        (planningOverride) => planningOverride.scopeId === "plate-7-palette-01",
      ),
    ).toBe(false);
  });

  it("binds color and material approvals to the exact backend candidate", () => {
    const plan = createDemoPlan();
    plan.colorResolutions = [
      {
        scopeId: "plate-1",
        scopeName: "Head",
        requirementId: "head-grey",
        candidateId: "candidate-grey-v2",
        sourceMaterial: "PLA",
        sourceHex: "#8E9089",
        targetMaterial: "PLA",
        targetHex: "#8E9089",
        predictedHex: "#9199A4",
        recipe: "Solid T4",
        deltaE00: 10.1,
        confidence: "Nominal",
        requiredT4SpoolId: "panchroma-translucent-grey",
        requiredT4Name: "Panchroma Translucent Grey",
        requiredT4Hex: "#9199A4",
        colorApproved: true,
        materialApproved: false,
        requiresMaterialSubstitution: false,
        canAddDedicatedSpool: true,
        recommendation: "Use the nearest CMY + Grey result or add a dedicated spool.",
      },
      {
        scopeId: "plate-6",
        scopeName: "Frame",
        requirementId: "frame-grey-petg",
        candidateId: "candidate-frame-pla-grey",
        sourceMaterial: "PETG",
        sourceHex: "#ADB1B2",
        targetMaterial: "PLA",
        targetHex: "#ADB1B2",
        predictedHex: "#A8ADB0",
        recipe: "CMY + Grey",
        deltaE00: 2.2,
        confidence: "Nominal",
        requiredT4SpoolId: "panchroma-translucent-grey",
        requiredT4Name: "Panchroma Translucent Grey",
        requiredT4Hex: "#9199A4",
        colorApproved: true,
        materialApproved: true,
        requiresMaterialSubstitution: true,
        canAddDedicatedSpool: true,
        recommendation: "Add PETG Grey or explicitly accept PLA.",
      },
      {
        scopeId: "plate-8",
        scopeName: "Heat shield",
        requirementId: "shield-black-abs",
        candidateId: "candidate-shield-pla-black",
        sourceMaterial: "ABS",
        sourceHex: "#111111",
        targetMaterial: "PLA",
        targetHex: "#111111",
        predictedHex: "#080808",
        recipe: "Solid T4",
        deltaE00: 1.4,
        confidence: "Nominal",
        requiredT4SpoolId: "panchroma-basic-black",
        requiredT4Name: "Panchroma Basic Black",
        requiredT4Hex: "#080808",
        colorApproved: true,
        materialApproved: true,
        requiresMaterialSubstitution: true,
        canAddDedicatedSpool: false,
        recommendation: "Explicitly accept the ABS to PLA material substitution.",
      },
    ];

    const request = buildReplanRequest(plan, true);
    expect(
      request.scopeOverrides.find((entry) => entry.scopeId === "plate-1"),
    ).toMatchObject({
      approvedColorFallbacks: [
        { requirementId: "head-grey", candidateId: "candidate-grey-v2" },
      ],
      materialSubstitutions: [],
    });
    expect(
      request.scopeOverrides.find((entry) => entry.scopeId === "plate-6"),
    ).toMatchObject({
      approvedColorFallbacks: [
        {
          requirementId: "frame-grey-petg",
          candidateId: "candidate-frame-pla-grey",
        },
      ],
      materialSubstitutions: [
        {
          requirementId: "frame-grey-petg",
          candidateId: "candidate-frame-pla-grey",
          sourceMaterial: "PETG",
          targetMaterial: "PLA",
          acknowledged: true,
        },
      ],
    });
    expect(
      request.scopeOverrides.find((entry) => entry.scopeId === "plate-8"),
    ).toMatchObject({
      approvedColorFallbacks: [
        {
          requirementId: "shield-black-abs",
          candidateId: "candidate-shield-pla-black",
        },
      ],
      materialSubstitutions: [
        {
          requirementId: "shield-black-abs",
          candidateId: "candidate-shield-pla-black",
          sourceMaterial: "ABS",
          targetMaterial: "PLA",
          acknowledged: true,
        },
      ],
    });
  });

  it("drops approvals that are not represented by the exact current candidate", () => {
    const plan = createDemoPlan();
    plan.scopeSelections = plan.scopeSelections.map((selection) => {
      if (selection.scopeId === "plate-1") {
        return {
          ...selection,
          approvedColorFallbacks: [
            {
              requirementId: "plate-1-requirement-2",
              candidateId: "superseded-candidate",
            },
            {
              requirementId: "removed-requirement",
              candidateId: "removed-candidate",
            },
          ],
          materialSubstitutions: [],
        };
      }
      if (selection.scopeId === "plate-6") {
        return {
          ...selection,
          approvedColorFallbacks: [
            {
              requirementId: "plate-6-requirement-1",
              candidateId: "removed-material-candidate",
            },
          ],
          materialSubstitutions: [
            {
              requirementId: "plate-6-requirement-1",
              candidateId: "removed-material-candidate",
              sourceMaterial: "PETG",
              targetMaterial: "PLA",
              acknowledged: true,
            },
          ],
        };
      }
      return selection;
    });
    plan.colorResolutions = [
      {
        scopeId: "plate-1",
        scopeName: "Head",
        requirementId: "plate-1-requirement-2",
        candidateId: "current-candidate",
        sourceMaterial: "PLA",
        sourceHex: "#8E9089",
        targetMaterial: "PLA",
        targetHex: "#8E9089",
        predictedHex: "#9199A4",
        recipe: "Solid T4 Grey",
        deltaE00: 10.1,
        confidence: "Nominal",
        requiredT4SpoolId: "panchroma-translucent-grey",
        requiredT4Name: "Panchroma Translucent Grey",
        requiredT4Hex: "#9199A4",
        colorApproved: true,
        materialApproved: false,
        requiresMaterialSubstitution: false,
        canAddDedicatedSpool: true,
        recommendation: "Use the current candidate.",
      },
    ];

    const request = buildReplanRequest(plan, true);

    expect(
      request.scopeOverrides.find((entry) => entry.scopeId === "plate-1"),
    ).toMatchObject({
      approvedColorFallbacks: [
        {
          requirementId: "plate-1-requirement-2",
          candidateId: "current-candidate",
        },
      ],
      materialSubstitutions: [],
    });
    expect(
      request.scopeOverrides.find((entry) => entry.scopeId === "plate-6"),
    ).toMatchObject({
      approvedColorFallbacks: [],
      materialSubstitutions: [],
    });
  });

  it("invalidates color and material approvals after inventory changes", () => {
    const plan = createDemoPlan();
    plan.colorResolutions = [
      {
        scopeId: "plate-6",
        scopeName: "Frame",
        requirementId: "plate-6-requirement-1",
        candidateId: "frame-petg-to-pla-grey-v1",
        sourceMaterial: "PETG",
        sourceHex: "#ADB1B2",
        targetMaterial: "PLA",
        targetHex: "#ADB1B2",
        predictedHex: "#A8ADB0",
        recipe: "CMY + Grey",
        deltaE00: 2.2,
        confidence: "Nominal",
        requiredT4SpoolId: "panchroma-translucent-grey",
        requiredT4Name: "Panchroma Translucent Grey",
        requiredT4Hex: "#9199A4",
        colorApproved: true,
        materialApproved: true,
        requiresMaterialSubstitution: true,
        canAddDedicatedSpool: true,
        recommendation: "Use the approved candidate.",
      },
    ];
    plan.scopeSelections = plan.scopeSelections.map((selection) =>
      selection.scopeId === "plate-6"
        ? {
            ...selection,
            approvedColorFallbacks: [
              {
                requirementId: "plate-6-requirement-1",
                candidateId: "frame-petg-to-pla-grey-v1",
              },
            ],
            materialSubstitutions: [
              {
                requirementId: "plate-6-requirement-1",
                candidateId: "frame-petg-to-pla-grey-v1",
                sourceMaterial: "PETG",
                targetMaterial: "PLA",
                acknowledged: true,
              },
            ],
          }
        : selection,
    );

    const invalidated = invalidateColorApprovals(plan);

    expect(invalidated.colorResolutions[0]).toMatchObject({
      colorApproved: false,
      materialApproved: false,
    });
    expect(
      invalidated.scopeSelections.find((entry) => entry.scopeId === "plate-6"),
    ).toMatchObject({
      approvedColorFallbacks: [],
      materialSubstitutions: [],
    });
    expect(plan.colorResolutions[0]).toMatchObject({
      colorApproved: true,
      materialApproved: true,
    });
  });
});
