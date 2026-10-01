import { useState, useEffect, useRef, useCallback, useMemo } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { terminalExists } from '@/lib/terminal-daemon'
import { closeV2Session } from '@/stores/tabs'
import { useRoom, useRoomProjects, useRoomSupports } from '@/components/Room/RoomContext'
import { TerminalPane } from '@/kessel-term/TerminalPane'
import { agentChatId } from '@/lib/terminal-id'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import {
  mapMsgResponseToStatus,
  type MsgResponse,
} from '@/components/Terminal/terminalCompose'
import {
  ContinueNewChatDialog,
  continueSendError,
  providerKeyForCommand,
  type ContinueNewChatSource,
  type ContinueSpawnRequest,
} from '@/components/ChatHistory/ContinueNewChatDialog'
import { agentDisplayName, claimAgentLock, resumeChatArgs, setChatSession, reconcileColdBootSession, type ColdBootDecision } from '@/lib/workspace-agent'
import { ProviderIcon } from '@/components/AgentIcon/ProviderIcon'
import { useStore } from 'zustand'
import { subscribeToWorkspaceSessionEvents, onChatHistoryChanged } from '@/stores/session-events'
import { chatDisplayName, resolvePinnedChatCopyableAddress } from '@/lib/chat-session-tab'
import { SessionViewMenu } from '@/components/SessionView/SessionViewMenu'
import { chatHarnessName } from '@/components/SessionView/chatHarness'
import { DEFAULT_SPLIT_LEFT, DEFAULT_SPLIT_RIGHT, type SplitPaneView } from '@/components/SessionView/sessionViewTab'
import { PinnedSessionBody } from '@/components/SessionView/AgentSessionChrome'
import { ThreadOverlayPane } from '@/components/SessionView/ThreadOverlayPane'
import { ChatterOverlayPane } from '@/components/SessionView/ChatterOverlayPane'
import { useSessionViewTab } from '@/components/SessionView/useSessionViewTab'
import type { SessionViewTab } from '@/components/SessionView/sessionViewTab'
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
import type { ServerScope } from '@/kessel/server-scope'

interface AgentChatPaneProps {
  agentName: string
  projectPath: string
  /** 0.37.12 — canonical session id restored from the serialized layout.
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
  /** Pinned-chat retention — fired when the DAEMON broadcasts
   *  SessionRemoved for this workspace's canonical session (daemon-owned
   *  path only; fires after the pane flips to its idle state). The
   *  PinnedChatRetainer uses it to evict a BACKGROUNDED retained
   *  instance so the next visit remounts fresh and re-runs
   *  ensure-pinned-chat's find-or-spawn — preserving today's
   *  revisit-respawns behavior instead of parking an idle pane. */
  onDaemonSessionRemoved?: () => void
}

/**
 * Chat pinned tab — runs the workspace agent's persistent canonical
 * session (any provider's harness since agent-degeneralization Slice 4;
 * the daemon resolves which command to spawn from the stored harness).
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
export function AgentChatPane({ agentName, projectPath, restoredSessionId, onDaemonSessionRemoved }: AgentChatPaneProps): React.JSX.Element {
  // Resolve project id synchronously from the projects store; the chat tab
  // will not render until a real id is available so the legacy collision
  // bug can never reappear via this surface.
  //
  // MS3 — resolved in THIS room's own project list: a path is only an
  // identity inside the server it came from.
  const projectId = useRoomProjects((projects) => {
    return projects.find((p) => p.path === projectPath)?.id ?? null
  })

  // Capability gate (#683 / PRD §6). Subscribes so a host switch
  // (local↔remote) flips the path live. `local` always supports it.
  const daemonOwnsChat = useRoomSupports('daemon-pinned-chat')

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
      onDaemonSessionRemoved={onDaemonSessionRemoved}
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
  /** Daemon-owned path only — see AgentChatPaneProps. */
  onDaemonSessionRemoved?: () => void
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
  /** Slice 4 — the picked row's provider rides along so the daemon can
   *  persist workspace_sessions.harness with the id (the canonical
   *  session's agent may differ from the workspace default). */
  onSwitchSession: (newSessionId: string, provider: string) => void
  viewTab: SessionViewTab
  splitLeft: SplitPaneView
  splitRight: SplitPaneView
  onViewTabChange: (tab: SessionViewTab) => void
  onSplitLeft: (view: SplitPaneView) => void
  onSplitRight: (view: SplitPaneView) => void
  /** v1 harness from ensure or the launch command. Null when known but not v1. */
  chatProvider: string | null
  /** False until ensure/launch has named the program. Does not grey a fresh Codex. */
  harnessReady: boolean
  /** Live pinned harness, including non-v1 (pi, cursor, hermes). */
  pinnedProvider: string | null
  /** Opens the shared continue dialog. Does not switch sessions. */
  onContinueNewChat: (source: ContinueNewChatSource) => void
}

interface HistorySession {
  sessionId: string
  title: string
  timestamp: number
  messageCount: number
  /** Discovery provider id ("claude"/"cursor"/"gemini"/"pi"/"codex"/…). */
  provider: string
  customName?: string | null
}

