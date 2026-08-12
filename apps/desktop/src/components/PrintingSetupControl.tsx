import { useEffect, useRef, useState } from "react";
import { CheckCircle2, ChevronDown, Printer, Settings2 } from "lucide-react";

import type { PrintingSetup } from "../services/printing-setup";

interface PrintingSetupControlProps {
  setup: PrintingSetup | null;
  persistenceStatus?: "none" | "persisted" | "session";
  isOpen: boolean;
  onOpenChange: (open: boolean) => void;
  onSave: (setup: PrintingSetup) => void;
}

export function PrintingSetupControl({
  setup,
  persistenceStatus = setup ? "persisted" : "none",
  isOpen,
  onOpenChange,
  onSave,
}: PrintingSetupControlProps) {
  const [a1MiniAvailable, setA1MiniAvailable] = useState(
    setup?.secondaryPrinter === "a1-mini",
  );
  const summaryRef = useRef<HTMLElement>(null);

  useEffect(() => {
    if (isOpen) {
      setA1MiniAvailable(setup?.secondaryPrinter === "a1-mini");
    }
  }, [isOpen, setup]);

  const printerCount = setup?.secondaryPrinter === "a1-mini" ? 2 : 1;
  const setupSummary = setup
    ? `${printerCount} ${printerCount === 1 ? "printer" : "printers"} available`
    : "Review the printers you physically have";
  const statusLabel =
    persistenceStatus === "persisted"
      ? "Saved"
      : persistenceStatus === "session"
        ? "Available this session"
        : "Review once";

  return (
    <details
      id="printing-setup"
      className="printing-setup"
      open={isOpen}
      onToggle={(event) => {
        if (event.currentTarget.open !== isOpen) {
          onOpenChange(event.currentTarget.open);
        }
      }}
    >
      <summary ref={summaryRef}>
        <span className="printing-setup__summary-icon" aria-hidden="true">
          <Settings2 />
        </span>
        <span className="printing-setup__summary-copy">
          <strong>Printing setup</strong>
          <small>{setupSummary}</small>
        </span>
        <span
          className={
            persistenceStatus === "persisted"
              ? "printing-setup__status is-saved"
              : persistenceStatus === "session"
                ? "printing-setup__status is-session"
                : "printing-setup__status"
          }
        >
          {statusLabel}
        </span>
        <ChevronDown className="setup-disclosure-chevron" aria-hidden="true" />
      </summary>

      <form
        className="printing-setup__form"
        onSubmit={(event) => {
          event.preventDefault();
          onSave({
            schemaVersion: 1,
            primaryPrinter: "u1",
            secondaryPrinter: a1MiniAvailable ? "a1-mini" : null,
          });
          onOpenChange(false);
          window.requestAnimationFrame(() => summaryRef.current?.focus());
        }}
      >
        <div className="printing-setup__intro">
          <div>
            <p className="eyebrow">Available equipment</p>
            <h3>Which printers can this app plan for?</h3>
          </div>
          <p>
            Save the hardware you physically have. Project setup separately
            chooses which printers and U1 method to use before 3MF analysis.
          </p>
        </div>

        <fieldset className="printing-setup__printers">
          <legend>Available printers</legend>
          <div className="printing-setup__primary">
            <span aria-hidden="true">
              <CheckCircle2 />
            </span>
            <span>
              <strong>Snapmaker U1</strong>
              <small>Primary printer · Always included</small>
            </span>
          </div>
          <label className="printing-setup__secondary">
            <input
              type="checkbox"
              checked={a1MiniAvailable}
              onChange={(event) => setA1MiniAvailable(event.target.checked)}
            />
            <span className="printing-setup__secondary-icon" aria-hidden="true">
              <Printer />
            </span>
            <span>
              <strong>Bambu Lab A1 mini</strong>
              <small>I have this optional second printer available</small>
            </span>
          </label>
        </fieldset>

        <div className="printing-setup__actions">
          <button className="button button--dark" type="submit">
            Save printing setup
          </button>
          {setup ? (
            <button
              className="button button--secondary"
              type="button"
              onClick={() => {
                setA1MiniAvailable(setup.secondaryPrinter === "a1-mini");
                onOpenChange(false);
                window.requestAnimationFrame(() => summaryRef.current?.focus());
              }}
            >
              Cancel
            </button>
          ) : null}
        </div>
      </form>
    </details>
  );
}
