// Settings → Cores → Chat Hosting (Sew S32, K27) — typed bindings for the
// Sew server on this box: `/cli/sew/*` (S29 supervisor, S32 additions) and
// the vault person-confirm used by the person-only steps.
//
// Thin client: the daemon owns every decision (state machine, health loop,
// backups, grant hook). This file reads that truth and sends gestures. No
// component builds a route string; every call goes through here.
//
// Secrets: the recovery key and the owner-handoff code pass through these
// functions as return values only. Nothing here logs, stores or toasts them.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'
import { readConfirmIdentity, type ConfirmIdentity } from './confirm-password-ui'

/** Every CLI hint names the noun through this (one-line rename). */
export const SEW_CLI = 'k2 hostchat'


// ── wire types ────────────────────────────────────────────────────────────

export type SewState =
  | 'not-installed'
  | 'installing'
  | 'running'
  | 'degraded'
  | 'stopped'
  | 'error'
  | 'disabled'
  | 'upgrading'
  | 'removed'

/** What THIS caller may do (daemon-computed; a 200 on status never means
 *  the client may edit, S32-A5). */
export interface SewYou {
  canManage: boolean
  isOwner: boolean
  isAdmin: boolean
  passport: boolean
  canRecoveryKey: boolean
  canPurge: boolean
  canOpenAsOwner: boolean
}

export interface SewWarning {
  code: string
  alarm?: string
  message: string
  fix?: string
}

export interface SewUnit {
  active: string
  result?: string
  execMainStatus?: number
  nRestarts?: number
  memoryCurrent?: number | null
  memoryMax?: number | null
}

/** The pinned release file the daemon is fetching or used (0.46.7). */
export interface SewDownload {
  file?: string
  label?: string
  url?: string
  /** `download`, `cache` or `tarball`. */
  source?: string
  bytes?: number
  total?: number | null
}

export interface SewProgress {
  steps?: Record<string, { at?: number; seq?: number }>
  current?: string | null
  download?: SewDownload | null
  result?: string | null
  owner?: string
  from?: string
  to?: string
}

/** Ship kind `box-backup` (0.46.9): what K2 sees of the box's own backup. */
export interface SewBoxBackup {
  ok: boolean
  reason?: string | null
  reasons?: string[]
  coveredBy?: string | null
  excludedBy?: string | null
  timer?: string
  line?: string
}

export interface SewShip {
  kind: string
  path?: string | null
  /** Kind `box-backup` only. */
  boxBackup?: SewBoxBackup | null
  lastShippedId?: string | null
  lastShippedAt?: number | string | null
  lastError?: string | null
  lastErrorAt?: number | string | null
}

export interface SewBackupStatus {
  ship: SewShip
  lastOkAt?: string | null
  lastId?: string | null
  lastKind?: string | null
  ageMs?: number | null
  lastError?: string | null
  lastErrorAt?: string | null
  alarms?: string[]
}

export interface SewDrillStatus {
  lastOkAt?: string | null
  lastOk?: boolean | null
  lastError?: string | null
  ageMs?: number | null
  alarms?: string[]
}

export interface SewStatus {
  ok?: boolean
  supported: boolean
  released?: boolean
  notice?: string
  state: SewState | string
  consistent?: boolean
  note?: string | null
  pinnedVersion?: string
  installedVersion?: string | null
  previousVersion?: string | null
  onDiskVersion?: string | null
  upgradeAvailable?: boolean
  installId?: string | null
  identityProvider?: string
  units?: { sewDb?: SewUnit; sew?: SewUnit }
  alarms?: string[]
  warnings?: SewWarning[]
  /** The vault holds the current recovery key (same as `recoveryKeyInVault`
   *  since 0.46.7; older daemons: it was revealed once). */
  recoveryKeySaved?: boolean
  /** 0.46.7: Sew's recovery key is in the K2 vault (`recoveryKeyVaultName`). */
  recoveryKeyInVault?: boolean
  recoveryKeyVaultName?: string
  masterKeyAgeDays?: number | null
  rotationInProgress?: boolean
  backup?: SewBackupStatus
  drill?: SewDrillStatus
  mirror?: { kind: string; path: string | null }
  appUrl?: string | null
  publicUrl?: string | null
  owner?: 'set' | 'pending' | 'blocked' | 'none'
  target?: { enabled: boolean }
  voice?: Record<string, unknown> | null
  enableProgress?: SewProgress | null
  upgradeProgress?: SewProgress | null
  restoreProgress?: SewProgress | null
  lastError?: string | null
  purgeConfirmHostname?: string
  you?: SewYou
}

export interface SewPreflightCheck {
  id: string
  status: 'pass' | 'warn' | 'fail' | string
  detail: string
  hint: string
}

export interface SewBackup {
  id: string
  kind: string
  created_at: string
  bytes: number
  schema_version?: number
  sew_version?: string
}

export interface SewJob {
  id: string
  kind: string
  state: 'queued' | 'running' | 'succeeded' | 'failed' | 'dead' | string
  attempts?: number
  last_error?: string | null
  result?: unknown
}

export interface SewAppSpace {
  app: string
  spaceId: string
  boundAt?: number | null
  members: { added: number; held: number; pending: number }
}

export interface SewEnableRequest {
  identity: 'k2' | 'standalone'
  voice?: { publicUrl?: string; nodeIp?: string; mediaTarballPath?: string }
  tarballPath?: string
  recoveryKeyFile?: string
}

// ── errors ────────────────────────────────────────────────────────────────

export interface SewErrorInfo {
  code: string | null
  status: number | null
  message: string
}

/** The daemon's `{ok:false, error:{code, hint}}` as the thrown
 *  DaemonHttpError carries it (`message` = hint, `.code`, `.status`). */
