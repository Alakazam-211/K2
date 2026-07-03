# Grok CLI session storage — empirical study (2026-07-03)

Observed live: grok 0.2.82 (6d0b07d2de0f) [stable], running session in
/Users/z3thon/DevProjects/Cortana. Feeds the detect_grok_session /
parse_grok_sessions adapter (de-generalization slice 6).

## Storage layout — everything under ~/.grok/
- `active_sessions.json` — LIVE registry [{session_id, pid, cwd, opened_at}]
  (validate pid; stale entries possible; active_sessions.lock exists)
- `sessions/<percent-encoded-abs-cwd>/<uuidv7>/` — one dir per cwd
  (URL %-encoding, reversible), one dir per session (name = UUIDv7 = id)
  - `summary.json` ★ metadata SSOT: info.{id,cwd}, generated_title,
    session_summary, created_at/updated_at/last_active_at (RFC3339 UTC),
    num_messages/num_chat_messages, current_model_id, git_root_dir,
    head_branch, agent_name, sandbox_profile, session_kind ("subagent"
    ONLY on subagent sessions — absent on user sessions)
  - `chat_history.jsonl` — transcript; type ∈ system|user|assistant|reasoning;
    synthetic injected-context user lines carry "synthetic_reason" key; real
    prompts wrapped in <user_query>…</user_query>; NO per-message timestamps
  - `signals.json` — userMessageCount/assistantMessageCount/turnCount
  - `events.jsonl`, `updates.jsonl` (ACP-style _x.ai/session/update frames),
    rewind_points.jsonl, subagents/<child-uuid>/meta.json
  - `prompt_history.jsonl` (per-cwd, sibling of session dirs):
    {timestamp, session_id, prompt, is_bash}
- `sessions/session_search.sqlite` — FTS5 index, session_docs(session_id,
  cwd, updated_at UNIX-SECONDS, title, content). LAGS live state — secondary.
- `projects/<dash-slug>/mcps/` — MCP cache only, NOT sessions.
- leader process arch (~/.grok/leader.sock); file parsing avoids it.

## Session id: UUIDv7 (time-ordered; dir-name sort == chronological)

## CLI grammar (verified --help)
- `grok -r/--resume [<SESSION_ID>]` (id optional = most recent)
- `grok -c/--continue` (most recent FOR CURRENT CWD)
- `-s/--session-id <uuid>` = NEW sessions only (with --resume needs --fork-session)
- `--fork-session`, `--restore-code`, `grok sessions list|search|delete`
  (cwd-scoped, excludes subagents), `grok export <id>` (markdown)
- K2's RESUMABLE_CLI_TOOLS grok entry (`--resume`, flag-style) ✅ CORRECT;
  ChatHistory PROVIDER_CONFIG ✅ CORRECT; SESSION_FLAGS_TO_STRIP covers -c/-r.

## ChatSession mapping
- session_id = dir name / summary.json .info.id
- project    = summary.json .info.cwd (header-based like Pi — read the field,
               don't decode dir names; %-encoding of spaces/unicode untested)
- title      = generated_title → session_summary → first prompt_history line
               for that id → "Untitled" (title is server-generated after turn
               1 — very fresh sessions briefly lack it)
- timestamp  = updated_at/last_active_at (RFC3339→ms), fallback mtime
- message_count = signals.json user+assistant counts, or count
               chat_history.jsonl user/assistant lines WITHOUT
               synthetic_reason (num_chat_messages is too high — includes
               reasoning/tool_result)
- provider   = "grok"

## detect_grok_session(project_path) — Pi pattern
Walk ~/.grok/sessions/*/<uuid>/summary.json; SKIP session_kind=="subagent"
(else 1 user session + N subagents = N+1 rows — grok's own list filters
them); skip .lock files + non-UUID entries; filter .info.cwd via
matches_project_family(); newest by last_active_at. Fast path:
active_sessions.json for a live match (validate pid alive).

## Gotchas
1. Writes are LIVE (lsof-verified open handles; summary.json mtime advances
   mid-session) — discoverable within seconds, no exit-flush wait.
2. Subagent filtering is MANDATORY (see above).
3. sqlite index updated_at is unix SECONDS and lags — never the SSOT.
4. Sessions also upload to xAI cloud (upload_queue/) but history is fully
   local — no cloud dependency.
5. agent_name varies ("cursor", "grok-build-plan") — grok agent profiles;
   irrelevant to detection.
