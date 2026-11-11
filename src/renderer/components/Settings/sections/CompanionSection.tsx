import React, { useCallback, useEffect, useState } from 'react'
import { getDaemonWs, daemonHttpBase } from '@/kessel/daemon-ws'
import { useSettingsStore } from '@/stores/settings'
import { serverSupports } from '@/lib/server-capabilities'
import { onTunnelStatusChanged, onAppHello } from '@/stores/session-events'
import type { SettingEntry } from '../searchManifest'
import appStoreBadge from '@/assets/app-store-badge.svg?url'
import googlePlayBadge from '@/assets/google-play-badge.png'
import { primaryScope } from '@/kessel/server-scope'
import { PhonePairing, QrCode } from '../shared/PhonePairing'
import { BetaTag, Button } from '@/components/ui'
import { SETTING_ROW_HINT, SettingRow, SettingsGroup, SettingsLoadState } from '../controls/SettingControls'
import { HealthWord, StatusDot } from './ConnectionsParts'
import { usePhonePairingFocus, usePhonePairingFocusStore } from '@/stores/phone-pairing-focus'

// K2 Companion (task #615) — the mobile app and the Tunnel share ONE
// tunnel (the frpc reverse tunnel at <sub>.k2.dev). The phone pairs to the
// same public URL the daemon is exposed at, so Companion runs no tunnel of
// its own. The page, in the order a person needs it: can a phone reach this
// server (a live read of GET /cli/tunnel/status, with a way to Tunnel when
// it can't), get the app (store QRs and badges), pair a phone, and the
// paired phones (the shared PhonePairing cards).

// Store listings linked from https://k2.dev (home footer badges).
export const APP_STORE_URL = 'https://apps.apple.com/us/app/k2so/id6762076766'
export const PLAY_STORE_URL =
  'https://play.google.com/store/apps/details?id=com.alakazamlabs.k2so.companion'

const CONTENT_WIDTH = 'max-w-[52rem]'

interface TunnelStatus {
  running: boolean
  public_url: string | null
  frpc_installed: boolean
}

/** The daemon's reason from a failed read, in its own words. */
function failReason(body: unknown, status: number): string {
  if (body && typeof body === 'object') {
    const e = (body as { error?: unknown }).error
    if (typeof e === 'string' && e.trim()) return e
    if (e && typeof e === 'object') {
      const r = e as { hint?: unknown; message?: unknown }
      if (typeof r.hint === 'string' && r.hint.trim()) return r.hint
      if (typeof r.message === 'string' && r.message.trim()) return r.message
    }
  }
  return `HTTP ${status}`
}

async function tunnelStatus(): Promise<TunnelStatus> {
  const creds = await getDaemonWs(primaryScope())
  const res = await fetch(
    `${daemonHttpBase(creds)}/cli/tunnel/status?token=${creds.token}`,
    { method: 'GET' },
  )
  if (!res.ok) {
    const body = await res.json().catch(() => null)
    throw new Error(failReason(body, res.status))
  }
  return (await res.json()) as TunnelStatus
}

export const COMPANION_MANIFEST: SettingEntry[] = [
  { id: 'companion.status', section: 'companion', label: 'Reachable from your phone', description: 'Whether your phone can reach this server, and its address', keywords: ['status', 'tunnel', 'connect', 'reachable', 'subdomain', 'k2.dev', 'pair', 'address'] },
  { id: 'companion.get-app', section: 'companion', label: 'Get the K2 mobile app', description: 'Install the K2 Companion app on iPhone or Android', keywords: ['mobile', 'companion', 'app', 'download', 'install', 'iphone', 'ios', 'app store', 'android', 'google play', 'qr'] },
  { id: 'companion.pair', section: 'companion', label: 'Pair a phone', description: 'A one-time code and QR that sign the K2 phone app in', keywords: ['pair', 'phone', 'qr', 'code', 'companion', 'mobile', 'device', 'iphone', 'android', 'web access'] },
  { id: 'companion.phones', section: 'companion', label: 'Paired phones', description: 'Every phone paired with this server, with Remove', keywords: ['paired', 'phones', 'devices', 'remove', 'revoke', 'sign out', 'iphone', 'android'] },
]

