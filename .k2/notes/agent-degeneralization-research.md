# Agent De-generalization — Research Synthesis (2026-07-03)

Four read-only research agents mapped everything required to make the default
agent pluggable (global + per-workspace) and pinned-tab canonical sessions
multi-agent, across the Big 7: Claude, Codex, Gemini, Grok, Cursor Agent, Pi,
Hermes. This file is the SSOT for planning; nothing has been built.

Scope decisions (Rosson, 2026-07-03): CLI-tool layer already agent-agnostic;
`talk` HITL + `msg` resume need per-agent work; sandbox/canonical-message API
OUT OF SCOPE this arc; per-workspace default is non-retroactive; canonical
session is one user-picked session whose agent may differ from the workspace
default.

## 1. What already exists (more than expected)

- **Global `defaultAgent` setting already ships**: typed field in
  `AppSettings` (`crates/k2-core/src/app_settings.rs:72`), persisted to
  `~/.k2/settings.json`, with a dropdown already in Editors & Agents
  (`EditorsAgentsSection.tsx:54-76`, `DefaultAgentPickerInline`).
- **Agent roster is one daemon table** — `agent_presets` (seeded in
  `db/mod.rs:555`, reset copy `db_ops.rs:33`), already contains all Big 7 + 6
  more, fixed UUIDs. Settings list and the PresetsBar launcher both read it
  via `/cli/presets/list` → `stores/presets.ts`. No roster duplication in the
  live path.
- **Discovery adapters for FIVE providers already exist** in
  `crates/k2-core/src/chat_history.rs`: detect+parse for claude, cursor
  (CLI + IDE), gemini, pi, codex; aggregator `list_all_sessions`; dispatcher
  `detect_active_session(provider, …)`. On-disk layouts documented in code.
  The standalone ChatHistory browser UI is already multi-provider with
  per-agent resume flags (`PROVIDER_CONFIG`, `ChatHistory.tsx:50-57`).
- **`workspace_sessions.harness` column exists** (0039 migration, default
  'claude') — but is WRITE-ONLY. No code reads it to pick a command.
- **Per-agent resume knowledge exists in TS**:
  `shared/constants.ts::RESUMABLE_CLI_TOOLS` covers claude / cursor-agent /
  grok / gemini / pi / codex (Hermes missing).
- **Kessel + 0.40.22 interaction layer is agent-agnostic** (confirmed): SGR
  mouse, OSC 52, PtyWrite query replies, fullscreen detection all gate on
  terminal mode bits, never agent identity.
- **Per-workspace launch override exists**: AGENT.md `launch:` block
  (`workspace/launch_profile.rs`) — the closest existing per-workspace
  command mechanism; honored by canonical_session + providers.

## 2. Dragons (must-know before building)

1. **`defaultAgent` value-representation split (silently broken today)**:
   the picker writes the command's first token (`"claude"`); Cmd+Shift+T /
   `launchDefaultAgent` / AssistantBar match by command token (works), but
   ~10 consumers (AIFileEditor's 6 callers, `cli:ai-commit`, Sidebar,
   ChangesPanel, WorktreeBar…) match `p.id === defaultAgent` — never true, so
   they silently fall back to first-enabled preset (= Claude). The setting
   only appears to work because Claude is both default and fallback. UNIFY
   FIRST (recommend preset-id as stored value) before the setting is
   load-bearing.
2. **Pre-minted UUID assumption**: K2 mints a v4 UUID and passes
   `claude --session-id <uuid>` BEFORE spawn (`resume_chat.rs:214`,
   `v2_spawn.rs:483` — the latter gated on `command=="claude"`). Pi/codex/
   gemini generate their own ids, discoverable only post-hoc (header lines /
   newest-on-disk). The `detect_*_session_near` machinery exists for post-hoc
   adoption (heartbeat wakes use it) — the mint case must become
   "spawn fresh → adopt id post-hoc" for those agents.
3. **LIVE BUG (pre-existing, ships today)**: `k2 msg`/`talk` wake of a
   dormant workspace hardcodes `command:Some("claude")` + Claude flag grammar
   (`workspace_msg.rs:1057-1092,1229`), bypassing resume_chat entirely.
   Waking a pi/codex workspace spawns the WRONG binary. Inline argv, no seam.
4. **Dead roster file**: `src/shared/agent-catalog.ts` — imported nowhere,
   missing Hermes, colliding sortOrders. Delete or make authoritative.
5. **No agent availability detection** — editors/terminals get `which`
   checks (`editors.rs`), agents don't. Needed if pickers should show only
   installed agents.
6. **Auth is Claude-only** (claude-auth store, claude_auth_host, per-project
   anthropic_api_key). Out of scope but "Grok default" carries no Grok auth.
