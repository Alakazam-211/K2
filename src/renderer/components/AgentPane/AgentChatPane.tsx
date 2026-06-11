import { useState, useEffect, useRef, useCallback, useMemo } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { terminalExists } from '@/lib/terminal-daemon'
import { useProjectsStore } from '@/stores/projects'
import { useTabsStore } from '@/stores/tabs'
import { TerminalPane } from '@/terminal-v2/TerminalPane'
import { agentChatId } from '@/lib/terminal-id'
import { getDaemonWs, daemonHttpBase } from '@/kessel/daemon-ws'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { agentDisplayName, resumeChatArgs, reconcileColdBootSession, type ColdBootDecision } from '@/lib/workspace-agent'
import { useActiveAgentsStore } from '@/stores/active-agents'
import { useServerSupports } from '@/lib/server-capabilities'
import { subscribeToWorkspaceSessionEvents } from '@/stores/session-events'
import {
  initialBreakerState,
  recordSpawn,
  recordExit,
  resetBreaker,
  isSelfRetrigger,
  MAX_RAPID_EXITS,
  type BreakerState,
  type ResolveMemo,
} from '@/lib/chat-spawn-breaker'

interface AgentChatPaneProps {
  agentName: string
  projectPath: string
  /** 0.37.12 — Claude session id restored from the serialized layout.
   *  When present, AgentChatPane skips the
   *  `k2so_agents_resume_chat_args` daemon roundtrip and builds the
   *  launch config directly so the same session resumes immediately.
   *  Renderer holds the canonical record; the daemon's auto-stamp
   *  hook reconciles DB state on the next spawn. See
   *  `.k2so/prds/canonical-lane-restore.md`.
   *
   *  0.39.39 (#683) — in the DAEMON-OWNED path this is downgraded to a
   *  NON-AUTHORITATIVE offline hint (PRD §4.5 / D3): it's forwarded to
   *  `ensure-pinned-chat` so an unreachable daemon could still resolve
   *  it, but the daemon reads `workspace_sessions` canonically and wins
   *  whenever it's reachable. It only drives the renderer's resolve loop
   *  in the capability-gated FALLBACK path. */
  restoredSessionId?: string
}

/**
 * Chat pinned tab — runs the workspace agent's persistent Claude session.
 *
 * Replaces the "Chat" sub-tab from the pre-0.36.0 single AgentPane.
 * Sibling tab is `AgentInboxPane`; both are pinned by `tabs.ts`.
 *
 * Terminal id is project-namespaced (`agent-chat:<project_id>:<agent>`)
 * so two workspaces sharing an agent name don't collide on a single
 * PTY — see `.k2so/prds/heartbeats-sidebar-audit.md` Phase 1.
 *
 * ── 0.39.39 (#683) daemon-owned pinned-chat lifecycle ──────────────
 * `.k2so/prds/daemon-owned-pinned-chat.md`. The DAEMON now owns the
 * pinned-chat session lifecycle (find-or-spawn, resume/fresh decision,
 * refresh, dropdown-switch respawn). The renderer asks the daemon to
 * `ensure-pinned-chat`, attaches the grid-WS, and renders. This kills
 * the renderer-side resolve loop — and with it the self-retrigger guard
 * + spawn-loop circuit breaker, which only existed to bound that loop.
 *
 * Capability-gated on `daemon-pinned-chat` (FEATURES, 0.39.39). When the
 * active host supports it (always true for `local`) → the new
 * daemon-owned path runs (NO breaker). When it does NOT (an older /
 * remote daemon without the `ensure-pinned-chat` route) → we fall back
 * to the pre-0.39.39 renderer-orchestrated path WITH the self-retrigger
 * guard + circuit breaker intact (the band-aids still protect the only
 * code path that can still loop).
 */
export function AgentChatPane({ agentName, projectPath, restoredSessionId }: AgentChatPaneProps): React.JSX.Element {
  // Resolve project id synchronously from the projects store; the chat tab
  // will not render until a real id is available so the legacy collision
  // bug can never reappear via this surface.
  const projectId = useProjectsStore((s) => {
    return s.projects.find((p) => p.path === projectPath)?.id ?? null
  })

  // Capability gate (#683 / PRD §6). Subscribes so a host switch
  // (local↔remote) flips the path live. `local` always supports it.
  const daemonOwnsChat = useServerSupports('daemon-pinned-chat')

  if (!projectId) {
    return (
      <div className="flex items-center justify-center h-full text-xs text-[var(--color-text-muted)]">
        Loading workspace…
      </div>
    )
  }

  // `key={projectId}` forces a clean remount when the workspace
  // switches. Without it, React reuses the same AgentChatTerminal
  // instance and `terminalIdRef` (initialized from
  // `useRef(agentChatId(projectId, agentName))`) keeps the stale
  // workspace's terminal id — defense-in-depth against the
  // cross-workspace pinned-chat collision fixed in 0.36.14.
  //
  // 0.37.5: project_id alone is the canonical workspace identity
  // (post-unification, one agent per workspace, agent name is
  // metadata not address). The agent name doesn't need to be in
  // the React key.
  //
  // 0.39.39: the React key ALSO folds in `daemonOwnsChat` so a
  // capability flip remounts cleanly into the other implementation
  // (no shared-hook-order hazard between the two component bodies).
  const key = `${projectId}:${daemonOwnsChat ? 'daemon' : 'legacy'}`
  return daemonOwnsChat ? (
    <AgentChatTerminalDaemon
      key={key}
      agentName={agentName}
      projectId={projectId}
      projectPath={projectPath}
      restoredSessionId={restoredSessionId}
    />
  ) : (
    <AgentChatTerminalLegacy
      key={key}
      agentName={agentName}
      projectId={projectId}
      projectPath={projectPath}
      restoredSessionId={restoredSessionId}
    />
  )
}

