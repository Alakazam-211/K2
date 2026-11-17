// Settings → Cores → Chat / Voice (Sew S32, K27).
//
// The owner (or an admin, or anyone the daemon says may manage Sew) runs
// the Sew server on this box with no terminal. The daemon owns every
// decision: the enable / upgrade machines, the health loop, backups, the
// grant hook. This page renders `GET /cli/sew/status` and sends gestures:
//
// - Reads: status (and `sew_changed` → refetch, debounced 250 ms), the
//   preflight while setup shows, the backup list and App Spaces when their
//   tabs open. Polling only while installing / upgrading (2 s) and while a
//   backup job runs (2 s); view refreshes, cleared on unmount.
// - Who may do what comes from `status.you` (daemon-computed), never from
//   guessing a role. Person-only steps (show the recovery key, purge, open
//   Sew as owner) take a vault person confirm first.
// - `supported: false` (a Mac or Windows daemon) → the shared Linux note
//   over sample data, every control off, and no call after the one status
//   read. A reply without `supported` / `state` is a failed read.
// - Secrets: the recovery key lives in this component's state only, the
//   handoff code in a local variable until the browser opens. Never a
//   store, a toast, a log line or a URL query.
// - Never `fetchProjects()`.
//
// Layout: the Settings reference page (header, one tab strip, card groups
// of rows). Agents, Approvals and Deliveries wait on S30 / S31 and have no
// tab until their routes exist (TODO(S30, S31, K28): `GET
// /cli/sew/admin/agents` + grant/set/revoke, `GET /cli/sew/approvals` +
// approve/deny, `GET /cli/sew/admin/deliveries` + set/reset).

import React, { useCallback, useEffect, useId, useRef, useState } from 'react'
import { openUrl } from '@tauri-apps/plugin-opener'
import { Button, Callout, Checkbox, Input, Radio, Toggle } from '@/components/ui'
import { primaryScope } from '@/kessel/server-scope'
import { isWebClient } from '@/lib/is-web'
import { onSewChanged } from '@/stores/session-events'
import { useWindowModeStore } from '@/stores/window-mode'
import { useSettingsStore } from '@/stores/settings'
import type { SettingEntry } from '../searchManifest'
import {
  SETTING_LIST_ROW_HOVER,
  SETTING_ROW_HINT,
  SETTING_ROW_LABEL,
  SaveNote,
  SettingRow,
  SettingsGroup,
  SettingsLoadState,
  useSaveStatus,
} from '../controls/SettingControls'
import { AccessSheet, DIALOG_FIELD, DIALOG_LABEL, DIALOG_MUTED } from './AccessDialogs'
import { LinuxOnlyNote } from './LinuxOnlyNote'
import {
  SAMPLE_APP_SPACES,
  SAMPLE_BACKUPS,
  SAMPLE_PREFLIGHT,
  SAMPLE_STATUS,
  SEW_ENABLE_STEPS,
  SEW_FIXABLE_CHECKS,
  SEW_NO_PERMISSION,
  SEW_PREFLIGHT_LABELS,
  alarmInfo,
  warningInfo,
  bindSewAppSpace,
  confirmSecretId,
  disableSew,
  downloadNote,
  enableLabel,
  enableSew,
  fetchSewAppSpaces,
  fetchSewBackups,
  fetchSewJob,
  fetchSewPreflight,
  fetchSewStatus,
  isNotBuilt,
  isSewStatus,
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
  sewFailure,
  type SewFailure,
  unbindSewAppSpace,
  uninstallSew,
  upgradeSew,
  youOf,
  type ShipKind,
  type SewAlarmAction,
  type SewAppSpace,
  type SewBackup,
  type SewBoxBackup,
  type SewPreflightCheck,
  type SewStatus,
} from './sew-api'
import { CommandLine, CopyButton, OnceSecret, SewFailed, StatusDot, SewStatusChip, TypeToConfirm, typedMatches, useAsk, usePersonConfirm } from './sew-ui'
import { RecordingPanel } from './SewRecordingPanel'
import { PRODUCT_NAME } from '@/lib/brand'

export const SEW_MANIFEST: SettingEntry[] = [
  { id: 'sew.status', section: 'sew', label: 'Chat / Voice', description: 'The chat and voice server on this box: status and alarms', keywords: ['sew', 'chat', 'voice', 'slack', 'sidecar', 'core', 'hosting', 'hostchat', 'status'] },
  { id: 'sew.enable', section: 'sew', label: 'Set up chat and voice', description: 'Check this server and set up chat and voice (Linux servers)', keywords: ['sew', 'chat', 'enable', 'install', 'preflight', 'voice', 'set up'] },
  { id: 'sew.upgrade', section: 'sew', label: 'Update chat', description: 'Update or roll back the chat server', keywords: ['sew', 'chat', 'upgrade', 'update', 'rollback', 'version'] },
  { id: 'sew.recovery-key', section: 'sew', label: 'Chat recovery key', description: `Show the recovery key ${PRODUCT_NAME} keeps in the vault (after you confirm it’s you)`, keywords: ['sew', 'chat', 'recovery', 'key', 'master key'] },
  { id: 'sew.backup', section: 'sew', label: 'Chat and voice backups', description: 'Back up now and see the saved backups', keywords: ['sew', 'chat', 'backup', 'restore', 'drill'] },
  { id: 'sew.offbox', section: 'sew', label: 'Copies on another machine', description: 'Where copies of chat backups and keys are kept, on another machine', keywords: ['sew', 'chat', 'offbox', 'off-box', 'mirror', 'ship', 'backup'] },
  { id: 'sew.public-url', section: 'sew', label: 'Chat web address', description: 'Where people open chat in a browser (setup links use it)', keywords: ['sew', 'chat', 'public url', 'host', 'setup link', 'web address'] },
  { id: 'sew.open-as-owner', section: 'sew', label: 'Open chat as owner', description: 'Open the chat web app as its owner (after you confirm it’s you)', keywords: ['sew', 'chat', 'owner', 'handoff', 'open', 'app url'] },
  { id: 'sew.app-spaces', section: 'sew', label: 'App Spaces', description: "Each App's chat Spaces; people join through their App access", keywords: ['sew', 'chat', 'space', 'app', 'members', 'hearth'] },
  { id: 'sew.recording', section: 'sew', label: 'Recording in Apps', description: "Who may record in each App's calls, its server's transcript reads, the no-recording lock", keywords: ['sew', 'chat', 'voice', 'recording', 'record', 'transcript', 'consent', 'cui', 'fci'] },
  { id: 'sew.uninstall', section: 'sew', label: 'Turn off or uninstall chat and voice', description: 'Turn off, uninstall or delete chat and voice', keywords: ['sew', 'chat', 'disable', 'uninstall', 'purge', 'delete', 'turn off'] },
]

type Tab = 'overview' | 'backups' | 'app-spaces' | 'recording'
const TABS: Tab[] = ['overview', 'backups', 'app-spaces', 'recording']
const TAB_LABEL: Record<Tab, string> = { overview: 'Overview', backups: 'Backups and keys', 'app-spaces': 'App Spaces', recording: 'Recording' }
const TAB_SETTINGS_ID: Record<Tab, string> = { overview: 'sew.status', backups: 'sew.backup', 'app-spaces': 'sew.app-spaces', recording: 'sew.recording' }

const POLL_MS = 2000
const EVENT_DEBOUNCE_MS = 250
const RECOVERY_WARNING =
  'Anyone with this key, a backup and the key copy can read every message. Keep any copy you make as safe as the vault itself.'

/** A page-level result under the header (an action's outcome). */
type Notice = { tone: 'ok' | 'error'; text: string; detail?: string }

function isLive(state: string): boolean {
  return state === 'running' || state === 'degraded' || state === 'stopped'
}

function isBusyState(state: string): boolean {
  return state === 'installing' || state === 'upgrading'
}

function mb(n: number | null | undefined): string {
  return typeof n === 'number' ? `${Math.round(n / 2 ** 20).toLocaleString()} MB` : 'unknown'
}

/** A daemon time (ISO or unix seconds) as local date and time. */
function when(v: string | number | null | undefined): string {
  if (v === null || v === undefined || v === '') return ''
  const d = typeof v === 'number' ? new Date(v * 1000) : new Date(v)
  return Number.isNaN(d.getTime()) ? String(v) : d.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })
}

function sentence(s: string): string {
  const t = s.trim()
  return t ? t[0].toUpperCase() + t.slice(1) : t
}

const VALUE = 'type-row text-[var(--color-text-primary)]'

/** An App's short name as a person reads it: "jobs" → "Jobs". */
function appName(id: string): string {
  const t = id.trim().replace(/[-_]+/g, ' ')
  return t ? t[0].toUpperCase() + t.slice(1) : id
}

/** "the Jobs App’s Space": never "Jobs’s". */
function appSpace(id: string): string {
  return `the ${appName(id)} App’s Space`
}

/** A daemon hint that is a shell command (shown quietly with Copy), not a sentence. */
function looksLikeCommand(s: string): boolean {
  return /^(k2|sudo|systemctl|apt|cat|ufw)\s/.test(s.trim()) || /\s--[a-z]/.test(s)
}

const STATE_HINT: Record<string, string> = {
  running: 'Chat and calls are working.',
  degraded: 'Working, but something needs attention.',
  stopped: `Chat is down. ${PRODUCT_NAME} keeps trying to bring it back.`,
  error: 'Setup stopped on a problem. Resume it below.',
  disabled: 'Turned off. Messages and files are kept.',
  installing: `${PRODUCT_NAME} is setting it up. This takes a few minutes.`,
  upgrading: 'Chat is updating. It’s unavailable for a moment.',
  removed: 'Uninstalled. Messages, files and keys are kept.',
}

