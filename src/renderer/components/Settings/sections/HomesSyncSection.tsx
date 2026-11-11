// Settings → Network → Homes Sync (homes-sync-v1, 0.46.1).
//
// Rosson (ticket 4f537560): one home server per person, local-first. The
// desktop syncs its Homes with its own daemon; a phone picks its server and
// that server lists the devices syncing from it. Here: this computer's sync
// state, "Change Home Sync" / "Sync again", and the devices syncing their Homes
// from this computer, each with Remove. No "personal device" labels.

import React, { useCallback, useEffect, useState } from 'react'
import { Button } from '@/components/ui'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { onHomesDeviceRemoved, onHomesChanged } from '@/stores/session-events'
import { scopeForHost } from '@/kessel/server-scope'
import {
  parseDevices,
  readControl,
  startSyncing,
  statusLine,
  stopSyncing,
  useHomesSyncStatus,
  type HomesSyncTransport,
  type KeyValue,
  type SyncedDevice,
} from '@/lib/homes-sync/runner'
import { localKv, localTransport } from '@/lib/homes-sync'
import { parseSyncState } from '@/lib/homes-sync/engine'
import { HOMES_SYNC_STORAGE_KEY } from '@/stores/homes'
import type { SettingEntry } from '../searchManifest'

export const HOMES_SYNC_MANIFEST: SettingEntry[] = [
  {
    id: 'homes-sync.devices',
    section: 'homes-sync',
    label: 'Homes Sync',
    description: 'Keep your Homes in step between this computer and your phone; see and remove synced devices',
    keywords: ['homes', 'home', 'sync', 'devices', 'phone', 'companion', 'my home'],
  },
]

const PLATFORM_LABEL: Record<string, string> = {
  ios: 'iPhone / iPad',
  android: 'Android',
  macos: 'Mac',
  windows: 'Windows',
  linux: 'Linux',
  web: 'Browser',
}

function ago(ms: number | null, now: number): string {
  if (ms === null) return 'never synced'
  const s = Math.max(0, Math.round((now - ms) / 1000))
  if (s < 60) return 'synced just now'
  const m = Math.round(s / 60)
  if (m < 60) return `synced ${m} min ago`
  const h = Math.round(m / 60)
  if (h < 48) return `synced ${h} h ago`
  return `synced ${Math.round(h / 24)} days ago`
}

