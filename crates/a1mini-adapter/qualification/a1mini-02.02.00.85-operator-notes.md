# Bambu Lab A1 mini 02.02.00.85 qualification notes

Qualification was completed on 2026-08-03 in the exact Bambu Studio
02.02.00.85 installation whose executable and effective system-profile hashes
are listed in the typed qualification documents.

## Hash-bound documents

| Document | Bytes | SHA-256 |
|---|---:|---|
| Typed qualification report | 4,906 | `0d63ff24192cb327a368741cf48c1fdcac216b6f8e5e0d0b1e8807e3b4d97a32` |
| Release-gate record | 804 | `31d42dd03163eabc37a1620928fee7184be5b96a87215c00c406d46b842f867a` |

The release record embeds the exact report hash. The parser denies unknown
fields, requires the complete ordered profile baseline, and fails closed when
either document changes.

## PLA GUI cycle

| Role | Bytes | SHA-256 |
|---|---:|---|
| Source fixture | 201,258,183 | `78a613193c05f96c77e0db5c7a5a0ce0eb051936752ae2b0def09ae8202cab8c` |
| Writer candidate | 191,067,298 | `6ebe63cd19e12fec30b11e910bbf30f9abba0a37e8b3028c031e411be2d53c85` |
| First GUI save | 176,242,005 | `9561ee677e75710c22f01b1e01d925ec5cabdb70e525d8256f0cc11ad3646bdc` |
| Reopened GUI save | 176,241,951 | `c1cb69848f3396fa9d649fcd31df32de662060cf3187c4b59af5e63187536f41` |

## PETG GUI cycle

| Role | Bytes | SHA-256 |
|---|---:|---|
| Source fixture | 202,162,511 | `f0ad280964c0f0a3bdae1c3b8cc187d572da2986010f6e2c08e4e803db846b81` |
| Writer candidate | 71,225,060 | `4c737de3a6b36ef453744f4fc2f4bf370b4c7a283ba1867e051cd033edf3d075` |
| First GUI save | 66,053,860 | `ca87bcb9baeaee56f1b01e378ed40216b5e231fbfd78962df5f4de0592835efd` |
| Reopened GUI save | 66,054,359 | `a88feec5bd2490025191f001d00d943e51219f32c062bf96752facfee019f3ad` |

Each candidate was opened, sliced, saved, fully closed, reopened, sliced again,
and saved again. Both three-file semantic validators returned `isValid=true`.
The user-provided geometry remains local qualification evidence and is not a
redistributable repository fixture.