const OWNER_TEXT: Record<string, string> = { set: 'Set up', pending: 'Setting up…', blocked: 'Failed: try again from User Access', none: 'Not yet' }

interface PanelProps {
  status: SewStatus
  sample: boolean
  canMutate: boolean
  reload: () => Promise<void>
  go: (tab: Tab, anchor?: string) => void
  notify: (n: Notice) => void
}

// ── Overview: status, attention, setup, updates, web app, turn off ───────

function StatusGroup({ status }: PanelProps): React.JSX.Element {
  const notInstalled = status.state === 'not-installed'
  const removed = status.state === 'removed'
  const db = status.units?.sewDb
  const sew = status.units?.sew
  // While setup runs, a row waits for its step: the unit report can say
  // "active" before K2 has finished with it.
  const setup = status.state === 'installing' ? (status.enableProgress?.steps ?? {}) : null
  const stepDone = (id: string): boolean => (setup ? Boolean(setup[id]) : true)
  const unitWord = (active: string | undefined, step: string): string => (!stepDone(step) ? 'waiting' : active === 'active' ? 'running' : 'stopped')
  const unitsUp = db?.active === 'active' && sew?.active === 'active'
  // The services line follows the state: a turned-off server never reads as
  // running, whatever the last unit report said.
  const servicesText =
    status.state === 'disabled'
      ? 'Turned off'
      : setup
        ? `Database ${unitWord(db?.active, 'db')}, chat ${unitWord(sew?.active, 'start')}`
        : unitsUp && isLive(status.state)
          ? 'Database and chat running'
          : `Database ${unitWord(db?.active, 'db')}, chat ${unitWord(sew?.active, 'start')}`
  const servicesUp = servicesText === 'Database and chat running'
  const ownerText = !stepDone('link') ? 'Waiting' : (OWNER_TEXT[status.owner ?? 'none'] ?? 'Not yet')
  const versionText = !stepDone('install-bin') ? 'Waiting' : status.installedVersion ? `Version ${status.installedVersion}` : 'Not reported'
  return (
    <div className="space-y-3">
      <SettingsGroup
        title="Status"
        card
        settingId="sew.status"
        testId="sew-status"
        state={
          notInstalled ? (
            <SettingsLoadState kind="empty" title="Chat and voice isn’t set up yet" testId="sew-not-installed">
              This server doesn’t run team chat or voice calls yet. Set it up below; it takes a few minutes.
            </SettingsLoadState>
          ) : removed ? (
            <SettingsLoadState kind="empty" title="Chat and voice is uninstalled" testId="sew-removed">
              Its messages, files and keys are still on this server. Set it up again below to bring them back.
            </SettingsLoadState>
          ) : undefined
        }
      >
        <SettingRow label="Chat and voice" hint={STATE_HINT[status.state]}>
          <SewStatusChip state={status.state} />
        </SettingRow>
        <SettingRow label="Chat" hint={status.upgradeAvailable && status.pinnedVersion ? `Version ${status.pinnedVersion} is available. Update it below.` : undefined}>
          <span className={VALUE} data-testid="sew-version">{versionText}</span>
        </SettingRow>
        <SettingRow label="Sign-in" hint="Chosen at setup. Changing it means deleting chat data and setting it up again.">
          <span className={VALUE}>{status.identityProvider === 'standalone' ? 'Chat’s own accounts' : `${PRODUCT_NAME} accounts`}</span>
        </SettingRow>
        <SettingRow label="You as owner in chat" hint={`${PRODUCT_NAME} makes the server owner chat’s owner too.`}>
          <span className={VALUE} data-testid="sew-owner">{ownerText}</span>
        </SettingRow>
        {db || sew ? (
          <SettingRow label="Services" hint={servicesUp && sew?.memoryMax ? `Chat uses ${mb(sew.memoryCurrent)} of its ${mb(sew.memoryMax)} memory limit.` : undefined}>
            <span className={VALUE} data-testid="sew-services">
              {servicesText}
            </span>
          </SettingRow>
        ) : null}
      </SettingsGroup>
      {status.consistent === false ? (
        <Callout tone="warn" className="type-hint space-y-0.5">
          <p className="font-medium">The server and K2 disagree about whether chat is running.</p>
          <p>K2 checks again on its own. If this stays, restart the server.</p>
          {status.note || status.lastError ? <p className="text-[var(--color-text-secondary)]">The server said: {(status.note ?? status.lastError ?? '').replace(/[.\s]+$/, '')}.</p> : null}
        </Callout>
      ) : null}
      {status.lastError && (status.state === 'error' || status.state === 'stopped') ? (
        <Callout tone="error" className="type-hint space-y-0.5">
          <p className="font-medium">{status.state === 'error' ? 'Setup stopped on a problem.' : 'Chat stopped on a problem.'}</p>
          <p>{status.state === 'error' ? 'Resume setup below; it picks up at the step that stopped.' : `${PRODUCT_NAME} keeps trying to start it again.`}</p>
          <p className="text-[var(--color-text-secondary)]">The server said: {status.lastError.replace(/[.\s]+$/, '')}.</p>
        </Callout>
      ) : null}
    </div>
  )
}

const ACTION: Record<SewAlarmAction, { label: string; tab: Tab; anchor: string }> = {
  backup: { label: 'Go to backups', tab: 'backups', anchor: 'sew.backup' },
  offbox: { label: 'Choose where copies go', tab: 'backups', anchor: 'sew.offbox' },
  'recovery-key': { label: 'Go to the recovery key', tab: 'backups', anchor: 'sew.recovery-key' },
  'public-url': { label: 'Set the web address', tab: 'overview', anchor: 'sew.public-url' },
}

function AttentionGroup({ status, go }: PanelProps): React.JSX.Element | null {
  const alarms = status.alarms ?? []
  const warnings = status.warnings ?? []
  if (alarms.length === 0 && warnings.length === 0) return null
  const offboxOnly = alarms.length > 0 && alarms.every((a) => a === 'keys_mirror_stale' || a === 'k2_backup_offbox_none')
  return (
    <SettingsGroup
      title="Needs attention"
      card
      testId="sew-alarms"
      hint={offboxOnly ? `Every ${PRODUCT_NAME} server shows Degraded until copies of its chat backups and keys are kept on another machine.` : undefined}
    >
      {alarms.map((a) => {
        const info = alarmInfo(a)
        const act = info.action ? ACTION[info.action] : null
        return (
          <SettingRow
            key={a}
            testId={`sew-alarm-${a}`}
            label={
              <span className="flex items-center gap-2">
                <StatusDot color={info.severity === 'error' ? 'var(--color-status-error-soft)' : 'var(--color-status-warn)'} />
                {info.title}
              </span>
            }
            hint={
              <>
                {info.fix}
                {info.command ? <CommandLine command={info.command} ask={info.ask} /> : null}
              </>
            }
          >
            {act ? <Button onClick={() => go(act.tab, act.anchor)}>{act.label}</Button> : null}
          </SettingRow>
        )
      })}
      {warnings.map((w) => {
        const info = warningInfo(w)
        const act = info.action ? ACTION[info.action] : null
        return (
          <SettingRow
            key={w.code}
            testId={`sew-warning-${w.code}`}
            label={
              <span className="flex items-center gap-2">
                <StatusDot color="var(--color-status-warn)" />
                {info.title}
              </span>
            }
            hint={
              <>
                {info.fix}
                {info.command ? <CommandLine command={info.command} ask={info.ask} /> : null}
                {info.detail ? <span className="block mt-1 text-[var(--color-text-muted)]">The server said: {info.detail.replace(/[.\s]+$/, '')}.</span> : null}
              </>
            }
          >
            {act ? <Button onClick={() => go(act.tab, act.anchor)}>{act.label}</Button> : null}
          </SettingRow>
        )
      })}
    </SettingsGroup>
  )
}

const CHECK_DOT: Record<string, string> = { fail: 'var(--color-status-error-soft)', warn: 'var(--color-status-warn)' }
const CHECK_WORD: Record<string, string> = { fail: 'Needs fixing', warn: 'Works, could be better' }

