// Garden v3 data sources — what one placement's frame gets for the
// `sources.*` verbs (contract-sources-v1 §6): its bound addresses (the
// daemon's `sourceAddresses`: the placement's config.source / sources /
// latest, or the manifest's fixed names), the window's reader, and K2's Ask
// box. Kept apart from `ZenCustomFrame` so the bridge tests can build it.

import type { ZenCustomSources } from './zen-custom-bridge'
import type { ZenCustomWidgetPayload } from './zen-custom-types'
import { useZenGardensStore } from './zen-gardens'
import { openZenSourceAsk } from './zen-source-ask'
import { canonicalSourceAddress } from './zen-sources'
import { zenSourcesReader } from './zen-sources-live'

/** The placement's addresses, canonical, never more than the daemon bound. */
export function zenBoundSourceAddresses(w: Pick<ZenCustomWidgetPayload, 'sourceAddresses'>): string[] {
  const out: string[] = []
  for (const a of w.sourceAddresses ?? []) {
    const c = canonicalSourceAddress(a)
    if (c && !out.includes(c)) out.push(c)
  }
  return out
}

export function zenCustomSourcesFor(gardenId: string, widget: () => ZenCustomWidgetPayload): ZenCustomSources {
  return {
    bound: () => zenBoundSourceAddresses(widget()),
    get reader() {
      return zenSourcesReader()
    },
    ask: async (address, itemKey) => {
      const garden = useZenGardensStore.getState().gardens.find((g) => g.id === gardenId)
      return openZenSourceAsk({
        gardenId,
        gardenName: garden?.name ?? 'this Garden',
        address,
        itemKey,
        view: zenSourcesReader().cached(address) ?? null,
        madeBy: null,
      })
    },
  }
}
