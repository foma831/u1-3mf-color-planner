# Snapmaker U1 Direct 2.3.5 qualification notes

Qualified at `2026-08-03T22:05:28Z` against Snapmaker Orca 2.3.5 with the
hash-pinned executable and effective installed Snapmaker profile pack recorded
in the typed qualification documents.

## Artifacts

| Artifact | Bytes | SHA-256 |
|---|---:|---|
| Source fixture | 98,361,035 | `1b20d6124353d3bc31c4ea554e482ba6f8ef281dd23e2df6bbc0d561b4f6e0d7` |
| Writer candidate | 101,737,352 | `7bd3668d143dabc57643b820c49b9ba1130534b2b1d5555e4a7c8fe80059aa30` |
| First GUI save | 97,526,200 | `1796ca7e2c18ad89579e13024197de8e0f84a715d806d39fbed18b538178fb31` |
| Reopened GUI save | 97,526,235 | `5a7cca46b77650b6c86328e1b287cbf2f87ab0007c48e77970222da83d1295b3` |
| Typed qualification report | 5,763 | `41d62acb065f70551934c150eebda25ee3144e1a95d3bf7ad1c8285d8cb23f69` |

The version-scoped semantic digest for the candidate and both GUI saves is
`2886230c0f2a30c47f288aeed1eecaf84363fa1dcc6c1ee96acf5e9836f9a35e`.

## GUI sequence

1. Opened the writer candidate in a fresh Snapmaker Orca process.
2. Verified Snapmaker U1, Textured PEI Plate, 0.4 mm nozzles, four Polymaker
   General PLA profiles, T1 black, T2 cyan, T3 yellow, and T4 grey.
3. Verified by-layer sequencing, non-spiral mode, the ribbed prime tower, and
   the expected plate/object layout.
4. Sliced successfully with all four toolheads, a printed prime tower, and 372
   filament changes; saved the first GUI project.
5. Quit the application completely, launched a fresh process, and opened the
   first GUI save.
6. Repeated the machine, material, color, layout, and profile checks, then
   sliced successfully again with 372 filament changes and saved the second GUI
   project.

The only visible notices were performance suggestions for `Body.lst.stl` and
`Tail.lst.stl`, each containing more than one million triangles. No repair,
missing-profile, custom-profile, or incompatible-profile warning appeared.

This GUI matrix directly exercised the Polymaker General PLA leaf profile on
all four toolheads. Generic PLA and Generic PETG remain inside the exact
hash-pinned profile closure and writer validation surface, but this record does
not claim a separate GUI round trip for those two leaf profiles.

## Automated evidence

`validate-u1-gui-round-trip` reported `isValid=true` with no issues. It proved:

- strict-unsliced validity for the candidate and the exact Snapmaker Orca 2.3.5
  regenerated-artifact policy for both GUI saves;
- exact Stage-B bed/sequence/spiral/timelapse globals and their stability;
- exact ribbed prime-tower settings and stable lower-left coordinates
  `x=14.5`, `y=212`;
- stable T1-T4 profile names, leaf setting IDs, colors, materials, and plate
  filament map;
- exact source `identify_id` preservation by the writer, positive and globally
  unique writer-candidate IDs, plus positive and unique process-local GUI IDs
  with a stable semantic instance bijection;
- stable geometry, parts, transforms, bounds, extruder assignments, plate
  membership, and zero embedded project presets.

The post-evidence verification passed the full Rust workspace tests, Clippy
with warnings denied, Rust formatting, TypeScript checking, 98 frontend tests,
and the production web build. The Direct-aware `doctor` command reports
`status=qualified`, `conversionAvailable=true`, and no installation issues.
An explicit negative `doctor` run against an unqualified profile source printed
`conversionAvailable=false` and exited nonzero.

A separate read-only Claude Opus review found no release-blocking issue. Its
profile-source binding, writer-candidate ID, and CLI exit-status hardening
findings were applied before the final test run recorded above.

## Screenshot digests

The six transient operator screenshots were hashed during the reviewed run:

1. candidate opened: `888c9145385656cf55b9b5ab8f8b790ffbe710034ebefd19b071946fd4d7f0a0`;
2. first slice: `979c4a883addd2d85351eeb0f5b7e2f9d2b9a58b08c7e8db41f78196429c7ae2`;
3. first save: `86dbd72d59ea3b2498c185a0c85a1469f35fd72e3e0fdd855d4ee52322f93906`;
4. clean reopen: `98523719d9a6807777139c75fca80a74591c2a9fe1daf1f9d0253cacf94f0c81`;
5. second slice: `899d7c49e2197f12c8b208e3633865044da3a1c87419397932ddb552b4650ce0`;
6. second save: `5a2bb4037d825939f9673d303baf7872e45335e346e4a25fbe7e4b45ac7c4b48`.

Screenshots are deliberately kept outside the strict JSON schemas; the release
gate depends on the exact source/candidate/save/report hashes and typed checks.
