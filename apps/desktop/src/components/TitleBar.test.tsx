// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { TitleBar } from "./TitleBar";

afterEach(cleanup);

describe("TitleBar", () => {
  it("uses the native window controls and keeps the custom title bar draggable", () => {
    const { container } = render(
      <TitleBar
        onOpenFilamentLibrary={vi.fn()}
        onOpenColorCalibration={vi.fn()}
      />,
    );

    expect(container.querySelector(".window-controls")).not.toBeInTheDocument();
    expect(container.querySelector(".title-bar")).toHaveAttribute(
      "data-tauri-drag-region",
    );
    expect(screen.getByText("U1 3MF Color Planner")).toHaveAttribute(
      "data-tauri-drag-region",
    );
  });

  it("opens real help content and restores focus when it closes", () => {
    render(
      <TitleBar
        onOpenFilamentLibrary={vi.fn()}
        onOpenColorCalibration={vi.fn()}
      />,
    );

    const trigger = screen.getByRole("button", { name: "Open help" });
    trigger.focus();
    fireEvent.click(trigger);

    expect(
      screen.getByRole("heading", { name: "Plan, convert, and print" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Color accuracy boundary")).toBeInTheDocument();
    expect(screen.getByText(/Export JSON Plan saves a report only/i)).toBeInTheDocument();
    expect(screen.getByText(/after the previous job finishes/i)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Close help dialog" }));

    expect(
      screen.queryByRole("heading", { name: "Plan, convert, and print" }),
    ).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it("routes settings actions to the corresponding application views", () => {
    const onOpenFilamentLibrary = vi.fn();
    const onOpenColorCalibration = vi.fn();
    render(
      <TitleBar
        onOpenFilamentLibrary={onOpenFilamentLibrary}
        onOpenColorCalibration={onOpenColorCalibration}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Open settings" }));
    expect(screen.getByRole("heading", { name: "Settings" })).toBeInTheDocument();
    expect(screen.getByText("Snapmaker U1")).toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", { name: "Open Filament Library" }),
    );
    expect(onOpenFilamentLibrary).toHaveBeenCalledOnce();
    expect(
      screen.queryByRole("heading", { name: "Settings" }),
    ).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Open settings" }));
    fireEvent.click(
      screen.getByRole("button", { name: "Open Color Calibration" }),
    );
    expect(onOpenColorCalibration).toHaveBeenCalledOnce();
  });
});
