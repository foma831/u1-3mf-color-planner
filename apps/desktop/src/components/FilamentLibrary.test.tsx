// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";

import { useState, type ComponentProps } from "react";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { PhysicalSpool } from "../types";
import { createDemoPlan } from "../data/mock-plan";
import { DirectSpoolEditor } from "./DirectSpoolEditor";
import { FilamentLibrary } from "./FilamentLibrary";

const librarySpools: PhysicalSpool[] = [
  {
    id: "panchroma-cyan",
    source: "built-in",
    name: "Panchroma Translucent Cyan",
    colorName: "Cyan",
    hex: "#08ABFB",
    material: "PLA",
    sku: "PM-CY-001",
    colorBasis: "Nominal",
    available: true,
  },
  {
    id: "black-petg",
    source: "built-in",
    name: "PolyLite PETG Black",
    colorName: "Black",
    hex: "#202224",
    material: "PETG",
    colorBasis: "Measured",
    available: false,
  },
  {
    id: "user-workshop-red",
    source: "user",
    name: "Workshop Signal Red",
    colorName: "Signal Red",
    hex: "#C72E2A",
    material: "PLA",
    profile: "0.20 mm Workshop PLA",
    vendor: "Polymaker",
    productLine: "Panchroma",
    opticalDescriptor: "Opaque matte",
    minNozzleTemperatureC: 200,
    maxNozzleTemperatureC: 230,
    batchLot: "LOT-RED-7",
    calibrationReference: "flat-red-v1",
    notes: "Keep dry before color calibration.",
    colorBasis: "Nominal",
    available: true,
  },
];

function renderLibrary(
  overrides: Partial<ComponentProps<typeof FilamentLibrary>> = {},
) {
  const props: ComponentProps<typeof FilamentLibrary> = {
    spools: librarySpools,
    onAddSpool: vi.fn(),
    onUpdateSpool: vi.fn(),
    onAvailabilityChange: vi.fn(),
    onDeleteSpool: vi.fn().mockResolvedValue(true),
    ...overrides,
  };
  render(<FilamentLibrary {...props} />);
  return props;
}

afterEach(cleanup);

