# TUI Mouse Interaction Study — claude fullscreen, SGR forwarding, OSC 52

Date: 2026-07-02 · claude 2.1.198 (bun-compiled Mach-O; JS is JSC bytecode, so
everything below is **empirical**, captured on this machine)

Method: (a) a Python/pyte PTY harness that spawns `claude` on a pty, logs every
DECSET/OSC-52/kitty sequence in the raw byte stream, renders the screen, and
injects scripted SGR mouse bytes; (b) a scratch K2 daemon built from this
worktree (`/cli/sessions/v2/spawn` + grid WS) for the WRAPLINE question, with
`/bin/bash` as control. TUI mode was pinned per run with
`--settings '{"tui":"default"}'` vs `'{"tui":"fullscreen"}'` (the owner's global
`~/.claude/settings.json` already has `"tui": "fullscreen"`).

---

## Part 1 — What claude actually requests and handles

### 1.1 DECSET matrix: normal (inline) vs fullscreen

| Mode | normal (`tui:default`) | fullscreen (`tui:fullscreen`) |
|---|---|---|
| `?1049` alt screen | **no** | **yes** |
| `?1000` click reporting | **no** | **yes** |
| `?1002` button-drag reporting | **no** | **yes** |
| `?1003` any-motion (hover) | **no** | **yes** |
| `?1006` SGR encoding | **no** | **yes** |
| `?1004` focus in/out | yes | yes |
| `?2004` bracketed paste | yes | yes |
| `?2031` color-scheme reports | yes | yes |
| `?2026` synchronized output | per-frame set/reset | per-frame set/reset |
| kitty keyboard | push `ESC [ > 1 u` (+ pop `ESC [ < u`, re-asserted around every render) | same |

- Fullscreen entry emits, in one burst:
  `\x1b[?1049h \x1b[?1000h \x1b[?1002h \x1b[?1003h \x1b[?1006h` — the full
  hypothesis confirmed, **plus ?1003 any-motion** (hover for menus/links) which
  the hypothesis didn't include.
- Exit resets everything (`?1006l ?1003l ?1002l ?1000l ?1049l ?1004l ?2031l
  ?2004l` + kitty pops). In normal mode it *also* defensively resets the mouse
  modes it never set.
- Other stream traffic seen: private DSR `\x1b[?6n` (DECXCPR cursor-position
  query — crashes pyte, harmless for alacritty), OSC 0 (title), **OSC 8
  (hyperlinks)**, OSC 9 (notification), OSC 52.
- Cross-validated through K2's own grid bits (scratch daemon, real
  alacritty): normal → `mouseReport=false sgrMouse=false altScreen=false`;
  fullscreen → all three `true`. So `grid_snapshot.rs`'s existing bits are a
  correct and sufficient gate.
- Fullscreen toggles (docs, confirmed by the settings key on this machine):
  `/tui fullscreen|default` (v2.1.110+), `CLAUDE_CODE_NO_FLICKER=1` (legacy),
  settings `"tui"`. Mouse kill-switches: `CLAUDE_CODE_DISABLE_MOUSE=1`,
  `CLAUDE_CODE_DISABLE_MOUSE_CLICKS=1` (v2.1.195+).

### 1.2 Behavioral verification — exact sequences that worked

Input row was `❯ zebra quokka wombat` at 1-based row 30 (100×32 pty);
"quokka" at cols 9–14, all injected over the pty as plain bytes:

| Action | Bytes sent | Observed result |
|---|---|---|
| Click in input text | `\x1b[<0;11;30M` then `\x1b[<0;11;30m` | claude's cursor jumped from end-of-line (x=21) to the clicked cell boundary (x=10); a subsequent `X` keystroke inserted mid-word (`zebra quXokka…`) |
| Drag-select | press `\x1b[<0;9;30M`, motion `\x1b[<32;10;30M` … `\x1b[<32;14;30M`, release `\x1b[<0;14;30m` | visual highlight painted DURING the drag and persisting after release — **explicit truecolor bg `#264f78`** (SGR 48;2), *not* reverse video |
| Copy | (automatic on release) | `\x1b]52;c;cXVva2th\x07` — OSC 52, clipboard target `c`, BEL-terminated; base64 `cXVva2th` = `quokka`. **Re-emitted on several subsequent repaints** while the selection stayed live (5× observed) — a K2 consumer must dedupe |
| Highlight + backspace | `\x7f` after the drag | highlighted range deleted → `❯ zebra  wombat` |
| Double-click word select | two press/release pairs at the same cell within ~120 ms | word highlighted + its own OSC 52 (`d29tYmF0` = `wombat`). My earlier attempt with ~450 ms spacing did NOT register — claude has a real multi-click threshold |

