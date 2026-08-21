# U1 3MF Color Planner

A local desktop tool for analyzing Bambu/Orca project 3MF files, planning Snapmaker U1 print batches, comparing CMY+X Full Spectrum recipes with direct spool assignments, and routing eligible mono jobs to a Bambu Lab A1 mini.

The product UI, CLI, source code, logs, and generated machine-readable reports are written in English. The project specification and design discussion are maintained in Russian.

Практический запуск и полный операторский процесс описаны в [русском руководстве пользователя](docs/USER_GUIDE_RU.md).

## Quick start

Build and open the current local Apple Silicon application:

```sh
npm install
npm run build
open "target/release/bundle/macos/U1 3MF Color Planner.app"
```

The generated `.app` and `.dmg` are local/internal artifacts. The bundle uses
an ad-hoc signature and is not Developer ID signed or notarized for external
distribution.

## Implemented vertical slice

The repository currently provides:

1. a bounded, streaming ZIP/OPC analyzer that never extracts the source project;
2. Bambu/Orca object, part, instance, material, plate, TriangleSelector paint, and transformed AABB analysis;
3. deterministic U1 planning for CMY+T4 batches, Direct Spools, setup actions, and conservative A1 mini routing;
4. CIEDE2000 comparison with nominal-versus-measured calibration confidence;
5. a version and profile fingerprint gate for Snapmaker Orca 2.3.5;
6. an English Tauri/React UI with separate Print Plan, persistent Filament Library, and guided Print Run views;
7. actionable source-to-nearest color decisions with candidate-bound approval, dedicated-spool suggestions, and separate material-substitution consent;
8. explicit A1 mini recalculation with independent U1 and A1 mini plate queues;
9. native and browser JSON plan export, with native export bound to the latest backend-validated plan;
10. a versioned local filament catalogue with explicit In stock / Out of stock status and in-stock-only planning;
11. persisted target-plate progress with blocking T4, full Direct T1–T4 setup, and post-batch restore checkpoints between print jobs;
12. explicit per-mono-plate `Auto / Snapmaker U1 / Bambu Lab A1 mini` intent, stored by stable source-unit identity;
13. inventory-first forced color replacement, including an explicit mechanical-risk acknowledgement for polymer changes;
14. a project-wide Direct Spools palette for projects needing at most four
    physical Direct identities across all included source scopes, including
    role-separated source-to-CMY+X-to-physical-spool comparison, deliberate
    many-to-one reduction, and pair-specific material-risk acknowledgement;
15. a type-aware native 3MF package foundation with deterministic OPC/ZIP
    staging, Production Extension graph construction, stale-artifact
    classification/rejection, structural validation, immutable source snapshots,
    source SHA-256 binding, and no-clobber atomic publication;
16. a CLI for analysis, planning, Orca compatibility diagnostics, and
    structural validation of unsliced output packages;
17. a GUI-qualified Stage B writer for unsliced Snapmaker U1 0.4 Direct Spools
    projects, gated to the exact Snapmaker Orca 2.3.5 executable and effective
    installed profile pack used during qualification;
18. explicit Stage B U1 process globals, safe per-plate override normalization,
    source `identify_id` preservation, and a printed-height-aware conservative
    prime-tower placement envelope;
19. a deterministic A1 mini 0.4 single-plate writer with one external
    PLA/PETG spool, no-AMS semantics, independent 180 × 180 × 180 mm packing,
    and strict target-contract validation;
20. a deterministic Snapmaker U1 Full Spectrum writer with physical CMY+X
    loadouts, virtual mixed-filament definitions, solid-T4 assignments,
    subdivision/process globals, and active prime-tower validation;
21. mixed Direct / Full Spectrum / A1 mini desktop preflight and conversion,
    with one canonical artifact order, per-file loadouts and setup actions;
22. cooperative cancellation, private recovery records, startup cleanup,
    no-clobber atomic publication, and a relocatable checksummed mixed bundle;
