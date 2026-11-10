import React from 'react'
import { useSettingsStore } from '@/stores/settings'
import { Toggle } from '@/components/ui'
import { SettingRow } from '../controls/SettingControls'

// ── PRD boot-status-ghost §2.3 — "Hide this server's details from the internet" ──
// What `/boot-status` tells a caller with no valid sign-in on the tunnel,
// the app edge, a front door or the LAN:
//   off (compat, the 0.46.3 default) — phase, restart id, version and
//        feature list (older phones read those without signing in);
//   on  (ghost) — only "is it up" and the protocol range.
// This computer and anyone signed in always get the full status, so the
// app, phones on 4.0.4+, updates and reconnects keep working. Thin client:
// it reads and writes the daemon setting only.
//
// Home: Settings → K2 Connect → Policies, beside the login-ingress row
// (both REMOTE_ACCESS_KEYS → owner/admin gated).

export const BOOT_STATUS_PUBLIC_DETAIL = {
  compat:
    'Anyone can see that K2 is running here, its version and its features. Older phone apps need this.',
  ghost:
    'Visitors who aren\u2019t signed in see only that the server is up. Signed-in apps and phones still get everything; phone apps older than 4.0.4 lose newer features until they update.',
} as const

export function BootStatusPublicRow(): React.JSX.Element {
  const mode = useSettingsStore((s) => s.bootStatusPublic)
  const setMode = useSettingsStore((s) => s.setBootStatusPublic)
  return (
    <SettingRow
      settingId="k2-connect.boot-status-public"
      label="Hide this server's details from the internet"
      hint={BOOT_STATUS_PUBLIC_DETAIL[mode]}
    >
      <Toggle
        checked={mode === 'ghost'}
        onChange={(next) => void setMode(next ? 'ghost' : 'compat')}
        aria-label="Hide this server's details from the internet"
      />
    </SettingRow>
  )
}