Notes:
- Release must carry the SGR `m` final with the *button number* (`\x1b[<0;…m`),
  which only SGR can express — another reason `?1006` is mandatory.
- Motion bit is +32 on the button (drag with left button = `32`); hover motion
  under `?1003` (no button) would be `35`.
- Modifier encoding is the xterm standard (+4 shift, +8 alt, +16 ctrl); claude
  itself documents cmd/ctrl+click for links (2.1.181+), plain click for cursor.
- **Normal mode ignores injected mouse bytes safely**: same click sequence at
  the `tui:default` prompt produced no cursor move and no junk characters —
  claude's kitty-aware input parser swallows the unrecognized CSI. (A correct
  terminal wouldn't send them anyway since reporting is off.)
- Kitty keyboard being pushed (`>1u` = disambiguate-escapes only) does NOT
  require the terminal to encode keys kitty-style: our legacy `\x7f` backspace
  and plain text worked throughout.

### 1.3 WRAPLINE / soft-wrap verdict (owner follow-up)

Probed via the scratch K2 daemon's grid WS (80×26), which marks alacritty's
`Flags::WRAPLINE` as `wrapped: true` on the last CellRun of a soft-wrapped row
(`crates/k2-core/src/terminal/grid_snapshot.rs:102-110`, `:277-301`):

- **Control** — `echo <161-char sentence>` in bash: continuation rows carry
  `wrapped:true` (4 flagged rows). The plumbing works.
- **claude, normal mode**: model response painted as 3 visual rows, typed
  150-char input echoed as 2 rows — **0 wrapped-flagged rows anywhere**.
- **claude, fullscreen mode**: same — **0 wrapped-flagged rows**.

Verdict: **claude pre-wraps ALL of its text (prose output and input echo) to
the terminal width itself and paints each visual line as a discrete write in
both modes.** The terminal never soft-wraps, so `WRAPLINE` never sets, so K2's
grid-side logical-line rejoin (`TerminalPane.tsx` `handleCopy`, which joins
`wrapped` rows without a newline) can never reconstruct claude's logical lines
from grid data. Logical-line copy of claude content is only achievable via the
app's OWN selection + OSC 52 — which is exactly what claude ships. This
upgrades OSC 52 handling from nice-to-have to the only correct copy path for
TUI content.

Also answered: in normal (inline) mode claude enables **no** mouse reporting,
so K2's native selection remains fully available there; the whole forwarding
question is scoped to fullscreen/alt-screen apps.

---

## Part 2 — How terminals arbitrate forwarding vs local selection

### xterm.js
- Mousedown gate: forward to app only when mouse events are active AND no
  override modifier —
  `src/browser/services/MouseService.ts:239`:
  `if (!this._mouseStateService.areMouseEventsActive || this._selectionService.shouldForceSelection(ev)) return;`
  (local `SelectionService.handleMouseDown` is bound independently in
  `src/browser/CoreBrowserTerminal.ts:618`).
- The override convention: `src/browser/services/SelectionService.ts:437-447`
  `shouldForceSelection()` → **Shift** on Win/Linux; on macOS
  `event.altKey && macOptionClickForcesSelection` (Option-drag); plus an
  optional inverted `mouseEventsRequireAlt` mode. The selection service is
  `disable()`d while mouse events are active (`CoreBrowserTerminal.ts:621-628`)
  but still honors the forced path (`SelectionService.ts:471-478`).
