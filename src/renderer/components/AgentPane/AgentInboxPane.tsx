import { useState, useEffect, useCallback } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { agentDisplayName } from '@/lib/workspace-agent'
import { useTabsStore } from '@/stores/tabs'

// ── Types ───────────────────────────────────────────────────────────────

// Phase 2.1c Item 2 — migrated from the legacy `WorkItem` shape
// (returned by `k2so_agents_work_list` / `k2so_agents_workspace_inbox_list`)
// to the workspace-inbox primitive's `InboxItem` shape. Field rename:
// `assignedBy` → `from`, `itemType` is gone, new `id` (filename stem).
// The legacy `.k2so/agents/<name>/work/` per-agent surface is being
// retired alongside the Phase 2.1 1:1 (workspace==agent) refactor.
interface InboxItem {
  id: string
  filename: string
  folder: string
  title: string
  priority: string
  created: string
  source: string
  from: string
  bodyPreview: string
}

interface AgentProfile {
  isCoordinator: boolean
  agentType: string
}

interface AgentInboxPaneProps {
  agentName: string
  projectPath: string
}

// ── Helpers ─────────────────────────────────────────────────────────────

const priorityBadge = (p: string): string => {
  const colors: Record<string, string> = {
    critical: 'bg-red-500/15 text-red-400',
    high: 'bg-orange-500/15 text-orange-400',
    normal: 'bg-white/5 text-[var(--color-text-muted)]',
    low: 'bg-white/5 text-[var(--color-text-muted)] opacity-60',
  }
  return colors[p] || colors.normal
}

// ── Kanban Card ─────────────────────────────────────────────────────────

function KanbanCard({ item, onClick }: { item: InboxItem; onClick: () => void }): React.JSX.Element {
  return (
    <div
      onClick={onClick}
      className="px-3 py-2.5 bg-[var(--color-bg-elevated)] border border-[var(--color-border)] hover:border-[var(--color-text-muted)]/30 cursor-pointer transition-colors mb-2"
    >
      <div className="text-xs font-medium text-[var(--color-text-primary)] leading-snug">{item.title}</div>
      {item.bodyPreview && (
        <div className="text-[10px] text-[var(--color-text-muted)] leading-relaxed mt-1.5 line-clamp-2">{item.bodyPreview}</div>
      )}
      <div className="flex items-center gap-1.5 mt-2">
        <span className={`text-[9px] font-medium px-1.5 py-0.5 ${priorityBadge(item.priority)}`}>
          {item.priority}
        </span>
        <span className="text-[9px] text-[var(--color-text-muted)]">{item.source}</span>
      </div>
      {item.from && item.from !== 'user' && item.from !== 'self' && item.from !== 'cli' && item.from !== 'unknown' && (
        <div className="mt-2">
          <span className="text-[9px] font-medium px-1.5 py-0.5 bg-[var(--color-accent)]/10 text-[var(--color-accent)]">
            {item.from}
          </span>
        </div>
      )}
    </div>
  )
}

// ── Kanban Column ───────────────────────────────────────────────────────

function KanbanColumn({ title, items, color, projectPath, onOpenFile }: {
  title: string
  items: InboxItem[]
  color: string
  projectPath: string
  /**
   * Phase 2.1c Item 2 — `agentDir` prop dropped. After the inbox
   * primitive migration, all items resolve to `.k2so/inbox/[<folder>/]<filename>`
   * (workspace-level). Per-agent paths are no longer rendered here.
   */
  onOpenFile: (path: string) => void
}): React.JSX.Element {
  const resolvePath = (item: InboxItem): string => {
    const base = `${projectPath}/.k2/inbox`
    return item.folder ? `${base}/${item.folder}/${item.filename}` : `${base}/${item.filename}`
  }

  return (
    <div className="flex-1 min-w-0 flex flex-col">
      <div className="flex items-center gap-1.5 mb-2.5 px-1">
        <span className={`text-[10px] font-semibold uppercase tracking-wider ${color}`}>{title}</span>
        {items.length > 0 && (
          <span className="text-[9px] tabular-nums font-medium px-1.5 py-0.5 bg-white/5 text-[var(--color-text-muted)]">
            {items.length}
          </span>
        )}
      </div>
      <div className="flex-1 overflow-y-auto px-0.5">
        {items.length === 0 ? (
          <div className="px-3 py-4 text-[11px] text-[var(--color-text-muted)] text-center border border-dashed border-[var(--color-border)]">
            None
          </div>
        ) : (
          items.map((item) => (
            <KanbanCard
              key={item.filename}
              item={item}
              onClick={() => onOpenFile(resolvePath(item))}
            />
          ))
        )}
      </div>
    </div>
  )
}

