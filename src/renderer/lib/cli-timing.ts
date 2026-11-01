// Per-request timing for `/cli/*` calls (research-remote-thread-load-v1,
// "to confirm before building"): how long each request waited for one of
// its server's slots (`queuedMs`) and how long it then spent on the network
// (`networkMs`, the fetch and its body, retries and a session revive
// included). Kept in a small ring in memory, never sent anywhere.
//
// Quiet by default. To see it on a machine:
//
//   k2CliTiming.log(true)    // a console.debug line per request (remembered)
//   k2CliTiming.summary()    // per route: count, queued and network p50/p95/max
//   k2CliTiming.table()      // the last requests as a console table
//   k2CliTiming.clear()
//
// `k2CliTiming` is on `window` in every build (it holds only route names,
// host keys and numbers: no tokens, no query strings, no bodies).

/** Which queue a request took: local (uncapped), a remote server's normal
 *  slots, or its Thread lane (`daemon-cli.ts`). */
export type CliLane = 'local' | 'normal' | 'priority'

export interface CliTiming {
  /** Wall clock (ms) when the call started. */
  at: number
  /** The server's host key (`local`, `<sub>.k2.dev`, `host:port`). */
  host: string
  /** The part after `/cli/`, e.g. `thread`. */
  route: string
  method: string
  lane: CliLane
  /** Waiting for a slot. */
  queuedMs: number
  /** From the slot to the body read (retries and a revive included). */
  networkMs: number
  totalMs: number
  /** HTTP status, or null when the call threw before an answer. */
  status: number | null
}

export interface CliTimingRouteSummary {
  route: string
  count: number
  queuedP50: number
  queuedP95: number
  queuedMax: number
  networkP50: number
  networkP95: number
  networkMax: number
}

const MAX_KEPT = 300
const LOG_FLAG_KEY = 'k2.debug.cliTiming'

const ring: CliTiming[] = []
const lastActivity = new Map<string, number>()
let logOn = readLogFlag()

function readLogFlag(): boolean {
  try {
    return typeof localStorage !== 'undefined' && localStorage.getItem(LOG_FLAG_KEY) === '1'
  } catch {
    return false
  }
}

/** Note one finished `/cli` call. */
export function recordCliTiming(t: CliTiming): void {
  ring.push(t)
  if (ring.length > MAX_KEPT) ring.splice(0, ring.length - MAX_KEPT)
  lastActivity.set(t.host, t.at + t.totalMs)
  if (logOn) {
    // eslint-disable-next-line no-console
    console.debug(
      `[cli-timing] ${t.method} ${t.route} host=${t.host} lane=${t.lane} queued=${t.queuedMs.toFixed(0)}ms network=${t.networkMs.toFixed(0)}ms total=${t.totalMs.toFixed(0)}ms status=${t.status ?? 'threw'}`,
    )
  }
}

/** The kept timings, oldest first (a copy). */
export function getCliTimings(): CliTiming[] {
  return ring.slice()
}

/** When a `/cli` call to `host` last finished (wall clock ms), or null. */
export function lastCliActivityAt(host: string): number | null {
  return lastActivity.get(host) ?? null
}

/** Note traffic to `host` that did not ride `/cli` (the warm ping). */
export function noteHostActivity(host: string, at: number = Date.now()): void {
  lastActivity.set(host, at)
}

function pct(sorted: number[], p: number): number {
  if (sorted.length === 0) return 0
  const i = Math.min(sorted.length - 1, Math.max(0, Math.ceil((p / 100) * sorted.length) - 1))
  return Math.round(sorted[i])
}

/** Per route (optionally one host): count and queued / network percentiles. */
export function summarizeCliTimings(host?: string): CliTimingRouteSummary[] {
  const by = new Map<string, CliTiming[]>()
  for (const t of ring) {
    if (host && t.host !== host) continue
    const list = by.get(t.route) ?? []
    list.push(t)
    by.set(t.route, list)
  }
  const out: CliTimingRouteSummary[] = []
  for (const [route, list] of by) {
    const q = list.map((t) => t.queuedMs).sort((a, b) => a - b)
    const n = list.map((t) => t.networkMs).sort((a, b) => a - b)
    out.push({
      route,
      count: list.length,
      queuedP50: pct(q, 50),
      queuedP95: pct(q, 95),
      queuedMax: Math.round(q[q.length - 1] ?? 0),
      networkP50: pct(n, 50),
      networkP95: pct(n, 95),
      networkMax: Math.round(n[n.length - 1] ?? 0),
    })
  }
  return out.sort((a, b) => b.count - a.count)
}

/** Turn the per-request console line on or off (remembered on this computer). */
export function setCliTimingLog(on: boolean): void {
  logOn = on
  try {
    if (on) localStorage.setItem(LOG_FLAG_KEY, '1')
    else localStorage.removeItem(LOG_FLAG_KEY)
  } catch {
    /* storage blocked: on for this session only */
  }
}

/** Tests: forget everything. */
export function resetCliTimingsForTests(): void {
  ring.length = 0
  lastActivity.clear()
  logOn = false
}

export interface CliTimingConsole {
  log(on: boolean): void
  summary(host?: string): CliTimingRouteSummary[]
  list(): CliTiming[]
  table(limit?: number): void
  clear(): void
}

const consoleHelper: CliTimingConsole = {
  log: setCliTimingLog,
  summary: (host?: string) => {
    const s = summarizeCliTimings(host)
    // eslint-disable-next-line no-console
    console.table(s)
    return s
  },
  list: getCliTimings,
  table: (limit = 50) => {
    // eslint-disable-next-line no-console
    console.table(
      ring.slice(-limit).map((t) => ({
        at: new Date(t.at).toISOString().slice(11, 23),
        host: t.host,
        route: t.route,
        method: t.method,
        lane: t.lane,
        queued: Math.round(t.queuedMs),
        network: Math.round(t.networkMs),
        total: Math.round(t.totalMs),
        status: t.status ?? 'threw',
      })),
    )
  },
  clear: () => {
    ring.length = 0
  },
}

if (typeof window !== 'undefined') {
  ;(window as unknown as { k2CliTiming?: CliTimingConsole }).k2CliTiming = consoleHelper
}
