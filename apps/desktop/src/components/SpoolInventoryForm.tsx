import { useRef, useState, type FormEvent } from "react";
import { Plus } from "lucide-react";

import type { NewPhysicalSpoolInput } from "../types";

interface SpoolInventoryFormProps {
  idPrefix: string;
  spoolCount: number;
  onAddSpool: (spool: NewPhysicalSpoolInput) => void;
  heading?: string;
  description?: string;
  summaryLabel?: string;
  initialValues?: Partial<NewPhysicalSpoolInput>;
  initiallyOpen?: boolean;
  submitLabel?: string;
  onCancel?: () => void;
}

const initialDraft: NewPhysicalSpoolInput = {
  name: "",
  colorName: "",
  hex: "",
  material: "PLA",
  sku: "",
  profile: "",
  vendor: "",
  productLine: "",
  opticalDescriptor: "",
  minNozzleTemperatureC: undefined,
  maxNozzleTemperatureC: undefined,
  batchLot: "",
  calibrationReference: "",
  notes: "",
};

const validHex = /^#[0-9A-F]{6}$/;

function RequiredMark() {
  return (
    <>
      <span className="required-mark" aria-hidden="true">
        *
      </span>
      <span className="visually-hidden"> required</span>
    </>
  );
}

export function SpoolInventoryForm({
  idPrefix,
  spoolCount,
  onAddSpool,
  heading = "Physical spool inventory",
  description =
    "This scope has four or fewer mappings. Assign a spool to each source color; choose the same spool more than once to intentionally combine colors on one toolhead.",
  summaryLabel = "Add physical spool",
  initialValues,
  initiallyOpen = false,
  submitLabel = "Add spool",
  onCancel,
}: SpoolInventoryFormProps) {
  const seededDraft = { ...initialDraft, ...initialValues };
  const [draft, setDraft] = useState(seededDraft);
  const [hexError, setHexError] = useState<string | null>(null);
  const [temperatureError, setTemperatureError] = useState<string | null>(null);
  const maximumTemperatureRef = useRef<HTMLInputElement | null>(null);
  const pickerValue = validHex.test(draft.hex) ? draft.hex : "#808080";

  const updateDraft = <Key extends keyof NewPhysicalSpoolInput>(
    key: Key,
    value: NewPhysicalSpoolInput[Key],
  ) => setDraft((current) => ({ ...current, [key]: value }));

  const submitSpool = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!draft.name.trim() || !draft.colorName.trim() || !validHex.test(draft.hex)) {
      return;
    }
    if (
      draft.minNozzleTemperatureC !== undefined &&
      draft.maxNozzleTemperatureC !== undefined &&
      draft.minNozzleTemperatureC > draft.maxNozzleTemperatureC
    ) {
      setTemperatureError(
        "Maximum nozzle temperature must be greater than or equal to the minimum.",
      );
      maximumTemperatureRef.current?.focus();
      return;
    }

    const optionalValue = (value?: string) => value?.trim() || undefined;

    onAddSpool({
      name: draft.name.trim(),
      colorName: draft.colorName.trim(),
      hex: draft.hex,
      material: draft.material,
      sku: optionalValue(draft.sku),
      profile: optionalValue(draft.profile),
      vendor: optionalValue(draft.vendor),
      productLine: optionalValue(draft.productLine),
      opticalDescriptor: optionalValue(draft.opticalDescriptor),
      minNozzleTemperatureC: draft.minNozzleTemperatureC,
      maxNozzleTemperatureC: draft.maxNozzleTemperatureC,
      batchLot: optionalValue(draft.batchLot),
      calibrationReference: optionalValue(draft.calibrationReference),
      notes: optionalValue(draft.notes),
    });
    setDraft(seededDraft);
    setHexError(null);
    setTemperatureError(null);
  };

  const nameId = `${idPrefix}-spool-name`;
  const colorNameId = `${idPrefix}-color-name`;
  const pickerId = `${idPrefix}-color-picker`;
  const hexId = `${idPrefix}-hex-color`;
  const hexHelpId = `${idPrefix}-hex-help`;
  const hexErrorId = `${idPrefix}-hex-error`;
  const skuId = `${idPrefix}-sku`;
  const profileId = `${idPrefix}-profile`;
  const vendorId = `${idPrefix}-vendor`;
  const productLineId = `${idPrefix}-product-line`;
  const opticalDescriptorId = `${idPrefix}-optical-descriptor`;
  const minimumTemperatureId = `${idPrefix}-minimum-nozzle-temperature`;
  const maximumTemperatureId = `${idPrefix}-maximum-nozzle-temperature`;
  const temperatureHelpId = `${idPrefix}-temperature-help`;
  const temperatureErrorId = `${idPrefix}-temperature-error`;
  const batchLotId = `${idPrefix}-batch-lot`;
  const calibrationReferenceId = `${idPrefix}-calibration-reference`;
  const calibrationIdentityHelpId = `${idPrefix}-calibration-identity-help`;
  const notesId = `${idPrefix}-notes`;

  return (
    <section className="spool-inventory" aria-labelledby={`${idPrefix}-inventory-heading`}>
      <div className="spool-inventory__heading">
        <div>
          <h3 id={`${idPrefix}-inventory-heading`}>{heading}</h3>
          <p>{description}</p>
        </div>
        <span>{spoolCount} available</span>
      </div>
      <details className="spool-inventory__details" open={initiallyOpen || undefined}>
        <summary>
          <Plus aria-hidden="true" />
          {summaryLabel}
        </summary>
        <form onSubmit={submitSpool}>
          <fieldset>
            <legend>Spool details</legend>
            <div className="spool-form-grid">
              <div className="field spool-form-field">
                <label htmlFor={nameId}>
                  Spool name
                  <RequiredMark />
                </label>
                <input
                  id={nameId}
                  name="spool-name"
                  type="text"
                  value={draft.name}
                  maxLength={80}
                  pattern=".*\S.*"
                  required
                  onChange={(event) => updateDraft("name", event.target.value)}
                />
              </div>
              <div className="field spool-form-field">
                <label htmlFor={colorNameId}>
                  Color name
                  <RequiredMark />
                </label>
                <input
                  id={colorNameId}
                  name="color-name"
                  type="text"
                  value={draft.colorName}
                  maxLength={60}
                  pattern=".*\S.*"
                  required
                  onChange={(event) => updateDraft("colorName", event.target.value)}
                />
              </div>
              <div className="spool-color-fields">
                <div className="field spool-form-field spool-picker-field">
                  <label htmlFor={pickerId}>Color</label>
                  <input
                    id={pickerId}
                    name="color-picker"
                    type="color"
                    value={pickerValue}
                    onChange={(event) => {
                      setHexError(null);
                      updateDraft("hex", event.target.value.toUpperCase());
                    }}
                  />
                </div>
                <div className="field spool-form-field spool-hex-field">
                  <label htmlFor={hexId}>
                    HEX color
                    <RequiredMark />
                  </label>
                  <input
                    id={hexId}
                    name="hex-color"
                    type="text"
                    value={draft.hex}
                    placeholder="#C72E2A"
                    inputMode="text"
                    maxLength={7}
                    pattern="#[0-9A-Fa-f]{6}"
                    aria-describedby={`${hexHelpId}${hexError ? ` ${hexErrorId}` : ""}`}
                    aria-errormessage={hexError ? hexErrorId : undefined}
                    aria-invalid={hexError ? "true" : undefined}
                    required
                    onInvalid={() =>
                      setHexError("Enter a six-digit HEX color such as #C72E2A.")
                    }
                    onChange={(event) => {
                      setHexError(null);
                      updateDraft("hex", event.target.value.toUpperCase());
                    }}
                  />
                  <small id={hexHelpId}>Six-digit HEX, including #.</small>
                  {hexError ? (
                    <small className="field-error" id={hexErrorId} role="alert">
                      {hexError}
                    </small>
                  ) : null}
                </div>
              </div>
              <fieldset className="material-choice">
                <legend>
                  Material
                  <RequiredMark />
                </legend>
                {(["PLA", "PETG"] as const).map((material) => {
                  const materialId = `${idPrefix}-material-${material.toLowerCase()}`;
                  return (
                    <label htmlFor={materialId} key={material}>
                      <input
                        id={materialId}
                        name={`${idPrefix}-material`}
                        type="radio"
                        value={material}
                        checked={draft.material === material}
                        required
                        onChange={() => updateDraft("material", material)}
                      />
                      {material}
                    </label>
                  );
                })}
              </fieldset>
              <div className="field spool-form-field">
                <label htmlFor={skuId}>SKU (optional)</label>
                <input
                  id={skuId}
                  name="sku"
                  type="text"
                  value={draft.sku}
                  maxLength={60}
                  onChange={(event) => updateDraft("sku", event.target.value)}
                />
              </div>
              <div className="field spool-form-field">
                <label htmlFor={profileId}>Profile (optional)</label>
                <input
                  id={profileId}
                  name="profile"
                  type="text"
                  value={draft.profile}
                  maxLength={100}
                  onChange={(event) => updateDraft("profile", event.target.value)}
                />
              </div>
              <div className="field spool-form-field">
                <label htmlFor={vendorId}>Vendor (optional)</label>
                <input
                  id={vendorId}
                  name="vendor"
                  type="text"
                  value={draft.vendor}
                  maxLength={120}
                  onChange={(event) => updateDraft("vendor", event.target.value)}
                />
              </div>
              <div className="field spool-form-field">
                <label htmlFor={productLineId}>Product line (optional)</label>
                <input
                  id={productLineId}
                  name="product-line"
                  type="text"
                  value={draft.productLine}
                  maxLength={120}
                  onChange={(event) =>
                    updateDraft("productLine", event.target.value)
                  }
                />
              </div>
              <div className="field spool-form-field spool-form-field--wide">
                <label htmlFor={opticalDescriptorId}>
                  Optical / translucency descriptor (optional)
                </label>
                <input
                  id={opticalDescriptorId}
                  name="optical-descriptor"
                  type="text"
                  value={draft.opticalDescriptor}
                  maxLength={240}
                  placeholder="For example: translucent, matte, or opaque"
                  onChange={(event) =>
                    updateDraft("opticalDescriptor", event.target.value)
                  }
                />
              </div>
              <div className="field spool-form-field">
                <label htmlFor={minimumTemperatureId}>
                  Minimum nozzle temperature °C (optional)
                </label>
                <input
                  id={minimumTemperatureId}
                  name="minimum-nozzle-temperature"
                  type="number"
                  value={draft.minNozzleTemperatureC ?? ""}
                  min={0}
                  max={500}
                  step={1}
                  inputMode="numeric"
                  aria-describedby={temperatureHelpId}
                  onChange={(event) => {
                    setTemperatureError(null);
                    updateDraft(
                      "minNozzleTemperatureC",
                      event.target.value === ""
                        ? undefined
                        : Number(event.target.value),
                    );
                  }}
                />
              </div>
              <div className="field spool-form-field">
                <label htmlFor={maximumTemperatureId}>
                  Maximum nozzle temperature °C (optional)
                </label>
                <input
                  ref={maximumTemperatureRef}
                  id={maximumTemperatureId}
                  name="maximum-nozzle-temperature"
                  type="number"
                  value={draft.maxNozzleTemperatureC ?? ""}
                  min={0}
                  max={500}
                  step={1}
                  inputMode="numeric"
                  aria-describedby={`${temperatureHelpId}${
                    temperatureError ? ` ${temperatureErrorId}` : ""
                  }`}
                  aria-invalid={temperatureError ? "true" : undefined}
                  onChange={(event) => {
                    setTemperatureError(null);
                    updateDraft(
                      "maxNozzleTemperatureC",
                      event.target.value === ""
                        ? undefined
                        : Number(event.target.value),
                    );
                  }}
                />
              </div>
              <p
                id={temperatureHelpId}
                className="spool-form-hint spool-form-field--wide"
              >
                Store the manufacturer's recommended nozzle range. Slicer
                profiles remain authoritative at conversion time.
              </p>
              {temperatureError ? (
                <p
                  id={temperatureErrorId}
                  className="field-error spool-form-field--wide"
                  role="alert"
                >
                  {temperatureError}
                </p>
              ) : null}
              <div className="field spool-form-field">
                <label htmlFor={batchLotId}>Batch / lot (optional)</label>
                <input
                  id={batchLotId}
                  name="batch-lot"
                  type="text"
                  value={draft.batchLot}
                  maxLength={240}
                  aria-describedby={calibrationIdentityHelpId}
                  onChange={(event) => updateDraft("batchLot", event.target.value)}
                />
              </div>
              <div className="field spool-form-field">
                <label htmlFor={calibrationReferenceId}>
                  Calibration set / reference (optional)
                </label>
                <input
                  id={calibrationReferenceId}
                  name="calibration-reference"
                  type="text"
                  value={draft.calibrationReference}
                  maxLength={240}
                  aria-describedby={calibrationIdentityHelpId}
                  onChange={(event) =>
                    updateDraft("calibrationReference", event.target.value)
                  }
                />
              </div>
              <p
                id={calibrationIdentityHelpId}
                className="spool-form-hint spool-form-field--wide"
              >
                Changing either value creates a new physical calibration
                identity. Previous measured CMY+X samples will not be reused.
              </p>
              <div className="field spool-form-field spool-form-field--wide">
                <label htmlFor={notesId}>Notes (optional)</label>
                <textarea
                  id={notesId}
                  name="notes"
                  value={draft.notes}
                  rows={3}
                  maxLength={2000}
                  onChange={(event) => updateDraft("notes", event.target.value)}
                />
              </div>
            </div>
            <div className="spool-form-actions">
              <button className="button button--primary button--compact" type="submit">
                {submitLabel === "Add spool" ? <Plus aria-hidden="true" /> : null}
                {submitLabel}
              </button>
              {onCancel ? (
                <button
                  className="button button--secondary button--compact"
                  type="button"
                  onClick={onCancel}
                >
                  Cancel
                </button>
              ) : null}
            </div>
          </fieldset>
        </form>
      </details>
    </section>
  );
}
