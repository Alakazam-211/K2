// `k2.download` (prd-garden-board-needs-v1 GS1; security review #1, belt
// and braces): K2's own card the first time a widget saves a file to
// Downloads in a Garden.
//
// "Always" is kept by this computer's daemon, per Garden and widget
// (`GET/POST /cli/zen/downloads*`, `~/.k2/zen/downloads.json`), so every
// client (each window, the web client) honours the same answer, clearing a
// browser's storage never skips the card, and revoking it brings the card
// back everywhere. Only the owner token reads or changes that list, and
// the daemon audits every change. An "Always" an older app kept in this
// browser's localStorage is carried to the daemon once, before the first
// read of the list in an app load (`migrateLegacyZenDownloadConsent`).
// "Save once" is this one save: nothing is kept.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { zenLocalScope } from './zen-api'

/** The person's answer on K2's card. */
export type ZenDownloadAnswer = 'deny' | 'once' | 'always'

/** What K2's card shows (all K2's words; the widget gave only the name). */
export interface ZenDownloadAsk {
  widget: string
  garden: string
  name: string
  /** Bytes. */
  size: number
}

/** The daemon's list, injectable for tests. */
export interface ZenDownloadConsentStore {
  /** The widgets this Garden lets save files without asking. */
  widgets(gardenId: string): Promise<readonly string[]>
  /** "Always": the daemon keeps it. */
  allow(gardenId: string, widget: string): Promise<void>
}

/** `GET /cli/zen/downloads?garden=` on this computer's daemon. */
export async function fetchZenDownloadWidgets(gardenId: string): Promise<readonly string[]> {
  const r = await daemonCliGet<{ widgets?: unknown }>(zenLocalScope(), 'zen/downloads', { garden: gardenId })
  if (!Array.isArray(r.widgets) || !r.widgets.every((w) => typeof w === 'string')) {
    throw new Error('zen/downloads answered without a widgets list')
  }
  return r.widgets as string[]
}

/** `POST /cli/zen/downloads/allow` (the owner token: only the person's app;
 *  the daemon audits it). `migrated`: the one-time carry-over below. */
export async function allowZenDownloadWidget(gardenId: string, widget: string, migrated = false): Promise<void> {
  await daemonCliPost(zenLocalScope(), 'zen/downloads/allow', migrated ? { garden: gardenId, widget, migrated: true } : { garden: gardenId, widget })
}

/** Where an app before the daemon kept the list kept "Always": this
 *  browser's localStorage, a JSON array of `<gardenId>/<widget>`. */
export const LEGACY_DOWNLOAD_CONSENT_KEY = 'k2.zen.downloadsAllowed'

/** The bit of `Storage` the carry-over uses (injectable for tests). */
export type LegacyConsentStorage = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>

function legacyStorage(): LegacyConsentStorage | null {
  try {
    return globalThis.localStorage ?? null
  } catch {
    // A sandboxed or storage-blocked page has nothing to carry over.
    return null
  }
}

/** One-time carry-over of an "Always" this app kept in its own storage
 *  into the daemon's list: each `<garden>/<widget>` is allowed there
 *  (`migrated: true`, audited), and the key is removed once all of them
 *  landed. An entry the daemon refused stays for the next try; one that
 *  isn't `<garden>/<widget>` is dropped. Answers how many were carried. */
export async function migrateLegacyZenDownloadConsent(
  allow: (gardenId: string, widget: string) => Promise<void>,
  storage: LegacyConsentStorage | null = legacyStorage(),
): Promise<number> {
  const raw = storage?.getItem(LEGACY_DOWNLOAD_CONSENT_KEY)
  if (!storage || raw == null) return 0
  let entries: unknown
  try {
    entries = JSON.parse(raw)
  } catch {
    entries = []
  }
  const pairs = (Array.isArray(entries) ? entries : [])
    .filter((e): e is string => typeof e === 'string' && e.indexOf('/') > 0)
    .map((e) => [e.slice(0, e.indexOf('/')), e.slice(e.indexOf('/') + 1)] as const)
    .filter(([, w]) => w.length > 0 && !w.includes('/'))
  const left: string[] = []
  let carried = 0
  for (const [g, w] of pairs) {
    try {
      await allow(g, w)
      carried += 1
    } catch (err) {
      console.warn('[zen] k2.download: carrying an older Always answer to the daemon failed:', err)
      left.push(`${g}/${w}`)
    }
  }
  if (left.length > 0) storage.setItem(LEGACY_DOWNLOAD_CONSENT_KEY, JSON.stringify(left))
  else storage.removeItem(LEGACY_DOWNLOAD_CONSENT_KEY)
  return carried
}

let legacyCarry: Promise<number> | null = null

/** [`migrateLegacyZenDownloadConsent`] once per app load, before the first
 *  read of the daemon's list. */
function carryLegacyOnce(): Promise<number> {
  legacyCarry ??= migrateLegacyZenDownloadConsent((g, w) => allowZenDownloadWidget(g, w, true)).catch((err) => {
    console.warn('[zen] k2.download: carrying older Always answers failed:', err)
    return 0
  })
  return legacyCarry
}

/** Tests only. */
export function __resetZenDownloadCarryForTests(): void {
  legacyCarry = null
}

/** `POST /cli/zen/downloads/spend`: the daemon's hourly budget for this
 *  Garden's widget, shared by every client. Null when the save fits (and is
 *  counted), else the reason. */
export async function spendZenDownload(gardenId: string, widget: string, bytes: number): Promise<string | null> {
  const r = await daemonCliPost<{ allowed?: unknown; reason?: unknown }>(zenLocalScope(), 'zen/downloads/spend', {
    garden: gardenId,
    widget,
    bytes,
  })
  if (r.allowed === true) return null
  if (r.allowed === false) return typeof r.reason === 'string' && r.reason ? r.reason : 'the hour’s download budget is spent'
  throw new Error('zen/downloads/spend answered without allowed')
}

export const daemonZenDownloadConsent: ZenDownloadConsentStore = {
  widgets: async (gardenId) => {
    await carryLegacyOnce()
    return fetchZenDownloadWidgets(gardenId)
  },
  allow: (gardenId, widget) => allowZenDownloadWidget(gardenId, widget),
}

/** Ask (or not) and answer whether to go on. */
export async function zenDownloadConsent(
  store: ZenDownloadConsentStore,
  gardenId: string,
  widget: string,
  ask: ZenDownloadAsk,
  show: (ask: ZenDownloadAsk) => Promise<ZenDownloadAnswer>,
): Promise<boolean> {
  if ((await store.widgets(gardenId)).includes(widget)) return true
  const answer = await show(ask)
  if (answer === 'always') {
    try {
      await store.allow(gardenId, widget)
    } catch (err) {
      // The person said yes: save this one; they'll be asked again next time.
      console.warn('[zen] k2.download: keeping the answer failed:', err)
    }
  }
  return answer !== 'deny'
}
