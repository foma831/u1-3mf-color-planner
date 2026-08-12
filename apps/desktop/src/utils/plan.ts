import type {
  BatchSegment,
  DirectColorMapping,
  LoadedToolhead,
  PhysicalSpool,
  PlatePlan,
  PrintStrategy,
  ProjectDirectPaletteChoice,
  ProjectPlan,
  ReplanRequest,
  ScopeOverride,
  ToolheadAction,
  ToolheadId,
} from "../types";
import { deltaE00, qualityForDelta } from "./color";

export const toolheads: ToolheadId[] = ["T1", "T2", "T3", "T4"];

export const strategyLabel: Record<PrintStrategy, string> = {
  cmyx: "CMY+X Full Spectrum",
  "cmyx-solid": "CMY+X Solid",
  direct: "Direct Spools",
  "a1-mono": "A1 Mono",
};

export function isCmyxStrategy(strategy: PrintStrategy) {
  return strategy === "cmyx" || strategy === "cmyx-solid";
}

export function inStockSpools(spools: PhysicalSpool[]) {
  return spools.filter((spool) => spool.available);
}

export function isDirectStrategyAvailable(plate: PlatePlan) {
  const mappings = plate.mappings ?? [];
  const hasAuthoritativePairCount =
    typeof plate.directPairCount === "number" && plate.directPairCount > 0;
  const selectedSpoolIds = new Set(
    mappings
      .map((mapping) => mapping.selectedSpoolId)
      .filter((spoolId) => spoolId.length > 0),
  );
  return (
    plate.directEligible &&
    (hasAuthoritativePairCount || mappings.length <= 4) &&
    mappings.length > 0 &&
    selectedSpoolIds.size <= 4
  );
}

export function directPhysicalSpoolCount(plate: PlatePlan) {
  return new Set(
    (plate.mappings ?? [])
      .map((mapping) => mapping.selectedSpoolId)
      .filter((spoolId) => spoolId.length > 0),
  ).size;
}

export function createUniqueSpoolId(
  label: string,
  existingIds: Iterable<string>,
) {
  const slug = label
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  const baseId = `user-${slug || "spool"}`;
  const reservedIds = new Set(existingIds);

  if (!reservedIds.has(baseId)) return baseId;

  let suffix = 2;
  while (reservedIds.has(`${baseId}-${suffix}`)) suffix += 1;
  return `${baseId}-${suffix}`;
}

