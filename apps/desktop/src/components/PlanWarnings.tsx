import { TriangleAlert } from "lucide-react";

interface PlanWarningsProps {
  warnings: string[];
}

export function PlanWarnings({ warnings }: PlanWarningsProps) {
  if (warnings.length === 0) {
    return null;
  }

  return (
    <section className="plan-warnings" aria-labelledby="plan-warnings-heading">
      <details className="warning-disclosure warning-disclosure--global">
        <summary>
          <TriangleAlert aria-hidden="true" />
          <span id="plan-warnings-heading">Project warnings</span>
          <span className="warning-disclosure__count">{warnings.length}</span>
        </summary>
        <ul>
          {warnings.map((warning, index) => (
            <li key={`global-warning-${index}`}>{warning}</li>
          ))}
        </ul>
      </details>
    </section>
  );
}
