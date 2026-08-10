import { ShieldAlert } from "lucide-react";

import type { ExcludedSourceUnit } from "../types";

interface PartialConversionNoticeProps {
  available: boolean;
  exclusions: ExcludedSourceUnit[];
  reason: string;
  acknowledged: boolean;
  onAcknowledgedChange: (acknowledged: boolean) => void;
}

function sourcePlateLabel(sourcePlateId: number | null) {
  return sourcePlateId === null
    ? "Source plate not identified"
    : `Source Plate ${String(sourcePlateId).padStart(2, "0")}`;
}

export function PartialConversionNotice({
  available,
  exclusions,
  reason,
  acknowledged,
  onAcknowledgedChange,
}: PartialConversionNoticeProps) {
  if (exclusions.length === 0) return null;

  return (
    <section
      className="partial-conversion-notice"
      aria-labelledby="partial-conversion-heading"
    >
      <ShieldAlert aria-hidden="true" />
      <div className="partial-conversion-notice__content">
        <h3 id="partial-conversion-heading">
          {available ? "Partial conversion available" : "Partial conversion unavailable"}
        </h3>
        <p>
          The valid print jobs can be converted, but the source units below will
          not be included in generated project files or Print Run.
        </p>
        <ol className="partial-conversion-exclusions" role="list">
          {exclusions.map((exclusion) => (
            <li key={exclusion.errorIdentity}>
              <div>
                <strong>{exclusion.sourceUnitId}</strong>
                <span>{sourcePlateLabel(exclusion.sourcePlateId)}</span>
              </div>
              <p>{exclusion.reason}</p>
              <dl>
                <div>
                  <dt>Scope</dt>
                  <dd>{exclusion.scopeId}</dd>
                </div>
                <div>
                  <dt>Planning unit</dt>
                  <dd>{exclusion.planningUnitId}</dd>
                </div>
              </dl>
            </li>
          ))}
        </ol>
        {reason ? <p className="partial-conversion-notice__reason">{reason}</p> : null}
        <label className="partial-conversion-notice__approval">
          <input
            type="checkbox"
            checked={acknowledged}
            disabled={!available}
            aria-describedby="partial-conversion-approval-detail"
            onChange={(event) => onAcknowledgedChange(event.target.checked)}
          />
          <span>
            I understand that these source units will be excluded from this
            conversion.
          </span>
        </label>
        <p id="partial-conversion-approval-detail" className="form-help">
          This approval applies only to the exact analyzed plan above and resets
          whenever its evidence or planning choices change.
        </p>
      </div>
    </section>
  );
}