export function sewErrorInfo(e: unknown): SewErrorInfo {
  const rec = e && typeof e === 'object' ? (e as { code?: unknown; status?: unknown }) : {}
  const code = typeof rec.code === 'string' && rec.code ? rec.code : null
  const status = typeof rec.status === 'number' ? rec.status : null
  const message = e instanceof Error ? e.message : typeof e === 'string' ? e : 'Something went wrong.'
  return { code, status, message }
}

/** The no-permission line, everywhere the page says it. */
export const SEW_NO_PERMISSION = 'Only the server owner or an admin can manage chat and voice.'

/** What a person reads for a failed Sew call. */
export function sewErrorMessage(e: unknown): string {
  const info = sewErrorInfo(e)
  if (info.code === 'owner_only' || (info.status === 403 && !info.code)) return SEW_NO_PERMISSION
  if (info.code === 'lifecycle_in_progress') return 'Another chat and voice change is still running. Wait for it to finish'
  return info.message
}

/** A status reply the page can render: the daemon always sends a boolean
 *  `supported` and a string `state` (`supervisor::status_body`); anything
 *  else is treated as a failed read, never as a Mac. */
export function isSewStatus(v: unknown): v is SewStatus {
  const r = v && typeof v === 'object' ? (v as Record<string, unknown>) : null
  return !!r && typeof r.supported === 'boolean' && typeof r.state === 'string' && r.state !== ''
}

/** A route the daemon doesn't have yet (older daemon / S30–S31 not built). */
export function isNotBuilt(e: unknown): boolean {
  const { status, code } = sewErrorInfo(e)
  return status === 404 || status === 501 || code === 'not_built'
}

// ── tables ────────────────────────────────────────────────────────────────

/** The enable machine's steps, in `supervisor::enable_steps` order. */
export const SEW_ENABLE_STEPS: { id: string; label: string }[] = [
  { id: 'postgres', label: 'Install the database' },
  { id: 'master-key', label: 'Create the encryption key' },
  { id: 'stage', label: 'Download the chat server' },
  { id: 'install-bin', label: 'Install the chat program' },
  { id: 'install', label: 'Set up the chat server' },
  { id: 'enable-units', label: 'Turn on its services' },
  { id: 'db', label: 'Start the database' },
  { id: 'migrate', label: 'Prepare the database' },
  { id: 'start', label: 'Start chat' },
  { id: 'token', label: 'Connect K2 to chat' },
  { id: 'health', label: 'Check that it answers' },
  { id: 'link', label: 'Finish linking to K2' },
]

export const SEW_PREFLIGHT_LABELS: Record<string, string> = {
  os: 'Operating system',
  arch: 'Processor',
  systemd: 'Service manager',
  postgres: 'Database (PostgreSQL)',
  helper: 'K2’s system helper',
  pin: 'Chat release',
  source: 'Where the chat server comes from',
  ram: 'Memory',
  disk: 'Disk space',
  port: 'Chat’s network port',
  binary: 'Chat program',
  existing_data: 'Chat data already on this server',
  daemon_port: 'K2’s network port',
  vault: 'K2 vault',
  voice_ports: 'Voice call ports',
}

/** Checks the inputs can fix: enable stays possible while they fail. */
export const SEW_FIXABLE_CHECKS = ['source', 'existing_data']

/** "12.3 of 24.7 MB" / "from K2's cache" beside the download step. */
export function downloadNote(d: SewDownload | null | undefined): string | null {
  if (!d) return null
  if (d.source === 'cache') return `${d.label ?? 'The chat server'} from K2's download cache`
  if (d.source === 'tarball') return `${d.label ?? 'The chat server'} from the file you named`
  if (d.source !== 'download' || typeof d.bytes !== 'number') return null
  const mb = (n: number): string => (n / 1e6).toFixed(1)
  return typeof d.total === 'number' && d.total > 0
    ? `${d.label ?? 'The chat server'}: ${mb(d.bytes)} of ${mb(d.total)} MB`
    : `${d.label ?? 'The chat server'}: ${mb(d.bytes)} MB`
}

export type SewAlarmAction = 'backup' | 'offbox' | 'recovery-key' | 'public-url'

export interface SewAlarmInfo {
  severity: 'error' | 'warn'
  title: string
  /** What it means, in plain words. */
  fix: string
  /** What to ask a server admin to do, in plain words (with `command`). */
  ask?: string
  /** The command that admin runs, shown quietly with Copy. */
  command?: string
  action?: SewAlarmAction
}

const cli = (verb: string): string => `${SEW_CLI} ${verb}`
const HEALTH = { ask: 'Ask an admin to run a health check on the server. It says what to do next.', command: cli('doctor') }
const RENEW = { ask: 'Ask an admin to renew the voice certificate on the server.', command: cli('media renew') }

/** Every alarm Sew and K2 report (S29 §8.3 table, `supervisor::k2_alarms`,
 *  `voice::status`, `sew_wire::BACKUP_ALARMS`). Unknown alarms render
 *  with their name via {@link alarmInfo}. */
