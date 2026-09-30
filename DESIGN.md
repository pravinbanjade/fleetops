---
name: FleetOps
description: Open-source container management, racked as a controller's strip board for live Docker hosts.
colors:
  bay: "oklch(0.158 0.008 255)"
  strip: "oklch(0.198 0.01 255)"
  rule: "oklch(0.285 0.012 255)"
  float-strip: "oklch(0.215 0.011 255)"
  wash: "oklch(0.25 0.012 255)"
  field-rule: "oklch(0.33 0.013 255)"
  ink: "oklch(0.945 0.006 90)"
  ink-muted: "oklch(0.72 0.014 255)"
  cord: "oklch(0.82 0.15 78)"
  cord-ink: "oklch(0.21 0.045 60)"
  cord-soft: "oklch(0.82 0.15 78 / 0.1)"
  destructive: "oklch(0.7 0.18 25)"
  state-run: "oklch(0.76 0.15 158)"
  state-run-ink: "oklch(0.2 0.05 158)"
  state-wait: "oklch(0.78 0.15 60)"
  state-wait-ink: "oklch(0.22 0.06 50)"
  state-fail: "oklch(0.68 0.19 25)"
  state-fail-ink: "oklch(0.15 0.04 25)"
  live: "oklch(0.76 0.15 158)"
  light-bay: "oklch(0.967 0.004 255)"
  light-strip: "oklch(1 0 0)"
  light-rule: "oklch(0.885 0.006 255)"
  light-wash: "oklch(0.935 0.006 255)"
  light-field-rule: "oklch(0.84 0.008 255)"
  light-ink: "oklch(0.205 0.015 255)"
  light-ink-muted: "oklch(0.47 0.016 255)"
  light-cord: "oklch(0.53 0.125 62)"
  light-cord-ink: "oklch(1 0 0)"
  light-cord-soft: "oklch(0.53 0.125 62 / 0.1)"
  light-destructive: "oklch(0.54 0.2 27)"
  light-state-run: "oklch(0.5 0.115 158)"
  light-state-run-ink: "oklch(1 0 0)"
  light-state-wait: "oklch(0.75 0.155 62)"
  light-state-wait-ink: "oklch(0.25 0.07 50)"
  light-state-fail: "oklch(0.52 0.19 27)"
  light-state-fail-ink: "oklch(1 0 0)"
  light-live: "oklch(0.6 0.14 158)"
  terminal-black: "#0c0c0d"
  terminal-text: "#e5e5e5"
typography:
  headline:
    fontFamily: "Mona Sans Variable, ui-sans-serif, system-ui, sans-serif"
    fontSize: "1.375rem"
    fontWeight: 600
    lineHeight: 1.25
    letterSpacing: "-0.012em"
    fontVariation: "\"wdth\" 112"
  title:
    fontFamily: "Mona Sans Variable, ui-sans-serif, system-ui, sans-serif"
    fontSize: "1.125rem"
    fontWeight: 600
    lineHeight: "1.75rem"
    letterSpacing: "-0.012em"
    fontVariation: "\"wdth\" 112"
  panel-title:
    fontFamily: "Mona Sans Variable, ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.875rem"
    fontWeight: 600
    lineHeight: "1.25rem"
  body:
    fontFamily: "Mona Sans Variable, ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.875rem"
    fontWeight: 400
    lineHeight: "1.25rem"
  strip:
    fontFamily: "Mona Sans Variable, ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.875rem"
    fontWeight: 400
    lineHeight: "1.25rem"
    fontFeature: "\"tnum\""
    fontVariation: "\"wdth\" 88"
  readout:
    fontFamily: "Mona Sans Variable, ui-sans-serif, system-ui, sans-serif"
    fontSize: "1.125rem"
    fontWeight: 600
    lineHeight: "1.75rem"
    fontFeature: "\"tnum\""
    fontVariation: "\"wdth\" 88"
  label:
    fontFamily: "Mona Sans Variable, ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.75rem"
    fontWeight: 500
    lineHeight: "1rem"
  holder:
    fontFamily: "Mona Sans Variable, ui-sans-serif, system-ui, sans-serif"
    fontSize: "11px"
    fontWeight: 600
    lineHeight: 1
    letterSpacing: "0.04em"
    fontVariation: "\"wdth\" 88"
  mono:
    fontFamily: "JetBrains Mono Variable, ui-monospace, SFMono-Regular, Menlo, monospace"
    fontSize: "0.75rem"
    fontWeight: 400
    lineHeight: "1rem"
