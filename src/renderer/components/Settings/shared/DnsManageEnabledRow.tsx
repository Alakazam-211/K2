import React from 'react'
import { useCallback } from 'react'
import { useSettingsStore } from '@/stores/settings'
import { Toggle } from '@/components/ui'
import { SettingRow } from '../controls/SettingControls'

// ── DNS K1 — "Allow agents to manage DNS records" ──────────────────
// Per-host opt-in that lets agents create/update/delete DNS records at
// the registrar / zone for this host. DEFAULTS OFF (deny-by-default):
// DNS mutation is high-impact, so a host must be explicitly opted in.
// The daemon enforces the gate server-side via
// `dns_manage_allowed_for_path` (app master OR per-workspace).
// Per-workspace override: Workspaces → (workspace) → "Allow agents to
// manage DNS for this workspace" — this app-level switch opts in ALL
// workspaces at once (global master).
//
// Home: Settings → K2 Connect, beneath the remote-access group — high-
// impact capability grants live together and are owner/admin gated
// (REMOTE_ACCESS_KEYS includes `dnsManageEnabled`).
export function DnsManageEnabledRow(): React.JSX.Element {
  const allow = useSettingsStore((s) => s.dnsManageEnabled)
  const setAllow = useSettingsStore((s) => s.setDnsManageEnabled)

  const toggle = useCallback(() => {
    void setAllow(!allow)
  }, [allow, setAllow])

  return (
    <SettingRow
      settingId="k2-connect.dns-manage-enabled"
      label="Agents can change DNS records"
      hint={
        allow
          ? 'Agents here can add, change and delete DNS records for this server. Workspaces can also turn this on one at a time.'
          : 'Off (recommended). When on, agents here can add, change and delete DNS records for this server. Workspaces can also turn this on one at a time.'
      }
    >
      <Toggle checked={allow} onChange={toggle} aria-label="Agents can change DNS records" />
    </SettingRow>
  )
}
