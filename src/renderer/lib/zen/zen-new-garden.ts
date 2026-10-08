// Rosson 2026-10-08 — New Garden is one modal: a name and a browsable list
// of the catalog defaults (Diary first, then the plain starts). Pick one,
// name it, Create. For K2's own catalog Gardens the create click is the
// consent: no separate grant or scope dialog, just one plain sentence in
// the modal ("Diary can read your agents on this computer and post to
// their Threads."). The review dialog for widgets a user or an agent wrote
// is unchanged (`ZenGrantDialog`).
//
// The modal outlives the menu that opened it (the Garden switcher's
// dropdown, a `menu` widget's list), so whether it is open lives here, per
// window, and `ZenNewGardenHost` (mounted once by `ZenPage`) draws it.

import { create } from 'zustand'
import type { ZenGardenNewRequest, ZenScope, ZenTemplateInfo } from './zen-custom-types'
import { zenGrantEntries, zenScopeRows } from './zen-custom-scope'
import { LOCAL_HOME_HOST } from '@/lib/home-address'

export interface ZenNewGardenModalState {
  open: boolean
  /** A catalog short to open on, selected ("See it in New Garden"). */
  highlight: string | null
}

export const useZenNewGardenModal = create<ZenNewGardenModalState>(() => ({ open: false, highlight: null }))

export function openZenNewGarden(highlight: string | null = null): void {
  useZenNewGardenModal.setState({ open: true, highlight })
}

export function closeZenNewGarden(): void {
  useZenNewGardenModal.setState({ open: false, highlight: null })
}

/** This computer's agents: the one scope a K2 catalog Garden is given. */
export const ZEN_LOCAL_SCOPE: ZenScope = Object.freeze({ server: LOCAL_HOME_HOST }) as ZenScope

/** The cards, in the order the modal shows them: the catalog (the Diary
 *  first, by the daemon's `order`), then the starts (default, empty). */
export function zenNewGardenCards(list: readonly ZenTemplateInfo[]): ZenTemplateInfo[] {
  return [...list.filter((t) => t.section === 'catalog'), ...list.filter((t) => t.section === 'start')]
}

/** The scope a catalog Garden's widget gets in the create click: the fixed
 *  one (`local`), and this computer's agents when the catalog leaves it
 *  open (a K2 catalog Garden never asks for more in one click). */
export function zenCatalogScope(t: ZenTemplateInfo): ZenScope | null {
  if (!t.needsGrant) return null
  return ZEN_LOCAL_SCOPE
}

/** The one sentence the create click agrees to, or null when it grants nothing. */
export function zenCatalogConsent(t: ZenTemplateInfo): string | null {
  const g = t.needsGrant
  if (!g) return null
  if (g.consent) return g.consent
  const can: string[] = []
  if (g.caps.includes('agents:read')) can.push('read your agents on this computer')
  if (g.caps.includes('thread:read')) can.push('read their Threads')
  if (g.caps.includes('thread:post')) can.push('post to their Threads')
  if (g.caps.includes('presence:read')) can.push('see who is with them')
  if (can.length === 0) return null
  const last = can.pop() as string
  return `${t.label} can ${can.length ? `${can.join(', ')} and ${last}` : last}.`
}

/** The `grant` body for creating `t` (null when it grants nothing): the
 *  local scope and the agents it covers now, Sending on. */
export function zenCatalogGrantRequest(t: ZenTemplateInfo): ZenGardenNewRequest['grant'] | null {
  const scope = zenCatalogScope(t)
  if (!scope || !t.needsGrant) return null
  return {
    scope,
    sending: t.needsGrant.caps.includes('thread:post'),
    entries: zenGrantEntries(zenScopeRows(scope).rows),
  }
}

/** The fixed scope of a `k2:` widget from the catalog that grants it
 *  (`k2:diary@1` → this computer), so the review card of a preinstalled
 *  Diary asks no scope either. Null for any other widget. */
export function zenCatalogFixedScopeFor(widget: string, list: readonly ZenTemplateInfo[]): ZenScope | null {
  if (!widget.startsWith('k2:')) return null
  const t = list.find((x) => x.needsGrant?.widget === widget && x.needsGrant.scope === 'local')
  return t ? ZEN_LOCAL_SCOPE : null
}