rounded:
  xs: "2px"
  sm: "3px"
  md: "4px"
  lg: "6px"
  xl: "8px"
  full: "9999px"
spacing:
  hair: "2px"
  xs: "6px"
  sm: "8px"
  strip-y: "10px"
  md: "12px"
  lg: "16px"
  bay-gap: "20px"
  xl: "24px"
components:
  button-primary:
    backgroundColor: "{colors.cord}"
    textColor: "{colors.cord-ink}"
    typography: "{typography.body}"
    rounded: "{rounded.md}"
    padding: "8px 16px"
    height: "36px"
  button-outline:
    backgroundColor: "{colors.strip}"
    textColor: "{colors.ink}"
    rounded: "{rounded.md}"
    padding: "8px 16px"
    height: "36px"
  button-outline-hover:
    backgroundColor: "{colors.wash}"
  button-ghost:
    textColor: "{colors.ink-muted}"
    rounded: "{rounded.md}"
    padding: "8px 16px"
    height: "36px"
  button-ghost-hover:
    backgroundColor: "{colors.wash}"
    textColor: "{colors.ink}"
  input:
    backgroundColor: "{colors.bay}"
    textColor: "{colors.ink}"
    typography: "{typography.body}"
    rounded: "{rounded.md}"
    padding: "4px 12px"
    height: "36px"
  holder-run:
    backgroundColor: "{colors.state-run}"
    textColor: "{colors.state-run-ink}"
    typography: "{typography.holder}"
    rounded: "{rounded.xs}"
    padding: "0 6px"
    height: "20px"
    width: "4.75rem"
  holder-wait:
    backgroundColor: "{colors.state-wait}"
    textColor: "{colors.state-wait-ink}"
    typography: "{typography.holder}"
    rounded: "{rounded.xs}"
    padding: "0 6px"
    height: "20px"
    width: "4.75rem"
  holder-fail:
    backgroundColor: "{colors.state-fail}"
    textColor: "{colors.state-fail-ink}"
    typography: "{typography.holder}"
    rounded: "{rounded.xs}"
    padding: "0 6px"
    height: "20px"
    width: "4.75rem"
  holder-idle:
    textColor: "{colors.ink-muted}"
    typography: "{typography.holder}"
    rounded: "{rounded.xs}"
    padding: "0 6px"
    height: "20px"
    width: "4.75rem"
  panel:
    backgroundColor: "{colors.strip}"
    textColor: "{colors.ink}"
    rounded: "{rounded.lg}"
    padding: "16px"
  strip-row:
    typography: "{typography.strip}"
    padding: "10px 12px"
  strip-row-focused:
    backgroundColor: "{colors.cord-soft}"
  nav-link:
    textColor: "{colors.ink-muted}"
    typography: "{typography.body}"
    rounded: "{rounded.md}"
    padding: "0 8px"
    height: "32px"
  nav-link-active:
    backgroundColor: "{colors.cord-soft}"
    textColor: "{colors.ink}"
  top-bar:
    backgroundColor: "{colors.bay}"
    height: "48px"
    padding: "0 24px"
  rail:
    backgroundColor: "{colors.bay}"
    width: "15rem"
    padding: "16px 12px 12px"
  table-row:
    typography: "{typography.strip}"
    height: "44px"
    padding: "6px 12px"
  table-row-selected:
    backgroundColor: "{colors.cord-soft}"
---

# Design System: FleetOps

## Overview

**Creative North Star: "The Strip Bay"**

FleetOps is a controller's strip board. Every container, image, volume and event is a fielded strip: a row of boxed cells divided by hairline rules, with a squared state holder at its left edge, racked in bays by urgency. The board is cool near-black by default, and the chrome uses three greys only: the bay (page and rail), the strip (rows and panels) and the rule (every divider). Nothing on the board is decorative. The rest of the colour is either the operator's cord (amber) or a reading (state, environment identity, machine output).

