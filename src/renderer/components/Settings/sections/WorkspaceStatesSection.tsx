import React from 'react'
import { useCallback, useEffect, useState } from 'react'
// Plan B — workspace states are host-aware daemon data: route them through
// the `/cli/states/*` HTTP layer (local OR remote) instead of the
// localhost-pinned Tauri `states_*` invoke proxy. The old Tauri shims
// (commands/states.rs) emitted NO cross-window sync event, so there is no
// `emit(...)` to mirror here — `loadStates()` after each mutation is the
// full contract.
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { CAP_STATES, CAP_LABELS, CAP_COLORS, CAPABILITIES, type StateData } from '@shared/constants/capabilities'
import type { SettingEntry } from '../searchManifest'

export const WORKSPACE_STATES_MANIFEST: SettingEntry[] = [
  { id: 'workspace-states.list', section: 'workspace-states', label: 'Workspace States', description: 'Capability states that control what agents do automatically per workspace', keywords: ['workspace', 'state', 'locked', 'managed', 'build', 'maintenance', 'heartbeat', 'capability'] },
  { id: 'workspace-states.new', section: 'workspace-states', label: 'New State', description: 'Create a custom workspace state', keywords: ['new', 'create', 'state', 'custom'] },
  { id: 'workspace-states.capabilities', section: 'workspace-states', label: 'Capabilities Explained', description: 'Features / Issues / Crashes / Security / Audits and auto vs gated vs off', keywords: ['capabilities', 'features', 'issues', 'crashes', 'security', 'audits', 'auto', 'gated'] },
]

