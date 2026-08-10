# Bambu Lab A1 mini baseline capture

The A1 mini writer must be baseline-first. It must start from projects created
by the supported Bambu Studio GUI and must never convert a P1S, Snapmaker U1,
or other printer project by replacing only the printer name.

This procedure defines the golden evidence required before A1 mini 3MF export
can be enabled in a production build. The current single-plate adapter
completed independent PLA and PETG GUI round trips in Bambu Studio 02.02.00.85
on 3 August 2026; its hash-bound gate is now qualified.

## Frozen target configuration

All fixtures must use this target configuration:

- printer: **Bambu Lab A1 mini**;
- build volume: **180 x 180 x 180 mm**;
- nozzle: **one 0.4 mm nozzle**;
- build plate: **Bambu Textured PEI Plate**;
- material delivery: **one external spool, without AMS Lite**;
- process: the official **0.20 mm Standard** A1 mini preset;
- output: unsliced project 3MF that Bambu Studio can reopen and slice.

Use one exact Bambu Studio version and build for the whole fixture set. Record
the complete version string before capture and keep the fixtures under:

```text
fixtures/a1-mini-<bambu-studio-version>/
```

`Latest`, `current`, and mixed-version fixtures are not valid compatibility
identifiers. If the application, machine preset, process preset, or filament
preset changes, capture and qualify a new versioned fixture set.

Do not connect AMS Lite while capturing. If the workstation already has an AMS
configuration, explicitly select the external spool and verify that the plate
uses one physical filament slot. Do not add a second filament slot to any of
the fixtures.

## Required golden fixtures

For the current single-plate capability, create independent PLA and PETG
projects from a new, empty A1 mini project in Bambu Studio. A separate
multi-plate fixture is retained for a future native multi-plate capability and
does not gate the current one-output-file-per-plate writer.

| File | Material preset | Plates | Required contents |
|---|---|---:|---|
| `a1-mini-pla-solid-baseline.3mf` | Official Generic PLA preset for A1 mini 0.4 mm | 1 | One 20 mm cube centered on the plate |
| `a1-mini-petg-solid-baseline.3mf` | Official Generic PETG preset for A1 mini 0.4 mm | 1 | One 20 mm cube centered on the plate |
| `a1-mini-pla-multi-plate-baseline.3mf` | The same PLA preset as the PLA solid fixture | 2 | Future `a1_multi_plate` evidence; not part of the qualified single-plate gate |

The multi-plate fixture must use the same one-spool PLA configuration on both
plates. Its purpose is to establish Bambu Studio's plate, relationship,
thumbnail, and per-plate configuration layout. It is not an AMS or multi-color
fixture.

Use a controlled filament color in each project and record its exact HEX value.
Color is fixture metadata, not evidence that an arbitrary physical spool has a
qualified temperature or flow profile.

## GUI capture procedure

Repeat this procedure independently for every fixture:

1. Start Bambu Studio and record its complete version and build number.
2. Create a new project. Do not open or clone a P1S, U1, or customer project.
3. Select `Bambu Lab A1 mini`, the 0.4 mm nozzle, and `Bambu Textured PEI Plate`.
4. Select the official 0.20 mm Standard process preset for that target.
5. Select the required A1 mini PLA or PETG filament preset.
6. Disable AMS use and confirm that the project contains one external-spool
   filament definition.
7. Add the fixture geometry and plates listed above. Keep every object inside
   the bed polygon with normal slicer clearance.
8. Save the project through the GUI as `<fixture-name>.initial.3mf`.
9. Run `Slice Plate` for a single-plate fixture or `Slice All` for the
   multi-plate fixture. Treat any repair dialog, unsupported-printer warning,
   missing-profile warning, out-of-bounds warning, or slicing error as a
   failure.
10. Save, close the project, and fully exit Bambu Studio.
11. Reopen `<fixture-name>.initial.3mf` in the same Bambu Studio build.
12. Verify the printer, nozzle, build plate, process, material, filament color,
    plate count, object placement, and external-spool assignment.
13. Slice again. For the multi-plate fixture, inspect and slice both plates.
14. Save As the final filename from the fixture table, close it, reopen it one
    more time, and slice it again.