The density is that of an operations tool used beside a terminal. Rows are 40 to 44px, controls are 32 to 36px, and numerals are tabular so columns line up like a readout. One variable face, Mona Sans, carries every role through its width axis. Strip fields are cut narrow at 88% so more fits in a cell. Headlines are set wide at 112% so a page or an environment name reads like a painted callsign. JetBrains Mono is used only for machine output: timestamps, IDs, error text from Docker, key legends and logs.

The board is flat. Depth appears only when something floats above it (menus, dialogs, the mobile drawer). The world's one piece of motion is the holder flash. When Docker changes a container's state on screen, its holder rings once in the cord's amber and then settles. With reduced motion, the flash is dropped.

**Key Characteristics:**
- The chrome uses three greys per theme (bay, strip and rule), and the rail sits on the bay grey, separated by a rule.
- Amber is the only accent (the cord), and it marks the primary action, selection, focus and the active nav row.
- State hues are readings only: green means running, orange means transitional, red means failed, and an outline means idle.
- Each environment has an identity hue, shown as a 3px band and a squared tag, and that hue is never a state hue or the cord.
- Mona Sans is set on its width axis (88% for strip fields, 112% for callsigns), and mono is used only for machine output.
- The attention bay can be walked with j/k, and Enter opens logs. A g-chord jumps between pages.
- The interface is dark first, and light is fully supported.

## Colors

The palette is a cool slate board in three greys, with one amber cord and a fixed set of state readings. Dark is the default theme, and the `light-*` tokens are its paired values.

### Primary
- **The Cord** (`cord`; light: `light-cord`): the only accent. It is used for the primary button fill, checked checkboxes, the active tab underline, the sort arrow, the active nav icon, the focus ring, the text caret and the selection wash. In light mode it darkens to a burnt amber so white text and the ring hold AA on white strips. **Cord Ink** (`cord-ink`) is the text colour on a cord fill.
- **Cord Soft** (`cord-soft`): the cord at 10% opacity. It is the one tint for "this is where you are": the active nav row, a selected table row, the focused strip in a bay and the bulk-selection bar.

### Neutral
- **Bay** (`bay`): the page ground and the rail. It is also the input fill in dark mode, where fields sit at 60% of the bay so they read as recessed into the strip.
- **Strip** (`strip`): every panel, strip list, table, readout, details grid and card. In light mode it is pure white on a pale slate bay.
- **Rule** (`rule`): every hairline, including panel borders, row dividers, cell dividers, the rail edge and the top bar's bottom edge.
- **Float Strip** (`float-strip`): menus, dialogs and the command palette. It is one step up from the strip, so floating layers separate by tone as well as by shadow.
- **Wash** (`wash`): the hover fill for ghost and outline controls, nav links and inventory rows. It is never a resting surface.
- **Field Rule** (`field-rule`): the stronger edge on inputs, outline buttons, checkboxes and the idle holder, and the scrollbar thumb.
- **Ink** / **Ink Muted** (`ink`, `ink-muted`): primary text, and secondary text (labels, meta lines, column heads, inactive nav). Dark ink is warm (hue 90) against the cool board.

### State readings
- **Run** (`state-run` on `state-run-ink`): a holder for running, healthy or up.
- **Wait** (`state-wait` on `state-wait-ink`): a holder for paused, restarting or partial, and the reconnecting live dot and the connection-lost banner.
- **Fail** (`state-fail` on `state-fail-ink`): a holder for unhealthy, dead or down, and for an exited container with a non-zero exit code.
- **Live** (`live`): the pulsing connection dot and an environment's "up" dot.
- **Destructive** (`destructive`): destructive buttons, inline errors, the unavailable-environment alert and bad event actions (die, kill, oom).

Every fill and ink pair holds at least 4.5:1 in both themes.