export function WorkspaceStatesSection(): React.JSX.Element {
  const [states, setStates] = useState<StateData[]>([])
  const [editingState, setEditingState] = useState<StateData | null>(null)
  const [creating, setCreating] = useState(false)

  const loadStates = useCallback(async () => {
    try {
      const list = await daemonCliGet<StateData[]>('states/list')
      setStates(list)
    } catch (err) {
      console.error('[states] Failed to load:', err)
    }
  }, [])

  useEffect(() => { loadStates() }, [loadStates])

  const handleSave = async (entry: StateData) => {
    try {
      if (creating) {
        await daemonCliPost('states/create', {
          name: entry.name,
          description: entry.description,
          capFeatures: entry.capFeatures,
          capIssues: entry.capIssues,
          capCrashes: entry.capCrashes,
          capSecurity: entry.capSecurity,
          capAudits: entry.capAudits,
          heartbeat: entry.heartbeat === 1,
        })
      } else {
        await daemonCliPost('states/update', {
          id: entry.id,
          name: entry.name,
          description: entry.description,
          capFeatures: entry.capFeatures,
          capIssues: entry.capIssues,
          capCrashes: entry.capCrashes,
          capSecurity: entry.capSecurity,
          capAudits: entry.capAudits,
          heartbeat: entry.heartbeat === 1,
        })
      }
      setEditingState(null)
      setCreating(false)
      loadStates()
    } catch (err) {
      console.error('[states] Save failed:', err)
    }
  }

  const handleDelete = async (id: string) => {
    try {
      await daemonCliPost('states/delete', { id })
      loadStates()
    } catch (err) {
      console.error('[states] Delete failed:', err)
    }
  }

  const handleNew = () => {
    setCreating(true)
    setEditingState({
      id: '',
      name: 'New State',
      description: '',
      isBuiltIn: 0,
      capFeatures: 'gated',
      capIssues: 'gated',
      capCrashes: 'auto',
      capSecurity: 'auto',
      capAudits: 'off',
      heartbeat: 1,
      sortOrder: states.length,
    })
  }

  // Editor view
  if (editingState) {
    return (
      <div className="max-w-lg">
        <button
          onClick={() => { setEditingState(null); setCreating(false) }}
          className="text-xs text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] mb-4 no-drag cursor-pointer"
        >
          &larr; Back to states
        </button>

        <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-4">
          {creating ? 'Create State' : `Edit: ${editingState.name}`}
        </h2>

        {/* Name */}
        <div className="mb-3">
          <label className="text-[10px] text-[var(--color-text-muted)] block mb-1">Name</label>
          <input
            value={editingState.name}
            onChange={(e) => setEditingState({ ...editingState, name: e.target.value })}
            className="w-full px-2 py-1 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)]"
            disabled={editingState.isBuiltIn === 1}
          />
        </div>

        {/* Description */}
        <div className="mb-4">
          <label className="text-[10px] text-[var(--color-text-muted)] block mb-1">Description</label>
          <input
            value={editingState.description || ''}
            onChange={(e) => setEditingState({ ...editingState, description: e.target.value })}
            className="w-full px-2 py-1 text-xs bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)]"
          />
        </div>

        {/* Capabilities */}
        <div className="mb-4">
          <label className="text-[10px] text-[var(--color-text-muted)] block mb-2">Capabilities</label>
          <div className="space-y-2">
            {CAPABILITIES.map((cap) => (
              <div key={cap.key} className="flex items-center justify-between py-1.5 px-3 bg-[var(--color-bg-elevated)] border border-[var(--color-border)]">
                <div>
                  <span className="text-xs text-[var(--color-text-primary)]">{cap.label}</span>
                  <p className="text-[9px] text-[var(--color-text-muted)]">{cap.desc}</p>
                </div>
                <div className="flex gap-0.5">
                  {CAP_STATES.map((state) => (
                    <button
                      key={state}
                      onClick={() => setEditingState({ ...editingState, [cap.key]: state })}
                      className={`px-2 py-0.5 text-[9px] border transition-colors cursor-pointer no-drag ${
                        editingState[cap.key] === state
                          ? `border-[var(--color-accent)] ${CAP_COLORS[state]} bg-[var(--color-accent)]/10`
                          : 'border-[var(--color-border)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
                      }`}
                    >
                      {CAP_LABELS[state]}
                    </button>
                  ))}
                </div>
              </div>
            ))}
          </div>
        </div>

        {/* Heartbeat toggle */}
        <div className="flex items-center justify-between mb-6 py-1.5 px-3 bg-[var(--color-bg-elevated)] border border-[var(--color-border)]">
          <div>
            <span className="text-xs text-[var(--color-text-primary)]">Heartbeat</span>
            <p className="text-[9px] text-[var(--color-text-muted)]">Whether agents can wake up automatically</p>
          </div>
          <button
            onClick={() => setEditingState({ ...editingState, heartbeat: editingState.heartbeat ? 0 : 1 })}
            className={`w-8 h-4 flex items-center transition-colors no-drag cursor-pointer ${
              editingState.heartbeat ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
            }`}
          >
            <span className={`w-3 h-3 bg-white block transition-transform ${
              editingState.heartbeat ? 'translate-x-4.5' : 'translate-x-0.5'
            }`} />
          </button>
        </div>

        {/* Save button */}
        <button
          onClick={() => handleSave(editingState)}
          className="px-4 py-1.5 text-xs bg-[var(--color-accent)] text-white hover:opacity-90 transition-opacity no-drag cursor-pointer"
        >
          {creating ? 'Create State' : 'Save Changes'}
        </button>
      </div>
    )
  }

  // List view
  return (
    <div>
      <div className="flex items-center justify-between mb-4">
        <div>
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">Workspace States</h2>
          <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
            Define capability states that control what agents can do automatically per workspace.
          </p>
        </div>
        <button
          onClick={handleNew}
          className="px-3 py-1 text-[10px] text-[var(--color-accent)] border border-[var(--color-accent)]/30 hover:bg-[var(--color-accent)]/10 transition-colors no-drag cursor-pointer"
        >
          + New State
        </button>
      </div>

      {/* Capability columns */}
      <div className="mb-4">
        <span className="text-[10px] text-[var(--color-text-muted)] uppercase tracking-wider block mb-1.5">Capability Columns</span>
        <div className="space-y-1">
          {CAPABILITIES.map((cap) => (
            <div key={cap.key} className="flex items-baseline gap-2 text-[11px]">
              <span className="text-[var(--color-text-secondary)] font-medium w-16 flex-shrink-0">{cap.label}</span>
              <span className="text-[var(--color-text-muted)]">{cap.desc}</span>
            </div>
          ))}
        </div>
      </div>

      {/* Status levels */}
      <div className="mb-4">
        <span className="text-[10px] text-[var(--color-text-muted)] uppercase tracking-wider block mb-1.5">Status Levels</span>
        <div className="space-y-1">
          <div className="flex items-center gap-2 text-[11px]">
            <span className="w-2 h-2 rounded-full bg-green-400 flex-shrink-0" />
            <span className="text-[var(--color-text-secondary)] font-medium w-16 flex-shrink-0">Auto</span>
            <span className="text-[var(--color-text-muted)]">Agents handle this automatically without human approval</span>
          </div>
          <div className="flex items-center gap-2 text-[11px]">
            <span className="w-2 h-2 rounded-full bg-amber-400 flex-shrink-0" />
            <span className="text-[var(--color-text-secondary)] font-medium w-16 flex-shrink-0">Gated</span>
            <span className="text-[var(--color-text-muted)]">Requires human approval before agents act</span>
          </div>
          <div className="flex items-center gap-2 text-[11px]">
            <span className="w-2 h-2 rounded-full bg-[var(--color-text-muted)] flex-shrink-0" />
            <span className="text-[var(--color-text-secondary)] font-medium w-16 flex-shrink-0">Off</span>
            <span className="text-[var(--color-text-muted)]">Not functioning for this capability</span>
          </div>
        </div>
      </div>

      {/* State comparison table */}
      <div className="border border-[var(--color-border)] overflow-hidden">
        {/* Header */}
        <div className="grid gap-0 text-[var(--color-text-muted)] bg-[var(--color-bg-surface)]" style={{ gridTemplateColumns: '2fr repeat(5, 100px)' }}>
          <div className="px-4 py-2">
            <span className="text-[11px] font-medium">State</span>
          </div>
          {CAPABILITIES.map((cap) => (
            <div key={cap.key} className="px-2 py-2 text-center">
              <span className="text-[11px] font-medium">{cap.label}</span>
            </div>
          ))}
        </div>

        {/* Rows */}
        {states.map((entry) => (
          <div
            key={entry.id}
            className="grid gap-0 border-t border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)]/50 group"
            style={{ gridTemplateColumns: '2fr repeat(5, 100px)' }}
          >
            <div className="px-4 py-3 flex items-start gap-2">
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2">
                  <span className={`w-2 h-2 rounded-full flex-shrink-0 ${entry.heartbeat ? 'bg-green-400' : 'bg-[var(--color-text-muted)]'}`} />
                  <span className="text-[12px] font-medium text-[var(--color-text-primary)]">{entry.name}</span>
                  {entry.isBuiltIn === 1 && (
                    <span className="text-[9px] text-[var(--color-text-muted)] px-1 py-0.5 border border-[var(--color-border)] flex-shrink-0">DEFAULT</span>
                  )}
                </div>
                {entry.description && (
                  <p className="text-[11px] text-[var(--color-text-muted)] mt-1 pl-4 leading-relaxed">{entry.description}</p>
                )}
              </div>
              <div className="flex gap-1.5 opacity-0 group-hover:opacity-100 transition-opacity flex-shrink-0 mt-0.5">
                <button
                  onClick={() => setEditingState(entry)}
                  className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-accent)] no-drag cursor-pointer"
                >
                  Edit
                </button>
                {entry.isBuiltIn === 0 && (
                  <button
                    onClick={() => handleDelete(entry.id)}
                    className="text-[10px] text-red-400/50 hover:text-red-400 no-drag cursor-pointer"
                  >
                    Delete
                  </button>
                )}
              </div>
            </div>
            {CAPABILITIES.map((cap) => {
              const capState = entry[cap.key] as string
              return (
                <div key={cap.key} className="px-2 py-3 flex items-center justify-center">
                  <span className={`text-[11px] font-medium ${CAP_COLORS[capState] || ''}`}>
                    {CAP_LABELS[capState] || capState}
                  </span>
                </div>
              )
            })}
          </div>
        ))}
      </div>
    </div>
  )
}
