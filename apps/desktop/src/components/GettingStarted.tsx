import {
  CheckCircle2,
  Circle,
  FileSearch,
  LibraryBig,
  LoaderCircle,
  Settings2,
  Upload,
} from "lucide-react";
import type { ReactNode } from "react";

interface GettingStartedProps {
  availableSpoolCount: number;
  isInventoryLoading: boolean;
  isProjectSetupConfirmed: boolean;
  projectSetup: ReactNode;
  pendingFileName?: string;
  isAnalyzing: boolean;
  onOpenFilamentLibrary: () => void;
  onOpenProject: () => void;
  onAnalyzeProject: () => void;
}

export function GettingStarted({
  availableSpoolCount,
  isInventoryLoading,
  isProjectSetupConfirmed,
  projectSetup,
  pendingFileName,
  isAnalyzing,
  onOpenFilamentLibrary,
  onOpenProject,
  onAnalyzeProject,
}: GettingStartedProps) {
  const inventoryReady = !isInventoryLoading && availableSpoolCount > 0;
  const projectStepAvailable = inventoryReady;
  const projectFileStepAvailable = inventoryReady && isProjectSetupConfirmed;

  return (
    <section
      className="getting-started"
      aria-labelledby="getting-started-heading"
    >
      <header className="getting-started__header">
        <span className="getting-started__icon" aria-hidden="true">
          <FileSearch />
        </span>
        <div>
          <p className="eyebrow">First project</p>
          <h2 id="getting-started-heading">Create a print-ready plan</h2>
          <p>
            Confirm the physical spools you actually have, choose the printers
            and U1 method, then record what is loaded in each printer position.
            Analysis is read-only and never changes the source file.
          </p>
        </div>
      </header>

      <ol className="getting-started__steps">
        <li
          className={inventoryReady ? "is-complete" : "is-current"}
          aria-current={inventoryReady ? undefined : "step"}
        >
          <span className="getting-started__step-icon" aria-hidden="true">
            {isInventoryLoading ? (
              <LoaderCircle className="spin" />
            ) : inventoryReady ? (
              <CheckCircle2 />
            ) : (
              <LibraryBig />
            )}
          </span>
          <div>
            <span className="getting-started__step-label">Step 1</span>
            <h3>Confirm available spools</h3>
            <p>
              {isInventoryLoading
                ? "Loading the filament library…"
                : inventoryReady
                  ? `${availableSpoolCount} physical ${
                      availableSpoolCount === 1 ? "spool is" : "spools are"
                    } marked available.`
                  : "Mark only the spools that are physically available now."}
            </p>
            <button
              className="button button--secondary button--compact"
              type="button"
              disabled={isInventoryLoading}
              onClick={onOpenFilamentLibrary}
            >
              <LibraryBig aria-hidden="true" />
              {inventoryReady ? "Review Filament Library" : "Confirm inventory"}
            </button>
          </div>
        </li>

        <li
          className={
            isProjectSetupConfirmed
              ? "is-complete"
              : projectStepAvailable
                ? "is-current"
                : "is-upcoming"
          }
          aria-current={
            projectStepAvailable && !isProjectSetupConfirmed
              ? "step"
              : undefined
          }
        >
          <span className="getting-started__step-icon" aria-hidden="true">
            {isProjectSetupConfirmed ? (
              <CheckCircle2 />
            ) : projectStepAvailable ? (
              <Settings2 />
            ) : (
              <Circle />
            )}
          </span>
          <div>
            <span className="getting-started__step-label">Step 2</span>
            <h3>Choose how to print this project</h3>
            <p>
              Select the printers to use now and choose Automatic, Direct
              Spools, or CMY+X. Confirm the current T1–T4 and A1 mini loadout so
              the first plan can minimize filament changes.
            </p>
            {projectStepAvailable ? projectSetup : null}
          </div>
        </li>

        <li
          className={projectFileStepAvailable ? "is-current" : "is-upcoming"}
          aria-current={projectFileStepAvailable ? "step" : undefined}
        >
          <span className="getting-started__step-icon" aria-hidden="true">
            {isAnalyzing ? (
              <LoaderCircle className="spin" />
            ) : projectFileStepAvailable || pendingFileName ? (
              <FileSearch />
            ) : (
              <Circle />
            )}
          </span>
          <div>
            <span className="getting-started__step-label">Step 3</span>
            <h3>Open and analyze a 3MF</h3>
            <p>
              {isAnalyzing
                ? `Analyzing ${pendingFileName ?? "the selected project"}…`
                : pendingFileName
                  ? `${pendingFileName} is ready. Analyze it to build the print plan.`
                  : "Choose a 3MF file to continue. The source file will remain unchanged."}
            </p>
            {projectFileStepAvailable && !isAnalyzing ? (
              <button
                className={`button button--compact ${
                  pendingFileName ? "button--dark" : "button--primary"
                }`}
                type="button"
                onClick={pendingFileName ? onAnalyzeProject : onOpenProject}
              >
                {pendingFileName ? (
                  <FileSearch aria-hidden="true" />
                ) : (
                  <Upload aria-hidden="true" />
                )}
                {pendingFileName ? "Analyze Project" : "Open 3MF"}
                <span className="visually-hidden"> from Step 3</span>
              </button>
            ) : null}
          </div>
        </li>

        <li className="is-upcoming">
          <span className="getting-started__step-icon" aria-hidden="true">
            <Circle />
          </span>
          <div>
            <span className="getting-started__step-label">Step 4</span>
            <h3>Resolve, convert, and print</h3>
            <p>
              Review only the highlighted decisions. The app will guide you to
              conversion when the plan is ready.
            </p>
          </div>
        </li>
      </ol>
    </section>
  );
}
