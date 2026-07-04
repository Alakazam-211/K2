# TUI signal study: grok / hermes / cursor-agent (2026-07-03, live PTY capture)

Slice-5 empirical profiles. Method: python PTY harness, 120x40, scratch cwds,
two runs per agent (trivial prompt; approval gate + decline). Versions:
grok 0.2.82; Hermes Agent 0.18.0 (Nous); cursor-agent 2026.07.01-41b2de7.

## GROK
- title WORKING: `^[⠀-⣿] - <phrase> - grok$` (Waiting for response…/Thinking/
  Responding); IDLE: `^grok$` or `<session title> - grok$` (no glyph);
  **PERMISSION: title prefix `⚠ Action Required - `** — cleanest HITL signal
  of any agent, no screen scan needed.
- bell: never. progress OSC: none (9;4 hint NOT confirmed on 0.2.82).
- working phrases (status row): `⠙ Waiting for response… 0.0s` + `[stop]`;
  busy footer `Esc:cancel │ Ctrl+.:shortcuts` (NO space before colon — the
  existing 'esc to cancel' entry can't match); transcript `◆ Thinking…`
  (U+2026 ellipsis, existing 'thinking...' misses it).
- permission gate rows: `1 (●) Yes, and don't ask again for anything
  (always-approve mode)` / `2 (○) Yes, proceed` / `3 (○) No, reject (type to
  add feedback)`; footer `1/3:select │ Ctrl+o:yolo │ Ctrl+c:cancel`.
  Radio glyphs (●)/(○), numbered — NO ❯ cursor.
  **⚠ NEVER blind-Enter a grok gate: default selection = always-approve.**
  Esc does NOT dismiss; decline = `3`+Enter or Ctrl+C (cancels turn).
- first send in a FRESH cwd is intercepted by a project-directory picker
  (`Run Grok Build in a project directory?`) — injection must handle modal
  #1 (second Enter selects "current directory", safe).
- modes: alt-screen + full mouse (?1049 ?1000 ?1002 ?1003 ?1006) + ?2004h at
  ~1.1s; bracketed-paste poll trustworthy for "UI mounted".
- exit: Ctrl+D twice within ~1s. Resume: --resume [id] / -c; **premints via
  --session-id (claude-style, confirmed)**.
- **HARNESS GOTCHA (Kessel PtyWrite): grok echoes unconsumed DCS replies
  (XTVERSION `ESC P>|… ST`) into the composer as literal text — never send
  DCS replies to grok panes; CSI replies fine.**

## HERMES
- NO titles ever (OSC 0/1/2 unused). No bell. No progress OSC.
- busy footer: `msg=interrupt · /queue · /bg · /steer · Ctrl+C cancel`
  (idle footer = bare `❯`) — 'msg=interrupt' is THE stable busy signal.
  Spinner line = rotating kaomoji + unstable verb `(◔_◔) formulating...`.
  Status bar: `⚕ <model> │ 20.7K/256K │ [█░…] 8% │ 9s`.
- permission gate: boxed `❯`-cursored numbered menu, header `⚠️ Dangerous
  Command`, options Allow once / Allow for session / Add to allowlist /
  Deny, footer `↑/↓ to select, Enter to confirm (59s)` —
  **60s countdown AUTO-DENIES on expiry** (hard deadline for HITL relay).
  Benign commands run ungated. `❯ 1.` already fires K2's numbered fast-path.
- bracketed paste: toggled h/l per repaint by prompt_toolkit, held ON during
  model wait — USELESS as ready/idle signal; readiness = screen text only
  (footer `❯` present, 'msg=interrupt' absent). Startup slow: prompt ~3.6s,
  +3s agent init on first message.
- no alt screen, no mouse. Exit: single Ctrl+C at idle (prints
  `Resume this session with: hermes --resume <id>`). No premint.

## CURSOR-AGENT
- title: no busy state; flips to conversation summary at turn completion
  (corroborating idle signal, not an encoding). No bell (OSC terminator BEL
  only). No progress OSC; queries OSC 11 repeatedly; sets ?2031 + ?1004.
- working: `⠰⠰ Working` / `⠠⠜ Running N tokens` status; input bar gains
  `ctrl+c to stop` while mid-turn (best signal; also present during its
  two-step rejection-reason composer — still "mid-turn").
- permission gate verbatim: `$ echo test in .` / `Run this command?` /
  `Not in allowlist: echo` / `→ Run (once) (y)` · `Add Shell(echo) to
  allowlist? (tab)` · `Run Everything (shift+tab)` · `Skip (esc or n)`;
  transcript `Waiting for approval...`. Cursor glyph `→ ` (trust dialog `▶`).
  Decline = Esc THEN a reason composer (two steps).
- **TRUST GATE**: first spawn in an untrusted dir blocks on `⚠ Workspace
  Trust Required` and SWALLOWS typed text; ?2004h not set until trusted →
  paste-mode poll correctly reflects composer existence. Trust persists
  per-directory. K2 must answer (send `a`) or pre-trust before injecting.
- no alt screen, no mouse. Exit: Ctrl+C twice at idle.

## Proposed table entries
WORKING_SIGNALS += 'esc:cancel' (grok), 'starting session…' + 'thinking…'
(U+2026 variants), 'msg=interrupt' (hermes), 'ctrl+c to stop' (cursor).
PERMISSION_MARKERS += "run this command?", "not in allowlist:", "waiting for
approval" (cursor); "dangerous command", "enter to confirm" (hermes);
"action required", "ctrl+o:yolo", "no, reject" (grok); "do you trust"
(cursor trust). SELECT_MARKERS += "↑/↓ navigate" (grok, no 'to'),
"↑/↓ to select" (hermes), "enter:submit" (grok), "press the key shown"
(cursor). Option-cursor detector: add `→ ` rows + grok radio rows
`^\s*\d+ \((●|○)\)`. Grok permission detectable from TITLE stream alone.
None of the three emit OSC 9;4/133 (pi remains the only one) or BEL.
Bracketed-paste-at-startup: trustworthy grok + trusted-dir cursor; useless
hermes.