- Encoding: `src/common/services/MouseStateService.ts:91-114` (`eventCode`:
  SHIFT=4 ALT=8 CTRL=16 OR'd in; MOVE adds 32; wheel = 64|action) and the SGR
  encoder at `:143-146` — final `m` only for UP of a non-wheel button.
- Wheel-specific gating is separate from buttons: `MouseService.ts:468`
  `_consumeWheelEvent` returns 0 when `ev.shiftKey` (shift-wheel stays local),
  and an embedder `customWheelEventHandler` can veto forwarding
  (`MouseStateService.ts:235-237`). xterm.js also strips Alt from forwarded
  reports when Alt is the local-override key (`MouseService.ts:180-186`) so
  apps like tmux don't see phantom modifiers.

### alacritty
- Press: `alacritty/alacritty/src/input/mod.rs:618-619` `on_mouse_press` —
  `if !shift && self.ctx.mouse_mode() { …mouse_report(code, Pressed) } else { local click/double/triple-click selection }`.
  Release mirrors it at `:697-705`.
- Motion: `:500-516` — local selection updates when
  `(lmb||rmb pressed) && (shift || !mouse_mode)`; otherwise report motion
  (button+32) **only when the cell changed**, and buttonless hover (35) only
  under `MOUSE_MOTION` (?1003).
- Modifier encoding +4/+8/+16 at `:552-561`; SGR string
  `\x1b[<{b};{col+1};{line+1}{M|m}` at `:1103`.
- Copy-on-release for its own local selection, with the comment "to prevent
  flooding the display server" (`:720`) — same pattern claude implements
  app-side.
- Wheel (`scroll_terminal`, `:760-806`): mouse mode → SGR wheel buttons
  64/65/66/67; else alt-screen + alternate-scroll and **not shift** → arrow-key
  fallback; shift always keeps the wheel local.
- Even the cursor icon follows the convention: Text (I-beam) when
  `shift || !mouse_mode`, Default otherwise (`:1108-1113`).

### wezterm (incl. shared/mux sessions)
- `bypass_mouse_reporting_modifiers` — default **SHIFT**
  (`config/src/config.rs:1786-1788`); the gate strips the modifier and turns
  reporting off for that event
  (`wezterm-gui/src/termwindow/mouseevent.rs:923-930`).
- OSC 52 in a mux/shared session: the server forwards it as
  `Pdu::SetClipboard`; the **attached client** applies it to its local
  clipboard (`wezterm-client/src/pane/clientpane.rs:152-168`); a local GUI pane
  applies it directly (`wezterm-gui/src/termwindow/clipboard.rs:21`).
- tmux (documented `set-clipboard` behavior, for the shared-session question):
  tmux both stores the payload in its internal paste buffer AND passes OSC 52
  through to **every attached client** whose outer terminal advertises the
  `Ms` capability.

### alacritty_terminal — what K2's engine already gives us
- OSC 52 is parsed and surfaced as `Event::ClipboardStore(ClipboardType,
  String)` (`alacritty_terminal/src/term/mod.rs:1706-1719`), policy-gated by
  `config.osc52` whose **default is `OnlyCopy`**
  (`term/mod.rs:372-384`) — copy allowed, clipboard *read* (paste) denied.
  K2 pins `alacritty_terminal = "0.26.0-rc1"` (`crates/k2-core/Cargo.toml:118`).
- **K2 drops it today**: the `EventListener` in
  `crates/k2-core/src/terminal/alacritty_backend.rs:38-57` matches
  Wakeup/Title/Bell/ChildExit/Exit and swallows everything else in `_ => {}`.
  Nothing clipboard-related exists in `crates/k2-daemon/src/sessions_grid_ws.rs`
  or the `TerminalEventSink` trait
  (`crates/k2-core/src/terminal/event_sink.rs`).

---

## Part 3 — K2 implementation plan (2–3 PR-sized slices)

Daemon input path needs nothing for forwarding: the grid-WS `input` action
already carries arbitrary text and the wheel branch proves SGR bytes ride it
fine. Slice 2 is the only daemon work (OSC 52 egress).

### Slice 1 — client: forward press/drag/release as SGR (TerminalPane only)
- Gate: `snap.mouseReport && snap.sgrMouse` (the same predicate the wheel
  branch uses at `src/renderer/terminal-v2/TerminalPane.tsx:2463`). SGR-only,
  same as wheel: legacy X10 byte encoding can't ride the JSON text channel.
- On `pointerdown` (with pointer capture so drags outside the pane keep
  reporting) / `pointermove` / `pointerup`:
  - cell math: reuse the wheel/hover math
    (`TerminalPane.tsx:2469-2477`): `col = max(1, floor((clientX-rect.left-4)/cellW)+1)`,
    same for row — SGR is 1-based.
  - button: left=0 middle=1 right=2; motion adds **32**; modifiers add +4
    shift / +8 alt / +16 ctrl (xterm standard; strip the modifier that
    triggered a local override, as xterm.js does).
  - press/motion → final `M`; release → final `m` with the real button number.
  - **Motion only on cell change** (alacritty's rule, mod.rs:513) — this alone
    collapses a pixel-granular drag into ≤ cols+rows events.
- Local-selection override (the convention all three terminals share):
  **Shift-drag → K2's native DOM selection** (and **Option-drag on macOS**,
  matching iTerm/xterm.js `macOptionClickForcesSelection`); plain drag goes to
  the app when reporting is on. Implementation: check the modifier FIRST; if
  overriding, skip forwarding and let today's native-selection path run
  untouched. When forwarding, `preventDefault()` + `user-select: none` on the
  row surface for the duration so the browser doesn't paint a phantom local
  selection alongside claude's highlight. (terminal-v2's selection is native
  DOM over the row divs — there is no WebGL selection in terminal-v2 today —
  so suppress/restore is one class toggle.)
- Link interplay: keep the existing order — `linkClickMode==='cmd-click'` +
  `cmdHeldRef` (`TerminalPane.tsx:2148-2208`) wins over forwarding, so
  cmd-click still opens links locally and is NOT reported (mirrors claude's own
  cmd/ctrl-click-for-links behavior — double-handling would open it twice).
