// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  listPublishedOrientationPlates,
  optimizePublishedPlates,
} from "../services/project-analysis";
import type { PublishedConversionArtifact } from "../types";
import { PublishedOrientationPanel } from "./PublishedOrientationPanel";

vi.mock("../services/project-analysis", () => ({
  listPublishedOrientationPlates: vi.fn(),
  optimizePublishedPlates: vi.fn(),
}));

const artifact: PublishedConversionArtifact = {
  adapterId: "snapmaker-orca/u1-direct",
  target: "u1_direct",
  printer: "Snapmaker U1",
  strategy: "Direct Spools",
  slicer: "Snapmaker Orca",
  batchId: "batch-1",
  fileName: "project-direct.3mf",
  relativePath: "u1-direct/project-direct.3mf",
  path: "/tmp/bundle/u1-direct/project-direct.3mf",
  byteSize: 1024,
  sha256: "a".repeat(64),
  plateCount: 2,
  targetPlateIds: ["plate-1", "plate-2"],
  sourceUnitIds: ["unit-1", "unit-2"],
  loadout: [],
  setupActions: [],
  validationStatus: "passed",
  adapterEvidence: null,
};

describe("PublishedOrientationPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(listPublishedOrientationPlates).mockResolvedValue([
      { id: 1, name: "Plate 1", printableInstanceCount: 2 },
      { id: 2, name: "Plate 2", printableInstanceCount: 1 },
    ]);
    vi.mocked(optimizePublishedPlates).mockResolvedValue({
      sourcePath: artifact.path,
      destinationPath: "/tmp/project-direct-support-optimized.3mf",
      byteSize: 2048,
      sha256: "b".repeat(64),
      reports: [
        {
          plate_id: 1,
          repair_attempts: 1,
          source_score: 100,
          selected_score: 75,
          estimated_support_volume_improvement: 0.2,
          adhesion_mode: "reliable",
          reserved_process_envelope_mm: 18,
          maximum_adhesion_risk: {
            score: 42,
            level: "moderate",
            recommended_brim_width_mm: 8,
          },
          instances: [],
        },
      ],
    });
  });

  afterEach(cleanup);

  it("loads actual generated plates only after explicit opt-in", async () => {
    render(<PublishedOrientationPanel artifacts={[artifact]} />);

    expect(listPublishedOrientationPlates).not.toHaveBeenCalled();
    fireEvent.click(
      screen.getByRole("checkbox", {
        name: "Enable optional support-aware optimization after conversion",
      }),
    );

    expect(
      await screen.findByRole("checkbox", { name: "Plate 1 · 2 instances" }),
    ).toBeChecked();
    expect(
      screen.getByRole("checkbox", { name: "Plate 2 · 1 instance" }),
    ).toBeChecked();
    expect(listPublishedOrientationPlates).toHaveBeenCalledWith(artifact);
  });

  it("optimizes only checked plates into a separate copy", async () => {
    const onResultsChange = vi.fn();
    render(
      <PublishedOrientationPanel
        artifacts={[artifact]}
        onResultsChange={onResultsChange}
      />,
    );
    fireEvent.click(
      screen.getByRole("checkbox", {
        name: "Enable optional support-aware optimization after conversion",
      }),
    );
    fireEvent.click(
      await screen.findByRole("checkbox", { name: "Plate 2 · 1 instance" }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Optimize 1 selected plate" }),
    );

    await waitFor(() => {
      expect(optimizePublishedPlates).toHaveBeenCalledWith(
        artifact,
        [1],
        "reliable",
      );
    });
    expect(await screen.findByText("Combined score improvement 25.0%")).toBeVisible();
    expect(
      screen.getByText("/tmp/project-direct-support-optimized.3mf"),
    ).toBeVisible();
    expect(onResultsChange).toHaveBeenLastCalledWith([
      expect.objectContaining({
        sourcePath: artifact.path,
        destinationPath: "/tmp/project-direct-support-optimized.3mf",
      }),
    ]);
  });

  it("keeps successful copies and reports an unpackable artifact as an original fallback", async () => {
    const a1Artifact: PublishedConversionArtifact = {
      ...artifact,
      adapterId: "bambu-studio/a1-mini",
      target: "a1_mini_mono",
      printer: "Bambu Lab A1 mini",
      slicer: "Bambu Studio",
      batchId: "job-2",
      fileName: "project-a1-mini.3mf",
      relativePath: "a1-mini/project-a1-mini.3mf",
      path: "/tmp/bundle/a1-mini/project-a1-mini.3mf",
      sha256: "c".repeat(64),
    };
    vi.mocked(optimizePublishedPlates).mockImplementation((candidate) => {
      if (candidate.path === a1Artifact.path) {
        return Promise.reject(
          new Error(
            "Generated plate 1: invalid project structure: no support-aware orientation layout fit plate 1 within 50 bounded repair attempts",
          ),
        );
      }
      return Promise.resolve({
        sourcePath: artifact.path,
        destinationPath: "/tmp/project-direct-support-optimized.3mf",
        byteSize: 2048,
        sha256: "b".repeat(64),
        reports: [
          {
            plate_id: 1,
            repair_attempts: 1,
            source_score: 100,
            selected_score: 75,
            estimated_support_volume_improvement: 0.2,
            adhesion_mode: "reliable",
            reserved_process_envelope_mm: 18,
            maximum_adhesion_risk: {
              score: 42,
              level: "moderate",
              recommended_brim_width_mm: 8,
            },
            instances: [],
          },
        ],
      });
    });
    const onResultsChange = vi.fn();
    render(
      <PublishedOrientationPanel
        artifacts={[artifact, a1Artifact]}
        onResultsChange={onResultsChange}
      />,
    );
    fireEvent.click(
      screen.getByRole("checkbox", {
        name: "Enable optional support-aware optimization after conversion",
      }),
    );
    await screen.findAllByRole("checkbox", { name: "Plate 1 · 2 instances" });
    fireEvent.click(
      screen.getByRole("button", { name: "Optimize 4 selected plates" }),
    );

    expect(
      await screen.findByText("Other generated files kept their original layout"),
    ).toBeVisible();
    expect(screen.getByText(a1Artifact.path)).toBeVisible();
    expect(
      screen.getByText(/No safe support-aware layout fits this printer bed/),
    ).toBeVisible();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(onResultsChange).toHaveBeenLastCalledWith([
      expect.objectContaining({ sourcePath: artifact.path }),
    ]);
  });

  it("passes the explicitly selected maximum adhesion policy", async () => {
    render(<PublishedOrientationPanel artifacts={[artifact]} />);
    fireEvent.click(
      screen.getByRole("checkbox", {
        name: "Enable optional support-aware optimization after conversion",
      }),
    );
    await screen.findByRole("checkbox", { name: "Plate 1 · 2 instances" });
    fireEvent.click(
      screen.getByRole("radio", { name: /Maximum Add a two-layer raft/ }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Optimize 2 selected plates" }),
    );

    await waitFor(() => {
      expect(optimizePublishedPlates).toHaveBeenCalledWith(
        artifact,
        [1, 2],
        "maximum",
      );
    });
  });

  it("restores a recognized adhesion policy from the source project", async () => {
    render(
      <PublishedOrientationPanel
        artifacts={[artifact]}
        detectedAdhesion={{
          mode: "maximum",
          profileName: "U1 Planner Maximum Adhesion v2",
        }}
      />,
    );
    fireEvent.click(
      screen.getByRole("checkbox", {
        name: "Enable optional support-aware optimization after conversion",
      }),
    );

    expect(
      await screen.findByRole("radio", {
        name: /Maximum Add a two-layer raft/,
      }),
    ).toBeChecked();
    expect(
      screen.getByText("Restored maximum adhesion from the source 3MF."),
    ).toBeVisible();
  });

  it("fails safely when a planner adhesion profile was modified", async () => {
    render(
      <PublishedOrientationPanel
        artifacts={[artifact]}
        detectedAdhesion={{
          mode: "custom",
          profileName: "U1 Planner Reliable Adhesion v2",
        }}
      />,
    );
    fireEvent.click(
      screen.getByRole("checkbox", {
        name: "Enable optional support-aware optimization after conversion",
      }),
    );

    expect(
      await screen.findByRole("radio", {
        name: /Reliable \(recommended\)/,
      }),
    ).toBeChecked();
    expect(
      screen.getByText(/source uses a modified U1 Planner adhesion profile/),
    ).toBeVisible();
  });
});
