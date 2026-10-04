// prd-zen-mode-v1 Z8, Z11, Z12, Z15 — Zen config from THIS computer's daemon.
//
// `~/.k2/zen/` belongs to this computer, so every request here goes to
// `scopeForHost('local')`, never `primaryScope()`: a window switched to
// another server still reads and writes this computer's Zen (T4.2). The
// routes answer only the owner token (Z15a), which is what the local scope
// carries.
//
// Routes used (contract in `docs/zen-contract.md`):
//   POST /cli/zen/page/ensure {homeId, name}   first time a Home's Zen opens
//   GET  /cli/zen/get?home=<id>                the resolved page
//   POST /cli/zen/homes/sync {homes:[{id,name}]}  on Home create/rename/delete
//   `zen_changed` (app event, payload-free) on the local daemon's app bus
//
// Every body is parsed at the boundary (`parseZenGet`).

import { create } from 'zustand'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { scopeForHost, type ServerScope } from '@/kessel/server-scope'
import { onZenChanged, subscribeToActiveState } from '@/stores/session-events'
import { parseZenGet, ZenPageParseError, type ZenResolvedPage } from './zen-page'

/** This computer's daemon. The only scope Zen config ever uses. */
export function zenLocalScope(): ServerScope {
  return scopeForHost('local')
}

/** Why loading the page failed: the daemon didn't answer, or it answered
 *  something that isn't a page. */
export type ZenLoadFailure = { kind: 'unreachable' | 'unreadable'; message: string }

export type ZenConfigState =
  | { status: 'idle'; homeId: null; page: null; failure: null }
  | { status: 'loading'; homeId: string; page: ZenResolvedPage | null; failure: null }
  | { status: 'ready'; homeId: string; page: ZenResolvedPage; failure: null }
  | { status: 'failed'; homeId: string; page: null; failure: ZenLoadFailure }

export const useZenConfigStore = create<ZenConfigState>(() => ({
  status: 'idle',
  homeId: null,
  page: null,
  failure: null,
}))

/** `POST /cli/zen/page/ensure`: make `pages/<homeId>.toml` from the stub if
 *  it is missing (never overwrites; creates `~/.k2/zen/` on first use). */
export async function ensureZenPage(homeId: string, name: string): Promise<void> {
  await daemonCliPost(zenLocalScope(), 'zen/page/ensure', { homeId, name })
}

/** `GET /cli/zen/get?home=<id>`, parsed. */
export async function fetchZenPage(homeId: string): Promise<ZenResolvedPage> {
  const raw = await daemonCliGet<unknown>(zenLocalScope(), 'zen/get', { home: homeId })
  return parseZenGet(raw)
}

/** `POST /cli/zen/homes/sync`: refresh `homes.json` (id → name). */
export async function syncZenHomes(homes: ReadonlyArray<{ id: string; name: string }>): Promise<void> {
  await daemonCliPost(zenLocalScope(), 'zen/homes/sync', {
    homes: homes.map((h) => ({ id: h.id, name: h.name })),
  })
}

let loadSeq = 0
const ensured = new Set<string>()

/**
 * Load the Zen page for `homeId` into `useZenConfigStore`. Ensures the page
 * file once per Home per session (Z11), then reads the resolved page. A
 * newer call wins; an older answer is dropped.
 */
export async function loadZenPage(homeId: string, name: string): Promise<ZenConfigState> {
  const seq = ++loadSeq
  const prev = useZenConfigStore.getState()
  useZenConfigStore.setState({
    status: 'loading',
    homeId,
    page: prev.homeId === homeId ? prev.page : null,
    failure: null,
  } as ZenConfigState)
  let next: ZenConfigState
  try {
    if (!ensured.has(homeId)) {
      await ensureZenPage(homeId, name)
      ensured.add(homeId)
    }
    const page = await fetchZenPage(homeId)
    next = { status: 'ready', homeId, page, failure: null }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err)
    next = {
      status: 'failed',
      homeId,
      page: null,
      failure: { kind: err instanceof ZenPageParseError ? 'unreadable' : 'unreachable', message },
    }
  }
  if (seq === loadSeq) useZenConfigStore.setState(next)
  return next
}

/**
 * Call `fn` on every `zen_changed` from THIS computer's daemon, whatever
 * server the window is on. Holds a dedicated app socket to this computer's
 * daemon (`scopeForHost('local')`) while subscribed, so Zen never depends on
 * the window's own socket (which follows the server switcher). Returns the
 * unsubscribe (closes that socket).
 */
export function watchLocalZenChanged(fn: () => void): () => void {
  const local = zenLocalScope()
  const off = onZenChanged(local, fn)
  const closeSocket = subscribeToActiveState(local)
  return () => {
    off()
    closeSocket()
  }
}

/** Tests only. */
export function __resetZenApiForTests(): void {
  loadSeq = 0
  ensured.clear()
  useZenConfigStore.setState({ status: 'idle', homeId: null, page: null, failure: null })
}
