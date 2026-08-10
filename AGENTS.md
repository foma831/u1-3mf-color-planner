# Project instructions

- Keep all source code, identifiers, comments, UI strings, CLI output, test names, and logs in English.
- Keep user-facing project discussion and the main technical specification in Russian.
- Treat input 3MF files as immutable.
- Never extract an entire 3MF archive to disk. Read ZIP entries with explicit size and path limits.
- Preserve material identity. Do not merge identical RGB values across different materials.
- Use physical loadouts, not visible color count, as the U1 plate constraint.
- Direct Spool mode is available only when the selected scope has at most four effective material-color pairs.
- Do not copy GPL, AGPL, or PolyForm converter code into this repository.
- Use `apply_patch` for hand-authored file changes.
- Run formatting, unit tests, and relevant sample checks before handoff.

