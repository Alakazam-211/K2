import { useEffect, useRef, useState, useCallback, useMemo } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { terminalListRunning, terminalWrite } from '@/lib/terminal-daemon'
import { useRunningAgentsStore } from '@/stores/running-agents'
import { useTabsStore } from '@/stores/tabs'
import { useProjectsStore } from '@/stores/projects'
import { useActiveAgentsStore } from '@/stores/active-agents'
import AgentIcon from '@/components/AgentIcon/AgentIcon'
import { agentNameFromId } from '@/lib/terminal-id'

interface RunningAgentInfo {
  terminalId: string
  cwd: string
  command: string | null
  tabTitle?: string
  workspaceName?: string
}

export default function RunningAgentsPanel(): React.JSX.Element | null {
  const isOpen = useRunningAgentsStore((s) => s.isOpen)
  const close = useRunningAgentsStore((s) => s.close)

  const [agents, setAgents] = useState<RunningAgentInfo[]>([])
  const [query, setQuery] = useState('')
  const [selectedIndex, setSelectedIndex] = useState(0)
  const [sendingTo, setSendingTo] = useState<string | null>(null)
  const [message, setMessage] = useState('')
  const [copiedId, setCopiedId] = useState<string | null>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  const messageRef = useRef<HTMLInputElement>(null)
  const listRef = useRef<HTMLDivElement>(null)

  // Fetch running agents when panel opens
  useEffect(() => {
    if (!isOpen) return
    setQuery('')
    setSelectedIndex(0)
    setSendingTo(null)
    setMessage('')
    requestAnimationFrame(() => inputRef.current?.focus())

    const load = async () => {
      try {
        const result = (await terminalListRunning()) as RunningAgentInfo[]
        // Enrich with tab titles from frontend state
        const tabsState = useTabsStore.getState()
        const allTabs = [
          ...tabsState.tabs,
          ...tabsState.extraGroups.flatMap((g) => g.tabs),
          ...Object.values(tabsState.backgroundWorkspaces).flatMap((bg: any) => [
            ...(bg.tabs || []),
            ...(bg.extraGroups || []).flatMap((g: any) => g.tabs || []),
          ]),
        ]
        const projects = useProjectsStore.getState().projects
        const activeAgents = useActiveAgentsStore.getState().agents
        for (const agent of result) {
          // Match tab title and terminal command from tab items
          for (const tab of allTabs) {
            for (const [, pg] of tab.paneGroups) {
              const matchItem = pg.items.find((item: any) => item.type === 'terminal' && item.data?.terminalId === agent.terminalId)
              if (matchItem) {
                agent.tabTitle = tab.title
                // Get the CLI command from the terminal item data (e.g. "claude", "codex")
                const itemCommand = (matchItem.data as any)?.command
                if (itemCommand) agent.command = itemCommand
                break
              }
            }
            if (agent.tabTitle) break
          }
          // Also check the active agents store (polling-detected agents)
          const activeAgent = activeAgents.get(agent.terminalId)
          if (activeAgent?.command) {
            agent.command = activeAgent.command
          }
          // Match workspace name from project path
          for (const project of projects) {
            if (agent.cwd.startsWith(project.path)) {
              agent.workspaceName = project.name
              break
            }
          }
        }
        setAgents(result)
      } catch {
        setAgents([])
      }
    }
    load()
    const interval = setInterval(load, 3000)
    return () => clearInterval(interval)
  }, [isOpen])

  // Filter
  const filtered = useMemo(() => {
    if (!query.trim()) return agents
    const q = query.toLowerCase()
    return agents.filter(
      (a) =>
        (a.command || '').toLowerCase().includes(q) ||
        a.cwd.toLowerCase().includes(q) ||
        (a.tabTitle || '').toLowerCase().includes(q)
    )
  }, [agents, query])

  // Clamp selection
  useEffect(() => {
    if (selectedIndex >= filtered.length) setSelectedIndex(Math.max(0, filtered.length - 1))
  }, [filtered.length, selectedIndex])

  // Get hook status for each terminal
  const paneStatuses = useActiveAgentsStore((s) => s.paneStatuses)

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'Escape') {
        if (sendingTo) {
          setSendingTo(null)
          setMessage('')
          requestAnimationFrame(() => inputRef.current?.focus())
        } else {
          close()
        }
        e.preventDefault()
      } else if (e.key === 'ArrowDown' && !sendingTo) {
        e.preventDefault()
        setSelectedIndex((i) => Math.min(i + 1, filtered.length - 1))
      } else if (e.key === 'ArrowUp' && !sendingTo) {
        e.preventDefault()
        setSelectedIndex((i) => Math.max(i - 1, 0))
      } else if (e.key === 'Enter' && !sendingTo && filtered.length > 0) {
        e.preventDefault()
        const agent = filtered[selectedIndex]
        if (agent) {
          // Navigate to the terminal tab
          navigateToTerminal(agent.terminalId, agent.cwd)
          close()
        }
      }
    },
    [close, filtered, selectedIndex, sendingTo]
  )

  const navigateToTerminal = (terminalId: string, cwd: string) => {
    const tabsState = useTabsStore.getState()

    // 1. Check active workspace tabs
    for (const tab of tabsState.tabs) {
      for (const [, pg] of tab.paneGroups) {
        const match = pg.items.find(
          (item) => item.type === 'terminal' && (item.data as any).terminalId === terminalId
        )
        if (match) {
          useTabsStore.setState({ activeTabId: tab.id })
          return
        }
      }
    }

    // 2. Check extra groups (split columns)
    for (const group of tabsState.extraGroups) {
      for (const tab of group.tabs) {
        for (const [, pg] of tab.paneGroups) {
          const match = pg.items.find(
            (item) => item.type === 'terminal' && (item.data as any).terminalId === terminalId
          )
          if (match) {
            useTabsStore.setState({ activeTabId: tab.id })
            return
          }
        }
      }
    }

    // 3. Check background workspaces — need to switch workspace first
    for (const [key, bg] of Object.entries(tabsState.backgroundWorkspaces)) {
      const allBgTabs = [...(bg.tabs || []), ...(bg.extraGroups || []).flatMap((g: any) => g.tabs || [])]
      for (const tab of allBgTabs) {
        for (const [, pg] of tab.paneGroups) {
          const match = pg.items.find(
            (item: any) => item.type === 'terminal' && item.data?.terminalId === terminalId
          )
          if (match) {
            // key format is "projectId:workspaceId"
            const [projectId, workspaceId] = key.split(':')
            if (projectId && workspaceId) {
              useProjectsStore.getState().setActiveWorkspace(projectId, workspaceId)
              // After workspace restore, the tab should be active
              setTimeout(() => useTabsStore.setState({ activeTabId: tab.id }), 100)
            }
            return
          }
        }
      }
    }

    // 4. Fallback: match by CWD to find the right workspace, create a tab for
    //    the running terminal, and navigate to it. This handles companion-spawned
    //    and other background terminals that don't have tabs yet.
    const projects = useProjectsStore.getState().projects
    for (const project of projects) {
      if (cwd.startsWith(project.path)) {
        const ws = project.workspaces.find((w) => cwd.startsWith(w.worktreePath || project.path))
        if (ws) {
          useProjectsStore.getState().setActiveWorkspace(project.id, ws.id)
        } else if (project.workspaces[0]) {
          useProjectsStore.getState().setActiveWorkspace(project.id, project.workspaces[0].id)
        }
        // Create a tab for this running terminal so the user can see it
        setTimeout(() => {
          const store = useTabsStore.getState()
          // Double-check: did workspace switch already create a tab for this terminal?
          const alreadyExists = store.tabs.some((t) =>
            [...t.paneGroups.values()].some((pg) =>
              pg.items.some((item) => item.type === 'terminal' && (item.data as any).terminalId === terminalId)
            )
          )
          if (!alreadyExists) {
            // Determine the command from the agent info
            const agent = agents.find((a) => a.terminalId === terminalId)
            const cmd = agent?.command || 'shell'
            store.addTab(cwd, {
              title: cmd === 'shell' ? `Terminal` : cmd,
              command: cmd !== 'shell' ? cmd : undefined,
            })
            // The new tab gets a NEW terminal ID — but we need to connect it to the
            // EXISTING running terminal. Override the terminal ID in the new tab's data.
            const updatedStore = useTabsStore.getState()
            const newTab = updatedStore.tabs[updatedStore.tabs.length - 1]
            if (newTab) {
              const pg = [...newTab.paneGroups.values()][0]
              if (pg?.items[0]?.data) {
                (pg.items[0].data as any).terminalId = terminalId
              }
              // Update the pane group ID to match (terminal manager uses this)
              const oldPgId = pg?.id
              if (oldPgId && oldPgId !== terminalId) {
                newTab.paneGroups.delete(oldPgId)
                pg.id = terminalId
                newTab.paneGroups.set(terminalId, pg)
                newTab.mosaicTree = terminalId
              }
            }
          }
        }, 200) // brief delay for workspace switch to complete
        return
      }
    }
  }

  const handleSendMessage = async (terminalId: string) => {
    if (!message.trim()) return
    try {
      // Two-phase write: paste text first, then send Enter after delay
      // CLI LLMs swallow \r when it arrives in the same paste event
      await terminalWrite(terminalId, message)
      await new Promise((r) => setTimeout(r, 150))
      await terminalWrite(terminalId, '\r')
      setSendingTo(null)
      setMessage('')
      requestAnimationFrame(() => inputRef.current?.focus())
    } catch (err) {
      console.error('[running-agents] Send failed:', err)
    }
  }

  // Scroll selected into view
  useEffect(() => {
    if (!listRef.current) return
    const el = listRef.current.querySelector(`[data-index="${selectedIndex}"]`)
    el?.scrollIntoView({ block: 'nearest' })
  }, [selectedIndex])

  if (!isOpen) return null

  // Extract workspace name from cwd
  const workspaceName = (cwd: string) => {
    const parts = cwd.split('/')
    return parts[parts.length - 1] || cwd
  }

  const statusColor = (terminalId: string) => {
    const status = paneStatuses.get(terminalId)
    switch (status) {
      case 'working': return '#3b82f6'
      case 'permission': return '#ef4444'
      case 'review': return '#22c55e'
      default: return '#6b7280'
    }
  }

  const statusLabel = (terminalId: string) => {
    const status = paneStatuses.get(terminalId)
    switch (status) {
      case 'working': return 'Working'
      case 'permission': return 'Needs Permission'
      case 'review': return 'Review'
      default: return 'Idle'
    }
  }

  return (
    <div
      className="fixed inset-0 z-[9999] flex items-start justify-center pt-[15vh] no-drag"
      style={{ background: 'rgba(0, 0, 0, 0.6)', backdropFilter: 'blur(8px)' }}
      onClick={(e) => {
        if (e.target === e.currentTarget) close()
      }}
      onKeyDown={handleKeyDown}
    >
      <div
        className="w-[600px] max-h-[60vh] flex flex-col overflow-hidden border border-[var(--color-border)]"
        style={{ background: 'var(--color-bg-surface)', boxShadow: '0 24px 48px rgba(0, 0, 0, 0.5)' }}
      >
        {/* Search input */}
        <div className="flex items-center border-b border-[var(--color-border)] px-4 py-3">
          <svg
            width="14" height="14" viewBox="0 0 16 16" fill="none"
            className="flex-shrink-0 mr-3"
            style={{ color: 'var(--color-text-muted)' }}
          >
            <path
              d="M11.5 7a4.5 4.5 0 1 1-9 0 4.5 4.5 0 0 1 9 0ZM10.643 11.357a6 6 0 1 1 .714-.714l3.85 3.85a.5.5 0 0 1-.707.707l-3.857-3.843Z"
              fill="currentColor"
            />
          </svg>
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value)
              setSelectedIndex(0)
            }}
            placeholder="Filter running agents..."
            className="flex-1 bg-transparent text-sm text-[var(--color-text-primary)] placeholder-[var(--color-text-muted)] outline-none"
          />
          <span className="text-[10px] text-[var(--color-text-muted)] ml-2 flex-shrink-0">
            {filtered.length} active
          </span>
        </div>

        {/* Results */}
        <div ref={listRef} className="flex-1 overflow-y-auto">
          {filtered.length === 0 ? (
            <div className="px-4 py-8 text-center">
              <p className="text-xs text-[var(--color-text-muted)]">
                {agents.length === 0 ? 'No running CLI agents detected.' : 'No matches found.'}
              </p>
            </div>
          ) : (
            filtered.map((agent, index) => {
              const isSelected = index === selectedIndex
              const isSending = sendingTo === agent.terminalId

              return (
                <div key={agent.terminalId} data-index={index}>
                  <div
                    className={`flex items-center gap-3 px-4 py-2.5 cursor-pointer transition-colors ${
                      isSelected ? 'bg-white/[0.06]' : 'hover:bg-white/[0.03]'
                    }`}
                    onClick={() => {
                      navigateToTerminal(agent.terminalId, agent.cwd)
                      close()
                    }}
                    onMouseEnter={() => setSelectedIndex(index)}
                  >
                    {/* LLM icon */}
                    <AgentIcon agent={agent.command || 'shell'} size={16} />

                    {/* Info */}
                    <div className="flex-1 min-w-0">
                      <div className="flex items-center gap-2">
                        <span className="text-xs font-medium text-[var(--color-text-primary)]">
                          {agent.workspaceName || workspaceName(agent.cwd)}
                        </span>
                        <span className="text-[10px] text-[var(--color-text-muted)] truncate">
                          {agent.command || 'shell'}
                        </span>
                      </div>
                      {agent.tabTitle && agent.tabTitle !== (agent.workspaceName || workspaceName(agent.cwd)) && (
                        <div className="text-[10px] text-[var(--color-text-muted)] truncate">
                          {agent.tabTitle}
                        </div>
                      )}
                    </div>

                    {/* Status */}
                    <div className="flex items-center gap-2 flex-shrink-0">
                      <span
                        className="w-1.5 h-1.5 rounded-full"
                        style={{ backgroundColor: statusColor(agent.terminalId) }}
                      />
                      <span className="text-[10px] text-[var(--color-text-muted)]">
                        {statusLabel(agent.terminalId)}
                      </span>
                    </div>

                    {/* Copy workspace:agent identifier */}
                    <button
                      className={`flex h-6 w-6 items-center justify-center transition-colors flex-shrink-0 ${
                        copiedId === agent.terminalId
                          ? 'text-green-400'
                          : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-white/10'
                      }`}
                      onClick={(e) => {
                        e.stopPropagation()
                        const wsName = agent.workspaceName || workspaceName(agent.cwd)
                        const agName = agentNameFromId(agent.terminalId) ?? agent.terminalId
                        navigator.clipboard.writeText(`${wsName}:${agName}`)
                        setCopiedId(agent.terminalId)
                        setTimeout(() => setCopiedId((prev) => prev === agent.terminalId ? null : prev), 1500)
                      }}
                      title={`Copy: ${agent.workspaceName || workspaceName(agent.cwd)}:${agentNameFromId(agent.terminalId) ?? agent.terminalId}`}
                    >
                      {copiedId === agent.terminalId ? (
                        <svg className="w-3 h-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2.5} strokeLinecap="round" strokeLinejoin="round">
                          <polyline points="20 6 9 17 4 12" />
                        </svg>
                      ) : (
                        <svg className="w-3 h-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
                          <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
                          <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
                        </svg>
                      )}
                    </button>

                    {/* Send message */}
                    <button
                      className="flex h-6 w-6 items-center justify-center text-[var(--color-accent)] hover:text-white hover:bg-[var(--color-accent)]/20 transition-colors flex-shrink-0"
                      onClick={(e) => {
                        e.stopPropagation()
                        setSendingTo(agent.terminalId)
                        setMessage('')
                        requestAnimationFrame(() => messageRef.current?.focus())
                      }}
                      title="Send message to this terminal"
                    >
                      <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="currentColor">
                        <path d="M2.01 21L23 12 2.01 3 2 10l15 2-15 2z" />
                      </svg>
                    </button>
                  </div>

                  {/* Inline message input */}
                  {isSending && (
                    <div className="px-4 py-2 bg-white/[0.02] border-t border-[var(--color-border)]">
                      <div className="flex items-center gap-2">
                        <input
                          ref={messageRef}
                          type="text"
                          value={message}
                          onChange={(e) => setMessage(e.target.value)}
                          onKeyDown={(e) => {
                            if (e.key === 'Enter' && message.trim()) {
                              e.stopPropagation()
                              handleSendMessage(agent.terminalId)
                            } else if (e.key === 'Escape') {
                              e.stopPropagation()
                              setSendingTo(null)
                              setMessage('')
                              requestAnimationFrame(() => inputRef.current?.focus())
                            }
                          }}
                          placeholder="Type a message to send to this agent..."
                          className="flex-1 bg-transparent text-xs text-[var(--color-text-primary)] placeholder-[var(--color-text-muted)] outline-none border border-[var(--color-border)] px-2 py-1.5 focus:border-[var(--color-accent)]/50"
                        />
                        <button
                          onClick={() => handleSendMessage(agent.terminalId)}
                          disabled={!message.trim()}
                          className="px-3 py-1.5 text-[10px] font-medium bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent)]/90 transition-colors disabled:opacity-40"
                        >
                          Send
                        </button>
                      </div>
                      <p className="text-[9px] text-[var(--color-text-muted)] mt-1">
                        Text will be typed into the terminal and submitted with Enter.
                      </p>
                    </div>
                  )}
                </div>
              )
            })
          )}
        </div>

        {/* Footer */}
        <div className="px-4 py-2 border-t border-[var(--color-border)] flex items-center justify-between">
          <span className="text-[10px] text-[var(--color-text-muted)]">
            ↑↓ navigate · Enter open · Esc close
          </span>
          <span className="text-[10px] text-[var(--color-text-muted)]">⌘J</span>
        </div>
      </div>
    </div>
  )
}
