import { useEffect, useMemo, useRef, useState } from "react";
import { TriangleAlert } from "lucide-react";

import {
  ApplicationNavigation,
  type ApplicationView,
} from "./components/ApplicationNavigation";
import { BatchTimeline } from "./components/BatchTimeline";
import { AlternativePlateSelector } from "./components/AlternativePlateSelector";
import { BottomActionRail } from "./components/BottomActionRail";
import { ColorResolutionPanel } from "./components/ColorResolutionPanel";
import { ColorCalibration } from "./components/ColorCalibration";
import { CmyxReference } from "./components/CmyxReference";
import { ConversionDialog } from "./components/ConversionDialog";
import { FilamentLibrary } from "./components/FilamentLibrary";
import { GettingStarted } from "./components/GettingStarted";
import { PlateInspector } from "./components/PlateInspector";
import { PlateTable } from "./components/PlateTable";
import { PlanWarnings } from "./components/PlanWarnings";
import { PartialConversionNotice } from "./components/PartialConversionNotice";
import { PrintRun } from "./components/PrintRun";
import { PrintingSetupControl } from "./components/PrintingSetupControl";
import { ProjectSetupControl } from "./components/ProjectSetupControl";
import { ProjectDirectPalette } from "./components/ProjectDirectPalette";
import { ProjectHeader } from "./components/ProjectHeader";
import { StatusStrip } from "./components/StatusStrip";
import { TitleBar } from "./components/TitleBar";
import { WorkflowRail } from "./components/WorkflowRail";
import { createDemoPlan } from "./data/mock-plan";
import {
  analyzeProject,
  cancelNativeAnalysis,
  cancelNativeConversion,
  chooseProjectPath,
  chooseConversionDestination,
  convertNativeProject,
  exportNativePlan,
  inspectConversionCapabilities,
  isTauriRuntime,
  listenToConversionProgress,
  openConvertedOutput,
  prepareNativeConversion,
  revalidatePublishedConversion,
  replanProject,
  showConvertedOutputInFinder,
} from "./services/project-analysis";
import {
  createCalibrationIdentity,
  loadFilamentLibrary,
  saveFilamentLibrary,
} from "./services/filament-library";
import {
  loadPrintingSetup,
  savePrintingSetup,
  type PrintingSetup,
} from "./services/printing-setup";
import {
  constrainPlanningIntentToEquipment,
  createDefaultPlanningIntent,
  loadPlanningIntent,
  planningIntentEquals,
  savePlanningIntent,
  type PlanningIntent,
} from "./services/planning-intent";
import {
  constrainPrinterLoadoutToEquipment,
  createEmptyPrinterLoadout,
  loadPrinterLoadout,
  printerLoadoutEquals,
  savePrinterLoadout,
  type PrinterLoadoutProfile,
} from "./services/printer-loadout";
import {
  createRecommendedCmyxCalibrationProject,
  deleteCmyxCalibrationRecord,
  loadCmyxCalibrationLibrary,
  setCmyxCalibrationGeometryContext,
  upsertCmyxCalibrationMeasurement,
} from "./services/color-calibration";
import type {
  ActiveConversionOutputAction,
  AnalysisResult,
  CmyxPaletteOption,
  ColorResolution,
  CmyxCalibrationLibraryDocument,
  CmyxCalibrationMeasurementInput,
  CmyxCalibrationProjectInput,
  CmyxCalibrationProjectResult,
  CmyxGeometryContext,
  ConversionCapability,
  ConversionProgress,
  ConversionResult,
  DirectColorMapping,
  NewPhysicalSpoolInput,
  PartialConversionApproval,
  PhysicalSpool,
  PreparedConversion,
  PrinterPreference,
  PrinterLoadoutSnapshot,
  PrintStrategy,
  ProjectPlan,
  ProjectDirectPaletteChoice,
  ProjectSelection,
  PublishedConversionArtifact,
  ToolheadId,
} from "./types";
import {
  createUniqueSpoolId,
  applyProjectDirectPalette,
  buildReplanRequest,
  deriveBatches,
  derivePlanStats,
  directQuality,
  invalidateColorApprovals,
  isDirectStrategyAvailable,
  plateScopeIds,
  toolheads,
} from "./utils/plan";
import {
  createPrintRunFingerprint,
  isPublishedPrintRunBundleReady,
  loadUntrustedPublishedPrintRunBundle,
  savePublishedPrintRunBundle,
  type PublishedPrintRunBundle,
} from "./utils/print-run";

const initialPlan = createDemoPlan();
const BROWSER_OUT_OF_STOCK_ERROR =
  "Browser demo data cannot be revalidated with out-of-stock spools. Use the native app for an authoritative plan.";

function applyBrowserInventoryGuard(
  plan: ProjectPlan,
  spools: PhysicalSpool[],
): ProjectPlan {
  const blockingErrors = plan.blockingErrors.filter(
    (error) => error !== BROWSER_OUT_OF_STOCK_ERROR,
  );
  if (spools.some((spool) => !spool.available)) {
    blockingErrors.push(BROWSER_OUT_OF_STOCK_ERROR);
  }
  return {
    ...plan,
    spools,
    blockingErrors,
    planReady: blockingErrors.length === 0 && plan.omittedUnitCount === 0,
  };
}

