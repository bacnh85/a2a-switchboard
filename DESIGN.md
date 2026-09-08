---
name: Switchboard Telemetry
description: Light ops-console system for the a2a-switchboard admin UI — teal signal accent, zinc neutrals, data-is-mono discipline, de-carded flat surfaces.
colors:
  primary: "#18181b"
  accent: "#115e59"
  accent-hover: "#134e4a"
  accent-text: "#ffffff"
  text: "#18181b"
  text-muted: "#52525b"
  bg: "#fafafa"
  surface: "#ffffff"
  surface-muted: "#f4f4f5"
  border: "#e4e4e7"
  danger: "#7f1d1d"
  danger-bg: "#fee2e2"
  success: "#166534"
  success-bg: "#dcfce7"
  warning: "#92400e"
  warning-bg: "#fef3c7"
  identity-0: "#115e59"
  identity-0-bg: "#ccfbf1"
  identity-1: "#1e40af"
  identity-1-bg: "#dbeafe"
  identity-2: "#5b21b6"
  identity-2-bg: "#ede9fe"
  identity-3: "#881337"
  identity-3-bg: "#ffe4e6"
  identity-4: "#92400e"
  identity-4-bg: "#fef3c7"
  identity-5: "#166534"
  identity-5-bg: "#dcfce7"
  identity-6: "#164e63"
  identity-6-bg: "#cffafe"
  identity-7: "#7c2d12"
  identity-7-bg: "#ffedd5"
  chat-own: "#d6f0ec"
typography:
  body:
    fontFamily: system-ui, -apple-system, "Segoe UI", sans-serif
    fontSize: 1rem
    lineHeight: 1.5
  data:
    fontFamily: ui-monospace, "SF Mono", Menlo, Consolas, monospace
    fontSize: 0.875rem
    fontVariantNumeric: tabular-nums
  h1:
    fontSize: 1.953rem
    fontWeight: 700
    lineHeight: 1.2
  h2:
    fontSize: 1.563rem
    fontWeight: 700
  h3:
    fontSize: 1.25rem
    fontWeight: 600
  label:
    fontSize: 0.875rem
    fontWeight: 500
rounded:
  sm: 4px
  md: 6px
  lg: 8px
spacing:
  xs: 4px
  sm: 8px
  cell: 12px
  md: 16px
  lg: 24px
  xl: 32px
components:
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.accent-text}"
    rounded: "{rounded.md}"
    padding: "6px 14px"
    fontWeight: 500
  button-primary-hover:
    backgroundColor: "{colors.accent-hover}"
  button:
    backgroundColor: "{colors.surface-muted}"
    textColor: "{colors.text}"
    rounded: "{rounded.md}"
    padding: "6px 14px"
  button-danger:
    backgroundColor: "{colors.danger-bg}"
    textColor: "{colors.danger}"
  badge-ok:
    backgroundColor: "{colors.success-bg}"
    textColor: "{colors.success}"
    rounded: "{rounded.sm}"
  badge-bad:
    backgroundColor: "{colors.danger-bg}"
    textColor: "{colors.danger}"
    rounded: "{rounded.sm}"
  instrument-panel:
    backgroundColor: "{colors.surface}"
    rounded: "{rounded.lg}"
    shadow: elev-1
  data-table:
    backgroundColor: "transparent"
    textColor: "{colors.text}"
    rowSeparator: "1px solid {colors.border}"
  chat-avatar:
    backgroundColor: "{colors.identity-0-bg}"
    textColor: "{colors.identity-0}"
    size: 28px
    rounded: "9999px"
    typography: "{typography.data}"
  chat-bubble-own:
    backgroundColor: "{colors.chat-own}"
    textColor: "{colors.text}"
    rounded: "{rounded.lg}"
    padding: "6px 10px"
  chat-bubble:
    backgroundColor: "{colors.surface-muted}"
    textColor: "{colors.text}"
    rounded: "{rounded.lg}"
    padding: "6px 10px"
---

## Overview

Admin console for an A2A message switchboard: live routing topology, dense
audit logs, peer registry, credential handling. Identity: **switchboard
telemetry** — a calm light ops console where data is monospace and color is
reserved for signal (state, health, errors). One accent, neutrals everywhere
else. Separation by whitespace → background shift → elevation, in that order.

## Colors

