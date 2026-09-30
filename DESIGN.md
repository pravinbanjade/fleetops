---
name: FleetOps
description: Open-source container management; a quiet instrument panel for live Docker hosts.
colors:
  graphite: "oklch(0.205 0 0)"
  paper: "oklch(1 0 0)"
  ink: "oklch(0.145 0 0)"
  mist: "oklch(0.97 0 0)"
  slate-text: "oklch(0.556 0 0)"
  hairline: "oklch(0.922 0 0)"
  focus-grey: "oklch(0.556 0 0)"
  signal-red: "oklch(0.577 0.245 27.325)"
  night: "oklch(0.145 0 0)"
  night-panel: "oklch(0.205 0 0)"
  night-raised: "oklch(0.269 0 0)"
  night-text: "oklch(0.985 0 0)"
  night-slate-text: "oklch(0.708 0 0)"
  night-primary: "oklch(0.922 0 0)"
  night-hairline: "oklch(1 0 0 / 10%)"
  night-field: "oklch(1 0 0 / 15%)"
  night-focus-grey: "oklch(0.708 0 0)"
  night-signal-red: "oklch(0.704 0.191 22.216)"
  state-running: "oklch(0.508 0.118 165.612)"
  state-caution: "oklch(0.769 0.188 70.08)"
  state-caution-text: "oklch(0.279 0.077 45.635)"
  state-failed: "oklch(0.505 0.213 27.518)"
  live-dot: "oklch(0.596 0.145 163.225)"
  terminal-black: "#0c0c0d"
  terminal-text: "#e5e5e5"
  chart-axis: "#8a8a8a"
  series-cpu: "#3b82f6"
  series-memory: "#8b5cf6"
  series-in: "#10b981"
  series-out: "#f59e0b"
  series-read: "#06b6d4"
  series-write: "#ef4444"
typography:
  headline:
    fontFamily: "ui-sans-serif, system-ui, sans-serif, 'Apple Color Emoji', 'Segoe UI Emoji'"
    fontSize: "1.5rem"
    fontWeight: 600
    lineHeight: "2rem"
  title:
    fontFamily: "ui-sans-serif, system-ui, sans-serif"
    fontSize: "1rem"
    fontWeight: 600
    lineHeight: 1
  body:
    fontFamily: "ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.875rem"
    fontWeight: 400
    lineHeight: "1.25rem"
  label:
    fontFamily: "ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.75rem"
    fontWeight: 500
    lineHeight: "1rem"
  mono:
    fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, 'Liberation Mono', 'Courier New', monospace"
    fontSize: "0.75rem"
    fontWeight: 400
    lineHeight: "1rem"
rounded:
  xs: "2px"
  sm: "4px"
  md: "6px"
  lg: "8px"
  xl: "12px"
  full: "9999px"
spacing:
  "1": "4px"
  "1.5": "6px"
  "2": "8px"
  "3": "12px"
  "4": "16px"
  "6": "24px"
