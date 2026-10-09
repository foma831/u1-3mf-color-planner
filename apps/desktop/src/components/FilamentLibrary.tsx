import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type FormEvent,
} from "react";
import {
  CheckCircle2,
  CircleOff,
  LibraryBig,
  Pencil,
  Search,
  Trash2,
} from "lucide-react";

import type {
  NewPhysicalSpoolInput,
  PhysicalSpool,
  SpoolMaterial,
} from "../types";
import { inStockSpools } from "../utils/plan";
import { ColorSwatch } from "./ColorSwatch";
import { SpoolInventoryForm } from "./SpoolInventoryForm";

type AvailabilityFilter = "all" | "in-stock" | "out-of-stock";
type MaterialFilter = "all" | SpoolMaterial;
type DeleteFocusTarget = {
  spoolId: string | null;
  waitForRemovalId?: string;
};

interface FilamentLibraryProps {
  spools: PhysicalSpool[];
  onAddSpool: (spool: NewPhysicalSpoolInput) => void;
  onUpdateSpool: (spoolId: string, spool: NewPhysicalSpoolInput) => void;
  onAvailabilityChange: (spoolId: string, available: boolean) => void;
  onDeleteSpool: (spoolId: string) => Promise<boolean>;
}

function spoolMatchesQuery(spool: PhysicalSpool, query: string) {
  if (!query) return true;
  const searchable = [
    spool.name,
    spool.colorName,
    spool.hex,
    spool.material,
    spool.sku ?? "",
    spool.profile ?? "",
    spool.vendor ?? "",
    spool.productLine ?? "",
    spool.opticalDescriptor ?? "",
    spool.batchLot ?? "",
    spool.calibrationReference ?? "",
    spool.notes ?? "",
  ]
    .join(" ")
    .toLocaleLowerCase();
  return searchable.includes(query.toLocaleLowerCase());
}

function editInput(spool: PhysicalSpool): NewPhysicalSpoolInput {
  return {
    name: spool.name,
    colorName: spool.colorName,
    hex: spool.hex,
    material: spool.material,
    sku: spool.sku,
    profile: spool.profile,
    vendor: spool.vendor,
    productLine: spool.productLine,
    opticalDescriptor: spool.opticalDescriptor,
    minNozzleTemperatureC: spool.minNozzleTemperatureC,
    maxNozzleTemperatureC: spool.maxNozzleTemperatureC,
    batchLot: spool.batchLot,
    calibrationReference: spool.calibrationReference,
    notes: spool.notes,
  };
}

function nozzleTemperatureLabel(spool: PhysicalSpool) {
  const minimum = spool.minNozzleTemperatureC;
  const maximum = spool.maxNozzleTemperatureC;
  if (minimum !== undefined && maximum !== undefined) {
    return `${minimum}–${maximum} °C`;
  }
  if (minimum !== undefined) return `from ${minimum} °C`;
  if (maximum !== undefined) return `up to ${maximum} °C`;
  return null;
}

