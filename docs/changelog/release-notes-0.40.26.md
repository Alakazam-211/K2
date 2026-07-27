# K2 0.40.26 — Agents can ask you things

## Agent→Human Feedback system

Agents get a durable channel to ask humans for input — questions,
approvals, FYIs — that survives terminal scrollback and dormant sessions.
Modeled on AFSROW's feedback system (two tables: `feedback` +
`feedback_comments`, migration 0064). Everything is daemon-resident:
remote hosts expose their own queue to any front-end, and answering from
anywhere injects into that server's sessions.

- **`k2 feedback` CLI** (comprehension-gated 36/36 before build): `ask`
  (with `--kind question|approval|fyi`, `--options` one-tap choices,
  `--priority`, `--body/--body-file`), `list`, `show`, `comment`,
  `resolve`. `ask --wait [--timeout N]` blocks until answered and prints
  the bare answer (exit 0), safe for `$(...)` capture; timeout and
  closed-without-answer produce empty stdout + typed JSON on stderr +
  exit 1. Errors are ALWAYS JSON on stderr; short unique id prefixes work
  everywhere. `--options` splits on unescaped commas only — `\,` embeds a
  literal comma in a label.
- **Session auto-detect:** asks filed from sandbox cells carry
  `K2_SESSION_ID`; canonical agent sessions are detected via
  `CLAUDE_SESSION_ID`/`CLAUDE_CODE_SESSION_ID`. The human can jump from
  the ask straight into the asking session.
- **Daemon routes** `/cli/feedback/*` (create/list/show/comment/answer/
  resolve) + WireEvents `feedback:created`, `feedback:answered`,
  `feedback:commented`, `feedback:status-changed` on `/events`.

## The Feedback page (desktop)

- Top-bar **Feedback** button with a live waiting-count badge → a
  master-detail board (layout ported from the AFSROW reference): card
  list left (workspace icon + name top-left, kind tag in the footer,
  priority + inline status dropdown top-right), response panel right
  with **Thread | Agent** tabs.
- **Thread** is comment-only — no separate "answer" mode. Every human
  comment is injected into the asking agent's session as
  `[feedback:<short-id>] <text>` via the `k2 msg` delivery path
  (wake-if-dormant, best-effort, store-then-inject so a delivery failure
  never loses the message). The first human comment on a waiting
  question/approval is recorded as the answer (unblocks `--wait`); FYIs
  never auto-answer; agent comments store without echoing back.
- **Live thread updates:** `feedback:commented` triggers a coalesced
  refetch of the open thread — agent replies appear in-place in ~300ms,
  drafts survive refreshes.
- Filters: status chips with per-status counts, tokenized
  order-independent search, and a custom workspace picker with
  in-popover search, focus-group grouping, icons + colored borders
  (mirrors Settings → Workspaces).
- **Agent tab** embeds the asking session's terminal in place (kessel
  attach machinery; canonical wake via ensure-pinned-chat, sandbox wake
  via reopen). Inline status dropdown supports reopening
  (waiting ← resolved/dismissed); manually setting "answered" is
  rejected (400) to protect the `--wait` contract.
- **Desktop notifications** (tauri-plugin-notification, new dependency)
  for NEW asks only, and only when the window is unfocused or the page
  hidden.
- Persona templates (core writer + `k2 agent hire` archetypes) now teach
  agents the channel exists.

## Unseen-done indicator + completion chime

- Panes that finish working while unwatched (tab not active or window
  unfocused, 4s debounce, spawn grace) get an amber dot in the Active
  bar until viewed; precedence permission-red > spinner > amber > live.
- Optional soft completion chime (Web-Audio synthesized, 3s multi-agent
  throttle) — Settings → General toggle, fires only for unseen
  completions.

## Fixes

- `k2 feedback ask --options`: commas inside option labels no longer
  shatter the option list (`\,` escape; covered by an end-to-end CLI
  test against a stub daemon).
- Flaky `feedback` prefix-resolution unit test made deterministic
  (unique prefix computed against all rows).
- Esc closes an open filter/status popover before deselecting the item /
  closing the page.

## Also riding in this release

- Presence/multiplayer arc docs: research synthesis, build plan, and the
  approved V1 PRD (docs only; no behavior).
