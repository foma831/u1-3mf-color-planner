import {
  Disc3,
  FileSearch,
  FolderOutput,
  ListChecks,
  PlayCircle,
  Settings2,
  type LucideIcon,
} from "lucide-react";

interface WorkflowStep {
  label: string;
  icon: LucideIcon;
  state: "complete" | "current" | "upcoming";
}

interface WorkflowRailProps {
  inventoryReady: boolean;
  projectSetupReady: boolean;
  hasAnalysis: boolean;
  needsAttention: boolean;
  validatedPlanAvailable: boolean;
  printRunAvailable: boolean;
}

const stepDefinitions = [
  { label: "Inventory", icon: Disc3 },
  { label: "Setup", icon: Settings2 },
  { label: "Analyze", icon: FileSearch },
  { label: "Resolve", icon: ListChecks },
  { label: "Convert", icon: FolderOutput },
  { label: "Print", icon: PlayCircle },
] as const;

function workflowSteps({
  inventoryReady,
  projectSetupReady,
  hasAnalysis,
  needsAttention,
  validatedPlanAvailable,
  printRunAvailable,
}: WorkflowRailProps): WorkflowStep[] {
  const currentIndex = !inventoryReady
    ? 0
    : !projectSetupReady
      ? 1
      : !hasAnalysis
        ? 2
        : needsAttention || !validatedPlanAvailable
          ? 3
          : !printRunAvailable
            ? 4
            : 5;

  return stepDefinitions.map((step, index) => ({
    ...step,
    state:
      index < currentIndex
        ? "complete"
        : index === currentIndex
          ? "current"
          : "upcoming",
  }));
}

export function WorkflowRail(props: WorkflowRailProps) {
  const steps = workflowSteps(props);
  return (
    <aside className="workflow-rail" aria-label="Project progress">
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
    </aside>
  );
}