describe("FilamentLibrary", () => {
  it("keeps the permanent catalogue visible with textual stock status and filters", () => {
    const { onAvailabilityChange } = renderLibrary();

    expect(
      screen.getByRole("heading", { name: "Filament Library" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("table", {
        name: "Showing 3 of 3 catalogued spools.",
      }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("row", {
        name: /PolyLite PETG Black.*PETG.*Out of stock/i,
      }),
    ).toBeInTheDocument();
    expect(
      screen.getAllByText("In stock", { selector: ".filament-status" }),
    ).toHaveLength(2);
    expect(screen.getByText("Polymaker · Panchroma")).toBeInTheDocument();
    expect(screen.getByText("Optical: Opaque matte")).toBeInTheDocument();
    expect(screen.getByText("Nozzle 200–230 °C")).toBeInTheDocument();
    expect(screen.getByText("Lot LOT-RED-7 · Calibration flat-red-v1")).toBeInTheDocument();
    expect(screen.getByText("Notes")).toBeInTheDocument();
    expect(
      screen.getByText("Out of stock", { selector: ".filament-status" }),
    ).toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", {
        name: "Mark in stock: PolyLite PETG Black",
      }),
    );
    expect(onAvailabilityChange).toHaveBeenCalledWith("black-petg", true);

    fireEvent.change(screen.getByLabelText("Availability"), {
      target: { value: "out-of-stock" },
    });
    expect(
      screen.getByRole("table", {
        name: "Showing 1 of 3 catalogued spools.",
      }),
    ).toBeInTheDocument();
    expect(screen.queryByText("Panchroma Translucent Cyan")).not.toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("Search library"), {
      target: { value: "does not exist" },
    });
    expect(screen.getByText(/No spools match these filters/i)).toBeInTheDocument();
  });

  it("edits and deletes user spools while keeping built-ins read-only", async () => {
    const onUpdateSpool = vi.fn();
    const onDeleteSpool = vi.fn().mockResolvedValue(true);
    renderLibrary({ onUpdateSpool, onDeleteSpool });

    expect(
      screen.queryByRole("button", {
        name: "Edit Panchroma Translucent Cyan",
      }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", {
        name: "Delete Panchroma Translucent Cyan",
      }),
    ).not.toBeInTheDocument();
    expect(
      screen.getAllByText(/Built-in details cannot be edited or deleted/i),
    ).toHaveLength(2);

    fireEvent.click(
      screen.getByRole("button", { name: "Edit Workshop Signal Red" }),
    );
    const editHeading = screen.getByRole("heading", {
      name: "Edit Workshop Signal Red",
    });
    const editRegion = editHeading.closest("section");
    expect(editRegion).not.toBeNull();
    const editForm = within(editRegion!);
    fireEvent.change(editForm.getByLabelText(/Spool name/i), {
      target: { value: "Workshop Deep Red" },
    });
    fireEvent.click(editForm.getByRole("button", { name: "Save changes" }));
    expect(onUpdateSpool).toHaveBeenCalledWith(
      "user-workshop-red",
      expect.objectContaining({
        name: "Workshop Deep Red",
        colorName: "Signal Red",
      }),
    );

    fireEvent.click(
      screen.getByRole("button", { name: "Delete Workshop Signal Red" }),
    );
    expect(
      screen.getByRole("heading", { name: "Delete user spool?" }),
    ).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", {
        name: "Confirm delete Workshop Signal Red",
      }),
    );
    await waitFor(() =>
      expect(onDeleteSpool).toHaveBeenCalledWith("user-workshop-red"),
    );
  });

  it("moves focus into delete confirmation and restores it on cancel", () => {
    renderLibrary();

    const deleteButton = screen.getByRole("button", {
      name: "Delete Workshop Signal Red",
    });
    deleteButton.focus();
    fireEvent.click(deleteButton);

    const cancelButton = screen.getByRole("button", {
      name: "Cancel delete Workshop Signal Red",
    });
    expect(cancelButton).toHaveFocus();

    fireEvent.click(cancelButton);
    expect(
      screen.getByRole("button", {
        name: "Delete Workshop Signal Red",
      }),
    ).toHaveFocus();
  });

  it("moves focus to the next delete action, then the previous one, then the library heading", async () => {
    const userSpools: PhysicalSpool[] = [
      {
        ...librarySpools[2],
        id: "user-alpha",
        name: "Alpha Red",
      },
      librarySpools[2],
      {
        ...librarySpools[2],
        id: "user-zulu",
        name: "Zulu Red",
      },
    ];

    function DeletingLibrary() {
      const [spools, setSpools] = useState([
        ...librarySpools.slice(0, 2),
        ...userSpools,
      ]);
      return (
        <FilamentLibrary
          spools={spools}
          onAddSpool={vi.fn()}
          onUpdateSpool={vi.fn()}
          onAvailabilityChange={vi.fn()}
          onDeleteSpool={async (spoolId) => {
            setSpools((current) =>
              current.filter((spool) => spool.id !== spoolId),
            );
            return true;
          }}
        />
      );
    }

    render(<DeletingLibrary />);

    fireEvent.click(
      screen.getByRole("button", { name: "Delete Workshop Signal Red" }),
    );
    fireEvent.click(
      screen.getByRole("button", {
        name: "Confirm delete Workshop Signal Red",
      }),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Delete Zulu Red" }),
      ).toHaveFocus(),
    );

    fireEvent.click(screen.getByRole("button", { name: "Delete Zulu Red" }));
    fireEvent.click(
      screen.getByRole("button", { name: "Confirm delete Zulu Red" }),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Delete Alpha Red" }),
      ).toHaveFocus(),
    );

    fireEvent.click(screen.getByRole("button", { name: "Delete Alpha Red" }));
    fireEvent.click(
      screen.getByRole("button", { name: "Confirm delete Alpha Red" }),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("heading", { name: "Filament Library" }),
      ).toHaveFocus(),
    );
  });

  it("keeps the spool and confirmation open when persistent deletion fails", async () => {
    renderLibrary({ onDeleteSpool: vi.fn().mockResolvedValue(false) });

    fireEvent.click(
      screen.getByRole("button", { name: "Delete Workshop Signal Red" }),
    );
    fireEvent.click(
      screen.getByRole("button", {
        name: "Confirm delete Workshop Signal Red",
      }),
    );

    expect(
      await screen.findByText(
        "The spool was not deleted. Review the storage error and try again.",
      ),
    ).toHaveAttribute("role", "alert");
    expect(screen.getAllByText("Workshop Signal Red").length).toBeGreaterThan(0);
    expect(
      screen.getByRole("button", {
        name: "Confirm delete Workshop Signal Red",
      }),
    ).toBeEnabled();
  });

  it("adds a user spool through the shared accessible spool form", () => {
    const onAddSpool = vi.fn();
    renderLibrary({ onAddSpool });

    fireEvent.click(screen.getByText("Add spool to library"));
    const addHeading = screen.getByRole("heading", { name: "Add a user spool" });
    const addRegion = addHeading.closest("section");
    expect(addRegion).not.toBeNull();
    const addForm = within(addRegion!);
    fireEvent.change(addForm.getByLabelText(/Spool name/i), {
      target: { value: "Workshop White" },
    });
    fireEvent.change(addForm.getByLabelText(/Color name/i), {
      target: { value: "White" },
    });
    fireEvent.change(addForm.getByLabelText(/HEX color/i), {
      target: { value: "#F4F4F2" },
    });
    fireEvent.click(addForm.getByRole("button", { name: "Add spool" }));

    expect(onAddSpool).toHaveBeenCalledWith({
      name: "Workshop White",
      colorName: "White",
      hex: "#F4F4F2",
      material: "PLA",
      sku: undefined,
      profile: undefined,
      vendor: undefined,
      productLine: undefined,
      opticalDescriptor: undefined,
      minNozzleTemperatureC: undefined,
      maxNozzleTemperatureC: undefined,
      batchLot: undefined,
      calibrationReference: undefined,
      notes: undefined,
    });
  });

  it("accepts a valid HEX value after an earlier invalid attempt", () => {
    const onAddSpool = vi.fn();
    renderLibrary({ onAddSpool });

    fireEvent.click(screen.getByText("Add spool to library"));
    const addHeading = screen.getByRole("heading", { name: "Add a user spool" });
    const addRegion = addHeading.closest("section");
    expect(addRegion).not.toBeNull();
    const addForm = within(addRegion!);
    fireEvent.change(addForm.getByLabelText(/Spool name/i), {
      target: { value: "A1 Black" },
    });
    fireEvent.change(addForm.getByLabelText(/Color name/i), {
      target: { value: "Black" },
    });

    const hexInput = addForm.getByLabelText(/HEX color/i);
    fireEvent.invalid(hexInput);
    expect(addForm.getByRole("alert")).toHaveTextContent(
      "Enter a six-digit HEX color such as #C72E2A.",
    );

    fireEvent.change(addForm.getByLabelText("Color"), {
      target: { value: "#010101" },
    });
    expect(hexInput).toHaveValue("#010101");
    expect(hexInput).toBeValid();
    expect(addForm.queryByRole("alert")).not.toBeInTheDocument();

    fireEvent.click(addForm.getByRole("button", { name: "Add spool" }));
    expect(onAddSpool).toHaveBeenCalledWith({
      name: "A1 Black",
      colorName: "Black",
      hex: "#010101",
      material: "PLA",
      sku: undefined,
      profile: undefined,
      vendor: undefined,
      productLine: undefined,
      opticalDescriptor: undefined,
      minNozzleTemperatureC: undefined,
      maxNozzleTemperatureC: undefined,
      batchLot: undefined,
      calibrationReference: undefined,
      notes: undefined,
    });
  });

  it("captures production metadata and blocks an inverted nozzle range", () => {
    const onAddSpool = vi.fn();
    renderLibrary({ onAddSpool });

    fireEvent.click(screen.getByText("Add spool to library"));
    const addRegion = screen
      .getByRole("heading", { name: "Add a user spool" })
      .closest("section");
    expect(addRegion).not.toBeNull();
    const form = within(addRegion!);
    fireEvent.change(form.getByLabelText(/Spool name/i), {
      target: { value: "Panchroma Grey Lot 9" },
    });
    fireEvent.change(form.getByLabelText(/Color name/i), {
      target: { value: "Grey" },
    });
    fireEvent.change(form.getByLabelText(/HEX color/i), {
      target: { value: "#9199A4" },
    });
    fireEvent.change(form.getByLabelText("Vendor (optional)"), {
      target: { value: " Polymaker " },
    });
    fireEvent.change(form.getByLabelText("Product line (optional)"), {
      target: { value: " Panchroma " },
    });
    fireEvent.change(
      form.getByLabelText("Optical / translucency descriptor (optional)"),
      { target: { value: " Translucent grey " } },
    );
    fireEvent.change(
      form.getByLabelText("Minimum nozzle temperature °C (optional)"),
      { target: { value: "240" } },
    );
    const maximumTemperature = form.getByLabelText(
      "Maximum nozzle temperature °C (optional)",
    );
    fireEvent.change(maximumTemperature, { target: { value: "220" } });
    fireEvent.change(form.getByLabelText("Batch / lot (optional)"), {
      target: { value: " LOT-9 " },
    });
    fireEvent.change(
      form.getByLabelText("Calibration set / reference (optional)"),
      { target: { value: " flat-grey-v2 " } },
    );
    fireEvent.change(form.getByLabelText("Notes (optional)"), {
      target: { value: " Dry at 55 C. " },
    });

    fireEvent.click(form.getByRole("button", { name: "Add spool" }));
    expect(onAddSpool).not.toHaveBeenCalled();
    expect(form.getByRole("alert")).toHaveTextContent(
      "Maximum nozzle temperature must be greater than or equal to the minimum.",
    );
    expect(maximumTemperature).toHaveFocus();

    fireEvent.change(maximumTemperature, { target: { value: "250" } });
    fireEvent.click(form.getByRole("button", { name: "Add spool" }));
    expect(onAddSpool).toHaveBeenCalledWith(
      expect.objectContaining({
        vendor: "Polymaker",
        productLine: "Panchroma",
        opticalDescriptor: "Translucent grey",
        minNozzleTemperatureC: 240,
        maxNozzleTemperatureC: 250,
        batchLot: "LOT-9",
        calibrationReference: "flat-grey-v2",
        notes: "Dry at 55 C.",
      }),
    );
  });
});

describe("stock-aware spool selectors", () => {
  it("does not offer an out-of-stock spool for a Direct mapping", () => {
    const plan = createDemoPlan();
    plan.spools = plan.spools.map((spool) =>
      spool.id === "royal-purple" ? { ...spool, available: false } : spool,
    );

    render(
      <DirectSpoolEditor
        plateId="stock-aware"
        mappings={[plan.plates[0].mappings![1]]}
        spools={plan.spools}
        currentLoadout={plan.currentLoadout}
        restoreCmy={false}
        onRestoreChange={vi.fn()}
        onToolheadChange={vi.fn()}
        onSpoolChange={vi.fn()}
        onMaterialSubstitutionChange={vi.fn()}
        onAddSpool={vi.fn()}
      />,
    );

    const selector = screen.getByLabelText("Selected spool");
    expect(selector).toHaveValue("");
    expect(
      within(selector).queryByRole("option", {
        name: /Royal Purple/i,
      }),
    ).not.toBeInTheDocument();
    expect(screen.getByText("Review Out of stock")).toBeInTheDocument();
  });
});
