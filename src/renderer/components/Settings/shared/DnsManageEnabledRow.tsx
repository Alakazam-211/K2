import React from 'react'
import { useCallback } from 'react'
import { useSettingsStore } from '@/stores/settings'
import { Toggle } from '@/components/ui'

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
    <div className="flex items-center justify-between py-2" data-settings-id="k2-connect.dns-manage-enabled">
      <div className="flex-1 min-w-0 mr-3">
        <span className="text-xs text-[var(--color-text-secondary)]">
          Allow agents to manage DNS records
        </span>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
          {allow
            ? 'Agents on this host may create, update, and delete DNS records. Each workspace can also opt in individually under Workspaces.'
            : 'Off (recommended): agents cannot mutate DNS. Turn on to let agents manage DNS records for this host — each workspace can also opt in individually under Workspaces.'}
        </p>
      </div>
      <Toggle checked={allow} onChange={toggle} aria-label="Allow agents to manage DNS records" />
    </div>
  )
}
