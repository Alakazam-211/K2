import React, { useEffect, useMemo, useState } from 'react'
import { renderSVG } from 'uqr'
import { getDaemonWs, daemonHttpBase } from '@/kessel/daemon-ws'
import { useSettingsStore } from '@/stores/settings'
import { serverSupports } from '@/lib/server-capabilities'
import { onTunnelStatusChanged, onAppHello } from '@/stores/session-events'
import type { SettingEntry } from '../searchManifest'

// K2 Companion (task #615) — the mobile app and K2 Connect now share ONE
// tunnel (the frpc reverse tunnel exposed by K2 Connect at <sub>.k2.dev).
// The phone pairs to the same public URL the daemon is exposed at, so
// Companion no longer runs its own ngrok tunnel. This section is now just
// (a) a link to install the mobile app and (b) a live read of the K2
// Connect tunnel status (GET /cli/tunnel/status), reusing the same
// direct-daemon fetch pattern as K2ConnectSection.tsx.

// Store listings linked from https://k2.dev (home footer badges).
export const APP_STORE_URL = 'https://apps.apple.com/us/app/k2so/id6762076766'
export const PLAY_STORE_URL =
  'https://play.google.com/store/apps/details?id=com.alakazamlabs.k2so.companion'

interface TunnelStatus {
  running: boolean
  public_url: string | null
  frpc_installed: boolean
}

async function tunnelStatus(): Promise<TunnelStatus> {
  const creds = await getDaemonWs()
  const res = await fetch(
    `${daemonHttpBase(creds)}/cli/tunnel/status?token=${creds.token}`,
    { method: 'GET' },
  )
  if (!res.ok) throw new Error(`tunnel status ${res.status}`)
  return (await res.json()) as TunnelStatus
}

export const COMPANION_MANIFEST: SettingEntry[] = [
  { id: 'companion.get-app', section: 'companion', label: 'Get the K2 mobile app', description: 'Install the K2 Companion app on iPhone or Android', keywords: ['mobile', 'companion', 'app', 'download', 'install', 'iphone', 'ios', 'app store', 'android', 'google play', 'qr'] },
  { id: 'companion.status', section: 'companion', label: 'K2 Connect Status', description: 'Whether your daemon is reachable for the mobile app', keywords: ['status', 'tunnel', 'connect', 'reachable', 'subdomain', 'k2.dev', 'pair'] },
]