components:
  button-primary:
    backgroundColor: "{colors.graphite}"
    textColor: "{colors.paper}"
    typography: "{typography.body}"
    rounded: "{rounded.md}"
    padding: "8px 16px"
    height: "36px"
  button-outline:
    backgroundColor: "{colors.paper}"
    textColor: "{colors.ink}"
    rounded: "{rounded.md}"
    padding: "8px 16px"
    height: "36px"
  button-outline-hover:
    backgroundColor: "{colors.mist}"
  button-destructive:
    backgroundColor: "{colors.signal-red}"
    textColor: "{colors.paper}"
    rounded: "{rounded.md}"
    padding: "8px 16px"
    height: "36px"
  button-sm:
    rounded: "{rounded.md}"
    padding: "0 12px"
    height: "32px"
  input:
    backgroundColor: "transparent"
    textColor: "{colors.ink}"
    rounded: "{rounded.md}"
    padding: "4px 12px"
    height: "36px"
  badge-running:
    backgroundColor: "{colors.state-running}"
    textColor: "{colors.paper}"
    typography: "{typography.label}"
    rounded: "{rounded.full}"
    padding: "2px 8px"
  badge-caution:
    backgroundColor: "{colors.state-caution}"
    textColor: "{colors.state-caution-text}"
    typography: "{typography.label}"
    rounded: "{rounded.full}"
    padding: "2px 8px"
  badge-failed:
    backgroundColor: "{colors.state-failed}"
    textColor: "{colors.paper}"
    typography: "{typography.label}"
    rounded: "{rounded.full}"
    padding: "2px 8px"
  badge-neutral:
    backgroundColor: "transparent"
    textColor: "{colors.ink}"
    typography: "{typography.label}"
    rounded: "{rounded.full}"
    padding: "2px 8px"
  tabs-list:
    backgroundColor: "{colors.mist}"
    textColor: "{colors.slate-text}"
    rounded: "{rounded.lg}"
    padding: "3px"
    height: "36px"
  tabs-trigger-active:
    backgroundColor: "{colors.paper}"
    textColor: "{colors.ink}"
    rounded: "{rounded.md}"
  nav-link:
    textColor: "{colors.ink}"
    typography: "{typography.body}"
    rounded: "{rounded.md}"
    padding: "6px 8px"
  nav-link-active:
    backgroundColor: "{colors.mist}"
  card:
    backgroundColor: "{colors.paper}"
    textColor: "{colors.ink}"
    rounded: "{rounded.xl}"
    padding: "24px"
  code-block:
    backgroundColor: "{colors.mist}"
    textColor: "{colors.ink}"
    typography: "{typography.mono}"
    rounded: "{rounded.md}"
    padding: "16px"
  terminal:
    backgroundColor: "{colors.terminal-black}"
    textColor: "{colors.terminal-text}"
    typography: "{typography.mono}"
---

# Design System: FleetOps

## Overview

**Creative North Star: "The Instrument Panel"**

FleetOps looks like a well-made gauge cluster. The chrome is achromatic grey: navigation, controls, tables and cards are drawn in graphite on paper, or pale grey on night, with hairline borders. Colour is spent only on readings. A container's state, a failed pull, a CPU line and an ANSI-coloured log line are the only chromatic things on screen, so the eye goes straight to them. The screen's job is to report what the hosts are doing, not to decorate it.

The panel is compact and orderly for long working sessions and for incidents:

- controls are 36 px tall, with 32 px in toolbars;
- body text is 14 px;
- page gutters are 24 px;
- a fixed 224 px sidebar holds navigation.

Surfaces are nearly flat, and depth is a hairline or at most a one-pixel shadow. Machine output (logs, JSON, digests, the terminal) is set in monospace, and the terminal is always dark whatever the theme. Light, dark and system themes are equal citizens: every token has a night counterpart.

Components are **precise and restrained**. Two looks are ruled out:

- **Portainer's look.** FleetOps is an alternative, not a reskin: no blue-heavy card-grid dashboards.
- **Marketing gloss.** No gradients, glassmorphism, hero illustrations or decorative motion inside the app.

**Key Characteristics:**
- Achromatic chrome; colour only for state, alerts and live data.
- Compact controls: 36 px default, 32 px small, 24 px extra small.
- Hairline borders and near-flat surfaces.
- System sans for interface text, system monospace for machine output.
- Equal light and dark themes; the terminal is always dark.
- 8 px base radius, with a pill only for state badges.

## Colors

A greyscale instrument body with a small set of signal colours that each mean one thing.

### Primary
- **Graphite** (`graphite`): the primary action colour, used for solid buttons, the brand mark's field and selected text. On night it inverts to **Night Primary**, a pale grey solid with graphite text.

### Neutral
- **Paper** (`paper`) and **Night** (`night`): page backgrounds for the light and dark themes.
- **Ink** (`ink`) and **Night Text** (`night-text`): body text and headings.
- **Mist** (`mist`) and **Night Raised** (`night-raised`): quiet fills for hover and active nav rows, the tab track, code blocks (at 40% opacity), and secondary buttons.
- **Slate Text** (`slate-text`) and **Night Slate Text** (`night-slate-text`): secondary text such as descriptions, placeholders, empty states, "—" for missing values and section labels.
- **Hairline** (`hairline`) and **Night Hairline** (`night-hairline`): every border, table row divider and field stroke. On night, fields use **Night Field** (`night-field`).
- **Focus Grey** (`focus-grey`) and **Night Focus Grey** (`night-focus-grey`): focus rings, drawn solid. A control gets a 3 px ring, and a link a 2 px outline 2 px out. Both are at least 3:1 against every surface of their theme.
- **Night Panel** (`night-panel`): cards, popovers and dialogs in the dark theme.