23. exact-partition validation that rejects an omitted or doubly-routed batch,
    job, plate, or source unit before any writer runs.
24. an experimental bounded mesh-orientation optimizer that penalizes isolated
    support contacts, writes a no-clobber oriented source copy, and includes a
    reproducible comparison gate against Snapmaker Orca's native auto-orient.

The orientation experiment is available from the CLI:

```sh
u1-converter optimize-orientation model.3mf --object-id 122
u1-converter apply-optimized-orientation model.3mf oriented.3mf --object-id 122
u1-converter optimize-plate-orientations model.3mf --plate-id 1
u1-converter apply-optimized-plate-orientations model.3mf oriented.3mf \
  --plate-id 1 --adhesion-mode reliable
scripts/benchmark-orientation.sh Sample/Withered_Foxy.3mf 122
```

The desktop exposes plate-level optimization only after native project files
have been generated. In **Conversion complete**, enable **Optimize generated
plates**, select the actual plates found in each published 3MF, and save a
separate `-support-optimized.3mf` copy. The verified published bundle is never
modified, so its manifest and checksums remain valid.

Plate packing reserves an 18 mm process envelope on every side of each model
for generated supports and Auto Brim, in addition to the 2 mm model clearance.
Consequently, model geometry stays at least 19 mm from the printable-area edge
and neighboring model footprints stay at least 38 mm apart. A layout that fits
the meshes but not these process envelopes is rejected.

The optional desktop step offers Standard, Reliable, and Maximum bed-adhesion
policies. It records a bounded per-instance adhesion-risk assessment, writes the
selected brim/raft/slow-layer settings into the optimized project copy, and
uses a versioned `U1 Planner` project profile identity instead of relabeling or
overwriting an installed Snapmaker system preset.

For Snapmaker U1 projects with supports enabled, the v2 planner profile also
normalizes the support contract to `tree(auto)` with `tree_hybrid` style,
disables `support_on_build_plate_only`, uses three interface layers (four in
Maximum), 0.2 mm rectilinear-interlaced interface spacing, two tree walls, and
100/50 mm/s support/interface speeds. The top Z distance remains one project
layer. This avoids relying on the unsafe tree-interface combination preserved
by older v1 optimized copies.

When an optimized copy is analyzed again, the desktop restores its Standard,
Reliable, or Maximum selection only after all embedded brim, raft, slow-layer,
and first-layer speed values match the named versioned profile. A modified U1
Planner profile is reported as custom and defaults safely to Reliable; external
Orca profiles are not guessed from similar values.

Orientation changes are strictly opt-in; ordinary analysis, planning, and
conversion preserve source orientations. The plate command starts with the
best Pareto-safe candidate for every printable instance, runs one deterministic
packing pass, and explores at most 50 local repairs involving the failed,
largest-footprint, or tallest instances. A candidate may not worsen support
volume, small support-island count, or the combined score relative to that
instance's source orientation. The apply commands never overwrite their
destination. They rewrite only approved primary build-item transforms,
re-analyze the staged and published copy, and leave the source hash unchanged.
The result must still be reviewed and repacked before production conversion.

Both analysis and pre-publication validation cap ZIP expansion, XML depth, and
the size of a single XML lexical token. Package manifests can only be built from
the same staged bytes that passed the fixed strict-unsliced publication gate,
and their source identity must have been reverified by the writer. The writer
also rejects ambiguous EOCD/ZIP64 graphs before ZIP entry-table allocation and
publishes from an unexposed validated snapshot in a private staging directory.

The planner deliberately blocks rather than guessing when physical inventory,
geometry bounds, calibration context, or a reproducible Full Spectrum recipe is
missing. When a structurally valid nearest recipe exists outside the automatic
color threshold, the UI shows its predicted color, Delta E, recipe, and required
T4. The user may approve that exact fingerprinted candidate or add a physical
spool; PETG-to-PLA remains a separate mechanical-risk decision. Known bounds are
used conservatively; unknown or degenerate bounds stay on U1. For Direct Spools,
`Review conversion…` is available only when the plan is valid and every adapter
required by that specific mixed plan matches its exact executable, profile,
and qualification gate.

