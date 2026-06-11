/**
 * Terminal-ID format helpers for agent chat + heartbeat sessions.
 *
 * Mirror of `crates/k2so-core/src/agents/terminal_id.rs` — both sides
 * MUST agree on the format. Tests pin both implementations to the same
 * round-trip strings.
 *
 * Format reference:
 *
 * - `agent-chat:<projectId>:<agent>`                — chat tab session
 * - `agent-chat:wt:<workspaceId>`                   — worktree-scoped chat
 * - `agent-chat:<projectId>:<agent>:hb:<heartbeat>` — per-heartbeat session
 *
 * `:` is the separator so hyphenated agent / heartbeat names parse cleanly.
 *
 * Legacy formats (pre-0.36.0):
 *
 * - `agent-chat-<agent>`         — collided across projects sharing an agent
 * - `agent-chat-wt-<workspace>`  — already namespaced; renamed for consistency
 *
 * Both legacy forms are recognised by `parseTerminalId` for one release
 * window so unmigrated rows / stale CLI references resolve correctly.
 */

const PREFIX = 'agent-chat:'
const LEGACY_PREFIX = 'agent-chat-'
const WORKTREE_TAG = 'wt'
const HEARTBEAT_TAG = 'hb'

export type TerminalIdKind =
  | { kind: 'agent_chat'; projectId: string; agent: string }
  | { kind: 'heartbeat_chat'; projectId: string; agent: string; heartbeat: string }
  | { kind: 'worktree'; workspaceId: string }
  | { kind: 'legacy_agent_chat'; agent: string }
  | { kind: 'legacy_worktree'; workspaceId: string }

/**
 * Build the terminal id for an agent's Chat tab.
 *
 * **0.37.5:** the agent name is no longer part of the canonical
 * shape — post-unification (one agent per workspace), the
 * workspace's project_id alone is unique. The `agent` parameter
 * is kept for back-compat at the call site (every caller already
 * has it on hand) but it's IGNORED in the produced id. Pre-0.37.5
 * the id was `agent-chat:<projectId>:<agent>`; that suffix caused
 * the renderer to write the wrong shape to
 * `workspace_sessions.terminal_id` via `k2so_agents_lock`,
 * bypassing the SQL migration 0042 (C3PO 5c80bef1). The bare-pid
 * form keeps Tauri's terminal manager and the SQL canonical row
 * in agreement.
 */
export function agentChatId(projectId: string, agent: string): string {
  void agent
  return `${PREFIX}${projectId}`
}

/** Build the terminal id for a worktree-scoped chat session. */
export function worktreeChatId(workspaceId: string): string {
  return `${PREFIX}${WORKTREE_TAG}:${workspaceId}`
}

/** Build the terminal id for a per-heartbeat session. */
export function heartbeatChatId(projectId: string, agent: string, heartbeat: string): string {
  return `${PREFIX}${projectId}:${agent}:${HEARTBEAT_TAG}:${heartbeat}`
}

/** Whether this id is in the legacy pre-0.36.0 form. */
export function isLegacyTerminalId(id: string): boolean {
  return id.startsWith(LEGACY_PREFIX) && !id.startsWith(PREFIX)
}

/**
 * Parse a terminal id into its kind. Recognises both new (`:`-delimited)
 * and legacy (`-`-delimited) forms.
 *
 * Returns `null` for ids that don't match any known agent-chat shape.
 */
export function parseTerminalId(id: string): TerminalIdKind | null {
  if (id.startsWith(PREFIX)) {
    return parseNamespaced(id.slice(PREFIX.length))
  }
  if (id.startsWith(LEGACY_PREFIX)) {
    return parseLegacy(id.slice(LEGACY_PREFIX.length))
  }
  return null
}

function parseNamespaced(rest: string): TerminalIdKind | null {
  const colon = rest.indexOf(':')
  if (colon < 0) {
    // 0.37.5 bare-pid form: `agent-chat:<projectId>` (no agent suffix).
    // Post-unification the workspace's project_id alone is canonical.
    if (!rest) return null
    return { kind: 'agent_chat', projectId: rest, agent: '' }
  }
  const head = rest.slice(0, colon)
  const tail = rest.slice(colon + 1)

  if (head === WORKTREE_TAG) {
    if (!tail) return null
    return { kind: 'worktree', workspaceId: tail }
  }

  if (!head) return null

  // Heartbeat form: split tail on `:hb:` so hyphens in agent/heartbeat
  // names are preserved.
  const hbMarker = `:${HEARTBEAT_TAG}:`
  const hbIdx = tail.indexOf(hbMarker)
  if (hbIdx >= 0) {
    const agent = tail.slice(0, hbIdx)
    const heartbeat = tail.slice(hbIdx + hbMarker.length)
    if (!agent || !heartbeat) return null
    return { kind: 'heartbeat_chat', projectId: head, agent, heartbeat }
  }

  if (!tail) return null
  // Pre-0.37.5 form: `agent-chat:<projectId>:<agent>`. Still
  // recognized for back-compat with stale tab state and rows that
  // weren't migrated. Post-0.37.5 callers produce the bare-pid
  // form via `agentChatId` above.
  return { kind: 'agent_chat', projectId: head, agent: tail }
}

function parseLegacy(rest: string): TerminalIdKind | null {
  if (rest.startsWith('wt-')) {
    const workspaceId = rest.slice('wt-'.length)
    if (!workspaceId) return null
    return { kind: 'legacy_worktree', workspaceId }
  }
  if (!rest) return null
  return { kind: 'legacy_agent_chat', agent: rest }
}

/**
 * Returns true if this terminal id belongs to ANY agent-chat surface
 * (new or legacy form). Useful for filters that today use the
 * regex `/^agent-chat-(?:wt-)?/`.
 */
export function isAgentChatId(id: string): boolean {
  return parseTerminalId(id) !== null
}

/**
 * Extract the agent name from any agent-chat terminal id (new or legacy).
 * Returns null for non-agent-chat ids and for worktree-scoped ids
 * (which carry workspace_id, not agent).
 */
export function agentNameFromId(id: string): string | null {
  const parsed = parseTerminalId(id)
  if (!parsed) return null
  switch (parsed.kind) {
    case 'agent_chat':
    case 'heartbeat_chat':
      return parsed.agent
    case 'legacy_agent_chat':
      return parsed.agent
    case 'worktree':
    case 'legacy_worktree':
      return null
  }
}