- **accent (#115e59, deep teal):** the sole interaction color — links, primary
  actions, live edges and packets. Telecom-signal vernacular; intentionally NOT
  default blue/violet. Dark enough to pass APCA Lc ≥ 75 on both bg and surface.
- **primary/text (#18181b):** ink for headings and body on zinc neutrals.
- **text-muted (#52525b):** metadata, labels, captions. (One muted level only.)
- **bg (#fafafa) / surface (#ffffff) / surface-muted (#f4f4f5):** layered
  neutrals. surface-muted is the hover/quiet-fill layer, not a card color.
- **success / danger / warning:** semantic state ONLY — health, errors,
  pending. Never decoration. Each pairs with its *-bg tint for badges.
- **identity-0..7 (+ -bg tints):** participant identity colors for the
  chat/messenger — a data-viz category palette (like chart series), NOT a
  second accent. Dark fg on light same-hue tint; stable per-node assignment
  by hash. Never used for buttons, links, or decoration outside chat.
- No gradients. No glow. No dark mode (deliberate: light console identity).

## Typography

System sans for UI chrome and prose; **ui-monospace for ALL data** — peer
names, URLs, IPs, tokens, timestamps, methods, statuses, numeric cells — with
`font-variant-numeric: tabular-nums` so live-updating figures don't jitter.
Hierarchy by weight on a 1.25 modular scale (0.75 / 0.875 / 1 / 1.25 / 1.563 /
1.953 rem). Sentence case everywhere — headers, buttons, labels. No ALL-CAPS,
no letter-spacing on text.

## Layout

- Fixed 256px left sidebar (brand ◈, nav, version + sign out), content column
  max 1280px on an 8px spacing grid.
- KPI row: de-boxed numbers (plain on bg, tile on hover — they are links).
- Tables sit directly on bg with hairline row separators — no card wrapper.
- Instrument panels (topology canvas, login) are the only boxed surfaces.
- Mobile ≤ 880px: sidebar collapses to a top row; tables get horizontal
  scroll wrappers; flow-log trims method/ms columns.

## Elevation & Depth

Named levels only: `elev-0` none · `elev-1` resting instrument panel ·
`elev-2` login/floating · `elev-3` reserved (future popovers). Buttons and
tables are FLAT at rest — depth belongs to the map and overlays only.

## Shapes

Radius scale 4 / 6 / 8 px: sm on badges/chips, md on controls, lg on panels.
Topology node pills 17px half-height radius (SVG), gateway 10px rect.

## Components

- **button / button-primary / button-danger:** flat, 6px 14px, all five states
  (default/hover/focus-visible/active/disabled); danger confirms via inline
  `<details>` disclosure before submit.
- **data-table:** sentence-case header row in text-muted, hairline row rules,
  cell padding 8×12 (sm + cell token), mono data cells, right-aligned tabular
  numerics, ellipsified long names/URLs (title attr carries the full value),
  row hover = surface-muted.
- **stat (KPI):** 1.953rem mono number + 0.875rem muted label, plain on bg;
  hover tile surface-muted; hairline rule above the row. Severity = the number
  itself tinted warning/danger (the label carries the meaning for a11y).
- **status:** quiet by default — plain mono text for 2xx; `badge-bad` pill only
  for ≥400, plus 3px danger left border on the error row.
- **instrument-panel:** topology canvas = surface-muted tint, radius lg, no
  shadow (bg-shift separation); white node pills pop on it; wide flat ellipse
  layout with half-step start angle so small peer counts sit left/right of the
  gateway, not stacked. Communication log and settings sections = white
  surface, radius lg, no shadow. Login = white + elev-2 (the one floating
  surface).
- **token row:** masked-by-default mono token + Reveal + Copy controls.
- **chat (messenger):** two-pane grid (280px list | thread). Conversation
  items: 28px avatar circle (identity-<n> tint, dark initial), title,
  one-line muted preview, accent unread pill. Bubbles: max-width 78%,
  surface-muted for others, chat-own (accent-hue tint) for own, right-aligned
  and reversed for own; 4px gap between messages; sender name in identity
  color above the bubble; timestamp + tight overlapping SVG ✓/✓✓ delivery
  tick in the meta row; lone three-dot typing bubble while a send waits on
  an agent reply (reduced-motion: static dots); slash-command popover above
  the composer when typing / in a gateway DM (surface + elev-3, mono
  accent command names, muted descriptions — mirrors gateway_agent's
  commands); system/roster events centered, muted (err → danger). Composer: identity select, emoji
  popover (surface + elev-3), auto-growing textarea, primary Send. Mobile
  ≤880px: single pane, back button, list/thread swap via .thread-open.

## Do's and Don'ts

- DO declare default / hover / focus-visible / active / disabled on every
  interactive element; keyboard nav is primary.
- DO respect `prefers-reduced-motion`: the live packet animation and any
  transition ship a reduced-motion fallback (JS gates SMIL; CSS gates the rest).
- DO keep the log quiet: OK is the default state and stays unstyled; errors are
  the only colored rows.
- DON'T use card boxes for tables or KPIs; DON'T put shadows on buttons.
- DON'T use backdrop-filter, gradient orbs, colored glow, ALL-CAPS tracked
  labels, or 1px gray card borders — AI-slop signatures.
- DON'T introduce a second accent or tint pure black/white. (The chat
  identity palette is participant data-viz, not an accent.)
- DON'T color room/DM list chrome with identity colors — they identify
  nodes only (avatars, sender names).
