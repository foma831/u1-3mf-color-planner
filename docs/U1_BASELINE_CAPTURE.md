# Snapmaker Orca 2.3.5 baseline capture

The U1 writer must not reuse Bambu printer settings or treat a headless export
as a known-good project. Qualification must use the Snapmaker Orca GUI and the
exact 2.3.5 installation accepted by the adapter's version, executable-hash,
and profile-hash gates.

Stage B completed its exact Snapmaker Orca 2.3.5 GUI qualification on
2026-08-03. Its release record and separate typed report are hash-bound with
`status=qualified`; the Direct writer is available only when the local
executable and effective installed profile pack match that evidence.

## Stage B Direct candidate

Build a qualification candidate through the deliberately separate example
entry point. The desktop application cannot invoke this bypass:

```sh
mkdir -p "/path/to/qualification-output"
cargo run -p u1-application --example u1_direct_qualification -- \
  "Sample/Sailfin Dragon - Articulated Lizard by Raki-Box.3mf" \
  "/path/to/qualification-output" \
  "/Applications/Snapmaker Orca.app"
```

Use a destination where the expected `<source>__converted` directory does not
already exist; publication is intentionally no-clobber.

The qualified Sailfin candidate passes the native structural,
geometry-fingerprint, plate-membership, slot-remap, target-setting, and strict
bundle validators. It also completed the required two GUI save cycles described
below; the candidate and both GUI saves share one semantic digest under the
version-scoped validator.

## Qualified evidence

- source fixture SHA-256:
  `1b20d6124353d3bc31c4ea554e482ba6f8ef281dd23e2df6bbc0d561b4f6e0d7`;
- writer candidate SHA-256:
  `7bd3668d143dabc57643b820c49b9ba1130534b2b1d5555e4a7c8fe80059aa30`;
- first GUI save SHA-256:
  `1796ca7e2c18ad89579e13024197de8e0f84a715d806d39fbed18b538178fb31`;
- reopened/re-sliced GUI save SHA-256:
  `5a7cca46b77650b6c86328e1b287cbf2f87ab0007c48e77970222da83d1295b3`;
- shared semantic SHA-256:
  `2886230c0f2a30c47f288aeed1eecaf84363fa1dcc6c1ee96acf5e9836f9a35e`;
- qualification report SHA-256:
  `41d62acb065f70551934c150eebda25ee3144e1a95d3bf7ad1c8285d8cb23f69`.

Both slices completed with four toolheads, a printed prime tower, and 372
filament changes. The only visible notices concerned two meshes with more than
one million triangles; they were performance/simplification suggestions, not
repair, missing-profile, custom-profile, or incompatible-profile warnings.

## Stage B target contract to inspect

### Project globals and qualified quality overrides

The candidate starts from the exact U1 0.20 Standard target decisions:

- `curr_bed_type=Textured PEI Plate`;
- `print_sequence=by layer`;
- first- and other-layer sequence arrays containing only `0`, with sequence
  count `0`;
- `spiral_mode=0` and `spiral_mode_smooth=0`;
- `timelapse_type=0`, meaning traditional timelapse.

These are target-owned settings, not inherited Bambu intent. Equivalent source
per-plate values are normalized and reported. A duplicate, missing value,
conflicting value, or non-empty nested process override is a preparation error;
it must never be silently discarded. This includes `timelapse_type`: only `0`
is equivalent to the Stage-B traditional-timelapse target.

Direct Spools output may then apply qualified project-level quality overrides.
The selected layer height must remain in the 0.08–0.32 mm U1 range and may not
be coarser than the source. Wall speeds and outer-wall acceleration may only be
reduced from the target baseline, shell counts may only increase, and the
source `classic`/`arachne` wall generator is retained. Enabled support top and
bottom Z gaps track the selected layer height. Every applied key must be listed
in the first `different_settings_to_system` group so Orca cannot silently
restore a more aggressive system-preset value.

### Instance identity

Every selected `model_instance` must retain its source Orca `identify_id` in the
writer candidate. The writer fails closed when a selected source instance has
no `identify_id`; it must not manufacture or renumber one.

Stock Snapmaker Orca 2.3.5 normal **Save Project** intentionally serializes a
process-local instance ID instead of the imported `identify_id`. Those numbers
may therefore change during each clean GUI session and are not a persistent
project identity. GUI round-trip qualification must not claim numeric equality.
Instead, it requires a one-to-one semantic instance mapping by object/instance
membership, mesh digest, name, transform, bounds, extruder assignment, and
plate membership. Every GUI-assigned ID must also be positive, unique inside
its project, and internally consistent.