7. **Two settings lanes side-by-side**: typed `defaultAgent` field vs
   `projectSettings['__global__'].defaultEditor` free-form lane. Pick
   deliberately (typed field recommended; it's what exists for agents).

## 3. Seam map (5 chokepoints cover ~everything)

Rust:
1. `workspace/launch_profile.rs` → new `resolve_agent_command(workspace)`
   layering per-workspace default → global default_agent → "claude".
   Route both `default_launch_profile()` copies (canonical_session.rs:261,
   providers.rs:254), wake_headless, heartbeat_launch, agents_routes,
   v2_spawn recovery through it.
2. `workspace/resume_chat.rs::resolve_resume_chat_args_ex` — THE canonical
   resolver (pinned ensure, restart-recovery, CLI, companion, heartbeat all
   funnel here). Fuses identity + command + flag grammar + on-disk existence.
   Needs a Rust `ProviderResume` adapter: {command, resume_flag |
   resume_subcommand, session_file_exists_for, newest_on_disk_for},
   mirroring TS PROVIDER_CONFIG + core detect_*/parse_* triplet. Make
   `harness` load-bearing (read on resume, written from actual agent,
   carried through set-chat-session).
3. `agent_launch.rs::k2so_agents_build_launch` — already takes
   Option<String>; callers resolve via seam 1.

Renderer:
4. Extract duplicated `defaultAgent→preset→parseCommand` useMemo (6
   AIFileEditor callers + Cmd+Shift+T + launchDefaultAgent + cli:ai-commit)
   into one `useResolvedAgentCommand(workspace)` hook; generalize the
   `if command==='claude'` --append-system-prompt branches.
5. `shared/constants.ts::RESUMABLE_CLI_TOOLS` — dedupe ChatHistory's
   PROVIDER_CONFIG + AIFileEditor's inline resume into it; add Hermes.

No-clean-seam sites needing bespoke wiring: `workspace_msg.rs` (dragon 3);
`classify_routes.rs` HITL fast-path (Claude UI strings; needs per-agent
marker tables — see §5); inline `'claude'` literals in `tabs.ts:2006/2095`
(cold-boot pinned), `AgentChatPane.tsx:883`, `AgentPane.tsx:453`,
`ReviewPanel.tsx:322/328/682`, `AssistantBar.tsx:359`.

## 4. Schema / storage changes

- NEW `projects.default_agent` column (migration `00XX_project_default_agent
  .sql`) — precedent trail: agent_mode / 0054 / 0056 / 0060. Thread:
  `schema.rs` Project struct + SELECT/UPDATE → `db_routes.rs`
  ProjectsUpdateBody → TS `stores/projects.ts` Project → SettingDropdown in
  ProjectsSection right panel (copy the StateSelector pattern,
  `ProjectsSection.tsx:2058-2095`). Stamp from global default at first open;
  NULL = fall through to global (naturally non-retroactive).
- Make `workspace_sessions.harness` read/written truthfully; carry provider
  through `set-chat-session` (`workspace_routes.rs:97`,
  `update_session_id` schema.rs:1340).
- `workspace_tab_sessions` already stores {command, args_json} — ad-hoc tabs
  nearly work already.

## 5. Per-agent signal profile (from the activity/HITL audit)

Proposed record (fields with safe unknown-agent defaults — degrade to
output-cadence + stale-sweep, HITL Unavailable, no hook affordances):

    AgentSignalProfile {
      provider,
      title_working_re (claude: braille ^[⠀-⣿]),
      title_idle_re (claude: ✱-family),
      bell_means_idle (claude/codex true; pi false),
      working_phrases ("esc to interrupt" claude/codex; "esc to cancel"
        gemini; "working…/thinking…" pi),
      progress_osc (pi: OSC 9;4 — currently UNCONSUMED, the cleanest busy
        signal available),
      has_lifecycle_hooks (claude/cursor/gemini true; codex/grok/pi/hermes
        false — permission/review states dark for them),
      select_markers / permission_markers / option_cursor (per-agent HITL
        fast-path tables for classify_routes.rs),
      spawn_binary, resume_flag/resume_subcommand,
      ready_signal (bracketed-paste vs first-frame-delay — pi enables ?2004h
        at startup so the poll can't be trusted for it),
      inject_settle_ms (Claude-tuned 150/250/120 today)
    }

Degradation today (if default were pi): spinner flaky-on/laggy-off, no
permission state ever, talk auto-HITL no-ops, wake spawns wrong binary, copy
drags styled padding (no OSC 52 from pi). Codex: mostly-working scan, laggier
idle, same hook/HITL/wake gaps.

## 6. Known unknowns (need empirical/external research when building)

- Grok: renderer PROVIDER_CONFIG entry exists but NO core detector/parser —
  on-disk session location unknown to the codebase.
- Hermes: nothing anywhere (no discovery, no RESUMABLE_CLI_TOOLS entry, no
  install-guide entry).
- Codex/grok/cursor/hermes: bracketed-paste timing, bell semantics, title
  behavior, HITL footer wording — unstudied. Repeat the claude/pi empirical
  TUI-study methodology (scratch daemon + mode-bit observation + injected
  sequences) per agent.
- Cursor-Agent resume-id semantics differ between its two parsers
  (composerId vs chat-dir id) — reconcile.

## 7. Proposed slice order (draft, for discussion)

0. **Foundations**: unify defaultAgent representation (preset-id), extract
   `useResolvedAgentCommand`, delete dead agent-catalog.ts. Small, unblocks
   everything, fixes the silent-fallback bug.
1. **Per-workspace default**: projects.default_agent column + dropdown +
   stamp-at-first-open + Cmd+Shift+T/launchDefaultAgent/AIFileEditor callers
   read it (via the hook).
2. **Rust resolve seam**: `resolve_agent_command` + route the daemon spawn
   sites (wake_headless, heartbeat, agents_routes, v2_spawn recovery,
   default_launch_profile ×2).
3. **ProviderResume adapter + harness load-bearing**: generalize
   resume_chat/pinned/canonical + bespoke workspace_msg wiring (fixes the
   wake-wrong-binary live bug); post-hoc id adoption for non-mintable agents.
4. **Multi-agent canonical dropdown**: drop the provider==='claude' filter
   (AgentChatPane.tsx:194), provider icons per row, set-chat-session carries
   provider, rename claudeSessionId.
5. **Signal profiles**: activity markers + HITL tables per agent (+ consume
   pi's OSC 9;4).
6. **Discovery gaps**: grok + hermes adapters (after external research);
   Hermes in RESUMABLE_CLI_TOOLS + install guide.
Parallel: empirical TUI studies for the 5 unstudied agents; clone-to
per-agent re-root decision (or document Claude-only limitation).