function SetupGroup({ status, sample, canMutate, reload }: PanelProps): React.JSX.Element {
  const [checks, setChecks] = useState<SewPreflightCheck[] | null>(sample ? SAMPLE_PREFLIGHT : null)
  const [preflightError, setPreflightError] = useState<string | null>(null)
  // A reinstall keeps the sign-in the old install had.
  const [identity, setIdentity] = useState<'k2' | 'standalone'>(status.state === 'removed' && status.identityProvider === 'standalone' ? 'standalone' : 'k2')
  const [voiceOn, setVoiceOn] = useState(false)
  const [voiceUrl, setVoiceUrl] = useState('')
  const [nodeIp, setNodeIp] = useState('')
  const [mediaTarball, setMediaTarball] = useState('')
  const [advanced, setAdvanced] = useState(false)
  const [tarball, setTarball] = useState('')
  const [recoveryFile, setRecoveryFile] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<SewFailure | null>(null)
  const released = status.released !== false
  // Check again judges the typed path (not a stale closure).
  const tarballRef = useRef('')
  tarballRef.current = tarball
  const ids = { voice: useId(), ip: useId(), media: useId(), tar: useId(), rec: useId() }

  const [checking, setChecking] = useState(false)
  const runPreflight = useCallback(async () => {
    if (sample || !released) return
    setPreflightError(null)
    setChecking(true)
    try {
      const r = await fetchSewPreflight(tarballRef.current)
      setChecks(r.checks)
    } catch (e) {
      setPreflightError(sewErrorMessage(e))
    } finally {
      setChecking(false)
    }
  }, [sample, released])

  useEffect(() => {
    void runPreflight()
  }, [runPreflight])

  const failing = (checks ?? []).filter((c) => c.status === 'fail')
  const blocking = failing.filter((c) => !SEW_FIXABLE_CHECKS.includes(c.id))
  // A typed path stays visible (and is sent) even after the check it fixed
  // stops failing: setup must never drop it silently.
  const showSource = advanced || tarball.trim() !== '' || failing.some((c) => c.id === 'source')
  const showRecovery = advanced || recoveryFile.trim() !== '' || failing.some((c) => c.id === 'existing_data')
  const label = enableLabel(status.state) ?? 'Set up chat and voice'
  const disabled = !canMutate || !released || busy || checks === null || blocking.length > 0
  const fresh = status.state === 'not-installed' || status.state === 'removed'
  const reinstall = status.state === 'removed'

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
      setError(sewFailure(e))
      if (info.code === 'preflight_failed') await runPreflight()
    } finally {
      setBusy(false)
    }
  }

  const field = (id: string, lbl: string, value: string, set: (v: string) => void, placeholder: string): React.JSX.Element => (
    <div>
      <label htmlFor={id} className={DIALOG_LABEL}>
        {lbl}
      </label>
      <Input id={id} aria-label={lbl} value={value} disabled={!canMutate} placeholder={placeholder} onChange={(e) => set(e.target.value)} className={DIALOG_FIELD} />
    </div>
  )

  return (
    <div className="space-y-5" data-settings-id="sew.enable" data-testid="sew-enable-wizard">
      {!released ? (
        <Callout tone="warn" className="type-hint">
          {status.notice ?? `This version of ${PRODUCT_NAME} doesn’t name a chat release yet, so chat can’t be set up. Update ${PRODUCT_NAME}, then try again.`}
        </Callout>
      ) : null}
      <SettingsGroup
        title="Server checks"
        hint={`${PRODUCT_NAME} checks this server before setup. Anything marked Needs fixing has to be fixed first.`}
        card
        action={
          <Button disabled={sample || !released || checking} onClick={() => void runPreflight()}>
            {checking ? 'Checking…' : 'Check again'}
          </Button>
        }
        state={
          preflightError ? (
            <SettingsLoadState kind="error" title="Couldn’t check this server" detail={preflightError} action={<Button onClick={() => void runPreflight()}>Try again</Button>}>
              Setup stays off until the checks run. Try again in a moment.
            </SettingsLoadState>
          ) : checks === null ? (
            <SettingsLoadState kind="loading" title="Checking this server…" />
          ) : checks.length === 0 ? (
            <SettingsLoadState kind="empty" title="No checks reported">
              The server didn’t list any checks. Setup can go ahead.
            </SettingsLoadState>
          ) : undefined
        }
      >
        <div data-testid="sew-preflight" className="divide-y divide-[var(--color-border)]">
          {(checks ?? []).map((c) => (
            <div key={c.id} data-testid={`sew-check-${c.id}`} data-status={c.status}>
              <SettingRow
                label={
                  <span className="flex items-center gap-2">
                    <StatusDot color={CHECK_DOT[c.status] ?? 'var(--color-status-ok)'} />
                    {SEW_PREFLIGHT_LABELS[c.id] ?? sentence(c.id.replace(/_/g, ' '))}
                  </span>
                }
                hint={
                  c.detail || c.hint ? (
                    <>
                      {c.detail ? sentence(c.detail) : null}
                      {c.hint ? looksLikeCommand(c.hint) ? <CommandLine command={c.hint} /> : <span className="block mt-1">{sentence(c.hint)}</span> : null}
                    </>
                  ) : undefined
                }
              >
                <span className="type-hint text-[var(--color-text-secondary)]">{CHECK_WORD[c.status] ?? 'Ready'}</span>
              </SettingRow>
            </div>
          ))}
        </div>
      </SettingsGroup>

      <SettingsGroup
        title={fresh ? 'Set up chat and voice' : 'Turn chat and voice back on'}
        card
        hint={
          reinstall
            ? 'Sets chat up again on this server. Its messages, files and keys come back, with the sign-in it had.'
            : fresh
              ? `Team chat and voice calls for this server. They run beside ${PRODUCT_NAME}, with their own database and encrypted storage.`
              : 'Messages, files and keys were kept. Turning it back on brings them back.'
        }
      >
        {fresh && !reinstall ? (
          <SettingRow label="Sign-in" hint="How people sign in to chat. You can’t change it later without deleting chat data." stack>
            <fieldset className="space-y-2" disabled={!canMutate}>
              <legend className="sr-only">Sign-in</legend>
              <label className="flex items-start gap-2 cursor-pointer">
                <Radio name="sew-identity" className="mt-0.5" checked={identity === 'k2'} onChange={() => setIdentity('k2')} aria-label={`${PRODUCT_NAME} accounts`} />
                <span>
                  <span className={SETTING_ROW_LABEL}>K2 accounts</span>
                  <span className={`block ${SETTING_ROW_HINT}`}>People sign in with the account they use for K2.</span>
                </span>
              </label>
              <label className="flex items-start gap-2 cursor-pointer">
                <Radio name="sew-identity" className="mt-0.5" checked={identity === 'standalone'} onChange={() => setIdentity('standalone')} aria-label="Chat’s own accounts" />
                <span>
                  <span className={SETTING_ROW_LABEL}>Chat’s own accounts</span>
                  <span className={`block ${SETTING_ROW_HINT}`}>People get a setup link from chat and make an account there.</span>
                </span>
              </label>
            </fieldset>
          </SettingRow>
        ) : null}
        <SettingRow label="Voice calls" hint="Also set up calls. Leave off for chat only; you can add voice later." below={
          voiceOn ? (
            <div className="space-y-3 max-w-md">
              {field(ids.voice, 'Voice address', voiceUrl, setVoiceUrl, 'wss://voice.example.k2.dev (leave empty for the default)')}
              {field(ids.ip, 'Server IP address (optional)', nodeIp, setNodeIp, '203.0.113.10')}
              {field(ids.media, 'Voice download file (only without internet)', mediaTarball, setMediaTarball, '/path/to/sew-media.tar.gz')}
            </div>
          ) : undefined
        }>
          <Toggle checked={voiceOn} disabled={!canMutate} onChange={setVoiceOn} aria-label="Also set up voice calls" />
        </SettingRow>
        <SettingRow
          label="Install from files"
          hint="For a server without internet, or to bring back chat data from another server."
          below={
            showSource || showRecovery ? (
              <div className="space-y-3 max-w-md">
                {showSource ? field(ids.tar, 'Chat release file', tarball, setTarball, '/path/to/sew.tar.gz') : null}
                {showRecovery ? field(ids.rec, 'Recovery key file', recoveryFile, setRecoveryFile, '/path/to/recovery-key') : null}
              </div>
            ) : undefined
          }
        >
          <Button variant="quiet" aria-expanded={advanced} onClick={() => setAdvanced((v) => !v)}>
            {advanced ? 'Hide' : 'Show'}
          </Button>
        </SettingRow>
        <div className="px-4 py-3 space-y-3">
          {error ? <SewFailed what="Couldn’t start setup." failure={error} next="Nothing was changed. Fix what the server says, then try again." /> : null}
          <div className="flex items-center justify-between gap-4">
            <p className={SETTING_ROW_HINT}>{blocking.length > 0 ? 'Fix the checks marked Needs fixing first.' : `${PRODUCT_NAME} shows each step as it goes.`}</p>
            <Button variant="accent" disabled={disabled} onClick={() => void enable()}>
              {busy ? 'Starting…' : label}
            </Button>
          </div>
        </div>
      </SettingsGroup>
    </div>
  )
}

function Progress({ status }: { status: SewStatus }): React.JSX.Element | null {
  const p = status.enableProgress
  if (!p || (status.state !== 'installing' && status.state !== 'error')) return null
  const note = downloadNote(p.download)
  return (
    <SettingsGroup title={status.state === 'error' ? 'Setup steps' : 'Setting up'} hint={status.state === 'error' ? 'Resume picks up at the step that stopped.' : 'This page updates as each step finishes.'} card>
      <ol data-testid="sew-progress" className="divide-y divide-[var(--color-border)]">
        {SEW_ENABLE_STEPS.map((s) => {
          const done = Boolean(p.steps?.[s.id])
          const current = p.current === s.id
          const failed = current && status.state === 'error'
          const color = failed ? 'var(--color-status-error-soft)' : done ? 'var(--color-status-ok)' : current ? 'var(--color-status-working)' : 'var(--color-text-muted)'
          return (
            <li key={s.id} className="flex items-center justify-between gap-6 px-4 py-2" data-testid={`sew-step-${s.id}`} data-step={failed ? 'failed' : done ? 'done' : current ? 'current' : 'todo'}>
              <span className="flex items-center gap-2 min-w-0">
                <StatusDot color={color} />
                <span className={current || done ? SETTING_ROW_LABEL : 'type-row text-[var(--color-text-secondary)]'}>{s.label}</span>
                {s.id === 'stage' && note ? (
                  <span className="type-hint text-[var(--color-text-muted)]" data-testid="sew-download">
                    {note}
                  </span>
                ) : null}
              </span>
              <span className="type-hint text-[var(--color-text-secondary)] flex-shrink-0">{failed ? 'Stopped' : done ? 'Done' : current ? 'Working…' : 'Waiting'}</span>
            </li>
          )
        })}
      </ol>
    </SettingsGroup>
  )
}

