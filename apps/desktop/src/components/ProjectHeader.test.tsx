// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ProjectHeader } from "./ProjectHeader";

afterEach(cleanup);

describe("ProjectHeader analysis evidence", () => {
  it("shows source provenance, geometry coverage, and alternatives on demand", () => {
    render(
      <ProjectHeader
        summary={{
          fileName: "Withered_Foxy.3mf",
          sourceHash: "sha256:fixture",
          sourceDialect: "Bambu Studio 3MF",
          sourceApplication: "Bambu Studio 02.02.00.85",
          sourceByteSize: 202_162_511,
          sourcePlateCount: 12,
          objectCount: 89,
          instanceCount: 89,
          partCount: 390,
          printablePartCount: 388,
          paintedPartCount: 15,
          boundedInstanceCount: 89,
          usedFilamentCount: 10,
          unusedFilamentCount: 2,
          alternativePlateCount: 4,
        }}
        analysisSource="tauri"
        pendingSelection={null}
        isAnalyzing={false}
        isCancellingAnalysis={false}
        isNative
        onBrowserFile={vi.fn()}
        onChooseNativeFile={vi.fn()}
        onAnalyze={vi.fn()}
        onCancelAnalysis={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByText("Analysis details"));

    expect(screen.getByText("Bambu Studio 02.02.00.85")).toBeInTheDocument();
    expect(screen.getByText("193 MB")).toBeInTheDocument();
    expect(screen.getByText("89 / 89")).toBeInTheDocument();
    expect(screen.getByText("388 / 390")).toBeInTheDocument();
    expect(screen.getByText("Painted parts")).toBeInTheDocument();
    expect(screen.getByText("Unused filament slots")).toBeInTheDocument();
    expect(screen.getByText("Detected alternative plates")).toBeInTheDocument();
  });
});