### Environment identity
An environment's hue comes from a stable hash of its name, picked from six hues: 255, 292, 328, 205, 232 and 270. The same hue appears as a flat 3px band at the top of the rail and the top bar (`oklch(0.58 0.15 h)`), and as the EnvTag square (`oklch(0.5 0.14 h)` with white initials). The environment switcher is tinted with the same hue at 10% (16% on hover). None of the six hues falls in the cord's range (60–78) or in a state hue's range (25, 158).

### Machine output
Logs use a 16-colour ANSI palette, defined per theme as `--ansi-0` to `--ansi-15`. The light palette is a set of darker shades. The terminal is always dark (`terminal-black` ground, `terminal-text` foreground) and uses the dark ANSI set, whatever the theme.

### Named Rules
**The Three Greys Rule.** The structural chrome is bay, strip and rule, and nothing else. The float strip is used only for layers above the board, and the wash only under a pointer. A new surface picks one of the three greys; it does not introduce a fourth.

**The One Cord Rule.** Amber means "the operator's hand is here": the primary action, the selection, focus or the current page. It never labels a state, a metric or a category.

**The Readings Rule.** Green, orange and red appear only when they report Docker's state. An environment's hue identifies the environment and reports nothing.

**The Ink Chart Rule.** A metric is not a reading of health, so charts draw in ink only. The first series is the ink and the second is the muted ink, dashed. Axes and grid use the muted ink and the rule. State hues and the cord stay off charts.

## Typography

**Display / Interface Font:** Mona Sans Variable (with ui-sans-serif, system-ui)
**Mono Font:** JetBrains Mono Variable (with ui-monospace, SFMono-Regular, Menlo)

**Character:** One grotesque cut three ways. It is narrow for the fielded strip, normal width for prose and controls, and wide for callsigns. JetBrains Mono is kept for anything a machine wrote.

### Hierarchy
- **Headline** (600, 1.375rem, 1.25, width 112%, -0.012em): the page title, which is often the environment's name.
- **Title** (600, 1.125rem, width 112%): the attention bay's headline ("3 containers need attention"). The same callsign cut at 1rem is used for dialog titles, and at 15px for the FleetOps wordmark.
- **Panel title** (600, 0.875rem, normal width): the header of each board panel, next to a 16px muted icon.
- **Body** (400, 0.875rem, 1.25rem): controls, meta lines and prose.
- **Strip** (400/600, 0.875rem, width 88%, tabular): the fields of a strip list, table cells, detail labels and readouts. A strip's name is set in 600.
- **Readout** (600, 1.125rem, width 88%, tabular): live CPU and memory values, and the counts in the state tabs (these use the callsign cut).
- **Label** (500, 0.75rem): column heads, readout labels, group headings and asides, in muted ink and sentence case.
- **Holder** (600, 11px, width 88%, +0.04em, uppercase): the state word inside a holder, exactly as Docker reports it.
- **Mono** (0.75rem, or 10px for key legends): timestamps, IDs, Docker error strings, kbd legends and logs.

### Named Rules
**The Two Widths Rule.** Width is the hierarchy tool. Use 88% for fields that must fit a cell and 112% for names that must be recognised at a glance. Weight stays at 400, 500 or 600.

**The Machine Voice Rule.** Mono means a machine wrote it. Human-facing labels never use mono, and Docker's own error text always does.

**The Readout Rule.** Tables, `<time>` and `[data-numeric]` elements are tabular-figured by default, so numbers line up like a readout.

## Layout

The shell is a two-column grid on medium screens and up: a sticky 15rem rail on the bay grey and a fluid main column. Below `md` (768px), the rail becomes a left drawer at `min(18rem, 85vw)`, opened from a menu button in the top bar. The top bar is sticky, 48px high, and uses the bay at 85% with a backdrop blur. It holds the environment band, the breadcrumb (EnvTag, environment name, "/", section icon and label), the live state and the ⌘K palette trigger.

Pages have 16px side padding (24px from `md`), 20px at the top, 40px at the bottom, and a 20px gap between blocks. A page title row places the headline and a muted meta line on the left and actions on the right, aligned to the bottom edge. The dashboard is laid out as bays. The main column holds the attention bay, the state bar and tabs, and the running bay. From `xl` (1280px), a 24rem right column holds the events tape, disk usage and inventory. That column splits into two at `md` and stacks on small screens.