### Prime tower

For every multicolor plate, interpret `wipe_tower_x/y` as the lower-left corner
of the unrotated tower body, not its center. Verify the actual printed tower and
the conservative reservation used by the writer:

- 30 mm body width and 45 mm worst-case depth;
- a 15-degree stabilization cone evaluated at the plate's actual printed
  height (`max Z`), not at a fixed nominal height;
- 5 mm prime-tower brim;
- the complete 8 mm rib extension from the qualified ribbed profile;
- 1 mm tower clearance;
- 19 mm clearance around each printable object footprint: the writer pins
  `auto_brim` explicitly, and its Orca algorithm may grow to 18 mm, plus the
  qualified 1 mm object gap.

The complete cone/depth/brim/rib/clearance envelope must remain inside the U1
bed and must not overlap an expanded object footprint. A mono plate does not
print a prime tower, so it does not require this collision reservation.
The merged project profile must explicitly provide finite, non-negative
`brim_width` and `brim_object_gap` values at or below 5 mm and 1 mm. Retained
object/part overrides have the same bounds; malformed, negative, missing target
defaults, or larger values are outside the proof and must block conversion.
The wider 19 mm placement envelope additionally covers the explicitly pinned
`auto_brim` algorithm's 18 mm cap. Other object/part metadata is fail-closed:
the writer retains only its qualified identity/internal-print allowlist, known
bounded brim types, and target-equivalent disabled/zero support, raft, and
XY-compensation values. Painted brim is unsupported because its point sidecar
is not transferred. An unknown process override requires a new footprint
qualification.

### Plate filament map

The generated plate block must match the exact Snapmaker Orca 2.3.5 exporter
contract: `filament_map_mode="Auto For Flush"` and one literal `1` for each of
the four T1–T4 project filaments. Verify all four values after both GUI saves;
the native staged-output validator checks mode, cardinality, and plate IDs.

### System profiles

Stage B accepts only the exact hash-pinned Snapmaker Orca 2.3.5 effective system
profiles. Snapmaker Orca loads the active vendor pack from its application-data
`system/Snapmaker` directory after profile updates; bundled `.app/Resources`
profiles are only the seed. The writer must inspect and use the same effective
pack that the GUI will load, including its vendor-pack version and manifest.
It intentionally writes zero `process_settings_N.config`,
`filament_settings_N.config`, and `machine_settings_N.config` entries. The GUI
must open the candidate without missing/custom/incompatible-profile warnings,
and neither GUI save may inject an embedded preset entry.

## Required GUI sequence

Using the exact accepted Snapmaker Orca 2.3.5 installation:

1. Hash the writer candidate before opening it.
2. Open it and verify the U1 0.4 machine, target globals, plate/object count,
   writer-preserved source `identify_id` values, physical T1-T4 mapping, and
   the effective system profiles.
3. Confirm that no repair, incompatible-profile, missing-profile, or
   custom-profile warning appears.
4. Run **Slice All** and inspect tool mapping and every printed prime tower.
5. Save As a first new 3MF and hash those exact bytes.
6. Close the project and application.
7. Reopen the first GUI-saved 3MF and repeat the machine, globals, plate,
   semantic instance identity, positive/unique GUI ID, profile, mapping,
   warning, and prime-tower checks.
8. Run **Slice All** again, Save As a second new 3MF, and hash those exact bytes.
9. Structurally inspect both GUI-saved files. Confirm stable plate/object counts,
   target globals, the semantic instance bijection, T1-T4/profile identities,
   prime-tower coordinates, and zero embedded preset entries. Numeric GUI
   `identify_id` equality with the source is not a valid requirement.
10. Fill the typed report first. Only after independent review, hash its exact
    bytes and bind that hash into the release record.

Headless CLI slicing may be recorded as an additional smoke test, but it cannot
replace any GUI step.

## Typed qualification evidence

Qualification uses two strict documents. Do not add fields: both schemas reject
unknown keys.

### Detailed report

Fill
`crates/orca-adapter/qualification/u1-direct-2.3.5-report.json` first. It must
contain:

- schema version, adapter ID, application version, and exact executable hash;
- `profileSource=installed_system`, exact profile-pack version
  `02.02.53.02`, and exact SHA-256 of the effective `Snapmaker.json` manifest;
- `status=qualified` and a non-empty note;
- exact fixture name/hash, writer-candidate hash, first GUI-saved hash, reopened
  GUI-saved hash, seven GUI step booleans, and a non-empty UTC timestamp;
- every required profile `relativePath` and SHA-256 in exactly the same count
  and order as the adapter baseline;
