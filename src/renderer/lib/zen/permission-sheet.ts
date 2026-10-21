// Permission cards v1 in a Garden (prd-agent-settings-cards-v1 §8): the
// `k2-thread@1` kit draws the card from the bridge's `cards=2` view, and a
// person answers it on K2's own sheet OUTSIDE the frame (the RS-A15 / G8
// pattern). Widget code can't answer a permission card, write its note or
// undo it: a click inside widget code isn't proof of a person.
//
// - `projectZenPermission`: what a widget may read of the card (a closed
//   shape, words written by K2): the headline, the setting, how long, the
//   agent's words, the line for this viewer's state, and the label of the
//   one button the kit draws. Never the values, the notes, the approvers'
//   names, request ids or the Settings page.
// - `createZenPermissionSheet`: the bridge's `thread.answer` on a
//   permission card opens K2's sheet instead of answering. Only right after
//   the person pressed in the widget (or while it has the focus), one sheet
//   at a time per frame (`rate_limited` otherwise). The widget only hears
//   that the sheet closed (`null`); the card's new state reaches it as the
//   Thread's next view.

import type { OverlayChoice, OverlayThreadItem } from '@/components/SessionView/overlayThread'
import {
  permissionAgent,
  stateLine,
  waitingLine,
  type PermissionCardData,
  type PermissionItem,
  type PermissionViewer,
} from '@/components/SessionView/permissionCardView'

/** The card as a widget gets it (`doc.choice.permission`). */
export interface ZenWidgetPermission {
  kind: 'permission' | 'batch'
  /** `view.state` (a batch: `pending` or `done`). */
  state: string
  /** `approve` (this person can answer) or `wait`. */
  mode: 'approve' | 'wait'
  /** K2's sentence (a batch: "billing asks for 3 permissions."). */
  headline: string
  setting: string
  direction: 'loosen' | 'tighten'
  duration: string
  /** "<agent> says: …", or null. */
  why: string | null
  /** The waiting text, or the state line once it moved on; null while it
   *  waits on this person. */
  line: string | null
  /** The one button the kit draws (it opens K2's sheet), or null. */
  button: string | null
  /** How many requests (1 for a single card). */
  count: number
}

const cut = (s: string, n: number): string => (s.length > n ? `${s.slice(0, n - 1)}…` : s)

function lineFor(item: PermissionItem, viewer: PermissionViewer, agent: string, result: string | null): string | null {
  if (item.state === 'pending') return viewer.mode === 'approve' && viewer.canAnswer ? null : waitingLine(item, viewer, agent)
  return stateLine(item, viewer, agent, result)
}

function buttonFor(item: PermissionItem, viewer: PermissionViewer, nowSec: number): string | null {
  const approver = viewer.mode === 'approve' && viewer.canAnswer
  if (item.state === 'pending') {
    if (approver) return 'Review and answer'
    return item.noteBox.allowed ? 'Leave a note' : null
  }
  if (item.state === 'already_done' && !item.answer && approver) return 'Review'
  if (item.state === 'approved' && approver && item.undo && item.undo.until > nowSec) return 'Review'
  return null
}

/** The widget's copy of a permission card (closed shape, K2's words). */
export function projectZenPermission(p: PermissionCardData, from: string, nowSec: number): ZenWidgetPermission {
  const result = p.result?.message || null
  if (p.view.kind === 'batch') {
    const v = p.view
    const agent = v.items.find((i) => i.why)?.why?.by || from || 'The agent'
    const approver = v.viewer.mode === 'approve' && v.viewer.canAnswer
    const pending = v.items.filter((i) => i.state === 'pending').length
    const first = v.items[0]
    return {
      kind: 'batch',
      state: v.state,
      mode: v.viewer.mode,
      headline: cut(`${agent} asks for ${v.items.length} permissions.`, 300),
      setting: cut(v.items.map((i) => i.setting).join(', '), 300),
      direction: v.items.some((i) => i.direction === 'loosen') ? 'loosen' : 'tighten',
      duration: first ? cut(first.duration, 120) : '',
      why: first?.why ? cut(`${first.why.by} says: ${first.why.text}`, 300) : null,
      line:
        v.state === 'done'
          ? `${v.items.length - pending} of ${v.items.length} answered`
          : approver
            ? null
            : first
              ? waitingLine(first, v.viewer, agent)
              : null,
      button: v.state === 'pending' && approver && pending > 0 ? 'Review and answer' : null,
      count: v.items.length,
    }
  }
  const v = p.view
  const agent = permissionAgent(v, from)
  return {
    kind: 'permission',
    state: v.state,
    mode: v.viewer.mode,
    headline: cut(v.whatChanges || v.setting, 300),
    setting: cut(v.setting, 120),
    direction: v.direction,
    duration: cut(v.duration, 120),
    why: v.why ? cut(`${v.why.by} says: ${v.why.text}`, 300) : null,
    line: lineFor(v, v.viewer, agent, result),
    button: buttonFor(v, v.viewer, nowSec),
    count: 1,
  }
}

/** `permission` on a widget's choice, when the card is one. */
export function zenPermissionField(c: OverlayChoice, from: string, nowSec: number): { permission?: ZenWidgetPermission } {
  return c.permission ? { permission: projectZenPermission(c.permission, from, nowSec) } : {}
}

/** Is this item a permission card the sheet can draw? */
export function isZenPermissionItem(item: OverlayThreadItem | null | undefined): boolean {
  return !!item && item.doc.kind === 'choice' && !!item.doc.choice?.permission
}

/** What K2's sheet is asked to show. */
export interface ZenPermissionAsk {
  address: string
  cardId: string
}

export interface ZenPermissionSheetDeps {
  /** The item `cardId` on `address`'s conversation now, or null. */
  find(address: string, cardId: string): OverlayThreadItem | null
  /** Show K2's sheet; resolves when the person closes it. */
  show(ask: ZenPermissionAsk): Promise<void>
  /** Did the person just press in this frame (or does it have the focus)? */
  gesture(): boolean
}

export type ZenPermissionOpen = { ok: true } | { ok: false; code: 'rate_limited' | 'failed'; message: string }

export interface ZenPermissionSheet {
  /** `cardId` on `address` is a permission card (with a `cards=2` view). */
  handles(address: string, cardId: string): boolean
  /** Open K2's sheet for it; resolves when the person closed it. */
  open(address: string, cardId: string): Promise<ZenPermissionOpen>
}

export function createZenPermissionSheet(deps: ZenPermissionSheetDeps): ZenPermissionSheet {
  let open = false
  return {
    handles: (address, cardId) => isZenPermissionItem(deps.find(address, cardId)),
    async open(address, cardId) {
      if (!isZenPermissionItem(deps.find(address, cardId))) {
        return { ok: false, code: 'failed', message: 'That isn’t a permission card.' }
      }
      if (!deps.gesture()) {
        return { ok: false, code: 'failed', message: 'K2 opens a permission card only when the person clicks in the widget.' }
      }
      if (open) return { ok: false, code: 'rate_limited', message: 'K2 is already showing a permission card.' }
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