interface AgentChatTerminalProps {
  agentName: string
  projectId: string
  projectPath: string
  restoredSessionId?: string
}

// ── Shared header: display name, history dropdown, refresh button ─────────
//
// Both the daemon-owned and legacy implementations render the same chrome.
// Extracting it keeps the two bodies focused on their (very different)
// lifecycle logic. The dropdown / refresh handlers are injected so each
// path wires them to its own respawn mechanism.

interface ChatHeaderProps {
  displayName: string
  projectPath: string
  currentSessionId: string | null
  onRefresh: () => void
  refreshing: boolean
  onSwitchSession: (newSessionId: string) => void
}

interface HistorySession {
  sessionId: string
  title: string
  timestamp: number
  messageCount: number
}

function ChatHeader({
  displayName,
  projectPath,
  currentSessionId,
  onRefresh,
  refreshing,
  onSwitchSession,
}: ChatHeaderProps): React.JSX.Element {
  const [historySessions, setHistorySessions] = useState<HistorySession[]>([])
  const [historyOpen, setHistoryOpen] = useState(false)

  // 0.37.12 — fetch chat history for the dropdown title + popover list.
  // Runs on mount, when the current session changes (title converges
  // after the first message), and when the popover opens.
  useEffect(() => {
    let cancelled = false
    void daemonCliGet<Array<{
      sessionId: string
      title: string
      timestamp: number
      messageCount: number
      provider: string
    }>>('chat/list', { project_path: projectPath })
      .then((rows) => {
        if (cancelled) return
        const claudeOnly = rows
          .filter((r) => r.provider === 'claude' || !r.provider)
          .sort((a, b) => b.timestamp - a.timestamp)
        setHistorySessions(claudeOnly)
      })
      .catch((err) => {
        console.warn('[AgentChatPane] chat/list failed:', err)
      })
    return () => { cancelled = true }
  }, [projectPath, currentSessionId, historyOpen])

  const currentChatTitle = useMemo<string>(() => {
    if (!currentSessionId) return 'New chat'
    const found = historySessions.find((s) => s.sessionId === currentSessionId)
    if (found?.title) return found.title
    return 'New chat'
  }, [historySessions, currentSessionId])

  return (
    <div className="px-3 py-2 border-b border-[var(--color-border)] flex-shrink-0 flex items-center gap-3 relative">
      <span className="text-xs font-semibold text-[var(--color-text-primary)] truncate flex-shrink-0">
        {displayName}
      </span>
      {/* spacer pushes the dropdown + refresh to the right. */}
      <div className="flex-1" />
      {/* 0.37.12 — chat-history dropdown. Lets the user switch the
          pinned chat to a different past session (escape hatch for
          orphaned/deleted sessions or just to revisit). */}
      <button
        type="button"
        onClick={() => setHistoryOpen((v) => !v)}
        title="Switch pinned chat to a different past Claude session"
        aria-label="Switch pinned chat session"
        aria-haspopup="listbox"
        aria-expanded={historyOpen}
        className="inline-flex items-center gap-1 px-2 py-0.5 rounded text-[10px] font-medium text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] transition-colors no-drag cursor-pointer min-w-0"
      >
        <span className="truncate max-w-[28ch]">{currentChatTitle}</span>
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className="flex-shrink-0">
          <path d={historyOpen ? 'M18 15l-6-6-6 6' : 'M6 9l6 6 6-6'} />
        </svg>
      </button>
      {historyOpen && (
        <div
          role="listbox"
          className="absolute right-12 top-full mt-1 z-20 w-[36ch] max-h-[60vh] overflow-y-auto bg-[var(--color-bg-elevated)] border border-[var(--color-border)] shadow-2xl py-1"
        >
          {historySessions.length === 0 ? (
            <div className="px-3 py-2 text-[10px] text-[var(--color-text-muted)]">
              No past sessions yet.
            </div>
          ) : (
            historySessions.map((s) => {
              const isCurrent = s.sessionId === currentSessionId
              return (
                <button
                  key={s.sessionId}
                  type="button"
                  role="option"
                  aria-selected={isCurrent}
                  onClick={() => {
                    setHistoryOpen(false)
                    onSwitchSession(s.sessionId)
                  }}
                  className={`w-full text-left px-3 py-1.5 text-[11px] flex items-center gap-2 transition-colors no-drag cursor-pointer ${
                    isCurrent
                      ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                      : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]'
                  }`}
                >
                  <span className="flex-1 truncate">{s.title || 'Untitled chat'}</span>
                  <span className="flex-shrink-0 text-[9px] text-[var(--color-text-muted)] opacity-70">
                    {s.messageCount}
                  </span>
                  {isCurrent && (
                    <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className="flex-shrink-0 text-[var(--color-accent)]">
                      <path d="M5 12l5 5 9-11" />
                    </svg>
                  )}
                </button>
              )
            })
          )}
        </div>
      )}
      <button
        type="button"
        onClick={onRefresh}
        disabled={refreshing}
        title="Restart chat session — kills the current Claude process and spawns a fresh resume. Use after typing `exit` or when the session is unresponsive."
        aria-label="Refresh chat session"
        className="inline-flex items-center justify-center h-5 w-5 rounded text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50 disabled:cursor-not-allowed transition-colors flex-shrink-0"
      >
        {/* Inline SVG keeps this self-contained (no icon-lib dep). */}
        <svg
          xmlns="http://www.w3.org/2000/svg"
          width="14"
          height="14"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
          className={refreshing ? 'animate-spin' : ''}
          aria-hidden="true"
        >
          <path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
          <path d="M3 3v5h5" />
          <path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16" />
          <path d="M16 16h5v5" />
        </svg>
      </button>
    </div>
  )
}

