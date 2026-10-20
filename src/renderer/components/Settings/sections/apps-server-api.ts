// Settings → Apps → This server (0.46.3, "Apps on for real"). Typed bindings
// for the App box routes plus the plain-words helpers the panel uses. Thin
// client: the daemon owns the Apps switch, the box doctor and what prepare
// and engine install do; this file reads that truth and sends the owner's
// gestures to the window's server (`primaryScope()`), local or remote over
// K2 Connect (the routes take an Owner login).
//
//   GET  /cli/app/box                      the doctor (`k2 app doctor`)
//   POST /cli/app/box/prepare {dryRun}     what prepare would change
//   POST /cli/app/box/prepare {installPodman?}  the owner's apply
//   POST /cli/app/engine/install {}        the guarded App engine

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { daemonErrorOf } from '@/lib/daemon-error'
import { settingsUpdate } from '@/lib/daemon-settings'
import { primaryScope } from '@/kessel/server-scope'

export type CheckStatus = 'ok' | 'warn' | 'fail' | 'unknown'

export interface BoxCheck {
  id: string
  status: CheckStatus
  detail: string
  fix: string | null
}

export interface BoxDoctor {
  ready: boolean
  /** Apps (beta) on (the daemon's view, env included). */
  apps: boolean
  /** The unwalled process engine on (owner opt-in, dev machines). */
  processEngine: boolean
  linux: boolean
  helperProtocol: number
  appsMinProtocol: number
  runtime: string | null
  checks: BoxCheck[]
  /** 0.46.3: the automatic box prep (`appsAutoPrepare`); null on older daemons. */
  autoPrepare: AutoPrepareStatus | null
}

export interface AutoPrepareStatus {
  enabled: boolean
  /** `ready` | `prepared` | `skipped` | `failed`; null before the first run. */
  state: string | null
  /** Why it skipped or failed, or what it changed. */
  detail: string | null
}

export interface PodmanBundle {
  version: string
  source: string
  url: string
  sha256: string
  size: number | null
  writes: string[]
}

export interface PodmanPlan {
  /** keep | distro | offer (needs installPodman) | install */
  action: string
  have: string | null
  os: string | null
  note: string | null
  bundle: PodmanBundle | null
}

export interface PrepareDryRun {
  steps: string[]
  podman: PodmanPlan
  box: BoxDoctor | null
}