/** Bring the Pair a phone block into view and put the cursor on its first
 *  control. Looks for a few frames: the block lays out after mount. */
export function revealPhonePairing(): void {
  let tries = 0
  const reveal = (): void => {
    const el = document.querySelector<HTMLElement>('[data-settings-id="companion.pair"]')
    if (!el) {
      if (++tries < 30) requestAnimationFrame(reveal)
      return
    }
    el.scrollIntoView({ behavior: 'smooth', block: 'center' })
    el.querySelector<HTMLElement>('button:not([disabled]), input:not([disabled]), select:not([disabled])')?.focus({
      preventScroll: true,
    })
    el.classList.add('settings-search-pulse')
    window.setTimeout(() => el.classList.remove('settings-search-pulse'), 1500)
  }
  requestAnimationFrame(() => requestAnimationFrame(reveal))
}

export function CompanionSection(): React.JSX.Element {
  const [status, setStatus] = useState<TunnelStatus | null>(null)
  const [statusError, setStatusError] = useState<string | null>(null)
  const [checking, setChecking] = useState(false)
  const [copied, setCopied] = useState<'idle' | 'done' | 'failed'>('idle')

  const refresh = useCallback(async (): Promise<void> => {
    try {
      const s = await tunnelStatus()
      setStatus(s)
      setStatusError(null)
    } catch (e) {
      setStatusError(e instanceof Error ? e.message : String(e))
    }
  }, [])

  // The setup checklist's phone row: open on pairing (once per request).
  const pairingFocusSeq = usePhonePairingFocus()
  useEffect(() => {
    if (usePhonePairingFocusStore.getState().takeRequest()) revealPhonePairing()
  }, [pairingFocusSeq])

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
    const run = (): void => {
      if (!cancelled) void refresh()
    }
    run()

    if (serverSupports('daemon-broadcasts')) {
      const offHello = onAppHello(primaryScope(), run)
      const offTunnel = onTunnelStatusChanged(primaryScope(), (e) => {
        if (cancelled) return
        setStatusError(null)
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

    const interval = setInterval(run, 5000)
    return () => {
      cancelled = true
      clearInterval(interval)
    }
  }, [refresh])

  const publicUrl = status?.public_url ?? null
  const reachable = !!status?.running && !!publicUrl

  const goToTunnel = (): void => {
    useSettingsStore.getState().setSection('k2-connect')
  }
  const checkAgain = async (): Promise<void> => {
    setChecking(true)
    try {
      await refresh()
    } finally {
      setChecking(false)
    }
  }
  const copyAddress = async (url: string): Promise<void> => {
    try {
      await navigator.clipboard.writeText(url)
      setCopied('done')
    } catch {
      setCopied('failed')
    }
    setTimeout(() => setCopied('idle'), 2000)
  }

  // ── Status: can a phone reach this server? ──
  // A read that failed while an earlier answer is on screen keeps that
  // answer; only a first read that failed shows the error card.
  const statusState = !status ? (
    statusError ? (
      <SettingsLoadState
        kind="error"
        title="Couldn't check whether your phone can reach this server"
        detail={statusError}
        testId="companion-status-error"
        action={
          <Button onClick={() => void checkAgain()} disabled={checking}>
            {checking ? 'Checking…' : 'Try again'}
          </Button>
        }
      >
        K2 couldn&apos;t read the tunnel&apos;s status.
      </SettingsLoadState>
    ) : (
      <SettingsLoadState kind="loading" title="Checking whether your phone can reach this server…" testId="companion-status-loading" />
    )
  ) : undefined

  const notReachableHint =
    status && status.frpc_installed === false
      ? 'Phones reach this server through its tunnel, and the tunnel helper isn’t installed on this computer. Set it up in Tunnel.'
      : 'Phones reach this server through its tunnel, which is off. Turn it on in Tunnel, then pair your phone.'

  return (
    <div className={`w-full ${CONTENT_WIDTH} space-y-6 pb-6`}>
      <header className="min-w-0">
        <h2 className="type-title text-[var(--color-text-primary)] flex items-center gap-2">
          K2 Companion
          <BetaTag />
        </h2>
        <p className={SETTING_ROW_HINT}>Use K2 from your phone. Install the app, then pair it with this server.</p>
      </header>

      <SettingsGroup title="Status" card settingId="companion.status" testId="companion-status" state={statusState}>
        <SettingRow
          testId="companion-status-row"
          label={
            <span className="inline-flex items-center gap-2 flex-wrap" aria-live="polite">
              <StatusDot tone={reachable ? 'ok' : 'neutral'} />
              <HealthWord tone={reachable ? 'ok' : 'neutral'}>{reachable ? 'Reachable' : 'Not reachable from a phone'}</HealthWord>
              {reachable && (
                <a
                  href={publicUrl as string}
                  target="_blank"
                  rel="noreferrer"
                  data-k2-focus=""
                  className="relative px-0.5 -mx-0.5 font-mono text-[var(--color-accent-text)] hover:underline no-drag"
                >
                  {publicUrl}
                </a>
              )}
            </span>
          }
          hint={
            <>
              {reachable ? 'Your phone reaches this server at this address, at home or away.' : notReachableHint}
              {copied === 'failed' && (
                <span role="alert" className="block text-[var(--color-status-error-soft)]">
                  Couldn&apos;t copy the address. Select it and copy it by hand.
                </span>
              )}
            </>
          }
        >
          {reachable ? (
            <Button variant="quiet" className="flex-shrink-0" onClick={() => void copyAddress(publicUrl as string)}>
              {copied === 'done' ? 'Copied' : 'Copy address'}
            </Button>
          ) : (
            <Button variant="accent" className="flex-shrink-0" onClick={goToTunnel}>
              Open Tunnel
            </Button>
          )}
        </SettingRow>
      </SettingsGroup>

      <SettingsGroup
        title="Get the app"
        hint="Scan a code with your phone's camera, or open the store page."
        card
        settingId="companion.get-app"
      >
        <div className="px-4 py-4 flex flex-wrap gap-8">
          <StoreTile url={APP_STORE_URL} label="App Store" platform="iPhone" badgeSrc={appStoreBadge} />
          <StoreTile url={PLAY_STORE_URL} label="Google Play" platform="Android" badgeSrc={googlePlayBadge} />
        </div>
      </SettingsGroup>

      <PhonePairing mode="all" scope={primaryScope()} />
    </div>
  )
}

const STORE_TILE = 140

/** A store's QR above its badge: one link to the store page. */
function StoreTile({
  url,
  label,
  platform,
  badgeSrc,
}: {
  url: string
  label: string
  platform: string
  badgeSrc: string
}): React.JSX.Element {
  return (
    <a
      href={url}
      target="_blank"
      rel="noreferrer"
      data-k2-focus=""
      aria-label={`Get K2 for ${platform} on ${label}`}
      className="relative flex flex-col items-center gap-2 p-1.5 -m-1.5 no-drag cursor-pointer"
    >
      <QrCode url={url} size={STORE_TILE} border={2} label={`${label} QR code`} />
      {/* The two badges differ in height: one box keeps the captions level. */}
      <span className="flex items-center justify-center" style={{ width: STORE_TILE, height: 52 }}>
        <img src={badgeSrc} alt="" className="block h-auto" style={{ width: STORE_TILE }} />
      </span>
      <span className="type-hint text-[var(--color-text-secondary)]">For {platform}</span>
    </a>
  )
}