// Shared hook: resolve the AGENT.md `display_name:` (falls back to the
// technical agent name). Used by both implementations.
function useDisplayName(projectPath: string, agentName: string): string {
  const [displayName, setDisplayName] = useState<string>(agentName)
  useEffect(() => {
    let cancelled = false
    agentDisplayName(projectPath)
      .then((n) => { if (!cancelled && n) setDisplayName(n) })
      .catch(() => { /* keep agentName as fallback */ })
    return () => { cancelled = true }
  }, [projectPath, agentName])
  useEffect(() => {
    let unlisten: (() => void) | null = null
    let cancelled = false
    listen('sync:projects', () => {
      agentDisplayName(projectPath)
        .then((n) => { if (n) setDisplayName(n) })
        .catch(() => {})
    }).then((u) => { if (cancelled) u(); else unlisten = u })
    return () => { cancelled = true; unlisten?.() }
  }, [projectPath])
  return displayName
}

// ── Wire types for ensure-pinned-chat (FROZEN CONTRACT, #683) ────────────
//
// POST /cli/workspace/ensure-pinned-chat ← { project, forceRespawn? }
// → { sessionId, claudeSessionId, resumedExisting, command, args,
//     cols, rows, reused }
interface EnsurePinnedChatResponse {
  sessionId: string
  claudeSessionId: string
  resumedExisting: boolean
  command: string
  args: string[]
  cols: number
  rows: number
  reused: boolean
}

async function ensurePinnedChat(
  projectPath: string,
  opts?: { forceRespawn?: boolean; restoredSessionId?: string; explicitSelection?: boolean },
): Promise<EnsurePinnedChatResponse> {
  // host-aware POST (daemonCliPost). The daemon is authoritative;
  // restoredSessionId rides along ONLY as an offline hint (PRD §4.5 / D3).
  // explicitSelection (Issue B) is set ONLY on a dropdown session switch:
  // it tells the daemon resolver to honor the just-persisted session_id
  // directly and skip the GH#24 converge fallback (no silent revert).
  return daemonCliPost<EnsurePinnedChatResponse>('workspace/ensure-pinned-chat', {
    project: projectPath,
    ...(opts?.forceRespawn ? { forceRespawn: true } : {}),
    ...(opts?.restoredSessionId ? { restoredSessionId: opts.restoredSessionId } : {}),
    ...(opts?.explicitSelection ? { explicitSelection: true } : {}),
  })
}

/**
 * 0.39.39 (#683) — DAEMON-OWNED pinned-chat lifecycle.
 *
 * The daemon owns spawn/resume/refresh/switch. This component:
 *  - mount       → POST ensure-pinned-chat {project} → attach grid-WS
 *  - refresh     → POST ensure-pinned-chat {project, forceRespawn:true};
 *                  re-attach on the daemon's `SessionAdded` broadcast
 *  - switch      → set-chat-session (persist) then ensure(forceRespawn:true)
 *  - SessionAdded(this workspace)   → re-attach (the daemon respawned)
 *  - SessionRemoved(this workspace) → show idle (NO auto-respawn)
 *
 * No resolve loop, no `stampSessionId`, no `restoredSessionId`-driven
 * re-resolve, no self-retrigger guard, NO circuit breaker — those only
 * existed to bound a loop the daemon-owned model removes by construction.
 *
 * TerminalPane attaches by registering its v2 session under the canonical
 * workspace key (`attachAgentName={projectId}`). Since `ensure-pinned-chat`
 * spawns under THAT SAME key, TerminalPane's idempotent `/cli/sessions/
 * v2/spawn` (no command/args) reuses the daemon-spawned PTY rather than
 * creating a new one — that's the "attach". A bumped `attachNonce` forces
 * a clean re-attach after a forceRespawn / SessionAdded.
 */
