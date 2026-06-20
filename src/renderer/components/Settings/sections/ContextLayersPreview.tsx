import React, { useCallback, useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet } from '@/lib/daemon-cli'
import Markdown from '@/components/Markdown/Markdown'
import remarkGfm from 'remark-gfm'

type SkillTier = 'manager' | 'agent_template' | 'custom_agent'

interface SkillLayer {
  filename: string
  title: string
  preview: string
  path: string
}

interface AgentHeartbeat {
  id: string
  projectId: string
  name: string
  frequency: string
  specJson: string
  wakeupPath: string
  enabled: boolean
  lastFired: string | null
  createdAt: number
}

interface Props {
  projectPath: string
  agentMode: string | null
  onOpenSettings?: () => void
  onEditHeartbeat?: (heartbeatName: string) => void
}

const LOCKED_LAYERS: Record<SkillTier, string[]> = {
  manager: [
    'Identity + Workspace State',
    'Connected Workspaces',
    'Team Roster',
    'Standing Orders',
    'Decision Framework',
    'Delegation + Review',
    'Communication Commands',
  ],
  agent_template: [
    'Identity',
    'Check In + Status + Done',
    'File Reservations',
  ],
  custom_agent: [
    'Identity',
    'Check In + Status + Done',
    'Cross-Workspace Messaging',
    'File Reservations',
  ],
}

const LOCKED_LAYER_DESCRIPTIONS: Record<string, string> = {
  'Identity + Workspace State': '**Auto-generated per workspace.** Workspace name, current mode (Build/Managed/Maintenance/Locked), and mode description.',
  'Connected Workspaces': '**Auto-generated.** Workspaces connected via workspace relations — outgoing and incoming.',
  'Team Roster': '**Auto-generated.** All agent templates in this workspace with their roles. Used to decide delegation targets.',
  'Standing Orders': '**Auto-generated.** 9-step wake checklist: `k2so checkin` → triage messages → triage work by priority → handle simple / delegate complex → check active agents → review completed work → update status → mark done.',
  'Decision Framework': '**Auto-generated.** Complexity (Simple vs Complex) × workspace mode (Build / Managed / Maintenance / Locked).',
  'Delegation + Review': '**Auto-generated.** Delegation: choose agent → create work item → `k2so delegate` → agent works in worktree. Review: `k2so review approve/reject/feedback`.',
  'Communication Commands': '**Auto-generated.** `k2so checkin`, `status`, `done`, `msg`, `reserve`, `release`.',
  'Identity': '**Auto-generated per agent.** Agent name + workspace it belongs to.',
  'Check In + Status + Done': '**Auto-generated.** `k2so checkin` (wake briefing), `k2so status "msg"` (report), `k2so done` / `k2so done --blocked "reason"`.',
  'File Reservations': '**Auto-generated.** `k2so reserve <paths>` (claim), `k2so release`.',
  'Cross-Workspace Messaging': '**Auto-generated.** `k2so msg <workspace>:inbox "text"`, `k2so msg --wake` for urgent delivery.',
}

type LayerKind = 'auto' | 'global' | 'ws' | 'heartbeat'

interface LayerEntry {
  key: string
  name: string
  kind: LayerKind
  subtitle?: string
  description?: string
  editAction?: () => void
  // For heartbeat rows — when expanded, fetch WAKEUP.md content from disk
  loadOnExpand?: () => Promise<string>
}

function tierForMode(mode: string | null): SkillTier | null {
  const m = mode || 'off'
  if (m === 'manager' || m === 'coordinator' || m === 'pod') return 'manager'
  if (m === 'custom') return 'custom_agent'
  // 'agent' (K2 Agent) doesn't have a tab in AgentSkillsSection today — no preview
  return null
}