When the union of all included source scopes needs four or fewer physical
Direct identities, the UI also offers **Project-wide Direct Spools**. Semantic
rows remain separate by material, role, RGB, and source profile for CMY+X and
consent. Role-only duplicates with the same material, RGB, and explicitly
declared source profile share one physical identity, spool, and toolhead;
unknown profiles remain separate. The UI shows both counts. Each row keeps the
original source pair, every current CMY+X prediction, and the physical spool
selected for print time visible together. If the CMY+X result is different
between scopes, the row says **Varies by scope** and lists each result. Reusing
one in-stock spool for several other physical identities is an explicit
many-to-one color reduction and assigns that spool to one shared T1–T4
toolhead. A material change requires a separate acknowledgement for every
semantic source row. Applying the palette changes planning intent;
**Recalculate Plan** performs authoritative backend validation. Above four
physical Direct identities, the UI shows the exact semantic and physical
counts and reason instead of offering an invalid apply action.

This whole-project form works over the existing included source scopes. The
current release does not provide arbitrary user-created object groups.

## Native writer stages

| Stage | Scope | Current status |
|---|---|---|
| A | Generic bounded OPC/3MF writer and structural validator | Implemented |
| B | Snapmaker U1 0.4 Direct Spools | GUI-qualified for the exact Snapmaker Orca 2.3.5 executable and installed profile pack; conversion enabled when the plan is valid |
| C | Bambu Lab A1 mini single-spool output | GUI-qualified for independent PLA and PETG round trips in the exact Bambu Studio 02.02.00.85 installation |
| D | Snapmaker U1 CMY+X Full Spectrum output | GUI-qualified for native project structure and interoperability in exact Snapmaker Orca 2.3.5; physical color accuracy remains per-user calibration/approval evidence |

Stage B completed its exact GUI qualification on 3 August 2026. The desktop
checks the application version, executable SHA-256, effective installed vendor
pack, profile closure, auxiliary runtime tables, and the hash-bound GUI report;
on the qualified installation it reports `qualified` and permits Direct Spools
conversion for a valid canonical plan.

Stages C and D completed their exact GUI qualification on 3 August 2026. The
A1 validator proves one packed
plate, one external spool, PLA/PETG identity, no AMS, geometry, parts,
transforms, and placements across two GUI saves. The Full Spectrum validator
additionally proves physical T1–T4 identity, virtual definitions and decoded
paint assignments, solid T4 use, subdivision/process globals, and prime-tower
stability across its six-plate candidate and two GUI saves. Both release gates
are exact-byte hash-bound to their reports and still fail closed if the
application, profiles, record, report, or artifact identities change.

Full Spectrum writer qualification deliberately does not claim that a nominal
recipe reproduces a physically accurate color. Recipe accuracy remains bound
to a measured calibration sample for the exact CMY+X loadout, or to the
planner's explicit fingerprint-bound color approval. A GUI-qualified writer
therefore proves that Snapmaker Orca preserves and slices the requested recipe;
it does not turn nominal RGB estimates into measured results.

The exact Sailfin qualification candidate passed the native structural,
geometry, plate-membership, slot-remap, target-setting, and bundle validators,
then completed two clean Snapmaker Orca GUI open/slice/save cycles separated by
a full application quit. Candidate and both GUI saves have the same semantic
digest under the version-scoped round-trip validator. This GUI matrix directly
exercised the Polymaker General PLA leaf profile; Generic PLA and Generic PETG
are still hash-pinned and writer-validated, but were not separate GUI matrices
in this qualification record.

