import React, { useCallback } from 'react'
import { useSettingsStore, normalizeConnectLoginIngress } from '@/stores/settings'
import type { ConnectLoginIngress } from '@shared/types'

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
    label: 'Only via K2 edge (recommended)',
    detail:
      'Password sign-in over the tunnel is accepted only through this server’s web address (its K2 edge). Direct hits on the tunnel get 404.',
  },
  {
    value: 'any',
    label: 'Any client',
    detail:
      'Any client may attempt password sign-in over the tunnel. Use only when this server is not behind the K2 edge (self-hosted tunnel).',
  },
  {
    value: 'off',
    label: 'Off',
    detail:
      'No password sign-in over the tunnel at all. Sign in locally or on the LAN; existing sessions and tokens keep working.',
  },
]

export function ConnectLoginIngressRow(): React.JSX.Element {
  const mode = useSettingsStore((s) => s.connectLoginIngress)
  const setMode = useSettingsStore((s) => s.setConnectLoginIngress)

  const onChange = useCallback(
    (e: React.ChangeEvent<HTMLSelectElement>) => {
      void setMode(normalizeConnectLoginIngress(e.target.value))
    },
    [setMode],
  )

  const current = CONNECT_LOGIN_INGRESS_OPTIONS.find((o) => o.value === mode) ?? CONNECT_LOGIN_INGRESS_OPTIONS[0]

  return (
    <div
      className="flex items-center justify-between gap-3 py-2"
      data-settings-id="k2-connect.login-ingress"
    >
      <div className="flex-1 min-w-0 mr-3">
        <span className="text-xs text-[var(--color-text-secondary)]">
          Password sign-in over the tunnel
        </span>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">{current.detail}</p>
      </div>
      <select
        value={mode}
        onChange={onChange}
        aria-label="Password sign-in over the tunnel"
        className="flex-shrink-0 px-2 py-1 text-[11px] bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag cursor-pointer"
      >
        {CONNECT_LOGIN_INGRESS_OPTIONS.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
    </div>
  )
}
