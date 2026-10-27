// `k2.download` (prd-garden-board-needs-v1 GS1; security review #1, belt
// and braces): K2's own card the first time a widget saves a file to
// Downloads in a Garden.
//
// "Always" is kept by this computer's daemon, per Garden and widget
// (`GET/POST /cli/zen/downloads*`, `~/.k2/zen/downloads.json`), so every
// client (each window, the web client) honours the same answer, clearing a
// browser's storage never skips the card, and revoking it brings the card
// back everywhere. Only the owner token reads or changes that list.
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

/** `POST /cli/zen/downloads/allow` (the owner token: only the person's app). */
export async function allowZenDownloadWidget(gardenId: string, widget: string): Promise<void> {
  await daemonCliPost(zenLocalScope(), 'zen/downloads/allow', { garden: gardenId, widget })
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
  widgets: fetchZenDownloadWidgets,
  allow: allowZenDownloadWidget,
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
