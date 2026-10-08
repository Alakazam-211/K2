// Thread survives a tab rename (prd-thread-survives-tab-rename-v1 S4).
//
// A renamed sidecar answers at a new address (`k2/3` → `k2/reviewer`); the
// old one keeps working on a 0.45.1+ daemon. Three things can tell this
// client the new address:
//   - the workspace socket's `session_address_changed` (S3),
//   - the conversation's overlay socket `address` frame (TR13),
//   - a Thread HTTP answer with `movedFrom`, or a Thread 404 on an older
//     daemon (self-heal: look the address up again).
// They all land here, and the Thread views (`useSidecarOverlayAddr`, the
// Thread hook) follow without a remount.

export interface ThreadAddressChange {
  /** The address the chat answers at now. Empty = unknown, look it up. */
  address: string
  /** The address it answered at before (what a view may still hold). */
  previous: string
  conversationId?: string | null
  paneGroupId?: string | null
  workspacePath?: string | null
}

type Listener = (change: ThreadAddressChange) => void

const listeners = new Set<Listener>()

export function subscribeThreadAddress(listener: Listener): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

export function publishThreadAddress(change: ThreadAddressChange): void {
  for (const l of [...listeners]) {
    try {
      l(change)
    } catch (e) {
      console.error('[thread-address] listener failed', e)
    }
  }
}

/** A Thread call for `addr` missed (404) or answered `movedFrom`: ask the
 *  sidecar views holding `addr` to look their address up again. */
export function requestThreadAddressRelookup(addr: string, address = ''): void {
  if (!addr) return
  publishThreadAddress({ address, previous: addr })
}

/** The person-readable text of a failed Thread call: the daemon's
 *  `{error:{code,hint}}` hint, else the message. */
export function threadErrorText(e: unknown): string {
  const raw = e instanceof Error ? e.message : String(e ?? '')
  try {
    const parsed = JSON.parse(raw) as { error?: { hint?: unknown; code?: unknown } | string }
    const err = parsed?.error
    if (typeof err === 'string' && err) return err
    if (err && typeof err === 'object' && typeof err.hint === 'string' && err.hint) return err.hint
  } catch {
    /* not JSON */
  }
  return raw || 'Thread send failed'
}

/** True when a Thread call failed because the address did not resolve
 *  (an older daemon after a rename, or a stale view): look it up again. */
export function isThreadAddressMiss(e: unknown): boolean {
  const raw = e instanceof Error ? e.message : String(e ?? '')
  return raw.includes('unknown overlay addr') || raw.includes('"code":"not_found"')
}

/** Show an address under its current name (Q5): a stored `from`/`to` that
 *  is one of this chat's old addresses renders as the current one. */
export function displayThreadAddress(
  stored: string,
  current: string | null | undefined,
  past: readonly string[] | null | undefined,
): string {
  if (!stored || !current || !past || past.length === 0) return stored
  return past.includes(stored) ? current : stored
}

/** True when `change` is about a view holding `addr` / `conversationId` /
 *  `paneGroupId`. */
export function threadAddressChangeMatches(
  change: ThreadAddressChange,
  view: { addr?: string | null; conversationId?: string | null; paneGroupId?: string | null },
): boolean {
  if (view.paneGroupId && change.paneGroupId && change.paneGroupId === view.paneGroupId) return true
  if (view.conversationId && change.conversationId && change.conversationId === view.conversationId) {
    return true
  }
  return Boolean(view.addr) && change.previous === view.addr
}