Spacing follows a 4px base. Rows use 10px or 12px vertical padding, cells 12px or 16px horizontal. Groups have 6–8px gaps, and bays 20px. Control heights are 36px (default), 32px (small and nav rows), 24px (extra small), a 44px table row and a 36px table head.

## Elevation & Depth

The board is flat. Bays, strips and panels separate by tone (bay, strip) and by 1px rules, never by shadow. Shadows are used only for layers that float above the board, plus two small touches that give controls a physical edge.

### Shadow Vocabulary
- **Cord lip** (`box-shadow: inset 0 1px 0 oklch(1 0 0 / 0.18), 0 1px 2px oklch(0 0 0 / 0.2)`): the primary button only, giving it a lit top edge.
- **Seat** (`box-shadow: 0 1px 1px oklch(0 0 0 / 0.04)`): outline buttons.
- **Recess** (`box-shadow: inset 0 1px 1px oklch(0 0 0 / 0.05)`): inputs and textareas.
- **Menu float** (`box-shadow: 0 12px 32px -8px oklch(0 0 0 / 0.35), 0 1px 3px oklch(0 0 0 / 0.12)`): dropdown menus.
- **Dialog float** (`box-shadow: 0 24px 60px -12px oklch(0 0 0 / 0.45), 0 2px 6px oklch(0 0 0 / 0.15)`): dialogs and the command palette, over a `oklch(0.1 0.01 255 / 0.6)` scrim.
- **Auth card** (`box-shadow: 0 12px 40px -16px oklch(0 0 0 / 0.35)`): the single card on the login and setup screens.

### Named Rules
**The Flat Bay Rule.** Anything racked on the board has no shadow. A shadow means the layer can be dismissed.

## Shapes

Corners are tight and squared. They step from 2px (holders, EnvTags, badges, checkboxes, skeleton bars and link focus rings) to 4px (buttons, inputs, nav rows), 6px (panels, tables, readouts) and 8px (dialogs, the auth card). The holder and the EnvTag stay nearly square on purpose, because they are printed tags on a strip, not pills. Fully round shapes are kept for dots (live, up and down), progress tracks (state bar, disk and readout shares).

A strip is a box of cells. Table cells carry left rules (the rule at 50–70%) so each field reads as a boxed column, and rows are divided by full rules. Strip lists inside a panel run edge to edge, with the panel's own border as the frame.

## Components

### Buttons
Compact, with a physical edge, and they press 1px down.
- **Shape:** squared (4px).
- **Primary:** a cord fill with cord ink, the cord lip shadow, and 36px height with 16px padding. Hover drops the fill to 88%.
- **Outline:** a strip fill with a field rule edge and the seat shadow. Hover fills with the wash.
- **Ghost:** muted ink with no fill. Hover fills with the wash and brings the ink to full. Used for icon buttons in the top bar and the drawer.
- **Destructive:** a destructive fill. The ink is white in light mode and fail ink in dark mode.
- **Link:** ink text with a rule-coloured underline that turns cord on hover.
- **Focus:** a 3px cord ring. **Active:** a 1px downward press. **Transitions:** 150ms ease-out on colour, border, shadow and transform.
- **Sizes:** 24px, 32px, 36px and 40px, plus square icon sizes. Icons are 16px, or 12px at the smallest size.

### State Holder (signature)
A squared tag at the left of every strip.
- It is 20px high, at least 4.75rem wide and 2px rounded, and holds the state word in uppercase, exactly as Docker reports it.
- Run, wait and fail are filled with their state colour and state ink. Idle (created, exited cleanly) is a field-rule outline with muted ink.
- A container that exited with a non-zero code reads as fail. One that exited cleanly reads as idle.
- **Flash:** when Docker changes the state while it is on screen, the holder's ring expands from 0 to 7px as the cord fades out, over 900ms `cubic-bezier(0.16, 1, 0.3, 1)`. The cord colour is read from the current theme when the flash runs. The last state is remembered for each container ID, so a strip that moves to another bay still flashes. With reduced motion there is no flash.