### Signals
- **Running Green** (`state-running`): the `running`, `healthy` and environment `up` badges, with white text (5.4:1). The brighter **Live Dot** (`live-dot`) marks the dashboard's live state, next to its words.
- **Caution Amber** (`state-caution`): the `paused`, `restarting` and `partial` badges, with **Caution Ink** text (`state-caution-text`, 7.0:1), since white on amber fails.
- **Failed Red** (`state-failed`): the `unhealthy`, `dead` and environment `down` badges, with white text (6.4:1) in both themes.
- **Signal Red** (`signal-red`, `night-signal-red`): destructive buttons, bad event actions (`die`, `kill`, `oom`), inline errors (`role="alert"`) and the lost-connection banner, which uses the red at 10% as its fill.
- **Chart series** (`series-*`): one fixed colour per metric, the same on both themes: CPU blue, memory violet, network in green and out amber, block read cyan and write red. Axes and grid are **Chart Axis** grey (`chart-axis`, and the same grey at 18% for grid lines).
- **ANSI palette**: the 16 terminal colours are CSS variables `--ansi-0` to `--ansi-15`, darker in light mode so they read on paper and brighter in dark mode. The terminal uses the dark set on **Terminal Black** (`terminal-black`).

### Named Rules
**The Readings-Only Rule.** Colour marks state, alerts and live data, and nothing else. A coloured button, icon, border or background that isn't a reading or a destructive action breaks the panel.

**The One-Meaning Rule.** Each signal colour means one thing. Green is healthy or running, amber is transitional, red is failed or destructive. Don't reuse them for decoration or for categories.

**The Canvas Hex Rule.** Anything drawn on a canvas (uPlot charts, xterm.js) uses a concrete hex value, because a canvas can't read CSS variables. Anything in the DOM uses the theme variables.

## Typography

**Interface Font:** the system UI sans stack (`ui-sans-serif, system-ui, sans-serif`)
**Machine-Output Font:** the system monospace stack (`ui-monospace, SFMono-Regular, Menlo, …`)

**Character:** Native, zero-download type that looks like the operator's own OS. It is legible at small sizes, with a monospace companion for anything a machine wrote.

### Hierarchy
- **Headline** (600, 1.5rem/2rem): one page title per page, truncated on one line. For a resource page it is the resource's name, and on the dashboard it is the environment's name.
- **Title** (600, 1rem, line-height 1): card and dialog titles.
- **Body** (400, 0.875rem/1.25rem): table cells, form fields, nav links, buttons and descriptions. This is almost all the text in the app.
- **Label** (500, 0.75rem/1rem): state badges, nav section headings ("Administration") and captions.
- **Mono** (400, 0.75rem/1rem): logs, inspect JSON, image history, audit details, IDs and digests. Wrapping is opt-in.

### Named Rules
**The Machine-Speaks-Mono Rule.** Text a machine produced (log lines, JSON, commands, digests, shell output) is set in monospace. Text written for the operator is set in the sans.

**The Fourteen-Pixel Floor.** Body text in the interface is 14 px (0.875rem). Only labels, badges and mono output go down to 12 px.

## Layout

- **App shell:**
  - A fixed-width 224 px sidebar sits on the left, with a hairline right border and 12 px padding. It holds the wordmark, environment switcher, command palette trigger, environment navigation, admin navigation, and the user menu at the bottom.
  - The main column takes the rest of the width (`min-w-0`, so wide tables scroll inside their box rather than widening the page).
- **Pages:** a single column with 24 px padding and 16 px between blocks. The title row has the headline on the left and the actions on the right, and both wrap when narrow.
- **Rhythm:** a 4 px base scale, used as 4, 6, 8, 12, 16 and 24 px.
  - 8 px is the everyday gap: between controls in a toolbar, and between icon and label.
  - 16 px separates page blocks, and 24 px frames the page.
- **Density:**
  - Table cells have 8 px padding and 40 px header rows.
  - Rows have hover and selected fills in Mist.
  - Lists are virtualised where they can grow large (logs).