function UpdatesGroup({ status, canMutate, reload, notify }: PanelProps): React.JSX.Element | null {
  const { ask, sheet } = useAsk()
  const [error, setError] = useState<{ what: string; failure: SewFailure } | null>(null)
  const [busy, setBusy] = useState<'upgrade' | 'rollback' | null>(null)
  const result = status.upgradeProgress?.result ?? null
  const showRollback = result === 'done' || Boolean(status.previousVersion)
  if (!status.upgradeAvailable && result !== 'rolled_back' && !showRollback) return null

  const act = async (kind: 'upgrade' | 'rollback'): Promise<void> => {
    const ok = await ask(
      kind === 'upgrade'
        ? {
            title: `Update chat to ${status.pinnedVersion ?? 'the newest version'}?`,
            message: 'Chat backs up first, then updates. It’s unavailable for a moment. If anything fails, it goes back to this version on its own.',
            confirmLabel: 'Update',
          }
        : {
            title: 'Roll chat back?',
            message: 'Chat goes back to the version before the last update, with its data from the backup taken before that update. Messages since then are lost.',
            confirmLabel: 'Roll back',
            destructive: true,
          },
    )
    if (!ok) return
    setBusy(kind)
    setError(null)
    try {
      if (kind === 'upgrade') await upgradeSew()
      else await rollbackSew()
      notify({ tone: 'ok', text: kind === 'upgrade' ? 'Chat is updating. This page shows when it’s done.' : 'Chat is rolling back. This page shows when it’s done.' })
      await reload()
    } catch (e) {
      setError({ what: kind === 'upgrade' ? 'Couldn’t start the update.' : 'Couldn’t roll back.', failure: sewFailure(e) })
    } finally {
      setBusy(null)
    }
  }

  return (
    <div className="space-y-3" data-settings-id="sew.upgrade">
      <SettingsGroup title="Updates" card>
        {status.upgradeAvailable ? (
          <SettingRow label={`Version ${status.pinnedVersion} is available`} hint={`You run ${status.onDiskVersion ?? status.installedVersion ?? 'an older version'}. Chat backs up before it updates.`}>
            <Button variant="accent" disabled={!canMutate || busy !== null} onClick={() => void act('upgrade')}>
              {busy === 'upgrade' ? 'Updating…' : `Update to ${status.pinnedVersion}`}
            </Button>
          </SettingRow>
        ) : null}
        {showRollback ? (
          <SettingRow label="Go back to the previous version" hint="Restores the backup taken before the last update. Messages since then are lost.">
            <Button variant="danger-quiet" disabled={!canMutate || busy !== null} onClick={() => void act('rollback')}>
              {busy === 'rollback' ? 'Rolling back…' : 'Roll back…'}
            </Button>
          </SettingRow>
        ) : null}
      </SettingsGroup>
      {result === 'rolled_back' ? (
        <Callout tone="warn" className="type-hint space-y-0.5">
          <p className="font-medium">The last update didn’t finish, so chat went back to the version before.</p>
          <p>Chat works as before. Try the update again later.</p>
          {status.lastError ? <p className="text-[var(--color-text-secondary)]">The server said: {status.lastError.replace(/[.\s]+$/, '')}.</p> : null}
        </Callout>
      ) : null}
      {error ? <SewFailed what={error.what} failure={error.failure} next="Nothing changed. Try again in a moment." /> : null}
      {sheet}
    </div>
  )
}