export const SEW_ALARMS: Record<string, SewAlarmInfo> = {
  db_down: { severity: 'error', title: 'Chat’s database is down', fix: 'The chat server can’t reach its database, so it can’t load or save messages.', ...HEALTH },
  jobs_dead: { severity: 'warn', title: 'Some background jobs failed', fix: 'Chat gave up on some background work.', ...HEALTH },
  keys_provider_down: { severity: 'error', title: 'Chat can’t read its encryption key', fix: 'Chat can’t open any message until it can.', ...HEALTH },
  keys_current_month_missing: { severity: 'error', title: 'This month’s encryption key is missing', fix: 'New messages can’t be saved.', ...HEALTH },
  keys_mirror_stale: { severity: 'warn', title: 'Keys aren’t kept on another machine', fix: 'Choose where copies of the keys go, under Backups and keys.', action: 'offbox' },
  // Sew 0.10.5: a folder copy of the keys is checked; while one of these
  // stands, chat holds key changes until the copy is good again.
  keys_mirror_unmounted: { severity: 'error', title: 'The folder for copies of the keys isn’t there', fix: 'Chat holds key changes until the other disk is mounted again, or you choose another place under Backups and keys.', action: 'offbox' },
  keys_mirror_same_device: { severity: 'error', title: 'Copies of the keys are on the same disk', fix: 'A copy on the same disk doesn’t survive losing it, so chat holds key changes. Choose a folder on another disk under Backups and keys.', action: 'offbox' },
  keys_mirror_unsynced: { severity: 'warn', title: 'Copies of the keys are behind', fix: 'Some key changes haven’t reached the copy yet. Check that the other disk is mounted and has space, under Backups and keys.', action: 'offbox' },
  keys_shred_pending: { severity: 'warn', title: 'A key is still being deleted', fix: 'Chat finishes this on its own. Nothing to do unless it stays.' },
  keys_resurrected: { severity: 'error', title: 'A deleted key came back', fix: 'A restored copy of the keys holds a key that was deleted.', ...HEALTH },
  keys_rewrap_stalled: { severity: 'warn', title: 'Changing the encryption key stalled', fix: 'The key change stopped partway.', ask: 'Ask an admin to start the key change again on the server, then run a health check.', command: cli('rotate-master') },
  search_lagging: { severity: 'warn', title: 'Search is behind', fix: 'New messages may not show up in search yet. It catches up on its own.' },
  backup_stale: { severity: 'warn', title: 'No backup in 36 hours', fix: 'Back up now, and check that scheduled backups run.', action: 'backup' },
  backup_failed: { severity: 'error', title: 'The last backup failed', fix: 'Back up now to see what went wrong.', action: 'backup' },
  drill_failed: { severity: 'error', title: 'The last restore test failed', fix: 'A backup couldn’t be restored in the test.', ...HEALTH },
  drill_stale: { severity: 'warn', title: 'No restore test in 8 days', fix: 'The server tests a backup every day. Check that scheduled restore tests run.' },
  k2_mgmt_unreachable: { severity: 'error', title: 'K2 can’t reach the chat server', fix: 'Chat’s services run, but K2 can’t talk to them.', ...HEALTH },
  k2_recovery_key_not_saved: { severity: 'warn', title: 'Recovery key isn’t in the vault yet', fix: 'K2 copies it into the vault on its own, at start or when you show it. If this stays, check that the vault is open.', action: 'recovery-key' },
  k2_master_key_due: { severity: 'warn', title: 'Time to change the encryption key', fix: 'Changing it now and then keeps old messages safe if a key ever leaks.', ask: 'Ask an admin to change the key on the server.', command: cli('rotate-master') },
  k2_vault_key_mismatch: { severity: 'error', title: 'K2’s key doesn’t match chat’s', fix: 'Don’t change anything yet.', ...HEALTH },
  k2_helper_pin_mismatch: { severity: 'warn', title: 'K2’s system helper is out of date', fix: 'Update K2 on this server, then update chat.' },
  k2_jwks_url_drift: { severity: 'error', title: 'Chat can’t check K2 sign-ins', fix: 'K2’s network port changed.', ...HEALTH },
  k2_owner_blocked: { severity: 'warn', title: 'Setting you up as owner failed', fix: 'Try again from User Access, where it syncs people to chat.' },
  k2_backup_offbox_none: { severity: 'warn', title: 'Backups stay on this server', fix: 'Choose where copies of the backups go, so a disk loss doesn’t take them too.', action: 'offbox' },
  k2_backup_not_shipped: { severity: 'warn', title: 'The last backup wasn’t copied to another machine', fix: 'Check where copies of the backups go, under Backups and keys.', action: 'offbox' },
  k2_upgrade_rollback_failed: { severity: 'error', title: 'An update failed and couldn’t go back', fix: 'Someone has to fix this on the server itself.', ask: 'Ask an admin to read the update’s record on the server.', command: 'cat /usr/local/lib/sew/upgrade-state.json' },
  k2_turn_cert_missing: { severity: 'error', title: 'Voice calls have no certificate', fix: 'Calls can’t connect securely.', ...RENEW },
  k2_turn_cert_expired: { severity: 'error', title: 'The voice certificate expired', fix: 'Calls can’t connect securely.', ...RENEW },
  k2_turn_cert_expiring: { severity: 'warn', title: 'The voice certificate expires soon', fix: 'K2 renews it on its own before then.', ask: 'To renew it now, ask an admin to renew the voice certificate on the server.', command: cli('media renew') },
  k2_turn_tls_mismatch: { severity: 'error', title: 'Voice uses the wrong certificate', fix: 'Calls may fail to connect.', ...RENEW },
  k2_voice_probe_failed: { severity: 'warn', title: 'A test call from outside failed', fix: 'The server’s firewall may be blocking the voice ports.', ask: 'Ask an admin to open the voice ports, then test a call again on the server.', command: cli('media probe') },
  voice_engine_down: { severity: 'error', title: 'Voice calls are down', fix: 'The voice service stopped.', ask: 'Ask an admin to check the voice service on the server.', command: cli('media status') },
  voice_not_installed: { severity: 'warn', title: 'Voice calls aren’t installed', fix: 'Chat works; calls don’t yet.', ask: 'Ask an admin to install voice on the server.', command: cli('media install') },
}