- both auxiliary runtime-table `relativePath`/SHA-256 pairs in exactly the same
  count and order as the auxiliary baseline;
- all of these typed checks set to `true`:
  - `machineProfileLoaded`;
  - `processProfileLoaded`;
  - `t1T4MappingVerified`;
  - `primeTowerVerified`;
  - `firstGuiSavedStructurallyValid`;
  - `reopenedGuiSavedStructurallyValid`;
  - `geometryAndPlacementsStable`;
  - `targetGlobalsStable`;
  - `writerCandidateIdentifyIdsPositiveAndUnique`;
  - `writerCandidateSourceIdentifyIdsPreserved`;
  - `guiIdentifyIdsUnique`;
  - `instanceIdentityBijectionStable`;
  - `plateCountStable`;
  - `objectCountStable`;
  - `sliceCompletedWithoutRepairWarning`;
  - `noIncompatibleProfileWarning`;
  - `noMissingProfileWarning`;
  - `noCustomProfileWarning`;
  - `noEmbeddedPresetsAfterFirstSave`;
  - `noEmbeddedPresetsAfterReopenedSave`;
  - `derivedProfileIdentityStable`.

The typed booleans explicitly record target-global stability, exact source-ID
preservation by the writer, and semantic identity across GUI canonicalization.
The report note should summarize the comparison context. Keep
screenshots or operator notes outside the strict JSON report and reference them
through the reviewed qualification process; do not add ad-hoc JSON fields.

### Release record

Only after the report is final, update
`crates/orca-adapter/qualification/u1-direct-2.3.5.json`:

- set `guiRoundTripPassed=true` and `status=qualified`;
- repeat the exact effective profile source, pack version, and manifest hash;
- repeat the exact fixture, candidate, GUI-saved hashes, step flags, and UTC
  timestamp from the report;
- set `qualificationReportSha256` to the SHA-256 of the exact report file bytes;
- provide a non-empty release note.

The gate independently parses both typed documents, hashes the report bytes,
compares all shared evidence, checks the exact ordered profile baseline, and
requires every typed check. Changing only a status or boolean cannot unlock
production.

Do not put personal paths, printer network addresses, cloud account data, or
proprietary geometry into either qualification document.

## Full Spectrum fixture (Stage D)

The Direct qualification above does not qualify Full Spectrum. Full Spectrum
therefore completed its own separate six-plate GUI round trip on 3 August 2026.
The reusable procedure remains:

1. Start a new **Snapmaker U1 (0.4 nozzle)** project with the official
   **0.08 mm Full Spectrum** process.
2. Configure T1-T4 with the official
   **Snapmaker PLA Full Spectrum @U1 0.4 nozzle** profile.
3. Assign Cyan `#08ABFB`, Magenta `#D93B90`, Yellow `#F9ED3D`, and Grey
   `#9199A4` to T1-T4 respectively.
4. Import a simple public model with no private metadata.
5. Add at least one mixed Full Spectrum definition and one solid T4 region, and
   assign both to printable geometry.
6. Slice, save, close, reopen, slice again, and save a second time.

This fixture has its own schema-v2 adapter evidence and does not reuse the
Stage B Direct report. The Full Spectrum report qualifies native project
structure, recipe preservation, and Snapmaker Orca interoperability. It does
not claim physical color accuracy: a production recipe still carries per-user
measured calibration provenance for the exact CMY+X loadout or an explicit
fingerprint-bound color approval from the planner.

## Supplemental checks

After capture, run:

```sh
cargo run -p u1-converter-cli -- doctor
cargo run -p u1-converter-cli -- validate-output "/path/to/writer-candidate.3mf"
cargo run -p u1-converter-cli -- validate-u1-gui-round-trip \
  --source "Sample/Sailfin Dragon - Articulated Lizard by Raki-Box.3mf" \
  "/path/to/writer-candidate.3mf" \
  "/path/to/first-gui-save.3mf" \
  "/path/to/reopened-gui-save.3mf"
```

`validate-output` intentionally remains a strict unsliced-output gate. A normal
Snapmaker Orca GUI save adds freshly generated slice metadata and previews and
uses a known 2.3.5 Content Types quirk, so it must be checked only by the
version-scoped three-file validator. That validator does not allow G-code,
checksums, unknown derived parts, dangling preview references, or embedded
presets.

The adapter is accepted only after the native validators and the complete
manual GUI sequence both pass and the two hash-bound qualification documents
are installed. Those conditions are now satisfied for the exact qualified
Snapmaker Orca 2.3.5 installation and effective profile pack.