export function HomesSyncSection(props: {
  transport?: HomesSyncTransport
  kv?: KeyValue | null
}): React.JSX.Element {
  // Stable for the page's life (a new object per render would re-run the
  // device fetch on every render).
  const [transport] = useState<HomesSyncTransport>(() => props.transport ?? localTransport())
  const [kv] = useState<KeyValue | null>(() => (props.kv !== undefined ? props.kv : localKv()))
  const status = useHomesSyncStatus()
  const [devices, setDevices] = useState<SyncedDevice[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [enabled, setEnabled] = useState(() => readControl(kv).enabled)
  const supported = transport.supported()

  const deviceId = parseSyncState(kv?.getItem(HOMES_SYNC_STORAGE_KEY) ?? null)?.deviceId ?? ''

  const refresh = useCallback(async (): Promise<void> => {
    if (!supported) return
    try {
      setDevices(parseDevices(await transport.listDevices(deviceId)))
      setError(null)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [supported, transport, deviceId])

  useEffect(() => {
    void refresh()
    const scope = scopeForHost('local')
    const offs = [onHomesDeviceRemoved(scope, () => void refresh()), onHomesChanged(scope, () => void refresh())]
    return () => offs.forEach((off) => off())
  }, [refresh])

  // Rosson 11-11: "Stop syncing" read as a scare word. The button says
  // "Change Home Sync"; turning it off is a choice inside, with what it
  // keeps spelled out.
  const change = async (): Promise<void> => {
    const ok = await useConfirmDialogStore.getState().confirm({
      title: 'Change Home Sync',
      message:
        'This computer keeps its Homes in step with your phone through this computer’s K2 server. You can turn that off for this computer: its Homes stay here, nothing is deleted, and you can turn it back on at any time.',
      confirmLabel: 'Turn off for this computer',
    })
    if (ok) await stop()
  }

  const stop = async (): Promise<void> => {
    setBusy(true)
    try {
      await stopSyncing(kv, transport)
      setEnabled(false)
      await refresh()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  const start = (): void => {
    startSyncing(kv)
    setEnabled(true)
    // The runner registers this computer; the list follows.
    setTimeout(() => void refresh(), 1500)
  }

  const remove = async (d: SyncedDevice): Promise<void> => {
    const ok = await useConfirmDialogStore.getState().confirm({
      title: 'Remove device',
      message: d.current
        ? 'Take this computer off Homes Sync? Its Homes stay as they are here.'
        : `Take “${d.name}” off Homes Sync? Its Homes stay on that device, and it can sync again later from its own settings.`,
      confirmLabel: 'Remove',
      destructive: true,
    })
    if (!ok) return
    setBusy(true)
    try {
      if (d.current) {
        await stopSyncing(kv, transport)
        setEnabled(false)
      } else {
        await transport.remove(d.deviceId)
      }
      await refresh()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  const now = Date.now()
  return (
    <div className="max-w-2xl" data-settings-id="homes-sync.devices" data-testid="homes-sync-section">
      <h2 className="type-title text-[var(--color-text-primary)] mb-1">Homes Sync</h2>
      <p className="type-hint text-[var(--color-text-muted)] mb-4">
        Your Homes stay in step between this computer and your phone through this computer’s K2 server. Every device keeps
        its own copy, so Home always opens, even offline; changes made while apart are combined when they meet again.
        Which Home you’re looking at stays on each device. Your prompt stash syncs the same way, with these devices; its
        files stay on the device that saved them.
      </p>

      {!supported ? (
        <p className="type-row text-[var(--color-text-secondary)]" data-testid="homes-sync-unsupported">
          This computer’s K2 server doesn’t sync Homes yet. Update K2 to turn it on.
        </p>
      ) : (
        <>
          <div className="flex items-center justify-between gap-3 border border-[var(--color-border)] px-3 py-2 mb-4">
            <div className="min-w-0">
              <div className="type-row text-[var(--color-text-primary)]">This computer</div>
              <div className="type-hint text-[var(--color-text-muted)]" data-testid="homes-sync-status">
                {enabled ? statusLine(status) : statusLine({ ...status, phase: 'off' })}
              </div>
            </div>
            {enabled && status.phase !== 'removed' ? (
              <Button variant="ghost" disabled={busy} onClick={() => void change()}>
                Change Home Sync
              </Button>
            ) : (
              <Button variant="accent" disabled={busy} onClick={start}>
                {status.phase === 'removed' ? 'Sync again' : 'Start syncing'}
              </Button>
            )}
          </div>

          <h3 className="type-heading text-[var(--color-text-primary)] mb-1">Devices syncing from this computer</h3>
          <p className="type-hint text-[var(--color-text-muted)] mb-2">
            Phones and computers that keep their Homes here. Removing one stops its sync; its Homes stay on it.
          </p>
          {error && (
            <p className="type-hint text-[var(--color-status-error,#e5484d)] mb-2" role="alert">
              {error}
            </p>
          )}
          {devices === null ? (
            <p className="type-hint text-[var(--color-text-muted)]">Loading…</p>
          ) : devices.length === 0 ? (
            <p className="type-hint text-[var(--color-text-muted)]" data-testid="homes-sync-no-devices">
              No devices sync their Homes here yet.
            </p>
          ) : (
            <ul className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]">
              {devices.map((d) => (
                <li key={d.deviceId} className="flex items-center justify-between gap-3 px-3 py-2" data-testid="homes-sync-device">
                  <div className="min-w-0">
                    <div className="type-row text-[var(--color-text-primary)] truncate">
                      {d.name}
                      {d.current && <span className="ml-2 type-hint text-[var(--color-text-muted)]">(this computer)</span>}
                    </div>
                    <div className="type-hint text-[var(--color-text-muted)]">
                      {PLATFORM_LABEL[d.platform] ?? d.platform} · {ago(d.lastSyncAt, now)}
                    </div>
                  </div>
                  <Button variant="ghost" disabled={busy} onClick={() => void remove(d)} aria-label={`Remove ${d.name}`}>
                    Remove
                  </Button>
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </div>
  )
}

export default HomesSyncSection
