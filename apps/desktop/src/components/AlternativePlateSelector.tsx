import type { AlternativePlate } from "../types";

interface AlternativePlateSelectorProps {
  plates: AlternativePlate[];
  onChange: (plateId: number, included: boolean) => void;
}

export function AlternativePlateSelector({
  plates,
  onChange,
}: AlternativePlateSelectorProps) {
  if (plates.length === 0) return null;

  return (
    <section
      className="alternative-plates"
      aria-labelledby="alternative-plates-heading"
      aria-describedby="alternative-plates-description"
    >
      <div className="alternative-plates__intro">
        <h3 id="alternative-plates-heading">Alternative source plates</h3>
        <p id="alternative-plates-description">
          These joint sets appear to be alternatives and are excluded by default.
          Select the versions to include, then validate the plan.
        </p>
      </div>
      <fieldset className="alternative-plates__choices">
        <legend className="visually-hidden">Joint plates to include</legend>
        {plates.map((plate) => (
          <label className="alternative-plate" key={plate.id}>
            <input
              type="checkbox"
              checked={plate.included}
              onChange={(event) => onChange(plate.id, event.currentTarget.checked)}
            />
            <span>
              <strong>{plate.name}</strong>
              <small>
                Source plate {plate.id} · {plate.included ? "Included" : "Excluded"}
              </small>
            </span>
          </label>
        ))}
      </fieldset>
    </section>
  );
}
