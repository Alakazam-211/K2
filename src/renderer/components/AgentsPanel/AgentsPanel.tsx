import { useState, useEffect, useCallback, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { useProjectsStore } from '@/stores/projects'
import { useTabsStore } from '@/stores/tabs'
import { useSettingsStore } from '@/stores/settings'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'

interface K2soAgentInfo {
  name: string
  role: string
  inboxCount: number
  activeCount: number
  doneCount: number
  isCoordinator: boolean // legacy field name from backend; true = manager agent
  agentType: string // "k2so" | "custom" | "manager" | "coordinator" | "agent-template"
}

export default function AgentsPanel(): React.JSX.Element {
  const [agents, setAgents] = useState<K2soAgentInfo[]>([])
  const [wsInboxCount, setWsInboxCount] = useState(0)
  const [loading, setLoading] = useState(true)
  const [showCreate, setShowCreate] = useState(false)
  const [newName, setNewName] = useState('')
  const [newRole, setNewRole] = useState('')
  const [creating, setCreating] = useState(false)
  const [createType, setCreateType] = useState<string>('agent-template')
  const nameInputRef = useRef<HTMLInputElement>(null)

  const activeProject = useProjectsStore((s) => {
    const id = s.activeProjectId
    return id ? s.projects.find((p) => p.id === id) : null
  })

  const fetchAgents = useCallback(async () => {
    if (!activeProject) {
      setAgents([])
      setLoading(false)
      return
    }
    try {
      const result = await invoke<K2soAgentInfo[]>('k2so_agents_list', {
        projectPath: activeProject.path,
      })
      setAgents(result)
    } catch {
      setAgents([])
    } finally {
      setLoading(false)
    }
  }, [activeProject])

  const fetchWsInbox = useCallback(async () => {
    if (!activeProject) return
    // Phase 2.1c Item 2 — migrated to the workspace inbox primitive's
    // lightweight count endpoint (avoids pulling full payloads for
    // a sidebar badge). Defaults to 0 on error to keep the sidebar
    // intact when the daemon is briefly unreachable.
    try {
      const count = await invoke<number>('k2so_inbox_count', { projectPath: activeProject.path })
      setWsInboxCount(count)
    } catch {
      setWsInboxCount(0)
    }
  }, [activeProject])

  useEffect(() => {
    fetchAgents()
    fetchWsInbox()
    const interval = setInterval(() => { fetchAgents(); fetchWsInbox() }, 10_000)
    return () => clearInterval(interval)
  }, [fetchAgents, fetchWsInbox])

  useEffect(() => {
    if (showCreate) {
      requestAnimationFrame(() => nameInputRef.current?.focus())
    }
  }, [showCreate])

  const handleCreate = useCallback(async (typeOverride?: string) => {
    if (!activeProject || !newName.trim() || !newRole.trim()) return
    setCreating(true)
    try {
      await daemonCliGet('agents/create', {
        project: activeProject.path,
        name: newName.trim().toLowerCase().replace(/\s+/g, '-'),
        role: newRole.trim(),
        agent_type: typeOverride || createType,
      })
      setNewName('')
      setNewRole('')
      setShowCreate(false)
      await fetchAgents()
    } catch (e) {
      console.error('[agents] Create failed:', e)
    } finally {
      setCreating(false)
    }
  }, [activeProject, newName, newRole, createType, fetchAgents])

  const handleLaunch = useCallback(async (agentName: string) => {
    if (!activeProject) return
    try {
      const launchInfo = await invoke<{
        command: string
        args: string[]
        cwd: string
        agentName: string
      }>('k2so_agents_build_launch', {
        projectPath: activeProject.path,
        agentName,
      })
      useTabsStore.getState().addTab(launchInfo.cwd, {
        title: `Agent: ${launchInfo.agentName}`,
        command: launchInfo.command,
        args: launchInfo.args,
      })
    } catch (e) {
      console.error('[agents] Launch failed:', e)
    }
  }, [activeProject])

  const handleDelete = useCallback(async (name: string) => {
    if (!activeProject) return
    const confirmed = await useConfirmDialogStore.getState().confirm({
      title: `Delete Agent "${name}"?`,
      message: 'This will delete the agent and all its work items.',
      confirmLabel: 'Delete',
      destructive: true,
    })
    if (!confirmed) return
    try {
      await daemonCliGet('agents/delete', { project: activeProject.path, name })
      await fetchAgents()
    } catch (e) {
      console.error('[agents] Delete failed:', e)
    }
  }, [activeProject, fetchAgents])

  // Status dot color based on work state
  const statusColor = (agent: K2soAgentInfo): string => {
    if (agent.activeCount > 0) return '#eab308' // yellow — working
    if (agent.doneCount > 0) return '#22c55e'   // green — has completed work
    if (agent.inboxCount > 0) return '#3b82f6'  // blue — has pending work
    return '#6b7280'                              // gray — idle
  }

  const agenticEnabled = useSettingsStore((s) => s.agenticSystemsEnabled)

  if (!agenticEnabled) {
    return (
      <div className="h-full flex flex-col items-center justify-center p-4 gap-2">
        <p className="text-[10px] text-[var(--color-text-muted)] text-center">
          Agentic Systems is off
        </p>
        <p className="text-[9px] text-[var(--color-text-muted)] text-center">
          Enable it in Settings &gt; General
        </p>
      </div>
    )
  }

  if (!activeProject) {
    return (
      <div className="h-full flex items-center justify-center p-4">
        <p className="text-[10px] text-[var(--color-text-muted)]">No workspace selected</p>
      </div>
    )
  }

  const agentMode = activeProject.agentMode || 'off'

  const openAgentPane = (name: string) => {
    if (!activeProject) return
    useTabsStore.getState().openAgentPane(name, activeProject.path)
  }

  const AgentRow = ({ agent }: { agent: K2soAgentInfo }) => (
    <div
      className="px-3 py-2 border-b border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] transition-colors cursor-pointer"
      onClick={() => openAgentPane(agent.name)}
    >
      <div className="flex items-center gap-2">
        <span
          className="w-1.5 h-1.5 rounded-full flex-shrink-0"
          style={{ backgroundColor: statusColor(agent) }}
          title={
            agent.activeCount > 0 ? 'Working' :
            agent.doneCount > 0 ? 'Has completed work' :
            agent.inboxCount > 0 ? 'Has pending work' : 'Idle'
          }
        />
        <div className="flex-1 min-w-0">
          <div className="text-xs text-[var(--color-text-primary)] truncate">
            {agent.name}
            {agent.isCoordinator && (
              <span className="ml-1.5 text-[9px] text-[var(--color-accent)] font-medium">MANAGER</span>
            )}
          </div>
          <div className="text-[10px] text-[var(--color-text-muted)] truncate">{agent.role}</div>
        </div>
        <button
          onClick={(e) => { e.stopPropagation(); handleLaunch(agent.name) }}
          className="px-2 py-0.5 text-[10px] font-medium bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent)]/90 transition-colors no-drag cursor-pointer flex-shrink-0"
          title="Launch agent session"
        >
          Launch
        </button>
      </div>
    </div>
  )

  if (agentMode === 'off') {
    return (
      <div className="h-full flex flex-col items-center justify-center p-4 gap-2">
        <p className="text-[10px] text-[var(--color-text-muted)] text-center">
          No agent mode enabled for this workspace
        </p>
        <p className="text-[9px] text-[var(--color-text-muted)] text-center">
          Enable Agent or Workspace Manager mode in workspace settings
        </p>
      </div>
    )
  }

  if (agentMode === 'custom' || agentMode === 'agent') {
    const targetType = agentMode === 'custom' ? 'custom' : 'k2so'
    const label = agentMode === 'custom' ? 'Custom Agent' : 'K2 Agent'
    const typeDesc = agentMode === 'custom'
      ? 'A single agent that runs from its persona on the heartbeat. No K2 infrastructure is injected.'
      : 'A planner agent that builds PRDs, milestones, and technical plans for this workspace.'
    const singleAgent = agents.find((a) => a.agentType === targetType)

    return (
      <div className="h-full flex flex-col overflow-hidden">
        <div className="px-3 py-4 flex flex-col gap-3">
          {singleAgent ? (
            <>
              <div>
                <h3 className="text-sm font-medium text-[var(--color-text-primary)]">{singleAgent.name}</h3>
                <span className="text-[9px] font-medium text-[var(--color-accent)] uppercase tracking-wider">{label}</span>
              </div>
              <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">{typeDesc}</p>
              <div className="flex gap-2">
                <button
                  onClick={() => handleLaunch(singleAgent.name)}
                  className="px-3 py-1.5 text-[10px] font-medium bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent)]/90 transition-colors no-drag cursor-pointer"
                >
                  Launch
                </button>
                <button
                  onClick={() => openAgentPane(singleAgent.name)}
                  className="px-3 py-1.5 text-[10px] font-medium text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer"
                >
                  View
                </button>
              </div>
            </>
          ) : (
            <div className="flex flex-col gap-2">
              <p className="text-[10px] text-[var(--color-text-muted)]">
                No {label.toLowerCase()} configured yet.
              </p>
              <button
                onClick={async () => {
                  if (!activeProject) return
                  try {
                    // Regenerate the workspace SKILL.md (also auto-
                    // scaffolds `.k2so/` layout + manager/k2so-agent
                    // dirs on first call).
                    await daemonCliPost('agents/regenerate-workspace-skill', { project_path: activeProject.path })
                    await fetchAgents()
                  } catch (e) {
                    console.error('[agents] Setup failed:', e)
                  }
                }}
                className="self-start px-3 py-1.5 text-[10px] font-medium bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent)]/90 transition-colors no-drag cursor-pointer"
              >
                Set Up Agent
              </button>
            </div>
          )}
        </div>
      </div>
    )
  }

  // Manager mode — show manager + agent templates

  const manager = agents.find((a) => a.isCoordinator)
  const agentTemplates = agents.filter((a) => !a.isCoordinator && a.agentType !== 'custom' && a.agentType !== 'k2so')
  const totalDelegated = agentTemplates.reduce((sum, a) => sum + a.inboxCount + a.activeCount, 0)
  const totalDone = agentTemplates.reduce((sum, a) => sum + a.doneCount, 0)

  return (
    <div className="h-full flex flex-col overflow-hidden">
      {/* Manager section */}
      {manager && (
        <div className="border-b border-[var(--color-border)]">
          <div className="px-3 py-1.5">
            <span className="text-[9px] font-medium text-[var(--color-accent)] uppercase tracking-wider">Workspace Manager</span>
          </div>
          <div
            className="px-3 py-2 border-b border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] transition-colors cursor-pointer"
            onClick={() => openAgentPane(manager.name)}
          >
            <div className="flex items-center gap-2">
              <span
                className="w-1.5 h-1.5 rounded-full flex-shrink-0"
                style={{ backgroundColor: statusColor(manager) }}
              />
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-1.5">
                  <span className="text-xs text-[var(--color-text-primary)] truncate">{manager.name}</span>
                  <span className="text-[9px] font-medium text-[var(--color-accent)]">MANAGER</span>
                </div>
                <div className="text-[10px] text-[var(--color-text-muted)] truncate">{manager.role}</div>
              </div>
              <div className="flex items-center gap-1 text-[9px] text-[var(--color-text-muted)] flex-shrink-0">
                {wsInboxCount > 0 && <span className="text-[var(--color-accent)]" title="Undelegated">{wsInboxCount}u</span>}
                {totalDelegated > 0 && <span className="text-yellow-400" title="Delegated">{totalDelegated}d</span>}
                {totalDone > 0 && <span className="text-green-400" title="Done">{totalDone}✓</span>}
              </div>
              <button
                onClick={(e) => { e.stopPropagation(); handleLaunch(manager.name) }}
                className="px-2 py-0.5 text-[10px] font-medium bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent)]/90 transition-colors no-drag cursor-pointer flex-shrink-0"
                title="Launch manager session"
              >
                Launch
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Agent Templates header */}
      <div className="flex items-center px-3 py-2 border-b border-[var(--color-border)]">
        <span className="text-[10px] font-medium text-[var(--color-text-muted)] uppercase tracking-wider flex items-center gap-1.5">
          Agent Templates
          {agentTemplates.length > 0 && (
            <span className="text-[9px] tabular-nums font-medium px-1 py-0.5 bg-white/5 text-[var(--color-text-muted)]">{agentTemplates.length}</span>
          )}
        </span>
      </div>

      {/* Agent template list */}
      <div className="flex-1 overflow-y-auto">
        {loading ? (
          <div className="p-3">
            <p className="text-[10px] text-[var(--color-text-muted)]">Loading...</p>
          </div>
        ) : agentTemplates.length === 0 ? (
          <div className="p-3">
            <p className="text-[10px] text-[var(--color-text-muted)]">
              No agent templates yet. Click + to create one.
            </p>
          </div>
        ) : (
          agentTemplates.map((agent) => <AgentRow key={agent.name} agent={agent} />)
        )}
      </div>

      {/* Workspace inbox summary */}
      <WorkspaceInboxButton projectPath={activeProject.path} />
    </div>
  )
}

interface WorkItem {
  filename: string
  title: string
  priority: string
  assignedBy: string
  created: string
  itemType: string
  folder: string
}

// ── Workspace Inbox ───────────────────────────────────────────────────────

function WorkspaceInboxButton({ projectPath }: { projectPath: string }): React.JSX.Element | null {
  const [count, setCount] = useState(0)

  useEffect(() => {
    const check = async () => {
      // Phase 2.1c Item 2 — workspace inbox primitive count endpoint.
      try {
        const c = await invoke<number>('k2so_inbox_count', { projectPath })
        setCount(c)
      } catch { setCount(0) }
    }
    check()
    const interval = setInterval(check, 15_000)
    return () => clearInterval(interval)
  }, [projectPath])

  const openBoard = () => {
    // Open the workspace board as an agent pane for the workspace itself
    useTabsStore.getState().openAgentPane('__workspace__', projectPath)
  }

  return (
    <div className="border-t border-[var(--color-border)] flex-shrink-0 px-3 py-2">
      <button
        onClick={openBoard}
        className="w-full flex items-center justify-between px-2.5 py-1.5 bg-[var(--color-bg-elevated)] border border-[var(--color-border)] hover:border-[var(--color-text-muted)]/30 transition-colors no-drag cursor-pointer"
      >
        <span className="text-[10px] text-[var(--color-text-secondary)]">Work Board</span>
        {count > 0 && (
          <span className="text-[9px] tabular-nums font-medium px-1.5 py-0.5 bg-[var(--color-accent)]/10 text-[var(--color-accent)]">
            {count}
          </span>
        )}
      </button>
    </div>
  )
}