- **Responsive:**
  - Desktop-first, and used mainly on desktop browsers.
  - Breakpoints are used sparingly (Tailwind's `sm`, `md` and `lg` at 640, 768 and 1024 px), mostly to switch form grids and dialogs from one column to two.
  - Tables scroll horizontally inside their container rather than reflowing.

## Elevation & Depth

The system is flat. Hairline borders do almost all the separation. Shadows are the smallest available:

- a 1 px shadow on outline buttons and fields;
- a small ambient shadow on cards;
- the default overlay shadow on menus and dialogs, which sit above a dimmed page.

Tone carries hierarchy instead:

- Paper, then Mist fills for active and hovered rows.
- On night: Night, then Night Panel for cards and popovers, then Night Raised for hover.

### Shadow Vocabulary
- **Hairline lift** (`box-shadow: 0 1px 2px 0 rgb(0 0 0 / 0.05)`): outline buttons and inputs, to separate them from the page without a visible edge.
- **Card rest** (`box-shadow: 0 1px 3px 0 rgb(0 0 0 / 0.1), 0 1px 2px -1px rgb(0 0 0 / 0.1)`): cards on the dashboard and detail pages.

### Named Rules
**The Hairline-First Rule.** Separate with a 1 px Hairline border before reaching for a shadow. A shadow bigger than Card rest belongs only to floating layers: menus, dialogs and the command palette.

## Shapes

Gently rounded and consistent:

| Radius | Value | Used for |
|---|---|---|
| Base | 8 px (`--radius`) | tab tracks |
| md | 6 px | buttons, inputs, nav rows, code blocks, tab triggers |
| sm | 4 px | small inner elements |
| xl | 12 px | cards |
| xs | 2 px | the dialog close button |

Pills (full radius) are used only for badges, so a rounded capsule always reads as a status. Borders are 1 px Hairline everywhere. Icons are Lucide line icons at 16 px (12 px in extra-small buttons), inheriting the text colour.

## Components

### Buttons
Precise and quiet. A solid button is the one thing on a toolbar that asks to be pressed.
- **Shape:** 6 px corners, a 16 px icon with an 8 px gap, 14 px medium text and no wrapping.
- **Primary:** a Graphite fill with Paper text, 36 px tall with 16 px side padding. Hover darkens to 90% opacity.
- **Outline:** the default for secondary actions (Prune, Build, Start or Stop in toolbars). It has a Hairline border, a Paper fill and the Hairline-lift shadow. Hover fills with Mist. On night the fill is the Night Field grey at 30%, rising to 50% on hover.
- **Destructive:** a Signal Red fill with white text, used only for the confirm step of irreversible actions (Remove, Delete).
- **Ghost and link:** ghost is for icon buttons and menu triggers (with a Mist hover), and link is for inline text actions.
- **Sizes:** default 36 px, sm 32 px (row and bulk actions), xs 24 px, and square icon sizes 36, 32 and 24 px.
- **Focus:** a Focus Grey border with a solid 3 px Focus Grey ring, the same on every control. Disabled controls drop to 50% opacity and stop taking clicks.

### State Badges
The system's main reading.
- **Style:** a pill with 2 px × 8 px padding and 12 px medium text. It shows the state word in lowercase, exactly as Docker reports it.
- **Tones:**
  - Running Green with white text for `running`, `healthy` and `up`;
  - Caution Amber with Caution Ink text for `paused`, `restarting` and `partial`;
  - Failed Red with white text for `unhealthy`, `dead` and `down`;
  - every other state (`exited`, `created`) is an outline pill in Ink with a Hairline border, so a stopped container is neutral rather than alarming.

### Cards / Containers
- **Corner Style:** 12 px.
- **Background:** Paper, or Night Panel on dark.
- **Shadow Strategy:** Card rest (see Elevation).
- **Border:** 1 px Hairline.
- **Internal Padding:** 24 px, with 24 px between the header, content and footer.

### Inputs / Fields
- **Style:** 36 px tall, with a transparent fill, a Hairline stroke (Night Field on dark), 6 px corners, 12 px side padding and the Hairline-lift shadow. Placeholder text is Slate Text. Selected text inverts to Graphite.
- **Focus:** the shared Focus Grey border with its solid 3 px ring.
- **Error / Disabled:** `aria-invalid` turns the stroke and ring Signal Red. Disabled fields are at 50% opacity with a not-allowed cursor.
- **Toggles:** Radix checkboxes, each with a text label to its right.

### Navigation
- **Sidebar links:** 14 px body text with a 16 px Lucide icon and an 8 px gap, 6 px × 8 px padding and 6 px corners. Hover fills with Mist. The active link is Mist-filled and medium weight, and there is no accent colour.
- **Section labels:** 12 px medium in Slate Text ("Administration").
- **Tabs:** a Mist track with 8 px corners, 3 px padding and 36 px height, holding 14 px medium triggers. The active trigger is Paper with a hairline lift. Container pages use tabs for Overview through Files.

### Tables
- Full-width and 14 px, with 40 px header rows in medium weight.
- Rows have Hairline dividers and a Mist hover at 50%. Selected rows are Mist.
- Row checkboxes hug the left edge. Filtering, column visibility and bulk actions sit above the table.
- Live cells, such as CPU and memory, pair a 60-point sparkline with a right-aligned tabular-figure value.

### Status Readout (signature: the dashboard)
The environment's home, read in urgency order.
- **Liveness line:** under the headline, a small Live Dot and plain words: "Live", "Reconnecting. Showing data as of …" or "Unavailable. Showing the last known state, as of …". Data that isn't current is always stamped with its time, never dimmed below contrast.
- **Headline sentence:** an h2 in 18 px semibold that answers the first question ("Nothing needs attention", "2 containers need attention").
- **Attention list:** a hairline-bordered list, most urgent first. Each row has a State Badge, the container name (linking to its logs) and Docker's own status text.
- **State strip:** outline chips of count and State Badge, each linking to the containers list filtered to that state.
- **Host readouts:** CPU and memory now-values in headline type, each with a 160 × 32 sparkline whose scale follows the minute's peak.
- **Below, in two columns:** Recent events (time-only stamps, bad actions in Signal Red) and Disk usage (hairline proportion bars in Slate Text on Mist).
- **Footer:** the other inventory counts as plain links.
- **Don't** rebuild it as a row of stat cards.

### Live Readouts (signature)
The instrument part of the panel.
- **Sparklines:** inline SVG polylines, 1.25 px stroke in the text colour, drawn in table cells.
- **Charts:** uPlot at 160 px tall, with the y axis from zero, Chart Axis grey axes and fixed per-metric series colours.
- **Logs:** a virtualised monospace view with ANSI colour, search hits as `<mark>` (the current hit in Mist), and a stderr marker.
- **Terminal:** xterm.js, always dark, on Terminal Black with the bright ANSI set.

## Do's and Don'ts

### Do:
- **Do** keep chrome achromatic. Graphite, Paper, Mist, Slate Text and Hairline, with their night counterparts, draw every non-reading surface.
- **Do** use a State Badge (pill, lowercase Docker state) whenever a resource's state is shown. Green means running, amber transitional, red failed, and the outline pill is for everything else.
- **Do** define every new colour as a theme variable with a night counterpart. Use a concrete hex only for canvas drawing (charts, terminal).
- **Do** use outline buttons for secondary and toolbar actions, and keep a solid Graphite button to the one primary action on a surface.
- **Do** set machine output in the system monospace at 12 px, and interface text in the system sans at 14 px.
- **Do** separate surfaces with 1 px Hairline borders and at most the Card-rest shadow.
- **Do** check every badge fill against its text at 4.5:1 or better, and every focus indicator at 3:1, in both themes.
- **Do** keep controls on the 36, 32 and 24 px heights, spaced on the 4 px scale (8 px everyday gap, 16 px between blocks, 24 px page padding).

### Don't:
- **Don't** make it look like Portainer: no blue-heavy chrome and no dashboard built from a grid of coloured stat cards.
- **Don't** add marketing gloss inside the app: gradients, glassmorphism, hero illustrations or decorative motion.
- **Don't** colour icons, borders, nav items or backgrounds for emphasis. If it isn't a reading or a destructive action, it stays grey.
- **Don't** use pills for anything but status, or signal colours for anything but their one meaning.
- **Don't** show `exited` or `created` in red or amber. A stopped container is neutral, not an alarm.
- **Don't** give the terminal a light theme. It stays on Terminal Black in both themes.
