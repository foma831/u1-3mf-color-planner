import { useEffect, useId, useRef, useState } from "react";
import {
  BookOpenCheck,
  CircleHelp,
  LibraryBig,
  Pipette,
  Printer,
  Settings,
  X,
} from "lucide-react";

interface TitleBarProps {
  onOpenFilamentLibrary: () => void;
  onOpenColorCalibration: () => void;
  onOpenPrintingSetup: () => void;
}

type UtilityDialog = "help" | "settings";

export function TitleBar({
  onOpenFilamentLibrary,
  onOpenColorCalibration,
  onOpenPrintingSetup,
}: TitleBarProps) {
  const instanceId = useId().replace(/:/g, "");
  const dialogRef = useRef<HTMLDialogElement>(null);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const returnFocusRef = useRef<HTMLButtonElement | null>(null);
  const fallbackDialogRef = useRef(false);
  const [activeDialog, setActiveDialog] = useState<UtilityDialog | null>(null);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;

    if (activeDialog && !dialog.open) {
      if (typeof dialog.showModal === "function") {
        fallbackDialogRef.current = false;
        dialog.showModal();
      } else {
        // jsdom and older embedded webviews do not expose showModal().
        fallbackDialogRef.current = true;
        dialog.setAttribute("open", "");
        closeButtonRef.current?.focus();
      }
      return;
    }

    if (!activeDialog && dialog.open) {
      if (typeof dialog.close === "function") dialog.close();
      else dialog.removeAttribute("open");
    }
    if (!activeDialog && fallbackDialogRef.current) {
      returnFocusRef.current?.focus();
      returnFocusRef.current = null;
      fallbackDialogRef.current = false;
    }
  }, [activeDialog]);

  const openDialog = (dialog: UtilityDialog, trigger: HTMLButtonElement) => {
    returnFocusRef.current = trigger;
    setActiveDialog(dialog);
  };

  const finishClose = () => {
    setActiveDialog(null);
    returnFocusRef.current?.focus();
    returnFocusRef.current = null;
  };

  const openDestination = (destination: () => void) => {
    setActiveDialog(null);
    destination();
  };

  const titleId = `${instanceId}-utility-dialog-title`;
  const descriptionId = `${instanceId}-utility-dialog-description`;
  const isHelp = activeDialog === "help";

  return (
    <>
      <header className="title-bar" data-tauri-drag-region>
        <strong className="title-bar__name" data-tauri-drag-region>
          U1 3MF Color Planner
        </strong>
        <div className="title-bar__utilities">
          <button
            className="icon-button"
            type="button"
            aria-label="Open help"
            onClick={(event) => openDialog("help", event.currentTarget)}
          >
            <CircleHelp aria-hidden="true" />
          </button>
          <button
            className="icon-button"
            type="button"
            aria-label="Open application setup"
            onClick={(event) => openDialog("settings", event.currentTarget)}
          >
            <Settings aria-hidden="true" />
          </button>
        </div>
      </header>

      <dialog
        ref={dialogRef}
        className="utility-dialog"
        aria-labelledby={titleId}
        aria-describedby={descriptionId}
        onClose={finishClose}
      >
        {activeDialog ? (
          <div className="utility-dialog__content">
            <div className="utility-dialog__header">
              <span className="utility-dialog__title-icon" aria-hidden="true">
                {isHelp ? <BookOpenCheck /> : <Settings />}
              </span>
              <div>
                <p className="eyebrow">
                  {isHelp ? "Operator guide" : "Setup & readiness"}
                </p>
                <h2 id={titleId}>
                  {isHelp ? "Plan, convert, and print" : "Application setup"}
                </h2>
              </div>
              <button
                ref={closeButtonRef}
                className="icon-button"
                type="button"
                aria-label={`Close ${activeDialog} dialog`}
                autoFocus
                onClick={() => setActiveDialog(null)}
              >
                <X aria-hidden="true" />
              </button>
            </div>

            {isHelp ? (
              <div className="utility-dialog__body">
                <p id={descriptionId}>
                  Convert a Bambu Studio project into qualified native project
                  files for Snapmaker U1 and, when selected, Bambu Lab A1 mini.
                </p>
                <ol className="utility-dialog__steps">
                  <li>
                    <strong>Confirm physical inputs.</strong> Mark only the
                    spools that are available now and keep Available printers
                    up to date.
                  </li>
                  <li>
                    <strong>Choose Project setup.</strong> Select U1-only or U1
                    with A1 mini, then choose Automatic, Direct Spools only, or
                    CMY+X only before the first analysis.
                  </li>
                  <li>
                    <strong>Analyze the source.</strong> Review every target
                    plate, source color, material, and omitted unit. The first
                    plan already uses the Project setup you confirmed.
                  </li>
                  <li>
                    <strong>Resolve and refine.</strong> Approve intentional
                    color or material replacements, adjust eligible per-plate
                    routing when needed, then recalculate the plan.
                  </li>
                  <li>
                    <strong>Review conversion output.</strong> Review conversion
                    opens preflight so you can check every output and warning,
                    choose a destination, then select Convert projects. Export
                    JSON plan saves a report only; it does not create 3MF files.
                  </li>
                  <li>
                    <strong>Follow Print Run.</strong> Open each unsliced
                    project in its target slicer. Make every stated spool change
                    after the previous job finishes and before starting the next
                    one.
                  </li>
                </ol>
                <section
                  className="utility-dialog__notice"
                  aria-labelledby={`${instanceId}-color-boundary`}
                >
                  <h3 id={`${instanceId}-color-boundary`}>
                    Color accuracy boundary
                  </h3>
                  <p>
                    CMY+X recipes are physically color-qualified only when they
                    use your measured calibration record. An uncalibrated match
                    remains an explicit visual approximation that requires your
                    approval.
                  </p>
                </section>
              </div>
            ) : (
              <div className="utility-dialog__body">
                <p id={descriptionId}>
                  Confirm physical inputs and review the qualification-bound
                  printer targets before conversion.
                </p>
                <dl className="utility-dialog__targets">
                  <div>
                    <dt>Snapmaker U1</dt>
                    <dd>
                      Four 0.4 mm toolheads · CMY fixed to T1–T3 · swappable T4
                    </dd>
                  </div>
                  <div>
                    <dt>Bambu Lab A1 mini</dt>
                    <dd>
                      One 0.4 mm nozzle · single-spool mono output · 180 × 180
                      mm build area
                    </dd>
                  </div>
                </dl>
                <div className="utility-dialog__settings-actions">
                  <button
                    className="button"
                    type="button"
                    onClick={() => openDestination(onOpenPrintingSetup)}
                  >
                    <Printer aria-hidden="true" />
                    Available printers
                  </button>
                  <button
                    className="button"
                    type="button"
                    onClick={() => openDestination(onOpenFilamentLibrary)}
                  >
                    <LibraryBig aria-hidden="true" />
                    Open Filament Library
                  </button>
                  <button
                    className="button"
                    type="button"
                    onClick={() => openDestination(onOpenColorCalibration)}
                  >
                    <Pipette aria-hidden="true" />
                    Open Color Calibration
                  </button>
                </div>
                <p className="utility-dialog__footnote">
                  Nozzle, process, and bed settings are preserved from or
                  resolved by the qualified slicer adapter. Always review the
                  generated project in its target slicer before printing.
                </p>
              </div>
            )}
          </div>
        ) : null}
      </dialog>
    </>
  );
}