// ── Main Component ──────────────────────────────────────────────────────

/**
 * Inbox pinned tab — shows the work queue for an agent or the
 * workspace-level board (when agentName === '__workspace__').
 *
 * Replaces the "Work" sub-tab from the pre-0.36.0 single AgentPane.
 * Sibling tab is `AgentChatPane`; both are pinned by `tabs.ts`.
 */
export function AgentInboxPane({ agentName, projectPath }: AgentInboxPaneProps): React.JSX.Element {
  const isWorkspaceBoard = agentName === '__workspace__'

  const [profile, setProfile] = useState<AgentProfile | null>(null)
  // Top-level inbox arrivals (untriaged). Workspace inbox primitive.
  const [inboxItems, setInboxItems] = useState<InboxItem[]>([])
  // Items the agent moved into `active/`.
  const [activeItems, setActiveItems] = useState<InboxItem[]>([])
  // Items the agent moved into `done/`.
  const [doneItems, setDoneItems] = useState<InboxItem[]>([])
  // 0.37.4: header label is the agent's display name (AGENT.md
  // `display_name:` → `name:` → projects.name fallback). Daemon-side
  // helper is mtime-cached so this fetch is cheap; we still hold a
  // local copy so the header doesn't flicker every re-render.
  const [displayName, setDisplayName] = useState<string>(agentName)

  const fetchProfile = useCallback(async () => {
    if (isWorkspaceBoard) return
    try {
      const result = await invoke<string | { content: string }>('k2so_agents_get_profile', { projectPath, agentName })
      const raw = typeof result === 'string' ? result : (result.content || '')
      const fmMatch = raw.match(/^---\n([\s\S]*?)\n---/)
      let isCoordinator = false
      let agentType = 'agent-template'
      if (fmMatch) {
        const fm = fmMatch[1]
        isCoordinator = fm.match(/^pod_leader:\s*(.+)$/m)?.[1]?.trim() === 'true'
          || fm.match(/^coordinator:\s*(.+)$/m)?.[1]?.trim() === 'true'
          || fm.match(/^manager:\s*(.+)$/m)?.[1]?.trim() === 'true'
        const rawType = fm.match(/^type:\s*(.+)$/m)?.[1]?.trim() || 'agent-template'
        agentType = rawType === 'pod-leader' || rawType === 'manager'
          ? 'coordinator'
          : rawType === 'pod-member' ? 'agent-template' : rawType
      }
      setProfile({ isCoordinator, agentType })
    } catch {
      setProfile(null)
    }
  }, [projectPath, agentName, isWorkspaceBoard])

  const isManager = profile?.isCoordinator
    || profile?.agentType === 'coordinator'
    || profile?.agentType === 'manager'

  // Phase 2.1c Item 2 — migrated to the workspace inbox primitive
  // (`k2so_inbox_list`). The legacy per-agent `.k2so/agents/<name>/work/`
  // queues are being retired alongside the Phase 2.1 1:1
  // (workspace==agent) refactor; the kanban now reads the workspace
  // inbox directly. The three columns map to:
  //   - Inbox     → top-level (.k2so/inbox/*.md)
  //   - Active    → .k2so/inbox/active/*.md
  //   - Done      → .k2so/inbox/done/*.md
  // Manager / workspace-board views render the same data with
  // different column labels (Unassigned / In Progress / Review).
  const fetchWork = useCallback(async () => {
    try {
      const [inbox, active, done] = await Promise.all([
        invoke<InboxItem[]>('k2so_inbox_list', { projectPath, folder: '' }),
        invoke<InboxItem[]>('k2so_inbox_list', { projectPath, folder: 'active' }),
        invoke<InboxItem[]>('k2so_inbox_list', { projectPath, folder: 'done' }),
      ])
      setInboxItems(inbox)
      setActiveItems(active)
      setDoneItems(done)
    } catch {
      // Defensive: keep last-good state on transient daemon errors.
      // Avoid resetting to [] mid-render so the kanban doesn't
      // flicker between empty and populated on each poll failure.
    }
    // Suppress unused-var warning when isWorkspaceBoard/isManager
    // are read only to drive the column labels below.
    void isWorkspaceBoard
    void isManager
  }, [projectPath, isWorkspaceBoard, isManager])

  useEffect(() => {
    fetchProfile()
    fetchWork()
    const interval = setInterval(fetchWork, 10_000)
    return () => clearInterval(interval)
  }, [fetchProfile, fetchWork])

  useEffect(() => {
    let cancelled = false
    if (isWorkspaceBoard) { setDisplayName(agentName); return }
    agentDisplayName(projectPath)
      .then((n) => { if (!cancelled && n) setDisplayName(n) })
      .catch(() => { /* keep agentName as fallback */ })
    return () => { cancelled = true }
  }, [projectPath, agentName, isWorkspaceBoard])

  // 0.37.4: when the user changes the agent display name in
  // Settings, the daemon emits SyncProjects → renderer fires
  // `sync:projects`. Re-fetch on that signal so this header
  // updates without a page reload.
  useEffect(() => {
    if (isWorkspaceBoard) return
    let unlisten: (() => void) | null = null
    let cancelled = false
    listen('sync:projects', () => {
      agentDisplayName(projectPath)
        .then((n) => { if (n) setDisplayName(n) })
        .catch(() => {})
    }).then((u) => { if (cancelled) u(); else unlisten = u })
    return () => { cancelled = true; unlisten?.() }
  }, [projectPath, isWorkspaceBoard])

  const openFile = (filePath: string): void => useTabsStore.getState().openFileAsTab(filePath)

  // Phase 2.1c Item 2 — all three views (single-agent / manager /
  // workspace-board) render the same workspace-inbox-primitive
  // data, just with different column labels. Per-agent fan-out is
  // intentionally gone (see fetchWork notes above).

  return (
    <div className="h-full flex flex-col bg-[var(--color-bg)] overflow-hidden">
      <div className="px-3 py-2 border-b border-[var(--color-border)] flex-shrink-0 flex items-center gap-3">
        <span className="text-xs font-semibold text-[var(--color-text-primary)] truncate">
          {isWorkspaceBoard ? 'Work Board' : displayName}
        </span>
        {profile?.isCoordinator && (
          <span className="text-[9px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 px-1.5 py-0.5 flex-shrink-0">
            MANAGER
          </span>
        )}
      </div>

      <div className="flex-1 overflow-hidden min-h-0 relative">
        {isWorkspaceBoard ? (
          <div className="absolute inset-0 z-10 flex gap-3 p-3 overflow-y-auto">
            <KanbanColumn title="Unassigned" items={inboxItems} color="text-[var(--color-accent)]" projectPath={projectPath} onOpenFile={openFile} />
            <KanbanColumn title="In Progress" items={activeItems} color="text-yellow-400" projectPath={projectPath} onOpenFile={openFile} />
            <KanbanColumn title="Review" items={doneItems} color="text-green-400" projectPath={projectPath} onOpenFile={openFile} />
          </div>
        ) : isManager ? (
          <div className="absolute inset-0 z-10 flex gap-3 p-3 overflow-y-auto bg-[var(--color-bg)]">
            <KanbanColumn title="Inbox" items={inboxItems} color="text-[var(--color-accent)]" projectPath={projectPath} onOpenFile={openFile} />
            <KanbanColumn title="Delegated" items={activeItems} color="text-yellow-400" projectPath={projectPath} onOpenFile={openFile} />
            <KanbanColumn title="Review" items={doneItems} color="text-green-400" projectPath={projectPath} onOpenFile={openFile} />
          </div>
        ) : (
          <div className="absolute inset-0 z-10 flex gap-3 p-3 overflow-y-auto bg-[var(--color-bg)]">
            <KanbanColumn title="Inbox" items={inboxItems} color="text-[var(--color-accent)]" projectPath={projectPath} onOpenFile={openFile} />
            <KanbanColumn title="Active" items={activeItems} color="text-yellow-400" projectPath={projectPath} onOpenFile={openFile} />
            <KanbanColumn title="Done" items={doneItems} color="text-green-400" projectPath={projectPath} onOpenFile={openFile} />
          </div>
        )}
      </div>
    </div>
  )
}
