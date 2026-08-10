import { LibraryBig, ListTree, Pipette, PlayCircle } from "lucide-react";

export type ApplicationView = "plan" | "library" | "calibration" | "run";

interface ApplicationNavigationProps {
  activeView: ApplicationView;
  printRunAvailable: boolean;
  onChange: (view: ApplicationView) => void;
}

const items = [
  { id: "plan", label: "Print Plan", icon: ListTree },
  { id: "library", label: "Filament Library", icon: LibraryBig },
  { id: "calibration", label: "Color Calibration", icon: Pipette },
  { id: "run", label: "Print Run", icon: PlayCircle },
] as const;

export function ApplicationNavigation({
  activeView,
  printRunAvailable,
  onChange,
}: ApplicationNavigationProps) {
  return (
    <nav className="application-navigation" aria-label="Application views">
      {items.map(({ id, label, icon: Icon }) => {
        const disabled = id === "run" && !printRunAvailable;
        return (
          <button
            className={`application-navigation__item ${
              activeView === id ? "is-active" : ""
            }`}
            type="button"
            key={id}
            aria-current={activeView === id ? "page" : undefined}
            aria-describedby={disabled ? "print-run-navigation-hint" : undefined}
            title={
              disabled
                ? "Analyze, validate, and successfully convert a native print plan before starting."
                : undefined
            }
            disabled={disabled}
            onClick={() => onChange(id)}
          >
            <Icon aria-hidden="true" />
            {label}
          </button>
        );
      })}
      <span id="print-run-navigation-hint" className="visually-hidden">
        Analyze, validate, and successfully convert a native print plan before
        starting a print run.
      </span>
    </nav>
  );
}