/** Info for any alarm string; unknown ones keep their name as the title. */
export function alarmInfo(alarm: string): SewAlarmInfo {
  return SEW_ALARMS[alarm] ?? { severity: 'warn', title: alarm, fix: 'K2 has no description for this one yet.', ask: 'Ask an admin to check chat’s status on the server.', command: cli('status') }
}

/** The daemon's setup warnings (`supervisor::split_setup_warnings`,
 *  `public_url_warning`, `voice::status`) in plain words. An unknown one
 *  shows a plain title with the server's own words quietly. */
const SEW_WARNINGS: Record<string, Omit<SewAlarmInfo, 'severity'>> = {
  no_offbox_key_copy: { title: 'Keys aren’t kept on another machine', fix: 'Backups can’t be read after a disk loss without a copy of the keys. Choose where copies of the keys go.', action: 'offbox' },
  public_url_unset: { title: 'Chat’s web address isn’t set', fix: 'Setup links and invites won’t open from outside this server until it is.', action: 'public-url' },
  voice_unprobed: { title: 'Calls haven’t been tested from outside yet', fix: 'Test a call from another network, so you know people outside can join.', ask: 'Then ask an admin to record the test on the server.', command: cli('media probe') },
  voice_probe_stale: { title: 'The last outside call test is out of date', fix: 'Test a call from another network again.', ask: 'Then ask an admin to record the test on the server.', command: cli('media probe') },
}

export function warningInfo(w: SewWarning): Omit<SewAlarmInfo, 'severity'> & { detail?: string } {
  return SEW_WARNINGS[w.code] ?? { title: 'Chat reported something to check', fix: 'K2 has no plain description for this one yet.', detail: w.message }
}

/** A failed call in plain words: `line` says what went wrong (null when
 *  K2 has no plain wording for it), `next` what to do (null: the caller's),
 *  `detail` the server's own words, shown quietly after. Codes and hints
 *  are the daemon's (`sew::supervisor`, `sew::s32`, `sew::voice`). */
export interface SewFailure {
  line: string | null
  next: string | null
  detail: string
  code: string | null
}

export function sewFailure(e: unknown): SewFailure {
  const { code, status, message } = sewErrorInfo(e)
  const hint = message
  const f = (line: string | null, next: string | null = null): SewFailure => ({ line, next, detail: hint, code })
  if (code === 'owner_confirm_required') return f('This step needs you to confirm it’s you first.', 'Try again and confirm when K2 asks.')
  if (code === 'owner_only' || code === 'forbidden' || code === 'wrong_credential' || (status === 403 && !code)) return f(SEW_NO_PERMISSION, 'Ask the server’s owner or an admin to do it.')
  if (code === 'owner_not_ready') return f('Chat is still setting you up as owner.', 'Wait a minute, then try again.')
  if (status === 429) return f('Too many tries.', 'Wait a minute, then try again.')
  if (code === 'backup_busy') return f('A backup is already running.', 'Wait for it to finish; it shows under Saved backups.')
  if (code === 'lifecycle_in_progress' || code === 'upgrade_in_progress' || code === 'rotation_in_progress') return f('Another chat change is still running.', 'Wait for it to finish, then try again.')
  if (code === 'not_installed' || code === 'unknown_sew_binary') return f('Chat isn’t installed on this server.', 'Set it up from the Overview tab first.')
  if (code === 'not_running' || code === 'sew_not_running' || code === 'mgmt_unreachable' || code === 'sew_unreachable' || code === 'sew_unavailable') return f('Chat isn’t running, so it can’t do this right now.', 'Wait for chat to come back (the Overview tab shows it), then try again.')
  if (code === 'backups_unavailable') return f('This chat server can’t make backups.', 'Update chat from the Overview tab, then try again.')
  if (code === 'vault_closed' || code === 'vault_error') return f('The K2 vault is locked, so K2 can’t reach chat’s keys.', 'Open the vault (Settings → Vault), then try again.')
  if (code === 'invalid_path') return f('That folder isn’t there on the server, or K2 can’t write to it.', 'Check that the disk is mounted and the folder exists, then try again.')
  if (code === 'space_bound') return f('That Space is already connected to another App.', 'Check the Space ID, or disconnect it from the other App first.')
  if (code === 'space_not_found') return f('Chat has no Space with that ID.', 'Copy the Space ID again from the App’s settings.')
  if (code === 'app_mismatch') return f('That Space belongs to a different App.', 'Check the App and the Space ID.')
  if (code === 'space_unbound') return f('That Space isn’t connected to an App here.', 'Connect it under App Spaces first.')
  if (code === 'provider_is_standalone') return f('Chat here uses its own accounts, so it has no App Spaces.')
  if (code === 'not_available') return f('That choice isn’t available yet.', 'Pick another one.')
  if (code === 'preflight_failed') return f('The server checks found something to fix first.', 'Fix what’s marked Needs fixing, then try again.')
  if (code === 'config_failed' || code === 'restart_failed' || code === 'reconfigure_failed') return f('Chat didn’t accept the change.', 'Nothing changed. Try again in a moment.')
  if (code === 'db_error' || code === 'io_error') return f('K2 couldn’t save the change on the server.', 'Try again in a moment.')
  if (code === 'unsupported') return f('Chat and voice run on Linux servers only.')
  if (code === 'backup_failed') {
    if (/no space|disk full|ENOSPC|out of (disk|space)/i.test(hint)) return f('The backup ran out of disk space.', 'Free up space on the server, then back up again.')
    return f('The backup didn’t finish.', 'Your earlier backups are untouched. Try again in a moment.')
  }
  if (code === 'bad_request' && /https:\/\/|lowercase, no port, no path/.test(hint)) return f('The address must start with https:// and have no path.', 'Use just the address, like https://chat.example.com, then save again.')
  if (code === 'bad_request' && /kind dir needs path/.test(hint)) return f('Pick a folder for the copies.', null)
  return f(null)
}

