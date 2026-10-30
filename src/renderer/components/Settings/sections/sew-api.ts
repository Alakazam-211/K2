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

/** Every CLI hint names the noun through this (one-line rename). */
export const SEW_CLI = 'k2 hostchat'

/** The daemon reports `supported: false` (macOS, Windows): the page shows
 *  this banner over sample data and never mutates. */
export const SEW_MAC_BANNER = 'CHAT HOSTING (SEW) ONLY RUNS ON LINUX DEPLOYMENTS. THIS PAGE IS JUST HERE FOR EXAMPLE PURPOSES.'

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

export interface SewShip {
  kind: string
  path?: string | null
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
  recoveryKeySaved?: boolean
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

/** What a person reads for a failed Sew call. */
export function sewErrorMessage(e: unknown): string {
  const info = sewErrorInfo(e)
  if (info.code === 'owner_only' || (info.status === 403 && !info.code)) {
    return 'Only the box owner or an admin can manage Chat Hosting.'
  }
  if (info.code === 'lifecycle_in_progress') return 'Another Sew action is running. Wait for it to finish.'
  return info.message
}

/** A route the daemon doesn't have yet (older daemon / S30–S31 not built). */
export function isNotBuilt(e: unknown): boolean {
  const { status, code } = sewErrorInfo(e)
  return status === 404 || status === 501 || code === 'not_built'
}

// ── tables ────────────────────────────────────────────────────────────────

/** The enable machine's steps, in `supervisor::enable_steps` order. */
export const SEW_ENABLE_STEPS: { id: string; label: string }[] = [
  { id: 'master-key', label: 'Create master key' },
  { id: 'stage', label: 'Download Sew (pinned release)' },
  { id: 'install-bin', label: 'Install binary' },
  { id: 'install', label: 'Install Sew' },
  { id: 'enable-units', label: 'Enable services' },
  { id: 'db', label: 'Start database' },
  { id: 'migrate', label: 'Prepare database' },
  { id: 'start', label: 'Start Sew' },
  { id: 'token', label: 'Connect to Sew' },
  { id: 'health', label: 'Health check' },
  { id: 'link', label: 'Link to K2' },
]

export const SEW_PREFLIGHT_LABELS: Record<string, string> = {
  os: 'Operating system',
  arch: 'CPU architecture',
  systemd: 'systemd',
  postgres: 'PostgreSQL',
  helper: 'K2 root helper',
  pin: 'Pinned release',
  source: 'Release source',
  ram: 'Memory',
  disk: 'Disk space',
  port: 'Sew port',
  binary: 'Sew binary',
  existing_data: 'Existing Sew data',
  daemon_port: 'K2 daemon port',
  vault: 'K2 vault',
  voice_ports: 'Voice ports',
}

/** Checks the inputs can fix: enable stays possible while they fail. */
export const SEW_FIXABLE_CHECKS = ['source', 'existing_data']

/** "12.3 of 24.7 MB" / "from K2's cache" beside the download step. */
export function downloadNote(d: SewDownload | null | undefined): string | null {
  if (!d) return null
  if (d.source === 'cache') return `${d.label ?? 'Sew'} from K2's download cache`
  if (d.source === 'tarball') return `${d.label ?? 'Sew'} from the file you named`
  if (d.source !== 'download' || typeof d.bytes !== 'number') return null
  const mb = (n: number): string => (n / 1e6).toFixed(1)
  return typeof d.total === 'number' && d.total > 0
    ? `${d.label ?? 'Sew'}: ${mb(d.bytes)} of ${mb(d.total)} MB`
    : `${d.label ?? 'Sew'}: ${mb(d.bytes)} MB`
}

export type SewAlarmAction = 'backup' | 'offbox' | 'recovery-key' | 'public-url'

export interface SewAlarmInfo {
  severity: 'error' | 'warn'
  title: string
  fix: string
  action?: SewAlarmAction
}

const cli = (verb: string): string => `${SEW_CLI} ${verb}`

/** Every alarm Sew and K2 report (S29 §8.3 table, `supervisor::k2_alarms`,
 *  `voice::status`, `sew_wire::BACKUP_ALARMS`). Unknown alarms render
 *  verbatim via {@link alarmInfo}. */
export const SEW_ALARMS: Record<string, SewAlarmInfo> = {
  db_down: { severity: 'error', title: "Sew's database is down", fix: 'Sew cannot reach its Postgres. Check sew-db.service.' },
  jobs_dead: { severity: 'warn', title: 'Background jobs failed', fix: `Some Sew jobs gave up. Run ${cli('doctor')}.` },
  keys_provider_down: { severity: 'error', title: 'Sew cannot read its master key', fix: `Sew cannot decrypt anything. Run ${cli('doctor')}.` },
  keys_current_month_missing: { severity: 'error', title: "This month's key is missing", fix: `New messages cannot be stored. Run ${cli('doctor')}.` },
  keys_mirror_stale: { severity: 'warn', title: 'Keys are not copied off this machine', fix: 'Set a keyring mirror on another disk.', action: 'offbox' },
  keys_shred_pending: { severity: 'warn', title: 'Key deletion is pending', fix: 'Sew will finish it. Nothing to do unless this stays.' },
  keys_resurrected: { severity: 'error', title: 'A deleted key came back', fix: `A restored keyring holds a shredded key. Run ${cli('doctor')}.` },
  keys_rewrap_stalled: { severity: 'warn', title: 'Key rotation stalled', fix: `Run ${cli('rotate-master')} again, then ${cli('doctor')}.` },
  search_lagging: { severity: 'warn', title: 'Search is behind', fix: 'New messages may not show in search yet.' },
  backup_stale: { severity: 'warn', title: 'No backup in 36 hours', fix: 'Back up now and check the backup timer.', action: 'backup' },
  backup_failed: { severity: 'error', title: 'The last backup failed', fix: 'Back up now to see the error.', action: 'backup' },
  drill_failed: { severity: 'error', title: 'The last restore drill failed', fix: `A backup could not be restored. Run ${cli('doctor')}.` },
  drill_stale: { severity: 'warn', title: 'No restore drill in 8 days', fix: 'Check sew-drill.timer.' },
  k2_mgmt_unreachable: { severity: 'error', title: 'K2 cannot reach Sew', fix: `The services run but K2 cannot talk to them. Run ${cli('doctor')}.` },
  k2_recovery_key_not_saved: { severity: 'warn', title: 'Recovery key not saved', fix: 'Save it off this machine. It is the only way back if this box dies.', action: 'recovery-key' },
  k2_master_key_due: { severity: 'warn', title: 'Master key is due for rotation', fix: `Run ${cli('rotate-master')}.` },
  k2_vault_key_mismatch: { severity: 'error', title: "K2's key does not match Sew's", fix: `Do not change anything. Run ${cli('doctor')}.` },
  k2_helper_pin_mismatch: { severity: 'warn', title: 'Sew helper is out of date', fix: 'Update K2 on this box, then upgrade Sew.' },
  k2_jwks_url_drift: { severity: 'error', title: 'Sew cannot verify K2 logins', fix: `K2's port changed. Run ${cli('doctor')} to fix it.` },
  k2_owner_blocked: { severity: 'warn', title: 'Owner setup failed', fix: 'Retry owner setup from User Access sync.' },
  k2_backup_offbox_none: { severity: 'warn', title: 'Backups stay on this machine', fix: 'Set a backup destination on another disk.', action: 'offbox' },
  k2_backup_not_shipped: { severity: 'warn', title: 'The last backup was not copied off', fix: 'Check the backup destination.', action: 'offbox' },
  k2_upgrade_rollback_failed: { severity: 'error', title: 'Upgrade failed and could not roll back', fix: 'A person must act on the box. See /usr/local/lib/sew/upgrade-state.json.' },
  k2_turn_cert_missing: { severity: 'error', title: 'Voice has no TURN certificate', fix: `Run ${cli('media renew')}.` },
  k2_turn_cert_expired: { severity: 'error', title: "Voice's TURN certificate expired", fix: `Run ${cli('media renew')}.` },
  k2_turn_cert_expiring: { severity: 'warn', title: "Voice's TURN certificate expires soon", fix: `K2 renews it on its own; run ${cli('media renew')} to do it now.` },
  k2_turn_tls_mismatch: { severity: 'error', title: 'Voice serves the wrong certificate', fix: `Run ${cli('media renew')}, then ${cli('doctor')}.` },
  k2_voice_probe_failed: { severity: 'warn', title: 'A call test from outside failed', fix: `Check the box firewall for the voice ports, then ${cli('media probe')}.` },
  voice_engine_down: { severity: 'error', title: 'The voice engine is down', fix: `Run ${cli('media status')}.` },
  voice_not_installed: { severity: 'warn', title: 'Voice is not installed', fix: `Run ${cli('media install')}.` },
}

/** Info for any alarm string; unknown ones render verbatim. */
export function alarmInfo(alarm: string): SewAlarmInfo {
  return SEW_ALARMS[alarm] ?? { severity: 'warn', title: alarm, fix: `Run ${cli('status')} for details.` }
}

/** Status chip label + color token per state. */
export function sewStateChip(state: string): { label: string; color: string } {
  switch (state) {
    case 'not-installed':
      return { label: 'Not installed', color: 'var(--color-text-muted)' }
    case 'installing':
      return { label: 'Installing…', color: 'var(--color-status-working)' }
    case 'running':
      return { label: 'Running', color: 'var(--color-status-ok)' }
    case 'degraded':
      return { label: 'Degraded', color: 'var(--color-status-warn)' }
    case 'stopped':
      return { label: 'Stopped', color: 'var(--color-status-error-soft)' }
    case 'error':
      return { label: 'Error', color: 'var(--color-status-error-soft)' }
    case 'disabled':
      return { label: 'Disabled', color: 'var(--color-text-muted)' }
    case 'upgrading':
      return { label: 'Upgrading…', color: 'var(--color-status-working)' }
    case 'removed':
      return { label: 'Removed (data kept)', color: 'var(--color-text-muted)' }
    default:
      return { label: state, color: 'var(--color-text-muted)' }
  }
}

/** States the enable wizard shows in, and its button label. */
export function enableLabel(state: string): string | null {
  switch (state) {
    case 'not-installed':
      return 'Enable Sew'
    case 'error':
      return 'Resume'
    case 'disabled':
    case 'stopped':
    case 'removed':
      return 'Re-enable'
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

function offboxBody(kind: OffboxKind, path: string): Record<string, unknown> {
  return kind === 'dir' ? { kind, path: path.trim() } : { kind }
}

export async function setSewShip(kind: OffboxKind, path = ''): Promise<{ ok: boolean }> {
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

// ── person confirm (vault) ────────────────────────────────────────────────

/** The person-only steps and their vault confirm actions. */
export type SewConfirmAction = 'sew-recovery-key' | 'sew-purge' | 'chat-owner-handoff'

/** `secretId` per action: the install id (or `none`) for the S29 steps;
 *  none for the owner handoff (the daemon takes it unbound). */
export function confirmSecretId(action: SewConfirmAction, installId: string | null | undefined): string | undefined {
  return action === 'chat-owner-handoff' ? undefined : installId || 'none'
}

export async function startSewConfirm(action: SewConfirmAction, secretId: string | undefined): Promise<{ methods: string[] }> {
  const r = await daemonCliPost<{ methods?: string[] }>(primaryScope(), 'vault/confirm/start', {
    action,
    ...(secretId !== undefined ? { secretId } : {}),
  })
  return { methods: Array.isArray(r?.methods) ? r.methods : [] }
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
  alarms: ['k2_recovery_key_not_saved', 'k2_backup_offbox_none', 'backup_stale'],
  warnings: [
    {
      code: 'no_offbox_key_copy',
      alarm: 'keys_mirror_stale',
      message: "no off-box key copy: backups can't be read after a disk loss without one",
    },
  ],
  recoveryKeySaved: false,
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
