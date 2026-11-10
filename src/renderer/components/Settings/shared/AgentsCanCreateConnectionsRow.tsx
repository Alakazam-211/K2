import React from 'react'
import { useCallback } from 'react'
import { useSettingsStore } from '@/stores/settings'
import { Toggle } from '@/components/ui'
import { SettingRow } from '../controls/SettingControls'

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
    <SettingRow
      settingId="k2-connect.agents-can-create-connections"
      label="Agents can add connections"
      hint={
        allow
          ? 'Agents here can add and remove connections between workspaces. Workspaces can also turn this on one at a time. You can always manage connections yourself.'
          : 'Off (recommended). When on, agents here can add and remove connections between workspaces. You can always manage connections yourself.'
      }
    >
      <Toggle checked={allow} onChange={toggle} aria-label="Agents can add connections" />
    </SettingRow>
  )
}