Stage B starts from the qualified U1 0.20 Standard profile and retains its
target-owned globals: Textured PEI Plate, by-layer sequencing, non-spiral mode,
and traditional timelapse. It then applies qualified source quality intent as
fail-closed project overrides: a finer supported layer height, the source wall
generator, wall-speed and acceleration ceilings, shell minimums, and support
gaps tied to the selected layer height. Equivalent source per-plate overrides
are normalized to target-owned globals and reported;
conflicting or ambiguous overrides are rejected instead of being silently
discarded, including a plate-level timelapse override. Each selected instance
also retains its source Orca `identify_id`.

For multicolor plates, `wipe_tower_x/y` is treated as the lower-left corner of
the unrotated tower body. Placement reserves a bed- and collision-checked
envelope using the plate's actual printed height, the stabilization cone, the
45 mm worst-case depth, brim, full rib extension, tower clearance, and object
clearance. A mono plate does not require this tower envelope.
The merged target process and every retained object-level `brim_width` and
`brim_object_gap` override are bounded to 5 mm and 1 mm. Tower placement uses a
19 mm object envelope because the explicitly pinned `auto_brim` can grow to
18 mm, plus the qualified 1 mm gap. Larger, missing target defaults, or
malformed values fail closed. Object/part metadata is retained only through an
explicit Stage B allowlist; unknown per-object process overrides (including support,
raft, or XY-expansion values outside their target-equivalent disabled/zero state)
block conversion instead of weakening the tower-collision proof. Painted brims
are rejected because Stage B does not transfer their point sidecar; bounded
automatic `brim_ears` remain supported.

Qualified global source support intent is different: `normal(auto)` and
`tree(auto)`, the threshold angle, and build-plate-only intent are transferred to
the target project and declared together with quality overrides in
`different_settings_to_system`. This keeps
merged target plates merged while letting the target slicer generate supports
for the combined geometry and only one prime tower.

The desktop mixed-bundle contract is:

```text
<safe-source>-<identity>__converted/
  u1-full-spectrum/
    <source>__U1__batch-01__Full-Spectrum.3mf
  u1-direct/
    <source>__U1__batch-02__Direct-Spools.3mf
  a1-mini/
    <source>__A1-mini__plate-01__spool.3mf
  manifest.json
  conversion-plan.json
  conversion-report.html
  checksums.sha256
```

Only target directories used by the plan are present. There is one unsliced
3MF per U1 batch/loadout and one per A1 mini target plate. `manifest.json`
records source identity, artifact hashes, target plate IDs, loadouts, setup
actions, and source-unit mappings. The backend verifies that this published
contract is identical to the approved preflight and that the staged directory
contains no undeclared file, nested directory, or symlink.
The schema-v2 manifest stores only the source leaf file name (never its host
path), exact source byte size and SHA-256, analyzed source
application/version/dialect, and the converter build identity. The build
identity always contains the Cargo package version and contains either the
compile-time `U1_PLANNER_GIT_IDENTITY` value or the deterministic literal
`Unknown`. Each physical-slot manifest record keeps `profile`, `settingId`, and
`filamentId` as separate identities. In project settings, `filament_ids` is
populated from the leaf system preset's `setting_id`, matching a normal
Snapmaker Orca 2.3.5 Save Project. The inherited material-family `filament_id`
remains separate manifest inventory metadata.
The adapter resolves profiles from the effective application-data
`system/Snapmaker` vendor pack used by the GUI and pins its manifest, version,
inheritance closure, and auxiliary compatibility tables; it does not mix those
bytes with an older bundled seed.
Each generated plate also uses Snapmaker Orca 2.3.5's exact `Auto For Flush`
compatibility mapping, with one literal `1` derived per T1–T4 project filament;
the staged output validator checks its mode, cardinality, and plate IDs.
`conversion-plan.json` stores the canonical backend plan, provenance, and
fingerprint without duplicating the artifact payload or the full manifest.
`conversion-report.html` is the deterministic offline rendering of
`manifest.json`. `checksums.sha256` covers every artifact and all three
metadata files. Publication is
atomic and no-clobber; the source 3MF is never copied into the result bundle.
Schema-v1 bundles are recognized during restart recovery but rejected
fail-closed because they lack the required provenance; the operator must
convert the current plan again.