function ChatHeader({
  displayName,
  projectPath,
  currentSessionId,
  onRefresh,
  refreshing,
  onSwitchSession,
  viewTab,
  splitLeft,
  splitRight,
  onViewTabChange,
  onSplitLeft,
  onSplitRight,
  chatProvider,
  harnessReady,
  pinnedProvider,
  onContinueNewChat,
}: ChatHeaderProps): React.JSX.Element {
  const room = useRoom()
  useEffect(() => {
    if (!harnessReady || chatProvider) return
    if (viewTab === 'chat') onViewTabChange('terminal')
    if (splitLeft === 'chat') onSplitLeft(DEFAULT_SPLIT_LEFT)
    if (splitRight === 'chat') onSplitRight(DEFAULT_SPLIT_RIGHT)
  }, [
    harnessReady,
    viewTab,
    splitLeft,
    splitRight,
    chatProvider,
    onViewTabChange,
    onSplitLeft,
    onSplitRight,
  ])
  const [historySessions, setHistorySessions] = useState<HistorySession[]>([])
  const [historyOpen, setHistoryOpen] = useState(false)
  const [historyEpoch, setHistoryEpoch] = useState(0)

  useEffect(() => onChatHistoryChanged(room.scope, () => {
    setHistoryEpoch((n) => n + 1)
  }), [room])

  // 0.37.12 — fetch chat history for the dropdown title + popover list.
  // Runs on mount, when the current session changes (title converges
  // after the first message), and when the popover opens.
  //
  // Slice 4 (agent-degeneralization) — EVERY provider's sessions are
  // listed (chat/list is the multi-provider aggregator), still scoped to
  // this workspace path. Any of them can become the canonical session;
  // rows missing a provider (older daemons) degrade to "claude".
  useEffect(() => {
    let cancelled = false
    void Promise.all([
      daemonCliGet<Array<{
        sessionId: string
        title: string
        timestamp: number
        messageCount: number
        provider?: string
        archived?: boolean
        customName?: string | null
      }>>(room.scope, 'chat/list', { project_path: projectPath }),
      daemonCliGet<Record<string, string>>(room.scope, 'chat/custom-names').catch(() => ({}) as Record<string, string>),
    ])
      .then(([rows, names]) => {
        if (cancelled) return
        const nameMap = names ?? {}
        // Resume picker: hide user-archived sessions (restore first).
        const sorted = rows
          .filter((r) => !r.archived)
          .map((r) => {
            const provider = r.provider || 'claude'
            return {
              ...r,
              provider,
              customName: r.customName || nameMap[`${provider}:${r.sessionId}`] || null,
            }
          })
          .sort((a, b) => b.timestamp - a.timestamp)
        setHistorySessions(sorted)
      })
      .catch((err) => {
        console.warn('[AgentChatPane] chat/list failed:', err)
      })
    return () => { cancelled = true }
  }, [projectPath, currentSessionId, historyOpen, historyEpoch])

  const currentSession = useMemo(
    () =>
      currentSessionId
        ? historySessions.find((s) => s.sessionId === currentSessionId)
        : undefined,
    [historySessions, currentSessionId],
  )
  const currentChatTitle = currentSession
    ? chatDisplayName(currentSession) || 'New chat'
    : 'New chat'
  const canContinue = currentSessionId != null && currentSessionId !== ''

  return (
    <div
      className="border-b border-[var(--color-border)] flex-shrink-0 relative"
      data-testid="pinned-chat-header"
    >
    <div className="px-3 grid grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-stretch">
      <div className="flex min-w-0 items-stretch">
      <SessionViewMenu
        value={viewTab}
        splitLeft={splitLeft}
        splitRight={splitRight}
        chatEligible={Boolean(chatProvider)}
        onChange={onViewTabChange}
        onSplitLeft={onSplitLeft}
        onSplitRight={onSplitRight}
      />
      </div>
      <span className="pt-2 pb-[7px] text-xs font-semibold text-[var(--color-text-primary)] truncate min-w-0 flex items-center justify-center text-center">
        {displayName}
      </span>
      <div className="flex min-w-0 items-center justify-end gap-1">
      {/* 0.37.12 — chat-history dropdown. Lets the user switch the
          pinned chat to a different past session (escape hatch for
          orphaned/deleted sessions or just to revisit). */}
      <button
        type="button"
        onClick={() => setHistoryOpen((v) => !v)}
        title="Switch pinned chat to a different past session (any agent)"
        aria-label="Switch pinned chat session"
        aria-haspopup="listbox"
        aria-expanded={historyOpen}
        className="self-center inline-flex items-center gap-1 px-2 py-0.5 rounded text-[10px] font-medium text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] transition-colors no-drag cursor-pointer min-w-0"
      >
        {currentSession && (
          <ProviderIcon provider={currentSession.provider} size={12} />
        )}
        <span className="truncate max-w-[28ch]">{currentChatTitle}</span>
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className="flex-shrink-0">
          <path d={historyOpen ? 'M18 15l-6-6-6 6' : 'M6 9l6 6 6-6'} />
        </svg>
      </button>
      {historyOpen && (
        <div className="absolute right-12 top-full mt-1 z-20 w-[36ch] bg-[var(--color-bg)] border border-[var(--color-border)] shadow-2xl">
          <button
            type="button"
            data-testid="pinned-continue-new-chat"
            disabled={!canContinue}
            title={canContinue ? undefined : 'No chat to continue.'}
            onClick={() => {
              if (!canContinue || !currentSessionId) return
              setHistoryOpen(false)
              const provider = currentSession?.provider || pinnedProvider || 'claude'
              const displayName = currentSession
                ? (chatDisplayName(currentSession) || 'New chat')
                : currentChatTitle
              onContinueNewChat({
                provider,
                sessionId: currentSessionId,
                projectPath,
                displayName,
              })
            }}
            className={`w-full text-left px-3 py-1.5 text-[11px] text-[var(--color-text-primary)] no-drag ${
              canContinue
                ? 'hover:bg-[var(--color-bg-hover)] cursor-pointer'
                : 'opacity-50 cursor-not-allowed'
            }`}
          >
            Continue in a new chat…
          </button>
          <div
            role="listbox"
            data-testid="pinned-session-list"
            className="max-h-[60vh] overflow-y-auto border-t border-[var(--color-border)] py-1"
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
                  key={`${s.provider}:${s.sessionId}`}
                  type="button"
                  role="option"
                  aria-selected={isCurrent}
                  onClick={() => {
                    setHistoryOpen(false)
                    onSwitchSession(s.sessionId, s.provider)
                  }}
                  className={`w-full text-left px-3 py-1.5 text-[11px] flex items-center gap-2 transition-colors no-drag cursor-pointer ${
                    isCurrent
                      ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                      : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]'
                  }`}
                >
                  {/* Slice 4 — provider mark so mixed-agent lists scan. */}
                  <ProviderIcon provider={s.provider} size={12} />
                  <span className="flex-1 truncate">{chatDisplayName(s) || 'Untitled chat'}</span>
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
        </div>
      )}
      <button
        type="button"
        onClick={onRefresh}
        disabled={refreshing}
        title="Restart chat session — kills the current agent process and spawns a fresh resume. Use after typing `exit` or when the session is unresponsive."
        aria-label="Refresh chat session"
        className="self-center inline-flex items-center justify-center h-5 w-5 rounded text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50 disabled:cursor-not-allowed transition-colors flex-shrink-0"
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
    </div>
    </div>
  )
}

