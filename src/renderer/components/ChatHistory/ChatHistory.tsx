import { useState, useEffect, useMemo, useCallback, useRef } from 'react'
import { onChatHistoryChanged } from '@/stores/session-events'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { useProjectsStore } from '@/stores/projects'
import { useTabsStore, openApiHostSessionTab, type TerminalItemData } from '@/stores/tabs'
import { useSettingsStore } from '@/stores/settings'
import { usePresetsStore } from '@/stores/presets'
import { resolveAgentCommand } from '@/lib/agent-resolve'
import { ProviderIcon } from '@/components/AgentIcon/ProviderIcon'
import { KeyCombo } from '@/components/KeySymbol'
import { resolveChatHistoryHost } from './resolveHost'
import {
  collectStoreTabs,
  chatDisplayName,
  findTabByPaneGroupId,
  restampSessionTabs,
} from '@/lib/chat-session-tab'
import { IconAutonomous } from '@/components/icons/IconAutonomous'
import { useHeartbeatSessionsStore } from '@/stores/heartbeat-sessions'
import { sessionIdsTargetedByHeartbeats } from '@/lib/heartbeat-delivery'

// ── Types ────────────────────────────────────────────────────────────

interface ChatSession {
  sessionId: string
  project: string
  title: string
  timestamp: number // unix ms
  provider: string
  messageCount: number
  originBranch: string | null
  archived?: boolean
  archivedAt?: number
  customName?: string | null
}

/** Date buckets inside the General collapsible (not top-level sections). */
type DateGroup = 'Today' | 'Yesterday' | 'This Week' | 'This Month' | 'Older'

// A sandbox chat = an API-triggered session that ran INSIDE a hardened cell in
// this workspace. Listed from the daemon's sandbox index; clicking re-launches
// it in its sandbox (audit-resume) — it surfaces as its orange tab.
interface SandboxChat {
  sessionId: string
  title: string
  timestamp: number
  messageCount: number
}

interface ApiChat {
  sessionId: string
  agentName: string
  live: boolean
  lastSeenAt: number
  fromApi?: boolean
}

// ── CLI tool config ─────────────────────────────────────────────────

// Per-provider resume contract. Either `resumeFlag` ("flag-style":
// `<command> <preset-args> <flag> <uuid>`) OR `resumeSubcommand`
// ("subcommand-style": `<command> <preset-args> <subcommand> <uuid>`).
// Codex is the only subcommand-style provider currently.
interface ProviderConfig {
  command: string
  label: string
  resumeFlag?: string
  resumeSubcommand?: string
}

const PROVIDER_CONFIG: Record<string, ProviderConfig> = {
  claude: { command: 'claude', label: 'Claude', resumeFlag: '--resume' },
  cursor: { command: 'cursor-agent', label: 'Cursor', resumeFlag: '--resume' },
  grok: { command: 'grok', label: 'Grok', resumeFlag: '--resume' },
  gemini: { command: 'gemini', label: 'Gemini', resumeFlag: '--resume' },
  pi: { command: 'pi', label: 'Pi', resumeFlag: '--session' },
  codex: { command: 'codex', label: 'Codex', resumeSubcommand: 'resume' },
  hermes: { command: 'hermes', label: 'Hermes', resumeFlag: '--resume' },
}

/// Get the preset args (e.g. --dangerously-skip-permissions) for a provider command.
/// Parses the user's agent preset to extract flags that should carry over to resumed sessions.
///
/// Strips any session-selection flag the preset may already carry (`--resume`,
/// `--continue`, `-c`, `-r`, `--session`) so it can't conflict with the
/// explicit `<resumeFlag> <sessionId>` we append. This matters most for Pi:
/// `--resume` opens an interactive picker, so leaving it in the preset would
/// shadow our `--session <uuid>` and trap the uuid as a chat message.
const SESSION_FLAGS_TO_STRIP = new Set(['--resume', '-r', '--continue', '-c', '--session'])

