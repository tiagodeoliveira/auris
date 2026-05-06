# PWA UX

The PWA is the mobile control surface during a meeting. It runs inside
the EvenHub Flutter WebView on a phone and inside the EvenHub simulator
during development; it is not designed as a desktop browser app.

This doc captures the design system, the screen-state model, and the
interaction patterns. The visual language is a shared light theme used
by both the PWA and the Mac overlay: glassy white/ice panels, blue
primary actions, amber moment capture, red destructive actions, and
dark high-contrast text.

---

## Design tokens

Defined in `packages/pwa/src/style.css` at the top of the file as CSS
variables.

### Palette

| Token            | Hex       | Use                                                |
| ---------------- | --------- | -------------------------------------------------- |
| `--bg-primary`   | `#eef4fb` | App background.                                    |
| `--bg-secondary` | `#f8fbff` | Top bar, sticky CTA region, soft sections.         |
| `--bg-card`      | `#ffffff` | Cards, inputs, settings modal.                     |
| `--text-primary` | `#243044` | Body text.                                         |
| `--text-light`   | `#17212e` | Headings, primary content.                         |
| `--text-muted`   | `#6b7b8e` | Mono labels, hint text, disabled affordances.      |
| `--brand-blue`   | `#2563eb` | Primary action, focus, active modes, memory badge. |
| `--moment-amber` | `#f2b705` | Moment capture and moment feedback.                |
| `--danger-red`   | `#e5484d` | Stop/destructive actions and error states.         |
| `--border-dark`  | `#d5dee9` | Section dividers.                                  |
| `--border-light` | `#c7d2df` | Input / chip borders.                              |

The old `--rust-*` variables remain as aliases for compatibility while
older components are migrated.

### Typography

Loaded from Google Fonts in the PWA:

- **Display** — `Bebas Neue`, used sparingly for mobile screen titles.
- **Body** — `Space Grotesk`, used for textareas, item bodies, button
  labels, settings forms.
- **Mono** — `JetBrains Mono`, used for technical labels (mode tabs,
  metadata keys, timestamps, the memory badge). Always small, always
  uppercase, always letter-spaced.

The `.label-mono` utility class applies the technical-label treatment.

### Geometry

- Border radius: `--radius: 6px` baseline; pills use `999px`.
- Border weight: `1px`. Dashed for "draft / not yet committed"
  affordances (`+ ADD`, `EXTRACT TAGS` when armed).
- Section padding: `16px` to `24px` horizontal depending on density.

### Mac overlay companion theme

The Mac overlay uses native SwiftUI fonts rather than PWA web fonts, but
shares the same semantic palette:

- panel `#f7fafe`, card `#ffffff`, input `#eef4fa`
- text `#17212e`, muted `#647386`
- blue `#2563eb`, amber `#f2b705`, danger `#e5484d`

The overlay keeps one stable wide footprint across compose, starting,
and live states. Active meeting mode is a horizontal HUD: status/control
rail on the left, mode tabs and transcript on the right, moment feedback
as a separate pill so labels do not truncate.

---

## Screen states

The PWA has four logical states, identified by the combination of
`meetingState` and `glassesView`.

| State                 | `meetingState` | `glassesView` | Visible components                                                                        |
| --------------------- | -------------- | ------------- | ----------------------------------------------------------------------------------------- |
| Idle                  | `idle`         | `idle`        | top-bar, compose-region, kv-editor, compose-start                                         |
| Listening (dictation) | `idle`         | `listening`   | top-bar, compose-region (mic active), kv-editor, compose-start                            |
| Active meeting        | `active`       | `active_list` | top-bar, header-strip (with memory badge), kv-editor, mode-tabs, items-mirror, cta-region |
| Paused meeting        | `paused`       | `active_list` | same as Active; cta-region shows Resume                                                   |

Each component subscribes to `meetingState` and toggles its own
`display: none` outside its valid states. The parent index.ts is
unaware of visibility logic.

### Mount order = layout

Components are appended to `#app` in this order, top to bottom:

```
top-bar              # always
compose-region       # idle only
header-strip         # active/paused only
kv-editor            # always (wedged between idle compose and active header)
compose-start        # idle only
mode-tabs            # active/paused only
items-mirror         # active/paused only
cta-region           # active/paused/listening
[overlays: settings-modal, toasts, error-overlay]
```

The kv-editor's _position_ between the idle and active surfaces is the
key to avoiding layout jumps when a meeting starts. It stays visually
docked below either compose-region (idle) or header-strip (active),
because both are in the same layout slot.

---

## Interaction patterns

### Compose surface (idle)

A multi-line textarea with a microphone toggle and an Extract Tags
button.