function AgentChatTerminalDaemon({ agentName, projectId, projectPath, restoredSessionId }: AgentChatTerminalProps): React.JSX.Element {
  const containerRef = useRef<HTMLDivElement>(null)
  const terminalIdRef = useRef(agentChatId(projectId, agentName))
  const displayName = useDisplayName(projectPath, agentName)

  // P1.A — bind this pinned-Chat pane to ITS OWN project upfront (see the
  // legacy body for the full rationale). Idempotent.
  useEffect(() => {
    useActiveAgentsStore.getState().bindPaneProject(terminalIdRef.current, projectId)
  }, [projectId])

  // Resolve phase. `ensuring` → first ensure in flight; `ready` → the
  // daemon confirmed a live session and we render TerminalPane; `idle` →
  // the daemon reported the session removed (e.g. user typed `exit`) — we
  // show the idle pane with a Retry button and DO NOT auto-respawn;
  // `error` → ensure failed.
  type Phase =
    | { kind: 'ensuring' }
    | { kind: 'ready'; sessionId: string; claudeSessionId: string }
    | { kind: 'idle' }
    | { kind: 'error'; message: string }
  const [phase, setPhase] = useState<Phase>({ kind: 'ensuring' })
  const [refreshing, setRefreshing] = useState(false)
  // Bumped to force TerminalPane to re-attach (remount) after a daemon
  // respawn (refresh / dropdown switch / SessionAdded). The daemon kills
  // the old PTY + spawns a new one under the same key; a fresh TerminalPane
  // re-runs its idempotent spawn POST and binds to the NEW PTY.
  const [attachNonce, setAttachNonce] = useState(0)

  // #689 — the session id TerminalPane is currently attached to. The
  // remount-guard: a SessionAdded broadcast only forces a re-attach
  // (attachNonce bump) when it represents a GENUINE change — a session id
  // different from the one we already attached to. Without this guard the
  // mount path's OWN side-effect re-attached the pane: mount → ensure(false)
  // → daemon emits SessionAdded for the session we just ensured → onAdded
  // bumped attachNonce → TerminalPane REMOUNTED (the 1–3s flicker + the tab
  // icon/label reset on the throwaway mount). Same "don't react to your own
  // side-effect" class as #682. `ensure()` and `onAdded` both stamp this ref
  // with the session they settled on, so the echo for that session no-ops.
  const attachedSessionIdRef = useRef<string | null>(null)

  // The claude session id currently pinned (from the last ensure / the
  // SessionAdded event). Drives the dropdown highlight + title lookup.
  const claudeSessionId =
    phase.kind === 'ready' ? phase.claudeSessionId : null

  // ── ensure() — the one daemon RPC this component leans on ────────────
  // Idempotent on the daemon side. `forceRespawn` kills + respawns. The
  // SessionAdded broadcast re-attaches; but we also adopt the ensure
  // RESPONSE directly so a cold mount renders without waiting for the WS.
  const ensure = useCallback(
    async (forceRespawn: boolean, explicitSelection = false): Promise<void> => {
      try {
        const res = await ensurePinnedChat(projectPath, {
          forceRespawn,
          // Issue B — only true on a dropdown switch; honors the picked
          // session id at the daemon and skips the converge fallback.
          explicitSelection,
          // Offline hint only (D3) — daemon wins when reachable.
          restoredSessionId,
        })
        setPhase({
          kind: 'ready',
          sessionId: res.sessionId,
          claudeSessionId: res.claudeSessionId,
        })
        // #689 — record which session we're now attached to so the
        // SessionAdded echo for THIS session is recognised as our own
        // side-effect and doesn't trigger a spurious remount.
        attachedSessionIdRef.current = res.sessionId
        // A respawn produced a NEW PTY; force TerminalPane to re-attach so
        // it binds to it. We stamped the ref FIRST so the daemon's
        // SessionAdded echo for this same new session won't double-bump.
        // (A cold mount with forceRespawn=false does NOT bump — TerminalPane
        // just mounted once and is already on the right PTY.)
        if (forceRespawn) setAttachNonce((n) => n + 1)
      } catch (err) {
        console.error('[AgentChatPane] ensure-pinned-chat failed:', err)
        setPhase({
          kind: 'error',
          message: err instanceof Error ? err.message : String(err),
        })
      }
    },
    [projectPath, restoredSessionId],
  )

  // Mount → ensure (find-or-spawn; daemon resolves resume vs fresh). Cold
  // boot / relaunch-revive is just this call: the daemon reads
  // workspace_sessions canonically (#679 preserved).
  useEffect(() => {
    void ensure(false)
    // Only on (re)mount per workspace. `ensure` is stable across
    // restoredSessionId churn for THIS workspace; we deliberately don't
    // re-ensure when the offline hint changes (daemon is authoritative).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectPath])

  // Subscribe to the daemon's session lifecycle for THIS workspace.
  //   SessionAdded(agent_name === projectId) → re-attach (daemon respawned
  //     on refresh / switch / k2so msg find-or-spawn).
  //   SessionRemoved(agent_name === projectId) → show idle. NO auto-respawn
  //     (PRD §4: the daemon never auto-spawns a pinned chat).
  useEffect(() => {
    const unsubscribe = subscribeToWorkspaceSessionEvents(projectPath, {
      onAdded: (event) => {
        if (event.agent_name !== projectId) return
        // #689 — remount-guard. Only re-attach on a GENUINE session change.
        // A SessionAdded echoing the session we already ensured/attached
        // (our own mount/refresh side-effect) must be a no-op — bumping
        // attachNonce here was what remounted TerminalPane in the first
        // 1–3s and reset the tab icon/label (the flicker). When the session
        // id matches what we're attached to, do nothing.
        if (event.session_id === attachedSessionIdRef.current) return
        attachedSessionIdRef.current = event.session_id
        setPhase({
          kind: 'ready',
          sessionId: event.session_id,
          claudeSessionId: event.session_id,
        })
        setAttachNonce((n) => n + 1)
      },
      onRemoved: (event) => {
        if (event.agent_name !== projectId) return
        // #689 — the attached session is gone; clear the guard ref so a
        // later fresh session (Retry) is recognised as a genuine change and
        // re-attaches.
        attachedSessionIdRef.current = null
        setPhase({ kind: 'idle' })
      },
    })
    return unsubscribe
  }, [projectPath, projectId])

  // Refresh → forceRespawn. The daemon kills + respawns; we re-attach via
  // the ensure response AND the SessionAdded broadcast.
  const handleRefresh = useCallback(async (): Promise<void> => {
    if (refreshing) return
    setRefreshing(true)
    setPhase({ kind: 'ensuring' })
    await ensure(true)
    setRefreshing(false)
  }, [refreshing, ensure])

  // Dropdown switch → persist via set-chat-session, then forceRespawn so
  // the daemon respawns on the newly-selected session.
  const handleSwitchSession = useCallback(
    async (newSessionId: string): Promise<void> => {
      if (!newSessionId || newSessionId === claudeSessionId) return
      setRefreshing(true)
      setPhase({ kind: 'ensuring' })
      try {
        // HOST-AWARE persist of the pinned session (same route the legacy
        // path uses). GET because the route reads query params.
        await daemonCliGet('workspace/set-chat-session', {
          project: projectPath,
          session_id: newSessionId,
        })
      } catch (err) {
        console.error('[AgentChatPane] switchToSession DB update failed:', err)
        setRefreshing(false)
        // Re-ensure so we don't strand the pane in `ensuring`.
        void ensure(false)
        return
      }
      // Issue B — explicitSelection:true so the daemon honors the picked
      // session and never silently reverts to the newest on-disk session.
      // ensure() sets phase=error if the chosen session is gone on disk
      // (the daemon returns an Err) rather than swapping to a different one.
      await ensure(true, true)
      setRefreshing(false)
    },
    [claudeSessionId, projectPath, ensure],
  )

  if (phase.kind === 'error') {
    return (
      <div className="flex flex-col items-center justify-center h-full gap-3 px-6 text-center">
        <div className="text-xs font-semibold text-[var(--color-text-primary)]">
          Chat session failed to start
        </div>
        <div className="text-[11px] text-[var(--color-text-muted)] max-w-[40ch]">
          {phase.message || 'The daemon could not start the chat session.'}
        </div>
        <RetryButton onClick={handleRefresh} refreshing={refreshing} />
      </div>
    )
  }

  if (phase.kind === 'idle') {
    return (
      <div className="flex flex-col items-center justify-center h-full gap-3 px-6 text-center">
        <div className="text-xs font-semibold text-[var(--color-text-primary)]">
          Chat session ended
        </div>
        <div className="text-[11px] text-[var(--color-text-muted)] max-w-[40ch]">
          The chat process exited. Click Retry to start a fresh session.
        </div>
        <RetryButton onClick={handleRefresh} refreshing={refreshing} />
      </div>
    )
  }

  if (phase.kind === 'ensuring') {
    return (
      <div className="flex items-center justify-center h-full text-xs text-[var(--color-text-muted)]">
        Loading session…
      </div>
    )
  }

  // phase.kind === 'ready'
  return (
    <div ref={containerRef} className="h-full flex flex-col bg-[var(--color-bg)] overflow-hidden">
      <ChatHeader
        displayName={displayName}
        projectPath={projectPath}
        currentSessionId={claudeSessionId}
        onRefresh={() => void handleRefresh()}
        refreshing={refreshing}
        onSwitchSession={(sid) => void handleSwitchSession(sid)}
      />
      <div className="flex-1 min-h-0">
        <TerminalPane
          // Remount on each daemon respawn so TerminalPane re-attaches to
          // the NEW PTY under the canonical key.
          key={attachNonce}
          terminalId={terminalIdRef.current}
          cwd={projectPath}
          // NO command/args — the daemon already spawned the PTY in
          // ensure-pinned-chat. TerminalPane's idempotent /cli/sessions/
          // v2/spawn (keyed on attachAgentName) ATTACHES to it rather than
          // spawning fresh. The renderer no longer builds claude args.
          attachAgentName={projectId}
          seedLabel={displayName}
          lockLabel={true}
          // 0.39.39: NO onChildExit breaker wiring on the daemon-owned
          // path. Exit is observed by the DAEMON, which broadcasts
          // SessionRemoved → the subscription above flips us to idle. No
          // renderer-side breaker because there's no renderer respawn loop
          // to bound.
        />
      </div>
    </div>
  )
}

// Small shared retry button used by the daemon-owned error/idle panes.
function RetryButton({ onClick, refreshing }: { onClick: () => void; refreshing: boolean }): React.JSX.Element {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={refreshing}
      className="inline-flex items-center gap-1.5 px-3 py-1 rounded text-[11px] font-medium bg-[var(--color-bg-hover)] text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/20 disabled:opacity-50 disabled:cursor-not-allowed transition-colors no-drag cursor-pointer"
    >
      <svg
        xmlns="http://www.w3.org/2000/svg"
        width="12"
        height="12"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        className={refreshing ? 'animate-spin' : ''}
        aria-hidden="true"
      >
        <path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
        <path d="M3 3v5h5" />
        <path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16" />
        <path d="M16 16h5v5" />
      </svg>
      Retry
    </button>
  )
}

/**
 * PRE-0.39.39 renderer-orchestrated pinned-chat path — the FALLBACK used
 * when the active host does NOT support `daemon-pinned-chat` (an older or
 * remote daemon without the `ensure-pinned-chat` route). UNCHANGED from
 * the 0.39.38 implementation: the renderer resolves resume vs fresh,
 * builds claude args, spawns via TerminalPane, stamps the session id back,
 * and is protected by the #682 self-retrigger guard + circuit breaker.
 *
 * Kept verbatim (rather than deleted) BECAUSE this path is the only one
 * that can still loop, so the band-aids must stay attached to it. The
 * daemon-owned path above has neither the loop nor the band-aids.
 */
function AgentChatTerminalLegacy({ agentName, projectId, projectPath, restoredSessionId }: AgentChatTerminalProps): React.JSX.Element {
  const containerRef = useRef<HTMLDivElement>(null)
  const terminalIdRef = useRef(agentChatId(projectId, agentName))

  useEffect(() => {
    useActiveAgentsStore.getState().bindPaneProject(terminalIdRef.current, projectId)
  }, [projectId])
  const [launchConfig, setLaunchConfig] = useState<{
    command: string
    args: string[]
    cwd: string
  } | null>(null)
  const [ready, setReady] = useState(false)
  // K2 #682 — spawn-loop circuit breaker (see chat-spawn-breaker.ts).
  const breakerRef = useRef<BreakerState>(initialBreakerState())
  const [breakerTripped, setBreakerTripped] = useState(false)
  // K2 #682 — self-retrigger guard memo.
  const lastResolvedRef = useRef<ResolveMemo | null>(null)
  const displayName = useDisplayName(projectPath, agentName)

  // 0.37.12 — chat-history dropdown state.
  const [historySessions, setHistorySessions] = useState<Array<{
    sessionId: string
    title: string
    timestamp: number
    messageCount: number
  }>>([])
  const [historyOpen, setHistoryOpen] = useState(false)
  // The Claude session id currently driving the live PTY — derived from
  // launchConfig.args (`--resume <X>` or `--session-id <X>`).
  const currentSessionId = useMemo<string | null>(() => {
    const args = launchConfig?.args
    if (!args) return null
    for (let i = 0; i + 1 < args.length; i++) {
      if (args[i] === '--resume' || args[i] === '--session-id') {
        return args[i + 1]
      }
    }
    return null
  }, [launchConfig])

  // Bumped on every refresh-button click to force a clean remount of
  // TerminalPane (key={refreshNonce}) and a re-run of the resolve effect.
  const [refreshNonce, setRefreshNonce] = useState(0)
  const [refreshing, setRefreshing] = useState(false)

  // Listen for chat:refreshed broadcasts (cross-window remount).
  useEffect(() => {
    let cancelled = false
    let unlisten: (() => void) | null = null
    listen<{ projectPath: string }>('chat:refreshed', (event) => {
      if (event.payload.projectPath !== projectPath) return
      setLaunchConfig(null)
      setReady(false)
      setRefreshNonce((n) => n + 1)
    }).then((u) => { if (cancelled) u(); else unlisten = u })
    return () => { cancelled = true; unlisten?.() }
  }, [projectPath])

  const handleRefresh = useCallback(async (): Promise<void> => {
    if (refreshing) return
    setRefreshing(true)
    // K2 #682 — reset the breaker + self-retrigger memo for an explicit
    // "try again".
    breakerRef.current = resetBreaker()
    setBreakerTripped(false)
    lastResolvedRef.current = null
    // Kill the daemon-owned PTY (best-effort).
    try {
      const creds = await getDaemonWs()
      await fetch(
        `${daemonHttpBase(creds)}/cli/sessions/v2/close?token=${creds.token}`,
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ agent_name: projectId, force: true }),
        },
      ).catch(() => {})
    } catch { /* ignore — refresh proceeds either way */ }

    invoke('k2so_chat_refresh_broadcast', { projectPath })
      .catch((e) => console.warn('[chat-refresh] broadcast failed:', e))

    setLaunchConfig(null)
    setReady(false)
    setRefreshing(false)
  }, [projectId, projectPath, agentName, refreshing])

  // 0.37.12 — fetch chat history for the dropdown title + popover list.
  useEffect(() => {
    let cancelled = false
    void daemonCliGet<Array<{
      sessionId: string
      title: string
      timestamp: number
      messageCount: number
      provider: string
    }>>('chat/list', { project_path: projectPath })
      .then((rows) => {
        if (cancelled) return
        const claudeOnly = rows
          .filter((r) => r.provider === 'claude' || !r.provider)
          .sort((a, b) => b.timestamp - a.timestamp)
        setHistorySessions(claudeOnly)
      })
      .catch((err) => {
        console.warn('[AgentChatPane] chat/list failed:', err)
      })
    return () => { cancelled = true }
  }, [projectPath, currentSessionId, historyOpen])

  const currentChatTitle = useMemo<string>(() => {
    if (!currentSessionId) return 'New chat'
    const found = historySessions.find((s) => s.sessionId === currentSessionId)
    if (found?.title) return found.title
    return 'New chat'
  }, [historySessions, currentSessionId])

  // Switch the pinned chat tab to a different past session.
  const switchToSession = useCallback(async (newSessionId: string): Promise<void> => {
    if (!newSessionId || newSessionId === currentSessionId) {
      setHistoryOpen(false)
      return
    }
    setHistoryOpen(false)
    try {
      await daemonCliGet('workspace/set-chat-session', {
        project: projectPath,
        session_id: newSessionId,
      })
    } catch (err) {
      console.error('[AgentChatPane] switchToSession DB update failed:', err)
      return
    }
    try {
      useTabsStore.getState().stampAgentSessionId(agentName, projectPath, newSessionId, projectId)
    } catch (err) {
      console.warn('[AgentChatPane] stampAgentSessionId failed:', err)
    }
    void handleRefresh()
  }, [currentSessionId, projectPath, agentName, projectId, handleRefresh])

  useEffect(() => {
    // K2 #682 — self-retrigger guard.
    if (
      isSelfRetrigger(lastResolvedRef.current, {
        refreshNonce,
        projectPath,
        restoredSessionId,
      })
    ) {
      return
    }

    // K2 #682 — breaker gate.
    if (breakerRef.current.tripped) {
      return
    }

    let cancelled = false
    const stampSessionId = (sid: string | null | undefined): void => {
      lastResolvedRef.current = {
        refreshNonce,
        projectPath,
        sessionId: sid ?? null,
      }
      if (!sid) return
      try {
        useTabsStore.getState().stampAgentSessionId(agentName, projectPath, sid, projectId)
      } catch (err) {
        console.warn('[AgentChatPane] stampAgentSessionId failed:', err)
      }
    }
    const resolve = async (): Promise<void> => {
      const myTerminalId = terminalIdRef.current

      // GH#679 / GH#681 — Step 0: reconcile the layout hint against the
      // daemon's canonical session.
      if (restoredSessionId && !cancelled) {
        let decision: ColdBootDecision = { kind: 'fallback', sessionId: restoredSessionId }
        try {
          const canonical = await resumeChatArgs(projectPath)
          decision = reconcileColdBootSession(restoredSessionId, canonical)
        } catch (err) {
          console.warn('[AgentChatPane] cold-boot SQLite reconcile failed, using layout hint:', err)
        }
        if (cancelled) return

        if (decision.kind === 'fresh') {
          console.info(
            '[AgentChatPane] cold-boot fresh session (resumedExisting=false):',
            decision.sessionId,
            '— launching with --session-id, NOT --resume',
          )
          setLaunchConfig({
            command: 'claude',
            args: decision.args,
            cwd: projectPath,
          })
        } else {
          if (decision.kind === 'resume' && decision.sessionId !== restoredSessionId) {
            console.info(
              '[AgentChatPane] cold-boot revive: SQLite session',
              decision.sessionId,
              'overrides layout hint',
              restoredSessionId,
            )
          }
          setLaunchConfig({
            command: 'claude',
            args: ['--dangerously-skip-permissions', '--resume', decision.sessionId],
            cwd: projectPath,
          })
        }
        daemonCliGet('agents/lock', {
          project: projectPath,
          agent: agentName,
          terminal_id: myTerminalId,
          owner: 'user',
        }).catch(() => {})
        stampSessionId(decision.sessionId)
        setReady(true)
        return
      }

      // Step 1: Reattach if PTY already alive in this Tauri session
      try {
        const exists = await terminalExists(myTerminalId)
        if (!cancelled && exists) {
          setLaunchConfig(null)
          setReady(true)
          return
        }
      } catch { /* fall through */ }

      // Step 1b: Check the daemon for an existing session under this
      // workspace's canonical key.
      try {
        const json = await invoke<string>('k2so_session_lookup_by_agent', {
          agent: projectId,
        })
        const data = JSON.parse(json) as {
          sessionAlive?: boolean
          sessionId?: string | null
          isV2?: boolean
        }
        if (!cancelled && data.sessionAlive) {
          console.info(
            '[AgentChatPane] daemon has live session for',
            projectId,
            'session:',
            data.sessionId,
            'isV2:',
            data.isV2,
          )
        }
      } catch { /* informational only — fall through */ }

      // Step 2: Build a *bare resume* command for the chat tab.
      try {
        const result = await resumeChatArgs(projectPath)
        if (!cancelled && result) {
          setLaunchConfig({
            command: result.command,
            args: result.args,
            cwd: result.cwd,
          })
          daemonCliGet('agents/lock', {
            project: projectPath,
            agent: agentName,
            terminal_id: myTerminalId,
            owner: 'user',
          }).catch(() => {})
          stampSessionId(result.resumeSession)
          setReady(true)
          return
        }
      } catch (err) {
        console.warn('[AgentChatPane] resume_chat_args failed, falling back:', err)
      }

      // Step 3: Last-resort fallback — fresh session
      if (!cancelled) {
        setLaunchConfig({
          command: 'claude',
          args: ['--dangerously-skip-permissions'],
          cwd: projectPath,
        })
        daemonCliGet('agents/lock', {
          project: projectPath,
          agent: agentName,
          terminal_id: myTerminalId,
          owner: 'user',
        }).catch(() => {})
        setReady(true)
      }
    }
    resolve()
    return () => { cancelled = true }
  }, [agentName, projectId, projectPath, refreshNonce, restoredSessionId])

  // K2 #682 — record the spawn timestamp for the circuit breaker.
  useEffect(() => {
    if (ready && launchConfig?.command) {
      breakerRef.current = recordSpawn(breakerRef.current, Date.now())
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ready, refreshNonce])

  // K2 #682 — fold a child exit into the spawn-loop breaker.
  const handleChildExit = useCallback((exitCode: number | null): void => {
    const decision = recordExit(breakerRef.current, {
      now: Date.now(),
      exitCode,
    })
    breakerRef.current = decision.next
    if (decision.justTripped) {
      console.error(
        '[AgentChatPane] spawn-loop circuit breaker TRIPPED — chat exited',
        `${MAX_RAPID_EXITS}× rapidly; halting auto-respawn (manual refresh to retry)`,
      )
      setBreakerTripped(true)
    }
  }, [])

  if (breakerTripped) {
    return (
      <div className="flex flex-col items-center justify-center h-full gap-3 px-6 text-center">
        <div className="text-xs font-semibold text-[var(--color-text-primary)]">
          Chat session failed to start
        </div>
        <div className="text-[11px] text-[var(--color-text-muted)] max-w-[40ch]">
          The chat process exited repeatedly right after starting, so K2
          stopped retrying to avoid a spawn loop. Click Retry to try again.
        </div>
        <RetryButton onClick={() => void handleRefresh()} refreshing={refreshing} />
      </div>
    )
  }

  if (!ready) {
    return (
      <div className="flex items-center justify-center h-full text-xs text-[var(--color-text-muted)]">
        Loading session…
      </div>
    )
  }

  return (
    <div ref={containerRef} className="h-full flex flex-col bg-[var(--color-bg)] overflow-hidden">
      <div className="px-3 py-2 border-b border-[var(--color-border)] flex-shrink-0 flex items-center gap-3 relative">
        <span className="text-xs font-semibold text-[var(--color-text-primary)] truncate flex-shrink-0">
          {displayName}
        </span>
        <div className="flex-1" />
        <button
          type="button"
          onClick={() => setHistoryOpen((v) => !v)}
          title="Switch pinned chat to a different past Claude session"
          aria-label="Switch pinned chat session"
          aria-haspopup="listbox"
          aria-expanded={historyOpen}
          className="inline-flex items-center gap-1 px-2 py-0.5 rounded text-[10px] font-medium text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] transition-colors no-drag cursor-pointer min-w-0"
        >
          <span className="truncate max-w-[28ch]">{currentChatTitle}</span>
          <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className="flex-shrink-0">
            <path d={historyOpen ? 'M18 15l-6-6-6 6' : 'M6 9l6 6 6-6'} />
          </svg>
        </button>
        {historyOpen && (
          <div
            role="listbox"
            className="absolute right-12 top-full mt-1 z-20 w-[36ch] max-h-[60vh] overflow-y-auto bg-[var(--color-bg-elevated)] border border-[var(--color-border)] shadow-2xl py-1"
          >
            {historySessions.length === 0 ? (
              <div className="px-3 py-2 text-[10px] text-[var(--color-text-muted)]">
                No past sessions yet.
              </div>
            ) : (
              historySessions.map((s) => {
                const isCurrent = s.sessionId === currentSessionId
                return (
                  <button
                    key={s.sessionId}
                    type="button"
                    role="option"
                    aria-selected={isCurrent}
                    onClick={() => void switchToSession(s.sessionId)}
                    className={`w-full text-left px-3 py-1.5 text-[11px] flex items-center gap-2 transition-colors no-drag cursor-pointer ${
                      isCurrent
                        ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                        : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]'
                    }`}
                  >
                    <span className="flex-1 truncate">{s.title || 'Untitled chat'}</span>
                    <span className="flex-shrink-0 text-[9px] text-[var(--color-text-muted)] opacity-70">
                      {s.messageCount}
                    </span>
                    {isCurrent && (
                      <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className="flex-shrink-0 text-[var(--color-accent)]">
                        <path d="M5 12l5 5 9-11" />
                      </svg>
                    )}
                  </button>
                )
              })
            )}
          </div>
        )}
        <button
          type="button"
          onClick={() => void handleRefresh()}
          disabled={refreshing}
          title="Restart chat session — kills the current Claude process and spawns a fresh resume. Use after typing `exit` or when the session is unresponsive."
          aria-label="Refresh chat session"
          className="inline-flex items-center justify-center h-5 w-5 rounded text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50 disabled:cursor-not-allowed transition-colors flex-shrink-0"
        >
          <svg
            xmlns="http://www.w3.org/2000/svg"
            width="14"
            height="14"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
            className={refreshing ? 'animate-spin' : ''}
            aria-hidden="true"
          >
            <path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
            <path d="M3 3v5h5" />
            <path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16" />
            <path d="M16 16h5v5" />
          </svg>
        </button>
      </div>
      <div className="flex-1 min-h-0">
        <TerminalPane
          key={refreshNonce}
          terminalId={terminalIdRef.current}
          cwd={launchConfig?.cwd ?? projectPath}
          command={launchConfig?.command}
          args={launchConfig?.args}
          attachAgentName={projectId}
          seedLabel={displayName}
          lockLabel={true}
          // K2 #682 — feed child-exit into the spawn-loop circuit breaker.
          // ONLY the fallback path wires this: the daemon-owned path relies
          // on the daemon's SessionRemoved broadcast for exit.
          onChildExit={handleChildExit}
        />
      </div>
    </div>
  )
}
