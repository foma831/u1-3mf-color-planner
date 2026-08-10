// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ExcludedSourceUnit } from "../types";
import { PartialConversionNotice } from "./PartialConversionNotice";

const exclusions: ExcludedSourceUnit[] = [
  {
    scopeId: "plate-8",
    planningUnitId: "plate-8-unit-2",
    sourcePlateId: 8,
    sourceUnitId: "model-42",
    reason: "No schedulable physical loadout is available.",
    errorIdentity: "partial-error-v1:abc",
  },
  {
    scopeId: "unbound",
    planningUnitId: "unbound-unit",
    sourcePlateId: null,
    sourceUnitId: "model-99",
    reason: "The source unit cannot be assigned to a target plate.",
    errorIdentity: "partial-error-v1:def",
  },
];

afterEach(cleanup);

describe("PartialConversionNotice", () => {
  it("lists exact exclusions and requires an explicit available approval", () => {
    const onAcknowledgedChange = vi.fn();
    render(
      <PartialConversionNotice
        available
        exclusions={exclusions}
        reason="Two units are safely isolated from valid jobs."
        acknowledged={false}
        onAcknowledgedChange={onAcknowledgedChange}
      />,
    );

    expect(
      screen.getByRole("heading", { name: "Partial conversion available" }),
    ).toBeInTheDocument();
    expect(screen.getByText("model-42")).toBeInTheDocument();
    expect(screen.getByText("Source Plate 08")).toBeInTheDocument();
    expect(screen.getByText("Source plate not identified")).toBeInTheDocument();
    expect(
      screen.getByText("No schedulable physical loadout is available."),
    ).toBeInTheDocument();

    const checkbox = screen.getByRole("checkbox", {
      name: /I understand that these source units will be excluded/i,
    });
    expect(checkbox).toBeEnabled();
    fireEvent.click(checkbox);
    expect(onAcknowledgedChange).toHaveBeenCalledWith(true);
  });

  it("keeps approval disabled when the backend says partial conversion is unsafe", () => {
    render(
      <PartialConversionNotice
        available={false}
        exclusions={exclusions}
        reason="A project-level structural error cannot be excluded."
        acknowledged={false}
        onAcknowledgedChange={vi.fn()}
      />,
    );

    expect(
      screen.getByRole("checkbox", {
        name: /I understand that these source units will be excluded/i,
      }),
    ).toBeDisabled();
    expect(
      screen.getByText("A project-level structural error cannot be excluded."),
    ).toBeInTheDocument();
  });
});
