// Settings → Cores → Chat Hosting (Sew S32, K27).
//
// The owner (or an admin, or anyone the daemon says may manage Sew) runs
// the Sew server on this box with no terminal. The daemon owns every
// decision: the enable / upgrade machines, the health loop, backups, the
// grant hook. This page renders `GET /cli/sew/status` and sends gestures:
//
// - Reads: status (and `sew_changed` → refetch, debounced 250 ms), the
//   preflight while the wizard shows, the backup list and App Spaces when
//   their panels open. Polling only while installing / upgrading (2 s) and
//   while a backup job runs (2 s); view refreshes, cleared on unmount.
// - Who may do what comes from `status.you` (daemon-computed), never from
//   guessing a role. Person-only steps (reveal the recovery key, purge, open
//   Sew as owner) take a vault person confirm first.
// - `supported: false` (a Mac or Windows daemon) → the banner over sample
//   data, every control off, and no call after the one status read.
// - Secrets: the recovery key lives in this component's state only, the
//   handoff code in a local variable until the browser opens. Never a
//   store, a toast, a log line or a URL query.
// - Never `fetchProjects()`.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { openUrl } from '@tauri-apps/plugin-opener'
import { Button, Callout, SquareCheckbox, SquareRadio } from '@/components/ui'
import { primaryScope } from '@/kessel/server-scope'
import { isWebClient } from '@/lib/is-web'
import { onSewChanged } from '@/stores/session-events'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useToastStore } from '@/stores/toast'
import { useWindowModeStore } from '@/stores/window-mode'
import type { SettingEntry } from '../searchManifest'
import {
  SAMPLE_APP_SPACES,
  SAMPLE_BACKUPS,
  SAMPLE_PREFLIGHT,
  SAMPLE_STATUS,
  SEW_CLI,
  SEW_ENABLE_STEPS,
  SEW_FIXABLE_CHECKS,
  downloadNote,
  SEW_MAC_BANNER,
  SEW_PREFLIGHT_LABELS,
  alarmInfo,
  bindSewAppSpace,
  confirmSecretId,
  disableSew,
  enableLabel,
  enableSew,
  fetchSewAppSpaces,
  fetchSewBackups,
  fetchSewJob,
  fetchSewPreflight,
  fetchSewStatus,
  isNotBuilt,
  mintOwnerHandoff,
  ownerHandoffUrl,
  purgeSew,
  revealRecoveryKey,
  rollbackSew,
  runSewBackup,
  setSewAppUrl,
  setSewMirror,
  setSewPublicUrl,
  setSewShip,
  sewErrorInfo,
  sewErrorMessage,
  sewStateChip,
  unbindSewAppSpace,
  uninstallSew,
  upgradeSew,
  youOf,
  type OffboxKind,
  type SewAlarmAction,
  type SewAppSpace,
  type SewBackup,
  type SewPreflightCheck,
  type SewStatus,
} from './sew-api'
import { OnceSecret, SewStatusChip, TypeToConfirm, muted, secondary, typedMatches, usePersonConfirm } from './sew-ui'
import { SewAgentsPanelStub, SewApprovalsPanelStub, SewDeliveriesPanelStub } from './SewStubs'

export const SEW_MANIFEST: SettingEntry[] = [
  { id: 'sew.status', section: 'sew', label: 'Chat / Voice', description: 'The chat and voice server on this box, powered by Sew: status and alarms', keywords: ['sew', 'chat', 'voice', 'slack', 'sidecar', 'core', 'hosting', 'hostchat', 'status'] },
  { id: 'sew.enable', section: 'sew', label: 'Enable Chat / Voice Hosting', description: 'Preflight and install chat and voice hosting (Linux)', keywords: ['sew', 'chat', 'enable', 'install', 'preflight', 'voice'] },
  { id: 'sew.upgrade', section: 'sew', label: 'Upgrade Sew', description: 'Upgrade or roll back the Sew server', keywords: ['sew', 'chat', 'upgrade', 'rollback', 'version'] },
  { id: 'sew.recovery-key', section: 'sew', label: 'Sew recovery key', description: 'Reveal the recovery key once (person confirm)', keywords: ['sew', 'chat', 'recovery', 'key', 'master key'] },
  { id: 'sew.backup', section: 'sew', label: 'Chat and voice backups', description: 'Back up now and see the bundles', keywords: ['sew', 'chat', 'backup', 'restore', 'drill'] },
  { id: 'sew.offbox', section: 'sew', label: 'Off-box copies', description: 'Backup destination and keyring mirror off this machine', keywords: ['sew', 'chat', 'offbox', 'off-box', 'mirror', 'ship', 'backup'] },
  { id: 'sew.public-url', section: 'sew', label: 'Sew public URL', description: "Where Sew's web app lives (setup links)", keywords: ['sew', 'chat', 'public url', 'host', 'setup link'] },
  { id: 'sew.open-as-owner', section: 'sew', label: 'Open Sew as owner', description: "Open Sew's web app as its install owner (person confirm)", keywords: ['sew', 'chat', 'owner', 'handoff', 'open', 'app url'] },
  { id: 'sew.app-spaces', section: 'sew', label: 'App Spaces', description: "Each App's Sew Spaces; members come from App user grants", keywords: ['sew', 'chat', 'space', 'app', 'members', 'hearth'] },
  { id: 'sew.uninstall', section: 'sew', label: 'Disable or uninstall Chat / Voice Hosting', description: 'Disable, uninstall or purge chat and voice hosting', keywords: ['sew', 'chat', 'disable', 'uninstall', 'purge', 'delete'] },
]

type Selection =
  | 'server'
  | 'alarms'
  | 'backups'
  | 'offbox'
  | 'public-url'
  | 'app-spaces'
  | 'agents'
  | 'approvals'
  | 'deliveries'

const POLL_MS = 2000
const EVENT_DEBOUNCE_MS = 250
const RECOVERY_WARNING =
  'This key plus a backup bundle plus the keyring mirror can read every message. Store it off this machine. If this box dies, it is the only way back.'

function isLive(state: string): boolean {
  return state === 'running' || state === 'degraded' || state === 'stopped'
}

function isBusyState(state: string): boolean {
  return state === 'installing' || state === 'upgrading'
}

function mib(n: number | null | undefined): string {
  return typeof n === 'number' ? `${Math.round(n / 2 ** 20)} MiB` : '?'
}

function Row({ label, children }: { label: string; children: React.ReactNode }): React.JSX.Element {
  return (
    <div className="flex items-start gap-3 py-1 text-xs">
      <span className={`w-36 flex-shrink-0 ${muted}`}>{label}</span>
      <span className="flex-1 min-w-0 text-[var(--color-text-primary)] break-words">{children}</span>
    </div>
  )
}

function Title({ children }: { children: React.ReactNode }): React.JSX.Element {
  return <h3 className="text-sm font-medium text-[var(--color-text-primary)]">{children}</h3>
}