15. Record the result and SHA-256 hashes in `BASELINE.json`. Keep both the
    initial and final files so the package diff can be reviewed.

The final file, not the initial save, is the writer baseline candidate.
Headless export, command-line conversion, and a project that has not survived
the GUI save-close-reopen-slice cycle are not acceptable golden baselines.

## Preset identity and hashes

Record both the selected preset identity and the resolved settings used to
create each fixture. Display names alone are insufficient because they may be
localized or reused by later versions.

For the machine, process, and filament presets, record:

- the exact UI display name;
- `setting_id` or other stable internal identifier stored by Bambu Studio;
- the complete inheritance chain;
- the path relative to the Bambu Studio resources or user-preset root;
- SHA-256 of every source preset file in that inheritance chain;
- SHA-256 of the fully resolved, canonically serialized settings object;
- whether the preset is system-provided or user-modified.

The fixture set is invalid if any selected preset is an unsaved user override.
If a vendor preset must be cloned, give it a versioned name, export it with the
fixture evidence, and hash the exported file and its resolved settings.

At minimum, `BASELINE.json` must contain this information:

```json
{
  "schemaVersion": 1,
  "target": {
    "printer": "Bambu Lab A1 mini",
    "buildVolumeMm": [180, 180, 180],
    "nozzleDiameterMm": 0.4,
    "buildPlate": "Bambu Textured PEI Plate",
    "amsLite": false,
    "activeFilamentSlots": 1
  },
  "application": {
    "name": "Bambu Studio",
    "version": "<exact version and build>",
    "applicationSha256": "<sha256>"
  },
  "presets": {
    "machine": {
      "displayName": "<exact name>",
      "settingId": "<exact id>",
      "sourceSha256": ["<sha256 for each inherited file>"],
      "resolvedSha256": "<sha256>"
    },
    "process": {
      "displayName": "<exact name>",
      "settingId": "<exact id>",
      "sourceSha256": ["<sha256 for each inherited file>"],
      "resolvedSha256": "<sha256>"
    },
    "filaments": {
      "pla": {
        "displayName": "<exact name>",
        "settingId": "<exact id>",
        "sourceSha256": ["<sha256 for each inherited file>"],
        "resolvedSha256": "<sha256>"
      },
      "petg": {
        "displayName": "<exact name>",
        "settingId": "<exact id>",
        "sourceSha256": ["<sha256 for each inherited file>"],
        "resolvedSha256": "<sha256>"
      }
    }
  },
  "fixtures": [
    {
      "file": "a1-mini-pla-solid-baseline.3mf",
      "initialSha256": "<sha256>",
      "finalSha256": "<sha256>",
      "plateCount": 1,
      "material": "PLA",
      "filamentHex": "<#RRGGBB>",
      "saveSucceeded": true,
      "closeSucceeded": true,
      "reopenSucceeded": true,
      "sliceSucceeded": true,
      "secondSaveSucceeded": true,
      "secondReopenAndSliceSucceeded": true,
      "warnings": []
    }
  ]
}
```

Do not store personal absolute paths, printer serial numbers, LAN addresses,
cloud account identifiers, access tokens, or customer geometry in the evidence.

## Metadata isolation rule

The A1 mini adapter must construct output from an approved A1 mini baseline.
It must never copy P1S or Snapmaker U1 machine metadata into an A1 project.

In particular, never copy source values for:

- printer model, variant, printable area, excluded areas, or height limits;
- machine limits, kinematics, nozzle count, or toolhead offsets;
- start, end, layer-change, pause, or tool-change G-code;
- AMS mappings, purge/flush matrices, ramming, or multi-tool settings;
- build plate compatibility expressions;
- device identifiers, network metadata, calibration state, or cloud metadata;
- stale sliced data, G-code, thumbnails, time estimates, or material estimates.

Only explicitly whitelisted print intent may be transferred after validation,
such as a supported layer height, wall count, infill intent, support intent, or
brim intent. Machine, process, plate, and filament values come from the A1 mini
baseline and its approved presets.

Bambu Studio may itself serialize fields whose names mention AMS even when AMS
is not active. Do not delete or invent such fields based on their names. The
golden fixture defines their target structure; validation must prove that the
effective project configuration uses one external spool and no AMS Lite.

