import { ArrowRight, RefreshCw } from "lucide-react";

import type { BatchSegment } from "../types";

interface BatchTimelineProps {
  batches: BatchSegment[];
  totalPlates: number;
  isStale?: boolean;
}

export function BatchTimeline({ batches, totalPlates, isStale = false }: BatchTimelineProps) {
  return (
    <section className="timeline-section" aria-labelledby="batch-timeline-heading">
      <div className="section-heading-row">
        <div>
          <h2 id="batch-timeline-heading">Batch timeline</h2>
          <p>
            {isStale
              ? "Pending edits — validate choices to refresh the authoritative schedule."
              : "Setup boundaries follow the planned plate order."}
          </p>
        </div>
        <span className="timeline-count">
          {batches.length} {batches.length === 1 ? "batch" : "batches"}
        </span>
      </div>
      <ol className="batch-timeline" aria-label="Print batches">
        {batches.map((batch, index) => (
          <li
            className={`batch-segment batch-segment--${batch.strategy}`}
            key={`${batch.id}-${batch.startOrder}`}
            style={{ flexGrow: batch.plateCount / Math.max(totalPlates, 1) }}
          >
            <span className="batch-segment__bar">
              <strong>{batch.label}</strong>
              <span>
                Plates {batch.startOrder}
                {batch.endOrder !== batch.startOrder ? `–${batch.endOrder}` : ""}
              </span>
            </span>
            <span className="batch-segment__detail">{batch.detail}</span>
            {batch.setupActions.length > 0 ? (
              <details className="batch-actions">
                <summary>
                  {batch.setupActions.length} setup {batch.setupActions.length === 1 ? "action" : "actions"}
                </summary>
                <ul>
                  {batch.setupActions.map((action) => (
                    <li key={action}>{action}</li>
                  ))}
                </ul>
              </details>
            ) : null}
            {index < batches.length - 1 ? (
              <span className="batch-boundary">
                {batch.strategy === "direct" ||
                batches[index + 1]?.strategy === "direct" ? (
                  <RefreshCw aria-hidden="true" />
                ) : (
                  <ArrowRight aria-hidden="true" />
                )}
                After Plate {String(batch.endOrder).padStart(2, "0")}
              </span>
            ) : null}
          </li>
        ))}
      </ol>
    </section>
  );
}