### Strips and Bays
- **Attention bay:** a strip list in a panel, sorted most urgent first. Each strip has a holder, the container name in strip weight 600 (linked to its logs), the Docker status line in muted ink, and a muted "Logs" affordance. A strip whose link has focus fills with cord-soft.
- **Walk:** j/k or the arrow keys move focus from strip to strip, and Enter opens the focused strip's logs. A key legend in 10px mono sits beside the bay's headline.
- **State bar:** a 6px rounded track split into run, wait, fail and idle segments with 1px gaps. Below it, state tabs show a count (in the callsign cut) next to a holder, and each links to the filtered list.
- **Running bay:** strips with a holder, name, image, then CPU and memory cells. Each cell is a live 80×20 sparkline in ink at 75%, with a tabular value.
- **Sparklines:** a 1.25px line over a dim full-width track (the ink at 18%) that spans the 60-sample window, so a short history reads as recent rather than as a stub. The newest value is on the right, and values above the scale are clipped rather than rescaled. Memory sparklines are scaled to the container's memory limit, or to the host's MemTotal in the host readout.
- **Events tape:** a time-ordered strip list with mono timestamps, capped at 24rem. Its bottom edge fades out through a 2.5rem mask. The list is focusable for keyboard scrolling and shows a 2px cord outline inset on focus.

### Cards / Panels
- **Corner Style:** 6px.
- **Background:** strip, with a 1px rule border.
- **Shadow Strategy:** none (see the Flat Bay Rule).
- **Header:** a panel title with a 16px muted icon on the left and a muted label aside on the right, over a rule. The padding is 10px by 16px.
- **Internal Padding:** 16px, or flush for strip lists.

### Readouts
A 15.5rem strip cell in the page title row, one each for CPU and memory. It holds a muted label and icon, a 96×20 sparkline, a tabular value with a muted "of N" detail, and a 2px share track filled in ink at 60%. The track's width changes over 700ms.

### Tables
- The table sits in a strip panel with the strip width cut throughout. The head is the muted fill at 50%, with 12px/500 muted column labels.
- Cells are 44px high, divided by left rules. Row hover uses the wash at 60%. A selected row uses cord-soft, and the sort arrow is cord.
- The filter field is 32px with a search icon and a mono "/" key hint. The bulk-selection bar uses cord-soft with a cord edge at 40%.
- **Urgency bays:** when a list sorted ascending by state is racked into bays, each bay is its own row group labelled for assistive technology. On the containers list the bays are Needs attention, Changing, Running and Stopped. A sighted-only label row (28px, muted fill at 30%, 12px/500 muted label, tabular count at 70%) heads each bay.
- **Phone columns:** below 640px a table can keep only its phone columns. On the containers list these are the holder, name and status, and those cells wrap instead of truncating.
- Loading shows 2px-rounded pulsing skeleton bars. Errors appear as inline destructive text in the table body.

### Inputs / Fields
- **Style:** a field rule edge with the recess shadow and 4px corners, at 36px. The fill is strip in light mode and the bay at 60% in dark mode.
- **Focus:** the edge turns cord, with a 3px cord ring at 35%.
- **Error:** a destructive edge and ring. **Disabled:** 50% opacity.
- **Checkbox:** a 16px square with 2px corners that fills with the cord when checked.

### Navigation
- **Rail:** the bay grey with a rule on its right edge. From the top down, it holds the environment band, the FleetOps mark and wordmark (callsign cut), the environment switcher, the environment nav, the administration nav (admins only) and the user menu above a rule.
- **Nav row:** 32px high with 4px corners, a 16px Lucide icon at stroke 1.75, and a label in muted ink. Hover fills with the wash. The active row is cord-soft with ink text at 500 weight and a cord icon. Hovering a row reveals its chord legend ("G C") in 10px mono.
- **g-chord:** pressing g, then a page key within 1.2s, jumps to that page (d, c, i, v, n or p). The chord is ignored while typing or with a modifier held.
- **Environment switcher:** a 48px strip button tinted with the environment's hue, holding an EnvTag, a small muted "Environment" field label, the environment name, an up/down dot and a chevron.
- **Tabs:** a rule underline across the full width. The active tab has a 2px cord underline and ink text.
- **Mobile:** the rail moves into a left drawer with a scrim, and any navigation closes it.