export function buildReplanRequest(
  plan: ProjectPlan,
  restoreCmyAfterDirect: boolean,
  a1MiniEnabled = false,
  allowDirectPaletteReduction = false,
  defaultStrategy: "auto" | "cmyx" | "direct" = "auto",
): ReplanRequest {
  const availableSpoolIds = new Set(
    inStockSpools(plan.spools).map((spool) => spool.id),
  );
  const resolutionKey = (
    scopeId: string,
    requirementId: string,
    candidateId: string,
  ) => JSON.stringify([scopeId, requirementId, candidateId]);
  const currentResolutions = new Map(
    plan.colorResolutions.map((resolution) => [
      resolutionKey(
        resolution.scopeId,
        resolution.requirementId,
        resolution.candidateId,
      ),
      resolution,
    ]),
  );
  const excludedAlternativePlateIds = new Set(
    plan.alternativePlates
      .filter((plate) => !plate.included)
      .map((plate) => plate.id),
  );
  const belongsToExcludedAlternativePlate = (scopeId: string) =>
    [...excludedAlternativePlateIds].some((plateId) => {
      const sourceScopeId = `plate-${plateId}`;
      return scopeId === sourceScopeId || scopeId.startsWith(`${sourceScopeId}-`);
    });
  const overridesByScope = new Map<string, ScopeOverride>();
  for (const selection of plan.scopeSelections) {
    if (belongsToExcludedAlternativePlate(selection.scopeId)) continue;
    overridesByScope.set(selection.scopeId, {
      scopeId: selection.scopeId,
      strategy: selection.strategy,
      assignments: selection.assignments
        .filter((assignment) => availableSpoolIds.has(assignment.spoolId))
        .map((assignment) => ({ ...assignment })),
      approvedColorFallbacks: (selection.approvedColorFallbacks ?? [])
        .filter((approval) =>
          currentResolutions.get(
            resolutionKey(
              selection.scopeId,
              approval.requirementId,
              approval.candidateId,
            ),
          )?.colorApproved,
        )
        .map((approval) => ({ ...approval })),
      materialSubstitutions: (selection.materialSubstitutions ?? [])
        .filter((substitution) => {
          const resolution = currentResolutions.get(
            resolutionKey(
              selection.scopeId,
              substitution.requirementId,
              substitution.candidateId,
            ),
          );
          return (
            resolution?.colorApproved === true &&
            resolution.materialApproved &&
            resolution.requiresMaterialSubstitution &&
            substitution.acknowledged &&
            substitution.sourceMaterial === resolution.sourceMaterial &&
            substitution.targetMaterial === resolution.targetMaterial
          );
        })
        .map((substitution) => ({ ...substitution })),
    });
  }
  const visibleOverrides = new Set<string>();
  for (const plate of plan.plates) {
    if (
      plate.strategy === "a1-mono" ||
      belongsToExcludedAlternativePlate(plate.scopeId)
    ) {
      continue;
    }
    const retainDirectAssignments =
      plate.strategy === "direct" || allowDirectPaletteReduction;
    const assignments =
      retainDirectAssignments
        ? (plate.mappings ?? []).flatMap((mapping) =>
            mapping.selectedSpoolId &&
            availableSpoolIds.has(mapping.selectedSpoolId)
              ? [
                  {
                    requirementId: mapping.id,
                    spoolId: mapping.selectedSpoolId,
                    toolhead: mapping.directToolhead,
                    allowMaterialSubstitution:
                      mapping.materialSubstitutionAcknowledged ?? false,
                  },
                ]
              : [],
          )
        : [];
    const visibleRequirementIds = new Set(
      retainDirectAssignments
        ? (plate.mappings ?? []).map((mapping) => mapping.id)
        : [],
    );
    const existing = overridesByScope.get(plate.scopeId);
    if (!visibleOverrides.has(plate.scopeId)) {
      const assignmentsByRequirement = new Map(
        [
          ...(existing?.assignments ?? []).filter(
            (assignment) =>
              !visibleRequirementIds.has(assignment.requirementId),
          ),
          ...assignments,
        ].map((assignment) => [assignment.requirementId, assignment]),
      );
      overridesByScope.set(plate.scopeId, {
        scopeId: plate.scopeId,
        strategy: plate.strategy === "direct" ? "direct" : "cmyx",
        assignments: [...assignmentsByRequirement.values()],
        approvedColorFallbacks: existing?.approvedColorFallbacks ?? [],
        materialSubstitutions: existing?.materialSubstitutions ?? [],
      });
      visibleOverrides.add(plate.scopeId);
      continue;
    }
    if (!existing) continue;
    const assignmentsByRequirement = new Map(
      [
        ...existing.assignments.filter(
          (assignment) =>
            !visibleRequirementIds.has(assignment.requirementId),
        ),
        ...assignments,
      ].map((assignment) => [assignment.requirementId, assignment]),
    );
    overridesByScope.set(plate.scopeId, {
      scopeId: plate.scopeId,
      strategy: plate.strategy === "direct" ? "direct" : "cmyx",
      assignments: [...assignmentsByRequirement.values()],
      approvedColorFallbacks: existing.approvedColorFallbacks,
      materialSubstitutions: existing.materialSubstitutions,
    });
  }

  for (const resolution of plan.colorResolutions) {
    const existing = overridesByScope.get(resolution.scopeId);
    if (!existing) continue;

    const approvedColorFallbacks = resolution.colorApproved
      ? [
          ...existing.approvedColorFallbacks.filter(
            (approval) => approval.requirementId !== resolution.requirementId,
          ),
          {
            requirementId: resolution.requirementId,
            candidateId: resolution.candidateId,
          },
        ]
      : existing.approvedColorFallbacks.filter(
          (approval) => approval.requirementId !== resolution.requirementId,
        );
    const materialSubstitutions =
      resolution.colorApproved &&
      resolution.materialApproved &&
      resolution.requiresMaterialSubstitution
      ? [
          ...existing.materialSubstitutions.filter(
            (substitution) =>
              substitution.requirementId !== resolution.requirementId,
          ),
          {
            requirementId: resolution.requirementId,
            candidateId: resolution.candidateId,
            sourceMaterial: resolution.sourceMaterial,
            targetMaterial: resolution.targetMaterial,
            acknowledged: true as const,
          },
        ]
      : existing.materialSubstitutions.filter(
          (substitution) =>
            substitution.requirementId !== resolution.requirementId,
        );
    overridesByScope.set(resolution.scopeId, {
      ...existing,
      approvedColorFallbacks,
      materialSubstitutions,
    });
  }
  const scopeOverrides = [...overridesByScope.values()];

  return {
    defaultStrategy,
    confirmedSpools: plan.spools
      .filter((spool) => spool.source === "user" && spool.available)
      .map(({ id, name, colorName, hex, material, sku, profile, colorBasis, available }) => ({
        id,
        name,
        colorName,
        hex,
        material,
        ...(sku ? { sku } : {}),
        ...(profile ? { profile } : {}),
        colorBasis,
        available,
      })),
    scopeOverrides,
    unitPrinterOverrides: plan.unitPrinterSelections.map((selection) => ({
      ...selection,
      preference:
        !a1MiniEnabled && selection.preference === "a1-mini"
          ? "auto"
          : selection.preference,
    })),
    currentLoadout: plan.currentLoadout
      .filter((loadout) => availableSpoolIds.has(loadout.spoolId))
      .map((loadout) => ({ ...loadout })),
    currentA1SpoolId:
      plan.currentA1SpoolId && availableSpoolIds.has(plan.currentA1SpoolId)
        ? plan.currentA1SpoolId
        : null,
    restoreCmyAfterDirect,
    a1MiniEnabled,
    allowDirectPaletteReduction,
    includedAlternativePlateIds: plan.alternativePlates
      .filter((plate) => plate.included)
      .map((plate) => plate.id),
  };
}