Restart recovery additionally requires a backend-private trusted publication
receipt stored under the application-data `publication-receipts-v1` directory.
It is distinct from the frontend's local Print Run receipt: the frontend record
is only an untrusted lookup hint and cannot authorize recovery. The private
receipt binds the converter build, canonical source path/byte size/SHA-256,
plan fingerprint, canonical preflight hash, output path and directory identity,
and the relative path, byte size, and SHA-256 of every metadata file and 3MF
artifact. Recovery rechecks those identities and fails closed on any missing,
stale, moved, replaced, or modified evidence. If the trusted receipt is missing
or stale, choose another destination or move/remove the old untrusted bundle
before converting again; no-clobber publication never overwrites that path.
The checksummed bundle itself can still be copied and opened in its target
slicer, but backend Print Run recovery is deliberately bound to its original
published path and directory identity.

## Verified sample

`Sample/Withered_Foxy.3mf` currently analyzes as:

- 12 source plates;
- 89 objects and instances;
- 390 parts, of which 388 are printable positive parts;
- transformed bounds for all 388 printable parts, all 89 objects, and all 89 instances;
- 10 used source filament slots;
- 74 mono objects and 15 multi-color objects;
- 7,308,333 vertices and 14,616,548 triangles.

The real-project native UI contract test preserves the 12-source-plate count,
exposes detected joint alternatives as excluded choices, and never promotes
inferred source colors into confirmed physical inventory.

The accepted sample planning fixture produces 8 target plates in 3 U1 batches.
Its single T4 boundary is before Target Plate 03: Panchroma Translucent Grey is
replaced with Panchroma Basic Black after Target Plate 02 has finished.

Direct Spools intent is stored per source scope, independently of its current
U1 or A1 mini presentation row. Revalidating an A1-routed mono job therefore
does not erase the user's spool assignments. Native export rejects stale,
empty, or client-mutated plans and writes the backend's canonical validated
serialization after rechecking the source file identity.

The Filament Library is independent of the selected 3MF. Native builds save it
atomically in the application data directory; the browser demo uses local
storage. Out-of-stock entries remain visible in the catalogue but are excluded
from CMY+X, Direct Spools, T4, and A1 mini candidates. Changing the library
invalidates inventory-dependent approvals and requires replanning.

Filament Library schema v2 also stores optional vendor, product line, optical
descriptor, nozzle-temperature range, batch/lot, calibration reference, and
notes. Native migration publishes a new v2 file atomically while retaining the
v1 file as a recovery copy. Batch/lot or calibration-reference changes rotate a
separate physical calibration identity, so measured CMY+X evidence from an old
lot cannot be reused silently.

CMY+X Calibration Library schema v2 stores measurement provenance beside the
exact loadout, qualified process, recipe, and coupon geometry. Every new record
requires an RFC 3339 measurement instant, one explicit method (`Instrument Lab
→ sRGB`, `Instrument sRGB`, `Reliable manual sRGB`, or `Visual swatch
comparison`), and confirmation that the value came from a physical printed
swatch rather than a nominal preview. Instrument/reference details and operator
notes are optional. Native and browser migrations retain schema-v1 data as a
recovery copy and mark migrated records `legacy_unverified`; those records stay
visible but are excluded from measured color prediction until re-recorded with
complete provenance.

Print Run is an operator checklist, not printer control. It records completed
target plates and blocks the next job at every planned non-Keep setup action.
Pure T4 boundaries use an exact replacement-spool checkpoint; Direct Spools
show the complete T1–T4 transition, and post-batch restore actions must be
confirmed immediately after the batch. Progress is keyed by the source hash and
deterministic plan fingerprint, so a changed plan cannot reuse stale completion
marks.

## Development

Rust is managed with `rustup`. Repository npm scripts locate the active Cargo
toolchain automatically, including Homebrew rustup installations whose Cargo
proxy is not present in the interactive shell `PATH`.