function KindBadge({ kind }: { kind: LayerKind }): React.JSX.Element {
  const style: Record<LayerKind, string> = {
    auto: 'text-[var(--color-text-muted)] italic',
    global: 'text-[var(--color-accent)]',
    ws: 'text-amber-400',
    heartbeat: 'text-emerald-400',
  }
  return <span className={`text-[10px] ${style[kind]}`}>{kind}</span>
}

export function ContextLayersPreview({ projectPath, agentMode, onOpenSettings, onEditHeartbeat }: Props): React.JSX.Element | null {
  const tier = tierForMode(agentMode)
  const [customLayers, setCustomLayers] = useState<SkillLayer[]>([])
  const [heartbeats, setHeartbeats] = useState<AgentHeartbeat[]>([])
  const [hasProjectContext, setHasProjectContext] = useState(false)
  const [expanded, setExpanded] = useState<string | null>(null)
  const [expandedBody, setExpandedBody] = useState<string>('')

  const loadLayers = useCallback(async () => {
    if (!tier) { setCustomLayers([]); return }
    try {
      const list = await daemonCliGet<SkillLayer[]>('skill-layers/list', { tier })
      setCustomLayers(list)
    } catch {
      setCustomLayers([])
    }
  }, [tier])

  const loadHeartbeats = useCallback(async () => {
    try {
      const list = await invoke<AgentHeartbeat[]>('k2so_heartbeat_list', { projectPath })
      setHeartbeats(list.filter((h) => h.enabled))
    } catch {
      setHeartbeats([])
    }
  }, [projectPath])

  const checkProjectContext = useCallback(async () => {
    try {
      const r = await daemonCliGet<{ content: string }>('fs/read-file', { path: `${projectPath}/.k2/PROJECT.md` })
      setHasProjectContext(!!r.content && r.content.trim().length > 0)
    } catch {
      setHasProjectContext(false)
    }
  }, [projectPath])

  useEffect(() => {
    loadLayers()
    loadHeartbeats()
    checkProjectContext()
  }, [loadLayers, loadHeartbeats, checkProjectContext])

  if (!tier) return null

  const entries: LayerEntry[] = []

  for (const name of LOCKED_LAYERS[tier]) {
    entries.push({
      key: `auto-${name}`,
      name,
      kind: 'auto',
      description: LOCKED_LAYER_DESCRIPTIONS[name] || `*Auto-generated section: ${name}*`,
      editAction: onOpenSettings,
    })
  }

  for (const layer of customLayers) {
    entries.push({
      key: `global-${layer.filename}`,
      name: layer.title,
      kind: 'global',
      subtitle: layer.preview || undefined,
      description: `**Custom layer** (applies to all ${tier === 'manager' ? 'manager' : tier === 'custom_agent' ? 'custom-agent' : 'template'} workspaces). Edit in Settings → Canonical Agent Flow.`,
      editAction: onOpenSettings,
      loadOnExpand: async () => {
        try {
          const r = await daemonCliGet<{ content: string }>('skill-layers/get-content', { tier, filename: layer.filename })
          return r.content || '*Empty layer.*'
        } catch {
          return '*Failed to load layer content.*'
        }
      },
    })
  }

  if (hasProjectContext) {
    entries.push({
      key: 'ws-project-context',
      name: 'Project Context',
      kind: 'ws',
      subtitle: '.k2/PROJECT.md',
      description: '**Workspace-scoped.** Shared codebase knowledge — tech stack, conventions, key directories. Injected into every agent launch via --append-system-prompt.',
      loadOnExpand: async () => {
        try {
          const r = await daemonCliGet<{ content: string }>('fs/read-file', { path: `${projectPath}/.k2/PROJECT.md` })
          return r.content || '*Empty file.*'
        } catch {
          return '*Failed to load PROJECT.md.*'
        }
      },
    })
  }

  for (const hb of heartbeats) {
    entries.push({
      key: `heartbeat-${hb.id}`,
      name: `${hb.name}`,
      kind: 'heartbeat',
      subtitle: `${hb.frequency} · WAKEUP.md`,
      description: `**Per-heartbeat wake trigger.** Shipped as the user message (positional argv) when this schedule fires. Expand to see the content, or edit from the Heartbeats panel above.`,
      editAction: onEditHeartbeat ? () => onEditHeartbeat(hb.name) : undefined,
      loadOnExpand: async () => {
        try {
          const r = await daemonCliGet<{ content: string }>('fs/read-file', { path: `${projectPath}/${hb.wakeupPath}` })
          return r.content || '*Empty WAKEUP.md.*'
        } catch {
          return '*Failed to load WAKEUP.md.*'
        }
      },
    })
  }

  const handleToggle = async (entry: LayerEntry) => {
    if (expanded === entry.key) { setExpanded(null); setExpandedBody(''); return }
    setExpanded(entry.key)
    if (entry.loadOnExpand) {
      setExpandedBody('*Loading…*')
      const body = await entry.loadOnExpand()
      setExpandedBody(body)
    } else {
      setExpandedBody(entry.description || '')
    }
  }

  return (
    <div>
      <div className="flex items-center justify-between mb-1">
        <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
          Context Layers
          <span className="ml-2 text-[9px] tabular-nums font-medium px-1.5 py-0.5 bg-white/5 text-[var(--color-text-muted)]">{entries.length}</span>
        </h3>
        {onOpenSettings && (
          <button
            onClick={onOpenSettings}
            className="text-[10px] text-[var(--color-accent)] hover:text-[var(--color-accent-hover)] no-drag cursor-pointer"
            title="Edit layers in Settings → Canonical Agent Flow"
          >
            Edit layers ↗
          </button>
        )}
      </div>
      <p className="text-[10px] text-[var(--color-text-muted)] mb-2 leading-relaxed">
        Everything that ships to the agent each wake. Click a layer to preview what's in it.
      </p>
      <div className="border border-[var(--color-border)]">
        {entries.map((entry) => {
          const isOpen = expanded === entry.key
          return (
            <React.Fragment key={entry.key}>
              <div
                onClick={() => handleToggle(entry)}
                className={`flex items-center justify-between px-3 py-1.5 border-b border-[var(--color-border)] last:border-b-0 cursor-pointer transition-colors ${
                  isOpen ? 'bg-white/[0.06]' : 'hover:bg-white/[0.03]'
                }`}
              >
                <div className="flex items-center gap-2 min-w-0">
                  <span className={`w-1 h-4 rounded-sm flex-shrink-0 ${
                    entry.kind === 'auto' ? 'bg-[var(--color-text-muted)]/30' :
                    entry.kind === 'global' ? 'bg-[var(--color-accent)]' :
                    entry.kind === 'ws' ? 'bg-amber-400/60' :
                    'bg-emerald-400/60'
                  }`} />
                  <div className="min-w-0">
                    <span className="text-xs text-[var(--color-text-primary)] block truncate">{entry.name}</span>
                    {entry.subtitle && (
                      <span className="text-[10px] text-[var(--color-text-muted)] block truncate">{entry.subtitle}</span>
                    )}
                  </div>
                </div>
                <KindBadge kind={entry.kind} />
              </div>
              {isOpen && (
                <div className="px-3 py-2 border-b border-[var(--color-border)] bg-black/20">
                  <div className="prose prose-invert prose-xs max-w-none text-[11px] text-[var(--color-text-secondary)] leading-relaxed">
                    <Markdown remarkPlugins={[remarkGfm]}>{expandedBody}</Markdown>
                  </div>
                  {entry.editAction && (
                    <button
                      onClick={(e) => { e.stopPropagation(); entry.editAction?.() }}
                      className="mt-2 text-[10px] text-[var(--color-accent)] hover:text-[var(--color-accent-hover)] no-drag cursor-pointer"
                    >
                      Edit ↗
                    </button>
                  )}
                </div>
              )}
            </React.Fragment>
          )
        })}
      </div>
    </div>
  )
}