/** A URL row: the value and Edit; editing opens a field under the row. */
function UrlRow({
  label,
  hint,
  value,
  ariaLabel,
  placeholder,
  canMutate,
  requireHttps,
  allowClear,
  settingId,
  onSave,
}: {
  label: string
  hint: string
  value: string | null
  ariaLabel: string
  placeholder: string
  canMutate: boolean
  requireHttps: boolean
  allowClear: boolean
  settingId?: string
  onSave: (url: string) => Promise<void>
}): React.JSX.Element {
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(value ?? '')
  const [busy, setBusy] = useState<'save' | 'clear' | null>(null)
  const [error, setError] = useState<SewFailure | null>(null)
  const save = useSaveStatus()
  const id = useId()
  const bad = requireHttps && draft.trim() !== '' && !draft.trim().startsWith('https://')

  const run = async (url: string): Promise<void> => {
    setBusy(url ? 'save' : 'clear')
    setError(null)
    try {
      await onSave(url)
      setEditing(false)
      save.saved()
    } catch (e) {
      setError(sewFailure(e))
    } finally {
      setBusy(null)
    }
  }

  return (
    <SettingRow
      label={label}
      hint={
        <>
          {hint}
          {!editing ? <span className="block mt-1 text-[var(--color-text-primary)] break-all">{value ?? 'Not set'}</span> : null}
        </>
      }
      settingId={settingId}
      below={
        editing || error ? (
          <div className="space-y-2">
            {editing ? (
              <form
                className="flex flex-wrap items-center gap-2"
                onSubmit={(e) => {
                  e.preventDefault()
                  if (!bad && draft.trim()) void run(draft)
                }}
              >
                <label htmlFor={id} className="sr-only">
                  {ariaLabel}
                </label>
                <Input id={id} aria-label={ariaLabel} autoFocus value={draft} placeholder={placeholder} onChange={(e) => setDraft(e.target.value)} className="flex-1 min-w-[14rem]" />
                <Button type="submit" variant="accent" disabled={busy !== null || bad || !draft.trim()}>
                  {busy === 'save' ? 'Saving…' : 'Save'}
                </Button>
                <Button variant="ghost" disabled={busy !== null} onClick={() => { setEditing(false); setError(null) }}>
                  Cancel
                </Button>
              </form>
            ) : null}
            {bad ? <p className="type-hint text-[var(--color-status-error-soft)]">Use an address that starts with https://.</p> : null}
            {error ? <SewFailed what={busy === null && !editing ? `Couldn’t clear the ${label.toLowerCase()}.` : `Couldn’t save the ${label.toLowerCase()}.`} failure={error} next="The old address still applies. Check it, then try again." /> : null}
          </div>
        ) : undefined
      }
    >
      <SaveNote state={save.state} />
      {!editing ? (
        <>
          {allowClear && value ? (
            <Button variant="quiet" disabled={!canMutate || busy !== null} onClick={() => void run('')}>
              {busy === 'clear' ? 'Clearing…' : 'Clear'}
            </Button>
          ) : null}
          <Button disabled={!canMutate} onClick={() => { setDraft(value ?? ''); setEditing(true) }}>
            {value ? 'Edit' : 'Set'}
          </Button>
        </>
      ) : null}
    </SettingRow>
  )
}

function WebAppGroup({ status, sample, canMutate, reload }: PanelProps): React.JSX.Element | null {
  const you = youOf(status)
  const { confirm, ui } = usePersonConfirm()
  const [error, setError] = useState<SewFailure | null>(null)
  const [opening, setOpening] = useState(false)
  const viewerReadOnly = useWindowModeStore((s) => s.resolved && s.mode === 'viewer')
  if (status.state !== 'running' && status.state !== 'degraded') return null

  const appUrl = status.appUrl ?? null
  const web = isWebClient()
  const reason = web
    ? `Open it from the ${PRODUCT_NAME} desktop app.`
    : !appUrl
      ? 'Set the owner sign-in address first.'
      : status.owner !== 'set'
        ? 'Chat is still setting you up as owner.'
        : !you.canOpenAsOwner
          ? 'Only the server owner opens chat as its owner.'
          : null
  const canOpen = !sample && !viewerReadOnly && reason === null

  const open = async (): Promise<void> => {
    if (!appUrl) return
    setError(null)
    setOpening(true)
    try {
      await openAsOwner(appUrl)
    } finally {
      setOpening(false)
    }
  }

  const openAsOwner = async (appUrl: string): Promise<void> => {
    let code: string | null = null
    try {
      const token = await confirm('chat-owner-handoff', confirmSecretId('chat-owner-handoff', status.installId), 'open chat as its owner')
      if (!token) return
      code = (await mintOwnerHandoff(token)).code
    } catch (e) {
      setError(sewFailure(e))
      return
    }
    try {
      await openUrl(ownerHandoffUrl(appUrl, code))
    } catch {
      setError({ line: 'Your browser didn’t open.', next: 'Open chat in your browser yourself and sign in there.', detail: '', code: null })
    } finally {
      code = null
    }
  }

  return (
    <div className="space-y-3">
      <SettingsGroup title="Web app" card hint="Where people open chat in a browser.">
        <UrlRow
          label="Web address"
          hint="Where people reach chat. Setup links use it."
          value={status.publicUrl ?? null}
          ariaLabel="Chat web address"
          placeholder="https://chat.example.k2.dev"
          canMutate={canMutate}
          requireHttps
          allowClear={false}
          settingId="sew.public-url"
          onSave={async (url) => {
            await setSewPublicUrl(url)
            await reload()
          }}
        />
        <UrlRow
          label="Owner sign-in address"
          hint="Open as owner signs you in here."
          value={appUrl}
          ariaLabel="Owner sign-in address"
          placeholder="https://chat.example.k2.dev"
          canMutate={canMutate}
          requireHttps={false}
          allowClear
          onSave={async (url) => {
            await setSewAppUrl(url)
            await reload()
          }}
        />
        <SettingRow label="Open chat as owner" hint={reason ?? 'Opens chat in your browser, signed in as its owner, after you confirm it’s you.'} settingId="sew.open-as-owner">
          <Button disabled={!canOpen || opening} onClick={() => void open()}>
            {opening ? 'Opening…' : 'Open as owner'}
          </Button>
        </SettingRow>
      </SettingsGroup>
      {error ? <SewFailed what="Couldn’t open chat as owner." failure={error} next="Try again. If it keeps failing, sign in to chat in your browser." /> : null}
      {ui}
    </div>
  )
}

function UninstallSheet({
  status,
  canMutate,
  purgeOnly = false,
  onClose,
  onDone,
}: {
  status: SewStatus
  canMutate: boolean
  /** Already uninstalled: this only deletes the data left behind. */
  purgeOnly?: boolean
  onClose: () => void
  onDone: (n: Notice) => void
}): React.JSX.Element {
  const you = youOf(status)
  const { confirm, ui } = usePersonConfirm()
  const [dropGrants, setDropGrants] = useState(false)
  const [purge, setPurge] = useState(purgeOnly)
  const [keepBackups, setKeepBackups] = useState(true)
  const [typed, setTyped] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<SewFailure | null>(null)
  const expectHost = status.purgeConfirmHostname ?? ''
  const canPurge = canMutate && you.canPurge
  const ready = canMutate && !busy && (!purge || (canPurge && typedMatches(typed, expectHost)))

  const run = async (): Promise<void> => {
    if (!ready) return
    setBusy(true)
    setError(null)
    try {
      if (purge) {
        const token = await confirm('sew-purge', confirmSecretId('sew-purge', status.installId), 'purge all chat and voice data')
        if (!token) return
        const r = await purgeSew({ confirmHostname: typed.trim(), keepBackups, dropGrants, confirmToken: token })
        onDone({ tone: 'ok', text: purgeOnly ? 'The old chat data is deleted from this server.' : 'Chat and voice are uninstalled, and their messages, files and keys are deleted.', detail: r.warning })
      } else {
        await uninstallSew(dropGrants)
        onDone({ tone: 'ok', text: 'Chat and voice are uninstalled. Messages, files and keys are kept; set it up again to bring them back.' })
      }
    } catch (e) {
      setError(sewFailure(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <>
      <AccessSheet
        title={purgeOnly ? 'Delete the old chat data?' : 'Uninstall chat and voice?'}
        intro={purgeOnly ? 'Every message, file and key the old install left on this server is deleted. This can’t be undone.' : 'Chat and calls stop for everyone, and the chat server is removed from this server.'}
        onClose={onClose}
        busy={busy}
        testId="sew-uninstall-dialog"
        footer={
          <>
            <Button variant="ghost" size="md" onClick={onClose} disabled={busy} autoFocus>
              Cancel
            </Button>
            <Button variant="danger-muted" size="md" disabled={!ready} onClick={() => void run()}>
              {busy ? (purgeOnly ? 'Deleting…' : 'Uninstalling…') : purgeOnly ? 'Delete data' : purge ? 'Uninstall and delete data' : 'Uninstall'}
            </Button>
          </>
        }
      >
        {purgeOnly ? null : (
          <>
        <p className={DIALOG_MUTED}>Messages, files and keys stay on this server unless you delete them here. Setting chat up again brings them back.</p>
        <label className="flex items-start gap-2 cursor-pointer">
          <Checkbox className="mt-0.5" checked={dropGrants} disabled={!canMutate || busy} onChange={(e) => setDropGrants(e.target.checked)} aria-label="Also remove everyone's chat and voice access" />
          <span>
            <span className={SETTING_ROW_LABEL}>Also remove everyone’s chat and voice access</span>
            <span className={`block ${DIALOG_MUTED}`}>People and agents lose their chat access in User Access too.</span>
          </span>
        </label>
        <label className={`flex items-start gap-2 ${canPurge ? 'cursor-pointer' : ''}`}>
          <Checkbox className="mt-0.5" checked={purge} disabled={!canPurge || busy} onChange={(e) => setPurge(e.target.checked)} aria-label="Also delete all chat and voice data" />
          <span>
            <span className={SETTING_ROW_LABEL}>Also delete all chat and voice data</span>
            <span className={`block ${DIALOG_MUTED}`}>{you.canPurge ? 'Every message, file and key on this server. This can’t be undone.' : 'Only the server owner can delete chat data.'}</span>
          </span>
        </label>
          </>
        )}
        {purge ? (
          <div className={purgeOnly ? 'space-y-3' : 'space-y-3 pl-6'}>
            <Callout tone="warn" className="type-hint leading-relaxed">
              This also deletes the recovery key in the K2 vault. Backups kept on another machine can’t be read without it, so show and copy it
              first (Backups and keys) if you want to keep them readable. K2 doesn’t delete copies kept elsewhere.
            </Callout>
            <TypeToConfirm label={`Type ${expectHost} to confirm`} expect={expectHost} value={typed} onChange={setTyped} disabled={!canPurge || busy} />
            <label className="flex items-start gap-2 cursor-pointer">
              <Checkbox className="mt-0.5" checked={keepBackups} disabled={busy} onChange={(e) => setKeepBackups(e.target.checked)} aria-label="Keep backup bundles" />
              <span>
                <span className={SETTING_ROW_LABEL}>Keep the backups on this server</span>
                <span className={`block ${DIALOG_MUTED}`}>They can’t be read without the recovery key.</span>
              </span>
            </label>
          </div>
        ) : null}
        {error ? <SewFailed what={purgeOnly ? 'Couldn’t delete the old chat data.' : 'Couldn’t uninstall chat and voice.'} failure={error} next="Nothing was removed. Try again in a moment." /> : null}
      </AccessSheet>
      {ui}
    </>
  )
}

function TurnOffGroup({ status, canMutate, reload, notify }: PanelProps): React.JSX.Element | null {
  const { ask, sheet } = useAsk()
  const [uninstalling, setUninstalling] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<SewFailure | null>(null)
  if (status.state === 'not-installed') return null
  const canDisable = ['running', 'degraded', 'stopped', 'error'].includes(status.state)
  const removed = status.state === 'removed'

  const disable = async (): Promise<void> => {
    const ok = await ask({
      title: 'Turn off chat and voice?',
      message: 'Chat and calls stop for everyone, and agents can’t post. Messages, files and keys are kept, and Turn back on brings everything back.',
      confirmLabel: 'Turn off',
      destructive: true,
    })
    if (!ok) return
    setBusy(true)
    setError(null)
    try {
      await disableSew()
      notify({ tone: 'ok', text: 'Chat and voice are off. Messages, files and keys are kept.' })
      await reload()
    } catch (e) {
      setError(sewFailure(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-3" data-settings-id="sew.uninstall">
      <SettingsGroup title={removed ? 'Old chat data' : 'Turn off or uninstall'} card testId="sew-danger">
        {canDisable ? (
          <SettingRow label="Turn off chat and voice" hint="Stops chat and calls for everyone until you turn them back on. Nothing is deleted.">
            <Button variant="danger-quiet" disabled={!canMutate || busy} onClick={() => void disable()}>
              {busy ? 'Turning off…' : 'Turn off…'}
            </Button>
          </SettingRow>
        ) : null}
        {removed ? (
          <SettingRow label="Delete the old chat data" hint={youOf(status).canPurge ? 'Messages, files and keys from before are still on this server. Deleting them can’t be undone.' : 'Only the server owner can delete chat data.'}>
            <Button variant="danger-quiet" disabled={!canMutate || !youOf(status).canPurge || uninstalling} onClick={() => setUninstalling(true)}>
              Delete data…
            </Button>
          </SettingRow>
        ) : (
          <SettingRow label="Uninstall" hint="Removes the chat server. You choose whether to keep its data.">
            <Button variant="danger-quiet" disabled={!canMutate || busy || uninstalling} onClick={() => setUninstalling(true)}>
              Uninstall…
            </Button>
          </SettingRow>
        )}
      </SettingsGroup>
      {error ? <SewFailed what="Couldn’t turn off chat and voice." failure={error} next="Chat is still running. Try again in a moment." /> : null}
      {sheet}
      {uninstalling ? (
        <UninstallSheet
          status={status}
          canMutate={canMutate}
          purgeOnly={removed}
          onClose={() => setUninstalling(false)}
          onDone={(n) => {
            setUninstalling(false)
            notify(n)
            void reload()
          }}
        />
      ) : null}
    </div>
  )
}

function OverviewPanel(props: PanelProps): React.JSX.Element {
  const { status } = props
  const setup = enableLabel(status.state) !== null
  return (
    <div className="space-y-6">
      <StatusGroup {...props} />
      <AttentionGroup {...props} />
      {setup ? <SetupGroup {...props} /> : null}
      <Progress status={status} />
      <UpdatesGroup {...props} />
      <WebAppGroup {...props} />
      <TurnOffGroup {...props} />
    </div>
  )
}

// ── Backups and keys ──────────────────────────────────────────────────────

const KIND_LABEL: Record<string, string> = { scheduled: 'Nightly', manual: 'Made by hand', 'pre-upgrade': 'Before an update' }

function BackupsGroup({ status, sample, canMutate, reload, notify }: PanelProps): React.JSX.Element {
  const [list, setList] = useState<SewBackup[] | null>(sample ? SAMPLE_BACKUPS : null)
  const [listError, setListError] = useState<string | null>(null)
  const [job, setJob] = useState<string | null>(null)
  const [error, setError] = useState<SewFailure | null>(null)
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
            if (j.state === 'dead') setError(sewFailure(Object.assign(new Error(j.last_error ?? 'the backup job gave up'), { code: 'backup_failed' })))
            else notify({ tone: 'ok', text: 'Backup done. It’s at the top of Saved backups.' })
            await Promise.all([reload(), loadList()])
          }
        })
        .catch((e) => {
          if (!stop) setError(sewFailure(e))
        })
    }, POLL_MS)
    return () => {
      stop = true
      window.clearInterval(t)
    }
  }, [job, reload, loadList, notify])

  const backupNow = async (): Promise<void> => {
    setRunning(true)
    setError(null)
    try {
      const r = await runSewBackup()
      if (r.backup) {
        setRunning(false)
        notify({ tone: 'ok', text: 'Backup done. It’s at the top of Saved backups.' })
        await Promise.all([reload(), loadList()])
      } else {
        setJob(r.jobId)
      }
    } catch (e) {
      setRunning(false)
      setError(sewFailure(e))
    }
  }

  const live = status.state === 'running' || status.state === 'degraded'
  const ship = b?.ship
  const shipText =
    ship?.kind === 'dir'
      ? `To ${ship.path ?? 'a folder'}`
      : ship?.kind === 'box-backup'
        ? ship.boxBackup?.ok
          ? 'With this server’s own backup'
          : 'With this server’s own backup, not confirmed yet'
        : 'Not yet'
  const rows = [...(list ?? [])].sort((x, y) => (x.created_at < y.created_at ? 1 : -1))

  return (
    <div className="space-y-6">
      <div className="space-y-3">
        <SettingsGroup
          title="Backups"
          card
          settingId="sew.backup"
          hint="The server backs chat up every night. Back up now before a big change."
          action={
            <Button variant="accent" disabled={!canMutate || !live || running} onClick={() => void backupNow()}>
              {running ? 'Backing up…' : 'Back up now'}
            </Button>
          }
        >
          <SettingRow label="Last backup">
            <span className={b?.lastOkAt ? VALUE : 'type-row text-[var(--color-status-warn-text)]'}>{b?.lastOkAt ? when(b.lastOkAt) : 'Never'}</span>
          </SettingRow>
          <SettingRow
            label="Copy on another machine"
            hint={
              ship?.lastError ? (
                <>
                  The last copy didn’t reach the other machine. Check that its folder is there and has room.
                  <span className="block text-[var(--color-text-muted)]">The server said: {ship.lastError.replace(/[.\s]+$/, '')}.</span>
                </>
              ) : ship?.kind === 'box-backup' && !ship.boxBackup?.ok ? (
                <>
                  K2 can’t see the server’s backup keep them on another machine yet. Check that scheduled backups are turned on.
                  {ship.boxBackup?.reason ? <span className="block text-[var(--color-text-muted)]">The server said: {ship.boxBackup.reason.replace(/[.\s]+$/, '')}.</span> : null}
                </>
              ) : undefined
            }
          >
            <span className={ship?.kind === 'dir' || ship?.boxBackup?.ok ? VALUE : 'type-row text-[var(--color-status-warn-text)]'} data-testid="sew-ship-box-backup">
              {shipText}
            </span>
          </SettingRow>
          <SettingRow
            label="Last restore test"
            hint={
              <>
                Each night the server checks that a backup can be restored.
                {d?.lastOk === false && d.lastError ? <span className="block text-[var(--color-text-muted)]">The server said: {d.lastError.replace(/[.\s]+$/, '')}.</span> : null}
              </>
            }
          >
            <span className={d?.lastOk === false ? 'type-row text-[var(--color-status-warn-text)]' : VALUE}>{d?.lastOkAt ? (d.lastOk === false ? 'Failed' : `Passed ${when(d.lastOkAt)}`) : 'Not reported yet'}</span>
          </SettingRow>
        </SettingsGroup>
        {error ? (
          <SewFailed
            what="Couldn’t back up chat and voice."
            failure={error}
            next="Your earlier backups are untouched. Try again in a moment."
            action={
              <Button disabled={!canMutate || !live || running} onClick={() => void backupNow()}>
                {running ? 'Backing up…' : 'Back up again'}
              </Button>
            }
          />
        ) : null}
      </div>

      <SettingsGroup
        title="Saved backups"
        card
        hint="Restoring one replaces all chat data, so an admin does it from a terminal on the server."
        testId="sew-backups"
        state={
          listError ? (
            <SettingsLoadState kind="error" title="Couldn’t load the saved backups" detail={listError} action={<Button onClick={() => void loadList()}>Try again</Button>}>
              The backups themselves are safe; this page just couldn’t list them.
            </SettingsLoadState>
          ) : list === null ? (
            <SettingsLoadState kind="loading" title="Loading saved backups…" />
          ) : rows.length === 0 ? (
            <SettingsLoadState kind="empty" title="No backups yet">
              Back up now makes the first one. After that the server backs up every night.
            </SettingsLoadState>
          ) : undefined
        }
      >
        {rows.map((x) => (
          <div key={x.id} className="px-4 py-3" data-testid="sew-backup-row" data-backup={x.id}>
            <p className={SETTING_ROW_LABEL}>{when(x.created_at)}</p>
            <p className={SETTING_ROW_HINT}>
              {KIND_LABEL[x.kind] ?? sentence(x.kind)} · {mb(x.bytes)}
            </p>
          </div>
        ))}
      </SettingsGroup>
    </div>
  )
}

