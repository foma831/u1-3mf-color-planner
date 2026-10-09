// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { CmyxReference } from "./CmyxReference";

afterEach(cleanup);

describe("CmyxReference", () => {
  it("shows the complete CMY reference by default", () => {
    render(<CmyxReference />);

    expect(
      screen.getByRole("heading", { name: "Full Spectrum Color Reference" }),
    ).toBeVisible();
    expect(
      screen.getByText("Showing 46 of 46 unique CMY colors."),
    ).toBeVisible();
    expect(screen.getAllByRole("row")).toHaveLength(47);
  });

  it("switches to the theoretical CMYK palette and explains its K requirement", () => {
    render(<CmyxReference />);

    fireEvent.click(screen.getByRole("radio", { name: /CMYK Full Spectrum/i }));

    expect(
      screen.getByText("Showing 135 of 135 unique CMYK colors."),
    ).toBeVisible();
    expect(screen.getByText(/opaque Black remains solid-only/i)).toBeVisible();
  });

  it("filters by HEX and recipe metadata", () => {
    render(<CmyxReference />);
    const table = screen.getByRole("region", {
      name: "CMY Full Spectrum color table",
    });

    fireEvent.change(screen.getByRole("searchbox", { name: "Search colors" }), {
      target: { value: "#08ABFB" },
    });

    expect(within(table).getAllByRole("row")).toHaveLength(2);
    expect(within(table).getByRole("rowheader", { name: "#08ABFB" })).toBeVisible();
  });
});
