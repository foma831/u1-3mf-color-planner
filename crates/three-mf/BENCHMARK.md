# Large-sample analysis benchmark

This benchmark covers the two largest sample fixtures:

- `Sample/Withered_Foxy.3mf` (202,162,511 bytes, SHA-256
  `f0ad280964c0f0a3bdae1c3b8cc187d572da2986010f6e2c08e4e803db846b81`)
- `Sample/Withered_Foxy_A1_mini_No_AMS.3mf` (201,258,183 bytes, SHA-256
  `78a613193c05f96c77e0db5c7a5a0ce0eb051936752ae2b0def09ae8202cab8c`)

It times only `analyze_project`; the release build is completed before timing.
The analyzer still snapshots and hashes the immutable compressed input and
streams every required ZIP/XML entry without extracting the archive.

Run it from the workspace root:

```sh
cargo build --release -p u1-three-mf --example analyze_benchmark
target/release/examples/analyze_benchmark "Sample/Withered_Foxy.3mf"
target/release/examples/analyze_benchmark "Sample/Withered_Foxy_A1_mini_No_AMS.3mf"
```

## 2026-08-03 result

Environment: Apple M4 (`Mac16,10`), macOS 15.7.5, Rust 1.97.1. Runs were
sequential with a warm filesystem cache; compilation was excluded.

| Fixture | Revision | Release wall-clock samples (s) | Median (s) | Change |
|---|---|---|---:|---:|
| Withered Foxy | Before mesh-stream hot-path optimization | 9.990633, 10.116869, 10.091975 | 10.091975 | - |
| Withered Foxy | After | 6.483249, 6.544853, 6.461369 | 6.483249 | -35.76% |
| A1 mini No AMS | Before mesh-stream hot-path optimization | 9.644802, 9.787825, 10.044214 | 9.787825 | - |
| A1 mini No AMS | After | 6.237233, 6.159387, 6.207811 | 6.207811 | -36.58% |

Every run reported the expected fixture hash and stable semantic counters.
Both fixtures contain 7,308,333 vertices, 14,616,548 triangles, and 89
objects. Withered Foxy reports 179 entries and 12 plates; A1 mini No AMS
reports 155 entries and 9 plates.

Safety and semantic regression coverage is provided by the full release test
suite, the exact fixture acceptance test, bulk-vs-single-byte XML token-limit
test, and duplicate vertex/triangle attribute rejection test:

```sh
cargo test --release -p u1-three-mf
cargo clippy -p u1-three-mf --all-targets -- -D warnings
```
