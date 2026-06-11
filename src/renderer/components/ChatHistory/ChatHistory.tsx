import { useState, useEffect, useMemo, useCallback, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { useProjectsStore } from '@/stores/projects'
import { useTabsStore, type TerminalItemData } from '@/stores/tabs'
import { useSettingsStore } from '@/stores/settings'
import { usePresetsStore } from '@/stores/presets'
import AgentIcon from '@/components/AgentIcon/AgentIcon'
import { KeyCombo } from '@/components/KeySymbol'
import { resolveChatHistoryHost } from './resolveHost'

// ── Types ────────────────────────────────────────────────────────────

interface ChatSession {
  sessionId: string
  project: string
  title: string
  timestamp: number // unix ms
  provider: string
  messageCount: number
  originBranch: string | null
}

type DateGroup = 'Pinned' | 'Today' | 'Yesterday' | 'This Week' | 'This Month' | 'Older'

// ── CLI tool config ─────────────────────────────────────────────────

// Per-provider resume contract. Either `resumeFlag` ("flag-style":
// `<command> <preset-args> <flag> <uuid>`) OR `resumeSubcommand`
// ("subcommand-style": `<command> <subcommand> <uuid>` — no preset
// args, since the saved session carries its own model/permissions).
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
  gemini: { command: 'gemini', label: 'Gemini', resumeFlag: '--resume' },
  pi: { command: 'pi', label: 'Pi', resumeFlag: '--session' },
  codex: { command: 'codex', label: 'Codex', resumeSubcommand: 'resume' },
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

const GROUP_ORDER: DateGroup[] = ['Pinned', 'Today', 'Yesterday', 'This Week', 'This Month', 'Older']

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

/** Map chat history provider names to AgentIcon agent names */
const PROVIDER_AGENT_NAME: Record<string, string> = {
  claude: 'Claude',
  cursor: 'Cursor Agent',
  gemini: 'Gemini',
  pi: 'Pi',
  codex: 'Codex',
}

function ProviderIcon({ provider }: { provider: string }): React.JSX.Element {
  const agentName = PROVIDER_AGENT_NAME[provider] ?? provider
  return <AgentIcon agent={agentName} size={14} />
}

interface ChatStoragePaths {
  claudeHistoryFile: string | null
  claudeSessionsDirs: string[]
  cursorChatsDirs: string[]
  geminiChatsDirs: string[]
  piChatsDirs: string[]
  codexSessionsDirs: string[]
  codexHistoryFile: string | null
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

  const fetchSessions = useCallback(async (showLoading = false) => {
    if (!projectPath) {
      setSessions([])
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
  }, [projectPath])

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

  const grouped = useMemo(() => {
    const groups = new Map<DateGroup, ChatSession[]>()

    // Filter by search query if present
    const q = searchQuery.toLowerCase().trim()
    const filtered = q
      ? sessions.filter((s) => s.title.toLowerCase().includes(q))
      : sessions

    // Sort newest first
    const sorted = [...filtered].sort((a, b) => b.timestamp - a.timestamp)

    for (const session of sorted) {
      const key = `${session.provider}:${session.sessionId}`
      const group = pinnedKeys.has(key) ? 'Pinned' as DateGroup : classifyDate(session.timestamp)
      const existing = groups.get(group)
      if (existing) {
        existing.push(session)
      } else {
        groups.set(group, [session])
      }
    }

    return groups
  }, [sessions, searchQuery, pinnedKeys])

  // Flat ordered list of visible sessions for keyboard navigation
  const flatSessions = useMemo(() => {
    const result: ChatSession[] = []
    for (const group of GROUP_ORDER) {
      const items = grouped.get(group)
      if (items) result.push(...items)
    }
    return result
  }, [grouped])

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
    const agent = useSettingsStore.getState().defaultAgent || 'claude'

    // Build the search prompt with available paths
    const locationLines: string[] = []
    if (paths.claudeHistoryFile) locationLines.push(`- Claude session index: ${paths.claudeHistoryFile}`)
    for (const dir of paths.claudeSessionsDirs) locationLines.push(`- Claude sessions: ${dir}`)
    for (const dir of paths.cursorChatsDirs) locationLines.push(`- Cursor chats: ${dir}`)
    for (const dir of paths.geminiChatsDirs) locationLines.push(`- Gemini chats: ${dir}`)
    for (const dir of paths.piChatsDirs) locationLines.push(`- Pi chats: ${dir}`)
    if (paths.codexHistoryFile) locationLines.push(`- Codex prompt index: ${paths.codexHistoryFile}`)
    for (const dir of paths.codexSessionsDirs) locationLines.push(`- Codex sessions: ${dir}`)

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
    const args = agent === 'claude' ? ['-p', prompt] : [prompt]

    tabsStore.addTabToGroup(targetGroup, projectPath, {
      title: `Search: ${searchQuery.trim().slice(0, 30)}`,
      command: agent,
      args,
    })
  }, [projectPath, searchQuery])

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

  const handleContextMenu = useCallback((session: ChatSession, e: React.MouseEvent) => {
    e.preventDefault()
    e.stopPropagation()
    const key = `${session.provider}:${session.sessionId}`
    const isPinned = pinnedKeys.has(key)

    // Show a simple context menu with pin + rename
    const menuDiv = document.createElement('div')
    menuDiv.style.cssText = `position:fixed;left:${e.clientX}px;top:${e.clientY}px;z-index:9999;background:#1e1e1e;border:1px solid #333;padding:2px 0;min-width:140px;font-size:11px;font-family:var(--font-mono,monospace);`
    const config = PROVIDER_CONFIG[session.provider]
    const resumeCmd = config
      ? `${config.command} ${config.resumeFlag} ${session.sessionId}`
      : `# unknown provider: ${session.provider}`

    const items = [
      { label: isPinned ? 'Unpin' : 'Pin', action: () => handleTogglePin(session) },
      { label: 'Rename', action: () => {
        setRenamingSession(session)
        setRenameValue(customNames[key] ?? session.title)
        setTimeout(() => renameInputRef.current?.focus(), 0)
      }},
      { label: 'Copy resume command', action: async () => {
        try {
          await navigator.clipboard.writeText(resumeCmd)
        } catch {
          // Fallback
          const ta = document.createElement('textarea')
          ta.value = resumeCmd
          document.body.appendChild(ta)
          ta.select()
          document.execCommand('copy')
          document.body.removeChild(ta)
        }
      }},
    ]
    const closeMenu = () => {
      if (menuDiv.parentNode) menuDiv.remove()
      document.removeEventListener('mousedown', dismiss)
    }
    for (const item of items) {
      const btn = document.createElement('button')
      btn.textContent = item.label
      btn.style.cssText = 'display:block;width:100%;text-align:left;padding:4px 12px;background:none;border:none;color:#ccc;cursor:pointer;font:inherit;'
      btn.onmouseenter = () => { btn.style.background = '#333' }
      btn.onmouseleave = () => { btn.style.background = 'none' }
      btn.onclick = () => { item.action(); closeMenu() }
      menuDiv.appendChild(btn)
    }
    document.body.appendChild(menuDiv)
    const dismiss = (ev: MouseEvent) => {
      if (!menuDiv.contains(ev.target as Node)) closeMenu()
    }
    setTimeout(() => document.addEventListener('mousedown', dismiss), 0)
  }, [pinnedKeys, customNames, handleTogglePin])

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
      // Update local custom names
      const key = `${renamingSession.provider}:${renamingSession.sessionId}`
      setCustomNames((prev) => ({ ...prev, [key]: renameValue.trim() }))
      // Update any open tab with this session's title
      const tabsStore = useTabsStore.getState()
      const oldTitle = customNames[key] ?? renamingSession.title
      tabsStore.renameTabByTitle(oldTitle, renameValue.trim())
    } catch (err) {
      console.error('[chat-history] Failed to rename:', err)
    }
    setRenamingSession(null)
  }, [renamingSession, renameValue, customNames])

  const handleSessionClick = useCallback(
    (session: ChatSession) => {
      if (!projectPath) return

      const config = PROVIDER_CONFIG[session.provider]
      if (!config) return

      const tabsStore = useTabsStore.getState()
      const key = `${session.provider}:${session.sessionId}`
      const displayTitle = customNames[key] ?? session.title

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
      //     Preset flags carry through (e.g. --dangerously-skip-permissions)
      //     so the resumed session has the same auth as a fresh launch.
      //   - Subcommand-style (Codex): `<subcommand> <uuid>`. Preset flags
      //     are dropped because Codex's resume subcommand only accepts a
      //     small subset of options (the saved session already carries
      //     model/permissions/cwd from when it was first started).
      let args: string[]
      if (config.resumeSubcommand) {
        args = [config.resumeSubcommand, session.sessionId]
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
            for (const [, pg] of tab.paneGroups) {
              for (const item of pg.items) {
                if (item.type !== 'terminal') continue
                const td = item.data as TerminalItemData
                if (td.command !== config.command) continue
                if (!td.args?.includes(session.sessionId)) continue
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

      tabsStore.addTabToGroup(targetGroup, projectPath, {
        title,
        command: config.command,
        args,
      })
    },
    [projectPath, customNames, activeWorkspace]
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
    <div className="h-full flex flex-col overflow-hidden">
      {/* Header */}
      <div className="px-3 py-2 border-b border-[var(--color-border)] flex items-center justify-between flex-shrink-0">
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
            <p className="text-[11px] text-red-400 font-mono">Failed to load history</p>
          </div>
        ) : sessions.length === 0 ? (
          <div className="px-3 py-6 text-center">
            <p className="text-[11px] text-[var(--color-text-muted)] font-mono">
              No conversations yet
            </p>
          </div>
        ) : searchQuery.trim() && grouped.size === 0 ? (
          <div className="px-3 py-6 text-center">
            <p className="text-[11px] text-[var(--color-text-muted)] font-mono">
              No matching conversations
            </p>
          </div>
        ) : (
          <div className="py-1">
            {(() => {
              let flatIndex = 0
              return GROUP_ORDER.map((group) => {
                const items = grouped.get(group)
                if (!items || items.length === 0) return null

                return (
                  <div key={group} className="mb-1">
                    {/* Group header */}
                    <div className="px-3 py-1.5 border-b border-white/[0.04]">
                      <span className="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)] font-mono">
                        {group}
                      </span>
                    </div>

                    {/* Session rows */}
                    {items.map((session) => {
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
                                  {customNames[`${session.provider}:${session.sessionId}`] ?? session.title}
                                </span>
                                <span className="text-[10px] text-[var(--color-text-muted)] font-mono leading-tight flex items-center gap-1.5 truncate">
                                  {session.messageCount > 0 && (
                                    <span className="flex-shrink-0">{session.messageCount} msg{session.messageCount !== 1 ? 's' : ''}</span>
                                  )}
                                </span>
                              </>
                            )}
                          </div>

                          <span className="text-[10px] text-[var(--color-text-muted)] font-mono flex-shrink-0 tabular-nums">
                            {formatTime(session.timestamp)}
                          </span>
                        </button>
                      )
                    })}
                  </div>
                )
              })
            })()}
          </div>
        )}
      </div>
    </div>
  )
}