function OffboxBlock({
  title,
  hint,
  noun,
  current,
  disabled,
  onSave,
  boxBackup,
}: {
  title: string
  hint: string
  noun: 'Backups' | 'Keys'
  current: { kind: string; path: string | null }
  disabled: boolean
  onSave: (kind: ShipKind, path: string) => Promise<boolean>
  /** The backup destination only: offer the box's own backup (0.46.9). */
  boxBackup?: SewBoxBackup | null | false
}): React.JSX.Element {
  const [kind, setKind] = useState<ShipKind>(current.kind === 'dir' ? 'dir' : current.kind === 'box-backup' && boxBackup !== false ? 'box-backup' : 'none')
  const [path, setPath] = useState(current.path ?? '')
  const [error, setError] = useState<SewFailure | null>(null)
  const [busy, setBusy] = useState(false)
  const save = useSaveStatus()
  const pathId = useId()
  const pathBad = kind === 'dir' && !path.trim().startsWith('/')
  const name = `${noun.toLowerCase()}-kind`
  const changed = kind !== (current.kind === 'dir' || current.kind === 'box-backup' ? current.kind : 'none') || (kind === 'dir' && path.trim() !== (current.path ?? ''))

  const run = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      if (await onSave(kind, path)) save.saved()
    } catch (e) {
      setError(sewFailure(e))
    } finally {
      setBusy(false)
    }
  }

  const option = (k: ShipKind, label: string, sub: string, aria: string): React.JSX.Element => (
    <label className={`flex items-start gap-3 px-4 py-3 ${disabled ? '' : 'cursor-pointer'}`}>
      <Radio name={name} className="mt-0.5" checked={kind === k} disabled={disabled} onChange={() => setKind(k)} aria-label={aria} />
      <span className="min-w-0">
        <span className={SETTING_ROW_LABEL}>{label}</span>
        <span className={`block ${SETTING_ROW_HINT}`}>{sub}</span>
      </span>
    </label>
  )

  return (
    <SettingsGroup title={title} hint={hint} card>
      {option('none', 'Only on this server', `A disk loss takes the ${noun.toLowerCase()} too.`, `${noun}: only on this server`)}
      {option('dir', 'A folder on another disk', 'A mounted disk or network share this server can write to.', `${noun}: a folder on another disk`)}
      {boxBackup !== false
        ? option('box-backup', 'With this server’s own backup', 'The server’s scheduled backup keeps them on another machine. The owner chooses this.', `${noun}: with this server's own backup`)
        : null}
      {kind === 'box-backup' && current.kind === 'box-backup' && boxBackup ? (
        <p className={`px-4 py-3 type-hint ${boxBackup.ok ? 'text-[var(--color-text-secondary)]' : 'text-[var(--color-status-warn-text)]'}`}>
          {boxBackup.ok ? `${PRODUCT_NAME} sees the server’s scheduled backup keep them on another machine.` : `${PRODUCT_NAME} can’t see the server’s backup keep them on another machine yet. Check that scheduled backups are turned on.`}
          {!boxBackup.ok && boxBackup.reason ? <span className="block text-[var(--color-text-muted)]">The server said: {boxBackup.reason.replace(/[.\s]+$/, '')}.</span> : null}
        </p>
      ) : null}
      {kind === 'dir' ? (
        <div className="px-4 py-3">
          <label htmlFor={pathId} className={DIALOG_LABEL}>
            Folder
          </label>
          <Input id={pathId} aria-label={`${noun} folder`} value={path} disabled={disabled} placeholder="/mnt/offsite/chat" onChange={(e) => setPath(e.target.value)} className="w-full max-w-md" />
          {pathBad && path ? <p className="type-hint text-[var(--color-status-error-soft)] mt-1">Use a full path that starts with /.</p> : null}
        </div>
      ) : null}
      <div className="px-4 py-3 space-y-2">
        {error ? <SewFailed what={`Couldn’t save where ${noun.toLowerCase()} go.`} failure={error} next="Nothing changed. Check the folder, then try again." /> : null}
        <div className="flex items-center justify-end gap-3">
          <SaveNote state={save.state} />
          <Button variant="accent" disabled={disabled || busy || pathBad || !changed} onClick={() => void run()}>
            {busy ? 'Saving…' : 'Save'}
          </Button>
        </div>
      </div>
    </SettingsGroup>
  )
}

