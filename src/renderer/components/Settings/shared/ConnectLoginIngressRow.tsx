import React, { useCallback } from 'react'
import { useSettingsStore, normalizeConnectLoginIngress } from '@/stores/settings'
import type { ConnectLoginIngress } from '@shared/types'
import { SettingDropdown, SettingRow } from '../controls/SettingControls'

// ── S1 (PRD connect-login-edge-only) — "Password sign-in over the tunnel" ──
// Where the daemon answers `POST /cli/auth/login` on TUNNEL ingress:
//   edge (default) — only requests attested by a K2 first-party edge (the
//                    `<sub>.app.k2.dev` Worker / k2.dev dashboard). Scanners
//                    hitting `<sub>.k2.dev` directly get 404.
//   any            — any client (the pre-gate behaviour; for self-hosters
//                    whose tunnel is not behind the K2 edge).
//   off            — never on the tunnel. Local/LAN sign-in and existing
//                    tokens keep working.
// Local / LAN ingress is never affected by this switch.
//
// Home: Settings → K2 Connect → Policies, beside the other host-scoped
// remote-access grants (all in REMOTE_ACCESS_KEYS → owner/admin gated).
// Same optimistic persistAndApply pattern as its siblings.

export const CONNECT_LOGIN_INGRESS_OPTIONS: ReadonlyArray<{
  value: ConnectLoginIngress
  label: string
  detail: string
}> = [
  {
    value: 'edge',
    label: 'Only at its k2.dev address',
    detail:
      'Recommended. People can sign in with a password only through this server’s k2.dev web address. Other attempts are refused.',
  },
  {
    value: 'any',
    label: 'From any app',
    detail:
      'Any app can try a password sign-in through the tunnel. Use this only with a tunnel you host yourself.',
  },
  {
    value: 'off',
    label: 'Off',
    detail:
      'No password sign-in from the internet. Sign in on this computer or your local network; apps already signed in keep working.',
  },
]

export function ConnectLoginIngressRow(): React.JSX.Element {
  const mode = useSettingsStore((s) => s.connectLoginIngress)
  const setMode = useSettingsStore((s) => s.setConnectLoginIngress)

  const onChange = useCallback(
    (value: string) => {
      void setMode(normalizeConnectLoginIngress(value))
    },
    [setMode],
  )

  const current = CONNECT_LOGIN_INGRESS_OPTIONS.find((o) => o.value === mode) ?? CONNECT_LOGIN_INGRESS_OPTIONS[0]

  return (
    <SettingRow
      settingId="k2-connect.login-ingress"
      label="Password sign-in from the internet"
      hint={current.detail}
      field
    >
      <SettingDropdown
        value={mode}
        onChange={onChange}
        ariaLabel="Password sign-in from the internet"
        fullWidth
        options={CONNECT_LOGIN_INGRESS_OPTIONS.map((o) => ({
          value: o.value,
          label: o.label,
        }))}
      />
    </SettingRow>
  )
}
