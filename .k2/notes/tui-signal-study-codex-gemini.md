# TUI signal study: codex + gemini (2026-07-03, live PTY capture — PARTIAL)

Both agents auth-broken on this box (no tokens spent, no logins attempted):
- codex 0.125.0: refresh token dead since ~2026-04-27 → needs `codex login`.
  TUI signals captured; EXEC-APPROVAL UI NOT captured (needs a live turn).
- gemini 0.42.0→0.49.0 (self-updated via its npm auto-updater, disclosed):
  oauth-personal deprecated → Antigravity migration required. Working phase
  UNKNOWN → safe defaults; startup/trust/auth dialogs + exit captured.
RECAPTURE TODO once auth fixed: codex exec-approval strings; gemini shell
confirmation + working-phrase verification (harness rerunnable in scratchpad).

## CODEX 0.125.0
- title WORKING: `^[⠀-⣿] <cwd-basename>` (braille ~100ms cycle — same family
  as claude); IDLE: bare cwd basename; empty title on exit.
- bell: **FALSE on default config** (research assumption NOT confirmed —
  zero BELs incl. turn-ends with focus-out).
- working phrase CONFIRMED verbatim: `• Working (0s • esc to interrupt)`;
  also `• Booting MCP server: <name> (…esc to interrupt)` during startup.
- dialogs: cursor glyph `›` (U+203A) + numbered options, footer
  `Press enter to continue` (MISSES all current SELECT_MARKERS); trust
  dialog "Do you trust the contents of this directory?" (cyan-highlight
  selection, no cursor glyph).
- modes: NO alt screen (scroll-region inline UI), ?1004 focus ON, ?2026
  sync-output heavy, no mouse. ?2004h at 0.4s — UNTRUSTWORTHY (dialogs
  block input 9-13s while it's set).
- INJECTION HAZARD: startup dialogs eat keystrokes as hotkeys — a pasted
  digit can select "1. Update now (npm install -g)". Gate on: composer `›`
  + model footer (`gpt-5.5 high fast · <path>`) + absence of "Press enter
  to continue". Avoid stray Esc (double-Esc = edit-previous-message).
- exit: `/quit` → prints `To continue this session, run codex resume <uuid>`.

## GEMINI 0.42.0/0.49.0 (partial)
- title IDLE: `◇  Ready (<folder>)` — set while auth dialog still blocks
  ("Ready" title ≠ input ready); empty on exit. WORKING: unknown.
- bell: false. progress OSC: none. No alt screen; mouse explicitly disabled.
- 'esc to cancel' (existing WORKING_SIGNALS entry): UNVERIFIED — keep.
- trust dialog: "Do you trust the files in this folder?" options
  `● 1. Trust folder / 2. Trust parent folder / 3. Don't trust` (`●` radio,
  NO footer). Auth footer `(Use Enter to select)` — HITS existing
  "enter to select" marker. j/k move, digits select.
- lifecycle hooks LIVE-CONFIRMED: SessionEnd hook executed on quit
  (~/.gemini/settings.json wiring works).
- ?2004h early pathology (same as pi). INJECTION HAZARD: injected prose's
  `j` moved the trust radio; later Enter confirmed "Trust parent folder" —
  blind injection can silently grant trust. Gate on composer presence +
  absence of "(Use Enter to select)" / "Do you trust".
- exit: Ctrl+C ×2 → `To resume this session: gemini --resume <uuid>`.

## Table verdicts
- 'esc to interrupt' (codex): CONFIRMED. 'esc to cancel' (gemini): unverified, keep.
- SELECT_MARKERS: add "press enter to continue" (codex), keep "enter to
  select" (hits gemini auth).
- PERMISSION_MARKERS: add "do you trust" (both agents' trust dialogs match
  NOTHING today).
- Cursored-option detector: add `›` (U+203A, codex) + `●` radio rows (gemini),
  alongside grok's `(●)/(○)` and cursor's `→` from the sibling study.
- Side-effects cleaned: codex config.toml + gemini trustedFolders.json trust
  entries reverted byte-clean. Not reverted: gemini self-update 0.42→0.49.
