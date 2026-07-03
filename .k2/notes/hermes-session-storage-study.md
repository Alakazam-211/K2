# Hermes Agent session storage — empirical study (2026-07-03)

Observed live: Hermes Agent v0.18.0 (Nous Research), Python app at
~/.hermes/hermes-agent (venv), launcher ~/.local/bin/hermes. Session run in
/Users/z3thon/DevProjects/Cortana with a deliberate subagent delegation.
Feeds detect_hermes_session / parse_hermes_sessions (slice 6) + the
ProviderResume hermes row (slice 3). Source ships locally — findings
verified against implementation.

## Storage: ONE SQLite DB (first adapter needing SQL, not file-walk)
~/.hermes/state.db (WAL mode). HERMES_HOME env overrides; profiles
(`hermes profile`) relocate the whole home to ~/.hermes/profiles/<name>/.
~/.hermes/sessions/*.json = DEAD legacy path (nothing written now).

sessions(id PK, source 'cli'|'subagent'|'tui'|'telegram'|…,
  parent_session_id, started_at REAL unix, ended_at (NULL=live/crashed),
  end_reason, message_count (incl tool rows), title (LLM-generated after
  turn 1, UNIQUE partial index, NULL when fresh), cwd LITERAL PATH,
  git_branch, git_repo_root, archived)
messages(session_id FK, role user|assistant|tool, content, tool_calls,
  timestamp REAL) + FTS5.

## Session id: YYYYMMDD_HHMMSS_<6-hex> (local time + uuid4 prefix;
lexicographic == chronological). Gateway variant 8-hex; cron ids
cron_<jobid>_<ts> — non-'cli' sources, so the source filter handles them.

## cwd association: literal `cwd` column. Hermes' own prefix matching
(cwd = ? OR cwd LIKE prefix||'/%') ≈ matches_project_family.

## SUBAGENTS: sibling row, source='subagent', parent_session_id set.
FILTER BY SOURCE, NOT parent_session_id — compression forks and /branch
children also carry parent_session_id but are REAL user sessions
(source stays 'cli'). Adapter predicate:
  WHERE source='cli' AND archived=0  (optionally IN ('cli','tui'))

## detect_hermes_session(project_path)
state.db absent → None; else
SELECT id FROM sessions WHERE source='cli' AND archived=0 AND
 (cwd=root OR cwd LIKE root||'/%') ORDER BY started_at DESC LIMIT 1.
Open SQLite READ-ONLY but WAL-CAPABLE: file:...?mode=ro — NOT immutable=1
(misses WAL = newest rows). Live-write verified (row exists within
seconds of launch; WAL advances mid-session).

## ChatSession mapping
session_id=sessions.id; project=sessions.cwd; title=title → first user
message → "Untitled"; timestamp=MAX(messages.timestamp)|ended_at|
started_at ×1000→ms; provider="hermes"; message_count=COUNT(messages
role IN (user,assistant)).

## CLI grammar (verified --help + source)
- `hermes --resume/-r <SESSION>` — by ID or TITLE; unknown → error (no
  silent new session).
- `hermes --continue/-c [NAME]` — most-recent GLOBAL (NOT cwd-scoped,
  unlike grok) — do not use; always resume by explicit id.
- NO PRE-MINT: --pass-session-id only injects into the system prompt;
  ids generated internally → post-hoc discovery (pi/codex family).
- `hermes sessions list|export|delete|prune|rename|browse` (subagents
  excluded from list).
- Resume-redirect: resolve_resume_session_id walks compression chains to
  the descendant tip — resuming an old id can land on a DIFFERENT id;
  treat stored ids as potentially superseded (end_reason='compression'
  chains; tip via Hermes' get_compression_tip semantics).

## RESUMABLE_CLI_TOOLS entry (src/shared/constants.ts)
'hermes': { resumeFlag: '--resume', provider: 'hermes' }
+ add 'hermes' to KNOWN_AGENT_COMMANDS. Name collision: `hermes` is also
Meta's JS engine binary — foreground detection should tolerate that.

## Gotchas
1. SQL not files: mtime-based discovery inapplicable; rusqlite query
   (k2-core already depends on SQLite — no new dep class).
2. Titles NULL on very fresh sessions (LLM-generated post turn 1).
3. Empty sessions pruned on rotation.
4. One logical conversation can span several rows via compression forks —
   list the tip only if polish wanted.
5. V1 scope: default HERMES_HOME only (skip profiles/).
