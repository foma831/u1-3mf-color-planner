# U1 2.3.5 golden fixtures

Place the GUI-produced, non-private baseline described in
[`docs/U1_BASELINE_CAPTURE.md`](../../docs/U1_BASELINE_CAPTURE.md) here.

Headless `--export-3mf` output is intentionally not accepted as a golden
fixture because Snapmaker Orca 2.3.5 can leave dangling thumbnail relationships
and fail its own reopen round trip on macOS.
