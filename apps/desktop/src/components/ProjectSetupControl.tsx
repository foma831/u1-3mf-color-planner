import { useEffect, useRef, useState } from "react";
import {
  CheckCircle2,
  ChevronDown,
  Printer,
  Settings2,
  SlidersHorizontal,
  TriangleAlert,
} from "lucide-react";

import type {
  DefaultPlanningStrategy,
  PlanningIntent,
} from "../services/planning-intent";
import type { PrinterLoadoutProfile } from "../services/printer-loadout";
import type { PrintingSetup } from "../services/printing-setup";
import type { PhysicalSpool, ToolheadId } from "../types";
import { ColorSwatch } from "./ColorSwatch";

interface ProjectSetupControlProps {
  intent: PlanningIntent;
  loadout: PrinterLoadoutProfile;
  equipmentSetup: PrintingSetup | null;
  availableSpools: PhysicalSpool[];
  hasAnalysis: boolean;
  a1PlateCount: number;
  isFixedPreview?: boolean;
  isOpen: boolean;
  isBusy?: boolean;
  isStale?: boolean;
  customDirectPalettesEnabled?: boolean;
  customDirectPalettesFixed?: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: (intent: PlanningIntent, loadout: PrinterLoadoutProfile) => void;
  onChangeEquipment: () => void;
  onCustomDirectPalettesChange?: (enabled: boolean) => void;
}

const strategyOptions: Array<{
  value: DefaultPlanningStrategy;
  title: string;
  detail: string;
}> = [
  {
    value: "auto",
    title: "Automatic",
    detail: "Compare CMY+X and Direct Spools for each source scope.",
  },
  {
    value: "direct",
    title: "Direct Spools only",
    detail:
      "Use available physical spools. Never fall back to CMY+X silently; each scope is limited to four effective material-color pairs.",
  },
  {
    value: "cmyx",
    title: "CMY+X Full Spectrum only",
    detail: "Use the fixed C/M/Y toolheads plus the selected T4 spool.",
  },
];