interface PanelProps {
  status: SewStatus
  sample: boolean
  canMutate: boolean
  reload: () => Promise<void>
  select: (s: Selection) => void
}

// ── Server panel ──────────────────────────────────────────────────────────

function EnableWizard({ status, sample, canMutate, reload }: PanelProps): React.JSX.Element {
  const [checks, setChecks] = useState<SewPreflightCheck[] | null>(sample ? SAMPLE_PREFLIGHT : null)
  const [preflightError, setPreflightError] = useState<string | null>(null)
  const [identity, setIdentity] = useState<'k2' | 'standalone'>('k2')
  const [voiceOn, setVoiceOn] = useState(false)
  const [voiceUrl, setVoiceUrl] = useState('')
  const [nodeIp, setNodeIp] = useState('')
  const [mediaTarball, setMediaTarball] = useState('')
  const [advanced, setAdvanced] = useState(false)
  const [tarball, setTarball] = useState('')
  const [recoveryFile, setRecoveryFile] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const released = status.released !== false
  // Re-run preflight judges the typed path (not a stale closure).
  const tarballRef = useRef('')
  tarballRef.current = tarball

  const runPreflight = useCallback(async () => {
    if (sample || !released) return
    setPreflightError(null)
    try {
      const r = await fetchSewPreflight(tarballRef.current)
      setChecks(r.checks)
    } catch (e) {
      setPreflightError(sewErrorMessage(e))
    }
  }, [sample, released])

  useEffect(() => {
    void runPreflight()
  }, [runPreflight])

  const failing = (checks ?? []).filter((c) => c.status === 'fail')
  const blocking = failing.filter((c) => !SEW_FIXABLE_CHECKS.includes(c.id))
  // A typed path stays visible (and is sent) even after the preflight it
  // fixed stops failing: Enable must never drop it silently.
  const showSource = advanced || tarball.trim() !== '' || failing.some((c) => c.id === 'source')
  const showRecovery = advanced || recoveryFile.trim() !== '' || failing.some((c) => c.id === 'existing_data')
  const label = enableLabel(status.state) ?? 'Enable Chat / Voice Hosting'
  const disabled = !canMutate || !released || busy || checks === null || blocking.length > 0

  const enable = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      await enableSew({
        identity,
        // Whatever was typed goes, shown or not (enableBody drops blanks).
        tarballPath: tarball,
        recoveryKeyFile: recoveryFile,
        voice: voiceOn ? { publicUrl: voiceUrl, nodeIp, mediaTarballPath: mediaTarball } : undefined,
      })
      await reload()
    } catch (e) {
      const info = sewErrorInfo(e)
      setError(sewErrorMessage(e))
      if (info.code === 'preflight_failed') await runPreflight()
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-3 border border-[var(--color-border)] p-3" data-settings-id="sew.enable" data-testid="sew-enable-wizard">
      {status.state === 'not-installed' ? (
        <p className={`text-[11px] ${secondary}`}>
          Chat / Voice Hosting gives this box team chat and voice calls, powered by Sew, Alakazam&apos;s team chat. It runs beside K2, with its own
          database and encrypted storage.
        </p>
      ) : null}
      {!released ? <Callout tone="warn">{status.notice ?? 'This K2 build pins no published Sew release yet.'}</Callout> : null}
      <div className="space-y-1">
        <div className="flex items-center gap-2">
          <span className="text-xs font-medium text-[var(--color-text-primary)]">Preflight</span>
          <Button disabled={sample || !released} onClick={() => void runPreflight()}>
            Re-run preflight
          </Button>
        </div>
        {preflightError ? <Callout tone="error">{preflightError}</Callout> : null}
        {checks === null && !preflightError ? <p className={`text-[11px] ${muted}`}>Checking…</p> : null}
        <ul className="space-y-0.5" data-testid="sew-preflight">
          {(checks ?? []).map((c) => (
            <li key={c.id} className="text-[11px] flex gap-2" data-testid={`sew-check-${c.id}`} data-status={c.status}>
              <span
                aria-label={c.status}
                className={
                  c.status === 'fail'
                    ? 'text-[var(--color-status-error-soft)]'
                    : c.status === 'warn'
                      ? 'text-[var(--color-status-warn)]'
                      : 'text-[var(--color-status-ok)]'
                }
              >
                {c.status === 'fail' ? '✕' : c.status === 'warn' ? '!' : '✓'}
              </span>
              <span className="text-[var(--color-text-primary)]">{SEW_PREFLIGHT_LABELS[c.id] ?? c.id}</span>
              <span className={muted}>{c.detail}</span>
              {c.hint ? <code className={`font-mono ${secondary}`}>{c.hint}</code> : null}
            </li>
          ))}
        </ul>
      </div>

      <fieldset className="space-y-1" disabled={!canMutate}>
        <legend className="text-xs font-medium text-[var(--color-text-primary)]">Identity</legend>
        <label className={`flex items-center gap-2 text-[11px] ${secondary}`}>
          <SquareRadio name="sew-identity" checked={identity === 'k2'} onChange={() => setIdentity('k2')} aria-label="K2 logins" />
          K2 logins (people sign in with their K2 account)
        </label>
        <label className={`flex items-center gap-2 text-[11px] ${secondary}`}>
          <SquareRadio
            name="sew-identity"
            checked={identity === 'standalone'}
            onChange={() => setIdentity('standalone')}
            aria-label="Sew's own logins"
          />
          Sew&apos;s own logins (setup links)
        </label>
        <p className={`text-[10px] ${muted}`}>Fixed for the life of this install. Changing it later means purge, then enable.</p>
      </fieldset>

      <fieldset className="space-y-1" disabled={!canMutate}>
        <label className={`flex items-center gap-2 text-xs ${secondary}`}>
          <SquareCheckbox checked={voiceOn} onChange={(e) => setVoiceOn(e.target.checked)} aria-label="Also set up voice" />
          Also set up voice
        </label>
        {voiceOn ? (
          <div className="pl-5 space-y-1">
            <input aria-label="Voice URL" placeholder="wss://voice.<box>.k2.dev (default)" value={voiceUrl} onChange={(e) => setVoiceUrl(e.target.value)} className="w-full border border-[var(--color-border)] bg-transparent px-2 py-1 text-xs font-mono" />
            <input aria-label="Node IP" placeholder="node IP (optional)" value={nodeIp} onChange={(e) => setNodeIp(e.target.value)} className="w-full border border-[var(--color-border)] bg-transparent px-2 py-1 text-xs font-mono" />
            <input aria-label="Voice tarball path" placeholder="/path/to/sew-media.tar.gz (only for a box without internet)" value={mediaTarball} onChange={(e) => setMediaTarball(e.target.value)} className="w-full border border-[var(--color-border)] bg-transparent px-2 py-1 text-xs font-mono" />
          </div>
        ) : null}
      </fieldset>

      <div className="space-y-1">
        <button type="button" className={`text-[10px] underline ${muted}`} onClick={() => setAdvanced((v) => !v)}>
          {advanced ? 'Hide advanced' : 'Advanced'}
        </button>
        {showSource ? (
          <input aria-label="Release tarball path" placeholder="/absolute/path/to/sew.tar.gz (only for a box without internet)" value={tarball} disabled={!canMutate} onChange={(e) => setTarball(e.target.value)} className="w-full border border-[var(--color-border)] bg-transparent px-2 py-1 text-xs font-mono" />
        ) : null}
        {showRecovery ? (
          <input aria-label="Recovery key file" placeholder="/absolute/path/to/recovery-key" value={recoveryFile} disabled={!canMutate} onChange={(e) => setRecoveryFile(e.target.value)} className="w-full border border-[var(--color-border)] bg-transparent px-2 py-1 text-xs font-mono" />
        ) : null}
      </div>

      {error ? <Callout tone="error">{error}</Callout> : null}
      <Button variant="accent" disabled={disabled} onClick={() => void enable()}>
        {busy ? 'Starting…' : label}
      </Button>
    </div>
  )
}

