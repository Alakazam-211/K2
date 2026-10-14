// Garden v3 data sources — the Garden's "Review N pending" (PRD §9.5, Q12;
// contract-sources-v1 §8, §11.4). The sources a Garden's placements name
// that are still `proposed` on their servers, grouped by server host key
// (`local`, `akzm.k2.dev`), for B2's batch dialog
// (`openSourcesBatchReview({gardenName, byServer})`).
//
// The dialog is B2's (Settings); it registers itself here
// (`registerZenSourcesBatchReview`) so the Garden never imports Settings and
// the two land independently. Until it is registered, the pill isn't shown.

import { parseSourceAddress, type ZenSourcesReader } from './zen-sources'

export interface ZenSourcesBatchArgs {
  gardenName?: string
  /** Host key → source names on that server. */
  byServer: Record<string, string[]>
}

export type ZenSourcesBatchOpener = (args: ZenSourcesBatchArgs) => void

let opener: ZenSourcesBatchOpener | null = null

/** B2's dialog registers its entry point (once, at module load). */
export function registerZenSourcesBatchReview(fn: ZenSourcesBatchOpener | null): void {
  opener = fn
}

export function zenSourcesBatchOpener(): ZenSourcesBatchOpener | null {
  return opener
}

/** The proposed sources among `addresses`, by server. A source K2 can't
 *  read now is left out (its card says why). */
export async function zenPendingSources(addresses: readonly string[], reader: ZenSourcesReader): Promise<Record<string, string[]>> {
  const out: Record<string, string[]> = {}
  const seen = new Set<string>()
  await Promise.all(
    addresses.map(async (a) => {
      const p = parseSourceAddress(a)
      if (!p) return
      const host = p.host ?? 'local'
      const key = `${host}\n${p.name}`
      if (seen.has(key)) return
      seen.add(key)
      try {
        const v = await reader.read(a)
        if (v.status !== 'proposed') return
        ;(out[host] ??= []).push(p.name)
      } catch (err) {
        console.warn(`[zen] pending check for ${a}:`, err)
      }
    }),
  )
  for (const names of Object.values(out)) names.sort()
  return out
}

export function zenPendingCount(byServer: Record<string, string[]>): number {
  return Object.values(byServer).reduce((n, names) => n + names.length, 0)
}