export function ProjectSetupControl({
  intent,
  loadout,
  equipmentSetup,
  availableSpools,
  hasAnalysis,
  a1PlateCount,
  isFixedPreview = false,
  isOpen,
  isBusy = false,
  isStale = false,
  customDirectPalettesEnabled = false,
  customDirectPalettesFixed = false,
  onOpenChange,
  onConfirm,
  onChangeEquipment,
  onCustomDirectPalettesChange,
}: ProjectSetupControlProps) {
  const [draft, setDraft] = useState(intent);
  const [loadoutDraft, setLoadoutDraft] = useState(loadout);
  const [loadoutError, setLoadoutError] = useState("");
  const editButtonRef = useRef<HTMLButtonElement>(null);
  const loadoutFieldsetRef = useRef<HTMLFieldSetElement>(null);

  useEffect(() => {
    if (!isOpen) return;
    setDraft(intent);
    setLoadoutDraft(loadout);
    setLoadoutError("");
  }, [intent, isOpen, loadout]);

  const availableA1Mini =
    equipmentSetup === null || equipmentSetup.secondaryPrinter === "a1-mini";
  const activeSpools = availableSpools.filter((spool) => spool.available);
  const spoolById = new Map(availableSpools.map((spool) => [spool.id, spool]));
  const activeSpoolIds = new Set(activeSpools.map((spool) => spool.id));
  const selectedLoadout = [
    ...loadoutDraft.currentLoadout.map((entry) => ({
      location: entry.toolhead,
      spoolId: entry.spoolId,
    })),
    ...(availableA1Mini && loadoutDraft.currentA1SpoolId
      ? [
          {
            location: "A1 mini" as const,
            spoolId: loadoutDraft.currentA1SpoolId,
          },
        ]
      : []),
  ];
  const selectedLocationBySpool = new Map(
    selectedLoadout.map((entry) => [entry.spoolId, entry.location]),
  );
  const staleLoadout = selectedLoadout.filter(
    (entry) => !activeSpoolIds.has(entry.spoolId),
  );
  const selectedA1Spool = loadoutDraft.currentA1SpoolId
    ? spoolById.get(loadoutDraft.currentA1SpoolId)
    : undefined;
  const loadedU1Count = loadoutDraft.currentLoadout.length;
  const summaryIntent = isOpen ? draft : intent;
  const strategyLabel = strategyOptions.find(
    (option) => option.value === summaryIntent.defaultStrategy,
  )?.title;
  const printerLabel = summaryIntent.a1MiniEnabled
    ? "U1 + A1 mini"
    : "Snapmaker U1";
  const fixedPreviewOverridesA1 =
    isFixedPreview && !intent.a1MiniEnabled && a1PlateCount > 0;
  const resultLabel = fixedPreviewOverridesA1
    ? `Fixed browser preview · ${a1PlateCount} A1 mini ${a1PlateCount === 1 ? "plate" : "plates"}`
    : intent.a1MiniEnabled
      ? `A1 mini requested · ${a1PlateCount} eligible ${a1PlateCount === 1 ? "plate" : "plates"}`
      : "A1 mini not requested";

  return (
    <section
      className={`project-setup ${isStale ? "is-stale" : ""}`}
      aria-labelledby="project-setup-heading"
    >
      <header className="project-setup__summary">
        <span className="project-setup__summary-icon" aria-hidden="true">
          <Settings2 />
        </span>
        <span className="project-setup__summary-copy">
          <strong id="project-setup-heading">Project setup</strong>
          <small>
            {strategyLabel} · {printerLabel}
          </small>
        </span>
        <span
          className={`project-setup__status ${
            isStale ? "is-pending" : hasAnalysis ? "is-applied" : ""
          }`}
        >
          {isStale
            ? "Changes pending"
            : isFixedPreview
              ? "Demo preview"
              : hasAnalysis
                ? "Applied"
                : "Step 2"}
        </span>
        {!isOpen ? (
          <button
            ref={editButtonRef}
            id="project-setup-change"
            className="button button--secondary button--compact"
            type="button"
            aria-expanded="false"
            aria-controls="project-setup-form"
            onClick={() => onOpenChange(true)}
          >
            Change setup
            <ChevronDown aria-hidden="true" />
          </button>
        ) : null}
      </header>

      {hasAnalysis ? (
        <p className="project-setup__result">
          {intent.a1MiniEnabled || fixedPreviewOverridesA1 ? (
            a1PlateCount > 0 ? (
              <CheckCircle2 aria-hidden="true" />
            ) : (
              <TriangleAlert aria-hidden="true" />
            )
          ) : (
            <Printer aria-hidden="true" />
          )}
          <span>
            <strong>{resultLabel}</strong>
            {fixedPreviewOverridesA1 ? (
              <small>
                The browser demo uses a fixed sample queue. Desktop analysis
                applies the Project setup you confirmed.
              </small>
            ) : intent.a1MiniEnabled && a1PlateCount === 0 ? (
              <small>
                The preference remains enabled. This plan found no mono jobs
                that satisfy A1 mini size, material, and spool constraints.
              </small>
            ) : null}
          </span>
        </p>
      ) : null}

      {isOpen ? (
        <form
          id="project-setup-form"
          className="project-setup__form"
          onSubmit={(event) => {
            event.preventDefault();
            const selectedSpoolIds = selectedLoadout.map(
              (entry) => entry.spoolId,
            );
            if (new Set(selectedSpoolIds).size !== selectedSpoolIds.length) {
              setLoadoutError(
                "One physical spool cannot be loaded in more than one printer position.",
              );
              loadoutFieldsetRef.current?.focus();
              return;
            }
            if (staleLoadout.length > 0) {
              setLoadoutError(
                "Choose an available spool or Unknown for every saved position that is no longer in the physical inventory.",
              );
              loadoutFieldsetRef.current?.focus();
              return;
            }
            setLoadoutError("");
            onConfirm(draft, {
              ...loadoutDraft,
              currentA1SpoolId: availableA1Mini
                ? loadoutDraft.currentA1SpoolId
                : null,
              updatedFrom: "manual",
            });
          }}
        >
          <div className="project-setup__groups">
            <fieldset className="project-setup__group">
              <legend>Printers for this project</legend>
              <p>
                Snapmaker U1 remains the primary printer. A1 mini receives only
                verified single-spool jobs.
              </p>
              <label className="project-setup__choice">
                <input
                  type="radio"
                  name="project-printers"
                  value="u1"
                  checked={!draft.a1MiniEnabled}
                  disabled={isBusy}
                  onChange={() =>
                    setDraft((current) => ({
                      ...current,
                      a1MiniEnabled: false,
                    }))
                  }
                />
                <span>
                  <strong>Snapmaker U1 only</strong>
                  <small>Keep every eligible job on the U1.</small>
                </span>
              </label>
              <label
                className={`project-setup__choice ${
                  availableA1Mini ? "" : "is-disabled"
                }`}
              >
                <input
                  type="radio"
                  name="project-printers"
                  value="u1-a1-mini"
                  checked={draft.a1MiniEnabled}
                  disabled={isBusy || !availableA1Mini}
                  onChange={() =>
                    setDraft((current) => ({
                      ...current,
                      a1MiniEnabled: true,
                    }))
                  }
                />
                <span>
                  <strong>Snapmaker U1 + Bambu Lab A1 mini</strong>
                  <small>
                    Use A1 mini for eligible mono jobs in this plan.
                  </small>
                </span>
              </label>
              {!availableA1Mini ? (
                <div className="project-setup__equipment-note">
                  <span>A1 mini is not listed in Available equipment.</span>
                  <button
                    className="button button--tertiary button--compact"
                    type="button"
                    onClick={onChangeEquipment}
                  >
                    Add A1 mini
                  </button>
                </div>
              ) : (
                <button
                  className="project-setup__equipment-link"
                  type="button"
                  onClick={onChangeEquipment}
                >
                  Change available equipment
                </button>
              )}
            </fieldset>

            <fieldset className="project-setup__group">
              <legend>U1 printing method</legend>
              <p>
                This choice controls the first plan. You can change it later and
                recalculate.
              </p>
              {strategyOptions.map((option) => (
                <label className="project-setup__choice" key={option.value}>
                  <input
                    type="radio"
                    name="default-strategy"
                    value={option.value}
                    checked={draft.defaultStrategy === option.value}
                    disabled={isBusy}
                    onChange={() =>
                      setDraft((current) => ({
                        ...current,
                        defaultStrategy: option.value,
                      }))
                    }
                  />
                  <span>
                    <strong>
                      {option.title}
                      {option.value === "auto" ? <em>Recommended</em> : null}
                    </strong>
                    <small>{option.detail}</small>
                  </span>
                </label>
              ))}
            </fieldset>
          </div>

          <fieldset
            ref={loadoutFieldsetRef}
            className="project-setup__loadout"
            tabIndex={-1}
            aria-describedby="project-setup-loadout-help"
          >
            <legend>Filament loaded now</legend>
            <div className="project-setup__loadout-heading">
              <p id="project-setup-loadout-help">
                Used to minimize swaps and create exact first setup
                instructions. Unknown positions are handled conservatively.
              </p>
              <span>{loadedU1Count}/4 U1 positions confirmed</span>
            </div>
            <div className="project-setup__loadout-grid">
              {(["T1", "T2", "T3", "T4"] as ToolheadId[]).map((toolhead) => {
                const selectedSpoolId =
                  loadoutDraft.currentLoadout.find(
                    (entry) => entry.toolhead === toolhead,
                  )?.spoolId ?? "";
                const selectedSpoolIsStale = Boolean(
                  selectedSpoolId && !activeSpoolIds.has(selectedSpoolId),
                );
                const selectedSpool = spoolById.get(selectedSpoolId);
                return (
                  <label key={toolhead}>
                    <span>U1 {toolhead}</span>
                    <div className="project-setup__spool-select">
                      <span
                        className={`project-setup__spool-swatch${
                          selectedSpool ? "" : " is-empty"
                        }`}
                        aria-hidden="true"
                        title={
                          selectedSpool
                            ? `${selectedSpool.colorName} · ${selectedSpool.hex}`
                            : "Unknown spool color"
                        }
                      >
                        {selectedSpool ? (
                          <ColorSwatch hex={selectedSpool.hex} size="small" />
                        ) : null}
                      </span>
                      <select
                        name={`current-${toolhead.toLowerCase()}-spool`}
                        value={selectedSpoolId}
                        disabled={isBusy}
                        aria-invalid={selectedSpoolIsStale || undefined}
                        onChange={(event) => {
                          const spoolId = event.target.value;
                          setLoadoutError("");
                          setLoadoutDraft((current) => ({
                            ...current,
                            currentLoadout: [
                              ...current.currentLoadout.filter(
                                (entry) => entry.toolhead !== toolhead,
                              ),
                              ...(spoolId ? [{ toolhead, spoolId }] : []),
                            ].sort((left, right) =>
                              left.toolhead.localeCompare(right.toolhead),
                            ),
                          }));
                        }}
                      >
                        <option value="">Unknown / not confirmed</option>
                        {selectedSpoolIsStale ? (
                          <option value={selectedSpoolId}>
                            Unavailable saved spool · {selectedSpoolId}
                          </option>
                        ) : null}
                        {activeSpools.map((spool) => {
                          const loadedAt = selectedLocationBySpool.get(spool.id);
                          const usedElsewhere =
                            loadedAt !== undefined && loadedAt !== toolhead;
                          return (
                            <option
                              key={spool.id}
                              value={spool.id}
                              disabled={usedElsewhere}
                            >
                              {spool.colorName} · {spool.material} · {spool.name}
                              {usedElsewhere ? ` — loaded in ${loadedAt}` : ""}
                            </option>
                          );
                        })}
                      </select>
                    </div>
                  </label>
                );
              })}
              {availableA1Mini ? (
                <label>
                  <span>A1 mini external spool</span>
                  <div className="project-setup__spool-select">
                    <span
                      className={`project-setup__spool-swatch${
                        selectedA1Spool ? "" : " is-empty"
                      }`}
                      aria-hidden="true"
                      title={
                        selectedA1Spool
                          ? `${selectedA1Spool.colorName} · ${selectedA1Spool.hex}`
                          : "Unknown spool color"
                      }
                    >
                      {selectedA1Spool ? (
                        <ColorSwatch hex={selectedA1Spool.hex} size="small" />
                      ) : null}
                    </span>
                    <select
                      name="current-a1-spool"
                      value={loadoutDraft.currentA1SpoolId ?? ""}
                      disabled={isBusy}
                      aria-invalid={
                        Boolean(
                          loadoutDraft.currentA1SpoolId &&
                          !activeSpoolIds.has(loadoutDraft.currentA1SpoolId),
                        ) || undefined
                      }
                      onChange={(event) => {
                        setLoadoutError("");
                        setLoadoutDraft((current) => ({
                          ...current,
                          currentA1SpoolId: event.target.value || null,
                        }));
                      }}
                    >
                      <option value="">Unknown / not confirmed</option>
                      {loadoutDraft.currentA1SpoolId &&
                      !activeSpoolIds.has(loadoutDraft.currentA1SpoolId) ? (
                        <option value={loadoutDraft.currentA1SpoolId}>
                          Unavailable saved spool ·{" "}
                          {loadoutDraft.currentA1SpoolId}
                        </option>
                      ) : null}
                      {activeSpools.map((spool) => {
                        const loadedAt = selectedLocationBySpool.get(spool.id);
                        const usedElsewhere =
                          loadedAt !== undefined && loadedAt !== "A1 mini";
                        return (
                          <option
                            key={spool.id}
                            value={spool.id}
                            disabled={usedElsewhere}
                          >
                            {spool.colorName} · {spool.material} · {spool.name}
                            {usedElsewhere ? ` — loaded in ${loadedAt}` : ""}
                          </option>
                        );
                      })}
                    </select>
                  </div>
                </label>
              ) : null}
            </div>
            {loadoutError ? (
              <p className="project-setup__loadout-error" role="alert">
                {loadoutError}
              </p>
            ) : staleLoadout.length > 0 ? (
              <p className="project-setup__loadout-warning">
                A saved spool is no longer marked available. Choose an available
                spool or Unknown before continuing.
              </p>
            ) : (
              <small>
                Each physical spool can occupy only one U1 toolhead or A1 mini
                position at a time.
              </small>
            )}
          </fieldset>

          {hasAnalysis && onCustomDirectPalettesChange ? (
            <details className="project-setup__advanced">
              <summary>
                <SlidersHorizontal aria-hidden="true" />
                Advanced Direct settings
              </summary>
              <label>
                <input
                  type="checkbox"
                  checked={customDirectPalettesEnabled}
                  disabled={isBusy || customDirectPalettesFixed}
                  onChange={(event) =>
                    onCustomDirectPalettesChange(event.target.checked)
                  }
                />
                <span>
                  <strong>Allow custom four-spool palettes per U1 plate</strong>
                  <small>
                    Explicitly map multiple source identities to no more than
                    four physical spools. Material identity remains visible.
                  </small>
                </span>
              </label>
            </details>
          ) : null}

          <div className="project-setup__actions">
            <button
              className="button button--dark"
              type="submit"
              disabled={isBusy}
            >
              {hasAnalysis ? "Apply & recalculate" : "Confirm project setup"}
            </button>
            {hasAnalysis ? (
              <button
                className="button button--secondary"
                type="button"
                disabled={isBusy}
                onClick={() => {
                  setDraft(intent);
                  setLoadoutDraft(loadout);
                  setLoadoutError("");
                  onOpenChange(false);
                  window.requestAnimationFrame(() =>
                    editButtonRef.current?.focus(),
                  );
                }}
              >
                Cancel
              </button>
            ) : null}
          </div>
        </form>
      ) : null}
    </section>
  );
}