/** Status chip label + color token per state. */
export function sewStateChip(state: string): { label: string; color: string } {
  switch (state) {
    case 'not-installed':
      return { label: 'Not installed', color: 'var(--color-text-muted)' }
    case 'installing':
      return { label: 'Setting up…', color: 'var(--color-status-working)' }
    case 'running':
      return { label: 'Running', color: 'var(--color-status-ok)' }
    case 'degraded':
      return { label: 'Degraded', color: 'var(--color-status-warn)' }
    case 'stopped':
      return { label: 'Stopped', color: 'var(--color-status-error-soft)' }
    case 'error':
      return { label: 'Error', color: 'var(--color-status-error-soft)' }
    case 'disabled':
      return { label: 'Turned off', color: 'var(--color-text-muted)' }
    case 'upgrading':
      return { label: 'Updating…', color: 'var(--color-status-working)' }
    case 'removed':
      return { label: 'Uninstalled, data kept', color: 'var(--color-text-muted)' }
    default:
      return { label: state, color: 'var(--color-text-muted)' }
  }
}

/** States the enable wizard shows in, and its button label. */
export function enableLabel(state: string): string | null {
  switch (state) {
    case 'not-installed':
      return 'Set up chat and voice'
    case 'error':
      return 'Resume setup'
    case 'removed':
      // Uninstalled: the page is the fresh setup again (its data comes back).
      return 'Set up chat and voice'
    case 'disabled':
    case 'stopped':
      return 'Turn back on'
    default:
      return null
  }
}

/** A deny-all `you` for an older daemon that doesn't report one. */
export const NO_YOU: SewYou = {
  canManage: false,
  isOwner: false,
  isAdmin: false,
  passport: false,
  canRecoveryKey: false,
  canPurge: false,
  canOpenAsOwner: false,
}

export function youOf(s: SewStatus | null): SewYou {
  return s?.you ?? NO_YOU
}

// ── routes ────────────────────────────────────────────────────────────────

export async function fetchSewStatus(health = true): Promise<SewStatus> {
  return daemonCliGet<SewStatus>(primaryScope(), 'sew/status', health ? { health: 1 } : undefined)
}

const STATUS_TTL_MS = 30_000
let statusCache: { at: number; p: Promise<SewStatus> } | null = null

/** A shared status read for pages that only need `supported`/`state`
 *  (User Access, Admin Access): one read per 30 s, however many mount. */
export function sewStatusCached(): Promise<SewStatus> {
  const now = Date.now()
  if (statusCache && now - statusCache.at < STATUS_TTL_MS) return statusCache.p
  const p = fetchSewStatus(false)
  statusCache = { at: now, p }
  p.catch(() => {
    if (statusCache?.p === p) statusCache = null
  })
  return p
}

export function resetSewStatusCacheForTests(): void {
  statusCache = null
}

/** `tarballPath` (absolute, when the person typed one) makes the daemon's
 *  `source` check judge that file instead of the cache / download. */
export async function fetchSewPreflight(tarballPath?: string): Promise<{ ready: boolean; checks: SewPreflightCheck[] }> {
  const tarball = tarballPath?.trim()
  const r = tarball
    ? await daemonCliGet<{ ready?: boolean; checks?: SewPreflightCheck[] }>(primaryScope(), 'sew/preflight', { tarballPath: tarball })
    : await daemonCliGet<{ ready?: boolean; checks?: SewPreflightCheck[] }>(primaryScope(), 'sew/preflight')
  const checks = Array.isArray(r?.checks) ? r.checks : []
  return { ready: r?.ready === true, checks }
}

/** `identity` is always explicit (fixed for the life of an install); the
 *  voice block and the paths go only when set. */
export function enableBody(req: SewEnableRequest): Record<string, unknown> {
  const body: Record<string, unknown> = { identity: req.identity }
  const tarball = req.tarballPath?.trim()
  const recovery = req.recoveryKeyFile?.trim()
  if (tarball) body.tarballPath = tarball
  if (recovery) body.recoveryKeyFile = recovery
  if (req.voice) {
    const v: Record<string, string> = {}
    const url = req.voice.publicUrl?.trim()
    const ip = req.voice.nodeIp?.trim()
    const media = req.voice.mediaTarballPath?.trim()
    if (url) v.publicUrl = url
    if (ip) v.nodeIp = ip
    if (media) v.mediaTarballPath = media
    body.voice = v
  }
  return body
}

export async function enableSew(req: SewEnableRequest): Promise<{ ok: boolean; state?: string }> {
  return daemonCliPost(primaryScope(), 'sew/server/enable', enableBody(req))
}

export async function upgradeSew(): Promise<{ ok: boolean; state?: string }> {
  return daemonCliPost(primaryScope(), 'sew/server/upgrade', {})
}

export async function rollbackSew(): Promise<{ ok: boolean }> {
  return daemonCliPost(primaryScope(), 'sew/server/rollback', {})
}

export async function disableSew(): Promise<{ ok: boolean }> {
  return daemonCliPost(primaryScope(), 'sew/server/disable', {})
}

export async function uninstallSew(dropGrants: boolean): Promise<{ ok: boolean }> {
  return daemonCliPost(primaryScope(), 'sew/server/uninstall', dropGrants ? { dropGrants: true } : {})
}

export async function purgeSew(opts: {
  confirmHostname: string
  keepBackups: boolean
  dropGrants: boolean
  confirmToken: string
}): Promise<{ ok: boolean; warning?: string }> {
  const body: Record<string, unknown> = {
    purge: true,
    confirmHostname: opts.confirmHostname,
    confirmToken: opts.confirmToken,
  }
  if (opts.keepBackups) body.keepBackups = true
  if (opts.dropGrants) body.dropGrants = true
  return daemonCliPost(primaryScope(), 'sew/server/uninstall', body)
}