**Mic toggle** — click to start a Soniox dictation session that fills
the textarea live with finalized + interim tokens. Click again to stop;
the transcript stays in the textarea, editable. The mic button is the
listening-state UI; the cta-region renders nothing during listening.

**Extract Tags** (`▸ EXTRACT TAGS`) — visible when the description
has content, sends `extract_metadata` to the server. Becomes
`▸ EXTRACTING…` while the LLM is working; chips appear in kv-editor
on success. Cmd/Ctrl+Enter inside the textarea is the keyboard
shortcut.

**Start Meeting** — full-width rust gradient. Sends `start_meeting`
with the description but _without_ a `metadata` field, so the server
preserves whatever's in `state.metadata` (extracted + edited chips).

### Metadata chips

Each metadata entry is a small pill with monospace key + transparent
inline-edit value input + delete button. Edit the value, press Enter
or blur — the change saves. Escape reverts. Add new entries via a
`+ ADD` chip that turns into a chip-shaped editor with `key` and
`value` placeholders.

The chip strip wraps as needed so 5+ fields fit in 1–2 rows. The full
form modal of earlier iterations is gone.

### Active meeting

**Header strip** — display title, elapsed timer, optional project tag,
and the memory badge (`★ memory · N recalled`) when mnemo recall
populated context.

**Mode tabs** — four monospace labels: `HIGHLIGHTS · TRANSCRIPT ·
ACTIONS · QUESTIONS`. Active tab shows a rust underline. Click to
emit `set_mode`.

**Items mirror** — scrollable list of cards, one per `Item`. Each
card has: timestamp pill, body text, optional mode-specific meta line
(owner/due for actions, importance for highlights, kind/context for
open_questions, speaker for transcript).

In transcript mode, when `transcript_interim` is non-empty, a dim
italic "live row" appears at the bottom of the list with `[ ⋯ ]` for
its timestamp. Auto-scroll keeps it visible.

### Stop confirmation

The Stop button arms on first tap (briefly shows "Tap again to
confirm") and commits on second. Prevents accidental data loss for a
non-undoable action. See ADR-0001 for why this is in-PWA rather than
on a glasses gesture.

### Settings modal

Reachable via the gear icon in the top bar. Two fields: server URL,
server token. Persisted to both `bridge.setLocalStorage` and browser
`localStorage` — read prefers the bridge with `localStorage` as
fallback. The `WS · BLE` indicator pair updates live based on
`wsStatus` and `bleConnected`.

### Toasts and errors

- **Toasts** — bottom-right, 4 s TTL. Used for transient warnings
  (extraction failed, set_mode failed). `error` events are surfaced as
  toasts.
- **Error overlay** — full-screen, non-dismissable. Used only for
  protocol-version mismatch (the only "this is unrecoverable until
  somebody updates code" condition).

---

## Memory badge

When the server emits a `prior_context_changed` event with non-empty
counts, the active-meeting header shows a small rust pill:

```
★ memory · N recalled
```

Where `N` is the total across `preferences + facts + episodes +
project_memories`. Hover (or long-press on touch) reveals a tooltip
with the per-dimension breakdown:

```
Prior context loaded for the LLM extractors:
3 preferences
12 facts
4 past discussions
2 project memories
```

The pill is hidden when `priorContext` is `null` (no recall yet,
recall failed, or recall returned empty). See ADR-0008 for what it
implies about the LLM extractor prompts.

---

## Component map

Every UI component lives in `packages/pwa/src/ui/` as a `mount<Name>`
function called once at boot from `ui/index.ts`. They share state via
the typed `Store<AppState>`.

| File                | Role                                                |
| ------------------- | --------------------------------------------------- |
| `top-bar.ts`        | WS / BLE indicators, settings gear. Always visible. |
| `compose-region.ts` | Idle textarea + mic + Extract Tags.                 |
| `compose-start.ts`  | Idle Start Meeting button.                          |
| `header-strip.ts`   | Active title + timer + memory badge.                |
| `kv-editor.ts`      | Metadata chip strip.                                |
| `mode-tabs.ts`      | Active mode selector.                               |
| `items-mirror.ts`   | Active items list with live transcript row.         |
| `cta-region.ts`     | Active Pause/Stop / Listening status.               |
| `settings-modal.ts` | Server URL / token form.                            |
| `toast.ts`          | Bottom-right transient messages.                    |
| `error-overlay.ts`  | Full-screen unrecoverable error.                    |

### Adding a new screen state

1. Add the new state value to `AppState` (typically a new field like
   `myFlag: boolean`).
2. Write a self-hiding component that subscribes to the slice and
   toggles its own visibility.
3. Mount it in `ui/index.ts` in the right vertical position relative
   to the existing components.

You should never need to modify the parent. If you do, the
self-hiding pattern is leaking and worth re-examining.