// Shared hook: resolve the AGENT.md `display_name:` (falls back to the
// technical agent name). Used by both implementations.
function useDisplayName(projectPath: string, agentName: string): string {
  const room = useRoom()
  const [displayName, setDisplayName] = useState<string>(agentName)
  useEffect(() => {
    let cancelled = false
    agentDisplayName(room.scope, projectPath)
      .then((n) => { if (!cancelled && n) setDisplayName(n) })
      .catch(() => { /* keep agentName as fallback */ })
    return () => { cancelled = true }
  }, [projectPath, agentName])
  useEffect(() => {
    // `sync:projects` is THIS computer's daemon (MS14): only a room that may
    // use local commands hears it.
    if (!room.localCommands) return
    let unlisten: (() => void) | null = null
    let cancelled = false
    listen('sync:projects', () => {
      agentDisplayName(room.scope, projectPath)
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
//     cols, rows, reused, provider?, pendingSessionDiscovery? }
interface EnsurePinnedChatResponse {
  sessionId: string
  /** The canonical conversation id. WIRE KEY keeps the historical
   *  `claudeSessionId` name (frozen daemon contract) even though the
   *  session may belong to ANY harness since Slice 3 — the renderer
   *  maps it to its provider-neutral `canonicalSessionId` state at
   *  this boundary and never leaks the claude-flavored name further. */
  claudeSessionId: string
  resumedExisting: boolean
  command: string
  args: string[]
  cols: number
  rows: number
  reused: boolean
  /** Slice 3 (additive) — harness that owns the ensured session. */
  provider?: string
  /** Slice 3 (additive) — self-minting fresh spawn; id adopted post-hoc. */
  pendingSessionDiscovery?: boolean
  /** Fresh continue. Absent on older daemons, which must not be treated as a spawn. */
  freshSpawn?: boolean
}

const FRESH_ARGV_REJECT = new Set(['--resume', '--fork-session', 'resume'])

function freshArgvProblem(args: string[] | undefined, sourceId: string): string | null {
  for (const arg of args ?? []) {
    if (FRESH_ARGV_REJECT.has(arg) || (sourceId.length > 0 && arg === sourceId)) {
      return 'The server resumed the current chat instead of starting a new one.'
    }
  }
  return null
}

async function watchFreshProviderId(opts: {
  scope: ServerScope
  projectPath: string
  sourceId: string
  targetProvider: string
  known: Set<string>
  cancelled: () => boolean
}): Promise<string | null> {
  const deadline = Date.now() + 20_000
  while (!opts.cancelled() && Date.now() < deadline) {
    try {
      const rows = await daemonCliGet<Array<{ sessionId?: string; provider?: string }>>(opts.scope,
        'chat/list',
        { project_path: opts.projectPath },
      )
      const hit = (rows ?? []).find((row) => {
        const id = row.sessionId?.trim() ?? ''
        const provider = row.provider || 'claude'
        return id.length > 0
          && id !== opts.sourceId
          && !opts.known.has(id)
          && provider === opts.targetProvider
      })
      if (hit?.sessionId) return hit.sessionId
    } catch {
      /* keep waiting for the new transcript */
    }
    await new Promise((resolve) => setTimeout(resolve, 400))
  }
  return null
}

async function ensurePinnedChat(
  scope: ServerScope,
  projectPath: string,
  opts?: { forceRespawn?: boolean; restoredSessionId?: string; explicitSelection?: boolean; freshProvider?: string },
): Promise<EnsurePinnedChatResponse> {
  // host-aware POST (daemonCliPost). The daemon is authoritative;
  // restoredSessionId rides along ONLY as an offline hint (PRD §4.5 / D3).
  // explicitSelection (Issue B) is set ONLY on a dropdown session switch:
  // it tells the daemon resolver to honor the just-persisted session_id
  // directly and skip the GH#24 converge fallback (no silent revert).
  // freshProvider is its own mode: no forceRespawn and no explicitSelection,
  // so an older daemon that ignores the field reuses the live PTY instead
  // of resuming the source.
  if (opts?.freshProvider) {
    return daemonCliPost<EnsurePinnedChatResponse>(scope, 'workspace/ensure-pinned-chat', {
      project: projectPath,
      freshProvider: opts.freshProvider,
    })
  }
  return daemonCliPost<EnsurePinnedChatResponse>(scope, 'workspace/ensure-pinned-chat', {
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
function AgentChatTerminalDaemon({ agentName, projectId, projectPath, restoredSessionId, onDaemonSessionRemoved }: AgentChatTerminalProps): React.JSX.Element {
  const room = useRoom()
  const containerRef = useRef<HTMLDivElement>(null)
  const terminalIdRef = useRef(agentChatId(projectId, agentName))
  const displayName = useDisplayName(projectPath, agentName)

  // P1.A — bind this pinned-Chat pane to ITS OWN project upfront (see the
  // legacy body for the full rationale). Idempotent.
  useEffect(() => {
    room.activity.bindPaneProject(terminalIdRef.current, projectId)
  }, [projectId])

  // Resolve phase. `ensuring` → first ensure in flight; `ready` → the
  // daemon confirmed a live session and we render TerminalPane; `idle` →
  // the daemon reported the session removed (e.g. user typed `exit`) — we
  // show the idle pane with a Retry button and DO NOT auto-respawn;
  // `error` → ensure failed.
  type Phase =
    | { kind: 'ensuring' }
    | { kind: 'ready'; sessionId: string; canonicalSessionId: string }
    | { kind: 'idle' }
    | { kind: 'error'; message: string }
  const [phase, setPhase] = useState<Phase>({ kind: 'ensuring' })
  const phaseRef = useRef(phase)
  phaseRef.current = phase
  const [refreshing, setRefreshing] = useState(false)
  // Bumped to force TerminalPane to re-attach (remount) after a daemon
  // respawn (refresh / dropdown switch / SessionAdded). The daemon kills
  // the old PTY + spawns a new one under the same key; a fresh TerminalPane
  // re-runs its idempotent spawn POST and binds to the NEW PTY.
  const [attachNonce, setAttachNonce] = useState(0)

  // Pinned-chat background retention — hold the grid-WS while this pane
  // is hidden IFF the workspace is in the canonical Active section
  // (daemon mirror, #672). Ties the exemption to the same membership the
  // daemon's reaper spares: while Active the canonical session is
  // effectively immortal, so a retained hidden attachment can never pin
  // a session past its Active window (there is no attach gate — see
  // active_reaper.rs). Leaving Active flips this false live and the
  // pane parks exactly as today. On a daemon without `canonical-active`
  // the mirror is empty ⇒ false ⇒ today's behavior.
  //
  // Attach-size PR3 audit: this is the retain-while-hidden path that
  // stops Active-section tab flips from full remount → re-toy-spawn.
  // Covered by AgentChatPane.test "pinned-chat retention — retainWhileHidden
  // threading". Do not gate or drop this prop without replacing that test.
  // Home M4 (MS14): the ROOM's server's Active set — B's project id is
  // never looked up in A's set.
  const retainWhileHidden = useStore(room.activeSet, (s) => s.activeProjectIds.has(projectId))

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
  // While a fresh continue is in flight, SessionAdded's session_id is the
  // daemon PTY uuid. Do not store it as the dropdown's provider id.
  const freshHoldProviderIdRef = useRef<string | null>(null)
  const discoveryGenRef = useRef(0)
  const [liveProvider, setLiveProvider] = useState<string | null>(null)
  const [continueSource, setContinueSource] = useState<ContinueNewChatSource | null>(null)

  const lastOverlayConvRef = useRef<string | null>(restoredSessionId ?? null)
  const overlayConv =
    phase.kind === 'ready' ? phase.canonicalSessionId : lastOverlayConvRef.current
  if (phase.kind === 'ready' && phase.canonicalSessionId) {
    lastOverlayConvRef.current = phase.canonicalSessionId
  }
  const {
    viewTab,
    setViewTab,
    splitLeft,
    splitRight,
    setSplitLeft,
    setSplitRight,
  } = useSessionViewTab(overlayConv)
  const [chatProvider, setChatProvider] = useState<string | null>(null)
  const [harnessReady, setHarnessReady] = useState(false)
  const [chatConversationId, setChatConversationId] = useState<string | null>(null)
  const [overlayAddr, setOverlayAddr] = useState('')
  useEffect(() => {
    let cancelled = false
    resolvePinnedChatCopyableAddress(room.scope, projectPath, projectId)
      .then((a) => {
        if (!cancelled && a?.clipboard) setOverlayAddr(a.clipboard)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [projectPath, projectId])

  // Latest onDaemonSessionRemoved in a ref so the session-events
  // subscription (deps: projectPath/projectId) fires the CURRENT
  // consumer without re-subscribing per render — the retainer passes an
  // inline closure. Same pattern as TerminalPane's onChildExitRef.
  const onDaemonSessionRemovedRef = useRef(onDaemonSessionRemoved)
  useEffect(() => {
    onDaemonSessionRemovedRef.current = onDaemonSessionRemoved
  }, [onDaemonSessionRemoved])

  // The canonical conversation id currently pinned (from the last ensure
  // / the SessionAdded event) — any harness's session since Slice 4.
  // Drives the dropdown highlight + title lookup.
  const canonicalSessionId =
    phase.kind === 'ready' ? phase.canonicalSessionId : null

  // Codex and Gemini mint the provider id after the first turn. Do not
  // open the transcript socket until that id exists. This poll reads the
  // list row; it does not spawn, close, or tail a file.
  const chatFaceOn =
    viewTab === 'chat' ||
    (viewTab === 'split' && (splitLeft === 'chat' || splitRight === 'chat'))
  useEffect(() => {
    if (!chatFaceOn || !harnessReady || !chatProvider || chatConversationId) return
    let cancelled = false
    const tick = async (): Promise<void> => {
      try {
        const rows = await daemonCliGet<Array<{
          agentName?: string
          sessionId?: string
          conversationId?: string
        }>>(room.scope, 'sessions/list-for-workspace', { path: projectPath })
        if (cancelled || !Array.isArray(rows)) return
        const row = rows.find((item) => item.agentName === projectId)
        const cid = row?.conversationId?.trim()
        if (cid && cid !== row?.sessionId) setChatConversationId(cid)
      } catch {
        /* keep waiting */
      }
    }
    void tick()
    const timer = setInterval(() => void tick(), 1000)
    return () => {
      cancelled = true
      clearInterval(timer)
    }
  }, [chatFaceOn, harnessReady, chatProvider, chatConversationId, projectPath, projectId])

  // ── ensure() — the one daemon RPC this component leans on ────────────
  // Idempotent on the daemon side. `forceRespawn` kills + respawns. The
  // SessionAdded broadcast re-attaches; but we also adopt the ensure
  // RESPONSE directly so a cold mount renders without waiting for the WS.
  const ensure = useCallback(
    async (forceRespawn: boolean, explicitSelection = false): Promise<void> => {
      // Home M4: a view-only room never find-or-spawns the pinned chat on
      // its server. It looks the live canonical session up and attaches
      // (TerminalPane posts attach_only); none live ⇒ the idle state.
      if (room.readOnly) {
        try {
          const rows = await daemonCliGet<Array<{
            agentName?: string
            sessionId?: string
            conversationId?: string
            command?: string | null
          }>>(room.scope, 'sessions/list-for-workspace', { path: projectPath })
          const row = Array.isArray(rows) ? rows.find((r) => r.agentName === projectId) : undefined
          if (!row?.sessionId) {
            setPhase({ kind: 'idle' })
            return
          }
          const cid = row.conversationId?.trim() || null
          setPhase({ kind: 'ready', sessionId: row.sessionId, canonicalSessionId: cid ?? '' })
          setChatProvider(chatHarnessName({ provider: undefined, command: row.command ?? undefined }))
          setHarnessReady(true)
          setChatConversationId(cid && cid !== row.sessionId ? cid : null)
          attachedSessionIdRef.current = row.sessionId
        } catch (err) {
          setPhase({ kind: 'error', message: err instanceof Error ? err.message : String(err) })
        }
        return
      }
      try {
        const res = await ensurePinnedChat(room.scope, projectPath, {
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
          // Boundary map: the wire key stays `claudeSessionId` (frozen
          // daemon contract) but the state is provider-neutral.
          canonicalSessionId: res.claudeSessionId,
        })
        setChatProvider(chatHarnessName({ provider: res.provider, command: res.command }))
        setHarnessReady(true)
        const ensuredProvider = res.provider || providerKeyForCommand(res.command)
        if (ensuredProvider) setLiveProvider(ensuredProvider)
        const providerId = res.claudeSessionId?.trim() ?? ''
        setChatConversationId(providerId || null)
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

  // Visit → ensure (find-or-spawn; daemon resolves resume vs fresh).
  // Mounting this pane is a visit or other need — seedBoot / Active-list
  // warming must NOT mount it (retainer is display-only until visit).
  // Cold boot / relaunch-revive of a visited pane is just this call:
  // the daemon reads workspace_sessions canonically (#679 preserved).
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
    const unsubscribe = subscribeToWorkspaceSessionEvents(room.scope, projectPath, {
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
        const held = freshHoldProviderIdRef.current
        setPhase({
          kind: 'ready',
          sessionId: event.session_id,
          canonicalSessionId: held ?? event.session_id,
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
        // Pinned-chat retention — let the retainer evict a backgrounded
        // instance (fires AFTER the idle flip so a kept-mounted pane —
        // foreground workspace — already shows the idle state).
        onDaemonSessionRemovedRef.current?.()
      },
    })
    return unsubscribe
  }, [projectPath, projectId])

  // Refresh → forceRespawn. The daemon kills + respawns; we re-attach via
  // the ensure response AND the SessionAdded broadcast.
  const handleRefresh = useCallback(async (): Promise<void> => {
    if (refreshing) return
    freshHoldProviderIdRef.current = null
    discoveryGenRef.current += 1
    setRefreshing(true)
    setPhase({ kind: 'ensuring' })
    await ensure(true)
    setRefreshing(false)
  }, [refreshing, ensure])

  // Dropdown switch → persist via set-chat-session, then forceRespawn so
  // the daemon respawns on the newly-selected session. Slice 4: the
  // picked row's PROVIDER rides along so the daemon persists
  // workspace_sessions.harness with the id — the respawn's
  // ensure-pinned-chat then resolves THAT harness's command+grammar
  // (a pi pick respawns `pi --session <id>`, not claude).
  const handleSwitchSession = useCallback(
    async (newSessionId: string, provider: string): Promise<void> => {
      if (!newSessionId || newSessionId === canonicalSessionId) return
      freshHoldProviderIdRef.current = null
      discoveryGenRef.current += 1
      setRefreshing(true)
      setPhase({ kind: 'ensuring' })
      try {
        // HOST-AWARE persist of the pinned session (same client the legacy
        // path uses; the route reads query params).
        await setChatSession(room.scope, projectPath, newSessionId, provider)
      } catch (err) {
        console.error('[AgentChatPane] switchToSession DB update failed:', err)
        setRefreshing(false)
        // Re-ensure so we don't strand the pane in `ensuring`.
        void ensure(false)
        return
      }
      // Match the legacy path: stamp layout so offline restore matches the
      // dropdown pick across refresh / relaunch (not only after ensure).
      try {
        room.tabs.getState().stampAgentSessionId(agentName, projectPath, newSessionId, projectId)
      } catch (err) {
        console.warn('[AgentChatPane] stampAgentSessionId failed:', err)
      }
      // Issue B — explicitSelection:true so the daemon honors the picked
      // session and never silently reverts to the newest on-disk session.
      // ensure() sets phase=error if the chosen session is gone on disk
      // (the daemon returns an Err) rather than swapping to a different one.
      await ensure(true, true)
      setRefreshing(false)
    },
    [canonicalSessionId, projectPath, ensure, agentName, projectId],
  )

  const runPinnedContinue = useCallback(async (
    source: ContinueNewChatSource,
    req: ContinueSpawnRequest,
    stillOpen: () => boolean,
  ): Promise<void> => {
    if (!stillOpen()) return
    const previous = phaseRef.current
    const myGen = ++discoveryGenRef.current
    freshHoldProviderIdRef.current = source.sessionId
    const known = new Set<string>([source.sessionId])
    try {
      const rows = await daemonCliGet<Array<{ sessionId?: string }>>(room.scope, 'chat/list', {
        project_path: projectPath,
      })
      for (const row of rows ?? []) {
        if (row.sessionId) known.add(row.sessionId)
      }
    } catch {
      /* source stays excluded */
    }
    if (!stillOpen() || myGen !== discoveryGenRef.current) {
      freshHoldProviderIdRef.current = null
      return
    }
    setRefreshing(true)
    setPhase({ kind: 'ensuring' })
    let spawned = false
    try {
      const res = await ensurePinnedChat(room.scope, projectPath, { freshProvider: req.targetProvider })
      if (!stillOpen() || myGen !== discoveryGenRef.current) {
        freshHoldProviderIdRef.current = null
        setPhase(previous.kind === 'ensuring'
          ? { kind: 'error', message: 'The chat could not be continued.' }
          : previous)
        return
      }
      if (res.freshSpawn !== true) {
        throw new Error('This server cannot continue a pinned chat in place.')
      }
      const argvError = freshArgvProblem(res.args, source.sessionId)
      if (argvError) throw new Error(argvError)
      const premint = res.claudeSessionId?.trim() ?? ''
      if (premint && premint === source.sessionId) {
        throw new Error('The server resumed the current chat instead of starting a new one.')
      }
      spawned = true
      attachedSessionIdRef.current = res.sessionId
      setPhase({
        kind: 'ready',
        sessionId: res.sessionId,
        canonicalSessionId: source.sessionId,
      })
      setChatProvider(chatHarnessName({ provider: res.provider, command: res.command }))
      setHarnessReady(true)
      const nextProvider = res.provider || providerKeyForCommand(res.command)
      if (nextProvider) setLiveProvider(nextProvider)
      setChatConversationId(null)
      setAttachNonce((n) => n + 1)
      if (!stillOpen()) return
      const msg = await daemonCliPost<MsgResponse>(room.scope, 'terminal/send-message', {
        session_id: res.sessionId,
        text: req.text,
      })
      if (!stillOpen() || myGen !== discoveryGenRef.current) return
      const status = mapMsgResponseToStatus(msg)
      if (status.kind !== 'delivered') {
        throw new Error(continueSendError(status))
      }
      const stamp = (id: string): void => {
        try {
          room.tabs.getState().stampAgentSessionId(agentName, projectPath, id, projectId)
        } catch (err) {
          console.warn('[AgentChatPane] stampAgentSessionId failed:', err)
        }
      }
      if (!res.pendingSessionDiscovery && premint) {
        await setChatSession(room.scope, projectPath, premint, req.targetProvider)
        stamp(premint)
        freshHoldProviderIdRef.current = null
        setPhase({
          kind: 'ready',
          sessionId: res.sessionId,
          canonicalSessionId: premint,
        })
        setChatConversationId(premint)
        setLiveProvider(req.targetProvider)
        return
      }
      void watchFreshProviderId({
        scope: room.scope,
        projectPath,
        sourceId: source.sessionId,
        targetProvider: req.targetProvider,
        known,
        cancelled: () => myGen !== discoveryGenRef.current,
      }).then((id) => {
        if (!id || myGen !== discoveryGenRef.current) return
        freshHoldProviderIdRef.current = null
        void setChatSession(room.scope, projectPath, id, req.targetProvider).then(() => {
          stamp(id)
          setPhase((prev) => (
            prev.kind === 'ready' ? { ...prev, canonicalSessionId: id } : prev
          ))
          setChatConversationId(id)
          setLiveProvider(req.targetProvider)
        })
      })
    } catch (err) {
      if (!spawned) {
        freshHoldProviderIdRef.current = null
        setPhase(previous.kind === 'ensuring'
          ? { kind: 'error', message: 'The chat could not be continued.' }
          : previous)
      }
      if (!stillOpen()) return
      throw err instanceof Error ? err : new Error(String(err))
    } finally {
      setRefreshing(false)
    }
  }, [agentName, projectId, projectPath])

  // The shared header (agent name + session dropdown + refresh) is rendered
  // in EVERY non-trivial phase — including the failure states — so a session
  // that won't load never traps the user: the dropdown stays visible and
  // clickable, so they can switch to a different past session. The failure
  // message + Retry render in the content area BELOW the header, not over it.
  const header = (
    <ChatHeader
      displayName={displayName}
      projectPath={projectPath}
      currentSessionId={canonicalSessionId}
      onRefresh={() => void handleRefresh()}
      refreshing={refreshing}
      onSwitchSession={(sid, provider) => void handleSwitchSession(sid, provider)}
      viewTab={viewTab}
      splitLeft={splitLeft}
      splitRight={splitRight}
      onViewTabChange={setViewTab}
      onSplitLeft={setSplitLeft}
      onSplitRight={setSplitRight}
      chatProvider={chatProvider}
      harnessReady={harnessReady}
      pinnedProvider={liveProvider}
      onContinueNewChat={setContinueSource}
    />
  )

  const openContinue = continueSource
  const continueDialog = openContinue ? (
    <ContinueNewChatDialog
      source={openContinue}
      onClose={() => setContinueSource(null)}
      onSpawn={(req, stillOpen) => runPinnedContinue(openContinue, req, stillOpen)}
    />
  ) : null

  if (phase.kind === 'error') {
    return (
      <>
      {continueDialog}
      <div ref={containerRef} className="h-full flex flex-col bg-[var(--color-bg)] overflow-hidden">
        {header}
        <OverlayTabBody viewTab={viewTab} overlayAddr={overlayAddr} overlayConv={overlayConv}>
        <div className="flex-1 min-h-0 flex flex-col items-center justify-center gap-3 px-6 text-center">
          <div className="text-xs font-semibold text-[var(--color-text-primary)]">
            Chat session failed to start
          </div>
          <div className="text-[11px] text-[var(--color-text-muted)] max-w-[40ch]">
            {phase.message || 'The daemon could not start the chat session.'}
          </div>
          <RetryButton onClick={handleRefresh} refreshing={refreshing} />
        </div>
        </OverlayTabBody>
      </div>
      </>
    )
  }

  if (phase.kind === 'idle') {
    return (
      <>
      {continueDialog}
      <div ref={containerRef} className="h-full flex flex-col bg-[var(--color-bg)] overflow-hidden">
        {header}
        <OverlayTabBody viewTab={viewTab} overlayAddr={overlayAddr} overlayConv={overlayConv}>
        <div className="flex-1 min-h-0 flex flex-col items-center justify-center gap-3 px-6 text-center">
          <div className="text-xs font-semibold text-[var(--color-text-primary)]">
            {room.readOnly ? `Not running on ${room.scope.label}` : 'Chat session ended'}
          </div>
          <div className="text-[11px] text-[var(--color-text-muted)] max-w-[40ch]">
            {room.readOnly
              ? 'View only (preview): this room does not start the chat. Retry looks again.'
              : 'The chat process exited. Click Retry to start a fresh session.'}
          </div>
          <RetryButton onClick={room.readOnly ? () => void ensure(false) : handleRefresh} refreshing={refreshing} />
        </div>
        </OverlayTabBody>
      </div>
      </>
    )
  }

  if (phase.kind === 'ensuring') {
    return (
      <>
      {continueDialog}
      <div className="flex items-center justify-center h-full text-xs text-[var(--color-text-muted)]">
        Loading session…
      </div>
      </>
    )
  }

  // phase.kind === 'ready'
  return (
    <>
    {continueDialog}
    <div ref={containerRef} className="h-full flex flex-col bg-[var(--color-bg)] overflow-hidden">
      {header}
      <PinnedSessionBody
        viewTab={viewTab}
        splitLeft={splitLeft}
        splitRight={splitRight}
        addr={overlayAddr}
        conversationId={overlayConv}
        chatConversationId={chatConversationId}
        chatProvider={chatProvider}
        agentName={projectId}
      >
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
          // R6/R18: pass ensure's session id as an eager marker. Live
          // knownSessionId is NOT attach-only (POST still command:null);
          // daemon R4+R5 remain the product. Defense-in-depth only.
          sessionId={phase.sessionId}
          seedLabel={displayName}
          lockLabel={true}
          showComposeBar
          // Retention exemption (daemon-owned path ONLY — the legacy
          // fallback keeps park-on-hidden, per the capability gate).
          retainWhileHidden={retainWhileHidden}
          // 0.39.39: NO onChildExit breaker wiring on the daemon-owned
          // path. Exit is observed by the DAEMON, which broadcasts
          // SessionRemoved → the subscription above flips us to idle. No
          // renderer-side breaker because there's no renderer respawn loop
          // to bound.
        />
      </PinnedSessionBody>
    </div>
    </>
  )
}

function OverlayTabBody({
  viewTab,
  overlayAddr,
  overlayConv,
  children,
}: {
  viewTab: SessionViewTab
  overlayAddr: string
  overlayConv: string | null
  children: React.ReactNode
}): React.JSX.Element {
  if (viewTab === 'thread') {
    return (
      <div className="flex-1 min-h-0 flex flex-col overflow-hidden">
        <ThreadOverlayPane addr={overlayAddr} conversationId={overlayConv} />
      </div>
    )
  }
  if (viewTab === 'chatter') {
    return (
      <div className="flex-1 min-h-0">
        <ChatterOverlayPane addr={overlayAddr} conversationId={overlayConv} />
      </div>
    )
  }
  return <>{children}</>
}

// Small shared retry button used by the daemon-owned error/idle panes.
function RetryButton({ onClick, refreshing }: { onClick: () => void; refreshing: boolean }): React.JSX.Element {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={refreshing}
      className="inline-flex items-center gap-1.5 px-3 py-1 rounded text-[11px] font-medium bg-transparent text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/20 disabled:opacity-50 disabled:cursor-not-allowed transition-colors no-drag cursor-pointer"
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
  const room = useRoom()
  const containerRef = useRef<HTMLDivElement>(null)
  const terminalIdRef = useRef(agentChatId(projectId, agentName))

  useEffect(() => {
    room.activity.bindPaneProject(terminalIdRef.current, projectId)
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

  // 0.37.12 — chat-history dropdown is now owned by the shared <ChatHeader>
  // (fetch + popover state + title). The legacy body only needs the CURRENT
  // session id to highlight + to compute the no-op short-circuit on switch.
  //
  // The session id currently driving the live PTY — derived from
  // launchConfig.args. Slice 4: the daemon's cold-boot args may speak any
  // provider's grammar, so scan every session-selection shape: flag-style
  // `--resume/--session-id <X>` (claude/grok/cursor/gemini), `--session <X>`
  // (pi), and the codex `resume <X>` subcommand (first token only).
  const currentSessionId = useMemo<string | null>(() => {
    const args = launchConfig?.args
    if (!args) return null
    if (args[0] === 'resume' && args[1]) return args[1]
    for (let i = 0; i + 1 < args.length; i++) {
      if (args[i] === '--resume' || args[i] === '--session-id' || args[i] === '--session') {
        return args[i + 1]
      }
    }
    return null
  }, [launchConfig])

  const lastOverlayConvRef = useRef<string | null>(restoredSessionId ?? null)
  if (currentSessionId) lastOverlayConvRef.current = currentSessionId
  const overlayConv = currentSessionId ?? lastOverlayConvRef.current
  const {
    viewTab,
    setViewTab,
    splitLeft,
    splitRight,
    setSplitLeft,
    setSplitRight,
  } = useSessionViewTab(overlayConv)
  const [overlayAddr, setOverlayAddr] = useState('')
  useEffect(() => {
    let cancelled = false
    resolvePinnedChatCopyableAddress(room.scope, projectPath, projectId)
      .then((a) => {
        if (!cancelled && a?.clipboard) setOverlayAddr(a.clipboard)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [projectPath, projectId])

  // Bumped on every refresh-button click to force a clean remount of
  // TerminalPane (key={refreshNonce}) and a re-run of the resolve effect.
  const [refreshNonce, setRefreshNonce] = useState(0)
  const [refreshing, setRefreshing] = useState(false)
  const [continueSource, setContinueSource] = useState<ContinueNewChatSource | null>(null)
  const runLegacyContinue = useCallback(async (
    _req: ContinueSpawnRequest,
    _stillOpen: () => boolean,
  ): Promise<void> => {
    // No fresh mode on this daemon. Do not refresh and do not resume.
    throw new Error('This server cannot continue a pinned chat in place.')
  }, [])

  // Listen for chat:refreshed broadcasts (cross-window remount).
  useEffect(() => {
    // `chat:refreshed` is a Tauri broadcast between THIS computer's windows.
    if (!room.localCommands) return
    let cancelled = false
    let unlisten: (() => void) | null = null
    listen<{ projectPath: string }>('chat:refreshed', (event) => {
      if (event.payload.projectPath !== projectPath) return
      setLaunchConfig(null)
      setReady(false)
      setRefreshNonce((n) => n + 1)
    }).then((u) => { if (cancelled) u(); else unlisten = u })
    return () => { cancelled = true; unlisten?.() }
  }, [room, projectPath])

  const handleRefresh = useCallback(async (): Promise<void> => {
    if (refreshing) return
    setRefreshing(true)
    // K2 #682 — reset the breaker + self-retrigger memo for an explicit
    // "try again".
    breakerRef.current = resetBreaker()
    setBreakerTripped(false)
    lastResolvedRef.current = null
    // Kill the daemon-owned PTY (best-effort) on THIS room's server (MS75).
    // closeV2Session never throws; refresh proceeds either way.
    await closeV2Session(room.scope, projectId)

    // MS67 — the cross-window remount broadcast is a local Tauri command.
    if (room.localCommands) {
      invoke('k2so_chat_refresh_broadcast', { projectPath })
        .catch((e) => console.warn('[chat-refresh] broadcast failed:', e))
    }

    setLaunchConfig(null)
    setReady(false)
    setRefreshing(false)
  }, [room, projectId, projectPath, agentName, refreshing])

  // Switch the pinned chat tab to a different past session. <ChatHeader>
  // closes its own popover before invoking this, so no dropdown state here.
  // Slice 4: the picked row's provider is persisted with the id so the
  // refresh's daemon resolve speaks that harness's grammar.
  const switchToSession = useCallback(async (newSessionId: string, provider: string): Promise<void> => {
    if (!newSessionId || newSessionId === currentSessionId) {
      return
    }
    try {
      await setChatSession(room.scope, projectPath, newSessionId, provider)
    } catch (err) {
      console.error('[AgentChatPane] switchToSession DB update failed:', err)
      return
    }
    try {
      room.tabs.getState().stampAgentSessionId(agentName, projectPath, newSessionId, projectId)
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
        room.tabs.getState().stampAgentSessionId(agentName, projectPath, sid, projectId)
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
          const canonical = await resumeChatArgs(room.scope, projectPath)
          decision = reconcileColdBootSession(restoredSessionId, canonical)
        } catch (err) {
          console.warn('[AgentChatPane] cold-boot SQLite reconcile failed, using layout hint:', err)
        }
        if (cancelled) return

        if (decision.kind === 'fallback') {
          // Daemon unreachable — we can't know the stored harness, so
          // degrade to claude grammar on the layout hint (the only
          // honest offline guess; pre-Slice-4 behavior preserved).
          setLaunchConfig({
            command: 'claude',
            args: ['--dangerously-skip-permissions', '--resume', decision.sessionId],
            cwd: projectPath,
          })
        } else {
          // Slice 4 — TRUST the daemon's per-harness decision: its
          // command+args already speak the canonical session's own
          // provider grammar (resume AND fresh). No renderer-built
          // claude argv on this path anymore.
          if (decision.kind === 'fresh') {
            console.info(
              '[AgentChatPane] cold-boot fresh session (resumedExisting=false):',
              decision.sessionId,
              '— spawning the daemon\'s fresh args, NOT a resume',
            )
          } else if (decision.sessionId !== restoredSessionId) {
            console.info(
              '[AgentChatPane] cold-boot revive: SQLite session',
              decision.sessionId,
              'overrides layout hint',
              restoredSessionId,
            )
          }
          setLaunchConfig({
            command: decision.command,
            args: decision.args,
            cwd: projectPath,
          })
        }
        claimAgentLock(room.scope, {
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
        const exists = await terminalExists(room.scope, myTerminalId)
        if (!cancelled && exists) {
          setLaunchConfig(null)
          setReady(true)
          return
        }
      } catch { /* fall through */ }

      // Step 1b: Check the daemon for an existing session under this
      // workspace's canonical key. Informational only. MS67 — the Tauri
      // command asks THIS computer's daemon, so only a room that may use
      // local commands runs it.
      if (room.localCommands) try {
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
        const result = await resumeChatArgs(room.scope, projectPath)
        if (!cancelled && result) {
          setLaunchConfig({
            command: result.command,
            args: result.args,
            cwd: result.cwd,
          })
          claimAgentLock(room.scope, {
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

      // Step 3: Last-resort fallback — fresh session. The daemon read
      // failed, so the stored harness is unknowable; claude grammar is
      // the deliberate offline degradation (matches the cold-boot
      // `fallback` decision above).
      if (!cancelled) {
        setLaunchConfig({
          command: 'claude',
          args: ['--dangerously-skip-permissions'],
          cwd: projectPath,
        })
        claimAgentLock(room.scope, {
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

  // Shared header (agent name + session dropdown + refresh). Rendered in the
  // breaker-tripped failure state too — so a chat that keeps crashing on load
  // never traps the user: the dropdown stays visible + clickable, letting them
  // switch to a different past session. The failure message + Retry render in
  // the content area BELOW the header, not over it. (Uses the same ChatHeader
  // as the daemon-owned path; `switchToSession` is the legacy switch handler.)
  const header = (
    <ChatHeader
      displayName={displayName}
      projectPath={projectPath}
      currentSessionId={currentSessionId}
      onRefresh={() => void handleRefresh()}
      refreshing={refreshing}
      onSwitchSession={(sid, provider) => void switchToSession(sid, provider)}
      viewTab={viewTab}
      splitLeft={splitLeft}
      splitRight={splitRight}
      onViewTabChange={setViewTab}
      onSplitLeft={setSplitLeft}
      onSplitRight={setSplitRight}
      chatProvider={launchConfig ? chatHarnessName({ command: launchConfig.command }) : null}
      harnessReady={launchConfig != null}
      pinnedProvider={providerKeyForCommand(launchConfig?.command)}
      onContinueNewChat={setContinueSource}
    />
  )

  const continueDialog = continueSource ? (
    <ContinueNewChatDialog
      source={continueSource}
      onClose={() => setContinueSource(null)}
      onSpawn={runLegacyContinue}
    />
  ) : null

  if (breakerTripped) {
    return (
      <>
      {continueDialog}
      <div ref={containerRef} className="h-full flex flex-col bg-[var(--color-bg)] overflow-hidden">
        {header}
        <OverlayTabBody viewTab={viewTab} overlayAddr={overlayAddr} overlayConv={overlayConv}>
        <div className="flex-1 min-h-0 flex flex-col items-center justify-center gap-3 px-6 text-center">
          <div className="text-xs font-semibold text-[var(--color-text-primary)]">
            Chat session failed to start
          </div>
          <div className="text-[11px] text-[var(--color-text-muted)] max-w-[40ch]">
            The chat process exited repeatedly right after starting, so K2
            stopped retrying to avoid a spawn loop. Click Retry to try again.
          </div>
          <RetryButton onClick={() => void handleRefresh()} refreshing={refreshing} />
        </div>
        </OverlayTabBody>
      </div>
      </>
    )
  }

  if (!ready) {
    return (
      <>
      {continueDialog}
      <div className="flex items-center justify-center h-full text-xs text-[var(--color-text-muted)]">
        Loading session…
      </div>
      </>
    )
  }

  return (
    <>
    {continueDialog}
    <div ref={containerRef} className="h-full flex flex-col bg-[var(--color-bg)] overflow-hidden">
      {header}
      <PinnedSessionBody
        viewTab={viewTab}
        splitLeft={splitLeft}
        splitRight={splitRight}
        addr={overlayAddr}
        conversationId={overlayConv}
        chatConversationId={currentSessionId}
        chatProvider={launchConfig ? chatHarnessName({ command: launchConfig.command }) : null}
        agentName={projectId}
      >
        <TerminalPane
          key={refreshNonce}
          terminalId={terminalIdRef.current}
          cwd={launchConfig?.cwd ?? projectPath}
          command={launchConfig?.command}
          args={launchConfig?.args}
          attachAgentName={projectId}
          seedLabel={displayName}
          lockLabel={true}
          showComposeBar
          // K2 #682 — feed child-exit into the spawn-loop circuit breaker.
          // ONLY the fallback path wires this: the daemon-owned path relies
          // on the daemon's SessionRemoved broadcast for exit.
          onChildExit={handleChildExit}
        />
      </PinnedSessionBody>
    </div>
    </>
  )
}