function OffboxGroups({ status, canMutate, reload }: PanelProps): React.JSX.Element {
  const { ask, sheet } = useAsk()
  const ship = status.backup?.ship ?? { kind: 'none', path: null }
  const mirror = status.mirror ?? { kind: 'none', path: null }
  return (
    <div className="space-y-6" data-settings-id="sew.offbox">
      <OffboxBlock
        title="Where copies of backups go"
        hint="Backups hold the encrypted chat data. Keep a copy on a different machine from this server."
        noun="Backups"
        current={{ kind: ship.kind, path: ship.path ?? null }}
        boxBackup={ship.boxBackup ?? null}
        disabled={!canMutate}
        onSave={async (kind, path) => {
          await setSewShip(kind, path)
          await reload()
          return true
        }}
      />
      <OffboxBlock
        title="Where copies of keys go"
        hint="The keys open the backups. Keep a copy on another machine too, apart from the backups."
        noun="Keys"
        current={mirror}
        boxBackup={false}
        disabled={!canMutate}
        onSave={async (kind, path) => {
          if (kind === 'box-backup') return false
          const ok = await ask({
            title: 'Change where copies of keys go?',
            message: 'Chat restarts to apply this, and reconnects on its own within a few seconds.',
            confirmLabel: 'Save and restart',
          })
          if (!ok) return false
          await setSewMirror(kind, path)
          await reload()
          return true
        }}
      />
      {sheet}
    </div>
  )
}

function RecoveryKeyGroup({ status, sample }: PanelProps): React.JSX.Element | null {
  const you = youOf(status)
  const { confirm, ui } = usePersonConfirm()
  const { ask, sheet } = useAsk()
  const [key, setKey] = useState<string | null>(null)
  const [error, setError] = useState<SewFailure | null>(null)
  const [showing, setShowing] = useState(false)
  const viewerReadOnly = useWindowModeStore((s) => s.resolved && s.mode === 'viewer')
  if (status.state === 'not-installed') return null
  const can = !sample && !viewerReadOnly && you.canRecoveryKey

  const reveal = async (): Promise<void> => {
    setError(null)
    const ok = await ask({ title: 'Show the recovery key?', message: RECOVERY_WARNING, confirmLabel: 'Continue' })
    if (!ok) return
    setShowing(true)
    try {
      const token = await confirm('sew-recovery-key', confirmSecretId('sew-recovery-key', status.installId), 'show the chat recovery key')
      if (!token) return
      const r = await revealRecoveryKey(token)
      setKey(r.recoveryKey)
    } catch (e) {
      const info = sewErrorInfo(e)
      setError(sewFailure(e))
    } finally {
      setShowing(false)
    }
  }

  const inVault = status.recoveryKeyInVault
  return (
    <div className="space-y-3" data-settings-id="sew.recovery-key">
      <SettingsGroup title="Recovery key" card hint="With a backup and the key copy, this key reads every message. Without it, backups can’t be opened.">
        <SettingRow
          label="Recovery key"
          hint={
            <span data-testid={inVault ? 'sew-recovery-in-vault' : undefined}>
              {inVault
                ? `It’s in the ${PRODUCT_NAME} vault (Vault → ${PRODUCT_NAME}’s own secrets), so the vault’s own backups cover it. You don’t need to copy it.`
                : can
                  ? 'Show it with a confirm that it’s you.'
                  : 'The server owner or an admin can show it, after confirming it’s them.'}
            </span>
          }
        >
          <Button disabled={!can || showing} onClick={() => void reveal()}>
            {showing ? 'Showing…' : 'Show recovery key'}
          </Button>
        </SettingRow>
      </SettingsGroup>
      {!inVault && status.recoveryKeySaved === false && isLive(status.state) ? (
        <Callout tone="warn" className="type-hint">
          The recovery key isn’t in the vault yet. K2 copies it there on its own, at start or when you show it.
        </Callout>
      ) : null}
      {error ? <SewFailed what="Couldn’t show the recovery key." failure={error} next="Nothing changed. Try again." /> : null}
      {sheet}
      {ui}
      {key !== null ? (
        <OnceSecret title="Chat recovery key" note={`It stays in the ${PRODUCT_NAME} vault. Copy it only if you want a copy outside ${PRODUCT_NAME}.`} warning={RECOVERY_WARNING} secret={key} onDone={() => setKey(null)} />
      ) : null}
    </div>
  )
}

function BackupsPanel(props: PanelProps): React.JSX.Element {
  return (
    <div className="space-y-6">
      <BackupsGroup {...props} />
      <OffboxGroups {...props} />
      <RecoveryKeyGroup {...props} />
    </div>
  )
}

// ── App Spaces (K31) ──────────────────────────────────────────────────────

function BindSheet({ onClose, onBound }: { onClose: () => void; onBound: (s: { app: string; spaceId: string }) => void }): React.JSX.Element {
  const [app, setApp] = useState('')
  const [spaceId, setSpaceId] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<SewFailure | null>(null)
  const formId = useId()
  const appId = useId()
  const spaceFieldId = useId()
  const ready = !!app.trim() && !!spaceId.trim() && !busy
  const run = async (): Promise<void> => {
    if (!ready) return
    setBusy(true)
    setError(null)
    try {
      await bindSewAppSpace(app, spaceId)
      onBound({ app: app.trim(), spaceId: spaceId.trim() })
    } catch (e) {
      setError(sewFailure(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <AccessSheet
      title="Connect an App Space"
      intro="People who can use the App join the Space. Who that is comes from the App’s access in User Access."
      onClose={onClose}
      busy={busy}
      testId="sew-bind-dialog"
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" form={formId} variant="accent" size="md" disabled={!ready}>
            {busy ? 'Connecting…' : 'Connect'}
          </Button>
        </>
      }
    >
      <form
        id={formId}
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault()
          void run()
        }}
      >
        <div>
          <label htmlFor={appId} className={DIALOG_LABEL}>
            App name, as Settings → Apps shows it
          </label>
          <Input id={appId} aria-label="App name" autoFocus value={app} placeholder="For example: jobs" onChange={(e) => setApp(e.target.value)} className={DIALOG_FIELD} />
        </div>
        <div>
          <label htmlFor={spaceFieldId} className={DIALOG_LABEL}>
            Space ID, from the App’s settings
          </label>
          <Input id={spaceFieldId} aria-label="Space ID" value={spaceId} placeholder="For example: spc_01HX4K2M9Q" onChange={(e) => setSpaceId(e.target.value)} className={DIALOG_FIELD} />
          <p className={`${DIALOG_MUTED} mt-1`}>In the App, open the Space’s settings and copy its ID.</p>
        </div>
        {error ? <SewFailed what="Couldn’t connect the Space." failure={error} next="Check the App name and the Space ID, then try again." /> : null}
      </form>
    </AccessSheet>
  )
}

function UnbindSheet({ space, onClose, onDone }: { space: SewAppSpace; onClose: () => void; onDone: (removed: boolean) => void }): React.JSX.Element {
  const [remove, setRemove] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<SewFailure | null>(null)
  const run = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      await unbindSewAppSpace(space.app, space.spaceId, remove)
      onDone(remove)
    } catch (e) {
      setError(sewFailure(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <AccessSheet
      title={`Disconnect ${appSpace(space.app)}?`}
      intro="The App stops adding people to this Space. The Space and its messages stay in chat."
      onClose={onClose}
      busy={busy}
      testId="sew-unbind-choice"
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy} autoFocus>
            Cancel
          </Button>
          <Button variant="danger-muted" size="md" disabled={busy} onClick={() => void run()}>
            {busy ? 'Disconnecting…' : 'Disconnect'}
          </Button>
        </>
      }
    >
      <div role="radiogroup" aria-label="Who stays in the Space" className="space-y-3">
        <label className="flex items-start gap-2 cursor-pointer">
          <Radio name="sew-unbind" className="mt-0.5" checked={!remove} onChange={() => setRemove(false)} aria-label="Keep everyone" />
          <span>
            <span className={SETTING_ROW_LABEL}>Keep everyone</span>
            <span className={`block ${DIALOG_MUTED}`}>Everyone stays in the Space. You can connect it again later.</span>
          </span>
        </label>
        <label className="flex items-start gap-2 cursor-pointer">
          <Radio name="sew-unbind" className="mt-0.5" checked={remove} onChange={() => setRemove(true)} aria-label={`Remove the people ${PRODUCT_NAME} added`} />
          <span>
            <span className={SETTING_ROW_LABEL}>Remove the people K2 added</span>
            <span className={`block ${DIALOG_MUTED}`}>People someone added by hand always stay.</span>
          </span>
        </label>
      </div>
      {error ? <SewFailed what="Couldn’t disconnect the Space." failure={error} next="Nothing changed. Try again in a moment." /> : null}
    </AccessSheet>
  )
}