export function projectDirectPaletteToolheads(
  plan: ProjectPlan,
  choices: ProjectDirectPaletteChoice[],
) {
  const choiceByMapping = new Map(
    choices.map((choice) => [choice.mappingId, choice]),
  );
  const uniqueSpoolIds = plan.projectDirectPalette.mappings
    .map((mapping) => choiceByMapping.get(mapping.id)?.spoolId ?? "")
    .filter((spoolId, index, values) => spoolId && values.indexOf(spoolId) === index);
  const assigned = new Map<string, ToolheadId>();
  const usedToolheads = new Set<ToolheadId>();

  for (const spoolId of uniqueSpoolIds) {
    const existing = plan.projectDirectPalette.mappings.find(
      (mapping) =>
        mapping.selectedSpoolId === spoolId &&
        !usedToolheads.has(mapping.directToolhead),
    );
    const loaded = plan.currentLoadout.find(
      (entry) => entry.spoolId === spoolId && !usedToolheads.has(entry.toolhead),
    );
    const toolhead = existing?.directToolhead ?? loaded?.toolhead;
    if (!toolhead) continue;
    assigned.set(spoolId, toolhead);
    usedToolheads.add(toolhead);
  }

  for (const spoolId of uniqueSpoolIds) {
    if (assigned.has(spoolId)) continue;
    const toolhead = toolheads.find((candidate) => !usedToolheads.has(candidate));
    if (!toolhead) break;
    assigned.set(spoolId, toolhead);
    usedToolheads.add(toolhead);
  }
  return assigned;
}

/**
 * Applies only user intent to the current view. The native planner remains the
 * authority: `buildReplanRequest` serializes these exact scope assignments and
 * the UI stays stale until the backend returns a fresh plan.
 */