```sh
npm install
npm run test:rust
npm run check
npm run dev
```

Run the deterministic repository verification gate before handing off a
change. It runs frontend Vitest tests, the TypeScript compiler, and the
production web build, then checks Rust formatting, Clippy warnings, all
workspace test targets, and Rust doctests using the locked dependency graph:

```sh
npm run verify
```

Analyze or plan from the CLI:

```sh
cargo run -p u1-converter-cli -- analyze Sample/Withered_Foxy.3mf
cargo run -p u1-converter-cli -- plan Sample/Withered_Foxy.3mf --fail-on-errors
cargo run -p u1-converter-cli -- doctor
cargo run -p u1-converter-cli -- validate-output converted-project.3mf
cargo run -p u1-converter-cli -- validate-a1-round-trip \
  candidate.3mf first-save.3mf reopened-save.3mf
cargo run -p u1-converter-cli -- validate-full-spectrum-round-trip \
  candidate.3mf first-save.3mf reopened-save.3mf
```

Confirmed physical spools and per-scope Direct assignments can be supplied as
JSON. Start from [examples/plan-options.example.json](examples/plan-options.example.json):

```sh
cargo run -p u1-converter-cli -- plan Sample/Withered_Foxy.3mf \
  --options examples/plan-options.example.json
```

The sample override is documentation only: its example requirement must be
changed to a material/color pair that the selected spool actually preserves.
Set `allow_u1_cross_source_repacking` to `true` only when compatible U1 units
from different source plates may share generated target plates. The default is
`false`; exact strategy, material, and process compatibility still apply, and
the source 3MF is never modified. Direct Spool partial loadouts may share a
target when every occupied T1-T4 position agrees. The planner promotes them to
their common four-slot superset before packing, while fast-mono work stays
separate and conflicting spools in the same toolhead never merge.

## Writer qualification record

Snapmaker Orca 2.3.5 can headlessly slice a plain STL for U1, but its headless
`--export-3mf` output on macOS is not a valid golden project: thumbnail
relationships are left dangling and reopening the generated project crashes the
CLI. Production writers therefore require versioned, GUI-saved golden projects
for each target and verification through save, close, reopen, slice, and save
again. No headless-generated archive is accepted as a baseline.

The supplied Sailfin Dragon U1 project is the qualified structural and
painted-mesh reference for the U1 Direct Spools writer. It does not contain a
serialized virtual Full Spectrum recipe. The converter built a Full Spectrum
qualification candidate from the canonical Withered Foxy plan, including
virtual mixed and solid-T4 assignments. Its exact six-plate candidate and two
GUI saves now form the Stage D writer evidence. The supplied A1 mini project
remains forensic input, while separately generated clean PLA and PETG
candidates and their GUI saves form the Stage C evidence; material/scale
changes from the forensic project are never inherited silently.

Build a Stage B qualification candidate with the deliberately separate example
entry point (the desktop application cannot call this bypass):

```sh
mkdir -p "/path/to/qualification-output"
cargo run -p u1-application --example u1_direct_qualification -- \
  "Sample/Sailfin Dragon - Articulated Lizard by Raki-Box.3mf" \
  "/path/to/qualification-output" \
  "/Applications/Snapmaker Orca.app"
```

For every generated candidate, use the exact accepted Snapmaker Orca 2.3.5
installation and record the following GUI sequence without skipping a step:

1. open the candidate and inspect the printer, plates, T1-T4 mapping, target
   globals, and source instance identities;
2. slice all plates, inspect each printed prime tower, and save the project;
3. close the project/application;
4. reopen the GUI-saved project and inspect the same mappings, globals, and
   identities;
5. slice all plates again, save it a second time, and structurally confirm that
   the re-save preserved the contract and did not inject embedded presets.

Qualification uses two separate, strict typed documents, both now qualified:

