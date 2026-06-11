import { useCallback, useState, useRef, useEffect, useMemo } from 'react'
import { useTabsStore, type TerminalItemData } from '@/stores/tabs'
import { useSettingsStore } from '@/stores/settings'
import { useProjectsStore } from '@/stores/projects'
import { useActiveAgentsStore } from '@/stores/active-agents'
import { agentChatId } from '@/lib/terminal-id'
import { useHeartbeatSessionsStore } from '@/stores/heartbeat-sessions'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliPost } from '@/lib/daemon-cli'
import { startTabDrag } from '@/components/Terminal/TerminalArea'
import { showContextMenu } from '@/lib/context-menu'
import AgentCloseDialog from '@/components/AgentCloseDialog/AgentCloseDialog'
import AgentIcon from '@/components/AgentIcon/AgentIcon'

interface TabBarProps {
  cwd: string
  groupIndex?: number
}

export function TabBar({ cwd, groupIndex = 0 }: TabBarProps): React.JSX.Element {
  const tabs = useTabsStore((s) => groupIndex === 0 ? s.tabs : s.extraGroups[groupIndex - 1]?.tabs ?? [])
  const activeTabId = useTabsStore((s) => groupIndex === 0 ? s.activeTabId : s.extraGroups[groupIndex - 1]?.activeTabId ?? null)
  const splitCount = useTabsStore((s) => s.splitCount)
  const addTabToGroup = useTabsStore((s) => s.addTabToGroup)
  const removeTabFromGroup = useTabsStore((s) => s.removeTabFromGroup)
  const setActiveTabInGroup = useTabsStore((s) => s.setActiveTabInGroup)
  const splitTerminalArea = useTabsStore((s) => s.splitTerminalArea)
  const unsplitTerminalArea = useTabsStore((s) => s.unsplitTerminalArea)
  const reorderTabs = useTabsStore((s) => s.reorderTabs)
  const agentMap = useActiveAgentsStore((s) => s.agents)
  const paneStatusMap = useActiveAgentsStore((s) => s.paneStatuses)
  // This TabBar is scoped to one workspace (`cwd`); resolve its projectId so
  // the pinned Chat tab can read its daemon-backed chat pane's working status.
  // The chat pane is keyed `agent-chat:<projectId>` in `paneStatuses` — it's
  // an `agent` item, NOT a `terminal` item, so the generic working-detection
  // below (which only inspects terminal panes) never sees it.
  const projectId = useProjectsStore((s) => s.projects.find((p) => p.path === cwd)?.id ?? null)

  // Heartbeat-tab detection key: a Set of every heartbeat's
  // `lastSessionId` (the Claude `--resume` target stamped in the DB
  // by smart_launch). A tab is considered a heartbeat tab when any
  // of its terminal items has one of these ids in its args — i.e.,
  // the tab is currently running `claude --resume <heartbeat-id>`.
  // This catches:
  //   - tabs opened via the SessionSurfaced flow (which spawn with
  //     these args by construction)
  //   - normal chat tabs that smart_launch's inject path happened to
  //     write the wakeup into (because they were already running
  //     --resume against the same session id)
  // The `activeTerminalId` column is the daemon's session_id, NOT the
  // renderer's terminalId, so cross-referencing on that key never
  // matches. lastSessionId IS the args value the renderer set when
  // it spawned, so it does match. See migration 0036 +
  // `.k2so/prds/heartbeat-active-session-tracking.md`.
  //
  // Selector returns the underlying entries array (Zustand can do
  // reference equality on it) and we useMemo the derived Set so the
  // identity is stable when nothing has changed — otherwise every
  // render allocates a new Set and the selector triggers an
  // infinite re-render loop.
  const heartbeatActiveEntries = useHeartbeatSessionsStore((s) => s.active)
  const heartbeatLastSessionIds = useMemo(() => {
    const ids = new Set<string>()
    for (const entry of heartbeatActiveEntries) {
      const sid = entry.row.lastSessionId
      if (sid) ids.add(sid)
    }
    return ids
  }, [heartbeatActiveEntries])

  const handleAddTab = useCallback(() => {
    addTabToGroup(groupIndex, cwd)
  }, [addTabToGroup, groupIndex, cwd])

  const [pendingClose, setPendingClose] = useState<{ tabId: string; agents: ReturnType<typeof useActiveAgentsStore.getState>['getAgentsInTab'] } | null>(null)

  // ── Inline tab rename (#653, manual half) ──
  // `editingTabId` holds the id of the regular tab whose label is
  // currently swapped for an <input>. Entered via the "Rename Tab"
  // context-menu item or a double-click on the label; committed on
  // Enter/blur, cancelled on Escape. setTabTitle already no-ops on
  // system-agent tabs and persists the title — this is purely the UI.
  const [editingTabId, setEditingTabId] = useState<string | null>(null)
  const renameInputRef = useRef<HTMLInputElement>(null)

  const commitRename = useCallback((tabId: string): void => {
    const value = renameInputRef.current?.value.trim() ?? ''
    // Empty → keep the old title (no-op rename), just exit edit mode.
    if (value) useTabsStore.getState().setTabTitle(tabId, value)
    setEditingTabId(null)
  }, [])

  // Focus + select the rename input whenever edit mode opens.
  useEffect(() => {
    if (!editingTabId) return
    const el = renameInputRef.current
    if (el) {
      el.focus()
      el.select()
    }
  }, [editingTabId])

  // ── Tab reorder state ──
  const [reorderDragIndex, setReorderDragIndex] = useState<number | null>(null)
  const [reorderDropIndex, setReorderDropIndex] = useState<number | null>(null)
  const reorderDropRef = useRef<number | null>(null)
  const reorderFromRef = useRef<number | null>(null)
  const tabBarRef = useRef<HTMLDivElement>(null)

  const handleTabMouseDown = useCallback((e: React.MouseEvent, index: number, tabId: string, tabTitle: string) => {
    if (e.button !== 0) return
    if ((e.target as HTMLElement).closest('button')) return

    e.preventDefault() // Prevent text selection from starting

    const startX = e.clientX
    const startY = e.clientY
    let started = false
    let mode: 'reorder' | 'cross-group' | null = null

    // Block all selection during drag
    const blockSelect = (ev: Event): void => ev.preventDefault()
    document.addEventListener('selectstart', blockSelect)

    // Returns the group index of the column under the cursor, or null if
    // the cursor is not over any column. Used both for live mode-switch
    // during mousemove and as a safety net on mouseup.
    const groupUnderCursor = (cx: number, cy: number): number | null => {
      const el = document.elementFromPoint(cx, cy) as HTMLElement | null
      const col = el?.closest('[data-tab-group-index]') as HTMLElement | null
      const attr = col?.dataset?.tabGroupIndex
      return attr === undefined ? null : parseInt(attr, 10)
    }

    const switchToCrossGroup = (cx: number, cy: number): void => {
      mode = 'cross-group'
      setReorderDragIndex(null)
      setReorderDropIndex(null)
      reorderDropRef.current = null
      reorderFromRef.current = null
      document.body.style.cursor = ''
      document.body.style.userSelect = ''
      document.removeEventListener('selectstart', blockSelect)
      startTabDrag({ groupIndex, tabId, tabTitle, mouseX: cx, mouseY: cy })
      document.removeEventListener('mousemove', handleMouseMove)
      document.removeEventListener('mouseup', handleMouseUp)
    }

    const handleMouseMove = (ev: MouseEvent): void => {
      const dx = ev.clientX - startX
      const dy = ev.clientY - startY

      if (!started && (Math.abs(dx) > 3 || Math.abs(dy) > 5)) {
        started = true

        // If multiple columns and dragging vertically, go straight to cross-group
        if (splitCount > 1 && Math.abs(dy) > Math.abs(dx) * 1.5) {
          switchToCrossGroup(ev.clientX, ev.clientY)
          return
        }

        mode = 'reorder'
        reorderFromRef.current = index
        setReorderDragIndex(index)
        document.body.style.cursor = 'grabbing'
        document.body.style.userSelect = 'none'
      }

      if (!started || mode !== 'reorder') return

      // If cursor moves over a different column, switch to cross-group drag.
      // Column-membership is more reliable than a fixed pixel threshold —
      // the previous bounds check (+10 px) failed when the user dropped just
      // a few pixels past the source column's edge, leaving the drag stuck
      // in reorder mode.
      if (splitCount > 1) {
        const overGroup = groupUnderCursor(ev.clientX, ev.clientY)
        if (overGroup !== null && overGroup !== groupIndex) {
          switchToCrossGroup(ev.clientX, ev.clientY)
          return
        }
      }

      // Find which tab slot the cursor is over
      if (!tabBarRef.current) return
      const items = tabBarRef.current.querySelectorAll<HTMLElement>('[data-tab-reorder-index]')
      let dropIdx = 0
      for (let i = 0; i < items.length; i++) {
        const rect = items[i].getBoundingClientRect()
        if (ev.clientX > rect.left + rect.width / 2) dropIdx = i + 1
      }
      reorderDropRef.current = dropIdx
      setReorderDropIndex(dropIdx)
    }

    const handleMouseUp = (ev: MouseEvent): void => {
      document.removeEventListener('mousemove', handleMouseMove)
      document.removeEventListener('mouseup', handleMouseUp)
      document.removeEventListener('selectstart', blockSelect)
      document.body.style.cursor = ''
      document.body.style.userSelect = ''

      if (started && mode === 'reorder') {
        // Safety net: if the drag ended over a different column without
        // ever crossing into cross-group mode (e.g., a fast horizontal
        // drag that skipped intermediate mousemoves), still treat it as
        // a cross-column move instead of reordering within the source.
        if (splitCount > 1) {
          const overGroup = groupUnderCursor(ev.clientX, ev.clientY)
          if (overGroup !== null && overGroup !== groupIndex) {
            useTabsStore.getState().moveTabToGroup(groupIndex, overGroup, tabId)
            setReorderDragIndex(null)
            setReorderDropIndex(null)
            reorderDropRef.current = null
            reorderFromRef.current = null
            return
          }
        }

        const fromIdx = reorderFromRef.current
        const dropIdx = reorderDropRef.current
        if (fromIdx !== null && dropIdx !== null && fromIdx !== dropIdx && fromIdx !== dropIdx - 1) {
          const insertAt = dropIdx > fromIdx ? dropIdx - 1 : dropIdx
          reorderTabs(fromIdx, insertAt, groupIndex)
        }
      }

      setReorderDragIndex(null)
      setReorderDropIndex(null)
      reorderDropRef.current = null
      reorderFromRef.current = null
    }

    document.addEventListener('mousemove', handleMouseMove)
    document.addEventListener('mouseup', handleMouseUp)
  }, [groupIndex, splitCount, reorderTabs])

  const handleCloseTab = useCallback(
    (e: React.MouseEvent, tabId: string) => {
      e.stopPropagation()
      const agents = useActiveAgentsStore.getState().getAgentsInTab(tabId)
      if (agents.length > 0) {
        setPendingClose({ tabId, agents })
      } else {
        removeTabFromGroup(groupIndex, tabId)
      }
    },
    [removeTabFromGroup, groupIndex]
  )

  const handleTabContextMenu = useCallback(async (e: React.MouseEvent, tabId: string) => {
    e.preventDefault()
    e.stopPropagation()

    // Get the default terminal name from settings
    const settings = useSettingsStore.getState()
    const defaultTerminal = (settings.projectSettings['__global__'] as any)?.defaultTerminal ?? 'Terminal'

    const allTabs = groupIndex === 0 ? useTabsStore.getState().tabs : useTabsStore.getState().extraGroups[groupIndex - 1]?.tabs ?? []
    const hasOtherTabs = allTabs.length > 1

    // Check if this tab has a running CLI tool — get the terminal ID for copy
    const tab = allTabs.find((t) => t.id === tabId)
    let tabTerminalId: string | null = null
    let fileViewerPath: string | null = null
    if (tab) {
      for (const [, pg] of tab.paneGroups) {
        for (const item of pg.items) {
          if (item.type === 'terminal') {
            const d = item.data as TerminalItemData
            if (d.command) { tabTerminalId = d.terminalId; break }
          } else if (item.type === 'file-viewer') {
            const d = item.data as { filePath?: string }
            if (d.filePath) { fileViewerPath = d.filePath; break }
          }
        }
        if (tabTerminalId || fileViewerPath) break
      }
    }

    const menuItems = [
      { id: 'rename', label: 'Rename Tab' },
      { id: 'open-terminal', label: `Open in ${defaultTerminal}` },
      ...(tabTerminalId ? [
        { id: 'copy-terminal-id', label: 'Copy Terminal ID' },
      ] : []),
      ...(fileViewerPath ? [
        { id: 'show-in-finder', label: 'Show in Finder' },
        { id: 'copy-file-path', label: 'Copy Path' },
      ] : []),
      { id: 'separator', label: '', type: 'separator' as const },
      { id: 'close', label: 'Close Tab' },
      ...(hasOtherTabs ? [
        { id: 'close-others', label: 'Close Other Tabs' },
        { id: 'close-all', label: 'Close All Tabs' },
      ] : []),
    ]

    const clickedId = await showContextMenu(menuItems)
    if (clickedId === 'rename') {
      // Enter inline-edit mode for this (regular) tab. The label→input
      // swap and commit/cancel wiring live in the tab render branch.
      setEditingTabId(tabId)
    } else if (clickedId === 'close') {
      const agents = useActiveAgentsStore.getState().getAgentsInTab(tabId)
      if (agents.length > 0) {
        setPendingClose({ tabId, agents })
      } else {
        removeTabFromGroup(groupIndex, tabId)
      }
    } else if (clickedId === 'close-others') {
      // Close all tabs except the right-clicked one
      for (const tab of allTabs) {
        if (tab.id !== tabId) {
          removeTabFromGroup(groupIndex, tab.id)
        }
      }
    } else if (clickedId === 'close-all') {
      for (const tab of allTabs) {
        removeTabFromGroup(groupIndex, tab.id)
      }
    } else if (clickedId === 'show-in-finder' && fileViewerPath) {
      daemonCliPost('fs/open-finder', { target: fileViewerPath }).catch((err) => console.warn('[tab-bar] show-in-finder', err))
    } else if (clickedId === 'copy-file-path' && fileViewerPath) {
      navigator.clipboard.writeText(fileViewerPath).catch((err) => console.warn('[tab-bar] copy-file-path', err))
    } else if (clickedId === 'copy-terminal-id' && tabTerminalId) {
      // 0.37.11 — Copy the RAW terminal id (v2 session UUID or
      // canonical key). Pre-fix this constructed a
      // `<workspace>:<agent>` "qualified" string which was nice to
      // read but did NOT work as input to any K2 CLI verb. The
      // daemon's `terminal read` / `terminal write` accept either
      // the bare v2 session UUID (in `active_terminal_id` shape)
      // or the v2 canonical key (`<project_id>` post-0.37.5).
      // `tabTerminalId` is already one of those — copy verbatim.
      navigator.clipboard.writeText(tabTerminalId).catch(() => {})
    } else if (clickedId === 'open-terminal') {
      // Find the cwd from the tab's first terminal pane
      const tabsState = useTabsStore.getState()
      const allTabs = groupIndex === 0 ? tabsState.tabs : tabsState.extraGroups[groupIndex - 1]?.tabs ?? []
      const tab = allTabs.find((t) => t.id === tabId)
      if (tab) {
        for (const [, pg] of tab.paneGroups) {
          for (const item of pg.items) {
            if (item.type === 'terminal') {
              const termCwd = (item.data as TerminalItemData).cwd
              // Open the cwd in the default terminal app
              if (defaultTerminal === 'Terminal') {
                import('@tauri-apps/plugin-opener').then(({ openPath }) => openPath(termCwd)).catch((e) => console.warn('[tab-bar]', e))
              } else {
                // Use the terminal app's CLI to open
                invoke('projects_open_in_terminal', { terminalApp: defaultTerminal, path: termCwd }).catch(() => {
                  // Fallback to generic open
                  import('@tauri-apps/plugin-opener').then(({ openPath }) => openPath(termCwd)).catch((e) => console.warn('[tab-bar]', e))
                })
              }
              return
            }
          }
        }
      }
    }
  }, [groupIndex, removeTabFromGroup])

  // Scroll active tab into view when it changes
  useEffect(() => {
    if (!activeTabId || !tabBarRef.current) return
    const container = tabBarRef.current
    const activeEl = container.querySelector(`[data-tab-id="${activeTabId}"]`) as HTMLElement | null
    if (activeEl) {
      activeEl.scrollIntoView({ behavior: 'smooth', block: 'nearest', inline: 'nearest' })
    }
  }, [activeTabId])

  return (
    <div
      className="flex h-9 items-center border-b border-[var(--color-border)] bg-[var(--color-bg-surface)] no-drag"
    >
      <div ref={tabBarRef} className="flex h-full flex-1 items-center overflow-x-auto tabbar-scroll">
        {tabs.map((tab, index) => {
          const isActive = tab.id === activeTabId
          const isDirty = tab.isDirty ?? false
          const agentsInTab = Array.from(agentMap.values()).filter(a => a.tabId === tab.id)
          const hasAgent = agentsInTab.length > 0
          const hasActiveAgent = agentsInTab.some(a => a.status === 'active')
          // Also check hook-based pane statuses (more reliable than polling)
          const terminalIds = Array.from(tab.paneGroups.values())
            .flatMap(pg => pg.items.filter(i => i.type === 'terminal').map(i => (i.data as TerminalItemData).terminalId))
          const hasWorkingHook = terminalIds.some(id => {
            const s = paneStatusMap.get(id)
            return s === 'working' || s === 'permission'
          })
          const isAgentActive = hasActiveAgent || hasWorkingHook
          const isDragged = reorderDragIndex === index
          const showDropBefore = reorderDropIndex === index
          const showDropAfter = reorderDropIndex === tabs.length && index === tabs.length - 1
          // Check if this tab contains a worktree detail pane
          const isWorktreeTab = Array.from(tab.paneGroups.values()).some(
            (pg) => pg.items.some((item) => item.type === 'agent' && (item.data as TerminalItemData & { agentName?: string }).agentName?.startsWith('__wt:'))
          )
          // Detect CLI LLM command running in this tab's terminals
          let cliAgent: string | null = null
          let isAgentPane = false
          for (const [, pg] of tab.paneGroups) {
            for (const item of pg.items) {
              if (item.type === 'terminal') {
                const cmd = (item.data as TerminalItemData).command
                if (cmd) { cliAgent = cmd; break }
              }
              if (item.type === 'agent') {
                isAgentPane = true
              }
            }
            if (cliAgent) break
          }

          // Pinned system agent tab — compact with just icon.
          // Post-0.36.0 the agent UI is split across two pinned tabs;
          // each gets its own icon based on the agent item's section.
          // Legacy rows (section === undefined) keep the K2 mascot.
          if (tab.isSystemAgent) {
            const firstItem = Array.from(tab.paneGroups.values())[0]?.items[0]
            const section: 'inbox' | 'chat' | undefined =
              firstItem?.type === 'agent'
                ? (firstItem.data as { section?: 'inbox' | 'chat' }).section
                : undefined

            // The pinned Chat tab's working signal lives on its daemon-backed
            // chat pane (`agent-chat:<projectId>`), invisible to the generic
            // terminal-pane check above. Read it directly so the icon→spinner
            // swap (and activity underline) fire. Chat-only — the Inbox tab is
            // a passive queue with no working state. Same flag the Active bar
            // uses (paneStatuses, set by recordTitleActivity / lifecycle hook).
            const chatStatus = section === 'chat' && projectId
              ? paneStatusMap.get(agentChatId(projectId, ''))
              : undefined
            const chatWorking = chatStatus === 'working' || chatStatus === 'permission'
            const systemTabActive = isAgentActive || chatWorking

            const iconSvg = (() => {
              if (systemTabActive) {
                return <span className="braille-spinner text-[11px]" />
              }
              if (section === 'inbox') {
                // Inbox tray with downward arrow into it
                return (
                  <svg className="w-4 h-4" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round">
                    <path d="M2 9.5 L4.5 4 H11.5 L14 9.5" />
                    <path d="M2 9.5 V13 a1 1 0 0 0 1 1 H13 a1 1 0 0 0 1 -1 V9.5" />
                    <path d="M2 9.5 H5.5 L6.5 11 H9.5 L10.5 9.5 H14" />
                    <path d="M8 1 V5.5" />
                    <path d="M6 3.5 L8 5.5 L10 3.5" />
                  </svg>
                )
              }
              if (section === 'chat') {
                // Speech-bubble outline with three dots
                return (
                  <svg className="w-4 h-4" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round">
                    <path d="M2 4 a1.5 1.5 0 0 1 1.5 -1.5 H12.5 a1.5 1.5 0 0 1 1.5 1.5 V10 a1.5 1.5 0 0 1 -1.5 1.5 H7.5 L4 14 V11.5 H3.5 a1.5 1.5 0 0 1 -1.5 -1.5 Z" />
                    <circle cx="5.5" cy="7" r="0.6" fill="currentColor" />
                    <circle cx="8" cy="7" r="0.6" fill="currentColor" />
                    <circle cx="10.5" cy="7" r="0.6" fill="currentColor" />
                  </svg>
                )
              }
              // Legacy fallback — K2 mascot for pre-split serialized rows
              return (
                <svg className="w-4 h-4" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M2 8 C2 2.5 4 0.5 8 0.5 C12 0.5 14 2.5 14 8 L13 9.5 L3 9.5 Z" />
                  <path d="M4 9.5 L3.5 11 L5 13.5 L11 13.5 L12.5 11 L12 9.5" />
                  <circle cx="4.8" cy="5.8" r="1.5" />
                  <circle cx="11.2" cy="5.8" r="1.5" />
                  <line x1="5.5" y1="10.8" x2="10.5" y2="10.8" />
                  <line x1="5.2" y1="11.8" x2="10.8" y2="11.8" />
                  <line x1="5.8" y1="12.8" x2="10.2" y2="12.8" />
                </svg>
              )
            })()

            return (
              <div
                key={tab.id}
                data-tab-id={tab.id}
                className={`group relative flex h-full w-9 flex-shrink-0 items-center justify-center border-r border-[var(--color-border)] transition-colors select-none cursor-pointer ${
                  isActive
                    ? 'bg-white/[0.08] text-[var(--color-accent)]'
                    : 'text-[var(--color-text-muted)] hover:bg-white/[0.04] hover:text-[var(--color-text-secondary)]'
                }`}
                onClick={() => setActiveTabInGroup(groupIndex, tab.id)}
                title={tab.title}
              >
                {iconSvg}
                {/* Activity underline */}
                {systemTabActive && (
                  <div className="absolute bottom-0 left-1 right-1 h-[2px] bg-[var(--color-accent)] rounded-full" />
                )}
              </div>
            )
          }

          return (
            <div
              key={tab.id}
              data-tab-id={tab.id}
              data-tab-reorder-index={index}
              className={`group relative flex h-full min-w-[100px] max-w-[200px] flex-shrink-0 items-center border-r border-[var(--color-border)] px-3 text-xs transition-colors select-none ${
                isActive
                  ? 'bg-white/[0.08] text-[var(--color-text-primary)]'
                  : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04]'
              } ${isDragged ? 'opacity-30' : ''}`}
              onClick={() => setActiveTabInGroup(groupIndex, tab.id)}
              onContextMenu={(e) => handleTabContextMenu(e, tab.id)}
              onMouseDown={(e) => handleTabMouseDown(e, index, tab.id, tab.title)}
            >
              {showDropBefore && <div className="absolute left-0 top-1 bottom-1 w-[2px] bg-[var(--color-accent)] z-10" />}
              {showDropAfter && <div className="absolute right-0 top-1 bottom-1 w-[2px] bg-[var(--color-accent)] z-10" />}
              {/* Tab icon — single icon with activity indicator beneath */}
              {(() => {
                // Determine which icon to show
                let icon: React.ReactNode = null
                if (tab.isPinnedFile) {
                  // #587 — pinned HTML file tab. A pin glyph reads as
                  // "kept open next to the Inbox".
                  icon = (
                    <svg className="w-3 h-3 text-[var(--color-accent)]" viewBox="0 0 16 16" fill="currentColor">
                      <path d="M9.828.722a.5.5 0 0 1 .354.146l4.95 4.95a.5.5 0 0 1-.707.707l-.71-.71-3.18 3.18a3.5 3.5 0 0 1-.4.3L11 11.106V14.5a.5.5 0 0 1-.854.354L7.5 12.207 4.854 14.854a.5.5 0 0 1-.708-.708L6.793 11.5 4.146 8.854A.5.5 0 0 1 4.5 8h3.394a3.5 3.5 0 0 0 .3-.4l3.18-3.18-.71-.71a.5.5 0 0 1 .354-.854z" />
                    </svg>
                  )
                } else if (isWorktreeTab) {
                  icon = (
                    <svg className="w-3 h-3 text-[var(--color-text-muted)] opacity-70" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round">
                      <circle cx="4" cy="4" r="1.5" /><circle cx="12" cy="4" r="1.5" /><circle cx="4" cy="12" r="1.5" />
                      <path d="M4 5.5v5M4 8h6c1.1 0 2-.9 2-2v-.5" />
                    </svg>
                  )
                } else if (isAgentPane) {
                  icon = (
                    <svg className="w-3 h-3 text-[var(--color-accent)]" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round">
                      <rect x="3" y="5" width="10" height="8" rx="1.5" /><circle cx="6" cy="9" r="1" /><circle cx="10" cy="9" r="1" />
                      <path d="M8 2v3" /><circle cx="8" cy="1.5" r="0.8" />
                    </svg>
                  )
                } else if (cliAgent) {
                  icon = <AgentIcon agent={cliAgent} size={12} />
                }

                // Activity state
                const isActive = hasAgent && hasActiveAgent

                if (icon) {
                  return (
                    <span className="flex-shrink-0 mr-1.5">
                      {icon}
                    </span>
                  )
                }

                return null
              })()}
              {editingTabId === tab.id ? (
                <input
                  ref={renameInputRef}
                  type="text"
                  defaultValue={tab.title}
                  // Stop the surrounding tab handlers (switch on click,
                  // reorder/cross-group drag on mousedown) from firing
                  // while the user interacts with the rename input.
                  onClick={(e) => e.stopPropagation()}
                  onMouseDown={(e) => e.stopPropagation()}
                  onDoubleClick={(e) => e.stopPropagation()}
                  onKeyDown={(e) => {
                    e.stopPropagation()
                    if (e.key === 'Enter') {
                      e.preventDefault()
                      commitRename(tab.id)
                    } else if (e.key === 'Escape') {
                      e.preventDefault()
                      setEditingTabId(null)
                    }
                  }}
                  onBlur={() => commitRename(tab.id)}
                  className="flex-1 min-w-0 px-1 py-0 text-xs leading-tight bg-[var(--color-bg-primary)] border border-[var(--color-accent)] text-[var(--color-text-primary)] outline-none"
                />
              ) : (
                <span
                  className={`truncate flex-1 ${isDirty ? 'italic' : ''}`}
                  // Double-click the label to rename (regular tabs only —
                  // system-agent tabs render in their own branch above).
                  onDoubleClick={(e) => {
                    e.stopPropagation()
                    setEditingTabId(tab.id)
                  }}
                >
                  {tab.title}
                </span>
              )}
              {/* Heartbeat tabs are "minimize, don't kill" — the
                  daemon-owned PTY keeps running so the heartbeat
                  keeps firing on schedule. Visually distinguish with
                  a `–` glyph instead of `×`. Detection: any terminal
                  item whose data carries `heartbeatName` (stamped at
                  surface time by the session:surfaced listener).
                  Close behavior is gated separately in
                  closeTerminalForRenderer; this is purely the
                  visual cue.
                  See `.k2so/prds/heartbeat-active-session-tracking.md`. */}
              {(() => {
                // Detect a heartbeat tab two ways:
                //   1. `heartbeatName` stamped on the terminal data
                //      (set by the SessionSurfaced flow at attach time).
                //   2. The terminal's id is currently the
                //      `active_terminal_id` of some live heartbeat row.
                // The second covers tabs opened via the legacy
                // fresh-resume path or before the surfaced flow shipped
                // — the heartbeat row is the source of truth so we
                // pick up the relationship without needing the data to
                // be stamped.
                const isHeartbeatTab = Array.from(tab.paneGroups.values()).some((pg) =>
                  pg.items.some((item) => {
                    if (item.type !== 'terminal') return false
                    const td = item.data as TerminalItemData
                    if ((td as any).heartbeatName) return true
                    if (!td.args) return false
                    for (const a of td.args) {
                      if (heartbeatLastSessionIds.has(a)) return true
                    }
                    return false
                  }),
                )
                // Close glyph picks up the heartbeat-vs-regular split.
                // The active-agent layer is independent: if the agent
                // is working we show the braille spinner by default
                // and reveal the close glyph on hover. Heartbeat tabs
                // get `–` (minimize), regular tabs get `×` — same hover
                // pattern, different glyph.
                const closeGlyph = isHeartbeatTab ? (
                  <svg width="8" height="8" viewBox="0 0 8 8" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
                    <line x1="1.5" y1="4" x2="6.5" y2="4" />
                  </svg>
                ) : (
                  <svg width="8" height="8" viewBox="0 0 8 8" fill="none" stroke="currentColor" strokeWidth="1.5">
                    <line x1="1" y1="1" x2="7" y2="7" /><line x1="7" y1="1" x2="1" y2="7" />
                  </svg>
                )
                return (
                  <button
                    className="ml-2 flex h-4 w-4 flex-shrink-0 items-center justify-center hover:bg-white/10 group/close"
                    onClick={(e) => handleCloseTab(e, tab.id)}
                    title={
                      isHeartbeatTab
                        ? isAgentActive
                          ? 'Heartbeat agent is working — hide anyway (PTY keeps running)'
                          : 'Hide tab — heartbeat keeps running in the background'
                        : isAgentActive
                          ? 'Agent is active — close anyway'
                          : 'Close tab'
                    }
                  >
                    {isAgentActive ? (
                      <>
                        <span className="braille-spinner text-[10px] text-[var(--color-text-muted)] group-hover/close:hidden" />
                        <span className="hidden group-hover/close:block">{closeGlyph}</span>
                      </>
                    ) : (
                      closeGlyph
                    )}
                  </button>
                )
              })()}
            </div>
          )
        })}
      </div>

      {/* Only show split/unsplit on the rightmost group's tab bar */}
      {groupIndex === splitCount - 1 && (
        <>
          {/* Split button */}
          {splitCount < 3 && (
            <button
              className="flex h-full w-9 flex-shrink-0 items-center justify-center text-[var(--color-text-muted)] transition-colors hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-secondary)]"
              onClick={() => splitTerminalArea(cwd)}
              title="Split into columns"
            >
              <svg
                width="12"
                height="12"
                viewBox="0 0 12 12"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.3"
              >
                <rect x="0.5" y="0.5" width="11" height="11" />
                <line x1="6" y1="0.5" x2="6" y2="11.5" />
              </svg>
            </button>
          )}

          {/* Unsplit button — only when split */}
          {splitCount > 1 && (
            <button
              className="flex h-full w-9 flex-shrink-0 items-center justify-center text-[var(--color-text-muted)] transition-colors hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-secondary)]"
              onClick={unsplitTerminalArea}
              title="Remove column"
            >
              <svg
                width="12"
                height="12"
                viewBox="0 0 12 12"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.3"
              >
                <rect x="0.5" y="0.5" width="11" height="11" />
              </svg>
            </button>
          )}
        </>
      )}

      {/* Add tab button */}
      <button
        className="flex h-full w-9 flex-shrink-0 items-center justify-center text-[var(--color-text-muted)] transition-colors hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-secondary)]"
        onClick={handleAddTab}
        title="New tab (Cmd+T)"
      >
        <svg
          width="12"
          height="12"
          viewBox="0 0 12 12"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.5"
        >
          <line x1="6" y1="1" x2="6" y2="11" />
          <line x1="1" y1="6" x2="11" y2="6" />
        </svg>
      </button>
      {/* Agent close confirmation dialog */}
      {pendingClose && (
        <AgentCloseDialog
          agents={pendingClose.agents}
          mode="tab"
          onConfirm={() => {
            removeTabFromGroup(groupIndex, pendingClose.tabId)
            setPendingClose(null)
          }}
          onCancel={() => setPendingClose(null)}
        />
      )}
    </div>
  )
}