export function applyProjectDirectPalette(
  plan: ProjectPlan,
  choices: ProjectDirectPaletteChoice[],
): ProjectPlan {
  const palette = plan.projectDirectPalette;
  if (!palette.available || palette.mappings.length === 0) {
    throw new Error(
      palette.unavailableReason ??
        "Project-wide Direct Spools is unavailable for this project.",
    );
  }

  const choiceByMapping = new Map(
    choices.map((choice) => [choice.mappingId, choice]),
  );
  const availableSpoolById = new Map(
    inStockSpools(plan.spools).map((spool) => [spool.id, spool]),
  );
  const spoolByPhysicalIdentity = new Map<string, string>();
  for (const mapping of palette.mappings) {
    const choice = choiceByMapping.get(mapping.id);
    const spool = choice ? availableSpoolById.get(choice.spoolId) : undefined;
    if (!choice || !spool) {
      throw new Error(
        `Choose one in-stock physical spool for ${mapping.sourceMaterial} ${mapping.sourceHex}.`,
      );
    }
    if (
      spool.material !== mapping.sourceMaterial &&
      !choice.materialSubstitutionAcknowledged
    ) {
      throw new Error(
        `Acknowledge the ${mapping.sourceMaterial} to ${spool.material} material change for ${mapping.sourceHex}.`,
      );
    }
    const existingSpool = spoolByPhysicalIdentity.get(mapping.physicalIdentityId);
    if (existingSpool && existingSpool !== spool.id) {
      throw new Error(
        `Semantic source rows in physical identity ${mapping.physicalIdentityId} must use one shared spool.`,
      );
    }
    spoolByPhysicalIdentity.set(mapping.physicalIdentityId, spool.id);
  }
  if (new Set(spoolByPhysicalIdentity.values()).size > palette.maximumPairCount) {
    throw new Error(
      `Project-wide Direct Spools can load at most ${palette.maximumPairCount} physical spools.`,
    );
  }

  const toolheadBySpool = projectDirectPaletteToolheads(plan, choices);
  const assignmentsByScope = new Map<
    string,
    ScopeOverride["assignments"]
  >();
  const mappingByRequirement = new Map<string, string>();
  const scopedRequirementKey = (scopeId: string, requirementId: string) =>
    JSON.stringify([scopeId, requirementId]);
  const updatedMappings = palette.mappings.map((mapping) => {
    const choice = choiceByMapping.get(mapping.id)!;
    const spool = availableSpoolById.get(choice.spoolId)!;
    const toolhead = toolheadBySpool.get(spool.id);
    if (!toolhead) {
      throw new Error("Project-wide Direct Spools could not allocate T1–T4 safely.");
    }
    const materialSubstitutionAcknowledged =
      spool.material !== mapping.sourceMaterial &&
      choice.materialSubstitutionAcknowledged;
    for (const reference of mapping.references) {
      const assignments = assignmentsByScope.get(reference.scopeId) ?? [];
      for (const requirementId of reference.requirementIds) {
        assignments.push({
          requirementId,
          spoolId: spool.id,
          toolhead,
          allowMaterialSubstitution: materialSubstitutionAcknowledged,
        });
        mappingByRequirement.set(
          scopedRequirementKey(reference.scopeId, requirementId),
          mapping.id,
        );
      }
      assignmentsByScope.set(reference.scopeId, assignments);
    }
    return {
      ...mapping,
      selectedSpoolId: spool.id,
      directToolhead: toolhead,
      materialSubstitutionAcknowledged,
    };
  });

  const existingScopeIds = new Set(
    plan.scopeSelections.map((selection) => selection.scopeId),
  );
  const scopeSelections = [
    ...plan.scopeSelections.map((selection) => {
      const assignments = assignmentsByScope.get(selection.scopeId);
      return assignments
        ? {
            ...selection,
            strategy: "direct" as const,
            assignments,
            approvedColorFallbacks: [],
            materialSubstitutions: [],
          }
        : selection;
    }),
    ...[...assignmentsByScope.entries()]
      .filter(([scopeId]) => !existingScopeIds.has(scopeId))
      .map(([scopeId, assignments]) => ({
        scopeId,
        strategy: "direct" as const,
        assignments,
        approvedColorFallbacks: [],
        materialSubstitutions: [],
      })),
  ];
  const updatedMappingById = new Map(
    updatedMappings.map((mapping) => [mapping.id, mapping]),
  );
  const targetedScopeIds = new Set(assignmentsByScope.keys());

  return {
    ...plan,
    projectDirectPalette: { ...palette, mappings: updatedMappings },
    scopeSelections,
    plates: plan.plates.map((plate) => {
      if (plate.printer !== "U1" || !targetedScopeIds.has(plate.scopeId)) {
        return plate;
      }
      return {
        ...plate,
        strategy: "direct" as const,
        mappings: plate.mappings?.map((mapping) => {
          const projectMappingId = mappingByRequirement.get(
            scopedRequirementKey(plate.scopeId, mapping.id),
          );
          const projectMapping = projectMappingId
            ? updatedMappingById.get(projectMappingId)
            : undefined;
          return projectMapping
            ? {
                ...mapping,
                selectedSpoolId: projectMapping.selectedSpoolId,
                directToolhead: projectMapping.directToolhead,
                materialSubstitutionAcknowledged:
                  projectMapping.materialSubstitutionAcknowledged,
              }
            : mapping;
        }),
      };
    }),
  };
}

