// Settings → Network → Home Sync (homes-sync-v1, 0.46.1). People see the
// sections inside Home as groups; the feature is Home sync.
//
// Rosson (ticket 4f537560): one home server per person, local-first. The
// desktop syncs its Homes with its own daemon; a phone picks its server and
// that server lists the devices syncing from it. Here: this computer's sync
// state, "Change Home sync" / "Sync again", and the devices syncing their Homes
// from this computer, each with Remove. No "personal device" labels.
//
// Card groups: This computer (a status row with a dot and the one action),
// then Devices syncing from this computer (a list; Remove asks first). A
// server without homes-sync-v1 gets one card that says so, with the next step.

import React, { useCallback, useEffect, useState } from 'react'
import { Button, Tag } from '@/components/ui'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useSettingsStore } from '@/stores/settings'
import { onHomesDeviceRemoved, onHomesChanged } from '@/stores/session-events'
import { scopeForHost } from '@/kessel/server-scope'
import {
  parseDevices,
  readControl,
  startSyncing,
  stopSyncing,
  useHomesSyncStatus,
  writeControl,
  type HomesSyncStatus,
  type HomesSyncTransport,
  type KeyValue,
  type SyncedDevice,
} from '@/lib/homes-sync/runner'
import { localKv, localTransport } from '@/lib/homes-sync'
import { parseSyncState } from '@/lib/homes-sync/engine'
import { daemonErrorText } from '@/lib/daemon-error'
import { HOMES_SYNC_STORAGE_KEY } from '@/stores/homes'
import type { SettingEntry } from '../searchManifest'
import {
  SETTING_LIST_ROW_HOVER,
  SETTING_ROW_HINT,
  SETTING_ROW_LABEL,
  SETTING_SERVER_REASON,
  SettingsGroup,
  SettingsLoadState,
} from '../controls/SettingControls'
import { StatusDot, type HealthTone } from './ConnectionsParts'

