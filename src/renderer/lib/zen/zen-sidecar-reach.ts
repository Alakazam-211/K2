// 0.46.7 Workbench (lane W2) — a Garden limit patched: a widget (and the
// Conversation widget that follows it) could reach a workspace's main
// agent only, never a sidecar (`<ws>/<name>`), so a Garden couldn't open a
// worktree agent's own Thread.
//
// This keeps THIS computer's sidecars as Garden rows (`<ws>/<name>::local`),
// read from the daemon (`GET /cli/workbench/agents`, the daemon's truth:
// every client sees the same list), cached per session and refreshed at
// most every 15 s while a Garden asks. `zen-custom-scope` puts each row
// right after its workspace's row in the reach. Rows on other computers are
// their Homes' rows as before (read only in the Workbench: a Thread runs
// where its agent runs).

import { create } from 'zustand'
import type { HomeRow } from '@/stores/homes'
import { daemonCliGet } from '@/lib/daemon-cli'
import { scopeForHost } from '@/kessel/server-scope'
import { LOCAL_HOME_HOST } from '@/lib/host-key'

/** Re-read the list no more often than this (ms). */
export const ZEN_SIDECAR_REFRESH_MS = 15_000

/** One sidecar as the reach keeps it. */
export interface ZenSidecarRow extends HomeRow {
  /** The parent workspace's row address (`ws::local`). */
  parent: string
  /** The Thread address, `ws/name`. */
  thread: string
}

interface State {
  status: 'idle' | 'loading' | 'ready' | 'failed'
  rows: ZenSidecarRow[]
  at: number
}

export const useZenSidecarReach = create<State>(() => ({ status: 'idle', rows: [], at: 0 }))

let asked = false

function isObj(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

/** `/cli/workbench/agents` → rows (anything malformed is left out). */
export function zenSidecarRowsFrom(raw: unknown): ZenSidecarRow[] {
  const list = isObj(raw) && Array.isArray(raw.agents) ? raw.agents : null
  if (!list) throw new Error('workbench/agents: not a list')
  const out: ZenSidecarRow[] = []
  const seen = new Set<string>()
  for (const a of list) {
    if (!isObj(a)) continue
    const { address, thread, workspace } = a
    if (typeof address !== 'string' || typeof thread !== 'string' || typeof workspace !== 'string') continue
    const addr = address.trim().toLowerCase()
    if (!addr.endsWith(`::${LOCAL_HOME_HOST}`) || !addr.includes('/') || seen.has(addr)) continue
    seen.add(addr)
    const name = typeof a.name === 'string' && a.name ? a.name : thread.split('/')[1] ?? thread
    const label = typeof a.workspaceLabel === 'string' && a.workspaceLabel ? `${name} · ${a.workspaceLabel}` : name
    out.push({
      address: addr,
      workspaceId: typeof a.workspaceId === 'string' ? a.workspaceId : null,
      label,
      parent: `${workspace.trim().toLowerCase()}::${LOCAL_HOME_HOST}`,
      thread: thread.trim().toLowerCase(),
    })
  }
  return out
}

/** Ask for the list (no-op while loading or fresh). Never writes a store
 *  during a render: the read starts next microtask. */
export function requestZenSidecarReach(now: number = Date.now()): void {
  const s = useZenSidecarReach.getState()
  if (asked || s.status === 'loading' || (s.status !== 'idle' && now - s.at < ZEN_SIDECAR_REFRESH_MS)) return
  asked = true
  queueMicrotask(() => {
    asked = false
    useZenSidecarReach.setState((p) => ({ ...p, status: 'loading', at: now }))
    void daemonCliGet<unknown>(scopeForHost(LOCAL_HOME_HOST), 'workbench/agents')
      .then((raw) => useZenSidecarReach.setState({ status: 'ready', rows: zenSidecarRowsFrom(raw), at: Date.now() }))
      .catch((err: unknown) => {
        // An older daemon (no route) or a refusal: no sidecar rows, said once.
        console.warn('[zen] this computer’s sidecars are unavailable to Gardens:', err)
        useZenSidecarReach.setState((p) => ({ ...p, status: 'failed', at: Date.now() }))
      })
  })
}

/** The sidecar rows now (asks for a refresh when stale). */
export function zenSidecarRows(): readonly ZenSidecarRow[] {
  requestZenSidecarReach()
  return useZenSidecarReach.getState().rows
}

/** `ws/name` for a sidecar row's address, else null (a workspace's row). */
export function zenSidecarThread(address: string): string | null {
  const t = address.trim().toLowerCase()
  const i = t.indexOf('::')
  const handle = i > 0 ? t.slice(0, i) : t
  const j = handle.indexOf('/')
  return j > 0 && j < handle.length - 1 && !handle.slice(j + 1).includes('/') ? handle : null
}

/** Tests only. */
export function __resetZenSidecarReachForTests(rows: ZenSidecarRow[] = [], status: State['status'] = 'idle'): void {
  asked = false
  useZenSidecarReach.setState({ status, rows, at: status === 'idle' ? 0 : Date.now() })
}