function getPresetArgsForProvider(provider: string): string[] {
  const config = PROVIDER_CONFIG[provider]
  if (!config) return []
  try {
    const presets = usePresetsStore.getState().presets
    const preset = presets.find((p) => p.command.split(/\s+/)[0] === config.command && p.enabled)
    if (preset) {
      // Parse the preset command to extract args after the command name
      const parts = preset.command.match(/(?:[^\s"']+|"[^"]*"|'[^']*')+/g) || []
      const cleaned = parts.map((p: string) => p.replace(/^["']|["']$/g, ''))
      return cleaned.slice(1).filter((a: string) => !SESSION_FLAGS_TO_STRIP.has(a))
    }
  } catch { /* preset store not available */ }
  return []
}

// ── Helpers ──────────────────────────────────────────────────────────

function classifyDate(timestamp: number): DateGroup {
  const now = new Date()
  const date = new Date(timestamp)

  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate())
  const startOfYesterday = new Date(startOfToday.getTime() - 86400000)
  const startOfWeek = new Date(startOfToday)
  startOfWeek.setDate(startOfToday.getDate() - startOfToday.getDay())
  const startOfMonth = new Date(now.getFullYear(), now.getMonth(), 1)

  if (date >= startOfToday) return 'Today'
  if (date >= startOfYesterday) return 'Yesterday'
  if (date >= startOfWeek) return 'This Week'
  if (date >= startOfMonth) return 'This Month'
  return 'Older'
}

function formatTime(timestamp: number): string {
  const date = new Date(timestamp)
  const group = classifyDate(timestamp)

  if (group === 'Today' || group === 'Yesterday') {
    return date.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
  }

  return date.toLocaleDateString([], { month: 'short', day: 'numeric' })
}

const DATE_GROUP_ORDER: DateGroup[] = ['Today', 'Yesterday', 'This Week', 'This Month', 'Older']

const AGE_ORANGE_MS = 20 * 86400000

function DateWithHeartbeat({
  timestamp,
  heartbeat,
}: {
  timestamp: number
  heartbeat: boolean
}): React.JSX.Element {
  const old = Date.now() - timestamp >= AGE_ORANGE_MS
  return (
    <span className="flex items-center gap-1 flex-shrink-0">
      {heartbeat && (
        <span
          className="flex-shrink-0 text-[var(--color-text-muted)] opacity-80"
          title="A heartbeat delivers into this chat"
        >
          <IconAutonomous className="w-3.5 h-3.5" />
        </span>
      )}
      <span
        className="text-[10px] font-mono tabular-nums"
        style={{
          color: old ? 'var(--color-status-working)' : 'var(--color-text-muted)',
        }}
        title={old ? 'Session is 20+ days old' : undefined}
      >
        {formatTime(timestamp)}
      </span>
    </span>
  )
}

/** Get the right-most leaf node ID in a mosaic tree */
function getRightmostLeaf(tree: unknown): string | null {
  if (tree === null || tree === undefined) return null
  if (typeof tree === 'string') return tree
  if (typeof tree === 'object' && tree !== null && 'second' in tree) {
    return getRightmostLeaf((tree as { second: unknown }).second)
  }
  if (typeof tree === 'object' && tree !== null && 'first' in tree) {
    return getRightmostLeaf((tree as { first: unknown }).first)
  }
  return null
}

// ── Icons ────────────────────────────────────────────────────────────
//
// Provider→icon mapping lives in the shared ProviderIcon module (also
// used by AgentChatPane's canonical-session dropdown) — see the import.

interface ChatStoragePaths {
  claudeHistoryFile: string | null
  claudeSessionsDirs: string[]
  cursorChatsDirs: string[]
  geminiChatsDirs: string[]
  piChatsDirs: string[]
  codexSessionsDirs: string[]
  codexHistoryFile: string | null
  grokSessionsDirs: string[]
  hermesStateDb: string | null
}

function SearchIcon(): React.JSX.Element {
  return (
    <svg
      className="w-3 h-3"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <circle cx="11" cy="11" r="8" />
      <path d="M21 21l-4.35-4.35" />
    </svg>
  )
}

function RefreshIcon(): React.JSX.Element {
  return (
    <svg
      className="w-3 h-3"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M21 2v6h-6" />
      <path d="M3 12a9 9 0 0 1 15-6.7L21 8" />
      <path d="M3 22v-6h6" />
      <path d="M21 12a9 9 0 0 1-15 6.7L3 16" />
    </svg>
  )
}

// ── Component ────────────────────────────────────────────────────────

interface ChatHistoryProps {
  /** Host workspace path supplied by the panel that mounts this view
   *  (LeftPanelContent / RightPanelContent → `rootPath`). Binds the panel
   *  to the workspace it lives in instead of the globally-active one.
   *  See issue #7 + `resolveChatHistoryHost`. Optional so any future
   *  caller that doesn't pass it falls back to the legacy global-pointer
   *  behavior rather than breaking. */
  projectPath?: string
}

export default function ChatHistory({ projectPath: hostProjectPath }: ChatHistoryProps = {}): React.JSX.Element {
  const [sessions, setSessions] = useState<ChatSession[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [searchVisible, setSearchVisible] = useState(false)
  const [searchQuery, setSearchQuery] = useState('')
  const [selectedIndex, setSelectedIndex] = useState(-1)
  const [customNames, setCustomNames] = useState<Record<string, string>>({})
  const [pinnedKeys, setPinnedKeys] = useState<Set<string>>(new Set())
  const [renamingSession, setRenamingSession] = useState<ChatSession | null>(null)
  const [renameValue, setRenameValue] = useState('')
  // Sandboxed chats (API-triggered sandbox sessions) — the audit-resume list.
  const [sandboxSessions, setSandboxSessions] = useState<SandboxChat[]>([])
  const [apiSessions, setApiSessions] = useState<ApiChat[]>([])
  // Three top-level chat sections (replaces single "Chats" collapsible).
  const [showPinned, setShowPinned] = useState(true)
  const [showGeneral, setShowGeneral] = useState(true)
  const [showArchived, setShowArchived] = useState(true)
  const [showApi, setShowApi] = useState(true)
  const [showSandbox, setShowSandbox] = useState(true)
  const [reopening, setReopening] = useState<string | null>(null)
  const [toast, setToast] = useState<string | null>(null)
  const toastTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const renameInputRef = useRef<HTMLInputElement>(null)
  const searchInputRef = useRef<HTMLInputElement>(null)
  const selectedRowRef = useRef<HTMLButtonElement>(null)
  const pollIntervalRef = useRef<ReturnType<typeof setInterval> | null>(null)

  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const activeWorkspaceId = useProjectsStore((s) => s.activeWorkspaceId)

  // Resolve the HOST workspace this panel is mounted inside — bound to the
  // `projectPath` prop, NOT the global active pointers. This is the #7 fix:
  // opening history in workspace A must show A's chats even when B is
  // globally active. Falls back to the global pointers only when no host
  // path is supplied (defensive — every real mount passes one).
  const { project: activeProject, workspace: activeWorkspace, projectPath } = useMemo(
    () => resolveChatHistoryHost(projects, hostProjectPath, activeProjectId, activeWorkspaceId),
    [projects, hostProjectPath, activeProjectId, activeWorkspaceId],
  )

  const heartbeatEntries = useHeartbeatSessionsStore((s) => s.active)
  const heartbeatLoadedFor = useHeartbeatSessionsStore((s) => s.loadedFor)
  const refreshHeartbeats = useHeartbeatSessionsStore((s) => s.refresh)
  const [pinnedWorkspaceSessionId, setPinnedWorkspaceSessionId] = useState<string | null>(null)

  useEffect(() => {
    if (!projectPath) return
    if (heartbeatLoadedFor !== projectPath) {
      void refreshHeartbeats(projectPath)
    }
  }, [projectPath, heartbeatLoadedFor, refreshHeartbeats])

  useEffect(() => {
    if (!projectPath) {
      setPinnedWorkspaceSessionId(null)
      return
    }
    const needsPinned = heartbeatEntries.some((e) => e.row.useWorkspaceSession)
    if (!needsPinned) {
      setPinnedWorkspaceSessionId(null)
      return
    }
    let cancelled = false
    void daemonCliGet<{ resumeSession?: string; resumedExisting?: boolean }>(
      'workspace/resume-chat-args',
      { project: projectPath },
    )
      .then((r) => {
        if (cancelled) return
        const sid = r?.resumedExisting && r.resumeSession?.trim() ? r.resumeSession.trim() : null
        setPinnedWorkspaceSessionId(sid)
      })
      .catch(() => {
        if (!cancelled) setPinnedWorkspaceSessionId(null)
      })
    return () => {
      cancelled = true
    }
  }, [projectPath, heartbeatEntries])

  const heartbeatSessionIds = useMemo(
    () =>
      sessionIdsTargetedByHeartbeats(
        heartbeatEntries.map((e) => e.row),
        pinnedWorkspaceSessionId,
      ),
    [heartbeatEntries, pinnedWorkspaceSessionId],
  )

  const fetchSessions = useCallback(async (showLoading = false) => {
    if (!projectPath) {
      setSessions([])
      setApiSessions([])
      setLoading(false)
      return
    }

    if (showLoading) setLoading(true)
    setError(null)

    try {
      const result = await daemonCliGet<ChatSession[]>('chat/list', { project_path: projectPath })
      setSessions(result)
    } catch (e) {
      console.error('[chat-history]', e)
      setError(String(e))
      setSessions([])
    } finally {
      setLoading(false)
    }

    // Sandboxed chats (best-effort, independent of the normal-chat fetch): the
    // API-triggered sandbox sessions for this workspace. Empty on hosts with no
    // sandbox history — the section just doesn't render.
    try {
      const sb = await daemonCliGet<SandboxChat[]>('sandbox/list', { project_path: projectPath })
      setSandboxSessions(Array.isArray(sb) ? sb : [])
    } catch {
      setSandboxSessions([])
    }
    try {
      const api = await daemonCliGet<ApiChat[]>('host-sessions/list', { project: projectPath })
      setApiSessions(Array.isArray(api) ? api : [])
    } catch {
      setApiSessions([])
    }
  }, [projectPath])

  // Re-launch a sandbox chat INSIDE its sandbox (audit-resume). The daemon
  // re-mounts the session's persistent layer + `claude --resume`; the cell
  // surfaces as its orange tab via the app-level adoption.
  const handleApiClick = useCallback((session: ApiChat) => {
    if (!projectPath) return
    openApiHostSessionTab({
      kind: 'session_added',
      workspace_path: projectPath,
      pane_group_id: null,
      agent_name: session.agentName,
      command: null,
      args: [],
      session_id: session.sessionId,
      isV2: true,
      sandbox_backend: 'host',
      forceAdopt: true,
    })
  }, [projectPath])

  const handleSandboxClick = useCallback(async (session: SandboxChat) => {
    if (!projectPath || reopening) return
    setReopening(session.sessionId)
    try {
      await daemonCliPost('sandbox/reopen', {
        project_path: projectPath,
        session_id: session.sessionId,
      })
    } catch (err) {
      console.error('[chat-history] sandbox reopen failed:', err)
    } finally {
      setReopening(null)
    }
  }, [projectPath, reopening])

  // Fetch custom names and pinned state
  const fetchCustomNames = useCallback(async () => {
    try {
      const names = await daemonCliGet<Record<string, string>>('chat/custom-names')
      setCustomNames(names)
    } catch {
      // ignore
    }
    try {
      // NOTE: chat/pinned is global-scoped (not project-scoped). That's safe
      // ONLY because the session list itself is project-scoped via chat/list
      // above — a pin key (`provider:sessionId`) can only ever match a row
      // that's already in THIS host's list, so cross-project pins never
      // surface here. If chat/list ever stops being project-scoped, this
      // would need scoping too. (Issue #7 secondary note — left as-is.)
      const pinned = await daemonCliGet<string[]>('chat/pinned')
      setPinnedKeys(new Set(pinned))
    } catch {
      // ignore
    }
  }, [])

  // Initial fetch
  useEffect(() => {
    fetchSessions(true)
    fetchCustomNames()
  }, [fetchSessions, fetchCustomNames])

  // 0.40.38 remote live-update — the host emits chat_history_changed on
  // session rename/pin/refresh; refetch immediately instead of waiting
  // for the 30s poll (which was the only path for REMOTE clients — the
  // legacy /events bus is loopback-only and never reached them).
  useEffect(() => {
    return onChatHistoryChanged(() => {
      fetchSessions(false)
      fetchCustomNames()
      if (!projectPath) return
      void daemonCliGet<ChatSession[]>('chat/list', { project_path: projectPath })
        .then((rows) => {
          const tabsStore = useTabsStore.getState()
          const tabs = collectStoreTabs(tabsStore)
          for (const s of Array.isArray(rows) ? rows : []) {
            restampSessionTabs(
              tabs,
              s.sessionId,
              chatDisplayName({ customName: s.customName, title: s.title }),
              tabsStore.setTabTitle,
            )
          }
        })
        .catch(() => { /* list refetch already logged in fetchSessions */ })
    })
  }, [fetchSessions, fetchCustomNames, projectPath])

  // Poll every 30 seconds for new sessions
  useEffect(() => {
    pollIntervalRef.current = setInterval(() => {
      fetchSessions(false) // silent refresh, no loading indicator
    }, 30_000)

    return () => {
      if (pollIntervalRef.current) {
        clearInterval(pollIntervalRef.current)
      }
    }
  }, [fetchSessions])

  /** Split into top-level sections: Pinned | General (date buckets) | Archived. */
  const { pinnedSessions, generalByDate, archivedSessions, generalCount } = useMemo(() => {
    const q = searchQuery.toLowerCase().trim()
    const filtered = q
      ? sessions.filter((s) => {
          if (s.title.toLowerCase().includes(q)) return true
          const custom = s.customName?.toLowerCase()
          if (custom?.includes(q)) return true
          const overlay = customNames[`${s.provider}:${s.sessionId}`]?.toLowerCase()
          return !!overlay?.includes(q)
        })
      : sessions
    const sorted = [...filtered].sort((a, b) => b.timestamp - a.timestamp)

    const pinned: ChatSession[] = []
    const archived: ChatSession[] = []
    const byDate = new Map<DateGroup, ChatSession[]>()
    const apiIds = new Set(apiSessions.map((s) => s.sessionId))

    for (const session of sorted) {
      if (apiIds.has(session.sessionId)) continue
      if (session.archived) {
        archived.push(session)
        continue
      }
      const key = `${session.provider}:${session.sessionId}`
      if (pinnedKeys.has(key)) {
        pinned.push(session)
        continue
      }
      const g = classifyDate(session.timestamp)
      const existing = byDate.get(g)
      if (existing) existing.push(session)
      else byDate.set(g, [session])
    }

    let gCount = 0
    for (const items of byDate.values()) gCount += items.length

    return {
      pinnedSessions: pinned,
      generalByDate: byDate,
      archivedSessions: archived,
      generalCount: gCount,
    }
  }, [sessions, searchQuery, pinnedKeys, apiSessions, customNames])

  // Flat ordered list for keyboard nav: Pinned → General date groups → Archived
  const flatSessions = useMemo(() => {
    const result: ChatSession[] = [...pinnedSessions]
    for (const group of DATE_GROUP_ORDER) {
      const items = generalByDate.get(group)
      if (items) result.push(...items)
    }
    result.push(...archivedSessions)
    return result
  }, [pinnedSessions, generalByDate, archivedSessions])

  // Reset selection when search query changes
  useEffect(() => {
    setSelectedIndex(-1)
  }, [searchQuery])

  // Scroll selected row into view
  useEffect(() => {
    selectedRowRef.current?.scrollIntoView({ block: 'nearest' })
  }, [selectedIndex])

  const handleAgenticSearch = useCallback(async () => {
    if (!projectPath || !searchQuery.trim()) return

    const paths = await daemonCliGet<ChatStoragePaths>('chat/storage-paths', { project_path: projectPath })
    // Resolve the default agent through the one seam (id-first,
    // legacy-token tolerant, first-enabled fallback).
    const resolved = resolveAgentCommand(
      usePresetsStore.getState().presets,
      useSettingsStore.getState().defaultAgent,
    )
    const agent = resolved?.command ?? 'claude'
    const baseArgs = resolved?.args ?? []

    // Build the search prompt with available paths
    const locationLines: string[] = []
    if (paths.claudeHistoryFile) locationLines.push(`- Claude session index: ${paths.claudeHistoryFile}`)
    for (const dir of paths.claudeSessionsDirs) locationLines.push(`- Claude sessions: ${dir}`)
    for (const dir of paths.cursorChatsDirs) locationLines.push(`- Cursor chats: ${dir}`)
    for (const dir of paths.geminiChatsDirs) locationLines.push(`- Gemini chats: ${dir}`)
    for (const dir of paths.piChatsDirs) locationLines.push(`- Pi chats: ${dir}`)
    if (paths.codexHistoryFile) locationLines.push(`- Codex prompt index: ${paths.codexHistoryFile}`)
    for (const dir of paths.codexSessionsDirs) locationLines.push(`- Codex sessions: ${dir}`)
    for (const dir of paths.grokSessionsDirs) locationLines.push(`- Grok sessions: ${dir}`)
    if (paths.hermesStateDb) locationLines.push(`- Hermes sessions DB (SQLite): ${paths.hermesStateDb}`)

    const prompt = [
      `Search through my conversation history for: "${searchQuery.trim()}"`,
      '',
      locationLines.length > 0
        ? `History locations:\n${locationLines.join('\n')}`
        : 'No conversation history files were found for this project.',
      '',
      'Read the relevant files and show which conversations match, with titles, dates, and relevant excerpts.',
    ].join('\n')

    const tabsStore = useTabsStore.getState()
    const targetGroup = tabsStore.splitCount > 1 ? tabsStore.splitCount - 1 : 0

    // Claude uses -p for print mode; other agents get the prompt as a positional arg
    const args = agent === 'claude' ? [...baseArgs, '-p', prompt] : [...baseArgs, prompt]

    tabsStore.addTabToGroup(targetGroup, projectPath, {
      title: `Search: ${searchQuery.trim().slice(0, 30)}`,
      command: agent,
      args,
    })
  }, [projectPath, searchQuery])

  const showToast = useCallback((msg: string) => {
    setToast(msg)
    if (toastTimerRef.current) clearTimeout(toastTimerRef.current)
    toastTimerRef.current = setTimeout(() => setToast(null), 2200)
  }, [])

  const handleArchive = useCallback(async (session: ChatSession) => {
    try {
      await daemonCliPost('chat/archive', {
        project_path: session.project || projectPath,
        provider: session.provider,
        session_id: session.sessionId,
      })
      await fetchSessions(false)
    } catch (err) {
      console.error('[chat-history] archive failed:', err)
      showToast(String(err))
    }
  }, [projectPath, fetchSessions, showToast])

  const handleRestore = useCallback(async (session: ChatSession) => {
    try {
      await daemonCliPost('chat/restore', {
        project_path: session.project || projectPath,
        provider: session.provider,
        session_id: session.sessionId,
      })
      await fetchSessions(false)
    } catch (err) {
      console.error('[chat-history] restore failed:', err)
      showToast(String(err))
    }
  }, [projectPath, fetchSessions, showToast])

  const handleTogglePin = useCallback(async (session: ChatSession) => {
    const key = `${session.provider}:${session.sessionId}`
    const isPinned = pinnedKeys.has(key)
    try {
      await daemonCliPost('chat/toggle-pin', {
        provider: session.provider,
        session_id: session.sessionId,
        pinned: !isPinned,
      })
      setPinnedKeys((prev) => {
        const next = new Set(prev)
        if (isPinned) next.delete(key)
        else next.add(key)
        return next
      })
    } catch (err) {
      console.error('[chat-history] Failed to toggle pin:', err)
    }
  }, [pinnedKeys])

  const copyText = useCallback(async (text: string) => {
    try {
      await navigator.clipboard.writeText(text)
    } catch {
      const ta = document.createElement('textarea')
      ta.value = text
      document.body.appendChild(ta)
      ta.select()
      document.execCommand('copy')
      document.body.removeChild(ta)
    }
  }, [])

  const handleContextMenu = useCallback((session: ChatSession, e: React.MouseEvent) => {
    e.preventDefault()
    e.stopPropagation()
    const key = `${session.provider}:${session.sessionId}`
    const isPinned = pinnedKeys.has(key)

    // Context menu: archived → Copy Path + resume + Restore;
    // Claude live → Pin/Rename/Copy Path/resume + Archive; others without Archive (P0).
    // Position after mount so menus near the bottom/right of the drawer flip
    // into the viewport instead of opening off-page.
    const menuDiv = document.createElement('div')
    menuDiv.style.cssText = `position:fixed;left:0;top:0;z-index:9999;visibility:hidden;background:var(--color-bg-elevated);border:1px solid var(--color-control-track-off);padding:2px 0;min-width:140px;font-size:11px;font-family:var(--font-mono,monospace);box-shadow:0 4px 12px rgba(0,0,0,0.35);`
    const config = PROVIDER_CONFIG[session.provider]
    const resumeCmd = config
      ? (config.resumeSubcommand
          ? `${config.command} ${config.resumeSubcommand} ${session.sessionId}`
          : `${config.command} ${config.resumeFlag} ${session.sessionId}`)
      : `# unknown provider: ${session.provider}`

    const copySessionPath = async () => {
      // Daemon resolves live path or Claude user-archive path on the host.
      const projectForResolve = session.project || projectPath || ''
      try {
        const res = await daemonCliGet<{ path: string | null; project: string | null }>(
          'chat/session-path',
          {
            provider: session.provider,
            session_id: session.sessionId,
            project_path: projectForResolve,
          },
        )
        const path = (res.path && res.path.trim()) || projectForResolve
        if (path) await copyText(path)
      } catch (err) {
        console.warn('[chat-history] session-path failed, copying project path:', err)
        if (projectForResolve) await copyText(projectForResolve)
      }
    }

    type MenuItem = { label: string; action: () => void | Promise<void> }
    let items: MenuItem[]
    if (session.archived) {
      items = [
        { label: 'Copy Path', action: () => copySessionPath() },
        { label: 'Copy resume command', action: async () => { await copyText(resumeCmd) } },
        { label: 'Restore', action: () => handleRestore(session) },
      ]
    } else {
      items = [
        { label: isPinned ? 'Unpin' : 'Pin', action: () => handleTogglePin(session) },
        { label: 'Rename', action: () => {
          setRenamingSession(session)
          setRenameValue(chatDisplayName({
            customName: session.customName ?? customNames[key],
            title: session.title,
          }))
          setTimeout(() => renameInputRef.current?.focus(), 0)
        }},
        { label: 'Copy Path', action: () => copySessionPath() },
        { label: 'Copy resume command', action: async () => { await copyText(resumeCmd) } },
      ]
      // P0: physical archive is Claude-only.
      if (session.provider === 'claude') {
        items.push({ label: 'Archive', action: () => handleArchive(session) })
      }
    }
    const closeMenu = () => {
      if (menuDiv.parentNode) menuDiv.remove()
      document.removeEventListener('mousedown', dismiss)
    }
    for (const item of items) {
      const btn = document.createElement('button')
      btn.textContent = item.label
      btn.style.cssText = 'display:block;width:100%;text-align:left;padding:4px 12px;background:none;border:none;color:#ccc;cursor:pointer;font:inherit;'
      btn.onmouseenter = () => { btn.style.background = 'var(--color-control-track-off)' }
      btn.onmouseleave = () => { btn.style.background = 'none' }
      btn.onclick = () => { item.action(); closeMenu() }
      menuDiv.appendChild(btn)
    }
    document.body.appendChild(menuDiv)
    // Flip / clamp into the viewport after layout (bottom-of-list right-click
    // previously opened downward and ran off the page).
    const pad = 8
    const rect = menuDiv.getBoundingClientRect()
    let left = e.clientX
    let top = e.clientY
    if (left + rect.width > window.innerWidth - pad) {
      left = Math.max(pad, window.innerWidth - rect.width - pad)
    }
    if (top + rect.height > window.innerHeight - pad) {
      // Prefer opening upward from the cursor.
      top = e.clientY - rect.height
    }
    if (top < pad) top = pad
    if (left < pad) left = pad
    menuDiv.style.left = `${left}px`
    menuDiv.style.top = `${top}px`
    menuDiv.style.visibility = 'visible'
    const dismiss = (ev: MouseEvent) => {
      if (!menuDiv.contains(ev.target as Node)) closeMenu()
    }
    setTimeout(() => document.addEventListener('mousedown', dismiss), 0)
  }, [pinnedKeys, customNames, handleTogglePin, handleArchive, handleRestore, projectPath, copyText])

  const handleRenameStart = useCallback((_session: ChatSession, _e: React.MouseEvent) => {
    // Now handled via context menu above
  }, [])

  const handleRenameSubmit = useCallback(async () => {
    if (!renamingSession || !renameValue.trim()) {
      setRenamingSession(null)
      return
    }
    try {
      await daemonCliPost('chat/rename', {
        provider: renamingSession.provider,
        session_id: renamingSession.sessionId,
        custom_name: renameValue.trim(),
      })
      const key = `${renamingSession.provider}:${renamingSession.sessionId}`
      const nextName = renameValue.trim()
      setCustomNames((prev) => ({ ...prev, [key]: nextName }))
      setSessions((prev) => prev.map((s) => (
        s.sessionId === renamingSession.sessionId && s.provider === renamingSession.provider
          ? { ...s, customName: nextName }
          : s
      )))
      const tabsStore = useTabsStore.getState()
      restampSessionTabs(
        collectStoreTabs(tabsStore),
        renamingSession.sessionId,
        nextName,
        tabsStore.setTabTitle,
      )
    } catch (err) {
      console.error('[chat-history] Failed to rename:', err)
      showToast(err instanceof Error ? err.message : String(err))
    }
    setRenamingSession(null)
  }, [renamingSession, renameValue, showToast])

  const handleSessionClick = useCallback(
    (session: ChatSession) => {
      if (!projectPath) return
      if (session.archived) {
        showToast('Restore first.')
        return
      }

      const config = PROVIDER_CONFIG[session.provider]
      if (!config) return

      const tabsStore = useTabsStore.getState()
      const key = `${session.provider}:${session.sessionId}`
      const displayTitle = chatDisplayName({
        customName: session.customName ?? customNames[key],
        title: session.title,
      })

      // Determine if we're resuming across worktree boundaries.
      // When the current workspace branch differs from the session's origin,
      // we fork the session (--fork-session) so the original stays clean and
      // the new worktree gets its own conversation branch.
      // Claude CLI --resume uses the new cwd for file operations, so the
      // worktree re-basing happens automatically via the terminal's cwd.
      //
      // originBranch is null for sessions created in the main repo (not a worktree).
      // worktreePath is null for the main workspace. We use worktreePath presence
      // (not branch name) to determine if we're actually in a worktree, since the
      // main workspace always has a branch name like "main" even though it's not
      // a worktree.
      const isCurrentlyInWorktree = activeWorkspace?.worktreePath != null
      const sessionFromWorktree = session.originBranch != null
      const isCrossWorktree =
        // Both in worktrees but different ones
        (sessionFromWorktree && isCurrentlyInWorktree && session.originBranch !== activeWorkspace?.branch)
        // One is a worktree, the other is main repo
        || (sessionFromWorktree !== isCurrentlyInWorktree)

      // Build resume args. Two shapes depending on provider:
      //   - Flag-style (Claude/Cursor/Gemini/Pi): `<preset-args> <flag> <uuid>`.
      //   - Subcommand-style (Codex): `<preset-args> <subcommand> <uuid>`
      //     (`codex --yolo resume <id>`). Same Settings → LLMs flags as a
      //     fresh launch; global flags sit in front of the subcommand.
      let args: string[]
      if (config.resumeSubcommand) {
        const presetArgs = getPresetArgsForProvider(session.provider)
        args = [...presetArgs, config.resumeSubcommand, session.sessionId]
      } else if (config.resumeFlag) {
        const presetArgs = getPresetArgsForProvider(session.provider)
        args = [...presetArgs, config.resumeFlag, session.sessionId]
      } else {
        args = [session.sessionId]
      }
      if (isCrossWorktree && config.command === 'claude') {
        args.push('--fork-session')
      }

      const title = isCrossWorktree && session.originBranch
        ? `${displayTitle} (from ${session.originBranch})`
        : displayTitle

      // Re-surface dedup: if a tab is already running this exact
      // session (same command + sessionId in args), focus it instead
      // of opening a duplicate. Cross-worktree forks are exempt —
      // --fork-session creates a *new* conversation branch, so the
      // user genuinely wants a fresh tab even if the origin session
      // is open elsewhere.
      if (!isCrossWorktree) {
        const groups: Array<{ tabs: typeof tabsStore.tabs, idx: number }> = [
          { tabs: tabsStore.tabs, idx: 0 },
          ...tabsStore.extraGroups.map((g, i) => ({ tabs: g.tabs, idx: i + 1 })),
        ]
        for (const { tabs, idx } of groups) {
          for (const tab of tabs) {
            if (tab.isSystemAgent) continue
            for (const [, pg] of tab.paneGroups) {
              for (const item of pg.items) {
                if (item.type !== 'terminal') continue
                const td = item.data as TerminalItemData
                if (td.command !== config.command) continue
                if (!td.args?.includes(session.sessionId)) continue
                if (td.heartbeatName || td.fromApi || td.attachAgentName?.startsWith('api-')) continue
                tabsStore.setTabTitle(tab.id, title, { locked: true })
                if (idx === 0) {
                  tabsStore.setActiveTab(tab.id)
                } else {
                  tabsStore.setActiveTabInGroup(idx, tab.id)
                }
                return
              }
            }
          }
        }
      }

      // If split into columns, open in the rightmost group
      const targetGroup = tabsStore.splitCount > 1 ? tabsStore.splitCount - 1 : 0

      const pgId = tabsStore.addTabToGroup(targetGroup, projectPath, {
        title,
        command: config.command,
        args,
        locked: true,
      })
      const st = useTabsStore.getState()
      const created = findTabByPaneGroupId(collectStoreTabs(st), pgId)
      if (created) st.setTabTitle(created.id, title, { locked: true })
    },
    [projectPath, customNames, activeWorkspace, showToast]
  )

  const handleSearchKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'Escape') {
        setSearchQuery('')
        setSearchVisible(false)
        setSelectedIndex(-1)
      } else if (e.key === 'Enter' && e.metaKey) {
        e.preventDefault()
        handleAgenticSearch()
      } else if (e.key === 'Enter' && !e.metaKey && selectedIndex >= 0 && selectedIndex < flatSessions.length) {
        e.preventDefault()
        handleSessionClick(flatSessions[selectedIndex])
      } else if (e.key === 'ArrowDown') {
        e.preventDefault()
        setSelectedIndex((i) => Math.min(i + 1, flatSessions.length - 1))
      } else if (e.key === 'ArrowUp') {
        e.preventDefault()
        setSelectedIndex((i) => Math.max(i - 1, -1))
      }
    },
    [handleAgenticSearch, handleSessionClick, flatSessions, selectedIndex]
  )

  // Empty-state keys on the resolved host path (not `activeProject`): when
  // bound by `projectPath` we can have a valid host path before the matching
  // project row is loaded into the store. `fetchSessions` already no-ops on
  // an undefined path, so this only blanks the panel when there's genuinely
  // no workspace to show.
  if (!projectPath) {
    return (
      <div className="h-full flex items-center justify-center p-4">
        <p className="text-xs text-[var(--color-text-muted)] font-mono">No project selected</p>
      </div>
    )
  }

  return (
    <div className="h-full flex flex-col overflow-hidden relative">
      {/* Header */}
      <div className="h-9 px-3 border-b border-[var(--color-border)] flex items-center justify-between flex-shrink-0">
        <span className="text-xs font-medium text-[var(--color-text-secondary)] font-mono">
          Chat History
        </span>
        <div className="flex items-center gap-0.5">
          <button
            className={`no-drag p-1 transition-colors ${
              searchVisible
                ? 'text-[var(--color-text-primary)]'
                : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
            }`}
            onClick={() => {
              setSearchVisible((v) => !v)
              if (!searchVisible) {
                setTimeout(() => searchInputRef.current?.focus(), 0)
              } else {
                setSearchQuery('')
              }
            }}
            title="Search"
          >
            <SearchIcon />
          </button>
          <button
            className="no-drag p-1 text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors"
            onClick={() => fetchSessions(true)}
            title="Refresh"
          >
            <RefreshIcon />
          </button>
        </div>
      </div>

      {/* Search bar */}
      {searchVisible && (
        <div className="px-3 py-2 border-b border-[var(--color-border)] flex flex-col gap-1.5 flex-shrink-0">
          <input
            ref={searchInputRef}
            type="text"
            className="no-drag w-full bg-white/[0.06] border border-[var(--color-border)] rounded px-2 py-1 text-[11px] font-mono text-[var(--color-text-secondary)] placeholder:text-[var(--color-text-muted)] outline-none focus:border-white/20"
            placeholder="Search chats..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            onKeyDown={handleSearchKeyDown}
          />
          {searchQuery.trim() && (
            <button
              className="no-drag flex items-center justify-between w-full px-2 py-1 rounded text-[11px] font-mono text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:bg-white/[0.06] transition-colors"
              onClick={handleAgenticSearch}
              title="Search agentically with your default agent (⌘↵)"
            >
              <span>Search Agentically</span>
              <KeyCombo combo="⌘↵" className="text-[10px] opacity-60" />
            </button>
          )}
        </div>
      )}

      {/* Content */}
      <div className="flex-1 overflow-y-auto">
        {loading ? (
          <div className="px-3 py-6 text-center">
            <p className="text-[11px] text-[var(--color-text-muted)] font-mono">Loading...</p>
          </div>
        ) : error ? (
          <div className="px-3 py-6 text-center">
            <p className="text-[11px] text-[var(--color-status-error-soft)] font-mono">Failed to load history</p>
          </div>
        ) : sessions.length === 0 && sandboxSessions.length === 0 ? (
          <div className="px-3 py-6 text-center">
            <p className="text-[11px] text-[var(--color-text-muted)] font-mono">
              No conversations yet
            </p>
          </div>
        ) : searchQuery.trim() && flatSessions.length === 0 ? (
          <div className="px-3 py-6 text-center">
            <p className="text-[11px] text-[var(--color-text-muted)] font-mono">
              No matching conversations
            </p>
          </div>
        ) : (
          <>
            {(() => {
              // Shared row renderer + flat index for search keyboard nav.
              let flatIndex = 0
              const renderSessionRow = (session: ChatSession): React.JSX.Element => {
                const idx = flatIndex++
                const isSelected = searchVisible && idx === selectedIndex
                return (
                  <button
                    key={`${session.provider}-${session.sessionId}`}
                    ref={isSelected ? selectedRowRef : undefined}
                    className={`no-drag w-full flex items-center gap-2 px-3 h-8 transition-colors text-left group ${
                      isSelected
                        ? 'bg-white/[0.08] text-[var(--color-text-primary)]'
                        : 'hover:bg-white/[0.04] active:bg-white/[0.06]'
                    }`}
                    onClick={() => {
                      if (renamingSession?.sessionId === session.sessionId && renamingSession?.provider === session.provider) return
                      handleSessionClick(session)
                    }}
                    onContextMenu={(e) => handleContextMenu(session, e)}
                  >
                    <ProviderIcon provider={session.provider} />

                    <div className="flex-1 min-w-0 flex flex-col justify-center">
                      {renamingSession?.sessionId === session.sessionId && renamingSession?.provider === session.provider ? (
                        <input
                          ref={renameInputRef}
                          type="text"
                          value={renameValue}
                          onChange={(e) => setRenameValue(e.target.value)}
                          onKeyDown={(e) => {
                            e.stopPropagation()
                            if (e.key === 'Enter') { e.preventDefault(); handleRenameSubmit() }
                            if (e.key === 'Escape') { e.preventDefault(); setRenamingSession(null) }
                          }}
                          onKeyUp={(e) => e.stopPropagation()}
                          onKeyPress={(e) => e.stopPropagation()}
                          onBlur={handleRenameSubmit}
                          onClick={(e) => e.stopPropagation()}
                          onMouseDown={(e) => e.stopPropagation()}
                          className="text-[11px] font-mono bg-white/[0.06] border border-[var(--color-accent)] text-[var(--color-text-primary)] px-1 py-0 outline-none w-full"
                          maxLength={100}
                        />
                      ) : (
                        <>
                          <span className="text-[11px] text-[var(--color-text-secondary)] font-mono truncate leading-tight">
                            {chatDisplayName({
                              customName: session.customName ?? customNames[`${session.provider}:${session.sessionId}`],
                              title: session.title,
                            })}
                          </span>
                          <span className="text-[10px] text-[var(--color-text-muted)] font-mono leading-tight flex items-center gap-1.5 truncate">
                            {session.messageCount > 0 && (
                              <span className="flex-shrink-0">{session.messageCount} msg{session.messageCount !== 1 ? 's' : ''}</span>
                            )}
                          </span>
                        </>
                      )}
                    </div>

                    <DateWithHeartbeat
                      timestamp={session.timestamp}
                      heartbeat={heartbeatSessionIds.has(session.sessionId)}
                    />
                  </button>
                )
              }

              const sectionHeader = (
                label: string,
                open: boolean,
                onToggle: () => void,
                count: number,
                /** Pinned sits under Chat History header which already has a bottom rule — skip top border. */
                omitTopBorder?: boolean,
              ): React.JSX.Element => (
                <button
                  type="button"
                  className={`no-drag w-full flex items-center gap-1.5 px-3 py-1.5 border-b border-[var(--color-border)] hover:bg-white/[0.03] transition-colors ${
                    omitTopBorder ? '' : 'border-t border-[var(--color-border)]'
                  }`}
                  onClick={onToggle}
                >
                  <span className="text-[9px] text-[var(--color-text-muted)] w-2">{open ? '▼' : '▶'}</span>
                  <span className="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)] font-mono">
                    {label}
                  </span>
                  <span className="text-[9px] text-[var(--color-text-muted)] ml-auto tabular-nums">{count}</span>
                </button>
              )

              return (
                <>
                  {/* ── Pinned (no top rule — Chat History header already has bottom border) ── */}
                  {sectionHeader('Pinned', showPinned, () => setShowPinned((v) => !v), pinnedSessions.length, true)}
                  {showPinned && (
                    <div className="py-0.5">
                      {pinnedSessions.length === 0 ? (
                        <p className="px-3 py-2 text-[10px] text-[var(--color-text-muted)] font-mono">No pinned chats</p>
                      ) : (
                        pinnedSessions.map(renderSessionRow)
                      )}
                    </div>
                  )}

                  {/* ── General (was Chats) — date subgroups only ── */}
                  {sectionHeader('General', showGeneral, () => setShowGeneral((v) => !v), generalCount)}
                  {showGeneral && (
                    <div className="py-0.5">
                      {generalCount === 0 ? (
                        <p className="px-3 py-2 text-[10px] text-[var(--color-text-muted)] font-mono">No chats</p>
                      ) : (
                        DATE_GROUP_ORDER.map((group) => {
                          const items = generalByDate.get(group)
                          if (!items || items.length === 0) return null
                          return (
                            <div key={group} className="mb-0.5">
                              <div className="px-3 py-1 border-b border-white/[0.04]">
                                <span className="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)] font-mono">
                                  {group}
                                </span>
                              </div>
                              {items.map(renderSessionRow)}
                            </div>
                          )
                        })
                      )}
                    </div>
                  )}

                  {/* ── API (host-sessions; close-as-minimize) ── */}
                  {sectionHeader('API', showApi, () => setShowApi((v) => !v), apiSessions.length)}
                  {showApi && (
                    <div className="py-0.5">
                      {apiSessions.length === 0 ? (
                        <p className="px-3 py-2 text-[10px] text-[var(--color-text-muted)] font-mono">No API sessions</p>
                      ) : (
                        apiSessions.map((s) => (
                          <button
                            key={s.sessionId}
                            type="button"
                            onClick={() => handleApiClick(s)}
                            title={s.live ? 'Open API session' : 'Resume API session'}
                            className="no-drag w-full flex items-center gap-2 px-3 h-8 hover:bg-white/[0.04] active:bg-white/[0.06] text-left group transition-colors"
                          >
                            <span
                              className={`w-1.5 h-1.5 rounded-full flex-shrink-0 ${
                                s.live
                                  ? 'bg-[var(--color-status-success)]'
                                  : 'bg-[var(--color-text-muted)]'
                              }`}
                            />
                            <div className="flex-1 min-w-0 flex flex-col justify-center">
                              <span className="text-[11px] text-[var(--color-text-secondary)] font-mono truncate leading-tight">
                                {s.agentName}
                              </span>
                              <span className="text-[10px] text-[var(--color-text-muted)] font-mono leading-tight truncate">
                                {s.live ? 'live · click to open' : 'idle · click to resume'}
                              </span>
                            </div>
                            <DateWithHeartbeat
                              timestamp={s.lastSeenAt > 1e12 ? s.lastSeenAt : s.lastSeenAt * 1000}
                              heartbeat={heartbeatSessionIds.has(s.sessionId)}
                            />
                          </button>
                        ))
                      )}
                    </div>
                  )}

                  {/* ── Archived ── */}
                  {sectionHeader('Archived', showArchived, () => setShowArchived((v) => !v), archivedSessions.length)}
                  {showArchived && (
                    <div className="py-0.5">
                      {archivedSessions.length === 0 ? (
                        <p className="px-3 py-2 text-[10px] text-[var(--color-text-muted)] font-mono">No archived chats</p>
                      ) : (
                        archivedSessions.map(renderSessionRow)
                      )}
                    </div>
                  )}
                </>
              )
            })()}
          </>
        )}

        {/* ── Section: Sandboxed (API-triggered cell sessions; re-launch in sandbox) ── */}
        {sandboxSessions.length > 0 && (
          <>
            <button
              className="no-drag w-full flex items-center gap-1.5 px-3 py-1.5 border-y border-white/[0.06] hover:bg-white/[0.03] transition-colors mt-1"
              onClick={() => setShowSandbox((v) => !v)}
            >
              <span className="text-[9px] text-[var(--color-text-muted)] w-2">{showSandbox ? '▼' : '▶'}</span>
              <span className="text-[10px] font-semibold uppercase tracking-wider text-[#e8843c] font-mono">Sandboxed</span>
              <span className="text-[9px] text-[var(--color-text-muted)] ml-auto tabular-nums">{sandboxSessions.length}</span>
            </button>
            {showSandbox && sandboxSessions.map((s) => (
              <button
                key={s.sessionId}
                onClick={() => handleSandboxClick(s)}
                disabled={reopening === s.sessionId}
                title="Re-launch this chat inside its sandbox"
                className="no-drag w-full flex items-center gap-2 px-3 h-8 hover:bg-white/[0.04] active:bg-white/[0.06] text-left group disabled:opacity-50 transition-colors"
              >
                <span className="w-1.5 h-1.5 rounded-full bg-[#e8843c] flex-shrink-0" />
                <div className="flex-1 min-w-0 flex flex-col justify-center">
                  <span className="text-[11px] text-[var(--color-text-secondary)] font-mono truncate leading-tight">{s.title}</span>
                  <span className="text-[10px] text-[var(--color-text-muted)] font-mono leading-tight truncate">
                    {reopening === s.sessionId ? 'launching in sandbox…' : 'sandbox · click to re-launch'}
                  </span>
                </div>
                <DateWithHeartbeat
                  timestamp={s.timestamp}
                  heartbeat={heartbeatSessionIds.has(s.sessionId)}
                />
              </button>
            ))}
          </>
        )}
      </div>

      {toast && (
        <div className="absolute bottom-3 left-1/2 -translate-x-1/2 z-50 px-3 py-1.5 rounded bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[11px] font-mono text-[var(--color-text-primary)] shadow-lg pointer-events-none">
          {toast}
        </div>
      )}
    </div>
  )
}