function Progress({ status }: { status: SewStatus }): React.JSX.Element | null {
  const p = status.enableProgress
  if (!p || (status.state !== 'installing' && status.state !== 'error')) return null
  const note = downloadNote(p.download)
  return (
    <ol className="space-y-0.5" data-testid="sew-progress">
      {SEW_ENABLE_STEPS.map((s) => {
        const done = Boolean(p.steps?.[s.id])
        const current = p.current === s.id
        const failed = current && status.state === 'error'
        return (
          <li key={s.id} className="text-[11px] flex gap-2" data-testid={`sew-step-${s.id}`} data-step={failed ? 'failed' : done ? 'done' : current ? 'current' : 'todo'}>
            <span className={failed ? 'text-[var(--color-status-error-soft)]' : done ? 'text-[var(--color-status-ok)]' : muted}>
              {failed ? '✕' : done ? '✓' : current ? '…' : '·'}
            </span>
            <span className={current ? 'text-[var(--color-text-primary)]' : secondary}>{s.label}</span>
            {s.id === 'stage' && note ? (
              <span className={muted} data-testid="sew-download">
                {note}
              </span>
            ) : null}
          </li>
        )
      })}
    </ol>
  )
}

function RecoveryKey({ status, sample, reload }: PanelProps): React.JSX.Element | null {
  const you = youOf(status)
  const { confirm, ui } = usePersonConfirm()
  const [key, setKey] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const viewerReadOnly = useWindowModeStore((s) => s.resolved && s.mode === 'viewer')
  if (status.state === 'not-installed') return null
  const can = !sample && !viewerReadOnly && you.canRecoveryKey

  const reveal = async (): Promise<void> => {
    setError(null)
    const ok = await useConfirmDialogStore.getState().confirm({
      title: 'Reveal the recovery key?',
      message: RECOVERY_WARNING,
      confirmLabel: 'Reveal',
    })
    if (!ok) return
    try {
      const token = await confirm('sew-recovery-key', confirmSecretId('sew-recovery-key', status.installId), 'reveal the Sew recovery key')
      if (!token) return
      const r = await revealRecoveryKey(token)
      setKey(r.recoveryKey)
    } catch (e) {
      const info = sewErrorInfo(e)
      setError(info.status === 403 && info.code === 'owner_only' ? 'Only the box owner or an admin can reveal the key, after a confirm.' : sewErrorMessage(e))
    }
  }

  return (
    <div className="space-y-2" data-settings-id="sew.recovery-key">
      <div className="text-xs font-medium text-[var(--color-text-primary)]">Keys</div>
      {status.recoveryKeySaved === false && isLive(status.state) ? (
        <Callout tone="warn">Save your recovery key now. Without it, a dead box means lost messages.</Callout>
      ) : null}
      {key !== null ? (
        <OnceSecret
          heading="Store this key now. K2 will not show it again unless you ask."
          warning={RECOVERY_WARNING}
          secret={key}
          onDone={() => {
            setKey(null)
            void reload()
          }}
        />
      ) : (
        <>
          {ui}
          <Button disabled={!can} onClick={() => void reveal()} title={can ? undefined : 'A person who may see the key does this, with a confirm.'}>
            Reveal recovery key
          </Button>
        </>
      )}
      {error ? <Callout tone="error">{error}</Callout> : null}
    </div>
  )
}

function OpenAsOwner({ status, sample, canMutate, reload }: PanelProps): React.JSX.Element | null {
  const you = youOf(status)
  const { confirm, ui } = usePersonConfirm()
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(status.appUrl ?? '')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const viewerReadOnly = useWindowModeStore((s) => s.resolved && s.mode === 'viewer')
  if (status.state !== 'running' && status.state !== 'degraded') return null

  const appUrl = status.appUrl ?? null
  const web = isWebClient()
  const reason = web
    ? 'Open from the K2 desktop app.'
    : !appUrl
      ? "Set Sew's app URL first."
      : status.owner !== 'set'
        ? 'Sew is still setting you up as owner.'
        : !you.canOpenAsOwner
          ? 'Only the box owner opens Sew as its owner.'
          : null
  const canOpen = !sample && !viewerReadOnly && reason === null

  const saveUrl = async (url: string): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      await setSewAppUrl(url)
      setEditing(false)
      await reload()
    } catch (e) {
      setError(sewErrorMessage(e))
    } finally {
      setBusy(false)
    }
  }

  const open = async (): Promise<void> => {
    if (!appUrl) return
    setError(null)
    let code: string | null = null
    try {
      const token = await confirm('chat-owner-handoff', confirmSecretId('chat-owner-handoff', status.installId), 'open Sew as its owner')
      if (!token) return
      code = (await mintOwnerHandoff(token)).code
    } catch (e) {
      const info = sewErrorInfo(e)
      setError(
        info.code === 'owner_only'
          ? 'Only the box owner can do this, from the K2 desktop app.'
          : info.code === 'owner_not_ready'
            ? 'Sew is still setting you up as owner.'
            : info.status === 429
              ? 'Too many attempts. Wait a minute.'
              : sewErrorMessage(e),
      )
      return
    }
    try {
      await openUrl(ownerHandoffUrl(appUrl, code))
    } catch {
      useToastStore.getState().addToast('Could not open your browser.', 'error')
    } finally {
      code = null
    }
  }

  return (
    <div className="space-y-2" data-settings-id="sew.open-as-owner">
      <div className="text-xs font-medium text-[var(--color-text-primary)]">Open Sew as owner</div>
      <Row label="App URL">
        {editing ? (
          <span className="flex flex-wrap items-center gap-2">
            <input aria-label="Sew app URL" value={draft} onChange={(e) => setDraft(e.target.value)} placeholder="https://hearth.<box>.k2.dev" className="flex-1 min-w-[12rem] border border-[var(--color-border)] bg-transparent px-2 py-1 text-xs font-mono" />
            <Button disabled={busy || !canMutate} onClick={() => void saveUrl(draft)}>
              Save
            </Button>
            <Button variant="ghost" onClick={() => setEditing(false)}>
              Cancel
            </Button>
          </span>
        ) : (
          <span className="flex flex-wrap items-center gap-2">
            <span className="font-mono">{appUrl ?? 'Not set'}</span>
            <Button disabled={!canMutate} onClick={() => { setDraft(appUrl ?? ''); setEditing(true) }}>
              Edit
            </Button>
            {appUrl ? (
              <Button disabled={!canMutate || busy} onClick={() => void saveUrl('')}>
                Clear
              </Button>
            ) : null}
          </span>
        )}
      </Row>
      {ui}
      <Button variant="accent" disabled={!canOpen} onClick={() => void open()} title={reason ?? undefined}>
        Open Sew as owner
      </Button>
      {reason ? <p className={`text-[10px] ${muted}`}>{reason}</p> : null}
      {error ? <Callout tone="error">{error}</Callout> : null}
    </div>
  )
}

