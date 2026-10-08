// prd-zen-mode-v1 Z8, Z12, Z15 and prd-zen-gardens-v1 G12, G23 — a
// Garden's resolved page from THIS computer's daemon.
//
// `~/.k2/zen/` belongs to this computer, so every request here goes to
// `scopeForHost('local')`, never `primaryScope()`: a window switched to
// another server still reads and writes this computer's Zen (T4.2). The
// routes answer only the owner token (Z15a), which is what the local scope
// carries.
//
// Routes used here (contract in `docs/zen-contract.md`):
//   GET  /cli/zen/get?garden=<id>   the resolved page of one Garden
//        (&preview=page|theme|both while Settings previews K2's default
//        for that Garden: prd-zen-garden-sync-defaults-v1 GS25, zen-sync.ts)
//   `zen_changed` (app event, payload-free) on the local daemon's app bus
// The Garden list and its routes are `zen-gardens.ts`.
//
// Every body is parsed at the boundary (`parseZenGet`).

import { create } from 'zustand'
import { daemonCliGet } from '@/lib/daemon-cli'
import { scopeForHost, type ServerScope } from '@/kessel/server-scope'
import { onZenChanged, subscribeToActiveState } from '@/stores/session-events'
import { parseZenGet, ZenPageParseError, type ZenResolvedPage } from './zen-page'
import { zenSyncPreviewParam } from './zen-sync'

/** This computer's daemon. The only scope Zen config ever uses. */
export function zenLocalScope(): ServerScope {
  return scopeForHost('local')
}

/** Why loading failed: the daemon didn't answer, answered something that
 *  isn't a page, or has no Gardens routes (older than this app, G19). */
export type ZenLoadFailure = { kind: 'unreachable' | 'unreadable' | 'outdated'; message: string }

/** An older daemon's answer for a Zen route it doesn't have (TG1.2). */
export function isZenRouteMissing(err: unknown): boolean {
  const msg = err instanceof Error ? err.message : String(err)
  return /unknown zen route/i.test(msg)
}

export function zenLoadFailure(err: unknown): ZenLoadFailure {
  const message = err instanceof Error ? err.message : String(err)
  if (isZenRouteMissing(err)) return { kind: 'outdated', message }
  return { kind: err instanceof ZenPageParseError ? 'unreadable' : 'unreachable', message }
}

export type ZenConfigState =
  | { status: 'idle'; gardenId: null; page: null; failure: null }
  | { status: 'loading'; gardenId: string; page: ZenResolvedPage | null; failure: null }
  | { status: 'ready'; gardenId: string; page: ZenResolvedPage; failure: null }
  | { status: 'failed'; gardenId: string; page: null; failure: ZenLoadFailure }

export const useZenConfigStore = create<ZenConfigState>(() => ({
  status: 'idle',
  gardenId: null,
  page: null,
  failure: null,
}))

/** `GET /cli/zen/get?garden=<id>`, parsed. While this window previews K2's
 *  default for the Garden, `preview=` asks for it (the daemon writes nothing). */
export async function fetchZenPage(gardenId: string): Promise<ZenResolvedPage> {
  const preview = zenSyncPreviewParam(gardenId)
  const raw = await daemonCliGet<unknown>(zenLocalScope(), 'zen/get', preview ? { garden: gardenId, preview } : { garden: gardenId })
  return parseZenGet(raw)
}

let loadSeq = 0

/**
 * Load the resolved page of `gardenId` into `useZenConfigStore`. A newer
 * call wins; an older answer is dropped.
 */
export async function loadZenPage(gardenId: string): Promise<ZenConfigState> {
  const seq = ++loadSeq
  const prev = useZenConfigStore.getState()
  useZenConfigStore.setState({
    status: 'loading',
    gardenId,
    page: prev.gardenId === gardenId ? prev.page : null,
    failure: null,
  } as ZenConfigState)
  let next: ZenConfigState
  try {
    const page = await fetchZenPage(gardenId)
    next = { status: 'ready', gardenId, page, failure: null }
  } catch (err) {
    next = { status: 'failed', gardenId, page: null, failure: zenLoadFailure(err) }
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
  useZenConfigStore.setState({ status: 'idle', gardenId: null, page: null, failure: null })
}