function isObj(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

function str(v: unknown): string | null {
  return typeof v === 'string' && v ? v : null
}

const STATUSES: ReadonlySet<string> = new Set(['ok', 'warn', 'fail', 'unknown'])

export function parseDoctor(raw: unknown): BoxDoctor {
  const v = isObj(raw) ? raw : {}
  const checks = Array.isArray(v.checks)
    ? v.checks.filter(isObj).map((c) => ({
        id: typeof c.id === 'string' ? c.id : '?',
        status: (typeof c.status === 'string' && STATUSES.has(c.status) ? c.status : 'unknown') as CheckStatus,
        detail: typeof c.detail === 'string' ? c.detail : '',
        fix: str(c.fix),
      }))
    : []
  return {
    ready: v.ready === true,
    // 0.46.2 and older answered `preview`.
    apps: v.apps === true || v.preview === true,
    processEngine: v.processEngine === true,
    linux: v.linux === true || checks.some((c) => c.id === 'platform' && c.status === 'ok'),
    helperProtocol: typeof v.helperProtocol === 'number' ? v.helperProtocol : 0,
    appsMinProtocol: typeof v.appsMinProtocol === 'number' ? v.appsMinProtocol : 0,
    runtime: str(v.runtime),
    checks,
    autoPrepare: parseAutoPrepare(v.autoPrepare),
  }
}

export function parseAutoPrepare(raw: unknown): AutoPrepareStatus | null {
  if (!isObj(raw)) return null
  const o = isObj(raw.outcome) ? raw.outcome : null
  const changed = o && Array.isArray(o.changed) ? o.changed.filter((c): c is string => typeof c === 'string') : []
  return {
    enabled: raw.enabled === true,
    state: o ? str(o.state) : null,
    detail: o ? (str(o.why) ?? str(o.error) ?? (changed.length ? changed.join('; ') : null)) : null,
  }
}

/** The owner's "Prepare this box automatically" (`appsAutoPrepare`). */
export async function setAutoPrepare(on: boolean): Promise<void> {
  await settingsUpdate({ appsAutoPrepare: on })
}

export function parseDryRun(raw: unknown): PrepareDryRun {
  const v = isObj(raw) ? raw : {}
  const p = isObj(v.podman) ? v.podman : {}
  const b = isObj(p.bundle) ? p.bundle : null
  return {
    steps: Array.isArray(v.steps) ? v.steps.filter((s): s is string => typeof s === 'string') : [],
    podman: {
      action: typeof p.action === 'string' ? p.action : 'distro',
      have: str(p.have),
      os: str(p.os),
      note: str(p.note),
      bundle: b
        ? {
            version: typeof b.version === 'string' ? b.version : '?',
            source: typeof b.source === 'string' ? b.source : '',
            url: typeof b.url === 'string' ? b.url : '',
            sha256: typeof b.sha256 === 'string' ? b.sha256 : '',
            size: typeof b.size === 'number' ? b.size : null,
            writes: Array.isArray(b.writes) ? b.writes.filter((w): w is string => typeof w === 'string') : [],
          }
        : null,
    },
    box: isObj(v.box) ? parseDoctor(v.box) : null,
  }
}

/** Apply needs `installPodman`: the box's Podman is too old and K2's
 *  pinned bundle is offered (the owner sees it in the dry run first). */
export function needsPodmanInstall(d: Pick<PrepareDryRun, 'podman'>): boolean {
  return d.podman.action === 'offer' || d.podman.action === 'install'
}

/** Every change the apply would make, one line each, in order. */
export function changeLines(d: PrepareDryRun): string[] {
  const out: string[] = []
  if (needsPodmanInstall(d) && d.podman.bundle) {
    const b = d.podman.bundle
    out.push(
      `Install K2's Podman ${b.version} for the k2app user only (the box has ${d.podman.have ?? 'none'}; the distro's Podman is left as it is)`,
    )
    for (const w of b.writes) out.push(`  writes ${w}`)
  }
  for (const s of d.steps) out.push(s)
  return out
}

/** The doctor row names, in plain words. Unknown ids show as they are. */
const CHECK_LABELS: Record<string, string> = {
  platform: 'Platform',
  processEngine: 'Unwalled process engine',
  helper: 'Root helper',
  helperStatus: 'Root helper status',
  kernel: 'Kernel',
  cgroupV2: 'cgroup v2',
  systemd: 'systemd',
  podman: 'Podman',
  pasta: 'pasta (networking)',
  ociRuntime: 'OCI runtime',
  conmon: 'conmon',
  boxPrep: 'Box prepared',
  appUser: 'k2app user',
  subid: 'Sub-UIDs',
  linger: 'Linger',
  delegation: 'cgroup delegation',
  reservedPorts: 'Reserved ports',
  nft: 'Firewall table',
  processUnit: 'Process unit',
  mac: 'MAC (AppArmor/SELinux)',
  engine: 'App engine',
  ptraceScope: 'ptrace scope',
  usernsRestricted: 'User namespaces',
  ownerConfirm: 'Owner confirm',
}

export function checkLabel(id: string): string {
  return CHECK_LABELS[id] ?? id
}

/** The App engine still to install (or update): its row isn't ok. */
export function engineNeeded(d: BoxDoctor): boolean {
  const row = d.checks.find((c) => c.id === 'engine')
  return d.linux && (!row || row.status !== 'ok')
}

/** Prepare still has work: a row whose fix is box prep isn't ok. */
export function prepareNeeded(d: BoxDoctor): boolean {
  return d.linux && d.checks.some((c) => c.status !== 'ok' && (c.fix ?? '').startsWith('k2 app box prepare'))
}

/** The window's server answered `apps_disabled` (Apps (beta) are off). */
export function isAppsOff(err: unknown): boolean {
  return daemonErrorOf(err)?.code === 'apps_disabled'
}

export async function fetchAppBox(): Promise<BoxDoctor> {
  return parseDoctor(await daemonCliGet<unknown>(primaryScope(), 'app/box'))
}

export async function prepareDryRun(): Promise<PrepareDryRun> {
  return parseDryRun(await daemonCliPost<unknown>(primaryScope(), 'app/box/prepare', { dryRun: true }))
}

export async function prepareApply(installPodman: boolean): Promise<{ box: BoxDoctor | null }> {
  const body = installPodman ? { installPodman: true } : {}
  const v = await daemonCliPost<unknown>(primaryScope(), 'app/box/prepare', body)
  return { box: isObj(v) && isObj(v.box) ? parseDoctor(v.box) : null }
}

export async function installEngine(): Promise<{ version: string | null }> {
  const v = await daemonCliPost<unknown>(primaryScope(), 'app/engine/install', {})
  const e = isObj(v) && isObj(v.engine) ? v.engine : {}
  return { version: str(e.version) }
}
