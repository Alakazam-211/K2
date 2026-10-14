// "Also pin the N sources it shows" when a person locks a Garden
// (contract-sources-v1 §8; PRD §10.2, GV65, Q19). Owner: B2.
//
// A lock stops agents changing the Garden on this computer; the sources its
// cards read can live on other servers, out of the lock's reach. Pinning a
// source (on ITS server, with the person's own login there) stops agents
// editing or removing it (423 `source_pinned`). The addresses come from the
// Garden's resolved page (`sourceAddresses` on each card, which the daemon
// fills from the placement's config or the manifest); a page without them
// (an older daemon) offers nothing.

import { daemonCliGet } from '@/lib/daemon-cli'
import { zenLocalScope } from '@/lib/zen/zen-api'
import { scopeForHost } from '@/kessel/server-scope'
import { pinSource } from '@/lib/sources/sources-api'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { sourcesErrorText } from './data-sources-ui'

/** Every source address a resolved Garden page shows, de-duplicated. */
export function gardenSourceAddresses(page: unknown): string[] {
  const out: string[] = []
  const widgets = (page as { widgets?: unknown } | null)?.widgets
  if (!Array.isArray(widgets)) return out
  for (const w of widgets) {
    const addrs = (w as { sourceAddresses?: unknown } | null)?.sourceAddresses
    if (!Array.isArray(addrs)) continue
    for (const a of addrs) {
      if (typeof a === 'string' && a !== '' && !out.includes(a)) out.push(a)
    }
  }
  return out
}

/** `fleet` → this computer; `fleet::akzm.k2.dev` → that server. */
export function splitAddress(address: string): { name: string; host: string } {
  const at = address.indexOf('::')
  return at < 0 ? { name: address, host: 'local' } : { name: address.slice(0, at), host: address.slice(at + 2) }
}

export interface PinOutcome {
  pinned: string[]
  failed: Array<{ address: string; text: string }>
}

/** Pin each address on its own server. */
export async function pinAddresses(
  addresses: string[],
  pin: (host: string, name: string) => Promise<unknown> = (host, name) => pinSource(scopeForHost(host), name),
): Promise<PinOutcome> {
  const out: PinOutcome = { pinned: [], failed: [] }
  for (const a of addresses) {
    const { name, host } = splitAddress(a)
    try {
      await pin(host, name)
      out.pinned.push(a)
    } catch (e) {
      out.failed.push({ address: a, text: sourcesErrorText(e) })
    }
  }
  return out
}

/** After a lock: offer to pin the sources the Garden shows. Returns what
 *  happened (null when there was nothing to offer or the person said no). */
export async function offerPinOnLock(gardenId: string, gardenName: string): Promise<PinOutcome | null> {
  let page: unknown
  try {
    page = await daemonCliGet<unknown>(zenLocalScope(), 'zen/get', { garden: gardenId })
  } catch {
    return null
  }
  const addresses = gardenSourceAddresses((page as { page?: unknown })?.page ?? page)
  if (addresses.length === 0) return null
  const n = addresses.length
  const ok = await useConfirmDialogStore.getState().confirm({
    title: `Also pin the ${n} source${n === 1 ? '' : 's'} it shows?`,
    message:
      `${gardenName} is locked on this computer. Pinning its sources stops agents editing or removing them on ` +
      `their servers too: ${addresses.join(', ')}.`,
    confirmLabel: n === 1 ? 'Pin it' : 'Pin them',
  })
  if (!ok) return null
  return pinAddresses(addresses)
}