/** The recovery key. The caller holds it in component state only. */
export async function revealRecoveryKey(confirmToken: string): Promise<{ recoveryKey: string; warning: string }> {
  return daemonCliPost(primaryScope(), 'sew/server/recovery-key', { confirmToken })
}

export async function runSewBackup(): Promise<{ ok: boolean; jobId: string; backup?: unknown }> {
  return daemonCliPost(primaryScope(), 'sew/server/backup', {})
}

export async function fetchSewJob(id: string): Promise<SewJob> {
  return daemonCliGet<SewJob>(primaryScope(), 'sew/job', { id })
}

export async function fetchSewBackups(): Promise<SewBackup[]> {
  const r = await daemonCliGet<{ backups?: SewBackup[] }>(primaryScope(), 'sew/backups')
  return Array.isArray(r?.backups) ? r.backups : []
}

export type OffboxKind = 'none' | 'dir'
/** The ship also takes `box-backup` (0.46.9, the owner only). */
export type ShipKind = OffboxKind | 'box-backup'

function offboxBody(kind: ShipKind, path: string): Record<string, unknown> {
  return kind === 'dir' ? { kind, path: path.trim() } : { kind }
}

export async function setSewShip(kind: ShipKind, path = ''): Promise<{ ok: boolean }> {
  return daemonCliPost(primaryScope(), 'sew/server/ship', offboxBody(kind, path))
}

export async function setSewMirror(kind: OffboxKind, path = ''): Promise<{ ok: boolean; restarted?: boolean }> {
  return daemonCliPost(primaryScope(), 'sew/server/mirror', offboxBody(kind, path))
}

export async function setSewPublicUrl(publicUrl: string): Promise<{ ok: boolean }> {
  return daemonCliPost(primaryScope(), 'sew/server/config', { publicUrl: publicUrl.trim() })
}

/** `''` clears it. */
export async function setSewAppUrl(url: string): Promise<{ ok: boolean; appUrl: string | null }> {
  return daemonCliPost(primaryScope(), 'sew/server/app-url', { url: url.trim() })
}

/** A single-use 60 s code. The caller opens it at once and drops it. */
export async function mintOwnerHandoff(confirmToken: string): Promise<{ code: string; expiresInMs: number }> {
  return daemonCliPost(primaryScope(), 'sew/owner-handoff', { confirmToken })
}

/** `<appUrl>/k2/owner-handoff#code=…`: the code rides in the fragment, so
 *  it never reaches a server log or a Referer. */
export function ownerHandoffUrl(appUrl: string, code: string): string {
  return `${appUrl.replace(/\/+$/, '')}/k2/owner-handoff#code=${encodeURIComponent(code)}`
}

export async function fetchSewAppSpaces(): Promise<SewAppSpace[]> {
  const r = await daemonCliGet<{ spaces?: SewAppSpace[] }>(primaryScope(), 'sew/app-spaces')
  return Array.isArray(r?.spaces) ? r.spaces : []
}

export async function bindSewAppSpace(app: string, spaceId: string): Promise<{ ok: boolean }> {
  return daemonCliPost(primaryScope(), 'sew/app-spaces/bind', { app: app.trim(), spaceId: spaceId.trim() })
}

/**
 * S32 Q3 (Rosson 2026-10-27): unbinding is a choice. By default everyone
 * stays (reversible); `removeAppMembers` removes only the members K2 added.
 */
export async function unbindSewAppSpace(app: string, spaceId: string, removeAppMembers = false): Promise<{ ok: boolean }> {
  return daemonCliPost(
    primaryScope(),
    'sew/app-spaces/unbind',
    removeAppMembers ? { app, spaceId, removeAppMembers: true } : { app, spaceId },
  )
}

// ── recording in Apps (Sew 0.9.0, K2 0.46.11) ─────────────────────────────
//
// Display only: the daemon owns every rule (who may widen, the lock, the
// org's keep days) and answers a widening with a confirm request.

export interface SewRecordingFlags {
  appRecording: boolean
  voiceRecordCap: boolean
  recordingRequired: boolean
  mgmtRecordingReads: boolean
}

export interface SewRecordingPolicy {
  key: string
  /** What K2 was asked for (null: never set here). */
  want: unknown
  /** What Sew last showed. */
  seen: unknown
  setBy: string | null
  delivered: boolean
}

export interface SewRecordingLock {
  spaceId: string
  lockedBy: string
  lockedAt: number
}

export interface SewRecordingPolicies {
  flags: SewRecordingFlags
  policies: SewRecordingPolicy[]
  locks: SewRecordingLock[]
  k5SpaceId: string | null
}

export interface AppRecordingSpace {
  spaceId: string
  governance: string
  /** null = the App's rule. */
  startLevel: string | null
  required: boolean | null
  locked: boolean
}

export interface AppRecordingView {
  app: { serviceId: string; name: string }
  homeClient: boolean
  startLevel: string
  required: boolean
  serverRead: boolean
  /** null = the org's `recording.audio_max_days`. */
  audioMaxDays: number | null
  delivered: boolean
  orgAudioMaxDays: number | null
  spaces: AppRecordingSpace[]
}

const rec = (v: unknown): Record<string, unknown> => (v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : {})
const num = (v: unknown): number | null => (typeof v === 'number' && Number.isFinite(v) ? v : null)
const text = (v: unknown): string | null => (typeof v === 'string' && v !== '' ? v : null)

export function recordingFlagsOf(v: unknown): SewRecordingFlags {
  const f = rec(v)
  return {
    appRecording: f.appRecording === true,
    voiceRecordCap: f.voiceRecordCap === true,
    recordingRequired: f.recordingRequired === true,
    mgmtRecordingReads: f.mgmtRecordingReads === true,
  }
}