function appLocalStorage() {
  if (typeof window === "undefined") return null;
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

function updateSelectedMappings(
  plan: ProjectPlan,
  selectedScopeId: string,
  update: (mappings: DirectColorMapping[]) => DirectColorMapping[],
) {
  return {
    ...plan,
    plates: plan.plates.map((plate) =>
      plateScopeIds(plate).includes(selectedScopeId) && plate.mappings
        ? { ...plate, mappings: update(plate.mappings) }
        : plate,
    ),
  };
}

function physicalIdentityMappingIds(
  mappings: DirectColorMapping[],
  active: DirectColorMapping,
) {
  return new Set(
    mappings
      .filter(
        (mapping) => mapping.physicalIdentityId === active.physicalIdentityId,
      )
      .map((mapping) => mapping.id),
  );
}

function sharedToolheadMappingIds(
  mappings: DirectColorMapping[],
  active: DirectColorMapping,
) {
  const physicalIds = physicalIdentityMappingIds(mappings, active);
  if (!active.selectedSpoolId) return physicalIds;
  for (const mapping of mappings) {
    if (mapping.selectedSpoolId === active.selectedSpoolId) {
      physicalIds.add(mapping.id);
    }
  }
  return physicalIds;
}

function directMappingOverrideKey(scopeId: string, inheritanceKey: string) {
  return JSON.stringify([scopeId, inheritanceKey]);
}

function applySpoolChoiceToMappings(
  mappings: DirectColorMapping[],
  mappingId: string,
  nextSpoolId: string,
): { mappings: DirectColorMapping[]; changed: boolean; error?: string } {
  const active = mappings.find((mapping) => mapping.id === mappingId);
  if (!active) return { mappings, changed: false };

  const activeIdentityIds = physicalIdentityMappingIds(mappings, active);
  if (
    mappings
      .filter((mapping) => activeIdentityIds.has(mapping.id))
      .every((mapping) => mapping.selectedSpoolId === nextSpoolId)
  ) {
    return { mappings, changed: false };
  }

  const assignedOutside = mappings.filter(
    (mapping) =>
      !activeIdentityIds.has(mapping.id) && Boolean(mapping.selectedSpoolId),
  );
  const sharedMapping = nextSpoolId
    ? assignedOutside.find((mapping) => mapping.selectedSpoolId === nextSpoolId)
    : undefined;
  const occupiedToolheads = new Set(
    assignedOutside.map((mapping) => mapping.directToolhead),
  );
  let nextToolhead = sharedMapping?.directToolhead ?? active.directToolhead;
  if (
    nextSpoolId &&
    !sharedMapping &&
    occupiedToolheads.has(active.directToolhead)
  ) {
    const freeToolhead = toolheads.find(
      (toolhead) => !occupiedToolheads.has(toolhead),
    );
    if (!freeToolhead) {
      return {
        mappings,
        changed: false,
        error:
          "No free T1–T4 toolhead is available for that physical spool. Resolve the existing toolhead assignments first.",
      };
    }
    nextToolhead = freeToolhead;
  }

  return {
    changed: true,
    mappings: mappings.map((mapping) =>
      activeIdentityIds.has(mapping.id)
        ? {
            ...mapping,
            selectedSpoolId: nextSpoolId,
            directToolhead: nextToolhead,
            materialSubstitutionAcknowledged: false,
          }
        : mapping,
    ),
  };
}

export default function App() {
  const [plan, setPlan] = useState(initialPlan);
  const [activeView, setActiveView] = useState<ApplicationView>("plan");
  const [printingSetup, setPrintingSetup] = useState<PrintingSetup | null>(() =>
    loadPrintingSetup(appLocalStorage()),
  );
  const [printingSetupPersistence, setPrintingSetupPersistence] = useState<
    "none" | "persisted" | "session"
  >(printingSetup ? "persisted" : "none");
  const [planningIntent, setPlanningIntent] = useState<PlanningIntent>(() =>
    constrainPlanningIntentToEquipment(
      loadPlanningIntent(appLocalStorage()) ??
        createDefaultPlanningIntent(printingSetup),
      printingSetup,
    ),
  );
  const [printerLoadout, setPrinterLoadout] = useState<PrinterLoadoutProfile>(
    () =>
      constrainPrinterLoadoutToEquipment(
        loadPrinterLoadout(appLocalStorage()) ?? createEmptyPrinterLoadout(),
        printingSetup,
      ),
  );
  const [appliedPlanningIntent, setAppliedPlanningIntent] =
    useState<PlanningIntent | null>(null);
  const [appliedPrinterLoadout, setAppliedPrinterLoadout] =
    useState<PrinterLoadoutProfile | null>(null);
  const [isProjectSetupConfirmed, setIsProjectSetupConfirmed] = useState(false);
  const [isPrintingSetupOpen, setIsPrintingSetupOpen] = useState(false);
  const [isSetupRecommendationOpen, setIsSetupRecommendationOpen] =
    useState(false);
  const [librarySpools, setLibrarySpools] = useState<PhysicalSpool[]>(
    initialPlan.spools,
  );
  const [isLibraryLoading, setIsLibraryLoading] = useState(true);
  const [isLibrarySaving, setIsLibrarySaving] = useState(false);
  const [libraryError, setLibraryError] = useState("");
  const [libraryErrorKind, setLibraryErrorKind] = useState<
    "load" | "save" | null
  >(null);
  const [calibrationLibrary, setCalibrationLibrary] =
    useState<CmyxCalibrationLibraryDocument | null>(null);
  const [isCalibrationLoading, setIsCalibrationLoading] = useState(false);
  const [isCalibrationSaving, setIsCalibrationSaving] = useState(false);
  const [isCalibrationProjectGenerating, setIsCalibrationProjectGenerating] =
    useState(false);
  const [calibrationError, setCalibrationError] = useState("");
  const [calibrationErrorKind, setCalibrationErrorKind] = useState<
    "load" | "mutation" | null
  >(null);
  const [hasAnalysis, setHasAnalysis] = useState(false);
  const [analysisSource, setAnalysisSource] = useState<
    AnalysisResult["source"] | null
  >(null);
  const [selectedPlateId, setSelectedPlateId] = useState("");
  const [isInspectorOpen, setIsInspectorOpen] = useState(false);
  const [pendingSelection, setPendingSelection] =
    useState<ProjectSelection | null>(null);
  const [isAnalyzing, setIsAnalyzing] = useState(false);
  const [isCancellingAnalysis, setIsCancellingAnalysis] = useState(false);
  const [isValidating, setIsValidating] = useState(false);
  const [activeSourcePath, setActiveSourcePath] = useState<string | null>(null);
  const [a1MiniEnabled, setA1MiniEnabled] = useState(
    planningIntent.a1MiniEnabled,
  );
  const [appliedA1MiniEnabled, setAppliedA1MiniEnabled] = useState(false);
  const [customDirectPalettesEnabled, setCustomDirectPalettesEnabled] =
    useState(initialPlan.customDirectPalettesEnabled);
  const [
    appliedCustomDirectPalettesEnabled,
    setAppliedCustomDirectPalettesEnabled,
  ] = useState(initialPlan.customDirectPalettesEnabled);
  const [colorNeedsRecalculation, setColorNeedsRecalculation] = useState(false);
  const [analysisError, setAnalysisError] = useState("");
  const [customPaletteFailure, setCustomPaletteFailure] = useState<{
    scopeId: string;
    message: string;
  } | null>(null);
  const [restoreCmy, setRestoreCmy] = useState(false);
  const [isPlanDirty, setIsPlanDirty] = useState(false);
  const [liveMessage, setLiveMessage] = useState("");
  const [conversionCapability, setConversionCapability] =
    useState<ConversionCapability | null>(null);
  const [authoritativePlanRevision, setAuthoritativePlanRevision] = useState(0);
  const [isCheckingConversion, setIsCheckingConversion] = useState(false);
  const [isPreparingConversion, setIsPreparingConversion] = useState(false);
  const [isConverting, setIsConverting] = useState(false);
  const [conversionDestination, setConversionDestination] = useState("");
  const [preparedConversion, setPreparedConversion] =
    useState<PreparedConversion | null>(null);
  const [conversionProgress, setConversionProgress] =
    useState<ConversionProgress | null>(null);
  const [conversionResult, setConversionResult] =
    useState<ConversionResult | null>(null);
  const [publishedPrintRunBundle, setPublishedPrintRunBundle] =
    useState<PublishedPrintRunBundle | null>(null);
  const [preferredPrintArtifacts, setPreferredPrintArtifacts] = useState<
    PublishedConversionArtifact[]
  >([]);
  const [printRunRecoveryStatus, setPrintRunRecoveryStatus] = useState<
    "idle" | "checking" | "recovered"
  >("idle");
  const [printRunRecoveryError, setPrintRunRecoveryError] = useState("");
  const [printRunRecoveryRevision, setPrintRunRecoveryRevision] = useState(0);
  const [conversionError, setConversionError] = useState("");
  const [conversionNeedsNewPreflight, setConversionNeedsNewPreflight] =
    useState(false);
  const [activeConversionOutputAction, setActiveConversionOutputAction] =
    useState<ActiveConversionOutputAction | null>(null);
  const [
    partialConversionAcknowledgementKey,
    setPartialConversionAcknowledgementKey,
  ] = useState<string | null>(null);
  const [
    experimentalDialectAcknowledgementKey,
    setExperimentalDialectAcknowledgementKey,
  ] = useState<string | null>(null);
  const librarySpoolsRef = useRef<PhysicalSpool[]>(initialPlan.spools);
  const libraryLoadQueue = useRef<Promise<void>>(Promise.resolve());
  const librarySaveQueue = useRef<Promise<void>>(Promise.resolve());
  const librarySaveVersion = useRef(0);
  const librarySaveFailure = useRef("");
  const calibrationLoadActive = useRef(false);
  const calibrationSaveActive = useRef(false);
  const calibrationProjectGenerationActive = useRef(false);
  const analysisRequestGenerationRef = useRef(0);
  const calibrationLoadQueue = useRef<Promise<void>>(Promise.resolve());
  const calibrationMutationQueue = useRef<Promise<void>>(Promise.resolve());
  const activeConversionIdRef = useRef<string | null>(null);
  const printRunRecoveryAttemptRef = useRef("");
  const explicitDirectMappingOverridesRef = useRef(new Set<string>());
  const browserFileInputRef = useRef<HTMLInputElement>(null);
  const isNative = isTauriRuntime();
  const a1MiniConfigured = printingSetup?.secondaryPrinter === "a1-mini";
  const a1MiniNeedsRecalculation = a1MiniEnabled !== appliedA1MiniEnabled;
  const planningIntentNeedsRecalculation =
    hasAnalysis && !planningIntentEquals(planningIntent, appliedPlanningIntent);
  const printerLoadoutNeedsRecalculation =
    hasAnalysis && !printerLoadoutEquals(printerLoadout, appliedPrinterLoadout);
  const customDirectPalettesNeedsRecalculation =
    customDirectPalettesEnabled !== appliedCustomDirectPalettesEnabled;
  const hasPendingPlanChanges =
    isPlanDirty ||
    a1MiniNeedsRecalculation ||
    planningIntentNeedsRecalculation ||
    printerLoadoutNeedsRecalculation ||
    customDirectPalettesNeedsRecalculation;
  const selectedPlate = hasAnalysis
    ? (plan.plates.find((plate) => plate.id === selectedPlateId) ??
      plan.plates[0])
    : undefined;
  const stats = useMemo(() => derivePlanStats(plan), [plan]);
  const executionPlanFingerprint = useMemo(
    () => createPrintRunFingerprint(plan),
    [plan],
  );
  const publishedPrintRunBundleIdentity = publishedPrintRunBundle
    ? `${publishedPrintRunBundle.result.outputDirectory}:${publishedPrintRunBundle.backendPlanFingerprint}`
    : "";

  useEffect(() => {
    setPreferredPrintArtifacts([]);
  }, [publishedPrintRunBundleIdentity]);
  const partialConversionEvidenceKey = useMemo(
    () =>
      JSON.stringify({
        sourceHash: plan.summary.sourceHash,
        partialConversion: plan.partialConversion,
        plates: plan.plates,
        batches: plan.batches,
        scopeSelections: plan.scopeSelections,
        unitPrinterSelections: plan.unitPrinterSelections,
        alternativePlates: plan.alternativePlates,
        currentLoadout: plan.currentLoadout,
        currentA1SpoolId: plan.currentA1SpoolId,
        spools: plan.spools,
        colorResolutions: plan.colorResolutions,
        backendPlanFingerprint: conversionCapability?.planFingerprint ?? null,
        authoritativePlanRevision,
        a1MiniEnabled,
        customDirectPalettesEnabled,
        restoreCmy,
        isPlanDirty,
      }),
    [
      a1MiniEnabled,
      customDirectPalettesEnabled,
      authoritativePlanRevision,
      conversionCapability?.planFingerprint,
      isPlanDirty,
      plan,
      restoreCmy,
    ],
  );
  const partialConversionAcknowledged =
    partialConversionAcknowledgementKey === partialConversionEvidenceKey;
  const experimentalDialectEvidenceKey =
    conversionCapability?.experimentalDialectApprovalRequired &&
    conversionCapability.experimentalDialectFingerprint
      ? `${plan.summary.sourceHash}:${conversionCapability.experimentalDialectFingerprint}`
      : null;
  const experimentalDialectAcknowledged =
    experimentalDialectEvidenceKey === null ||
    experimentalDialectAcknowledgementKey === experimentalDialectEvidenceKey;

  useEffect(() => {
    if (
      partialConversionAcknowledgementKey !== null &&
      partialConversionAcknowledgementKey !== partialConversionEvidenceKey
    ) {
      setPartialConversionAcknowledgementKey(null);
    }
  }, [partialConversionAcknowledgementKey, partialConversionEvidenceKey]);
  useEffect(() => {
    if (
      experimentalDialectAcknowledgementKey !== null &&
      experimentalDialectAcknowledgementKey !== experimentalDialectEvidenceKey
    ) {
      setExperimentalDialectAcknowledgementKey(null);
    }
  }, [experimentalDialectAcknowledgementKey, experimentalDialectEvidenceKey]);
  useEffect(() => {
    let canceled = false;
    const load = async () => {
      try {
        const library = await loadFilamentLibrary(initialPlan.spools);
        if (canceled) return;
        librarySpoolsRef.current = library.spools;
        setLibrarySpools(library.spools);
        setPlan((current) => ({ ...current, spools: library.spools }));
        librarySaveFailure.current = "";
        setLibraryErrorKind(null);
      } catch (error) {
        if (canceled) return;
        const message = error instanceof Error ? error.message : String(error);
        librarySaveFailure.current = message;
        setLibraryErrorKind("load");
        setLibraryError(`Filament library could not be loaded. ${message}`);
        setLiveMessage("Filament library could not be loaded.");
      } finally {
        if (!canceled) setIsLibraryLoading(false);
      }
    };
    libraryLoadQueue.current = load();
    return () => {
      canceled = true;
    };
  }, []);

  const loadCalibrationLibraryView = async () => {
    if (calibrationLibrary || calibrationLoadActive.current) return;
    calibrationLoadActive.current = true;
    setIsCalibrationLoading(true);
    setCalibrationError("");
    setCalibrationErrorKind(null);
    const operation = (async () => {
      try {
        const loaded = await loadCmyxCalibrationLibrary();
        setCalibrationLibrary(loaded);
        setCalibrationErrorKind(null);
        setLiveMessage("CMY+X calibration library loaded.");
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        setCalibrationErrorKind("load");
        setCalibrationError(
          `CMY+X calibration library could not be loaded. ${message}`,
        );
        setLiveMessage("CMY+X calibration library could not be loaded.");
      } finally {
        calibrationLoadActive.current = false;
        setIsCalibrationLoading(false);
      }
    })();
    calibrationLoadQueue.current = operation;
    await operation;
  };

  const invalidatePlanAfterCalibrationChange = () => {
    if (hasAnalysis) {
      setPlan((current) => ({
        ...invalidateColorApprovals(current),
        planReady: false,
      }));
      setIsPlanDirty(true);
      setColorNeedsRecalculation(true);
    }
    setConversionCapability(null);
    setPublishedPrintRunBundle(null);
    setPrintRunRecoveryStatus("idle");
    setPrintRunRecoveryError("");
    printRunRecoveryAttemptRef.current = "";
    setPreparedConversion(null);
    setConversionResult(null);
    setConversionProgress(null);
    setConversionDestination("");
    setConversionError("");
    setConversionNeedsNewPreflight(false);
    activeConversionIdRef.current = null;
  };

  const runCalibrationMutation = async (
    operation: () => Promise<CmyxCalibrationLibraryDocument>,
    successMessage: string,
  ) => {
    if (calibrationSaveActive.current || !calibrationLibrary) return false;
    calibrationSaveActive.current = true;
    setIsCalibrationSaving(true);
    setCalibrationError("");
    setCalibrationErrorKind(null);

    const mutation = (async () => {
      try {
        await calibrationLoadQueue.current;
        const saved = await operation();
        setCalibrationLibrary(saved);
        invalidatePlanAfterCalibrationChange();
        setLiveMessage(successMessage);
        return true;
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        setCalibrationErrorKind("mutation");
        setCalibrationError(
          `CMY+X calibration change could not be saved. ${message}`,
        );
        setLiveMessage("CMY+X calibration change could not be saved.");
        return false;
      } finally {
        calibrationSaveActive.current = false;
        setIsCalibrationSaving(false);
      }
    })();
    calibrationMutationQueue.current = mutation.then(() => undefined);
    return mutation;
  };

  const saveCalibrationMeasurement = (
    measurement: CmyxCalibrationMeasurementInput,
  ) =>
    runCalibrationMutation(
      () => upsertCmyxCalibrationMeasurement(measurement),
      `${measurement.id} saved as a measured CMY+X sample.${
        hasAnalysis ? " Recalculate the print plan before conversion." : ""
      }`,
    );

  const deleteCalibrationRecord = (recordId: string) =>
    runCalibrationMutation(
      () => deleteCmyxCalibrationRecord(recordId),
      `${recordId} deleted from measured CMY+X samples.${
        hasAnalysis ? " Recalculate the print plan before conversion." : ""
      }`,
    );

  const saveCalibrationPlanningGeometry = (
    geometryContext: CmyxGeometryContext,
  ) =>
    runCalibrationMutation(
      () => setCmyxCalibrationGeometryContext(geometryContext),
      `Planning geometry saved.${
        hasAnalysis ? " Recalculate the print plan before conversion." : ""
      }`,
    );

  const generateCalibrationProject = async (
    input: CmyxCalibrationProjectInput,
  ): Promise<CmyxCalibrationProjectResult | null> => {
    if (calibrationProjectGenerationActive.current) return null;
    calibrationProjectGenerationActive.current = true;
    setIsCalibrationProjectGenerating(true);
    try {
      const result = await createRecommendedCmyxCalibrationProject(input);
      if (result) {
        setLiveMessage(
          `${result.fileName} generated and validated as a ${result.swatchCount}-swatch qualification candidate.`,
        );
      }
      return result;
    } finally {
      calibrationProjectGenerationActive.current = false;
      setIsCalibrationProjectGenerating(false);
    }
  };

  useEffect(() => {
    const viewName = {
      plan: "Print Plan",
      library: "Filament Library",
      calibration: "Color Calibration",
      reference: "Color Reference",
      run: "Print Run",
    }[activeView];
    document.title = `${viewName} | U1 3MF Color Planner`;
    document.getElementById("main-content")?.focus();
  }, [activeView]);

  useEffect(() => {
    if (!isNative) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listenToConversionProgress((progress) => {
      if (disposed || progress.conversionId !== activeConversionIdRef.current) {
        return;
      }
      setConversionProgress(progress);
      setLiveMessage(progress.message);
    }).then((stopListening) => {
      if (disposed) stopListening();
      else unlisten = stopListening;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [isNative]);

  const reconcilePlanInventory = (
    current: ProjectPlan,
    nextSpools: PhysicalSpool[],
  ) => {
    const availableIds = new Set(
      nextSpools.filter((spool) => spool.available).map((spool) => spool.id),
    );
    const updated = {
      ...current,
      spools: nextSpools,
      currentLoadout: current.currentLoadout.filter((entry) =>
        availableIds.has(entry.spoolId),
      ),
    };
    const invalidated = hasAnalysis
      ? invalidateColorApprovals(updated)
      : updated;
    return analysisSource === "browser-demo"
      ? applyBrowserInventoryGuard(invalidated, nextSpools)
      : invalidated;
  };

  const queueLibrarySave = (
    nextSpools: PhysicalSpool[],
    successMessage: string,
  ) => {
    const version = librarySaveVersion.current + 1;
    librarySaveVersion.current = version;
    setIsLibrarySaving(true);
    setLibraryError("");
    setLibraryErrorKind(null);

    const previousSave = librarySaveQueue.current;
    const operation = libraryLoadQueue.current
      .then(() => previousSave.catch(() => undefined))
      .then(async () => {
        const saved = await saveFilamentLibrary(nextSpools);
        if (version !== librarySaveVersion.current) return false;
        librarySaveFailure.current = "";
        setLibraryErrorKind(null);
        librarySpoolsRef.current = saved.spools;
        setLibrarySpools(saved.spools);
        setPlan((current) => reconcilePlanInventory(current, saved.spools));
        setLiveMessage(successMessage);
        return true;
      })
      .catch((error) => {
        if (version !== librarySaveVersion.current) return false;
        const message = error instanceof Error ? error.message : String(error);
        librarySaveFailure.current = message;
        setLibraryErrorKind("save");
        setLibraryError(`Filament library could not be saved. ${message}`);
        setLiveMessage(
          "Filament library could not be saved. Replanning is blocked.",
        );
        return false;
      })
      .finally(() => {
        if (version === librarySaveVersion.current) setIsLibrarySaving(false);
      });

    librarySaveQueue.current = operation.then(() => undefined);
    return operation;
  };

  const applyLibraryChange = (
    nextSpools: PhysicalSpool[],
    successMessage: string,
  ) => {
    if (isLibraryLoading || libraryErrorKind === "load") {
      setLiveMessage(
        "Wait for the filament library to load before editing it.",
      );
      return;
    }
    librarySpoolsRef.current = nextSpools;
    setLibrarySpools(nextSpools);
    setPlan((current) => reconcilePlanInventory(current, nextSpools));
    if (hasAnalysis) {
      setIsPlanDirty(true);
      setColorNeedsRecalculation(true);
    }
    void queueLibrarySave(nextSpools, successMessage);
  };

  const selectPlate = (plateId: string) => {
    const plate = plan.plates.find((candidate) => candidate.id === plateId);
    if (!plate) return;
    setSelectedPlateId(plateId);
    setIsInspectorOpen(true);
    setLiveMessage(`${plate.title} selected for inspection.`);
  };

  const chooseNativeProject = async () => {
    try {
      const selection = await chooseProjectPath();
      if (selection) {
        const startsNewProject = hasAnalysis;
        analysisRequestGenerationRef.current += 1;
        setPendingSelection(selection);
        setIsAnalyzing(false);
        setIsCancellingAnalysis(false);
        setHasAnalysis(false);
        setAnalysisSource(null);
        setSelectedPlateId("");
        setIsInspectorOpen(false);
        setIsSetupRecommendationOpen(false);
        setActiveSourcePath(null);
        setA1MiniEnabled(planningIntent.a1MiniEnabled);
        setAppliedA1MiniEnabled(false);
        setAppliedPlanningIntent(null);
        setAppliedPrinterLoadout(null);
        if (startsNewProject) setIsProjectSetupConfirmed(false);
        setCustomDirectPalettesEnabled(false);
        setAppliedCustomDirectPalettesEnabled(false);
        explicitDirectMappingOverridesRef.current.clear();
        setColorNeedsRecalculation(false);
        setAnalysisError("");
        setCustomPaletteFailure(null);
        setIsPlanDirty(false);
        setAuthoritativePlanRevision((revision) => revision + 1);
        setConversionCapability(null);
        setPartialConversionAcknowledgementKey(null);
        setPublishedPrintRunBundle(null);
        setPrintRunRecoveryStatus("idle");
        setPrintRunRecoveryError("");
        printRunRecoveryAttemptRef.current = "";
        setLiveMessage(
          `${selection.fileName} selected. Choose Analyze Project to continue.`,
        );
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setAnalysisError(`The project picker could not open. ${message}`);
      setLiveMessage("The project picker could not open.");
    }
  };

  const chooseBrowserProject = (file: File) => {
    const startsNewProject = hasAnalysis;
    analysisRequestGenerationRef.current += 1;
    setPendingSelection({ fileName: file.name, browserFile: file });
    setIsAnalyzing(false);
    setIsCancellingAnalysis(false);
    setHasAnalysis(false);
    setAnalysisSource(null);
    setSelectedPlateId("");
    setIsInspectorOpen(false);
    setIsSetupRecommendationOpen(false);
    setActiveSourcePath(null);
    setA1MiniEnabled(planningIntent.a1MiniEnabled);
    setAppliedA1MiniEnabled(false);
    setAppliedPlanningIntent(null);
    setAppliedPrinterLoadout(null);
    if (startsNewProject) setIsProjectSetupConfirmed(false);
    setCustomDirectPalettesEnabled(false);
    setAppliedCustomDirectPalettesEnabled(false);
    explicitDirectMappingOverridesRef.current.clear();
    setColorNeedsRecalculation(false);
    setAnalysisError("");
    setCustomPaletteFailure(null);
    setIsPlanDirty(false);
    setAuthoritativePlanRevision((revision) => revision + 1);
    setConversionCapability(null);
    setPartialConversionAcknowledgementKey(null);
    setPublishedPrintRunBundle(null);
    setPrintRunRecoveryStatus("idle");
    setPrintRunRecoveryError("");
    printRunRecoveryAttemptRef.current = "";
    setLiveMessage(
      `${file.name} selected. Choose Analyze Project to continue.`,
    );
  };

  const runAnalysis = async () => {
    if (!pendingSelection) return;
    if (!isProjectSetupConfirmed) {
      setLiveMessage(
        "Confirm the visible Project setup before analyzing this 3MF.",
      );
      window.requestAnimationFrame(() => {
        document
          .getElementById("project-setup-heading")
          ?.scrollIntoView({ block: "center" });
      });
      return;
    }
    const selection = pendingSelection;
    const requestGeneration = analysisRequestGenerationRef.current + 1;
    analysisRequestGenerationRef.current = requestGeneration;
    setIsAnalyzing(true);
    setIsCancellingAnalysis(false);
    setAnalysisError("");
    setCustomPaletteFailure(null);
    try {
      await libraryLoadQueue.current;
      await librarySaveQueue.current;
      await calibrationMutationQueue.current;
      if (analysisRequestGenerationRef.current !== requestGeneration) return;
      if (librarySaveFailure.current) {
        throw new Error(
          `Save the filament library before analysis. ${librarySaveFailure.current}`,
        );
      }
      const result = await analyzeProject(
        selection,
        planningIntent,
        printerLoadout,
      );
      if (analysisRequestGenerationRef.current !== requestGeneration) return;
      const analyzedPlan =
        result.source === "browser-demo"
          ? applyBrowserInventoryGuard(result.plan, librarySpoolsRef.current)
          : result.plan;
      setPlan(analyzedPlan);
      setHasAnalysis(true);
      setAnalysisSource(result.source);
      setSelectedPlateId(analyzedPlan.plates[0]?.id ?? "");
      setIsInspectorOpen(false);
      setIsSetupRecommendationOpen(false);
      setRestoreCmy(analyzedPlan.restoreCmyByDefault);
      setActiveSourcePath(selection.sourcePath ?? null);
      const appliedA1MiniEnabled = planningIntent.a1MiniEnabled;
      setA1MiniEnabled(appliedA1MiniEnabled);
      setAppliedA1MiniEnabled(appliedA1MiniEnabled);
      setAppliedPlanningIntent({
        ...planningIntent,
        a1MiniEnabled: appliedA1MiniEnabled,
      });
      setAppliedPrinterLoadout(printerLoadout);
      setCustomDirectPalettesEnabled(analyzedPlan.customDirectPalettesEnabled);
      setAppliedCustomDirectPalettesEnabled(
        analyzedPlan.customDirectPalettesEnabled,
      );
      explicitDirectMappingOverridesRef.current.clear();
      setColorNeedsRecalculation(false);
      setIsPlanDirty(false);
      setAuthoritativePlanRevision((revision) => revision + 1);
      setConversionCapability(null);
      setPartialConversionAcknowledgementKey(null);
      setPendingSelection(null);
      setLiveMessage(
        `Analysis complete for ${analyzedPlan.summary.fileName}. ${analyzedPlan.plates.length} target plates planned${
          result.source === "browser-demo" ? " using browser demo data" : ""
        }.`,
      );
    } catch (error) {
      if (analysisRequestGenerationRef.current !== requestGeneration) return;
      setHasAnalysis(false);
      setAnalysisSource(null);
      const message = error instanceof Error ? error.message : String(error);
      setAnalysisError(`Analysis could not be completed. ${message}`);
      setLiveMessage(
        "Analysis could not be completed. Review the visible error and try again.",
      );
    } finally {
      if (analysisRequestGenerationRef.current === requestGeneration) {
        setIsAnalyzing(false);
      }
    }
  };

  const cancelAnalysis = async () => {
    if (!isAnalyzing || isCancellingAnalysis) return;
    analysisRequestGenerationRef.current += 1;
    setIsCancellingAnalysis(true);
    setAnalysisError("");
    setLiveMessage("Canceling analysis…");
    if (isNative) {
      try {
        await cancelNativeAnalysis();
      } catch {
        // The request generation above remains authoritative even if the
        // backend process is already exiting or the IPC channel closes.
      }
    }
    setIsAnalyzing(false);
    setIsCancellingAnalysis(false);
    setLiveMessage(
      "Analysis canceled. The selected 3MF is ready to analyze again.",
    );
  };

  const changeStrategy = (
    strategy: Extract<PrintStrategy, "cmyx" | "direct">,
  ) => {
    if (!selectedPlate || selectedPlate.printer !== "U1") return;
    if (strategy === "direct" && !isDirectStrategyAvailable(selectedPlate)) {
      return;
    }

    const selectedScopeIds = new Set(plateScopeIds(selectedPlate));
    setPlan((current) => ({
      ...current,
      plates: current.plates.map((plate) =>
        plate.printer === "U1" &&
        plateScopeIds(plate).some((scopeId) => selectedScopeIds.has(scopeId))
          ? { ...plate, strategy }
          : plate,
      ),
    }));
    setIsPlanDirty(true);
    setLiveMessage(
      `${selectedPlate.title} is set to ${
        strategy === "direct" ? "Direct Spools" : "CMY+X Full Spectrum"
      }. Changes are pending validation.`,
    );
  };

  const changeBulkStrategies = (
    scopeIds: string[],
    strategy: Extract<PrintStrategy, "cmyx" | "direct">,
  ) => {
    const selectedScopeIds = new Set(scopeIds);
    if (selectedScopeIds.size === 0) return;
    const selectedPlates = plan.plates.filter(
      (plate) =>
        plate.printer === "U1" &&
        plateScopeIds(plate).some((scopeId) => selectedScopeIds.has(scopeId)),
    );
    if (selectedPlates.length === 0) return;
    if (
      strategy === "direct" &&
      selectedPlates.some((plate) => !isDirectStrategyAvailable(plate))
    ) {
      setLiveMessage(
        "Direct Spools was not applied because at least one selected source scope is unavailable. Open that plate and choose Create 4-spool palette, or select only compatible plates.",
      );
      return;
    }

    setPlan((current) => ({
      ...current,
      plates: current.plates.map((plate) =>
        plate.printer === "U1" &&
        plateScopeIds(plate).some((scopeId) => selectedScopeIds.has(scopeId))
          ? { ...plate, strategy }
          : plate,
      ),
    }));
    setIsPlanDirty(true);
    setLiveMessage(
      `${selectedScopeIds.size} source scope${selectedScopeIds.size === 1 ? "" : "s"} (${selectedPlates.length} U1 plate${selectedPlates.length === 1 ? "" : "s"}) ${selectedPlates.length === 1 ? "is" : "are"} set to ${
        strategy === "direct" ? "Direct Spools" : "CMY+X Full Spectrum"
      }. Changes are pending validation.`,
    );
  };

  const changeToolhead = (mappingId: string, nextToolhead: ToolheadId) => {
    if (!selectedPlate) return;
    const selectedActive = (selectedPlate.mappings ?? []).find(
      (mapping) => mapping.id === mappingId,
    );
    if (!selectedActive) return;
    setPlan((current) =>
      updateSelectedMappings(current, selectedActive.scopeId, (mappings) => {
        const active = mappings.find((mapping) => mapping.id === mappingId);
        if (!active) return mappings;
        const activeGroupIds = sharedToolheadMappingIds(mappings, active);
        if (
          mappings
            .filter((mapping) => activeGroupIds.has(mapping.id))
            .every((mapping) => mapping.directToolhead === nextToolhead)
        ) {
          return mappings;
        }
        if (!active.selectedSpoolId) {
          return mappings.map((mapping) =>
            activeGroupIds.has(mapping.id)
              ? { ...mapping, directToolhead: nextToolhead }
              : mapping,
          );
        }
        const displaced = mappings.find(
          (mapping) =>
            Boolean(mapping.selectedSpoolId) &&
            mapping.directToolhead === nextToolhead &&
            !activeGroupIds.has(mapping.id),
        );
        const displacedGroupIds = displaced
          ? sharedToolheadMappingIds(mappings, displaced)
          : new Set<string>();
        return mappings.map((mapping) => {
          if (activeGroupIds.has(mapping.id)) {
            return { ...mapping, directToolhead: nextToolhead };
          }
          if (displacedGroupIds.has(mapping.id)) {
            return { ...mapping, directToolhead: active.directToolhead };
          }
          return mapping;
        });
      }),
    );
    setIsPlanDirty(true);
    setLiveMessage(
      `Direct spool toolheads updated. ${nextToolhead} is now assigned.`,
    );
  };

  const changeSpool = (mappingId: string, nextSpoolId: string) => {
    if (!selectedPlate) return;
    const nextSpool = plan.spools.find((spool) => spool.id === nextSpoolId);
    const selectedActive = (selectedPlate.mappings ?? []).find(
      (mapping) => mapping.id === mappingId,
    );
    if (!selectedActive) return;
    const preflight = applySpoolChoiceToMappings(
      selectedPlate.mappings ?? [],
      mappingId,
      nextSpoolId,
    );
    if (preflight.error) {
      setLiveMessage(preflight.error);
      return;
    }
    explicitDirectMappingOverridesRef.current.add(
      directMappingOverrideKey(
        selectedActive.scopeId,
        selectedActive.inheritanceKey,
      ),
    );
    const propagatedScopes = new Set<string>();
    const skippedScopes = new Set<string>();
    const updatesByPlate = new Map<string, DirectColorMapping[]>();
    updatesByPlate.set(selectedPlate.id, preflight.mappings);
    for (const plate of plan.plates) {
      if (
        plate.printer !== "U1" ||
        plate.id === selectedPlate.id ||
        !plate.mappings
      ) {
        continue;
      }
      const sameScope = plateScopeIds(plate).includes(selectedActive.scopeId);
      if (!sameScope && !customDirectPalettesEnabled) continue;
      const inherited = plate.mappings.find(
        (mapping) => mapping.inheritanceKey === selectedActive.inheritanceKey,
      );
      if (
        !inherited ||
        (!sameScope &&
          explicitDirectMappingOverridesRef.current.has(
            directMappingOverrideKey(
              inherited.scopeId,
              inherited.inheritanceKey,
            ),
          ))
      ) {
        continue;
      }
      const inheritedUpdate = applySpoolChoiceToMappings(
        plate.mappings,
        inherited.id,
        nextSpoolId,
      );
      if (inheritedUpdate.error) {
        skippedScopes.add(inherited.scopeId);
        continue;
      }
      updatesByPlate.set(plate.id, inheritedUpdate.mappings);
      if (!sameScope && inheritedUpdate.changed) {
        propagatedScopes.add(inherited.scopeId);
      }
    }
    const mergedPhysicalIdentityCount = nextSpoolId
      ? new Set(
          preflight.mappings
            .filter((mapping) => mapping.selectedSpoolId === nextSpoolId)
            .map((mapping) => mapping.physicalIdentityId),
        ).size
      : 0;
    setPlan({
      ...plan,
      plates: plan.plates.map((plate) => {
        const mappings = updatesByPlate.get(plate.id);
        return mappings ? { ...plate, mappings } : plate;
      }),
    });
    setIsPlanDirty(true);
    const propagationSummary =
      propagatedScopes.size > 0
        ? ` The same source identity was also updated on ${propagatedScopes.size} unedited ${
            propagatedScopes.size === 1 ? "plate scope" : "plate scopes"
          }.`
        : "";
    const skippedSummary =
      skippedScopes.size > 0
        ? ` ${skippedScopes.size} inherited ${
            skippedScopes.size === 1 ? "scope was" : "scopes were"
          } left unchanged because no free toolhead was available.`
        : "";
    setLiveMessage(
      nextSpool
        ? mergedPhysicalIdentityCount > 1
          ? `${mergedPhysicalIdentityCount} physical Direct identities will use ${nextSpool.colorName} on one spool. Recalculate the plan to validate the reduced palette and A1 Mono eligibility.${propagationSummary}${skippedSummary}`
          : `${nextSpool.colorName} assigned. Color difference and setup actions are pending validation.${propagationSummary}${skippedSummary}`
        : `Spool assignment cleared for the linked physical identity. Direct mapping requires review.${propagationSummary}${skippedSummary}`,
    );
  };

  const changeDirectMaterialSubstitution = (
    mappingId: string,
    acknowledged: boolean,
  ) => {
    if (!selectedPlate) return;
    const active = (selectedPlate.mappings ?? []).find(
      (mapping) => mapping.id === mappingId,
    );
    if (!active) return;
    explicitDirectMappingOverridesRef.current.add(
      directMappingOverrideKey(active.scopeId, active.inheritanceKey),
    );
    setPlan((current) =>
      updateSelectedMappings(current, active.scopeId, (mappings) =>
        mappings.map((mapping) =>
          mapping.id === mappingId
            ? { ...mapping, materialSubstitutionAcknowledged: acknowledged }
            : mapping,
        ),
      ),
    );
    setIsPlanDirty(true);
    setLiveMessage(
      acknowledged
        ? "Material substitution acknowledged. Recalculate the plan to validate it."
        : "Material substitution acknowledgement cleared.",
    );
  };

  const changePrinterPreference = (preference: PrinterPreference) => {
    if (!selectedPlate || selectedPlate.sourceUnitIds.length === 0) return;
    const selectedSourceUnits = new Set(selectedPlate.sourceUnitIds);
    setPlan((current) => {
      const existingSourceUnits = new Set(
        current.unitPrinterSelections.map(
          (selection) => selection.sourceUnitId,
        ),
      );
      return {
        ...current,
        unitPrinterSelections: [
          ...current.unitPrinterSelections.map((selection) =>
            selectedSourceUnits.has(selection.sourceUnitId)
              ? { ...selection, preference }
              : selection,
          ),
          ...selectedPlate.sourceUnitIds
            .filter((sourceUnitId) => !existingSourceUnits.has(sourceUnitId))
            .map((sourceUnitId) => ({ sourceUnitId, preference })),
        ],
      };
    });
    setIsPlanDirty(true);
    setLiveMessage(
      `${selectedPlate.title} is set to ${
        preference === "auto"
          ? "automatic printer selection"
          : preference === "u1"
            ? "Snapmaker U1"
            : "Bambu Lab A1 mini"
      }. Recalculate the plan to update printer queues.`,
    );
  };

  const changeCustomDirectPalettes = (enabled: boolean) => {
    setCustomDirectPalettesEnabled(enabled);
    if (!enabled) {
      setPlan((current) => ({
        ...current,
        plates: current.plates.map((plate) =>
          plate.printer === "U1" &&
          plate.strategy === "direct" &&
          (plate.directPairCount ?? plate.mappings?.length ?? 0) > 4
            ? { ...plate, strategy: "cmyx" as const }
            : plate,
        ),
      }));
    }
    const needsRecalculation = enabled !== appliedCustomDirectPalettesEnabled;
    setLiveMessage(
      needsRecalculation
        ? enabled
          ? "Custom four-spool palettes selected. Recalculate the plan to generate a proposed mapping for every U1 plate."
          : "Custom four-spool palettes cleared. Recalculate the plan to restore exact Direct eligibility limits."
        : "Custom Direct palette mode restored to the applied setting. No recalculation is required.",
    );
  };

  const applyWholeProjectDirectPalette = (
    choices: ProjectDirectPaletteChoice[],
  ) => {
    try {
      const updatedPlan = applyProjectDirectPalette(plan, choices);
      const physicalSpoolCount = new Set(
        choices.map((choice) => choice.spoolId),
      ).size;
      setPlan(updatedPlan);
      setIsPlanDirty(true);
      setLiveMessage(
        `Project-wide Direct Spools assigned ${updatedPlan.projectDirectPalette.effectivePairCount} semantic pairs across ${updatedPlan.projectDirectPalette.directPairCount} physical identities to ${physicalSpoolCount} physical ${
          physicalSpoolCount === 1 ? "spool" : "spools"
        }. Recalculate the plan to validate every source scope.`,
      );
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setLiveMessage(
        `Project-wide Direct Spools could not be applied. ${message}`,
      );
    }
  };

  const addSpool = (input: NewPhysicalSpoolInput) => {
    const id = createUniqueSpoolId(
      input.name || input.colorName,
      librarySpools.map((spool) => spool.id),
    );
    const nextSpools: PhysicalSpool[] = [
      ...librarySpools,
      {
        ...input,
        id,
        calibrationIdentity: createCalibrationIdentity(),
        source: "user",
        name: input.name.trim(),
        colorName: input.colorName.trim(),
        hex: input.hex.toUpperCase(),
        colorBasis: "Nominal",
        available: true,
      },
    ];
    applyLibraryChange(
      nextSpools,
      `${input.colorName.trim()} added to the filament library.${
        hasAnalysis ? " Recalculate the plan to use it." : ""
      }`,
    );
  };

  const updateLibrarySpool = (
    spoolId: string,
    input: NewPhysicalSpoolInput,
  ) => {
    const existing = librarySpools.find((spool) => spool.id === spoolId);
    if (!existing || existing.source !== "user") return;
    const normalizedIdentityText = (value?: string) => value?.trim() || "";
    const calibrationBatchChanged =
      normalizedIdentityText(existing.batchLot) !==
        normalizedIdentityText(input.batchLot) ||
      normalizedIdentityText(existing.calibrationReference) !==
        normalizedIdentityText(input.calibrationReference);
    const nextSpools = librarySpools.map((spool) =>
      spool.id === spoolId
        ? {
            ...spool,
            ...input,
            calibrationIdentity: calibrationBatchChanged
              ? createCalibrationIdentity()
              : (spool.calibrationIdentity ?? spool.id),
            name: input.name.trim(),
            colorName: input.colorName.trim(),
            hex: input.hex.toUpperCase(),
          }
        : spool,
    );
    applyLibraryChange(
      nextSpools,
      `${input.name.trim()} updated in the filament library.${
        hasAnalysis ? " Recalculate the plan to apply the change." : ""
      }`,
    );
  };

  const changeLibraryAvailability = (spoolId: string, available: boolean) => {
    const spool = librarySpools.find((candidate) => candidate.id === spoolId);
    if (!spool || spool.available === available) return;
    const nextSpools = librarySpools.map((candidate) =>
      candidate.id === spoolId ? { ...candidate, available } : candidate,
    );
    applyLibraryChange(
      nextSpools,
      `${spool.name} marked ${available ? "in stock" : "out of stock"}.${
        hasAnalysis ? " Recalculate the plan before printing." : ""
      }`,
    );
  };

  const deleteLibrarySpool = async (spoolId: string) => {
    const spool = librarySpools.find((candidate) => candidate.id === spoolId);
    if (!spool || spool.source !== "user") return false;
    if (isLibraryLoading || libraryErrorKind === "load") {
      setLiveMessage(
        "Wait for the filament library to load before editing it.",
      );
      return false;
    }

    const deleted = await queueLibrarySave(
      librarySpools.filter((candidate) => candidate.id !== spoolId),
      `${spool.name} deleted from the filament library.${
        hasAnalysis ? " Recalculate the plan before printing." : ""
      }`,
    );
    if (deleted && hasAnalysis) {
      setIsPlanDirty(true);
      setColorNeedsRecalculation(true);
    }
    return deleted;
  };

  const updateColorResolution = (
    selected: ColorResolution,
    update: (resolution: ColorResolution) => ColorResolution,
    forceCmyx = false,
  ) => {
    setPlan((current) => ({
      ...current,
      colorResolutions: current.colorResolutions.map((resolution) =>
        resolution.scopeId === selected.scopeId &&
        resolution.requirementId === selected.requirementId &&
        resolution.candidateId === selected.candidateId
          ? update(resolution)
          : resolution,
      ),
      plates: forceCmyx
        ? current.plates.map((plate) =>
            plate.scopeId === selected.scopeId && plate.printer === "U1"
              ? { ...plate, strategy: "cmyx" as const }
              : plate,
          )
        : current.plates,
    }));
    setIsPlanDirty(true);
    setColorNeedsRecalculation(true);
  };

  const acceptColorApproximation = (resolution: ColorResolution) => {
    updateColorResolution(
      resolution,
      (current) => ({
        ...current,
        colorApproved: true,
      }),
      true,
    );
    setLiveMessage(
      `Color approximation accepted for ${resolution.scopeName}. Recalculate the plan to apply it.`,
    );
  };

  const selectCmyxPaletteColor = (
    selected: ColorResolution,
    selectedOption: CmyxPaletteOption,
  ) => {
    const selectedIdentity =
      selected.sourceIdentityKey ??
      JSON.stringify([
        selected.scopeId,
        selected.requirementId,
        selected.sourceMaterial,
        selected.sourceHex,
      ]);
    const replacements = new Map<string, CmyxPaletteOption>();
    const affectedScopeIds = new Set<string>();
    for (const resolution of plan.colorResolutions) {
      const resolutionIdentity =
        resolution.sourceIdentityKey ??
        JSON.stringify([
          resolution.scopeId,
          resolution.requirementId,
          resolution.sourceMaterial,
          resolution.sourceHex,
        ]);
      if (resolutionIdentity !== selectedIdentity) continue;
      const option =
        resolution.scopeId === selected.scopeId &&
        resolution.requirementId === selected.requirementId
          ? selectedOption
          : resolution.paletteOptions?.find(
              (candidate) =>
                candidate.recipe === selectedOption.recipe &&
                candidate.predictedHex === selectedOption.predictedHex &&
                candidate.targetMaterial === selectedOption.targetMaterial &&
                candidate.requiredT4SpoolId === selectedOption.requiredT4SpoolId,
            );
      if (!option || option.candidateId === resolution.candidateId) continue;
      replacements.set(
        JSON.stringify([resolution.scopeId, resolution.requirementId]),
        option,
      );
      affectedScopeIds.add(resolution.scopeId);
    }
    const changedCount = replacements.size;
    if (changedCount === 0) return;
    setPlan((current) => {
      const colorResolutions = current.colorResolutions.map((resolution) => {
        const option = replacements.get(
          JSON.stringify([resolution.scopeId, resolution.requirementId]),
        );
        if (!option) return resolution;
        const requiresMaterialSubstitution =
          resolution.sourceMaterial !== option.targetMaterial;
        return {
          ...resolution,
          candidateId: option.candidateId,
          targetMaterial: option.targetMaterial,
          targetHex: option.targetHex,
          predictedHex: option.predictedHex,
          recipe: option.recipe,
          deltaE00: option.deltaE00,
          confidence: option.confidence,
          requiredT4SpoolId: option.requiredT4SpoolId,
          requiredT4Name: option.requiredT4Name,
          requiredT4Hex: option.requiredT4Hex,
          // Choosing a palette swatch is already an explicit decision. Keep a
          // separate approval step only when the choice also changes material.
          colorApproved: !requiresMaterialSubstitution,
          materialApproved: false,
          requiresMaterialSubstitution,
        };
      });
      return {
        ...current,
        colorResolutions,
        plates: current.plates.map((plate) =>
          plate.printer === "U1" &&
          (plate.scopeIds?.length ? plate.scopeIds : [plate.scopeId]).some(
            (scopeId) => affectedScopeIds.has(scopeId),
          )
            ? { ...plate, strategy: "cmyx" as const }
            : plate,
        ),
      };
    });
    setIsPlanDirty(true);
    setColorNeedsRecalculation(true);
    const requiresMaterialSubstitution =
      selected.sourceMaterial !== selectedOption.targetMaterial;
    setLiveMessage(
      requiresMaterialSubstitution
        ? `${selectedOption.predictedHex ?? selectedOption.targetHex} selected for ${changedCount} linked Full Spectrum ${changedCount === 1 ? "mapping" : "mappings"}. Accept the material change, then recalculate the plan.`
        : `${selectedOption.predictedHex ?? selectedOption.targetHex} selected and accepted for ${changedCount} linked Full Spectrum ${changedCount === 1 ? "mapping" : "mappings"}. Recalculate the plan to apply this exact CMY recipe.`,
    );
  };

  const acceptMaterialSubstitution = (resolution: ColorResolution) => {
    updateColorResolution(
      resolution,
      (current) => ({
        ...current,
        colorApproved: true,
        materialApproved: true,
      }),
      true,
    );
    setLiveMessage(
      `${resolution.sourceMaterial} to ${resolution.targetMaterial} substitution accepted for ${resolution.scopeName}. Recalculate the plan to apply it.`,
    );
  };

  const acceptAllSameMaterialApproximations = () => {
    const acceptedScopeIds = new Set(
      plan.colorResolutions
        .filter(
          (resolution) =>
            !resolution.requiresMaterialSubstitution &&
            !resolution.colorApproved,
        )
        .map((resolution) => resolution.scopeId),
    );
    const count = plan.colorResolutions.filter(
      (resolution) =>
        !resolution.requiresMaterialSubstitution && !resolution.colorApproved,
    ).length;
    if (count === 0) return;
    setPlan((current) => ({
      ...current,
      colorResolutions: current.colorResolutions.map((resolution) =>
        !resolution.requiresMaterialSubstitution
          ? { ...resolution, colorApproved: true }
          : resolution,
      ),
      plates: current.plates.map((plate) =>
        acceptedScopeIds.has(plate.scopeId) && plate.printer === "U1"
          ? { ...plate, strategy: "cmyx" as const }
          : plate,
      ),
    }));
    setIsPlanDirty(true);
    setColorNeedsRecalculation(true);
    setLiveMessage(
      `${count} same-material color ${count === 1 ? "approximation" : "approximations"} accepted. Material substitutions still require individual approval.`,
    );
  };

  const directUnavailableReasonForResolution = (
    resolution: ColorResolution,
  ) => {
    const scopePlate = plan.plates.find(
      (plate) => plate.scopeId === resolution.scopeId,
    );
    if (!scopePlate) {
      return "Direct Spools availability cannot be determined for this source scope.";
    }
    if (scopePlate.directEligible) return null;
    return (
      scopePlate.directEligibilityReason ??
      "This source scope cannot use Direct Spools with the current structure."
    );
  };

  const chooseExistingSpoolForResolution = (
    resolution: ColorResolution,
    spoolId: string,
    materialSubstitutionAcknowledged: boolean,
  ) => {
    const spool = plan.spools.find(
      (candidate) => candidate.id === spoolId && candidate.available,
    );
    if (!spool) {
      setLiveMessage(
        "The selected spool is no longer in stock. Choose another spool.",
      );
      return;
    }
    const unavailableReason = directUnavailableReasonForResolution(resolution);
    if (unavailableReason) {
      setLiveMessage(unavailableReason);
      return;
    }
    const changesMaterial = spool.material !== resolution.sourceMaterial;
    if (changesMaterial && !materialSubstitutionAcknowledged) {
      setLiveMessage(
        "Confirm the mechanical-property risk before forcing a different material.",
      );
      return;
    }

    const assignment = {
      requirementId: resolution.requirementId,
      spoolId: spool.id,
      toolhead: null,
      allowMaterialSubstitution: changesMaterial,
    };
    setPlan((current) => {
      const existingSelection = current.scopeSelections.find(
        (selection) => selection.scopeId === resolution.scopeId,
      );
      const nextSelection = {
        scopeId: resolution.scopeId,
        strategy: "direct" as const,
        assignments: [
          ...(existingSelection?.assignments ?? []).filter(
            (existing) => existing.requirementId !== resolution.requirementId,
          ),
          assignment,
        ],
        approvedColorFallbacks: (
          existingSelection?.approvedColorFallbacks ?? []
        ).filter(
          (approval) => approval.requirementId !== resolution.requirementId,
        ),
        materialSubstitutions: (
          existingSelection?.materialSubstitutions ?? []
        ).filter(
          (substitution) =>
            substitution.requirementId !== resolution.requirementId,
        ),
      };

      return {
        ...current,
        scopeSelections: existingSelection
          ? current.scopeSelections.map((selection) =>
              selection.scopeId === resolution.scopeId
                ? nextSelection
                : selection,
            )
          : [...current.scopeSelections, nextSelection],
        plates: current.plates.map((plate) =>
          plate.scopeId === resolution.scopeId
            ? {
                ...plate,
                strategy: "direct" as const,
                mappings: plate.mappings?.map((mapping) =>
                  mapping.id === resolution.requirementId
                    ? {
                        ...mapping,
                        selectedSpoolId: spool.id,
                        materialSubstitutionAcknowledged: changesMaterial,
                      }
                    : mapping,
                ),
              }
            : plate,
        ),
      };
    });
    setIsPlanDirty(true);
    setColorNeedsRecalculation(true);
    setLiveMessage(
      `${spool.colorName} will replace ${resolution.sourceHex} using Direct Spools. Recalculate the plan to rebuild loadouts and spool changes.`,
    );
  };

  const changeAlternativePlate = (plateId: number, included: boolean) => {
    const alternative = plan.alternativePlates.find(
      (plate) => plate.id === plateId,
    );
    if (!alternative || alternative.included === included) return;

    setPlan((current) => ({
      ...current,
      alternativePlates: current.alternativePlates.map((plate) =>
        plate.id === plateId ? { ...plate, included } : plate,
      ),
    }));
    setIsPlanDirty(true);
    setLiveMessage(
      `${alternative.name} will be ${included ? "included" : "excluded"}. Validate choices to rebuild the plan.`,
    );
  };

  const exportPlan = async () => {
    if (!hasAnalysis || hasPendingPlanChanges) return;
    const fileName = `${plan.summary.fileName.replace(/\.3mf$/i, "")}-print-plan.json`;
    if (!isNative) {
      if (typeof URL.createObjectURL === "function") {
        const browserExport = {
          ...plan,
          provenance: "browser-demo" as const,
          validated: false,
        };
        const blob = new Blob([JSON.stringify(browserExport, null, 2)], {
          type: "application/json",
        });
        const url = URL.createObjectURL(blob);
        const link = document.createElement("a");
        link.href = url;
        link.download = fileName;
        link.click();
        URL.revokeObjectURL(url);
        setLiveMessage(`Print plan exported as ${fileName}.`);
      } else {
        setLiveMessage("This browser cannot export the print plan.");
      }
      return;
    }

    try {
      if (!activeSourcePath) {
        setLiveMessage("Analyze a native project before exporting its plan.");
        return;
      }
      const destinationPath = await exportNativePlan(
        plan,
        fileName,
        activeSourcePath,
      );
      setLiveMessage(
        destinationPath
          ? `Print plan exported as ${fileName}.`
          : "Print plan export canceled.",
      );
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setLiveMessage(`Print plan export failed. ${message}`);
    }
  };

  const validateChoices = async (
    planningIntentOverride?: PlanningIntent,
    customDirectPaletteScopeId?: string,
    printerLoadoutOverride?: PrinterLoadoutProfile,
  ) => {
    if (!hasAnalysis) return;
    if (isNative && !activeSourcePath) {
      setLiveMessage("Analyze a native project before validating choices.");
      return;
    }

    const requestedPlanningIntent = planningIntentOverride ?? planningIntent;
    const requestedPrinterLoadout = printerLoadoutOverride ?? printerLoadout;
    const requestedA1MiniEnabled = requestedPlanningIntent.a1MiniEnabled;
    if (customDirectPaletteScopeId) setCustomPaletteFailure(null);
    if (planningIntentOverride) {
      if (printingSetup === null && requestedA1MiniEnabled) {
        const inferredSetup: PrintingSetup = {
          schemaVersion: 1,
          primaryPrinter: "u1",
          secondaryPrinter: "a1-mini",
        };
        const persisted = savePrintingSetup(appLocalStorage(), inferredSetup);
        setPrintingSetup(inferredSetup);
        setPrintingSetupPersistence(persisted ? "persisted" : "session");
      }
      setPlanningIntent(requestedPlanningIntent);
      setA1MiniEnabled(requestedA1MiniEnabled);
      savePlanningIntent(appLocalStorage(), requestedPlanningIntent);
    }
    if (printerLoadoutOverride) {
      setPrinterLoadout(requestedPrinterLoadout);
      savePrinterLoadout(appLocalStorage(), requestedPrinterLoadout);
    }
    setIsValidating(true);
    setConversionCapability(null);
    setPartialConversionAcknowledgementKey(null);
    const selectedScopeId =
      customDirectPaletteScopeId ?? selectedPlate?.scopeId;
    const selectedSourceUnitIds = new Set(selectedPlate?.sourceUnitIds ?? []);
    const requestedCustomDirectPalettesEnabled = customDirectPaletteScopeId
      ? true
      : customDirectPalettesEnabled;

    try {
      await libraryLoadQueue.current;
      await librarySaveQueue.current;
      await calibrationMutationQueue.current;
      if (librarySaveFailure.current) {
        throw new Error(
          `Save the filament library before replanning. ${librarySaveFailure.current}`,
        );
      }
      const request = buildReplanRequest(
        plan,
        restoreCmy,
        requestedA1MiniEnabled,
        requestedCustomDirectPalettesEnabled,
        requestedPlanningIntent.defaultStrategy,
        requestedPlanningIntent.allowU1CrossSourceRepacking,
        requestedPlanningIntent.dedicatedSupportSpoolId,
        requestedPlanningIntent.dedicatedSupportUsage,
      );
      request.currentLoadout = requestedPrinterLoadout.currentLoadout.map(
        (entry) => ({ ...entry }),
      );
      request.currentA1SpoolId = requestedPrinterLoadout.currentA1SpoolId;
      if (planningIntentOverride) {
        if (requestedPlanningIntent.defaultStrategy === "auto") {
          request.scopeOverrides = [];
        } else {
          const requestedStrategy = requestedPlanningIntent.defaultStrategy;
          const overridesByScope = new Map(
            request.scopeOverrides.map((override) => [
              override.scopeId,
              { ...override, strategy: requestedStrategy },
            ]),
          );
          for (const plate of plan.plates) {
            if (plate.printer !== "U1") continue;
            for (const scopeId of plateScopeIds(plate)) {
              if (!overridesByScope.has(scopeId)) {
                overridesByScope.set(scopeId, {
                  scopeId,
                  strategy: requestedStrategy,
                  assignments: [],
                  approvedColorFallbacks: [],
                  materialSubstitutions: [],
                });
              }
            }
          }
          request.scopeOverrides = [...overridesByScope.values()];
        }
      }
      if (customDirectPaletteScopeId) {
        request.scopeOverrides = [
          ...request.scopeOverrides.filter(
            (override) => override.scopeId !== customDirectPaletteScopeId,
          ),
          {
            scopeId: customDirectPaletteScopeId,
            strategy: "direct",
            assignments: [],
            approvedColorFallbacks: [],
            materialSubstitutions: [],
          },
        ];
      }
      if (isNative && activeSourcePath) {
        const replanned = await replanProject(activeSourcePath, request);
        setPlan(replanned);
        setIsPlanDirty(false);
        setAppliedA1MiniEnabled(requestedA1MiniEnabled);
        setAppliedPlanningIntent(requestedPlanningIntent);
        setAppliedPrinterLoadout(requestedPrinterLoadout);
        setCustomDirectPalettesEnabled(replanned.customDirectPalettesEnabled);
        setAppliedCustomDirectPalettesEnabled(
          replanned.customDirectPalettesEnabled,
        );
        setColorNeedsRecalculation(false);
        setIsSetupRecommendationOpen(false);
        setAuthoritativePlanRevision((revision) => revision + 1);
        const nextSelectedPlate =
          replanned.plates.find((plate) =>
            plate.sourceUnitIds.some((sourceUnitId) =>
              selectedSourceUnitIds.has(sourceUnitId),
            ),
          )?.id ??
          replanned.plates.find((plate) =>
            selectedScopeId
              ? plateScopeIds(plate).includes(selectedScopeId)
              : false,
          )
            ?.id ??
          replanned.plates[0]?.id ??
          "";
        setSelectedPlateId(nextSelectedPlate);
        if (customDirectPaletteScopeId) {
          const palettePlates = replanned.plates.filter(
            (plate) =>
              plate.printer === "U1" &&
              plate.scopeId === customDirectPaletteScopeId,
          );
          const preparedPlate = palettePlates.find(
            (plate) =>
              isDirectStrategyAvailable(plate) &&
              Boolean(plate.mappings?.length),
          );
          if (preparedPlate) {
            // Palette preparation is an intermediate recovery step: the
            // generated assignments still need one explicit authoritative
            // replan before conversion, even when every suggested spool is
            // already the user's intended choice and no select emits change.
            setIsPlanDirty(true);
            const mappingRows = palettePlates.flatMap(
              (plate) => plate.mappings ?? [],
            );
            const sourceIdentityCount = Math.max(
              ...palettePlates.map(
                (plate) =>
                  plate.directPairCount ??
                  plate.effectivePairCount ??
                  plate.logicalColorCount,
              ),
            );
            const physicalSpoolCount = new Set(
              mappingRows
                .map((mapping) => mapping.selectedSpoolId)
                .filter(Boolean),
            ).size;
            setCustomPaletteFailure(null);
            setLiveMessage(
              `Four-spool palette created for ${preparedPlate.title}. ${sourceIdentityCount} source Direct identities are mapped to ${physicalSpoolCount} physical ${physicalSpoolCount === 1 ? "spool" : "spools"}. Review every mapping before conversion.`,
            );
            window.requestAnimationFrame(() => {
              document
                .getElementById(`strategy-${preparedPlate.id}-direct`)
                ?.focus({ preventScroll: true });
            });
          } else {
            const reason =
              palettePlates.find((plate) => plate.directEligibilityReason)
                ?.directEligibilityReason ??
              "No compatible four-spool palette could be created from the available inventory.";
            const message = `${reason} Review compatible spools in the Filament Library, then try again.`;
            setCustomPaletteFailure({
              scopeId: customDirectPaletteScopeId,
              message,
            });
            setLiveMessage(
              `Four-spool palette could not be created. ${message}`,
            );
          }
        } else {
          setLiveMessage(
            `Choices validated. ${replanned.plates.length} target plates replanned.`,
          );
        }
      } else {
        await new Promise((resolve) => window.setTimeout(resolve, 350));
        const reviewCount = plan.plates
          .filter((plate) => plate.strategy === "direct")
          .flatMap((plate) => plate.mappings ?? [])
          .filter((mapping) => {
            const quality = directQuality(mapping, plan.spools);
            return (
              quality === "Review" ||
              quality === "Poor" ||
              quality === "Material mismatch"
            );
          }).length;
        setPlan((current) => {
          const batches = deriveBatches(current.plates);
          const u1CmyBatchCount = batches.filter(
            (batch) => batch.printer === "U1" && batch.strategy === "cmyx",
          ).length;
          return applyBrowserInventoryGuard(
            {
              ...current,
              batches,
              t4SwapCount: Math.max(0, u1CmyBatchCount - 1),
            },
            current.spools,
          );
        });
        setIsPlanDirty(false);
        setAppliedA1MiniEnabled(requestedA1MiniEnabled);
        setAppliedPlanningIntent(requestedPlanningIntent);
        setAppliedPrinterLoadout(requestedPrinterLoadout);
        setAppliedCustomDirectPalettesEnabled(
          requestedCustomDirectPalettesEnabled,
        );
        setPlan((current) => ({
          ...current,
          customDirectPalettesEnabled: requestedCustomDirectPalettesEnabled,
        }));
        setColorNeedsRecalculation(false);
        setIsSetupRecommendationOpen(false);
        setAuthoritativePlanRevision((revision) => revision + 1);
        const directReviewMessage =
          reviewCount > 0
            ? `${reviewCount} Direct ${
                reviewCount === 1 ? "mapping requires" : "mappings require"
              } review.`
            : "All Direct mappings are assigned.";
        setLiveMessage(
          `Choices validated locally using demo data. ${directReviewMessage}${
            requestedA1MiniEnabled
              ? " A1 mini routing requires native backend validation."
              : ""
          }`,
        );
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (customDirectPaletteScopeId) {
        setCustomPaletteFailure({
          scopeId: customDirectPaletteScopeId,
          message: `Palette creation failed. ${message} Try again, or review compatible spools in the Filament Library.`,
        });
      }
      setLiveMessage(`Choice validation failed. ${message}`);
    } finally {
      setIsValidating(false);
    }
  };

  const approveAndPrepareConversion = async () => {
    if (
      !isNative ||
      !activeSourcePath ||
      !conversionScopeAvailable ||
      (partialConversionPlanAvailable && !partialConversionAcknowledged) ||
      !conversionCapability?.available ||
      !conversionCapability.planFingerprint ||
      !experimentalDialectAcknowledged
    ) {
      setLiveMessage(
        partialConversionPlanAvailable && !partialConversionAcknowledged
          ? "Review and acknowledge every excluded source unit before converting valid jobs."
          : !experimentalDialectAcknowledged
            ? "Review and approve the fingerprint-bound Experimental source dialect warning before conversion."
            : (conversionCapability?.reason ??
              "Conversion is not available for the current plan."),
      );
      return;
    }
    const partialConversionApproval: PartialConversionApproval | null =
      partialConversionPlanAvailable
        ? { excludedSourceUnits: plan.partialConversion.exclusions }
        : null;
    setIsPreparingConversion(true);
    setConversionError("");
    setConversionResult(null);
    setConversionNeedsNewPreflight(false);
    activeConversionIdRef.current = null;
    try {
      const destination = await chooseConversionDestination();
      if (!destination) {
        setLiveMessage("Conversion canceled before preflight.");
        return;
      }
      const prepared = await prepareNativeConversion(
        activeSourcePath,
        plan.summary.sourceHash,
        conversionCapability.planFingerprint,
        partialConversionApproval,
        experimentalDialectEvidenceKey
          ? {
              sourceFingerprint:
                conversionCapability.experimentalDialectFingerprint!,
            }
          : null,
      );
      setConversionDestination(destination);
      setPreparedConversion(prepared);
      activeConversionIdRef.current = prepared.conversionId;
      const printers = [
        ...new Set(
          prepared.preparation.artifacts.map((artifact) => artifact.printer),
        ),
      ];
      setLiveMessage(
        `Conversion preflight prepared for ${prepared.preparation.artifacts.length} project ${
          prepared.preparation.artifacts.length === 1 ? "file" : "files"
        }${printers.length > 0 ? `: ${printers.join(", ")}` : ""}.`,
      );
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setConversionError(`Conversion preflight failed. ${message}`);
      setLiveMessage("Conversion preflight failed.");
    } finally {
      setIsPreparingConversion(false);
    }
  };

  const convertPreparedProjects = async (warningsAcknowledged: boolean) => {
    if (!preparedConversion || !conversionDestination) return;
    setIsConverting(true);
    setConversionError("");
    activeConversionIdRef.current = preparedConversion.conversionId;
    setConversionProgress({
      conversionId: preparedConversion.conversionId,
      stage: "starting",
      message: "Starting native project conversion…",
    });
    try {
      const result = await convertNativeProject(
        preparedConversion.preparationToken,
        conversionDestination,
        warningsAcknowledged ? preparedConversion.preparation.warnings : [],
      );
      setConversionResult(result);
      const publishedBundle: PublishedPrintRunBundle = {
        sourceHash: plan.summary.sourceHash,
        backendPlanFingerprint: preparedConversion.preparation.planFingerprint,
        executionPlanFingerprint,
        result,
      };
      setPublishedPrintRunBundle(publishedBundle);
      const receiptSaved = savePublishedPrintRunBundle(
        appLocalStorage(),
        plan,
        publishedBundle,
      );
      setPrintRunRecoveryStatus(receiptSaved ? "recovered" : "idle");
      setPrintRunRecoveryError(
        receiptSaved
          ? ""
          : "Published files are available in this session, but the restart-recovery receipt could not be saved locally.",
      );
      setPreparedConversion(null);
      setConversionNeedsNewPreflight(false);
      activeConversionIdRef.current = null;
      setLiveMessage(
        `${result.artifacts.length} validated project ${
          result.artifacts.length === 1 ? "file was" : "files were"
        } created.${receiptSaved ? " Restart recovery is ready." : " Restart recovery could not be saved."}`,
      );
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setPreparedConversion(null);
      activeConversionIdRef.current = null;
      setConversionNeedsNewPreflight(true);
      setConversionError(`Conversion failed. ${message}`);
      setLiveMessage(
        "Conversion failed. No incomplete bundle was published; open a new preflight before retrying.",
      );
    } finally {
      setIsConverting(false);
    }
  };

  const retryConversionPreflight = () => {
    if (isConverting || isPreparingConversion) return;
    setPreparedConversion(null);
    setConversionResult(null);
    setConversionError("");
    setConversionProgress(null);
    setConversionDestination("");
    setConversionNeedsNewPreflight(false);
    activeConversionIdRef.current = null;
    void approveAndPrepareConversion();
  };

  const cancelPreparedConversion = async () => {
    const conversionId = preparedConversion?.conversionId;
    if (!conversionId || !isConverting) return;
    setConversionProgress({
      conversionId,
      stage: "cancelling",
      message: "Stopping conversion before publication…",
    });
    try {
      const cancellation = await cancelNativeConversion(conversionId);
      if (cancellation.accepted) {
        setLiveMessage("Conversion cancellation requested.");
      } else if (cancellation.state === "published") {
        setLiveMessage(
          "The conversion was already published and was not removed.",
        );
      } else {
        setLiveMessage("The conversion is no longer running.");
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setConversionError(`Could not stop conversion. ${message}`);
    }
  };

  const runConversionOutputAction = async (
    kind: ActiveConversionOutputAction["kind"],
    artifact: PublishedConversionArtifact,
  ) => {
    if (activeConversionOutputAction) return;
    setActiveConversionOutputAction({ kind, path: artifact.path });
    setConversionError("");
    try {
      if (kind === "open") await openConvertedOutput(artifact);
      else await showConvertedOutputInFinder(artifact);
      setLiveMessage(
        kind === "open"
          ? `${artifact.fileName} was opened in ${artifact.slicer}.`
          : `${artifact.fileName} was shown in Finder.`,
      );
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setConversionError(
        kind === "open"
          ? `Could not open ${artifact.fileName} in ${artifact.slicer}. ${message}`
          : `Could not show ${artifact.fileName} in Finder. ${message}`,
      );
    } finally {
      setActiveConversionOutputAction(null);
    }
  };

  const closeConversionDialog = () => {
    if (isConverting) return;
    setPreparedConversion(null);
    setConversionResult(null);
    setConversionError("");
    setConversionProgress(null);
    setConversionDestination("");
    setConversionNeedsNewPreflight(false);
    setActiveConversionOutputAction(null);
    activeConversionIdRef.current = null;
  };

  const validatedChoicesAvailable =
    hasAnalysis &&
    !isAnalyzing &&
    !isValidating &&
    !hasPendingPlanChanges &&
    !isLibraryLoading &&
    !isLibrarySaving &&
    !libraryError;
  const validatedPlanAvailable = validatedChoicesAvailable && plan.planReady;
  const partialConversionEvidenceComplete =
    !plan.planReady &&
    plan.partialConversion.available &&
    plan.partialConversion.exclusions.length > 0 &&
    plan.omittedUnitCount === plan.partialConversion.exclusions.length &&
    plan.blockingErrors.length > 0;
  const partialConversionPlanAvailable =
    validatedChoicesAvailable && partialConversionEvidenceComplete;
  const conversionScopeAvailable =
    validatedPlanAvailable || partialConversionPlanAvailable;

  useEffect(() => {
    if (!isNative || !activeSourcePath || !conversionScopeAvailable) {
      setConversionCapability(null);
      setIsCheckingConversion(false);
      return;
    }
    let canceled = false;
    setIsCheckingConversion(true);
    void inspectConversionCapabilities(activeSourcePath)
      .then((capability) => {
        if (!canceled) setConversionCapability(capability);
      })
      .catch((error) => {
        if (canceled) return;
        const message = error instanceof Error ? error.message : String(error);
        setConversionCapability({
          available: false,
          reason: `Conversion capability check failed. ${message}`,
          planFingerprint: null,
          sourceDialectSupport: "Unsupported",
          experimentalDialectApprovalRequired: false,
          experimentalDialectFingerprint: null,
          adapters: [],
        });
      })
      .finally(() => {
        if (!canceled) setIsCheckingConversion(false);
      });
    return () => {
      canceled = true;
    };
  }, [
    activeSourcePath,
    authoritativePlanRevision,
    isNative,
    executionPlanFingerprint,
    plan.summary.sourceHash,
    conversionScopeAvailable,
  ]);

  useEffect(() => {
    if (
      !isNative ||
      analysisSource !== "tauri" ||
      !activeSourcePath ||
      !conversionScopeAvailable ||
      isCheckingConversion ||
      !conversionCapability?.available ||
      !conversionCapability.planFingerprint ||
      publishedPrintRunBundle ||
      (partialConversionPlanAvailable && !partialConversionAcknowledged) ||
      !experimentalDialectAcknowledged
    ) {
      return;
    }

    const stored = loadUntrustedPublishedPrintRunBundle(
      appLocalStorage(),
      plan,
    );
    if (!stored) return;

    const attemptKey = JSON.stringify([
      activeSourcePath,
      plan.summary.sourceHash,
      executionPlanFingerprint,
      conversionCapability.planFingerprint,
      stored.backendPlanFingerprint,
      stored.result.outputDirectory,
      printRunRecoveryRevision,
    ]);
    if (printRunRecoveryAttemptRef.current === attemptKey) return;
    printRunRecoveryAttemptRef.current = attemptKey;

    if (
      stored.backendPlanFingerprint !== conversionCapability.planFingerprint
    ) {
      setPrintRunRecoveryStatus("idle");
      setPrintRunRecoveryError(
        "A saved Print Run receipt exists for this source and layout, but its backend plan fingerprint is stale. Convert to a different destination, or move/remove the existing untrusted bundle before reusing the same destination; conversion never overwrites an existing output path.",
      );
      return;
    }

    let canceled = false;
    setPrintRunRecoveryStatus("checking");
    setPrintRunRecoveryError("");
    setLiveMessage(
      "A saved Print Run receipt was found. The native backend is revalidating its source, manifest, and project files.",
    );
    void revalidatePublishedConversion(
      activeSourcePath,
      plan.summary.sourceHash,
      conversionCapability.planFingerprint,
      stored.result.outputDirectory,
      experimentalDialectEvidenceKey
        ? {
            sourceFingerprint:
              conversionCapability.experimentalDialectFingerprint!,
          }
        : null,
    )
      .then((result) => {
        if (canceled) return;
        const recovered: PublishedPrintRunBundle = {
          sourceHash: plan.summary.sourceHash,
          backendPlanFingerprint: conversionCapability.planFingerprint!,
          executionPlanFingerprint,
          result,
        };
        if (
          result.outputDirectory !== stored.result.outputDirectory ||
          !isPublishedPrintRunBundleReady(plan, recovered)
        ) {
          throw new Error(
            "The backend-validated files do not cover this exact execution plan.",
          );
        }
        setPublishedPrintRunBundle(recovered);
        setPrintRunRecoveryStatus("recovered");
        setPrintRunRecoveryError("");
        savePublishedPrintRunBundle(appLocalStorage(), plan, recovered);
        setLiveMessage(
          `${result.artifacts.length} published project ${
            result.artifacts.length === 1 ? "file was" : "files were"
          } recovered and revalidated. Print Run is ready.`,
        );
      })
      .catch((error) => {
        if (canceled) return;
        const detail = error instanceof Error ? error.message : String(error);
        setPrintRunRecoveryStatus("idle");
        setPrintRunRecoveryError(
          `Saved Print Run files could not be recovered safely. ${detail}`,
        );
        setLiveMessage(
          "Saved Print Run recovery failed. Conversion is required unless recovery succeeds on retry.",
        );
      });
    return () => {
      canceled = true;
    };
  }, [
    activeSourcePath,
    analysisSource,
    conversionCapability,
    conversionScopeAvailable,
    executionPlanFingerprint,
    experimentalDialectAcknowledged,
    experimentalDialectEvidenceKey,
    isCheckingConversion,
    isNative,
    partialConversionAcknowledged,
    partialConversionPlanAvailable,
    plan,
    printRunRecoveryRevision,
    publishedPrintRunBundle,
  ]);

  const conversionAvailable =
    conversionScopeAvailable &&
    (!partialConversionPlanAvailable || partialConversionAcknowledged) &&
    experimentalDialectAcknowledged &&
    isNative &&
    !isCheckingConversion &&
    conversionCapability?.available === true;
  const conversionBlockReason = !validatedChoicesAvailable
    ? "Validate the current plan first"
    : !conversionScopeAvailable
      ? plan.partialConversion.available && !partialConversionEvidenceComplete
        ? "Partial conversion evidence is incomplete. Recalculate the plan before converting."
        : plan.partialConversion.reason || "Resolve the blocking choices first"
      : !isNative
        ? "Native conversion is available in the desktop app"
        : isCheckingConversion
          ? "Checking the installed writer adapters…"
          : partialConversionPlanAvailable && !partialConversionAcknowledged
            ? "Review and acknowledge every excluded source unit first"
            : !experimentalDialectAcknowledged
              ? "Approve the fingerprint-bound Experimental source dialect warning first"
              : (conversionCapability?.reason ??
                "Conversion is unavailable: no qualified writer adapter is installed.");
  const publishedPrintRunReady =
    (validatedPlanAvailable ||
      (partialConversionPlanAvailable && partialConversionAcknowledged)) &&
    analysisSource === "tauri" &&
    !isCheckingConversion &&
    conversionCapability?.planFingerprint ===
      publishedPrintRunBundle?.backendPlanFingerprint &&
    isPublishedPrintRunBundleReady(plan, publishedPrintRunBundle);
  const printRunAvailable = publishedPrintRunReady;
  const availableSpoolCount = librarySpools.filter(
    (spool) => spool.available,
  ).length;
  const inventoryReady =
    !isLibraryLoading && !libraryError && availableSpoolCount > 0;
  const initialSetupReady = inventoryReady && isProjectSetupConfirmed;
  const a1MiniPlateCount = hasAnalysis
    ? plan.plates.filter((plate) => plate.printer === "A1 mini").length
    : 0;
  const projectSetupIntent = useMemo<PlanningIntent>(
    () => ({
      defaultStrategy: planningIntent.defaultStrategy,
      a1MiniEnabled,
      allowU1CrossSourceRepacking:
        planningIntent.allowU1CrossSourceRepacking,
      dedicatedSupportSpoolId: planningIntent.dedicatedSupportSpoolId,
      dedicatedSupportUsage: planningIntent.dedicatedSupportUsage,
    }),
    [
      a1MiniEnabled,
      planningIntent.allowU1CrossSourceRepacking,
      planningIntent.dedicatedSupportSpoolId,
      planningIntent.dedicatedSupportUsage,
      planningIntent.defaultStrategy,
    ],
  );
  const unresolvedDecisionCount = plan.colorResolutions.length;
  const planNeedsAttention =
    hasPendingPlanChanges ||
    unresolvedDecisionCount > 0 ||
    plan.blockingErrors.length > 0 ||
    plan.omittedUnitCount > 0;
  const planStateLabel = isAnalyzing
    ? "Analyzing…"
    : !hasAnalysis
      ? !inventoryReady
        ? "Confirm inventory"
        : !isProjectSetupConfirmed
          ? "Confirm project setup"
          : pendingSelection
            ? "Ready to analyze"
            : "Ready for a project"
      : printRunAvailable
        ? "Print Run ready"
        : hasPendingPlanChanges
          ? "Changes pending"
          : unresolvedDecisionCount > 0
            ? `${unresolvedDecisionCount} ${
                unresolvedDecisionCount === 1 ? "decision" : "decisions"
              } required`
            : plan.blockingErrors.length > 0 || plan.omittedUnitCount > 0
              ? "Needs attention"
              : validatedPlanAvailable
                ? analysisSource === "browser-demo"
                  ? "Demo plan ready"
                  : "Ready to convert"
                : "Plan analyzed";
  const planStateTone = !hasAnalysis
    ? "plan-state--idle"
    : planNeedsAttention
      ? "plan-state--attention"
      : "";

  useEffect(() => {
    if (activeView === "run" && !printRunAvailable) {
      setActiveView("plan");
      setLiveMessage(
        "Print Run was closed because this plan no longer matches a published conversion.",
      );
    }
  }, [activeView, printRunAvailable]);
  const libraryPersistenceStatus =
    libraryErrorKind === "load"
      ? "Filament library is unavailable. Resolve the loading error before editing or planning."
      : libraryErrorKind === "save"
        ? "Changes are not saved. Resolve the saving error, then retry the change."
        : isLibraryLoading
          ? "Loading the persistent filament library…"
          : isLibrarySaving
            ? "Saving filament library…"
            : isNative
              ? "Saved in this app's local data directory."
              : "Saved in this browser profile for the demo.";
  const calibrationPersistenceStatus =
    calibrationErrorKind === "load"
      ? "Calibration measurements are unavailable until the loading error is resolved."
      : calibrationErrorKind === "mutation"
        ? "The last calibration change was not saved; existing records remain unchanged."
        : isCalibrationLoading
          ? "Loading measured CMY+X calibration records…"
          : isCalibrationSaving
            ? "Saving CMY+X calibration change…"
            : isNative
              ? "Saved in this app's local data directory and qualified by the native U1 process context."
              : "Saved only in this browser demo profile; not used by native planning.";

  const confirmInitialProjectSetup = (
    nextIntent: PlanningIntent,
    nextPrinterLoadout: PrinterLoadoutProfile,
  ) => {
    let equipmentWasPersisted = true;
    if (printingSetup === null && nextIntent.a1MiniEnabled) {
      const inferredSetup: PrintingSetup = {
        schemaVersion: 1,
        primaryPrinter: "u1",
        secondaryPrinter: "a1-mini",
      };
      equipmentWasPersisted = savePrintingSetup(
        appLocalStorage(),
        inferredSetup,
      );
      setPrintingSetup(inferredSetup);
      setPrintingSetupPersistence(
        equipmentWasPersisted ? "persisted" : "session",
      );
    }
    const intentWasPersisted = savePlanningIntent(
      appLocalStorage(),
      nextIntent,
    );
    const loadoutWasPersisted = savePrinterLoadout(
      appLocalStorage(),
      nextPrinterLoadout,
    );
    setPlanningIntent(nextIntent);
    setPrinterLoadout(nextPrinterLoadout);
    setA1MiniEnabled(nextIntent.a1MiniEnabled);
    setIsProjectSetupConfirmed(true);
    setIsSetupRecommendationOpen(false);
    const printerSummary = nextIntent.a1MiniEnabled
      ? "Snapmaker U1 and Bambu Lab A1 mini"
      : "Snapmaker U1 only";
    const strategySummary =
      nextIntent.defaultStrategy === "direct"
        ? "Direct Spools only"
        : nextIntent.defaultStrategy === "cmyx"
          ? "CMY+X Full Spectrum only"
          : "Automatic strategy";
    setLiveMessage(
      `Project setup confirmed: ${printerSummary}, ${strategySummary}.${
        equipmentWasPersisted && intentWasPersisted && loadoutWasPersisted
          ? ""
          : " The choice is available for this session but could not be saved."
      }`,
    );
  };

  const saveApplicationPrintingSetup = (nextSetup: PrintingSetup) => {
    const persisted = savePrintingSetup(appLocalStorage(), nextSetup);
    const removedActiveA1Mini =
      nextSetup.secondaryPrinter === null &&
      a1MiniEnabled &&
      analysisSource !== "browser-demo";
    setPrintingSetup(nextSetup);
    setPrintingSetupPersistence(persisted ? "persisted" : "session");
    if (hasAnalysis) {
      setIsSetupRecommendationOpen(true);
    }
    if (removedActiveA1Mini) {
      setA1MiniEnabled(false);
      const nextIntent = {
        ...planningIntent,
        a1MiniEnabled: false,
      };
      setPlanningIntent(nextIntent);
      savePlanningIntent(appLocalStorage(), nextIntent);
      if (!hasAnalysis) setIsProjectSetupConfirmed(false);
    }
    const nextPrinterLoadout = constrainPrinterLoadoutToEquipment(
      printerLoadout,
      nextSetup,
    );
    if (!printerLoadoutEquals(printerLoadout, nextPrinterLoadout)) {
      setPrinterLoadout(nextPrinterLoadout);
      savePrinterLoadout(appLocalStorage(), nextPrinterLoadout);
      if (!hasAnalysis) setIsProjectSetupConfirmed(false);
    }
    const printerSummary =
      nextSetup.secondaryPrinter === "a1-mini"
        ? "Snapmaker U1 and Bambu Lab A1 mini"
        : "Snapmaker U1 only";
    setLiveMessage(
      `Printing setup updated: ${printerSummary}.${
        removedActiveA1Mini
          ? " Recalculate the plan to remove A1 mini routing."
          : ""
      }${
        persisted
          ? ""
          : " The change is available for this session but could not be saved."
      }`,
    );
  };

  const saveCompletedPrintRunLoadout = (snapshot: PrinterLoadoutSnapshot) => {
    const nextPrinterLoadout = constrainPrinterLoadoutToEquipment(
      {
        schemaVersion: 1,
        currentLoadout: snapshot.currentLoadout.map((entry) => ({ ...entry })),
        currentA1SpoolId: snapshot.currentA1SpoolId,
        updatedFrom: "print-run",
      },
      printingSetup,
    );
    const persisted = savePrinterLoadout(appLocalStorage(), nextPrinterLoadout);
    setPrinterLoadout(nextPrinterLoadout);
    setAppliedPrinterLoadout(nextPrinterLoadout);
    setLiveMessage(
      persisted
        ? "The completed Print Run loadout was saved for the next project."
        : "The completed Print Run loadout is available for this session but could not be saved locally.",
    );
  };

  const openPrintingSetupEditor = () => {
    setActiveView("plan");
    setIsPrintingSetupOpen(true);
    window.requestAnimationFrame(() => {
      document
        .querySelector<HTMLInputElement>("#printing-setup input")
        ?.focus();
    });
  };

  const openApplicationView = (view: ApplicationView) => {
    setActiveView(view);
    if (view === "calibration") {
      void loadCalibrationLibraryView();
    }
    setLiveMessage(
      view === "plan"
        ? "Print Plan opened."
        : view === "library"
          ? "Filament Library opened."
          : view === "calibration"
            ? "Color Calibration opened."
            : view === "reference"
              ? "Color Reference opened."
              : "Print Run opened.",
    );
  };

  const projectSetupControl = (
    <ProjectSetupControl
      intent={projectSetupIntent}
      loadout={printerLoadout}
      equipmentSetup={printingSetup}
      availableSpools={librarySpools}
      hasAnalysis={hasAnalysis}
      a1PlateCount={a1MiniPlateCount}
      isFixedPreview={analysisSource === "browser-demo"}
      isOpen={
        isSetupRecommendationOpen ||
        (!hasAnalysis && !isProjectSetupConfirmed)
      }
      isBusy={isAnalyzing || isValidating || isLibrarySaving}
      isStale={hasPendingPlanChanges || isValidating}
      customDirectPalettesEnabled={customDirectPalettesEnabled}
      customDirectPalettesFixed={analysisSource === "browser-demo"}
      onOpenChange={setIsSetupRecommendationOpen}
      onConfirm={(nextIntent, nextPrinterLoadout) => {
        if (hasAnalysis) {
          void validateChoices(nextIntent, undefined, nextPrinterLoadout);
        } else {
          confirmInitialProjectSetup(nextIntent, nextPrinterLoadout);
        }
      }}
      onChangeEquipment={openPrintingSetupEditor}
      onCustomDirectPalettesChange={
        hasAnalysis ? changeCustomDirectPalettes : undefined
      }
    />
  );

  return (
    <div className="app-shell">
      <a className="skip-link" href="#main-content">
        Skip to current view
      </a>
      <TitleBar
        onOpenFilamentLibrary={() => openApplicationView("library")}
        onOpenColorCalibration={() => openApplicationView("calibration")}
        onOpenPrintingSetup={openPrintingSetupEditor}
      />
      <ApplicationNavigation
        activeView={activeView}
        printRunAvailable={printRunAvailable}
        onChange={openApplicationView}
      />
      {activeView === "plan" ? (
        <>
          <div
            className={`workspace-grid ${
              selectedPlate ? "" : "workspace-grid--without-inspector"
            }`}
          >
            <WorkflowRail
              inventoryReady={inventoryReady}
              projectSetupReady={initialSetupReady}
              hasAnalysis={hasAnalysis}
              needsAttention={planNeedsAttention}
              validatedPlanAvailable={validatedPlanAvailable}
              printRunAvailable={printRunAvailable}
            />
            <div className="central-column">
              <ProjectHeader
                summary={hasAnalysis ? plan.summary : null}
                analysisSource={hasAnalysis ? analysisSource : null}
                pendingSelection={pendingSelection}
                isAnalyzing={isAnalyzing}
                isCancellingAnalysis={isCancellingAnalysis}
                isNative={isNative}
                browserFileInputRef={browserFileInputRef}
                onBrowserFile={chooseBrowserProject}
                onChooseNativeFile={chooseNativeProject}
                onAnalyze={runAnalysis}
                onCancelAnalysis={cancelAnalysis}
                analysisAvailable={isProjectSetupConfirmed}
                analysisBlockReason="Confirm Project setup in Step 2 first"
              />
              <main id="main-content" className="main-workspace" tabIndex={-1}>
                <div className="plan-heading">
                  <div>
                    <h2>Print Plan</h2>
                    <p>
                      Review batch order, physical loadouts, and color fidelity
                      before conversion.
                    </p>
                  </div>
                  <span className={`plan-state ${planStateTone}`}>
                    <span aria-hidden="true" />
                    {planStateLabel}
                  </span>
                </div>
                <div className="registration-rule" aria-hidden="true" />
                {isPrintingSetupOpen ? (
                  <PrintingSetupControl
                    setup={printingSetup}
                    persistenceStatus={printingSetupPersistence}
                    isOpen={isPrintingSetupOpen}
                    onOpenChange={(open) => {
                      setIsPrintingSetupOpen(open);
                      if (!open && hasAnalysis) {
                        window.requestAnimationFrame(() => {
                          document
                            .getElementById("project-setup-change")
                            ?.focus();
                        });
                      }
                    }}
                    onSave={saveApplicationPrintingSetup}
                  />
                ) : null}
                {analysisError ? (
                  <p className="analysis-error" role="alert">
                    <TriangleAlert aria-hidden="true" />
                    {analysisError}
                  </p>
                ) : null}
                {hasAnalysis && printRunRecoveryStatus === "checking" ? (
                  <p className="print-run-recovery-status" role="status">
                    <span
                      className="print-run-recovery-spinner"
                      aria-hidden="true"
                    />
                    Revalidating saved Print Run files against this source and
                    plan…
                  </p>
                ) : null}
                {hasAnalysis && printRunRecoveryError ? (
                  <div className="print-run-recovery-error" role="alert">
                    <TriangleAlert aria-hidden="true" />
                    <p>{printRunRecoveryError}</p>
                    {isNative && activeSourcePath ? (
                      <button
                        className="button button--compact"
                        type="button"
                        disabled={printRunRecoveryStatus === "checking"}
                        onClick={() => {
                          printRunRecoveryAttemptRef.current = "";
                          setPrintRunRecoveryError("");
                          setPrintRunRecoveryRevision(
                            (revision) => revision + 1,
                          );
                        }}
                      >
                        Retry saved-run recovery
                      </button>
                    ) : null}
                  </div>
                ) : null}
                {hasAnalysis && plan.blockingErrors.length > 0 ? (
                  <section
                    className="plan-blockers"
                    aria-labelledby="plan-blockers-heading"
                  >
                    <TriangleAlert aria-hidden="true" />
                    <div>
                      <h3 id="plan-blockers-heading">
                        {plan.colorResolutions.length > 0
                          ? "Plan needs color decisions"
                          : "Plan has blocking errors"}
                      </h3>
                      <p>
                        {plan.partialConversion.available &&
                        plan.partialConversion.exclusions.length > 0
                          ? `${plan.partialConversion.exclusions.length} printable ${
                              plan.partialConversion.exclusions.length === 1
                                ? "unit cannot"
                                : "units cannot"
                            } be converted with the current plan. Full-plan export remains disabled; the valid jobs can be converted only after you review and acknowledge every exclusion below.`
                          : plan.omittedUnitCount > 0
                            ? `${plan.omittedUnitCount} printable ${
                                plan.omittedUnitCount === 1
                                  ? "unit is"
                                  : "units are"
                              } omitted. Export remains disabled until the choices are valid.`
                            : "Export remains disabled until the blocking choices are resolved."}
                      </p>
                      <details>
                        <summary>Technical details</summary>
                        <ul>
                          {plan.blockingErrors.map((error) => (
                            <li key={error}>{error}</li>
                          ))}
                        </ul>
                      </details>
                    </div>
                  </section>
                ) : null}
                {hasAnalysis && plan.partialConversion.exclusions.length > 0 ? (
                  <PartialConversionNotice
                    available={
                      plan.partialConversion.available &&
                      !isCheckingConversion &&
                      Boolean(conversionCapability?.planFingerprint)
                    }
                    exclusions={plan.partialConversion.exclusions}
                    reason={plan.partialConversion.reason}
                    acknowledged={partialConversionAcknowledged}
                    onAcknowledgedChange={(acknowledged) => {
                      setPartialConversionAcknowledgementKey(
                        acknowledged ? partialConversionEvidenceKey : null,
                      );
                      setLiveMessage(
                        acknowledged
                          ? `${plan.partialConversion.exclusions.length} excluded source ${
                              plan.partialConversion.exclusions.length === 1
                                ? "unit acknowledged"
                                : "units acknowledged"
                            }. Only valid jobs will be converted.`
                          : "Partial conversion acknowledgement cleared.",
                      );
                    }}
                  />
                ) : null}
                {hasAnalysis &&
                conversionCapability?.experimentalDialectApprovalRequired &&
                conversionCapability.experimentalDialectFingerprint ? (
                  <section
                    className="plan-blockers"
                    aria-labelledby="experimental-dialect-heading"
                  >
                    <TriangleAlert aria-hidden="true" />
                    <div>
                      <h3 id="experimental-dialect-heading">
                        Experimental source dialect
                      </h3>
                      <p>
                        This application/version has not passed the verified
                        source-dialect fixture. Conversion remains locked until
                        you approve this exact source fingerprint.
                      </p>
                      <label>
                        <input
                          type="checkbox"
                          checked={experimentalDialectAcknowledged}
                          onChange={(event) => {
                            setExperimentalDialectAcknowledgementKey(
                              event.target.checked
                                ? experimentalDialectEvidenceKey
                                : null,
                            );
                            setLiveMessage(
                              event.target.checked
                                ? "Experimental source dialect approved for this exact source fingerprint."
                                : "Experimental source dialect approval cleared.",
                            );
                          }}
                        />{" "}
                        I understand this source dialect is Experimental and
                        approve conversion of this exact file.
                      </label>
                      <p>
                        Fingerprint:{" "}
                        <code>
                          {conversionCapability.experimentalDialectFingerprint}
                        </code>
                      </p>
                    </div>
                  </section>
                ) : null}
                {hasAnalysis ? (
                  <PlanWarnings warnings={plan.globalWarnings} />
                ) : null}
                {hasAnalysis ? (
                  <>
                    {!isPrintingSetupOpen ? projectSetupControl : null}
                    <ColorResolutionPanel
                      resolutions={plan.colorResolutions}
                      spools={plan.spools}
                      onAcceptColor={acceptColorApproximation}
                      onSelectCmyxColor={selectCmyxPaletteColor}
                      onAcceptMaterialSubstitution={acceptMaterialSubstitution}
                      onAcceptAllSameMaterial={
                        acceptAllSameMaterialApproximations
                      }
                      onChooseExistingSpool={chooseExistingSpoolForResolution}
                      directUnavailableReason={
                        directUnavailableReasonForResolution
                      }
                      onAddSpool={addSpool}
                    />
                    <StatusStrip {...stats} />
                    <BatchTimeline
                      batches={plan.batches}
                      totalPlates={plan.plates.length}
                      isStale={hasPendingPlanChanges}
                    />
                    <PlateTable
                      plates={plan.plates}
                      spools={plan.spools}
                      selectedPlateId={selectedPlateId}
                      onSelectPlate={selectPlate}
                      a1MiniEnabled={a1MiniEnabled}
                      a1MiniConfigured={
                        a1MiniConfigured || analysisSource === "browser-demo"
                      }
                      hasPendingPlanChanges={hasPendingPlanChanges}
                      u1CrossSourceRepackingEnabled={
                        plan.u1CrossSourceRepackingEnabled
                      }
                      onBulkStrategyChange={changeBulkStrategies}
                    />
                    <details className="advanced-planning">
                      <summary>
                        <span>Advanced planning</span>
                        <small>
                          Alternative plates and project-wide Direct Spools
                        </small>
                      </summary>
                      <div className="advanced-planning__content">
                        <AlternativePlateSelector
                          plates={plan.alternativePlates}
                          onChange={changeAlternativePlate}
                        />
                        <ProjectDirectPalette
                          plan={plan}
                          isStale={hasPendingPlanChanges}
                          onApply={applyWholeProjectDirectPalette}
                        />
                      </div>
                    </details>
                  </>
                ) : (
                  <GettingStarted
                    availableSpoolCount={availableSpoolCount}
                    isInventoryLoading={isLibraryLoading}
                    isProjectSetupConfirmed={isProjectSetupConfirmed}
                    projectSetup={
                      isPrintingSetupOpen ? null : projectSetupControl
                    }
                    pendingFileName={pendingSelection?.fileName}
                    isAnalyzing={isAnalyzing}
                    onOpenFilamentLibrary={() => openApplicationView("library")}
                    onOpenProject={() => {
                      if (isNative) {
                        void chooseNativeProject();
                      } else {
                        browserFileInputRef.current?.click();
                      }
                    }}
                    onAnalyzeProject={() => void runAnalysis()}
                  />
                )}
              </main>
            </div>
            {selectedPlate ? (
              <PlateInspector
                plan={plan}
                plate={selectedPlate}
                isOpen={isInspectorOpen}
                onClose={() => {
                  setIsInspectorOpen(false);
                  setLiveMessage("Plate details closed.");
                  window.requestAnimationFrame(() => {
                    document
                      .getElementById(`plate-select-${selectedPlate.id}`)
                      ?.focus({ preventScroll: true });
                  });
                }}
                restoreCmy={restoreCmy}
                onStrategyChange={changeStrategy}
                onRestoreChange={(restore) => {
                  setRestoreCmy(restore);
                  setIsPlanDirty(true);
                  setLiveMessage(
                    restore
                      ? "Restore CMY setup actions added to the plan."
                      : "Restore CMY setup actions removed from the plan.",
                  );
                }}
                onToolheadChange={changeToolhead}
                onSpoolChange={changeSpool}
                onMaterialSubstitutionChange={changeDirectMaterialSubstitution}
                onSelectCmyxColor={selectCmyxPaletteColor}
                a1MiniEnabled={a1MiniEnabled}
                onPrinterPreferenceChange={changePrinterPreference}
                onAddSpool={addSpool}
                customDirectPalettesEnabled={plan.customDirectPalettesEnabled}
                isPreparingCustomPalette={isValidating}
                customPaletteError={
                  customPaletteFailure?.scopeId === selectedPlate.scopeId
                    ? customPaletteFailure.message
                    : undefined
                }
                onPrepareCustomPalette={
                  isNative && activeSourcePath
                    ? () => {
                        void validateChoices(undefined, selectedPlate.scopeId);
                      }
                    : undefined
                }
                onOpenFilamentLibrary={() => openApplicationView("library")}
              />
            ) : null}
          </div>
          {hasAnalysis ? (
            <BottomActionRail
              isBusy={
                isAnalyzing ||
                isValidating ||
                isLibrarySaving ||
                isPreparingConversion ||
                isConverting
              }
              isValidating={isValidating}
              planAvailable={hasAnalysis}
              exportAvailable={validatedPlanAvailable}
              exportBlockReason={
                plan.blockingErrors.length > 0 || plan.omittedUnitCount > 0
                  ? "Resolve blocking choices first"
                  : libraryError
                    ? "Resolve the filament library error"
                    : undefined
              }
              onExport={exportPlan}
              onValidate={validateChoices}
              validationAvailable={
                hasAnalysis &&
                !isLibraryLoading &&
                !libraryError &&
                (!isNative || Boolean(activeSourcePath))
              }
              planNeedsRecalculation={hasPendingPlanChanges}
              colorNeedsRecalculation={colorNeedsRecalculation}
              conversionStageAvailable={isNative && conversionScopeAvailable}
              conversionAvailable={conversionAvailable}
              conversionAdapters={conversionCapability?.adapters ?? []}
              isCheckingConversion={isCheckingConversion}
              conversionLabel={
                partialConversionPlanAvailable
                  ? "Review valid jobs"
                  : "Review conversion"
              }
              conversionBlockReason={conversionBlockReason}
              onApprove={approveAndPrepareConversion}
            />
          ) : null}
        </>
      ) : activeView === "library" ? (
        <main id="main-content" className="standalone-workspace" tabIndex={-1}>
          {libraryError ? (
            <p className="analysis-error" role="alert">
              <TriangleAlert aria-hidden="true" />
              {libraryError}
            </p>
          ) : null}
          <p className="library-persistence-status" role="status">
            {libraryPersistenceStatus}
          </p>
          {isLibraryLoading ? (
            <section
              className="empty-plan"
              aria-labelledby="library-loading-heading"
            >
              <div>
                <h2 id="library-loading-heading">Loading filament library</h2>
                <p>
                  Editing becomes available after the persistent inventory is
                  loaded.
                </p>
              </div>
            </section>
          ) : libraryErrorKind === "load" ? (
            <section
              className="empty-plan"
              aria-labelledby="library-unavailable-heading"
            >
              <TriangleAlert aria-hidden="true" />
              <div>
                <h2 id="library-unavailable-heading">
                  Filament library unavailable
                </h2>
                <p>Restart the app after resolving the storage error.</p>
              </div>
            </section>
          ) : (
            <FilamentLibrary
              spools={librarySpools}
              onAddSpool={addSpool}
              onUpdateSpool={updateLibrarySpool}
              onAvailabilityChange={changeLibraryAvailability}
              onDeleteSpool={deleteLibrarySpool}
            />
          )}
        </main>
      ) : activeView === "calibration" ? (
        <main id="main-content" className="standalone-workspace" tabIndex={-1}>
          {calibrationError ? (
            <p className="analysis-error" role="alert">
              <TriangleAlert aria-hidden="true" />
              {calibrationError}
            </p>
          ) : null}
          <p className="library-persistence-status" role="status">
            {calibrationPersistenceStatus}
          </p>
          {isCalibrationLoading ||
          (!calibrationLibrary && !calibrationError) ? (
            <section
              className="empty-plan"
              aria-labelledby="calibration-loading-heading"
            >
              <div>
                <h2 id="calibration-loading-heading">
                  Loading color calibration
                </h2>
                <p>
                  Measurement tools become available after stored records are
                  loaded.
                </p>
              </div>
            </section>
          ) : calibrationErrorKind === "load" || !calibrationLibrary ? (
            <section
              className="empty-plan"
              aria-labelledby="calibration-unavailable-heading"
            >
              <TriangleAlert aria-hidden="true" />
              <div>
                <h2 id="calibration-unavailable-heading">
                  Color calibration unavailable
                </h2>
                <p>
                  Resolve the storage problem, then retry without changing the
                  stored data.
                </p>
                <button
                  className="button"
                  type="button"
                  disabled={isCalibrationLoading}
                  onClick={() => void loadCalibrationLibraryView()}
                >
                  Retry loading
                </button>
              </div>
            </section>
          ) : (
            <ColorCalibration
              library={calibrationLibrary}
              spools={librarySpools}
              isNative={isNative}
              isInventoryReady={
                !isLibraryLoading && libraryErrorKind !== "load"
              }
              isSaving={isCalibrationSaving}
              isGeneratingProject={isCalibrationProjectGenerating}
              onGenerateProject={generateCalibrationProject}
              onSaveMeasurement={saveCalibrationMeasurement}
              onDeleteRecord={deleteCalibrationRecord}
              onSetPlanningGeometry={saveCalibrationPlanningGeometry}
            />
          )}
        </main>
      ) : activeView === "reference" ? (
        <main id="main-content" className="standalone-workspace" tabIndex={-1}>
          <CmyxReference />
        </main>
      ) : (
        <main id="main-content" className="standalone-workspace" tabIndex={-1}>
          <PrintRun
            plan={plan}
            isPlanValidated={
              (validatedPlanAvailable ||
                (partialConversionPlanAvailable &&
                  partialConversionAcknowledged)) &&
              analysisSource === "tauri"
            }
            publishedBundle={publishedPrintRunBundle}
            preferredArtifacts={preferredPrintArtifacts}
            finalLoadoutSaved={printerLoadoutEquals(printerLoadout, {
              schemaVersion: 1,
              currentLoadout: plan.plannedFinalLoadout,
              currentA1SpoolId: plan.plannedFinalA1SpoolId,
              updatedFrom: "print-run",
            })}
            onSaveFinalLoadout={saveCompletedPrintRunLoadout}
            onCurrentTargetChange={setSelectedPlateId}
            onOpenArtifact={(artifact) => openConvertedOutput(artifact)}
          />
        </main>
      )}
      <ConversionDialog
        prepared={preparedConversion}
        destinationDirectory={conversionDestination}
        isConverting={isConverting}
        progress={conversionProgress}
        result={conversionResult}
        error={conversionError}
        needsNewPreflight={conversionNeedsNewPreflight}
        activeOutputAction={activeConversionOutputAction}
        detectedAdhesion={plan.detectedAdhesion}
        onConvert={convertPreparedProjects}
        onCancelConversion={cancelPreparedConversion}
        onOpenOutput={(artifact) => {
          void runConversionOutputAction("open", artifact);
        }}
        onShowOutputInFinder={(artifact) => {
          void runConversionOutputAction("reveal", artifact);
        }}
        onPreferredArtifactsChange={setPreferredPrintArtifacts}
        onRetryPreflight={retryConversionPreflight}
        onOpenPrintRun={() => {
          closeConversionDialog();
          openApplicationView("run");
        }}
        onClose={closeConversionDialog}
      />
      <div
        className="visually-hidden"
        role="status"
        aria-live="polite"
        aria-atomic="true"
      >
        {liveMessage}
      </div>
    </div>
  );
}