### Environment Tag and Band
- **EnvTag:** a square with 2px corners (20px or 28px) in the environment's hue, holding up to two white initials in the strip cut at 700 weight. It is used in the breadcrumb, the switcher and the switcher menu.
- **Band:** a flat 3px strip of the hue across the top edge of the rail and the top bar. It is never shown without an environment.

### User Avatar
- **Blobatar:** every user is shown by a deterministic blobatar (`blobatar` + `@blobatar/react`) drawn from their username, so the same person is the same creature in the user menu, the Users list, the audit log and Account. It renders as a static `<img>` from a data URI (allowed by the CSP's `img-src data:`), with no network request and nothing stored.
- **Tile:** the face sits on a squared tile (20px with 2px corners, 28px with 4px, 40px with 6px) in a hairline border over the raised grey at 60%, like every identity mark on the board. The name always appears beside it, so the tile is `aria-hidden`.
- **System actor:** audit entries with no user show a Lucide `Bot` in the same tile, in the secondary text colour.
- Avatar hues are identity, like the EnvTag. They are not readings, and they never stand in for state.

### Live State
An 8px dot with a label in muted 12px text, at the right of the top bar. When the connection is live, the dot pulses in the live colour (a 2.4s ping, or none with reduced motion). Reconnecting shows a wait dot, and connecting a muted dot. While the connection is lost, a banner runs under the top bar with a wait fill at 12%, a wait edge at 40% and a spinning 16px refresh icon (2s turn, motion-safe), so it reads in the same tone as the dot. The dashboard's liveness line uses the same dots and says in words what is on screen and how old it is.

### Charts
The container stats charts (uPlot on canvas) sit in a strip panel with a 14px/600 title, a 160px plot, and axes in 11px JetBrains Mono. The legend uses the sans at 12px/500 in muted ink, with values in mono. They follow the Ink Chart Rule. A primary series is drawn in the ink over an 8% ink fill, and a secondary series (network out, block written) in the muted ink, dashed 4/3 with no fill. The axis uses the muted ink and the grid the rule. A canvas cannot read CSS variables, so each ink is read from the theme when the chart draws, and the chart redraws when the theme changes.

### Icons
Icons are Lucide, 16px in chrome (14px in breadcrumbs and readout labels, 20px for the bay headline's status icon). Every Lucide icon is drawn at stroke 1.75, set once for the whole app.

## Do's and Don'ts

### Do:
- **Do** build every new surface from the three greys (bay, strip, rule), with the float strip used only for layers that can be dismissed.
- **Do** mark the operator's place (primary action, selection, focus, current page) with the cord, and only with the cord.
- **Do** show a resource's state as a squared holder with Docker's exact word in uppercase, and read a non-zero exit as fail.
- **Do** set strip fields at width 88% and names that must be recognised at width 112%. Keep numerals tabular.
- **Do** give every environment-scoped view the environment's band and EnvTag, taking the hue from `envHue`.
- **Do** make any new strip list walkable with j/k and Enter when it is a list of things to act on.
- **Do** rack long lists that are sorted by state into labelled urgency bays, and name the phone columns that survive below 640px.
- **Do** say in words when the screen is not live, and how old the data is.

### Don't:
- **Don't** use amber to mean a state, a metric or a category. Transitional state is the orange wait reading, not the cord.
- **Don't** use green, orange or red for anything except Docker state, or add an environment hue inside the cord or state ranges.
- **Don't** colour a chart series with a state hue, the cord or a fixed hex. Use the ink and the muted ink.
- **Don't** put shadows on racked content (panels, strips, tables, readouts).
- **Don't** round holders or EnvTags into pills.
- **Don't** set human-facing labels in mono, or show Docker's error text in anything but mono.
- **Don't** set labels, headings or nav text in uppercase. Uppercase is kept for the holder's state word, EnvTag initials and key legends.
