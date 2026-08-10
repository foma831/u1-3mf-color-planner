# Snapmaker U1 Full Spectrum 2.3.5 qualification notes

Writer qualification was completed on 2026-08-03 in the exact Snapmaker Orca
2.3.5 installation whose executable, bundled Full Spectrum profile closure,
and effective installed physical-filament profile hashes are listed in the
typed qualification report.

## Hash-bound documents

| Document | Bytes | SHA-256 |
|---|---:|---|
| Typed qualification report | 5,290 | `5ccec6519a7fb90698845447fdf0fc758ac7c390e6c655b7514bf20ec4f4aa37` |
| Release-gate record | 1,469 | `4974f576f22c5c868852e57bcf35d1111222badf993c3c7ad899ea69e54441f2` |

The schema-v2 release record embeds the exact report hash. Its machine-readable
scope is `native_project_structure_and_gui_round_trip`; recipe accuracy uses
the separate `per_user_measured_calibration_or_explicit_color_approval` policy.

## Six-plate GUI cycle

| Role | Bytes | SHA-256 |
|---|---:|---|
| Source fixture | 202,162,511 | `f0ad280964c0f0a3bdae1c3b8cc187d572da2986010f6e2c08e4e803db846b81` |
| Writer candidate | 175,580,277 | `df06f2f2ce4ac7a564066c13b2fe05ae579be7cfbd08832473268911c4fd997e` |
| First GUI save | 162,051,592 | `6cee93e41fe5b03fbd47f499decb97dd71ef453dc34f661a95be797fb74e42ed` |
| Reopened GUI save | 162,052,186 | `50ef509e9e5bfadf834200ec110a2ff1cfa61569150a1cb6ed39100964a4dafa` |

All six plates were sliced before the first save and again after a full
application quit and reopen. The three-file semantic validator returned
`isValid=true`, including physical T1-T4 identity, virtual definitions, paint
assignments, solid T4 use, plate maps, subdivision globals, prime tower,
geometry, and placements.

This record does not claim a physical color coupon. A measured recipe remains
valid only for its exact per-user CMY+X calibration fingerprint and sample ID;
an explicit nominal color approval remains an approximation. The user-provided
geometry stays local qualification evidence and is not a redistributable
repository fixture.
