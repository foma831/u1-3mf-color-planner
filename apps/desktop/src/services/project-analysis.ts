import { open, save } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { createDemoPlan } from "../data/mock-plan";
import type {
  AnalysisResult,
  CancelAnalysisResult,
  CancelConversionResult,
  ConversionCapability,
  ConversionProgress,
  ConversionResult,
  ExperimentalDialectApproval,
  PartialConversionApproval,
  PreparedConversion,
  PublishedConversionArtifact,
  ProjectPlan,
  ProjectSelection,
  ReplanRequest,
} from "../types";

const DEMO_ANALYSIS_DELAY_MS = 650;

export function isTauriRuntime() {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export async function chooseProjectPath(): Promise<ProjectSelection | null> {
  if (!isTauriRuntime()) return null;

  const sourcePath = await open({
    multiple: false,
    directory: false,
    filters: [{ name: "3MF project", extensions: ["3mf"] }],
  });

  if (typeof sourcePath !== "string") return null;

  return {
    sourcePath,
    fileName: sourcePath.split(/[\\/]/).at(-1) ?? "Selected project.3mf",
  };
}

export async function analyzeProject(
  selection: ProjectSelection,
): Promise<AnalysisResult> {
  if (isTauriRuntime() && selection.sourcePath) {
    const plan = await invoke<ProjectPlan>("analyze_project", {
      sourcePath: selection.sourcePath,
    });

    return { plan, source: "tauri" };
  }

  await new Promise((resolve) => window.setTimeout(resolve, DEMO_ANALYSIS_DELAY_MS));
  return {
    plan: createDemoPlan(selection.fileName),
    source: "browser-demo",
  };
}

export function cancelNativeAnalysis() {
  return invoke<CancelAnalysisResult>("cancel_analysis");
}

export async function exportNativePlan(
  plan: ProjectPlan,
  suggestedFileName: string,
  sourcePath: string,
) {
  const destinationPath = await save({
    defaultPath: suggestedFileName,
    filters: [{ name: "Print plan JSON", extensions: ["json"] }],
  });
  if (!destinationPath) return null;

  await invoke("export_plan", {
    destinationPath,
    contents: JSON.stringify(plan, null, 2),
    sourcePath,
    sourceHash: plan.summary.sourceHash,
  });
  return destinationPath;
}

export function replanProject(sourcePath: string, request: ReplanRequest) {
  return invoke<ProjectPlan>("replan_project", { sourcePath, request });
}

export function inspectConversionCapabilities(sourcePath: string) {
  return invoke<ConversionCapability>("inspect_conversion_capabilities", {
    sourcePath,
  });
}

export async function chooseConversionDestination() {
  const destination = await open({
    multiple: false,
    directory: true,
    title: "Choose conversion destination",
  });
  return typeof destination === "string" ? destination : null;
}

export function prepareNativeConversion(
  sourcePath: string,
  sourceSha256: string,
  planFingerprint: string,
  partialConversionApproval: PartialConversionApproval | null,
  experimentalDialectApproval: ExperimentalDialectApproval | null,
) {
  return invoke<PreparedConversion>("prepare_conversion", {
    sourcePath,
    sourceSha256,
    planFingerprint,
    partialConversionApproval,
    experimentalDialectApproval,
  });
}

export function convertNativeProject(
  preparationToken: string,
  destinationDirectory: string,
  acknowledgedWarnings: readonly string[],
) {
  return invoke<ConversionResult>("convert_project", {
    preparationToken,
    destinationDirectory,
    acknowledgedWarnings,
  });
}

export function revalidatePublishedConversion(
  sourcePath: string,
  sourceSha256: string,
  planFingerprint: string,
  outputDirectory: string,
  experimentalDialectApproval: ExperimentalDialectApproval | null,
) {
  return invoke<ConversionResult>("revalidate_published_conversion", {
    sourcePath,
    sourceSha256,
    planFingerprint,
    outputDirectory,
    experimentalDialectApproval,
  });
}

export function cancelNativeConversion(conversionId: string) {
  return invoke<CancelConversionResult>("cancel_conversion", {
    conversionId,
  });
}

export function openConvertedOutput(artifact: PublishedConversionArtifact) {
  return invoke<void>("open_output_in_slicer", {
    path: artifact.path,
    adapterId: artifact.adapterId,
  });
}

export function showConvertedOutputInFinder(
  artifact: PublishedConversionArtifact,
) {
  return invoke<void>("show_output_in_finder", {
    path: artifact.path,
    adapterId: artifact.adapterId,
  });
}

export function listenToConversionProgress(
  handler: (progress: ConversionProgress) => void,
): Promise<UnlistenFn> {
  const internals = (window as Window & {
    __TAURI_INTERNALS__?: { transformCallback?: unknown };
  }).__TAURI_INTERNALS__;
  if (typeof internals?.transformCallback !== "function") {
    return Promise.resolve(() => undefined);
  }
  return listen<ConversionProgress>("conversion-progress", (event) => {
    handler(event.payload);
  });
}