## Packing gate

A valid baseline does not make an object fit the A1 mini. Before the writer is
allowed to use a planned A1 assignment, the production packing implementation
must independently prove all of the following against the frozen target
profile:

- transformed object bounds, brim, skirt, and configured clearance fit the
  180 x 180 mm bed polygon;
- height, including required clearance, does not exceed 180 mm;
- every packed object remains inside the printable polygon and avoids excluded
  areas;
- objects do not overlap after placement;
- orientation and mechanically relevant transforms are preserved;
- the packed plate passes the same structural checks after serialization.

A source plate that is too large must be repacked or split only at existing
printable-unit boundaries. The writer must not scale geometry, rotate a part,
or cut a single part merely to make it fit.

Until both this packing gate and the GUI golden baseline qualification pass,
the UI may plan or strictly pin a mono unit to A1 mini, but it must not label an
A1 3MF as printer-ready.

## Qualification checklist

A fixture set is accepted only when all of these checks pass:

- all ZIP paths and sizes pass the bounded OPC/ZIP reader;
- all OPC relationships resolve and no required entry is missing;
- the expected model, object, instance, and plate counts are preserved;
- machine, process, filament, and whole-project hashes are recorded;
- the effective target is A1 mini with one 0.4 mm nozzle and no AMS Lite;
- no P1S or U1 machine profile, machine G-code, toolhead definition, or bed
  geometry remains;
- independent PLA and PETG fixtures each pass GUI save, full application close,
  reopen, slice, save again, reopen again, and slice again;
- any future multi-plate capability has its own evidence proving that all
  plates remain independent;
- Bambu Studio shows no repair dialog or target/profile warning;
- a package diff has identified target-owned fields and the small whitelist of
  transferable print-intent fields;
- the manual run record includes screenshots or logs and the operator/date.

Any failed item blocks the production A1 mini writer. Structural validation is
required, but it does not replace the Bambu Studio GUI round trip.

## Qualification candidate CLI

The qualification command builds its canonical `PlanningInput` and
`PlanningResult` through the same application planning flow used by the
desktop application. It does not accept frontend-authored plate or placement
JSON.

First inspect the exact local installation:

```sh
cargo run -p u1-converter-cli -- doctor-a1 \
  --bambu-app "/Applications/BambuStudio.app"
```

Prepare a normal planning-options JSON file. It must enable `a1_mini`, include
an available physical PLA or PETG spool that is not reserved or loaded on the
U1, and route at least one source unit to `a1_mini`. Use `u1-converter plan`
first to obtain stable `source_unit_id` values. Then build the candidates into
an existing empty directory:

```sh
mkdir -p "/absolute/path/a1-qualification-output"
cargo run -p u1-converter-cli -- build-a1-qualification \
  "/absolute/path/source-fixture.3mf" \
  --options "/absolute/path/a1-qualification-options.json" \
  --output "/absolute/path/a1-qualification-output" \
  --bambu-app "/Applications/BambuStudio.app"
```

The command fails closed when the application planner reports hard errors,
when A1 placements are not fully packed, when source identity changes, or when
the exact application/profile hashes differ. It uses a deliberately separate
qualification-candidate API and never treats that bypass as a production
conversion. Existing output files are never overwritten.

Validate each generated unsliced candidate independently before opening it in
Bambu Studio:

```sh
cargo run -p u1-converter-cli -- validate-a1 \
  "/absolute/path/a1-qualification-output/candidate.3mf"
```

`doctor-a1` reports `qualified` and `conversionAvailable: true` only when the
exact supported installation, effective profile pack, embedded typed report,
and report hash all match. It exits with an error for an unsupported or
tampered installation/evidence bundle.

The qualified record is
`crates/a1mini-adapter/qualification/a1mini-02.02.00.85.json`; it binds the
exact bytes of
`crates/a1mini-adapter/qualification/a1mini-02.02.00.85-report.json`. The report
in turn binds the PLA and PETG source, writer candidate, first GUI save, and
reopened GUI save SHA-256 values plus every required profile hash. Changing
only a status boolean, any artifact hash, the report bytes, or their ordering
fails closed.