export function FilamentLibrary({
  spools,
  onAddSpool,
  onUpdateSpool,
  onAvailabilityChange,
  onDeleteSpool,
}: FilamentLibraryProps) {
  const instanceId = useId().replace(/:/g, "");
  const [query, setQuery] = useState("");
  const [availability, setAvailability] = useState<AvailabilityFilter>("all");
  const [material, setMaterial] = useState<MaterialFilter>("all");
  const [editingSpoolId, setEditingSpoolId] = useState<string | null>(null);
  const [deleteCandidate, setDeleteCandidate] = useState<PhysicalSpool | null>(
    null,
  );
  const [isDeleting, setIsDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState("");
  const editButtonRefs = useRef(new Map<string, HTMLButtonElement>());
  const deleteButtonRefs = useRef(new Map<string, HTMLButtonElement>());
  const deleteDialogRef = useRef<HTMLDialogElement | null>(null);
  const deleteCancelRef = useRef<HTMLButtonElement | null>(null);
  const libraryHeadingRef = useRef<HTMLHeadingElement | null>(null);
  const pendingDeleteFocus = useRef<DeleteFocusTarget | null>(null);
  const stockCount = inStockSpools(spools).length;
  const outOfStockCount = spools.length - stockCount;
  const editingSpool = spools.find((spool) => spool.id === editingSpoolId);
  const visibleSpools = useMemo(
    () =>
      [...spools]
        .filter((spool) => spoolMatchesQuery(spool, query.trim()))
        .filter((spool) => {
          if (availability === "in-stock") return spool.available;
          if (availability === "out-of-stock") return !spool.available;
          return true;
        })
        .filter((spool) => material === "all" || spool.material === material)
        .sort((left, right) =>
          left.name.localeCompare(right.name, undefined, {
            sensitivity: "base",
          }),
        ),
    [availability, material, query, spools],
  );

  const preventSearchNavigation = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
  };

  const finishEditing = (spoolId: string) => {
    setEditingSpoolId(null);
    window.setTimeout(() => {
      const editButton = editButtonRefs.current.get(spoolId);
      if (editButton) editButton.focus();
      else libraryHeadingRef.current?.focus();
    }, 0);
  };

  useEffect(() => {
    const dialog = deleteDialogRef.current;
    if (deleteCandidate) {
      if (dialog && !dialog.open) {
        if (typeof dialog.showModal === "function") {
          dialog.showModal();
        } else {
          // jsdom and older embedded webviews do not expose showModal().
          dialog.setAttribute("open", "");
          deleteCancelRef.current?.focus();
        }
      }
      return;
    }

    if (dialog?.open) {
      if (typeof dialog.close === "function") {
        dialog.close();
      } else {
        dialog.removeAttribute("open");
      }
    }

    const target = pendingDeleteFocus.current;
    if (!target) return;
    if (
      target.waitForRemovalId &&
      spools.some((spool) => spool.id === target.waitForRemovalId)
    ) {
      return;
    }
    pendingDeleteFocus.current = null;
    const deleteButton = target.spoolId
      ? deleteButtonRefs.current.get(target.spoolId)
      : undefined;
    if (deleteButton) {
      deleteButton.focus();
    } else {
      libraryHeadingRef.current?.focus();
    }
  }, [deleteCandidate, spools]);

  const cancelDelete = () => {
    if (!deleteCandidate || isDeleting) return;
    pendingDeleteFocus.current = { spoolId: deleteCandidate.id };
    setDeleteError("");
    setDeleteCandidate(null);
  };

  const confirmDelete = async () => {
    if (!deleteCandidate || isDeleting) return;

    const candidateIndex = visibleSpools.findIndex(
      (candidate) => candidate.id === deleteCandidate.id,
    );
    const nextUserSpool = visibleSpools
      .slice(candidateIndex + 1)
      .find((candidate) => candidate.source === "user");
    const previousUserSpool = visibleSpools
      .slice(0, candidateIndex)
      .reverse()
      .find((candidate) => candidate.source === "user");

    setIsDeleting(true);
    setDeleteError("");
    try {
      const deleted = await onDeleteSpool(deleteCandidate.id);
      if (!deleted) {
        setDeleteError(
          "The spool was not deleted. Review the storage error and try again.",
        );
        return;
      }

      pendingDeleteFocus.current = {
        spoolId: nextUserSpool?.id ?? previousUserSpool?.id ?? null,
        waitForRemovalId: deleteCandidate.id,
      };
      if (editingSpoolId === deleteCandidate.id) {
        setEditingSpoolId(null);
      }
      setDeleteCandidate(null);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setDeleteError(`The spool was not deleted. ${message}`);
    } finally {
      setIsDeleting(false);
    }
  };

  return (
    <section
      className="filament-library"
      aria-labelledby={`${instanceId}-filament-library-heading`}
    >
      <header className="filament-library__header">
        <div className="filament-library__title">
          <span className="filament-library__icon" aria-hidden="true">
            <LibraryBig />
          </span>
          <div>
            <h2
              ref={libraryHeadingRef}
              id={`${instanceId}-filament-library-heading`}
              tabIndex={-1}
            >
              Filament Library
            </h2>
            <p>
              Keep your complete spool catalogue here. Mark only the spools you
              physically have as Available now; only those are offered to the
              print planner.
            </p>
          </div>
        </div>
        <dl className="filament-library__counts" aria-label="Library counts">
          <div>
            <dt>Total</dt>
            <dd>{spools.length}</dd>
          </div>
          <div>
            <dt>Available now</dt>
            <dd>{stockCount}</dd>
          </div>
          <div>
            <dt>Not available</dt>
            <dd>{outOfStockCount}</dd>
          </div>
        </dl>
      </header>

      <SpoolInventoryForm
        idPrefix={`${instanceId}-library-add`}
        spoolCount={stockCount}
        onAddSpool={onAddSpool}
        heading="Add a user spool"
        description="Add it once, then update its physical availability whenever the spool runs out or is replaced."
        summaryLabel="Add spool to library"
      />

      <form
        className="filament-library__filters"
        role="search"
        onSubmit={preventSearchNavigation}
      >
        <fieldset>
          <legend className="visually-hidden">Filter filament library</legend>
          <div className="field filament-library__search">
            <label htmlFor={`${instanceId}-filament-search`}>
              Search library
            </label>
            <span className="filament-library__search-control">
              <Search aria-hidden="true" />
              <input
                id={`${instanceId}-filament-search`}
                name="filament-search"
                type="search"
                value={query}
                placeholder="Name, color, vendor, lot, calibration, or profile"
                onChange={(event) => setQuery(event.target.value)}
              />
            </span>
          </div>
          <div className="field">
            <label htmlFor={`${instanceId}-availability-filter`}>
              Availability
            </label>
            <select
              id={`${instanceId}-availability-filter`}
              name="availability-filter"
              value={availability}
              onChange={(event) =>
                setAvailability(event.target.value as AvailabilityFilter)
              }
            >
              <option value="all">All statuses</option>
              <option value="in-stock">Available now</option>
              <option value="out-of-stock">Not available</option>
            </select>
          </div>
          <div className="field">
            <label htmlFor={`${instanceId}-material-filter`}>Material</label>
            <select
              id={`${instanceId}-material-filter`}
              name="material-filter"
              value={material}
              onChange={(event) =>
                setMaterial(event.target.value as MaterialFilter)
              }
            >
              <option value="all">All materials</option>
              <option value="PLA">PLA</option>
              <option value="PETG">PETG</option>
              <option value="PVA">PVA</option>
            </select>
          </div>
        </fieldset>
      </form>

      <div
        className="filament-library__table-wrap"
        role="region"
        aria-label="Scrollable filament catalogue"
        tabIndex={0}
      >
        <table className="filament-library__table">
          <caption aria-live="polite" aria-atomic="true">
            Showing {visibleSpools.length} of {spools.length} catalogued spools.
          </caption>
          <thead>
            <tr>
              <th scope="col">Color</th>
              <th scope="col">Spool</th>
              <th scope="col">Material</th>
              <th scope="col">Status</th>
              <th scope="col">Actions</th>
            </tr>
          </thead>
          <tbody>
            {visibleSpools.length === 0 ? (
              <tr>
                <td className="filament-library__empty" colSpan={5}>
                  No spools match these filters. Clear the search or choose a
                  different status.
                </td>
              </tr>
            ) : (
              visibleSpools.map((spool) => {
                const nozzleTemperature = nozzleTemperatureLabel(spool);
                return (
                  <tr key={spool.id}>
                    <td>
                      <span className="filament-library__color">
                        <ColorSwatch hex={spool.hex} size="large" />
                        <span>
                          <strong>{spool.colorName}</strong>
                          <code>{spool.hex}</code>
                        </span>
                      </span>
                    </td>
                    <th scope="row">
                      <span className="filament-library__spool-name">
                        <strong>{spool.name}</strong>
                        {spool.vendor || spool.productLine ? (
                          <span>
                            {[spool.vendor, spool.productLine]
                              .filter(Boolean)
                              .join(" · ")}
                          </span>
                        ) : null}
                        <span>
                          {spool.source === "built-in"
                            ? "Built-in catalogue"
                            : "User spool"}
                          {spool.sku ? ` · ${spool.sku}` : ""}
                        </span>
                        {spool.profile ? <small>{spool.profile}</small> : null}
                        {spool.opticalDescriptor ? (
                          <small>Optical: {spool.opticalDescriptor}</small>
                        ) : null}
                        {spool.batchLot || spool.calibrationReference ? (
                          <small>
                            {spool.batchLot ? `Lot ${spool.batchLot}` : ""}
                            {spool.batchLot && spool.calibrationReference
                              ? " · "
                              : ""}
                            {spool.calibrationReference
                              ? `Calibration ${spool.calibrationReference}`
                              : ""}
                          </small>
                        ) : null}
                        {spool.notes ? (
                          <details className="filament-library__notes">
                            <summary>Notes</summary>
                            <p>{spool.notes}</p>
                          </details>
                        ) : null}
                      </span>
                    </th>
                    <td>
                      <span className="filament-library__material">
                        <strong>{spool.material}</strong>
                        {nozzleTemperature ? (
                          <small>Nozzle {nozzleTemperature}</small>
                        ) : null}
                      </span>
                    </td>
                    <td>
                      <span
                        className={`filament-status ${
                          spool.available
                            ? "filament-status--available"
                            : "filament-status--unavailable"
                        }`}
                        aria-label={
                          spool.available ? "Available now" : "Not available"
                        }
                      >
                        {spool.available ? (
                          <CheckCircle2 aria-hidden="true" />
                        ) : (
                          <CircleOff aria-hidden="true" />
                        )}
                        {spool.available ? "Available now" : "Not available"}
                      </span>
                    </td>
                    <td>
                      <div className="filament-library__actions">
                        <button
                          className="button button--secondary button--compact"
                          type="button"
                          aria-label={`${
                            spool.available
                              ? "Mark unavailable"
                              : "Mark available"
                          }: ${spool.name}`}
                          onClick={() =>
                            onAvailabilityChange(spool.id, !spool.available)
                          }
                        >
                          {spool.available ? (
                            <CircleOff aria-hidden="true" />
                          ) : (
                            <CheckCircle2 aria-hidden="true" />
                          )}
                          {spool.available
                            ? "Mark unavailable"
                            : "Mark available"}
                        </button>
                        {spool.source === "user" ? (
                          <>
                            <button
                              ref={(element) => {
                                if (element) {
                                  editButtonRefs.current.set(spool.id, element);
                                } else {
                                  editButtonRefs.current.delete(spool.id);
                                }
                              }}
                              className="button button--secondary button--compact"
                              type="button"
                              aria-label={`Edit ${spool.name}`}
                              onClick={() => {
                                setEditingSpoolId(spool.id);
                                setDeleteCandidate(null);
                              }}
                            >
                              <Pencil aria-hidden="true" />
                              Edit
                            </button>
                            <button
                              ref={(element) => {
                                if (element) {
                                  deleteButtonRefs.current.set(
                                    spool.id,
                                    element,
                                  );
                                } else {
                                  deleteButtonRefs.current.delete(spool.id);
                                }
                              }}
                              className="button button--secondary button--compact"
                              type="button"
                              aria-label={`Delete ${spool.name}`}
                              onClick={() => {
                                setDeleteError("");
                                setDeleteCandidate({ ...spool });
                              }}
                            >
                              <Trash2 aria-hidden="true" />
                              Delete
                            </button>
                          </>
                        ) : (
                          <small className="filament-library__built-in-note">
                            Built-in details cannot be edited or deleted. Mark
                            it unavailable to exclude it from planning.
                          </small>
                        )}
                      </div>
                    </td>
                  </tr>
                );
              })
            )}
          </tbody>
        </table>
      </div>

      {editingSpool?.source === "user" ? (
        <div className="filament-library__edit-panel">
          <SpoolInventoryForm
            key={editingSpool.id}
            idPrefix={`${instanceId}-library-edit-${editingSpool.id}`}
            spoolCount={stockCount}
            onAddSpool={(input) => {
              onUpdateSpool(editingSpool.id, input);
              finishEditing(editingSpool.id);
            }}
            heading={`Edit ${editingSpool.name}`}
            description="Update this user spool's metadata and slicer profile. Changing its lot or calibration reference safely retires earlier measured CMY+X evidence. Availability is controlled from the table."
            summaryLabel="Edit spool details"
            initialValues={editInput(editingSpool)}
            initiallyOpen
            focusOnMount
            submitLabel="Save changes"
            onCancel={() => finishEditing(editingSpool.id)}
          />
        </div>
      ) : null}

      <dialog
        ref={deleteDialogRef}
        className="filament-delete-dialog"
        aria-labelledby={`${instanceId}-delete-dialog-heading`}
        aria-describedby={`${instanceId}-delete-dialog-description`}
        onCancel={(event) => {
          event.preventDefault();
          cancelDelete();
        }}
      >
        {deleteCandidate ? (
          <div className="filament-delete-dialog__content">
            <span className="filament-delete-dialog__icon" aria-hidden="true">
              <Trash2 />
            </span>
            <div>
              <h3 id={`${instanceId}-delete-dialog-heading`}>
                Delete user spool?
              </h3>
              <p id={`${instanceId}-delete-dialog-description`}>
                <strong>{deleteCandidate.name}</strong> will be permanently
                removed from this filament library. This cannot be undone.
              </p>
              <div className="filament-delete-dialog__spool">
                <ColorSwatch hex={deleteCandidate.hex} size="large" />
                <span>
                  <strong>{deleteCandidate.colorName}</strong>
                  <small>
                    {deleteCandidate.material} · {deleteCandidate.hex}
                  </small>
                </span>
              </div>
              {deleteError ? (
                <p className="filament-delete-dialog__error" role="alert">
                  {deleteError}
                </p>
              ) : null}
              <div className="filament-delete-dialog__actions">
                <button
                  ref={deleteCancelRef}
                  className="button button--secondary"
                  type="button"
                  autoFocus
                  disabled={isDeleting}
                  aria-label={`Cancel delete ${deleteCandidate.name}`}
                  onClick={cancelDelete}
                >
                  Cancel
                </button>
                <button
                  className="button button--danger"
                  type="button"
                  disabled={isDeleting}
                  aria-label={`Confirm delete ${deleteCandidate.name}`}
                  onClick={() => void confirmDelete()}
                >
                  {isDeleting ? "Deleting…" : "Delete spool"}
                </button>
              </div>
            </div>
          </div>
        ) : null}
      </dialog>
    </section>
  );
}
