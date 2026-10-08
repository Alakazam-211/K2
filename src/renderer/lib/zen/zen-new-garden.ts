// Rosson 2026-10-08 — New Garden is one modal: a name and a browsable list
// of the catalog defaults (Diary first, then the plain starts). Pick one,
// name it, Create. Nothing to allow: widgets in your own Gardens need no
// permissions.
//
// The modal outlives the menu that opened it (the Garden switcher's
// dropdown, a `menu` widget's list), so whether it is open lives here, per
// window, and `ZenNewGardenHost` (mounted once by `ZenPage`) draws it.

import { create } from 'zustand'
import type { ZenTemplateInfo } from './zen-custom-types'

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

/** The cards, in the order the modal shows them: the catalog (the Diary
 *  first, by the daemon's `order`), then the starts (default, empty). */
export function zenNewGardenCards(list: readonly ZenTemplateInfo[]): ZenTemplateInfo[] {
  return [...list.filter((t) => t.section === 'catalog'), ...list.filter((t) => t.section === 'start')]
}
