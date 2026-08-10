import {
  Boxes,
  Grid3X3,
  RefreshCw,
  Repeat2,
  TriangleAlert,
} from "lucide-react";

interface StatusStripProps {
  targetPlates: number;
  batchCount: number;
  t4Swaps: number;
  a1SpoolChanges: number;
  fastMonoObjects: number;
  warnings: number;
}

const pluralize = (count: number, singular: string, plural = `${singular}s`) =>
  count === 1 ? singular : plural;

export function StatusStrip({
  targetPlates,
  batchCount,
  t4Swaps,
  a1SpoolChanges,
  fastMonoObjects,
  warnings,
}: StatusStripProps) {
  return (
    <section className="status-strip" aria-label="Print plan summary">
      <div className="status-metric">
        <Grid3X3 aria-hidden="true" />
        <strong>{targetPlates}</strong>
        <span>{pluralize(targetPlates, "target plate")}</span>
      </div>
      <div className="status-metric">
        <Boxes aria-hidden="true" />
        <strong>{batchCount}</strong>
        <span>{pluralize(batchCount, "batch", "batches")}</span>
      </div>
      <div className="status-metric">
        <Repeat2 aria-hidden="true" />
        <strong>{t4Swaps}</strong>
        <span>{pluralize(t4Swaps, "T4 swap")}</span>
      </div>
      <div className="status-metric">
        <RefreshCw aria-hidden="true" />
        <strong>{a1SpoolChanges}</strong>
        <span>{pluralize(a1SpoolChanges, "A1 spool change")}</span>
      </div>
      <div className="status-metric">
        <span className="mono-cube" aria-hidden="true" />
        <strong>{fastMonoObjects}</strong>
        <span>fast mono objects</span>
      </div>
      <div className={`status-metric ${warnings > 0 ? "status-metric--warning" : ""}`}>
        <TriangleAlert aria-hidden="true" />
        <strong>{warnings}</strong>
        <span>{pluralize(warnings, "warning")}</span>
      </div>
    </section>
  );
}
