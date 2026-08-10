// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ColorResolution, PhysicalSpool } from "../types";
import { ColorResolutionPanel } from "./ColorResolutionPanel";

afterEach(cleanup);

describe("ColorResolutionPanel material substitutions", () => {
  it("acknowledges ABS to PLA without offering an unsupported ABS spool form", () => {
    const resolution: ColorResolution = {
      scopeId: "plate-1",
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
      colorApproved: false,
      materialApproved: false,
      requiresMaterialSubstitution: true,
      canAddDedicatedSpool: false,
      recommendation:
        "Explicitly approve the ABS to PLA substitution; ABS inventory is not supported.",
    };
    const onAcceptMaterialSubstitution = vi.fn();

    render(
      <ColorResolutionPanel
        resolutions={[resolution]}
        spools={[]}
        onAcceptColor={vi.fn()}
        onAcceptMaterialSubstitution={onAcceptMaterialSubstitution}
        onAcceptAllSameMaterial={vi.fn()}
        onChooseExistingSpool={vi.fn()}
        directUnavailableReason={() => null}
        onAddSpool={vi.fn()}
      />,
    );

    expect(screen.getByText("Material change: ABS → PLA")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Choose / add spool" }),
    ).not.toBeInTheDocument();

    const acceptButton = screen.getByRole("button", {
      name: "Accept color and material change",
    });
    expect(acceptButton).toBeDisabled();

    fireEvent.click(
      screen.getByRole("checkbox", {
        name: "I understand the mechanical-property risk.",
      }),
    );
    expect(acceptButton).toBeEnabled();
    fireEvent.click(acceptButton);

    expect(onAcceptMaterialSubstitution).toHaveBeenCalledWith(resolution);
  });

  it("defensively hides an unsupported spool form even if stale data enables it", () => {
    const resolution: ColorResolution = {
      scopeId: "plate-2",
      scopeName: "Bracket",
      requirementId: "bracket-asa",
      candidateId: "candidate-bracket-pla",
      sourceMaterial: "ASA",
      sourceHex: "#333333",
      targetMaterial: "PLA",
      targetHex: "#333333",
      predictedHex: "#303030",
      recipe: "Solid T4",
      deltaE00: 1,
      confidence: "Nominal",
      requiredT4SpoolId: "panchroma-basic-black",
      requiredT4Name: "Panchroma Basic Black",
      requiredT4Hex: "#080808",
      colorApproved: false,
      materialApproved: false,
      requiresMaterialSubstitution: true,
      canAddDedicatedSpool: true,
      recommendation: "Explicitly approve the ASA to PLA substitution.",
    };

    render(
      <ColorResolutionPanel
        resolutions={[resolution]}
        spools={[]}
        onAcceptColor={vi.fn()}
        onAcceptMaterialSubstitution={vi.fn()}
        onAcceptAllSameMaterial={vi.fn()}
        onChooseExistingSpool={vi.fn()}
        directUnavailableReason={() => null}
        onAddSpool={vi.fn()}
      />,
    );

    expect(
      screen.queryByRole("button", { name: "Choose / add spool" }),
    ).not.toBeInTheDocument();
  });
});

describe("ColorResolutionPanel existing inventory", () => {
  const resolution: ColorResolution = {
    scopeId: "plate-3",
    scopeName: "Frame AMS",
    requirementId: "plate-3-requirement-1",
    candidateId: "candidate-petg-black",
    sourceMaterial: "PETG",
    sourceHex: "#000000",
    targetMaterial: "PLA",
    targetHex: "#080A0D",
    predictedHex: "#080A0D",
    recipe: "Dedicated T4",
    deltaE00: 2.1,
    confidence: "Nominal",
    requiredT4SpoolId: "black-pla",
    requiredT4Name: "Black PLA",
    requiredT4Hex: "#080A0D",
    colorApproved: false,
    materialApproved: false,
    requiresMaterialSubstitution: true,
    canAddDedicatedSpool: true,
    recommendation: "Choose a spool or approve the CMY+X fallback.",
  };
  const spools: PhysicalSpool[] = [
    {
      id: "petg-grey",
      source: "user",
      name: "Workshop PETG Grey",
      colorName: "Grey",
      hex: "#92979A",
      material: "PETG",
      colorBasis: "Nominal",
      available: true,
    },
    {
      id: "pla-white",
      source: "user",
      name: "Workshop PLA White",
      colorName: "White",
      hex: "#F7F7F5",
      material: "PLA",
      colorBasis: "Nominal",
      available: true,
    },
    {
      id: "out-of-stock-black",
      source: "user",
      name: "Empty Black PETG",
      colorName: "Black",
      hex: "#111111",
      material: "PETG",
      colorBasis: "Nominal",
      available: false,
    },
  ];

  it("lists in-stock spools before the add-new flow and requires cross-material consent", () => {
    const onChooseExistingSpool = vi.fn();
    render(
      <ColorResolutionPanel
        resolutions={[resolution]}
        spools={spools}
        onAcceptColor={vi.fn()}
        onAcceptMaterialSubstitution={vi.fn()}
        onAcceptAllSameMaterial={vi.fn()}
        onChooseExistingSpool={onChooseExistingSpool}
        directUnavailableReason={() => null}
        onAddSpool={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Choose / add spool" }));
    expect(screen.getByText("Choose an in-stock spool first")).toBeInTheDocument();
    expect(
      screen.getByRole("radio", { name: /Grey.*Workshop PETG Grey/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("radio", { name: /White.*Workshop PLA White/i }),
    ).toBeInTheDocument();
    expect(screen.queryByText("Empty Black PETG")).not.toBeInTheDocument();
    expect(screen.getByText("Need a different spool?")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("radio", { name: /White.*Workshop PLA White/i }));
    expect(screen.getAllByText("Material change: PETG → PLA")).toHaveLength(2);
    fireEvent.click(screen.getByRole("button", { name: "Use selected spool" }));
    expect(onChooseExistingSpool).not.toHaveBeenCalled();

    const riskCheckboxes = screen.getAllByRole("checkbox", {
      name: "I understand the mechanical-property risk.",
    });
    fireEvent.click(riskCheckboxes.at(-1)!);
    fireEvent.click(screen.getByRole("button", { name: "Use selected spool" }));
    expect(onChooseExistingSpool).toHaveBeenCalledWith(
      resolution,
      "pla-white",
      true,
    );
  });

  it("uses a same-material spool without a material substitution approval", () => {
    const onChooseExistingSpool = vi.fn();
    render(
      <ColorResolutionPanel
        resolutions={[resolution]}
        spools={spools}
        onAcceptColor={vi.fn()}
        onAcceptMaterialSubstitution={vi.fn()}
        onAcceptAllSameMaterial={vi.fn()}
        onChooseExistingSpool={onChooseExistingSpool}
        directUnavailableReason={() => null}
        onAddSpool={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Choose / add spool" }));
    fireEvent.click(screen.getByRole("radio", { name: /Grey.*Workshop PETG Grey/i }));
    fireEvent.click(screen.getByRole("button", { name: "Use selected spool" }));
    expect(onChooseExistingSpool).toHaveBeenCalledWith(
      resolution,
      "petg-grey",
      false,
    );
  });
});
