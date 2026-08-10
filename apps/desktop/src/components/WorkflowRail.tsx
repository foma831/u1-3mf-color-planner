import {
  CalendarDays,
  Disc3,
  FileBox,
  Palette,
  ShieldCheck,
  type LucideIcon,
} from "lucide-react";

interface WorkflowStep {
  label: string;
  icon: LucideIcon;
  state: "complete" | "current" | "upcoming";
}

interface WorkflowRailProps {
  hasAnalysis: boolean;
  hasPendingPlanChanges: boolean;
  planReady: boolean;
}

function workflowSteps({
  hasAnalysis,
  hasPendingPlanChanges,
  planReady,
}: WorkflowRailProps): WorkflowStep[] {
  if (!hasAnalysis) {
    return [
      { label: "Project", icon: FileBox, state: "current" },
      { label: "Materials", icon: Disc3, state: "upcoming" },
      { label: "Color Strategy", icon: Palette, state: "upcoming" },
      { label: "Print Plan", icon: CalendarDays, state: "upcoming" },
      { label: "Validate", icon: ShieldCheck, state: "upcoming" },
    ];
  }

  if (hasPendingPlanChanges) {
    return [
      { label: "Project", icon: FileBox, state: "complete" },
      { label: "Materials", icon: Disc3, state: "complete" },
      { label: "Color Strategy", icon: Palette, state: "current" },
      { label: "Print Plan", icon: CalendarDays, state: "upcoming" },
      { label: "Validate", icon: ShieldCheck, state: "upcoming" },
    ];
  }

  return [
    { label: "Project", icon: FileBox, state: "complete" },
    { label: "Materials", icon: Disc3, state: "complete" },
    { label: "Color Strategy", icon: Palette, state: "complete" },
    {
      label: "Print Plan",
      icon: CalendarDays,
      state: planReady ? "complete" : "current",
    },
    {
      label: "Validate",
      icon: ShieldCheck,
      state: planReady ? "current" : "upcoming",
    },
  ];
}

export function WorkflowRail(props: WorkflowRailProps) {
  const steps = workflowSteps(props);
  return (
    <nav className="workflow-rail" aria-label="Project workflow">
      <ol className="workflow-steps">
        {steps.map(({ label, icon: Icon, state }, index) => (
          <li
            className={`workflow-step workflow-step--${state}`}
            aria-current={state === "current" ? "step" : undefined}
            key={label}
          >
            <Icon aria-hidden="true" />
            <span>{label}</span>
            <span className="visually-hidden">
              {state === "complete"
                ? `Step ${index + 1}, complete`
                : state === "current"
                  ? `Step ${index + 1}, current`
                  : `Step ${index + 1}, upcoming`}
            </span>
          </li>
        ))}
      </ol>
    </nav>
  );
}