/** `GET /cli/sew/recording/policies`. */
export async function fetchSewRecordingPolicies(): Promise<SewRecordingPolicies> {
  const b = rec(await daemonCliGet<unknown>(primaryScope(), 'sew/recording/policies'))
  const policies = (Array.isArray(b.policies) ? b.policies : []).map((p) => {
    const r = rec(p)
    return { key: String(r.key ?? ''), want: r.want ?? null, seen: r.seen ?? null, setBy: text(r.setBy), delivered: r.delivered === true }
  })
  const locks = (Array.isArray(b.locks) ? b.locks : []).map((l) => {
    const r = rec(l)
    return { spaceId: String(r.spaceId ?? ''), lockedBy: String(r.lockedBy ?? ''), lockedAt: num(r.lockedAt) ?? 0 }
  })
  return { flags: recordingFlagsOf(b.flags), policies, locks, k5SpaceId: text(b.k5SpaceId) }
}

/** The Apps with Sew settings rows here (`GET /cli/sew/home`), the home client too. */
export async function fetchSewHomeAppIds(): Promise<string[]> {
  const b = rec(await daemonCliGet<unknown>(primaryScope(), 'sew/home'))
  const ids = (Array.isArray(b.apps) ? b.apps : []).map((a) => text(rec(a).app)).filter((x): x is string => x !== null)
  const home = text(b.homeClient)
  return home && !ids.includes(home) ? [...ids, home] : ids
}

/** `GET /cli/app/recording?app=`. */
export async function fetchAppRecording(app: string): Promise<AppRecordingView> {
  const b = rec(await daemonCliGet<unknown>(primaryScope(), 'app/recording', { app }))
  const a = rec(b.app)
  const r = rec(b.recording)
  const spaces = (Array.isArray(b.spaces) ? b.spaces : []).map((s) => {
    const x = rec(s)
    return {
      spaceId: String(x.spaceId ?? ''),
      governance: text(x.governance) ?? 'sew',
      startLevel: text(x.startLevel),
      required: typeof x.required === 'boolean' ? x.required : null,
      locked: x.locked === true,
    }
  })
  return {
    app: { serviceId: text(a.serviceId) ?? app, name: text(a.name) ?? app },
    homeClient: b.homeClient === true,
    startLevel: text(r.start_level) ?? 'none',
    required: r.required === true,
    serverRead: r.server_read === true,
    audioMaxDays: num(r.audio_max_days),
    delivered: b.delivered !== false,
    orgAudioMaxDays: num(b.orgAudioMaxDays),
    spaces,
  }
}

export interface AppRecordingChange {
  startLevel?: 'none' | 'admin' | 'moderator' | 'member'
  /** null = back to the org's keep days. */
  audioMaxDays?: number | null
  required?: boolean
  serverRead?: boolean
}

/** `POST /cli/app/recording/set`; a widening answers `vault_confirm_required`
 *  with the confirm `action` and the App's `serviceId`. */
export async function setAppRecording(app: string, change: AppRecordingChange, confirmToken?: string): Promise<unknown> {
  return daemonCliPost(primaryScope(), 'app/recording/set', { app, ...change, ...(confirmToken ? { confirmToken } : {}) })
}

/** `POST /cli/sew/recording/policy {changes, confirmToken?}`: the server's
 *  recording policies (an Owner or Admin in person; `recording.required`
 *  the owner). Without a confirm the daemon answers `owner_confirm_required`. */
export async function setSewRecordingPolicy(changes: Record<string, unknown>, confirmToken?: string): Promise<unknown> {
  return daemonCliPost(primaryScope(), 'sew/recording/policy', { changes, ...(confirmToken ? { confirmToken } : {}) })
}

/** `POST /cli/sew/app-spaces/recording-lock` (the owner, in person). */
export async function setSpaceRecordingLock(spaceId: string, lock: boolean, confirmToken?: string): Promise<unknown> {
  return daemonCliPost(primaryScope(), 'sew/app-spaces/recording-lock', { spaceId, lock, ...(confirmToken ? { confirmToken } : {}) })
}

// ── person confirm (vault) ────────────────────────────────────────────────

/** The person-only steps and their vault confirm actions. The recording
 *  ones are bound to the App's service id (`app-recording*`) or the Space
 *  (`recording-lock`). */
export type SewConfirmAction =
  | 'sew-recovery-key'
  | 'sew-purge'
  | 'chat-owner-handoff'
  | 'app-recording'
  | 'app-recording-owner'
  | 'recording-lock'
  | 'recording-policy'

/** `secretId` per action: the install id (or `none`) for the S29 steps;
 *  none for the owner handoff (the daemon takes it unbound). */
export function confirmSecretId(action: SewConfirmAction, installId: string | null | undefined): string | undefined {
  return action === 'chat-owner-handoff' ? undefined : installId || 'none'
}

export async function startSewConfirm(
  action: SewConfirmAction,
  secretId: string | undefined,
): Promise<{ methods: string[]; who: ConfirmIdentity }> {
  const r = await daemonCliPost<{ methods?: string[] }>(primaryScope(), 'vault/confirm/start', {
    action,
    ...(secretId !== undefined ? { secretId } : {}),
  })
  return { methods: Array.isArray(r?.methods) ? r.methods : [], who: readConfirmIdentity(r) }
}

export async function finishSewConfirm(
  action: SewConfirmAction,
  secretId: string | undefined,
  method: string,
  label: string,
  password?: string,
): Promise<{ token: string }> {
  return daemonCliPost(primaryScope(), 'vault/confirm/finish', {
    action,
    ...(secretId !== undefined ? { secretId } : {}),
    method,
    label: label.slice(0, 64),
    ...(password !== undefined ? { password } : {}),
  })
}