export function CompanionSection(): React.JSX.Element {
  const [status, setStatus] = useState<TunnelStatus | null>(null)

  // 0.39.39 (#675.5) — push-primary with snapshot-on-connect. The 5s tunnel-
  // status poll is replaced by a subscription to the app-level
  // `tunnel_status_changed` broadcast; the event carries `running` + the
  // predicted `publicUrl` so we render without a follow-up GET (we preserve
  // the last-snapshot `frpc_installed`, which the event doesn't carry). One
  // snapshot on mount + a re-snapshot on each WS (re)connect (`onAppHello`)
  // keeps current truth. Against an OLDER / REMOTE daemon without broadcasts
  // we KEEP the 5s poll fallback.
  useEffect(() => {
    let cancelled = false
    const refresh = async () => {
      try {
        const s = await tunnelStatus()
        if (!cancelled) setStatus(s)
      } catch {
        /* ignore — leave previous status / null */
      }
    }
    void refresh()

    if (serverSupports('daemon-broadcasts')) {
      const offHello = onAppHello(() => void refresh())
      const offTunnel = onTunnelStatusChanged((e) => {
        if (cancelled) return
        setStatus((prev) => ({
          running: e.running,
          public_url: e.publicUrl,
          // The event doesn't carry frpc_installed — keep the last snapshot's
          // value (defaults true once running, false otherwise, until the next
          // snapshot corrects it).
          frpc_installed: prev?.frpc_installed ?? e.running,
        }))
      })
      return () => {
        cancelled = true
        offHello()
        offTunnel()
      }
    }

    const interval = setInterval(() => void refresh(), 5000)
    return () => {
      cancelled = true
      clearInterval(interval)
    }
  }, [])

  const running = status?.running ?? false
  const publicUrl = status?.public_url ?? null
  const connected = running && !!publicUrl

  const goToK2Connect = () => {
    useSettingsStore.getState().setSection('k2-connect')
  }

  return (
    <div className="max-w-xl">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1 flex items-center gap-2">
        K2 Companion
        <span
          className="text-[8px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[var(--color-accent)]/15 text-[var(--color-accent)]"
          title="This feature is in beta — interface and behavior may change"
        >
          beta
        </span>
      </h2>
      <p className="text-[10px] text-[var(--color-text-muted)] mb-4">
        Reach your K2 agents from your phone. The mobile app pairs to the same
        public address K2 Connect exposes this daemon at — one tunnel, both
        clients. No separate setup here.
      </p>

      {/* ── Get the app ───────────────────────────────────────────── */}
      <div
        className="mb-4 px-3 py-3 border border-[var(--color-border)]"
        data-settings-id="companion.get-app"
      >
        <div className="text-xs text-[var(--color-text-primary)]">Get the K2 mobile app</div>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5 mb-3">
          Scan a code with your phone, or open the store page. Then pair the app
          to your daemon&apos;s public address (shown below).
        </p>
        <div className="flex flex-wrap gap-6">
          <StoreQrCard url={APP_STORE_URL} label="App Store" />
          <StoreQrCard url={PLAY_STORE_URL} label="Google Play" />
        </div>
      </div>

      {/* ── K2 Connect status (live) ──────────────────────────────── */}
      <div data-settings-id="companion.status">
        {connected ? (
          <div className="px-3 py-3 border border-[color-mix(in_srgb,var(--color-status-ok)_30%,transparent)] bg-[color-mix(in_srgb,var(--color-status-ok)_5%,transparent)]">
            <div className="flex items-center gap-2">
              <span className="w-2 h-2 flex-shrink-0 rounded-full" style={{ backgroundColor: 'var(--color-status-ok)' }} />
              <span className="text-xs text-[var(--color-text-secondary)]">Connected</span>
            </div>
            <p className="text-[10px] text-[var(--color-text-muted)] mt-2 leading-relaxed">
              Your daemon is reachable at the address below. Open the K2 app and
              pair to it.
            </p>
            <a
              href={publicUrl!}
              target="_blank"
              rel="noreferrer"
              className="inline-block mt-1.5 text-xs text-[var(--color-accent)] hover:underline font-mono break-all no-drag cursor-pointer"
            >
              {publicUrl}
            </a>
          </div>
        ) : (
          <div className="px-3 py-3 border border-[var(--color-border)]">
            <div className="flex items-center gap-2">
              <span className="w-2 h-2 flex-shrink-0 rounded-full" style={{ backgroundColor: 'var(--color-neutral)' }} />
              <span className="text-xs text-[var(--color-text-secondary)]">K2 Connect isn&apos;t running</span>
            </div>
            <p className="text-[10px] text-[var(--color-text-muted)] mt-2 leading-relaxed">
              The mobile app reaches this daemon through the K2 Connect tunnel.
              Start it to get a <span className="font-mono">https://&lt;sub&gt;.k2.dev</span> address
              you can pair to.
            </p>
            <button
              onClick={goToK2Connect}
              className="mt-2 text-[11px] text-[var(--color-accent)] hover:underline no-drag cursor-pointer"
            >
              Set up K2 Connect →
            </button>
          </div>
        )}
      </div>
    </div>
  )
}

function StoreQrCard({ url, label }: { url: string; label: string }): React.JSX.Element {
  const svg = useMemo(
    () =>
      renderSVG(url, {
        border: 1,
        ecc: 'M',
        blackColor: '#111111',
        whiteColor: '#ffffff',
      }),
    [url],
  )
  return (
    <a
      href={url}
      target="_blank"
      rel="noreferrer"
      className="flex flex-col items-center gap-1.5 no-drag cursor-pointer group"
    >
      <div
        className="bg-white p-1.5 border border-[var(--color-border)] [&>svg]:block [&>svg]:h-full [&>svg]:w-full"
        style={{ width: 272, height: 272 }}
        role="img"
        aria-label={`${label} QR code`}
        dangerouslySetInnerHTML={{ __html: svg }}
      />
      <span className="text-[11px] text-[var(--color-accent)] group-hover:underline">{label}</span>
    </a>
  )
}
