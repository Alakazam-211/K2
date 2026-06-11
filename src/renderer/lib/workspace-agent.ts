// Plan B (Bulk-2) — host-aware client for the workspace primary-agent
// display-name + resume-chat-args routes. These were the 3 daemon-proxy
// commands in `commands/k2so_agents.rs` (each `cli_get`-ed the daemon then
// fell back to an in-process k2so-core call if the daemon was unreachable).
//
// Moving them onto `daemonCli*` makes them work against ANY daemon (local
// OR a remote K2 Connect host) instead of the localhost-pinned Tauri proxy.
// The in-process fallback is intentionally dropped — the daemon is the
// single source of truth (daemon-first); a renderer talking to a remote
// daemon has no local k2so-core to fall back to anyway.
//
// Routes (confirmed against `crates/k2so-daemon/src/cli.rs`):
//   GET /cli/workspace/agent-display-name?project=<path>      → {display_name}
//   GET /cli/workspace/set-agent-display-name?project=&name=  → mutation (echo)
//   GET /cli/workspace/resume-chat-args?project=<path>        → ResumeChatArgs
//
// NB: all three are GET routes in the daemon (set-agent-display-name is a
// GET that mutates, matching the old `cli_get` proxy). The display-name body
// field is snake_case `display_name`; resume-chat-args is camelCase.

import { daemonCliGet } from '@/lib/daemon-cli'

/** Resolve the workspace's primary-agent display name. Total — the daemon
 *  always returns a string (display_name → name → project name fallback).
 *  Returns '' if the daemon read fails so callers degrade gracefully. */
export async function agentDisplayName(projectPath: string): Promise<string> {
  const r = await daemonCliGet<{ display_name?: string }>(
    'workspace/agent-display-name',
    { project: projectPath },
  )
  return r?.display_name ?? ''
}

/** Set the workspace's primary-agent display name. The daemon rewrites
 *  AGENT.md frontmatter, invalidates its cache, emits SyncProjects, and
 *  pushes the new label to any live canonical session. (GET-with-mutation,
 *  mirroring the old `cli_get` proxy — body is ignored.) */
export async function setAgentDisplayName(
  projectPath: string,
  name: string,
): Promise<void> {
  await daemonCliGet('workspace/set-agent-display-name', {
    project: projectPath,
    name,
  })
}

/** #657 — persist the pinned chat tab's canonical Claude session id to
 *  the daemon DB (`workspace_sessions.session_id`). Called on the
 *  dismiss-reap path BEFORE the chat PTY is closed so `claude --resume`
 *  has a target when the workspace is re-opened. (GET-with-mutation,
 *  mirroring `setAgentDisplayName` — the daemon route reads query
 *  params.) Resolves to the daemon's update result. */
export async function setChatSession(
  projectPath: string,
  sessionId: string,
): Promise<void> {
  await daemonCliGet('workspace/set-chat-session', {
    project: projectPath,
    session_id: sessionId,
  })
}

export interface ResumeChatArgs {
  command: string
  args: string[]
  cwd: string
  resumeSession?: string
  /** `true` when the daemon resolved an EXISTING `workspace_sessions.session_id`
   *  whose JSONL is on disk (resumable). `false` when it pre-allocated a fresh
   *  UUID because no usable saved session existed. Cold-boot revive uses this
   *  to decide whether the daemon's canonical session should override the
   *  renderer's layout hint (GH#679). */
  resumedExisting?: boolean
}

/** Resolve the `claude --resume <session>` (or fresh `claude`) launch args
 *  for the workspace's pinned chat tab. camelCase response. */
export async function resumeChatArgs(projectPath: string): Promise<ResumeChatArgs> {
  return daemonCliGet<ResumeChatArgs>('workspace/resume-chat-args', {
    project: projectPath,
  })
}

/** The cold-boot Step-0 launch decision. The pinned chat must only
 *  `--resume` a session that ACTUALLY EXISTS on disk; otherwise claude
 *  errors with "No conversation found with session ID: <uuid>"
 *  (GH#681). The daemon already knows the answer via `resumedExisting`,
 *  so this decision encodes WHICH command shape to spawn:
 *
 *   - `resume`   → `claude --dangerously-skip-permissions --resume <sessionId>`
 *                  (a real prior conversation — preserves GH#679 revive).
 *   - `fresh`    → spawn the daemon's returned `args` verbatim
 *                  (`--session-id <new>` — a clean NEW session pinned to
 *                  the persisted UUID; NEVER `--resume`).
 *   - `fallback` → daemon read failed; we couldn't verify the session.
 *                  Resume the layout hint to preserve offline / DB-race
 *                  resilience (the canonical-lane-restore design).
 */
export type ColdBootDecision =
  | { kind: 'resume'; sessionId: string }
  | { kind: 'fresh'; args: string[]; sessionId: string }
  | { kind: 'fallback'; sessionId: string }

/** GH#679 + GH#681 — cold-boot revive reconciliation. Given the
 *  layout-restored sessionId hint and the daemon's canonical
 *  `resume-chat-args` response (which reads `workspace_sessions.session_id`
 *  from SQLite + checks the JSONL on disk), decide HOW the pinned chat
 *  should launch.
 *
 *  `resumedExisting` is authoritative for whether we resume at all:
 *
 *   - `true`  → the daemon found a resumable conversation. Resume it
 *               (SQLite wins over the layout hint when they differ — the
 *               GH#679 dropdown-switch / crash-window revive).
 *   - `false` → the daemon pre-allocated a FRESH UUID (no usable saved
 *               session — e.g. a brand-new, never-chatted workspace).
 *               We must NOT `--resume` it; spawn the daemon's
 *               `--session-id <new>` args verbatim so claude starts a
 *               clean session pinned to the persisted id (GH#681).
 *   - `null`  → daemon unreachable. Fall back to resuming the layout
 *               hint, preserving offline / DB-race resilience.
 *
 *  Pure + side-effect-free so the decision is unit-testable without
 *  rendering AgentChatPane. */
export function reconcileColdBootSession(
  layoutHint: string,
  canonical: ResumeChatArgs | null,
): ColdBootDecision {
  if (!canonical) {
    return { kind: 'fallback', sessionId: layoutHint }
  }
  if (canonical.resumedExisting) {
    // A real prior conversation. SQLite wins when it differs from the
    // layout hint; otherwise the hint and the canonical agree.
    const sessionId = canonical.resumeSession || layoutHint
    return { kind: 'resume', sessionId }
  }
  // resumedExisting === false → fresh pre-allocated session. Use the
  // daemon's args directly (they are `--session-id <new>`). NEVER resume.
  return {
    kind: 'fresh',
    args: canonical.args,
    sessionId: canonical.resumeSession || layoutHint,
  }
}
