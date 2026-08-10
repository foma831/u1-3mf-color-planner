# Print Plan UI concept

Reference image: `design/print-plan-concept.png`

## Visual direction

The app is a precise desktop production tool, not a marketing site. It uses a cool white workspace, pale graphite rails, dark ink typography, and restrained cyan, magenta, and yellow registration-line accents. Layouts are open and table-driven, with small radii and very limited elevation.

The generated concept contains one intentional data error that must not be implemented: A1 mini rows may only use `A1 Mono`, never `CMY+X Full Spectrum`.

## Primary screen anatomy

1. Slim title bar.
2. Left workflow rail: `Project`, `Materials`, `Color Strategy`, `Print Plan`, `Validate`.
3. Project header with filename, analysis status, and source counts.
4. Main `Print Plan` area with a batch timeline, compact summary strip, and semantic target-plate table.
5. Right inspector for the selected plate, strategy comparison, toolhead assignments, and source-to-actual color mapping.
6. Bottom action rail with the toolhead mapping warning, `Export Plan`, and `Approve & Convert`.

## Design tokens

```css
:root {
  color-scheme: light;
  --color-canvas: #f5f7f8;
  --color-surface: #ffffff;
  --color-surface-subtle: #eef2f4;
  --color-ink: #15191d;
  --color-muted: #68727b;
  --color-border: #d5dce1;
  --color-border-strong: #aeb9c1;
  --color-focus: #087fcc;
  --color-cyan: #12a8df;
  --color-magenta: #d70a7b;
  --color-yellow: #f2c300;
  --color-success: #138a4b;
  --color-warning: #b97800;
  --color-danger: #b42318;
  --radius-control: 0.375rem;
  --radius-panel: 0.625rem;
  --shadow-float: 0 0.5rem 1.5rem rgb(21 25 29 / 0.09);
  --space-1: 0.25rem;
  --space-2: 0.5rem;
  --space-3: 0.75rem;
  --space-4: 1rem;
  --space-5: 1.5rem;
  --space-6: 2rem;
}
```

Use the system UI stack: `-apple-system`, `BlinkMacSystemFont`, `Segoe UI`, sans-serif. Do not load a remote font.

## Component families

- App shell and workflow navigation.
- Status summary strip.
- Batch timeline with explicit spool-change boundaries.
- Semantic data table with selected, warning, and disabled row states.
- Toolhead row with a swatch, slot, material, and action.
- Source/actual paired color swatches with textual HEX and Delta E.
- Native buttons, selects, checkboxes, progress, and dialogs.

## Accessibility and interaction

- Set `lang="en"` and provide a skip link to the main workspace.
- Use native controls and a semantic table with caption and scoped headers.
- Provide visible `:focus-visible` outlines.
- Never communicate status by color alone.
- Use a single polite live region for completed analysis and plan recalculation.
- Respect `prefers-reduced-motion` and avoid persistent motion.
- Keep long table regions scrollable with visible scroll affordance.

## Allowed primary-screen copy

- `U1 3MF Color Planner`
- `Project`, `Materials`, `Color Strategy`, `Print Plan`, `Validate`
- `Analysis complete`
- `CMY+X Full Spectrum`, `Direct Spools`, `A1 Mono`
- `Export Plan`, `Approve & Convert`
- `Toolhead mapping must be verified in Snapmaker Orca before printing.`