export const HOMES_SYNC_MANIFEST: SettingEntry[] = [
  {
    id: 'homes-sync.devices',
    section: 'homes-sync',
    label: 'Home Sync',
    description: 'Keep your groups in step between this computer and your phone; see and remove synced devices',
    keywords: ['groups', 'homes', 'home', 'sync', 'devices', 'phone', 'companion', 'my home'],
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

const PAGE = 'w-full max-w-[52rem] space-y-6 pb-6'

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

function changes(n: number): string {
  return n === 1 ? '1 change' : `${n} changes`
}

/** This computer's sync state in words: a dot, a short title and one line. */
export function homesStatusView(
  s: HomesSyncStatus,
  enabled: boolean,
  now: number,
): { tone: HealthTone; title: string; hint: string; reason: string | null } {
  if (!enabled) {
    return {
      tone: 'neutral',
      title: 'Off for this computer',
      hint: 'Its groups stay here and don’t sync with your phone. Turn it on to keep them in step.',
      reason: null,
    }
  }
  switch (s.phase) {
    case 'synced':
      return {
        tone: 'ok',
        title: 'On for this computer',
        hint:
          s.pending > 0
            ? `${changes(s.pending)} waiting to sync; the rest is up to date.`
            : s.lastSyncAt !== null
              ? `Up to date, ${ago(s.lastSyncAt, now)}.`
              : 'Up to date.',
        reason: null,
      }
    case 'syncing':
      return { tone: 'ok', title: 'Syncing now…', hint: 'Sending and fetching changes with this computer’s K2 server.', reason: null }
    case 'offline':
      return {
        tone: 'warn',
        title: 'Paused',
        hint:
          s.pending > 0
            ? `This computer’s K2 server can’t be reached. ${changes(s.pending)} wait here and sync when it’s back.`
            : 'This computer’s K2 server can’t be reached. Groups sync again when it’s back.',
        reason: null,
      }
    case 'removed':
      return {
        tone: 'warn',
        title: 'Taken off Home sync',
        hint: 'This computer was removed from Home sync. Its groups are still here; sync again to rejoin.',
        reason: null,
      }
    case 'unsupported':
      return {
        tone: 'neutral',
        title: 'Not available here',
        hint: 'This computer’s K2 server doesn’t sync groups. Update K2 to turn it on.',
        reason: null,
      }
    case 'error':
      return {
        tone: 'error',
        title: 'Couldn’t sync',
        hint: 'Home sync hit a problem and tries again by itself. Your groups here are unchanged.',
        reason: s.error,
      }
    case 'off':
    default:
      return { tone: 'neutral', title: 'Starting…', hint: 'Home sync starts in a moment.', reason: null }
  }
}

function Header(): React.JSX.Element {
  return (
    <header className="min-w-0">
      <h2 className="type-title text-[var(--color-text-primary)]">Home Sync</h2>
      <p className={`${SETTING_ROW_HINT} mt-1`}>
        Keep your groups the same on this computer and your phone. Each device keeps its own copy, so Home opens even offline,
        and changes made apart are combined when they meet again.
      </p>
    </header>
  )
}

/** What the last action did, in words: done (quiet, with a check) or failed
 *  (what failed, nothing changed, then the server's reason). */
type Outcome = { kind: 'done'; text: string } | { kind: 'failed'; text: string; reason: string } | null

function OutcomeNote({ outcome, testId }: { outcome: Outcome; testId: string }): React.JSX.Element | null {
  if (!outcome) return null
  if (outcome.kind === 'done') {
    return (
      <p role="status" data-testid={testId} className="mt-2 inline-flex items-center gap-1 type-hint text-[var(--color-status-ok-soft)]">
        <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3 flex-shrink-0">
          <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />
        </svg>
        {outcome.text}
      </p>
    )
  }
  const reason = outcome.reason.trim().replace(/[.\s]+$/, '')
  return (
    <div role="alert" data-testid={testId} className="mt-2">
      <p className="type-hint text-[var(--color-status-error-soft)] leading-relaxed max-w-[75ch]">{outcome.text} Nothing changed; try again.</p>
      {reason && <p className={SETTING_SERVER_REASON}>The server said: {reason}.</p>}
    </div>
  )
}

const openUpdates = (): void => {
  useSettingsStore.setState({ pendingUpdateCheck: true })
  useSettingsStore.getState().setSection('general')
}
const openCompanion = (): void => useSettingsStore.getState().openSettings('companion')

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
  const [loadError, setLoadError] = useState<{ plain: string; reason: string | null } | null>(null)
  const [busy, setBusy] = useState(false)
  const [enabled, setEnabled] = useState(() => readControl(kv).enabled)
  const [thisOutcome, setThisOutcome] = useState<Outcome>(null)
  const [listOutcome, setListOutcome] = useState<Outcome>(null)
  const supported = transport.supported()

  const deviceId = parseSyncState(kv?.getItem(HOMES_SYNC_STORAGE_KEY) ?? null)?.deviceId ?? ''

  const refresh = useCallback(async (): Promise<void> => {
    if (!supported) return
    let raw: unknown
    try {
      raw = await transport.listDevices(deviceId)
    } catch (err) {
      setLoadError({ plain: 'This page couldn’t read the list from this computer’s K2 server.', reason: daemonErrorText(err) })
      return
    }
    try {
      setDevices(parseDevices(raw))
      setLoadError(null)
    } catch {
      setLoadError({ plain: 'This computer’s K2 server sent a list K2 couldn’t read. Updating K2 usually fixes this.', reason: null })
    }
  }, [supported, transport, deviceId])

  useEffect(() => {
    void refresh()
    const scope = scopeForHost('local')
    const offs = [onHomesDeviceRemoved(scope, () => void refresh()), onHomesChanged(scope, () => void refresh())]
    return () => offs.forEach((off) => off())
  }, [refresh])

  const retry = (): void => {
    setDevices(null)
    setLoadError(null)
    void refresh()
  }

  const turnOff = async (): Promise<boolean> => {
    setBusy(true)
    try {
      await stopSyncing(kv, transport)
      setEnabled(false)
      await refresh()
      return true
    } catch (err) {
      // stopSyncing turned the control off before the server call failed:
      // put it back, so this computer keeps syncing as the page says.
      writeControl(kv, { enabled: true, epoch: readControl(kv).epoch })
      setEnabled(true)
      setThisOutcome({ kind: 'failed', text: 'Couldn’t take this computer off Home sync.', reason: daemonErrorText(err) })
      return false
    } finally {
      setBusy(false)
    }
  }

  // Rosson 11-11: "Stop syncing" read as a scare word. The button says
  // "Change Home sync"; turning it off is a choice inside, with what it
  // keeps spelled out.
  const change = async (): Promise<void> => {
    setThisOutcome(null)
    const ok = await useConfirmDialogStore.getState().confirm({
      title: 'Change Home sync',
      message:
        'This computer keeps its groups in step with your phone through this computer’s K2 server. You can turn that off for this computer: its groups stay here, nothing is deleted, and you can turn it back on at any time.',
      confirmLabel: 'Turn off for this computer',
    })
    if (!ok) return
    if (await turnOff()) setThisOutcome({ kind: 'done', text: 'Home sync is off for this computer. Its groups stay here.' })
  }

  const start = (): void => {
    startSyncing(kv)
    setEnabled(true)
    setThisOutcome({ kind: 'done', text: 'Home sync is on. This computer joins the list in a moment.' })
    // The runner registers this computer; the list follows.
    setTimeout(() => void refresh(), 1500)
  }

  const remove = async (d: SyncedDevice): Promise<void> => {
    setListOutcome(null)
    const ok = await useConfirmDialogStore.getState().confirm({
      title: d.current ? 'Remove this computer?' : `Remove ${d.name}?`,
      message: d.current
        ? 'This computer stops syncing its groups. Its groups stay as they are here, and you can turn Home sync back on at any time.'
        : `${d.name} stops syncing its groups with this computer. Its groups stay on that device, and it can sync again later from its own settings.`,
      confirmLabel: 'Remove',
      destructive: true,
    })
    if (!ok) return
    if (d.current) {
      if (await turnOff()) setListOutcome({ kind: 'done', text: 'Removed this computer. Its groups stay here.' })
      return
    }
    setBusy(true)
    try {
      await transport.remove(d.deviceId)
      await refresh()
      setListOutcome({ kind: 'done', text: `Removed ${d.name}. Its groups stay on it.` })
    } catch (err) {
      setListOutcome({ kind: 'failed', text: `Couldn’t remove ${d.name}.`, reason: daemonErrorText(err) })
    } finally {
      setBusy(false)
    }
  }

  if (!supported) {
    return (
      <div className={PAGE} data-settings-id="homes-sync.devices" data-testid="homes-sync-section">
        <Header />
        <SettingsLoadState
          kind="empty"
          title="This computer’s K2 server doesn’t sync groups yet"
          action={<Button onClick={openUpdates}>Check for updates</Button>}
          testId="homes-sync-unsupported"
        >
          Update K2 on this computer to 0.46.1 or newer, then come back here to turn on Home sync. Your groups stay on this
          computer until then.
        </SettingsLoadState>
      </div>
    )
  }

  const now = Date.now()
  const view = homesStatusView(status, enabled, now)
  const removed = enabled && status.phase === 'removed'

  const listState = loadError ? (
    <SettingsLoadState
      kind="error"
      title="Couldn’t load the devices"
      detail={loadError.reason}
      action={<Button onClick={retry}>Try again</Button>}
      testId="homes-sync-devices-error"
    >
      {loadError.plain} Your groups are unchanged.
    </SettingsLoadState>
  ) : devices === null ? (
    <SettingsLoadState kind="loading" title="Loading devices…" testId="homes-sync-devices-loading" />
  ) : devices.length === 0 ? (
    <SettingsLoadState
      kind="empty"
      title="No devices sync here yet"
      action={<Button onClick={openCompanion}>Pair a phone</Button>}
      testId="homes-sync-no-devices"
    >
      Pair K2 Companion on your phone and choose this computer as its server; it shows here once it syncs.
    </SettingsLoadState>
  ) : undefined

  return (
    <div className={PAGE} data-settings-id="homes-sync.devices" data-testid="homes-sync-section">
      <Header />

      <section>
        <SettingsGroup title="This computer" card testId="homes-sync-this-computer">
          <div className="px-4 py-3" data-tone={view.tone}>
            <div className="flex items-start justify-between gap-6">
              <div className="min-w-0">
                <div className={`${SETTING_ROW_LABEL} flex items-center gap-2`}>
                  <StatusDot tone={view.tone} />
                  <span data-testid="homes-sync-title">{view.title}</span>
                </div>
                <div className={SETTING_ROW_HINT} data-testid="homes-sync-status">
                  {view.hint}
                </div>
                {view.reason && <p className={SETTING_SERVER_REASON}>The server said: {view.reason.replace(/[.\s]+$/, '')}.</p>}
              </div>
              {enabled && !removed ? (
                <Button variant="quiet" className="flex-shrink-0" disabled={busy} onClick={() => void change()}>
                  Change Home sync
                </Button>
              ) : (
                <Button variant="accent" className="flex-shrink-0" disabled={busy} onClick={start}>
                  {removed ? 'Sync again' : 'Start syncing'}
                </Button>
              )}
            </div>
          </div>
        </SettingsGroup>
        <OutcomeNote outcome={thisOutcome} testId="homes-sync-this-outcome" />
      </section>

      <section>
        <SettingsGroup
          title="Devices syncing from this computer"
          hint="Phones and computers that keep their groups here. Removing one stops its sync; its groups stay on it. Your prompt stash syncs with them too, and its files stay on the device that saved them."
          card
          state={listState}
          testId="homes-sync-devices"
        >
          {(devices ?? []).map((d) => (
            <div key={d.deviceId} className={`px-4 py-3 ${SETTING_LIST_ROW_HOVER}`} data-testid="homes-sync-device">
              <div className="flex items-center justify-between gap-6">
                <div className="min-w-0">
                  <div className={`${SETTING_ROW_LABEL} flex items-center gap-2 min-w-0`}>
                    <span className="truncate">{d.name}</span>
                    {d.current && <Tag tone="neutral">This computer</Tag>}
                  </div>
                  <div className={SETTING_ROW_HINT}>
                    {PLATFORM_LABEL[d.platform] ?? d.platform} · {ago(d.lastSyncAt, now)}
                  </div>
                </div>
                <Button
                  variant="danger-quiet"
                  className="flex-shrink-0"
                  disabled={busy}
                  onClick={() => void remove(d)}
                  aria-label={`Remove ${d.name}`}
                >
                  Remove
                </Button>
              </div>
            </div>
          ))}
        </SettingsGroup>
        <OutcomeNote outcome={listOutcome} testId="homes-sync-list-outcome" />
      </section>
    </div>
  )
}

export default HomesSyncSection
