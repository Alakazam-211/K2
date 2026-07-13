import React from 'react'
import { useCallback } from 'react'
import { useSettingsStore } from '@/stores/settings'
import { Toggle } from '@/components/ui'

// ── C1 (0.40.45) — "Allow agents to create connections" ─────────────
// Per-host opt-in that lets agents add/remove workspace connections
// (`k2 connections add|remove`, relations create/delete). DEFAULTS OFF
// (deny-by-default): wiring two workspaces is a high-impact trust
// decision, so a host must be explicitly opted in. The owner (and
// Owner/Admin connect-users) can always add/remove connections
// regardless. The daemon enforces the gate server-side via
// `agents_can_create_connections_for_path` (app master OR per-workspace).
// Per-workspace override: Workspaces → (workspace) → "Allow agents to
// create connections for this workspace" — this app-level switch opts
// in ALL workspaces at once (global master).
//
// Home: Settings → K2 Connect, beneath the remote-access group —
// high-impact capability grants live together and are owner/admin gated
// (REMOTE_ACCESS_KEYS includes `agentsCanCreateConnections`).
export function AgentsCanCreateConnectionsRow(): React.JSX.Element {
  const allow = useSettingsStore((s) => s.agentsCanCreateConnections)
  const setAllow = useSettingsStore((s) => s.setAgentsCanCreateConnections)

  const toggle = useCallback(() => {
    void setAllow(!allow)
  }, [allow, setAllow])

  return (
    <div
      className="flex items-center justify-between py-2"
      data-settings-id="k2-connect.agents-can-create-connections"
    >
      <div className="flex-1 min-w-0 mr-3">
        <span className="text-xs text-[var(--color-text-secondary)]">
          Allow agents to create connections
        </span>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
          {allow
            ? 'Agents on this host may add and remove workspace connections. Each workspace can also opt in individually under Workspaces. You (the owner) can always manage connections.'
            : 'Off (recommended): agents cannot add or remove connections. Turn on to let agents wire workspaces on this host — each workspace can also opt in individually under Workspaces. You (the owner) can always manage connections.'}
        </p>
      </div>
      <Toggle
        checked={allow}
        onChange={toggle}
        aria-label="Allow agents to create connections"
      />
    </div>
  )
}