- `crates/orca-adapter/qualification/u1-direct-2.3.5.json` is the release gate;
- `crates/orca-adapter/qualification/u1-direct-2.3.5-report.json` is the detailed
  GUI report.

The gate record contains `schemaVersion`, `adapterId`, `applicationVersion`, the
exact `executableSha256`, effective profile source/pack/manifest identity,
`guiRoundTripPassed`, `status`, `note`, and evidence bound to the exact Sailfin
fixture, candidate, two GUI-saved files, seven GUI step flags, timestamp, and
`qualificationReportSha256`. That last value must equal the SHA-256 of the exact
report bytes. The report independently repeats the bound identities and GUI
steps, lists every required inheritance profile and auxiliary runtime table in
baseline order, and requires typed checks for profile loading, T1-T4 mapping,
prime tower, both GUI-saved files' structural validity, geometry/placement and
plate/object stability, target globals, writer source-ID preservation, valid
positive and globally unique writer IDs, valid GUI IDs, semantic instance
identity, warnings, zero embedded presets, and
derived profile identity. Unknown fields or any disagreement fail closed.
The hardening-specific report fields are
`firstGuiSavedStructurallyValid`, `reopenedGuiSavedStructurallyValid`,
`geometryAndPlacementsStable`, `targetGlobalsStable`, and
`writerCandidateIdentifyIdsPositiveAndUnique`,
`writerCandidateSourceIdentifyIdsPreserved`, `guiIdentifyIdsUnique`, and
`instanceIdentityBijectionStable`; all must be `true`. Stock Snapmaker Orca
2.3.5 may renumber process-local `identify_id` values during a normal GUI save,
so the GUI files are compared semantically rather than by those numbers.
Changing only the top-level boolean cannot unlock production.

Stage B intentionally writes zero process, filament, or machine
`settings_N.config` entries. The official Snapmaker Orca 2.3.5 exporter writes
such entries only for presets marked `is_project_embedded`; this adapter instead
accepts only exact hash-pinned system profiles. The structural validator rejects
any generated embedded-preset entry. GUI qualification must additionally prove
that the candidate opens without a missing/custom-profile warning and that a
GUI re-save does not inject embedded preset entries.

Negative Z is also an intentional supported case, not a blocker. Snapmaker
Orca's BuildVolume/PartPlate path supports sinking and clips geometry at Z=0.
The writer preserves negative Z without lifting the object, rejects an object
that is completely below the bed, and continues to validate XY bounds and
maximum Z conservatively.

The Direct Spools, Full Spectrum, and A1 mini GUI qualification matrices are
complete. Every production writer now supports cooperative cancellation, and
the desktop removes registered staging directories abandoned by a terminated
process. Publication remains no-clobber and atomic. Each writer stays
fail-closed unless its exact installation and separate hash-bound evidence gate
remain valid; Full Spectrum color accuracy additionally retains its per-user
calibration/explicit-approval boundary.

Capture and qualification procedures:

- [Snapmaker U1 baseline capture](docs/U1_BASELINE_CAPTURE.md)
- [Snapmaker U1 Direct qualification operator notes](crates/orca-adapter/qualification/u1-direct-2.3.5-operator-notes.md)
- [Snapmaker U1 Full Spectrum qualification operator notes](crates/orca-adapter/qualification/u1-full-spectrum-2.3.5-operator-notes.md)
- [Bambu Lab A1 mini baseline capture](docs/A1_MINI_BASELINE_CAPTURE.md)
- [Bambu Lab A1 mini qualification operator notes](crates/a1mini-adapter/qualification/a1mini-02.02.00.85-operator-notes.md)
- [Native 3MF writer implementation specification (Russian)](docs/NATIVE_3MF_WRITER_IMPLEMENTATION_SPEC_RU.md)

The detailed requirements are in [TECHNICAL_SPECIFICATION_RU.md](TECHNICAL_SPECIFICATION_RU.md).

## License

Licensed under the [Apache License 2.0](LICENSE).
