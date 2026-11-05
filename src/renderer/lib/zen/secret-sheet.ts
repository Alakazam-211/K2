// Secret cards in a custom Garden (catalog v50): a secret value never passes
// through Garden code. A widget sees a secret card as `{status, expiresAt}`
// (`zen-custom-projection`), and `thread.answer(address, cardId)` on one
// opens K2's own sheet OUTSIDE the frame (the permission-card pattern,
// `permission-sheet.ts`). The sheet draws the compact SecretCard that K2's
// Conversation widget draws, and the person's Save goes through the
// window's own answer path with no widget origin, exactly like K2's Thread.
//
// - Only right after the person pressed in the widget (a press: the frame
//   having the focus isn't enough), one sheet at a time per frame
//   (`rate_limited` otherwise), and only while the card is pending.
// - The widget only hears that the sheet closed (`null`); the card's new
//   status reaches it as the Thread's next view. Never the value.
// - The widget's own answers stay refused: a widget never sends a value
//   (UW50), and the daemon refuses a widget-origin answer that carries a
//   secret.

import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'

/** Is this item a secret card K2's sheet can draw? */
export function isZenSecretItem(item: OverlayThreadItem | null | undefined): boolean {
  return !!item && item.doc.kind === 'secret' && !!item.doc.secret
}

/** What K2's sheet is asked to show. */
export interface ZenSecretAsk {
  address: string
  cardId: string
}

export interface ZenSecretSheetDeps {
  /** The item `cardId` on `address`'s conversation now, or null. */
  find(address: string, cardId: string): OverlayThreadItem | null
  /** Show K2's sheet; resolves when the person closes it. */
  show(ask: ZenSecretAsk): Promise<void>
  /** Did the person press in this frame within the last 5 s? */
  pressed(): boolean
}

export type ZenSecretOpen = { ok: true } | { ok: false; code: 'rate_limited' | 'failed'; message: string }

export interface ZenSecretSheet {
  /** `cardId` on `address` is a secret card. */
  handles(address: string, cardId: string): boolean
  /** Open K2's sheet for it; resolves when the person closed it. */
  open(address: string, cardId: string): Promise<ZenSecretOpen>
}

export function createZenSecretSheet(deps: ZenSecretSheetDeps): ZenSecretSheet {
  let open = false
  return {
    handles: (address, cardId) => isZenSecretItem(deps.find(address, cardId)),
    async open(address, cardId) {
      const item = deps.find(address, cardId)
      if (!isZenSecretItem(item)) return { ok: false, code: 'failed', message: 'That isn’t a secret card.' }
      if (item!.doc.secret!.status !== 'pending') {
        return { ok: false, code: 'failed', message: 'That secret card isn’t waiting for an answer any more.' }
      }
      if (!deps.pressed()) {
        return { ok: false, code: 'failed', message: 'K2 opens a secret card only just after the person clicks in the widget.' }
      }
      if (open) return { ok: false, code: 'rate_limited', message: 'K2 is already showing a secret card.' }
      open = true
      try {
        await deps.show({ address, cardId })
      } finally {
        open = false
      }
      return { ok: true }
    },
  }
}