- Rate ceiling: reuse the wheel token-bucket shape
  (`TerminalPane.tsx:2438-2521`, 50 ms flush + cap) for drag-motion: coalesce
  to the LATEST cell per flush tick (~30 ms ⇒ ≤ ~33 motion events/s max on the
  wire) — motion is idempotent-latest, unlike wheel notches, so coalescing is
  lossless. Press/release always flush immediately and never coalesce.
- Wheel path: unchanged.
- Acceptance: in a K2 pane running `claude` fullscreen — click moves its
  cursor; plain drag paints claude's `#264f78` highlight; backspace deletes the
  range; shift-drag still makes a K2-native selection; cmd-click opens links
  once.

### Slice 2 — OSC 52 copy surface (daemon + client)
- `alacritty_backend.rs`: handle
  `AlacEvent::ClipboardStore(ty, text)` → new
  `TerminalEventSink::on_clipboard(terminal_id, selection, text)`
  (`event_sink.rs`). Keep alacritty's `Osc52::OnlyCopy` default — do NOT
  enable `ClipboardLoad` (an app reading the viewer's clipboard is a data
  exfiltration primitive; alacritty's own default agrees, and dropping the
  Load event means no response bytes are ever written, which is the safe
  no-op).
- `sessions_grid_ws.rs`: new `Outbound::Clipboard { selection, text }` frame
  (snake_case tag `clipboard`), size-capped (e.g. 1 MiB, oversize dropped with
  a log). Fan out to **all** connected grid-WS subscribers of the session —
  the daemon should not guess focus.
- Client policy (the multi-viewer/remote question): **apply to the OS
  clipboard only in the pane that is the active viewer** —
  `computeDesiredActive` in `src/renderer/terminal-v2/activeViewer.ts` already
  defines exactly this (visible + pane-focused + window-focused). This is the
  wezterm shape (server broadcasts `SetClipboard`, the attached client
  applies) with a K2 refinement: tmux stomps every attached client's
  clipboard, which is hostile when one owner watches the same session from a
  Mac and a phone; active-viewer-only means "the person actually looking at it
  gets the copy", and a remote K2 Connect viewer gets it on their local
  machine (which is the whole point — copy works remotely without the daemon
  touching macOS pasteboard APIs).
  - Dedupe consecutive identical payloads per session (claude re-emits the
    same OSC 52 on every repaint while a selection is live — 5× in one
    capture).
  - `navigator.clipboard.writeText` from the renderer; CLI/companion viewers
    can adopt the same frame later.
- Acceptance: drag-select in claude fullscreen inside K2 → paste anywhere on
  the viewer's machine yields the selected text; a second connected viewer
  that isn't focused does not have its clipboard replaced.

### Slice 3 — parity polish + guardrails
- `?1004` focus events: claude sets it in BOTH modes — send `\x1b[I` / `\x1b[O`
  on active-viewer gain/loss (activeViewer transitions already computed).
  Cheap, and claude likely uses it to pause spinners/notifications.
- `?1003` hover motion (buttonless, button code 35): claude sets it for
  hover-highlight of menus/links. Forward it **cell-change-gated + same token
  bucket**, and consider default-off over K2 Connect sessions (it's the only
  event class that fires with no button held, i.e. the flood risk); local
  daemon connections can default on.
- Settings: one "Terminal mouse reporting" toggle (default on) mirroring
  `CLAUDE_CODE_DISABLE_MOUSE`, tooltip documenting Shift/Option-drag = local
  selection. Optional later: kitty-keyboard-aware key encoding (claude pushes
  `>1u` but demonstrably accepts legacy input, so not needed for this feature).
- Risks & mitigations recap:
  - motion flood over K2 Connect → cell-change gate + latest-cell coalescing
    + 50 ms bucket (Slice 1), hover class default-off remote (Slice 3);
  - user expects local selection mid-TUI → Shift/Option override (the
    universal convention) + the setting;
  - clipboard stomping / multi-viewer double-copy → active-viewer-only apply +
    payload dedupe + never implementing OSC 52 read-back;
  - accidental double link-open → cmd-click handled locally and not forwarded.

---

## Appendix — raw evidence pointers (scratchpad, session-local)
- Harness: `scratchpad/harness.py`; scenarios `scen-*.txt`; captures
  `out-default-boot/`, `out-default/`, `out-fullscreen/`, `out-drag/`,
  `out-drag2/` (each: `raw.bin`, `events.log`, `screen-*.txt` with bg/reverse
  maps).
- Grid-WS wrap probes: `wrap-probe.ts`, logs `wrap-claude-default.log`,
  `wrap-claude-fullscreen.log` (bash control inline in transcript).
- Local clones cited: `terminal-research-repos/{xterm.js,alacritty,wezterm}`.