// ── chat grants (User Access / Admin Access) ──────────────────────────────

export interface ChatGrant {
  id: string
  subjectKind: string
  subjectId: string
  scope: string | null
  enabled: boolean
  hookState?: 'synced' | 'pending' | 'blocked' | 'held' | string
  hookError?: string
}

export function chatGrantsOf(raw: unknown): ChatGrant[] {
  const list = raw && typeof raw === 'object' && Array.isArray((raw as { grants?: unknown }).grants)
    ? ((raw as { grants: unknown[] }).grants)
    : []
  return list.flatMap((row) => {
    const r = row && typeof row === 'object' ? (row as Record<string, unknown>) : {}
    if (r.kind !== 'chat' || typeof r.id !== 'string' || typeof r.subjectKind !== 'string' || typeof r.subjectId !== 'string') {
      return []
    }
    return [{
      id: r.id,
      subjectKind: r.subjectKind,
      subjectId: r.subjectId,
      scope: typeof r.scope === 'string' ? r.scope : null,
      enabled: r.enabled !== false,
      ...(typeof r.hookState === 'string' ? { hookState: r.hookState } : {}),
      ...(typeof r.hookError === 'string' ? { hookError: r.hookError } : {}),
    }]
  })
}

export async function fetchChatGrants(): Promise<ChatGrant[]> {
  return chatGrantsOf(await daemonCliGet<unknown>(primaryScope(), 'skin/grants'))
}

export async function grantPersonSew(principalId: string, scope: 'member' | 'guest'): Promise<unknown> {
  return daemonCliPost(primaryScope(), 'skin/grants', { subjectKind: 'principal', subjectId: principalId, kind: 'chat', scope })
}

export async function updateSewGrantScope(id: string, scope: 'member' | 'guest'): Promise<unknown> {
  return daemonCliPost(primaryScope(), 'skin/grants/update', { id, scope })
}

export async function revokeSewGrant(id: string): Promise<{ success?: boolean; cascaded?: unknown[] }> {
  return daemonCliPost(primaryScope(), 'skin/grants/delete', { id })
}

/** The blocked outbox row for a subject (`k2:principal:<id>`), for Retry. */
export async function blockedHookRow(subjectKey: string): Promise<string | null> {
  const r = await daemonCliGet<{ blocked?: { id?: string; subjectKey?: string }[] }>(primaryScope(), 'skin/grant-hooks')
  const row = Array.isArray(r?.blocked) ? r.blocked.find((b) => b.subjectKey === subjectKey) : undefined
  return row?.id ?? null
}

export async function retryHookRow(id: string): Promise<unknown> {
  return daemonCliPost(primaryScope(), 'skin/grant-hooks/retry', { id })
}

// ── sample data (unsupported daemon) ──────────────────────────────────────

export const SAMPLE_STATUS: SewStatus = {
  supported: true,
  released: true,
  state: 'degraded',
  consistent: true,
  pinnedVersion: '0.3.0',
  installedVersion: '0.3.0',
  upgradeAvailable: false,
  installId: 'ins_01SAMPLE0000000000000000',
  identityProvider: 'k2',
  units: {
    sewDb: { active: 'active', nRestarts: 0, memoryCurrent: 210 * 2 ** 20, memoryMax: 2 * 2 ** 30 },
    sew: { active: 'active', nRestarts: 0, memoryCurrent: 140 * 2 ** 20, memoryMax: 1 * 2 ** 30 },
  },
  alarms: ['k2_backup_offbox_none', 'backup_stale'],
  warnings: [
    {
      code: 'no_offbox_key_copy',
      alarm: 'keys_mirror_stale',
      message: "no off-box key copy: backups can't be read after a disk loss without one",
    },
  ],
  recoveryKeySaved: true,
  recoveryKeyInVault: true,
  recoveryKeyVaultName: 'sew-recovery-key',
  masterKeyAgeDays: 12,
  backup: {
    ship: { kind: 'none', path: null },
    lastOkAt: '2026-10-18T03:00:00Z',
    lastId: '20261018T030000Z',
    lastKind: 'scheduled',
    alarms: ['backup_stale'],
  },
  drill: { lastOkAt: '2026-10-15T04:00:00Z', lastOk: true },
  mirror: { kind: 'none', path: null },
  appUrl: 'https://hearth.example.k2.dev',
  publicUrl: 'https://hearth.example.k2.dev',
  owner: 'set',
  target: { enabled: true },
  purgeConfirmHostname: 'hearth.example.k2.dev',
  you: NO_YOU,
}

export const SAMPLE_PREFLIGHT: SewPreflightCheck[] = [
  { id: 'os', status: 'pass', detail: 'Ubuntu 24.04', hint: '' },
  { id: 'arch', status: 'pass', detail: 'x86_64', hint: '' },
  { id: 'postgres', status: 'pass', detail: 'PostgreSQL 16', hint: '' },
  { id: 'ram', status: 'warn', detail: '3.8 GiB', hint: '4 GiB or more is better' },
]

export const SAMPLE_BACKUPS: SewBackup[] = [
  { id: '20261018T030000Z', kind: 'scheduled', created_at: '2026-10-18T03:00:00Z', bytes: 48 * 2 ** 20 },
  { id: '20261017T030000Z', kind: 'scheduled', created_at: '2026-10-17T03:00:00Z', bytes: 47 * 2 ** 20 },
]

export const SAMPLE_APP_SPACES: SewAppSpace[] = [
  { app: 'hearth', spaceId: 'spc_01SAMPLE', boundAt: null, members: { added: 12, held: 0, pending: 1 } },
]