function Upgrade({ status, canMutate, reload }: PanelProps): React.JSX.Element | null {
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const result = status.upgradeProgress?.result ?? null
  const showRollback = result === 'done' || Boolean(status.previousVersion)
  if (!status.upgradeAvailable && result !== 'rolled_back' && !showRollback) return null

  const act = async (kind: 'upgrade' | 'rollback'): Promise<void> => {
    const ok = await useConfirmDialogStore.getState().confirm(
      kind === 'upgrade'
        ? {
            title: `Upgrade Sew to ${status.pinnedVersion ?? 'the pinned version'}?`,
            message:
              'Sew backs up first, then swaps the binary and migrates. Sew is briefly unavailable. If anything fails, Sew rolls back on its own.',
            confirmLabel: 'Upgrade',
          }
        : {
            title: 'Roll Sew back?',
            message: 'Sew goes back to the previous version and its data. Anything newer than the pre-upgrade backup is lost.',
            confirmLabel: 'Roll back',
            destructive: true,
          },
    )
    if (!ok) return
    setBusy(true)
    setError(null)
    try {
      if (kind === 'upgrade') await upgradeSew()
      else await rollbackSew()
      await reload()
    } catch (e) {
      setError(sewErrorMessage(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-2" data-settings-id="sew.upgrade">
      {status.upgradeAvailable ? (
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-[11px] text-[var(--color-text-primary)]">
            Sew {status.pinnedVersion} is available (you run {status.onDiskVersion ?? status.installedVersion}).
          </span>
          <Button variant="accent" disabled={!canMutate || busy} onClick={() => void act('upgrade')}>
            Upgrade to {status.pinnedVersion}
          </Button>
        </div>
      ) : null}
      {result === 'rolled_back' ? <Callout tone="warn">The upgrade rolled back: {status.lastError ?? 'see k2 hostchat status'}</Callout> : null}
      {showRollback ? (
        <Button disabled={!canMutate || busy} onClick={() => void act('rollback')}>
          Roll back
        </Button>
      ) : null}
      {error ? <Callout tone="error">{error}</Callout> : null}
    </div>
  )
}

function DangerZone({ status, sample, canMutate, reload }: PanelProps): React.JSX.Element | null {
  const you = youOf(status)
  const { confirm, ui } = usePersonConfirm()
  const [dropGrants, setDropGrants] = useState(false)
  const [purge, setPurge] = useState(false)
  const [keepBackups, setKeepBackups] = useState(true)
  const [typed, setTyped] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  if (status.state === 'not-installed') return null
  const expectHost = status.purgeConfirmHostname ?? ''
  const canPurge = !sample && canMutate && you.canPurge
  const canDisable = ['running', 'degraded', 'stopped', 'error'].includes(status.state)

  const disable = async (): Promise<void> => {
    const ok = await useConfirmDialogStore.getState().confirm({
      title: 'Disable Chat / Voice Hosting?',
      message: "Chat and voice go down for everyone. Agents' k2 chat calls fail. New chat access is refused. Data is kept.",
      confirmLabel: 'Disable',
      destructive: true,
    })
    if (!ok) return
    setBusy(true)
    setError(null)
    try {
      await disableSew()
      await reload()
    } catch (e) {
      setError(sewErrorMessage(e))
    } finally {
      setBusy(false)
    }
  }

  const uninstall = async (): Promise<void> => {
    if (purge) {
      if (!typedMatches(typed, expectHost)) return
      const ok = await useConfirmDialogStore.getState().confirm({
        title: 'Delete all chat and voice data?',
        message:
          'Every message, file and key on this box is deleted. Off-box backups cannot be read without the recovery key. K2 does not delete off-box copies.',
        confirmLabel: 'Delete everything',
        destructive: true,
      })
      if (!ok) return
      setBusy(true)
      setError(null)
      try {
        const token = await confirm('sew-purge', confirmSecretId('sew-purge', status.installId), 'purge all chat and voice data')
        if (!token) return
        const r = await purgeSew({ confirmHostname: typed.trim(), keepBackups, dropGrants, confirmToken: token })
        useToastStore.getState().addToast(r.warning ?? 'Chat and voice data deleted.', 'warning')
        setTyped('')
        setPurge(false)
        await reload()
      } catch (e) {
        setError(sewErrorMessage(e))
      } finally {
        setBusy(false)
      }
      return
    }
    const ok = await useConfirmDialogStore.getState().confirm({
      title: 'Uninstall Chat / Voice Hosting?',
      message: 'Chat / Voice Hosting is removed. Data, the install id and the master key stay. Enable brings it back.',
      confirmLabel: 'Uninstall',
      destructive: true,
    })
    if (!ok) return
    setBusy(true)
    setError(null)
    try {
      await uninstallSew(dropGrants)
      await reload()
    } catch (e) {
      setError(sewErrorMessage(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-2 border border-[color-mix(in_srgb,var(--color-status-error-soft)_30%,transparent)] p-3" data-settings-id="sew.uninstall">
      <div className="text-xs font-medium text-[var(--color-status-error-soft)]">Danger zone</div>
      {canDisable ? (
        <Button disabled={!canMutate || busy} onClick={() => void disable()}>
          Disable Chat / Voice Hosting
        </Button>
      ) : null}
      <label className={`flex items-center gap-2 text-[11px] ${secondary}`}>
        <SquareCheckbox checked={dropGrants} disabled={!canMutate} onChange={(e) => setDropGrants(e.target.checked)} aria-label="Also remove everyone's chat and voice access" />
        Also remove everyone&apos;s chat and voice access
      </label>
      <label className={`flex items-center gap-2 text-[11px] ${secondary}`}>
        <SquareCheckbox checked={purge} disabled={!canPurge} onChange={(e) => setPurge(e.target.checked)} aria-label="Also delete all chat and voice data" />
        Also delete all chat and voice data {you.canPurge ? '' : '(the box owner only)'}
      </label>
      {purge ? (
        <div className="space-y-1 pl-5">
          <TypeToConfirm label={`Type ${expectHost} to confirm`} expect={expectHost} value={typed} onChange={setTyped} disabled={!canPurge} />
          <label className={`flex items-center gap-2 text-[11px] ${secondary}`}>
            <SquareCheckbox checked={keepBackups} onChange={(e) => setKeepBackups(e.target.checked)} aria-label="Keep backup bundles" />
            Keep backup bundles
          </label>
        </div>
      ) : null}
      {ui}
      <Button
        variant="danger"
        disabled={!canMutate || busy || (purge && (!canPurge || !typedMatches(typed, expectHost)))}
        onClick={() => void uninstall()}
      >
        {purge ? 'Uninstall and delete data' : 'Uninstall Chat / Voice Hosting'}
      </Button>
      {error ? <Callout tone="error">{error}</Callout> : null}
    </div>
  )
}

function ServerPanel(props: PanelProps): React.JSX.Element {
  const { status, select } = props
  const wizard = enableLabel(status.state) !== null
  const alarms = status.alarms ?? []
  const db = status.units?.sewDb
  const sew = status.units?.sew
  return (
    <div className="max-w-2xl space-y-4" data-settings-id="sew.status">
      <div className="flex items-center gap-2">
        <Title>Chat / Voice</Title>
        <SewStatusChip state={status.state} />
      </div>
      {status.state !== 'not-installed' ? (
        <div>
          <Row label="Sew version">
            {status.installedVersion ?? '—'}
            {status.pinnedVersion && status.pinnedVersion !== status.installedVersion ? ` (pinned ${status.pinnedVersion})` : ''}
          </Row>
          <Row label="Install id">
            <span className="font-mono">{status.installId ?? '—'}</span>
          </Row>
          <Row label="Logins">{status.identityProvider === 'standalone' ? "Sew's own logins" : 'K2 logins'}</Row>
          <Row label="Owner in Sew">
            {status.owner === 'set' ? 'Set' : status.owner === 'pending' ? 'Setting up…' : status.owner === 'blocked' ? 'Blocked (retry in User Access sync)' : '—'}
          </Row>
          {db || sew ? (
            <Row label="Services">
              sew-db {db?.active ?? '?'} · sew {sew?.active ?? '?'} · {mib(sew?.memoryCurrent)} / {mib(sew?.memoryMax)}
            </Row>
          ) : null}
        </div>
      ) : null}
      {status.consistent === false ? <Callout tone="warn">systemd and K2 disagree: {status.note ?? status.lastError ?? 'run k2 hostchat status'}</Callout> : null}
      {status.lastError && (status.state === 'error' || status.state === 'stopped') ? <Callout tone="error">{status.lastError}</Callout> : null}
      {(status.warnings ?? []).length > 0 ? (
        <ul className="space-y-1" data-testid="sew-warnings">
          {(status.warnings ?? []).map((w) => (
            <li key={w.code} className={`text-[11px] ${secondary}`}>
              {w.message}
              {w.fix ? <code className="ml-1 font-mono">{w.fix}</code> : null}
            </li>
          ))}
        </ul>
      ) : null}
      {alarms.length > 0 ? (
        <div className="space-y-1">
          {alarms.slice(0, 3).map((a) => (
            <p key={a} className={`text-[11px] ${secondary}`}>
              <span className="text-[var(--color-status-warn)]">●</span> {alarmInfo(a).title}
            </p>
          ))}
          <button type="button" className={`text-[10px] underline ${muted}`} onClick={() => select('alarms')}>
            All alarms ({alarms.length})
          </button>
        </div>
      ) : null}
      {wizard ? <EnableWizard {...props} /> : null}
      <Progress status={status} />
      <Upgrade {...props} />
      <RecoveryKey {...props} />
      <OpenAsOwner {...props} />
      <DangerZone {...props} />
    </div>
  )
}

// ── Alarms ────────────────────────────────────────────────────────────────

const ACTION_LABEL: Record<SewAlarmAction, string> = {
  backup: 'Go to Backups',
  offbox: 'Go to Off-box copies',
  'recovery-key': 'Reveal recovery key',
  'public-url': 'Go to Public URL',
}
const ACTION_TARGET: Record<SewAlarmAction, Selection> = {
  backup: 'backups',
  offbox: 'offbox',
  'recovery-key': 'server',
  'public-url': 'public-url',
}

function AlarmsPanel({ status, select }: PanelProps): React.JSX.Element {
  const alarms = status.alarms ?? []
  const offboxOnly = alarms.length > 0 && alarms.every((a) => a === 'keys_mirror_stale' || a === 'k2_backup_offbox_none')
  return (
    <div className="max-w-2xl space-y-3">
      <Title>Alarms</Title>
      {alarms.length === 0 ? <p className={`text-[11px] ${muted}`}>No alarms.</p> : null}
      {offboxOnly ? (
        <p className={`text-[11px] ${secondary}`}>Every K2 box shows Degraded until its chat and voice backups have off-box copies. Set them under Off-box copies.</p>
      ) : null}
      <ul className="space-y-2">
        {alarms.map((a) => {
          const info = alarmInfo(a)
          return (
            <li key={a} className="border border-[var(--color-border)] p-2 space-y-1" data-testid={`sew-alarm-${a}`}>
              <div className="flex items-center gap-2 text-xs">
                <span className={info.severity === 'error' ? 'text-[var(--color-status-error-soft)]' : 'text-[var(--color-status-warn)]'}>●</span>
                <span className="text-[var(--color-text-primary)]">{info.title}</span>
                <code className={`ml-auto text-[10px] font-mono ${muted}`}>{a}</code>
              </div>
              <p className={`text-[11px] ${secondary}`}>{info.fix}</p>
              {info.action ? (
                <Button onClick={() => select(ACTION_TARGET[info.action as SewAlarmAction])}>{ACTION_LABEL[info.action]}</Button>
              ) : null}
            </li>
          )
        })}
      </ul>
    </div>
  )
}

// ── Backups ───────────────────────────────────────────────────────────────

function BackupsPanel({ status, sample, canMutate, reload }: PanelProps): React.JSX.Element {
  const [list, setList] = useState<SewBackup[] | null>(sample ? SAMPLE_BACKUPS : null)
  const [listError, setListError] = useState<string | null>(null)
  const [job, setJob] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [running, setRunning] = useState(false)
  const b = status.backup
  const d = status.drill

  const loadList = useCallback(async () => {
    if (sample) return
    try {
      setList(await fetchSewBackups())
      setListError(null)
    } catch (e) {
      setListError(sewErrorMessage(e))
    }
  }, [sample])

  useEffect(() => {
    void loadList()
  }, [loadList])

  // A job Sew answered without a result yet: poll it (view refresh only).
  useEffect(() => {
    if (!job) return
    let stop = false
    const t = window.setInterval(() => {
      void fetchSewJob(job)
        .then(async (j) => {
          if (stop) return
          if (j.state === 'succeeded' || j.state === 'dead') {
            setJob(null)
            setRunning(false)
            if (j.state === 'dead') setError(j.last_error ?? 'The backup failed.')
            else useToastStore.getState().addToast('Chat and voice backup done.', 'success')
            await Promise.all([reload(), loadList()])
          }
        })
        .catch((e) => {
          if (!stop) setError(sewErrorMessage(e))
        })
    }, POLL_MS)
    return () => {
      stop = true
      window.clearInterval(t)
    }
  }, [job, reload, loadList])

  const backupNow = async (): Promise<void> => {
    setRunning(true)
    setError(null)
    try {
      const r = await runSewBackup()
      if (r.backup) {
        setRunning(false)
        useToastStore.getState().addToast('Chat and voice backup done.', 'success')
        await Promise.all([reload(), loadList()])
      } else {
        setJob(r.jobId)
      }
    } catch (e) {
      setRunning(false)
      setError(sewErrorMessage(e))
    }
  }

  const live = status.state === 'running' || status.state === 'degraded'
  return (
    <div className="max-w-2xl space-y-3" data-settings-id="sew.backup">
      <Title>Backups</Title>
      <div>
        <Row label="Last backup">{b?.lastOkAt ? `${b.lastOkAt}${b.lastId ? ` · ${b.lastId}` : ''}` : <span className="text-[var(--color-status-warn)]">Never</span>}</Row>
        <Row label="Copied off this machine">
          {b?.ship?.kind === 'dir' ? (
            <>
              <span className="font-mono">{b.ship.path}</span>
              {b.ship.lastShippedId ? ` · last ${b.ship.lastShippedId}` : ''}
              {b.ship.lastError ? <span className="text-[var(--color-status-error-soft)]"> · {b.ship.lastError}</span> : null}
            </>
          ) : (
            <span className="text-[var(--color-status-warn)]">Not set</span>
          )}
        </Row>
        <Row label="Last restore drill">
          {d?.lastOkAt ? (d.lastOk === false ? `Failed: ${d.lastError ?? ''}` : `Passed · ${d.lastOkAt}`) : 'Not reported yet'}
        </Row>
      </div>
      <Button variant="accent" disabled={!canMutate || !live || running} onClick={() => void backupNow()}>
        {running ? 'Backing up…' : 'Back up now'}
      </Button>
      {error ? <Callout tone="error">{error}</Callout> : null}
      {listError ? <Callout tone="error">{listError}</Callout> : null}
      <table className="w-full text-[11px]" data-testid="sew-backups">
        <thead>
          <tr className={muted}>
            <th className="text-left font-normal">Id</th>
            <th className="text-left font-normal">Kind</th>
            <th className="text-left font-normal">Time</th>
            <th className="text-right font-normal">Size</th>
          </tr>
        </thead>
        <tbody>
          {(list ?? []).map((x) => (
            <tr key={x.id} className="text-[var(--color-text-primary)]">
              <td className="font-mono">{x.id}</td>
              <td>{x.kind}</td>
              <td>{x.created_at}</td>
              <td className="text-right">{mib(x.bytes)}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <p className={`text-[10px] ${muted}`}>
        To restore one (it replaces ALL chat and voice data): <code className="font-mono">{SEW_CLI} restore &lt;id&gt; --confirm</code>
      </p>
    </div>
  )
}

// ── Off-box copies ────────────────────────────────────────────────────────

function OffboxBlock({
  title,
  noun,
  current,
  disabled,
  onSave,
}: {
  title: string
  noun: string
  current: { kind: string; path: string | null }
  disabled: boolean
  onSave: (kind: OffboxKind, path: string) => Promise<void>
}): React.JSX.Element {
  const [kind, setKind] = useState<OffboxKind>(current.kind === 'dir' ? 'dir' : 'none')
  const [path, setPath] = useState(current.path ?? '')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const pathBad = kind === 'dir' && !path.trim().startsWith('/')
  const save = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      await onSave(kind, path)
    } catch (e) {
      setError(sewErrorMessage(e))
    } finally {
      setBusy(false)
    }
  }
  const slug = noun.replace(/\s+/g, '-')
  return (
    <fieldset className="space-y-1 border border-[var(--color-border)] p-3" disabled={disabled}>
      <legend className="text-xs font-medium text-[var(--color-text-primary)] px-1">{title}</legend>
      <label className={`flex items-center gap-2 text-[11px] ${secondary}`}>
        <SquareRadio name={`${slug}-kind`} checked={kind === 'none'} onChange={() => setKind('none')} aria-label={`${title}: none`} />
        None ({noun} stay on this machine)
      </label>
      <label className={`flex items-center gap-2 text-[11px] ${secondary}`}>
        <SquareRadio name={`${slug}-kind`} checked={kind === 'dir'} onChange={() => setKind('dir')} aria-label={`${title}: a folder on another disk`} />
        A folder on another disk
      </label>
      <label className={`flex items-center gap-2 text-[11px] ${muted}`}>
        <SquareRadio name={`${slug}-kind`} disabled aria-label={`${title}: Alakazam escrow`} />
        Alakazam escrow (coming later)
      </label>
      {kind === 'dir' ? (
        <input aria-label={`${title} path`} value={path} onChange={(e) => setPath(e.target.value)} placeholder="/mnt/offbox/sew" className="w-full border border-[var(--color-border)] bg-transparent px-2 py-1 text-xs font-mono" />
      ) : null}
      {pathBad && path ? <p className="text-[10px] text-[var(--color-status-error-soft)]">Use an absolute path.</p> : null}
      {error ? <Callout tone="error">{error}</Callout> : null}
      <Button variant="accent" disabled={busy || pathBad} onClick={() => void save()}>
        Save
      </Button>
    </fieldset>
  )
}

function OffboxPanel({ status, canMutate, reload }: PanelProps): React.JSX.Element {
  const ship = status.backup?.ship ?? { kind: 'none', path: null }
  const mirror = status.mirror ?? { kind: 'none', path: null }
  return (
    <div className="max-w-2xl space-y-3" data-settings-id="sew.offbox">
      <Title>Off-box copies</Title>
      <p className={`text-[11px] ${secondary}`}>
        Backups hold encrypted data. The keyring mirror holds the keys that open it. Keep them on a different machine from this box, and keep
        the recovery key somewhere else again.
      </p>
      <OffboxBlock
        title="Backup destination"
        noun="backups"
        current={{ kind: ship.kind, path: ship.path ?? null }}
        disabled={!canMutate}
        onSave={async (kind, path) => {
          await setSewShip(kind, path)
          await reload()
        }}
      />
      <OffboxBlock
        title="Keyring mirror"
        noun="keys"
        current={mirror}
        disabled={!canMutate}
        onSave={async (kind, path) => {
          const ok = await useConfirmDialogStore.getState().confirm({
            title: 'Change the keyring mirror?',
            message: 'Sew restarts to apply this. Sessions reconnect on their own.',
            confirmLabel: 'Save and restart',
          })
          if (!ok) return
          await setSewMirror(kind, path)
          await reload()
        }}
      />
    </div>
  )
}

// ── Public URL ────────────────────────────────────────────────────────────

function PublicUrlPanel({ status, canMutate, reload }: PanelProps): React.JSX.Element {
  const [draft, setDraft] = useState(status.publicUrl ?? '')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const warn = (status.warnings ?? []).find((w) => w.code === 'public_url_unset')
  const save = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      await setSewPublicUrl(draft)
      await reload()
    } catch (e) {
      setError(sewErrorMessage(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="max-w-2xl space-y-3" data-settings-id="sew.public-url">
      <Title>Public URL</Title>
      <p className={`text-[11px] ${secondary}`}>
        Where people reach Sew&apos;s web app (an App such as Hearth on this box&apos;s k2.dev name or a bound domain). Sew builds setup
        links from it. Sew&apos;s own API stays on this box only.
      </p>
      <Row label="Now">{status.publicUrl ?? 'Not set'}</Row>
      {warn ? <Callout tone="warn">{warn.message}</Callout> : null}
      <div className="flex flex-wrap items-center gap-2">
        <input aria-label="Sew public URL" value={draft} onChange={(e) => setDraft(e.target.value)} placeholder="https://hearth.<box>.k2.dev" disabled={!canMutate} className="flex-1 min-w-[14rem] border border-[var(--color-border)] bg-transparent px-2 py-1 text-xs font-mono" />
        <Button variant="accent" disabled={!canMutate || busy || !draft.trim().startsWith('https://')} onClick={() => void save()}>
          Save
        </Button>
      </div>
      {error ? <Callout tone="error">{error}</Callout> : null}
    </div>
  )
}

// ── App Spaces (K31) ──────────────────────────────────────────────────────

function AppSpacesPanel({ sample, canMutate }: PanelProps): React.JSX.Element {
  const [spaces, setSpaces] = useState<SewAppSpace[] | null>(sample ? SAMPLE_APP_SPACES : null)
  const [notBuilt, setNotBuilt] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [app, setApp] = useState('')
  const [spaceId, setSpaceId] = useState('')
  const [busy, setBusy] = useState(false)
  // `app/space` whose unbind choice is open (S32 Q3: keep or remove).
  const [unbinding, setUnbinding] = useState<string | null>(null)

  const load = useCallback(async () => {
    if (sample) return
    try {
      setSpaces(await fetchSewAppSpaces())
      setError(null)
    } catch (e) {
      if (isNotBuilt(e)) setNotBuilt(true)
      else setError(sewErrorMessage(e))
    }
  }, [sample])

  useEffect(() => {
    void load()
  }, [load])

  const act = async (fn: () => Promise<unknown>): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      await fn()
      await load()
    } catch (e) {
      setError(sewErrorMessage(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="max-w-2xl space-y-3" data-settings-id="sew.app-spaces">
      <Title>App Spaces</Title>
      <p className={`text-[11px] ${secondary}`}>Members come only from each App&apos;s user grants; Admin Access never adds anyone.</p>
      <p className={`text-[11px] ${muted}`}>
        Unbinding keeps everyone by default. You can instead remove the members K2 added; people added by hand always stay.
      </p>
      {notBuilt ? <p className={`text-[11px] ${muted}`}>This server does not report App Spaces yet.</p> : null}
      {error ? <Callout tone="error">{error}</Callout> : null}
      <ul className="space-y-1" data-testid="sew-app-spaces">
        {(spaces ?? []).map((s) => (
          <li key={`${s.app}/${s.spaceId}`} className="flex items-center gap-2 text-[11px]">
            <span className="font-mono text-[var(--color-text-primary)]">{s.app}</span>
            <span className={`font-mono ${muted}`}>{s.spaceId}</span>
            <span className={secondary}>
              {s.members.added} in · {s.members.pending} pending · {s.members.held} held
            </span>
            {canMutate && unbinding !== `${s.app}/${s.spaceId}` ? (
              <Button className="ml-auto" disabled={busy} onClick={() => setUnbinding(`${s.app}/${s.spaceId}`)}>
                Unbind
              </Button>
            ) : null}
            {canMutate && unbinding === `${s.app}/${s.spaceId}` ? (
              <span className="ml-auto flex flex-wrap items-center gap-2" data-testid="sew-unbind-choice">
                <Button disabled={busy} onClick={() => void act(() => unbindSewAppSpace(s.app, s.spaceId, false)).then(() => setUnbinding(null))}>
                  Unbind, keep members
                </Button>
                <Button disabled={busy} onClick={() => void act(() => unbindSewAppSpace(s.app, s.spaceId, true)).then(() => setUnbinding(null))}>
                  Unbind and remove members K2 added
                </Button>
                <Button disabled={busy} onClick={() => setUnbinding(null)}>
                  Cancel
                </Button>
              </span>
            ) : null}
          </li>
        ))}
      </ul>
      {spaces !== null && spaces.length === 0 ? <p className={`text-[11px] ${muted}`}>No App Spaces bound yet.</p> : null}
      {canMutate && !notBuilt ? (
        <div className="flex flex-wrap items-center gap-2">
          <input aria-label="App id" placeholder="app id (e.g. hearth)" value={app} onChange={(e) => setApp(e.target.value)} className="border border-[var(--color-border)] bg-transparent px-2 py-1 text-xs font-mono" />
          <input aria-label="Space id" placeholder="Sew Space id" value={spaceId} onChange={(e) => setSpaceId(e.target.value)} className="border border-[var(--color-border)] bg-transparent px-2 py-1 text-xs font-mono" />
          <Button disabled={busy || !app.trim() || !spaceId.trim()} onClick={() => void act(() => bindSewAppSpace(app, spaceId))}>
            Bind
          </Button>
        </div>
      ) : null}
    </div>
  )
}

// ── the section root (master-detail) ──────────────────────────────────────

const NAV: { id: Selection; label: string; liveOnly: boolean }[] = [
  { id: 'alarms', label: 'Alarms', liveOnly: true },
  { id: 'backups', label: 'Backups', liveOnly: true },
  { id: 'offbox', label: 'Off-box copies', liveOnly: true },
  { id: 'public-url', label: 'Public URL', liveOnly: true },
  { id: 'app-spaces', label: 'App Spaces', liveOnly: true },
  { id: 'agents', label: 'Agents', liveOnly: false },
  { id: 'approvals', label: 'Approvals', liveOnly: false },
  { id: 'deliveries', label: 'Deliveries', liveOnly: false },
]

export function SewSection(): React.JSX.Element {
  const viewerReadOnly = useWindowModeStore((s) => s.resolved && s.mode === 'viewer')
  const [status, setStatus] = useState<SewStatus | null>(null)
  const [loadError, setLoadError] = useState<{ forbidden: boolean; message: string } | null>(null)
  const [selection, setSelection] = useState<Selection>('server')
  const mounted = useRef(true)

  const load = useCallback(async () => {
    try {
      const s = await fetchSewStatus(true)
      if (!mounted.current) return
      setStatus(s)
      setLoadError(null)
    } catch (e) {
      if (!mounted.current) return
      const info = sewErrorInfo(e)
      setLoadError({ forbidden: info.status === 403, message: sewErrorMessage(e) })
    }
  }, [])

  useEffect(() => {
    mounted.current = true
    void load()
    return () => {
      mounted.current = false
    }
  }, [load])

  const supported = status === null ? null : status.supported
  const busyState = status !== null && status.supported && isBusyState(status.state)

  // Live refresh: `sew_changed` (debounced). Not on a sample page: it stays
  // network-silent after its one read.
  useEffect(() => {
    if (supported !== true) return
    let t: number | null = null
    const off = onSewChanged(primaryScope(), () => {
      if (t !== null) window.clearTimeout(t)
      t = window.setTimeout(() => {
        t = null
        void load()
      }, EVENT_DEBOUNCE_MS)
    })
    return () => {
      if (t !== null) window.clearTimeout(t)
      off()
    }
  }, [supported, load])

  // While the daemon's machine runs: refresh the view every 2 s.
  useEffect(() => {
    if (!busyState) return
    const t = window.setInterval(() => void load(), POLL_MS)
    return () => window.clearInterval(t)
  }, [busyState, load])

  if (loadError && status === null) {
    if (loadError.forbidden) {
      return <p className={`p-6 text-xs ${muted}`} data-testid="sew-forbidden">Only the box owner or an admin can manage Chat / Voice Hosting.</p>
    }
    return (
      <div className="p-6 space-y-2">
        <Callout tone="error">Could not read the status of Chat / Voice Hosting: {loadError.message}</Callout>
        <Button onClick={() => void load()}>Retry</Button>
      </div>
    )
  }
  if (status === null) return <p className={`p-6 text-xs ${muted}`}>Loading…</p>

  const sample = !status.supported
  const view: SewStatus = sample ? SAMPLE_STATUS : status
  const canMutate = !sample && !viewerReadOnly && youOf(status).canManage
  const live = sample || isLive(view.state)
  const props: PanelProps = { status: view, sample, canMutate, reload: load, select: setSelection }
  const chip = sewStateChip(view.state)
  const alarmCount = (view.alarms ?? []).length

  return (
    <div className="flex flex-col h-full min-h-0">
      {sample ? (
        <div data-testid="sew-sample-banner" className="flex-shrink-0 px-4 py-2 text-[10px] font-bold tracking-wide text-center text-[var(--color-status-warn-text)] bg-[color-mix(in_srgb,var(--color-status-warn)_18%,transparent)] border-b border-[color-mix(in_srgb,var(--color-status-warn)_40%,transparent)]">
          {SEW_MAC_BANNER}
        </div>
      ) : null}
      <div className="flex flex-1 min-h-0">
        <div className="w-60 flex-shrink-0 border-r border-[var(--color-border)] flex flex-col min-h-0">
          <button
            type="button"
            onClick={() => setSelection('server')}
            className={`text-left px-3 py-2.5 border-b border-[var(--color-border)] no-drag cursor-pointer ${selection === 'server' ? 'bg-[var(--color-accent)]/10' : 'hover:bg-[var(--color-bg-hover)]'}`}
          >
            <div className="flex items-center gap-2">
              <span className="w-2 h-2 rounded-full flex-shrink-0" style={{ backgroundColor: chip.color }} />
              <span className="text-xs font-medium text-[var(--color-text-primary)]">Chat / Voice Hosting</span>
              <span className="flex-1" />
              <span className={`text-[9px] ${muted}`}>{view.installedVersion ? `Sew v${view.installedVersion}` : ''}</span>
            </div>
            <p className={`text-[10px] mt-0.5 truncate ${muted}`}>
              <span data-testid="sew-card-state">{chip.label}</span>
              {alarmCount > 0 ? ` · ${alarmCount} alarm${alarmCount === 1 ? '' : 's'}` : ''}
            </p>
          </button>
          {NAV.map((n) => {
            const enabled = !n.liveOnly || live
            return (
              <button
                key={n.id}
                type="button"
                disabled={!enabled}
                onClick={() => setSelection(n.id)}
                className={`flex items-center gap-2 text-left px-3 py-2 text-xs no-drag ${
                  !enabled
                    ? `${muted} cursor-not-allowed`
                    : selection === n.id
                      ? 'bg-[var(--color-accent)]/10 text-[var(--color-text-primary)] cursor-pointer'
                      : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] cursor-pointer'
                }`}
              >
                {n.label}
                {n.id === 'alarms' && alarmCount > 0 ? (
                  <span className="text-[9px] font-bold px-1.5 py-0.5 text-[var(--color-status-warn-text)] bg-[color-mix(in_srgb,var(--color-status-warn)_20%,transparent)]">
                    {alarmCount}
                  </span>
                ) : null}
              </button>
            )
          })}
        </div>
        <div className="flex-1 overflow-y-auto p-6 min-h-0">
          {!youOf(status).canManage && !sample ? (
            <p className={`text-[11px] mb-3 ${muted}`}>You can see Chat / Voice Hosting. Only the box owner, an admin or an agent with Manage Chat / Voice Hosting changes it.</p>
          ) : null}
          {selection === 'server' ? <ServerPanel {...props} /> : null}
          {selection === 'alarms' ? <AlarmsPanel {...props} /> : null}
          {selection === 'backups' ? <BackupsPanel {...props} /> : null}
          {selection === 'offbox' ? <OffboxPanel {...props} /> : null}
          {selection === 'public-url' ? <PublicUrlPanel {...props} /> : null}
          {selection === 'app-spaces' ? <AppSpacesPanel {...props} /> : null}
          {selection === 'agents' ? <SewAgentsPanelStub /> : null}
          {selection === 'approvals' ? <SewApprovalsPanelStub /> : null}
          {selection === 'deliveries' ? <SewDeliveriesPanelStub /> : null}
        </div>
      </div>
    </div>
  )
}