export function invalidateColorApprovals(plan: ProjectPlan): ProjectPlan {
  return {
    ...plan,
    colorResolutions: plan.colorResolutions.map((resolution) => ({
      ...resolution,
      colorApproved: false,
      materialApproved: false,
    })),
    scopeSelections: plan.scopeSelections.map((selection) => ({
      ...selection,
      approvedColorFallbacks: [],
      materialSubstitutions: [],
    })),
  };
}

function batchKey(plate: PlatePlan) {
  if (plate.strategy === "a1-mono") {
    return `${plate.printer}:${plate.strategy}:${plate.loadoutLabel}`;
  }
  if (plate.strategy === "direct") return `${plate.printer}:direct`;
  return `${plate.printer}:${plate.strategy}:${plate.loadoutLabel}`;
}

function batchLabel(plate: PlatePlan) {
  if (plate.strategy === "a1-mono") return "A1 Mono";
  if (plate.strategy === "direct") return "Direct Spools";
  return plate.strategy === "cmyx-solid" ? "CMY+X Solid" : plate.loadoutLabel;
}

export function deriveBatches(plates: PlatePlan[]): BatchSegment[] {
  return plates.reduce<BatchSegment[]>((segments, plate) => {
    const key = batchKey(plate);
    const last = segments.at(-1);
    if (last?.id === key) {
      last.endOrder = plate.order;
      last.plateCount += 1;
      return segments;
    }

    segments.push({
      id: key,
      label: batchLabel(plate),
      detail:
        plate.strategy === "direct"
          ? "Full T1–T4 setup boundary"
          : plate.strategy === "a1-mono"
            ? plate.loadoutLabel
            : strategyLabel[plate.strategy],
      strategy: plate.strategy,
      startOrder: plate.order,
      endOrder: plate.order,
      plateCount: 1,
      printer: plate.printer,
      setupActions: [],
    });
    return segments;
  }, []);
}

export function derivePlanStats(plan: ProjectPlan) {
  return {
    targetPlates: plan.plates.length,
    batchCount: plan.batches.length,
    t4Swaps: plan.t4SwapCount,
    a1SpoolChanges: plan.a1SpoolChangeCount,
    fastMonoObjects: plan.plates
      .filter((plate) => plate.isFastMono)
      .reduce((count, plate) => count + plate.objectCount, 0),
    warnings:
      plan.globalWarnings.length +
      plan.plates.reduce(
        (count, plate) => count + plate.warnings.length,
        0,
      ),
  };
}

export function selectedSpoolForToolhead(
  mappings: DirectColorMapping[],
  toolhead: ToolheadId,
) {
  return mappings.find((mapping) => mapping.directToolhead === toolhead)
    ?.selectedSpoolId;
}