function AppSpacesPanel({ sample, canMutate, notify }: PanelProps): React.JSX.Element {
  const [spaces, setSpaces] = useState<SewAppSpace[] | null>(sample ? SAMPLE_APP_SPACES : null)
  const [notBuilt, setNotBuilt] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [dialog, setDialog] = useState<{ kind: 'connect' } | { kind: 'disconnect'; space: SewAppSpace } | null>(null)

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

  const people = (n: number, one: string, many: string): string => `${n} ${n === 1 ? one : many}`

  return (
    <div className="space-y-3" data-settings-id="sew.app-spaces">
      <SettingsGroup
        title="App Spaces"
        card
        hint="Each App’s shared chat rooms. People join when they can use the App; Admin Access never adds anyone."
        action={
          canMutate && !notBuilt ? (
            <Button variant="accent" disabled={dialog !== null} onClick={() => setDialog({ kind: 'connect' })}>
              Connect an App Space
            </Button>
          ) : undefined
        }
        testId="sew-app-spaces"
        state={
          notBuilt ? (
            <SettingsLoadState kind="empty" title="This server doesn’t report App Spaces yet">
              Update chat from the Overview tab to manage App Spaces here.
            </SettingsLoadState>
          ) : error ? (
            <SettingsLoadState kind="error" title="Couldn’t load App Spaces" detail={error} action={<Button onClick={() => void load()}>Try again</Button>}>
              The Spaces themselves are fine; this page just couldn’t list them.
            </SettingsLoadState>
          ) : spaces === null ? (
            <SettingsLoadState kind="loading" title="Loading App Spaces…" />
          ) : spaces.length === 0 ? (
            <SettingsLoadState kind="empty" title="No App Spaces yet">
              Connect an App Space, and the people who can use the App join it.
            </SettingsLoadState>
          ) : undefined
        }
      >
        {(spaces ?? []).map((s) => (
          <div key={`${s.app}/${s.spaceId}`} data-testid="sew-app-space" className={`flex items-center justify-between gap-6 px-4 py-3 ${SETTING_LIST_ROW_HOVER}`}>
            <div className="min-w-0">
              <p className={SETTING_ROW_LABEL}>{appName(s.app)} App</p>
              <p className={SETTING_ROW_HINT}>
                {[
                  people(s.members.added, 'member', 'members'),
                  s.members.pending > 0 ? `${s.members.pending} waiting to join` : null,
                  s.members.held > 0 ? `${s.members.held} on hold until their App access is fixed` : null,
                ]
                  .filter(Boolean)
                  .join(' · ')}
              </p>
              <p className="type-hint text-[var(--color-text-muted)] break-all">
                <span className="sr-only">Space ID </span>
                <span className="font-mono">{s.spaceId}</span>
              </p>
            </div>
            {canMutate ? (
              <Button variant="danger-quiet" className="flex-shrink-0" aria-label={`Disconnect ${appSpace(s.app)}`} disabled={dialog !== null} onClick={() => setDialog({ kind: 'disconnect', space: s })}>
                Disconnect…
              </Button>
            ) : null}
          </div>
        ))}
      </SettingsGroup>
      {dialog?.kind === 'connect' ? (
        <BindSheet
          onClose={() => setDialog(null)}
          onBound={(b) => {
            setDialog(null)
            notify({ tone: 'ok', text: `Connected ${appSpace(b.app)}. People who can use the App join it.` })
            void load()
          }}
        />
      ) : null}
      {dialog?.kind === 'disconnect' ? (
        <UnbindSheet
          space={dialog.space}
          onClose={() => setDialog(null)}
          onDone={(removed) => {
            const app = dialog.space.app
            setDialog(null)
            const what = appSpace(app)
            notify({ tone: 'ok', text: removed ? `Disconnected ${what} and removed the people ${PRODUCT_NAME} added.` : `Disconnected ${what}. Everyone stays in it.` })
            void load()
          }}
        />
      ) : null}
    </div>
  )
}

// ── the section root ──────────────────────────────────────────────────────

export function SewSection(): React.JSX.Element {
  const viewerReadOnly = useWindowModeStore((s) => s.resolved && s.mode === 'viewer')
  const [status, setStatus] = useState<SewStatus | null>(null)
  const [loadError, setLoadError] = useState<{ forbidden: boolean; message: string } | null>(null)
  const [tab, setTab] = useState<Tab>('overview')
  const [anchor, setAnchor] = useState<string | null>(null)
  const [notice, setNotice] = useState<Notice | null>(null)
  const mounted = useRef(true)

  const load = useCallback(async () => {
    try {
      const s: unknown = await fetchSewStatus(true)
      if (!mounted.current) return
      if (!isSewStatus(s)) {
        setLoadError({ forbidden: false, message: 'the reply had no status in it' })
        return
      }
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

  // A jump from an alarm lands on its group.
  useEffect(() => {
    if (!anchor) return
    const el = document.querySelector(`[data-settings-id="${anchor}"]`)
    el?.scrollIntoView?.({ block: 'start', behavior: 'smooth' })
    setAnchor(null)
  }, [anchor, tab])

  const go = useCallback((t: Tab, a?: string) => {
    setTab(t)
    setAnchor(a ?? null)
  }, [])
  const notify = useCallback((n: Notice) => setNotice(n), [])

  const sample = status !== null && !status.supported
  const view: SewStatus | null = status === null ? null : sample ? SAMPLE_STATUS : status
  const live = view !== null && (sample || isLive(view.state))
  const tabs = live ? TABS : (['overview'] as Tab[])
  const shown: Tab = tabs.includes(tab) ? tab : 'overview'

  const header = (
    <header className="min-w-0">
      <h2 className="type-title text-[var(--color-text-primary)]">Chat / Voice</h2>
      <p className="type-hint text-[var(--color-text-secondary)] mt-1 leading-relaxed max-w-[75ch]">
        Team chat and voice calls for the people and Apps on this server. K2 sets them up, keeps them healthy and backs them up.
      </p>
    </header>
  )

  const frame = (body: React.ReactNode): React.JSX.Element => (
    <div className="h-full min-h-0 overflow-y-auto">
      <div className="p-6">
        <div className="w-full max-w-[52rem] space-y-5 pb-6">{body}</div>
      </div>
    </div>
  )

  if (status === null || view === null) {
    if (loadError?.forbidden) {
      return frame(
        <>
          {header}
          <SettingsLoadState
            kind="empty"
            title="You can’t manage chat and voice"
            testId="sew-forbidden"
            action={
              <>
                <Button variant="quiet" onClick={() => useSettingsStore.getState().openSettings('k2-access')}>
                  See who’s an admin
                </Button>
                <Button onClick={() => void load()}>Check again</Button>
              </>
            }
          >
            {SEW_NO_PERMISSION} Ask the server’s owner or an admin to make the change, or to give you admin access, then check again.
          </SettingsLoadState>
        </>,
      )
    }
    if (loadError) {
      return frame(
        <>
          {header}
          <SettingsLoadState kind="error" title="Couldn’t read chat and voice status" detail={loadError.message} action={<Button onClick={() => void load()}>Try again</Button>} testId="sew-load-error">
            Chat itself isn’t affected; this page just couldn’t ask the server how it is.
          </SettingsLoadState>
        </>,
      )
    }
    return frame(
      <>
        {header}
        <SettingsLoadState kind="loading" title="Reading chat and voice status…" testId="sew-loading">
          Asking the server how chat and calls are doing.
        </SettingsLoadState>
      </>,
    )
  }

  const canMutate = !sample && !viewerReadOnly && youOf(status).canManage
  const props: PanelProps = { status: view, sample, canMutate, reload: load, go, notify }
  const alarmCount = (view.alarms ?? []).length

  return frame(
    <>
      {header}
      {sample ? <LinuxOnlyNote what="Chat and voice" /> : null}
      {!sample && !youOf(status).canManage ? (
        <p className="type-hint text-[var(--color-text-secondary)]" data-testid="sew-read-only">
          You can see these settings. Only the server owner, an admin or an agent allowed to manage chat can change them.
        </p>
      ) : null}
      {loadError ? (
        <Callout tone="error" role="alert" className="type-hint">
          Couldn’t refresh the status, so this may be out of date. The server said: {loadError.message.replace(/[.\s]+$/, '')}.
        </Callout>
      ) : null}
      {notice ? (
        <Callout tone={notice.tone} role={notice.tone === 'error' ? 'alert' : 'status'} className="flex items-start justify-between gap-4" data-testid="sew-notice">
          <span className="type-hint space-y-0.5">
            <span className="block font-medium">{notice.text}</span>
            {notice.detail ? <span className="block">{notice.detail}</span> : null}
          </span>
          <Button variant="quiet" className="flex-shrink-0" onClick={() => setNotice(null)}>
            Dismiss
          </Button>
        </Callout>
      ) : null}

      <div role="tablist" aria-label="Chat / Voice" className="flex flex-wrap gap-0.5 border-b border-[var(--color-border)] w-full">
        {tabs.map((t) => {
          const active = shown === t
          return (
            <button
              key={t}
              type="button"
              role="tab"
              aria-selected={active}
              onClick={() => setTab(t)}
              data-settings-id={TAB_SETTINGS_ID[t]}
              data-k2-focus=""
              className={`relative px-3 py-2 type-control transition-colors no-drag cursor-pointer border-b-2 -mb-px outline-none ${
                active ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]' : 'border-transparent text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]'
              }`}
            >
              {TAB_LABEL[t]}
              {t === 'overview' && alarmCount > 0 ? <span className="sr-only">{`, ${alarmCount} alarm${alarmCount === 1 ? '' : 's'}`}</span> : null}
            </button>
          )
        })}
      </div>

      <div className="pt-1" role="tabpanel" aria-label={TAB_LABEL[shown]}>
        {shown === 'overview' ? <OverviewPanel {...props} /> : null}
        {shown === 'backups' ? <BackupsPanel {...props} /> : null}
        {shown === 'app-spaces' ? <AppSpacesPanel {...props} /> : null}
        {shown === 'recording' ? <RecordingPanel sample={sample} canMutate={canMutate} isOwner={youOf(status).isOwner} installId={status?.installId ?? null} /> : null}
      </div>
    </>,
  )
}