export function getToolheadActions(
  spools: PhysicalSpool[],
  currentLoadout: LoadedToolhead[],
  mappings: DirectColorMapping[],
  toolhead: ToolheadId,
  restoreCmy: boolean,
): ToolheadAction[] {
  const selectedMapping = mappings.find(
    (mapping) => mapping.directToolhead === toolhead,
  );
  const currentSpoolId = currentLoadout.find(
    (entry) => entry.toolhead === toolhead,
  )?.spoolId;
  const selectedSpoolId = selectedMapping?.selectedSpoolId;
  const spoolById = new Map(spools.map((spool) => [spool.id, spool]));
  const currentName = currentSpoolId
    ? spoolById.get(currentSpoolId)?.colorName ?? "Unknown spool"
    : "Unknown spool";
  const selectedName = selectedSpoolId
    ? spoolById.get(selectedSpoolId)?.colorName ?? "Unassigned"
    : "Unassigned";

  if (!selectedMapping) {
    return [{ kind: "Keep", spoolName: currentName }];
  }

  const selectedSpool = selectedSpoolId
    ? spoolById.get(selectedSpoolId)
    : undefined;
  if (!selectedSpool || !selectedSpool.available) {
    return [
      {
        kind: "Review",
        spoolName: selectedSpool ? "Out of stock" : "Unassigned",
      },
    ];
  }

  if (currentSpoolId === selectedSpoolId) {
    return [{ kind: "Keep", spoolName: currentName }];
  }

  const actions: ToolheadAction[] = [];
  if (currentSpoolId) actions.push({ kind: "Unload", spoolName: currentName });
  if (selectedSpoolId) actions.push({ kind: "Load", spoolName: selectedName });
  if (restoreCmy && currentSpoolId) {
    actions.push({ kind: "Restore", spoolName: currentName });
  }
  return actions;
}

export function directDelta(
  mapping: DirectColorMapping,
  spools: PhysicalSpool[],
) {
  const spool = spools.find(
    (candidate) =>
      candidate.id === mapping.selectedSpoolId && candidate.available,
  );
  return spool ? deltaE00(mapping.sourceHex, spool.hex) : null;
}

export function directQuality(
  mapping: DirectColorMapping,
  spools: PhysicalSpool[],
) {
  const spool = spools.find(
    (candidate) =>
      candidate.id === mapping.selectedSpoolId && candidate.available,
  );
  if (!spool) return "Review" as const;
  if (spool.material !== mapping.sourceMaterial) return "Material mismatch" as const;
  const delta = directDelta(mapping, spools);
  return delta === null ? ("Review" as const) : qualityForDelta(delta);
}

export function directAverageDelta(
  mappings: DirectColorMapping[] | undefined,
  spools: PhysicalSpool[],
) {
  if (!mappings?.length) return null;
  const deltas = mappings.map((mapping) => directDelta(mapping, spools));
  if (deltas.some((delta) => delta === null)) return null;
  return (
    deltas.reduce<number>((total, delta) => total + (delta ?? 0), 0) /
    deltas.length
  );
}

export function cmyAverageDelta(mappings: DirectColorMapping[] | undefined) {
  if (!mappings?.length) return null;
  const deltas = mappings.flatMap((mapping) =>
    mapping.cmyDeltaE00 === null ? [] : [mapping.cmyDeltaE00],
  );
  if (!deltas.length) return null;
  return deltas.reduce((total, delta) => total + delta, 0) / deltas.length;
}

export function plateLoadoutColors(
  plate: PlatePlan,
  spools: PhysicalSpool[],
) {
  if (plate.strategy !== "direct" || !plate.mappings) return plate.loadoutColors;
  return toolheads.flatMap((toolhead) => {
    const spoolId = selectedSpoolForToolhead(plate.mappings ?? [], toolhead);
    const spool = spools.find(
      (candidate) => candidate.id === spoolId && candidate.available,
    );
    return spool ? [spool.hex] : [];
  });
}

export function plateDisplayDelta(plate: PlatePlan, spools: PhysicalSpool[]) {
  return plate.strategy === "direct"
    ? directAverageDelta(plate.mappings, spools)
    : plate.estimatedDeltaE00;
}
