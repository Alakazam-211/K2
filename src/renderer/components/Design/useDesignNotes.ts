// Design tab notes tray (PRD C2, D-A2, D-A10): pin many notes, Send once.
//
// Per viewer and per Design item id. Never serialized into the shared
// layout (D-A2): two designers on two Design tabs each keep their own tray.
// In memory, so the tray survives the page reloading or navigating inside
// the same tab (dev servers reload on every save).
//
// Send rules:
//  - `delivered: true` → the sent notes leave the tray.
//  - `delivered: false` → the Thread item exists but the agent did not get
//    it yet; the notes stay and `pendingThreadItemId` is kept so Retry calls
//    redeliver on that item. Retry never posts a second Thread item.
//  - HTTP / transport error → nothing changes but the error line.

import { create } from 'zustand'
import type { ServerScope } from '@/kessel/server-scope'
import { buildNotesBody, redeliverDesignNotes, sendDesignNotes } from './designApi'
import type { DesignNoteDraft, NoteElement, NoteIntent } from './types'

/** Most notes one Design tab's tray holds: a 3 × 3 grid of tiles (Rosson
 *  2026-10-16). The limit is not shown; an Add past it is refused with
 *  NOTES_FULL_REASON. The daemon takes up to 20 per Send. */
export const MAX_DESIGN_NOTES = 9

export const NOTES_FULL_REASON = `You've hit the limit of ${MAX_DESIGN_NOTES} notes. Send some first.`

export interface DesignTray {
  notes: DesignNoteDraft[]
  sending: boolean
  error: string | null
  /** Stored but undelivered Thread item: Retry redelivers it. */
  pendingThreadItemId: string | null
  /** Note ids that the pending Thread item carries (cleared on delivery). */
  pendingNoteIds: string[]
}

export const EMPTY_TRAY: DesignTray = Object.freeze({
  notes: [],
  sending: false,
  error: null,
  pendingThreadItemId: null,
  pendingNoteIds: [],
}) as DesignTray

export interface NewNote {
  comment: string
  intent?: NoteIntent
  element?: NoteElement
  url: string
  title: string
  viewport?: { width: number; height: number }
}

export type AddResult = { ok: true; id: string } | { ok: false; reason: string }

export interface SendTarget {
  scope: ServerScope
  workspace: string
}

interface DesignNotesState {
  trays: Record<string, DesignTray>
  add(itemId: string, note: NewNote): AddResult
  edit(itemId: string, noteId: string, patch: Partial<Pick<DesignNoteDraft, 'comment' | 'intent'>>): void
  remove(itemId: string, noteId: string): void
  clear(itemId: string): void
  /** Close the tab: forget its tray. */
  drop(itemId: string): void
  send(itemId: string, target: SendTarget): Promise<void>
  /** Send now (⌘↩) in the note editor: just the typed note, straight
   *  away, as its own Send (its own details file). It sits in the tray
   *  while it sends (even a full one), so a failed Send keeps it. */
  sendNow(itemId: string, target: SendTarget, typed: NewNote): Promise<void>
  /** A tile's own Send: just this note (its own Thread item and details
   *  file); the tile leaves the tray once delivered. */
  sendOne(itemId: string, noteId: string, target: SendTarget): Promise<void>
  retry(itemId: string, target: SendTarget): Promise<void>
}

let seq = 0
function newNoteId(): string {
  seq += 1
  return `dn-${Date.now().toString(36)}-${seq}`
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

export const useDesignNotesStore = create<DesignNotesState>((set, get) => {
  const tray = (itemId: string): DesignTray => get().trays[itemId] ?? EMPTY_TRAY
  const patch = (itemId: string, p: Partial<DesignTray>): void =>
    set((s) => ({ trays: { ...s.trays, [itemId]: { ...(s.trays[itemId] ?? EMPTY_TRAY), ...p } } }))

  const settle = (itemId: string, sentIds: readonly string[], threadItemId: string, delivered: boolean): void => {
    if (delivered) {
      const gone = new Set(sentIds)
      const t = tray(itemId)
      patch(itemId, {
        notes: t.notes.filter((n) => !gone.has(n.id)),
        sending: false,
        error: null,
        pendingThreadItemId: null,
        pendingNoteIds: [],
      })
    } else {
      patch(itemId, {
        sending: false,
        error: 'Posted to the Thread, but the agent did not get it yet. Retry delivers the same notes.',
        pendingThreadItemId: threadItemId,
        pendingNoteIds: [...sentIds],
      })
    }
  }

  /** One Send of these notes: one Thread item, one details file. */
  const sendNotes = async (itemId: string, target: SendTarget, sent: readonly DesignNoteDraft[]): Promise<void> => {
    patch(itemId, { sending: true, error: null })
    try {
      const r = await sendDesignNotes(target.scope, buildNotesBody(target.workspace, sent, itemId))
      settle(itemId, sent.map((n) => n.id), r.threadItemId, r.delivered)
    } catch (e) {
      patch(itemId, { sending: false, error: errorText(e) })
    }
  }

  const addNote = (itemId: string, note: NewNote, cap: number): AddResult => {
    const t = tray(itemId)
    if (t.notes.length >= cap) return { ok: false, reason: NOTES_FULL_REASON }
    const comment = note.comment.trim()
    if (!comment) return { ok: false, reason: 'Write a note first.' }
    const id = newNoteId()
    const draft: DesignNoteDraft = {
      id,
      comment,
      intent: note.intent ?? 'change',
      url: note.url,
      title: note.title,
      ...(note.element ? { element: note.element } : {}),
      ...(note.viewport ? { viewport: note.viewport } : {}),
    }
    patch(itemId, { notes: [...t.notes, draft] })
    return { ok: true, id }
  }

  return {
    trays: {},

    add(itemId, note) {
      return addNote(itemId, note, MAX_DESIGN_NOTES)
    },

    async sendNow(itemId, target, typed) {
      if (!typed.comment.trim() || tray(itemId).sending) return
      const res = addNote(itemId, typed, MAX_DESIGN_NOTES + 1)
      if (!res.ok) {
        patch(itemId, { error: res.reason })
        return
      }
      return get().sendOne(itemId, res.id, target)
    },

    edit(itemId, noteId, p) {
      const t = tray(itemId)
      // A note already inside an undelivered Thread item is fixed text.
      if (t.pendingNoteIds.includes(noteId)) return
      patch(itemId, {
        notes: t.notes.map((n) =>
          n.id === noteId
            ? { ...n, ...(p.comment !== undefined ? { comment: p.comment } : {}), ...(p.intent ? { intent: p.intent } : {}) }
            : n,
        ),
      })
    },

    remove(itemId, noteId) {
      const t = tray(itemId)
      patch(itemId, { notes: t.notes.filter((n) => n.id !== noteId) })
    },

    clear(itemId) {
      patch(itemId, { notes: [], error: null, pendingThreadItemId: null, pendingNoteIds: [] })
    },

    drop(itemId) {
      set((s) => {
        const { [itemId]: _gone, ...rest } = s.trays
        return { trays: rest }
      })
    },

    async send(itemId, target) {
      const t = tray(itemId)
      if (t.sending || t.notes.length === 0) return
      // An undelivered Thread item is still waiting: Send means Retry.
      if (t.pendingThreadItemId) return get().retry(itemId, target)
      return sendNotes(itemId, target, t.notes)
    },

    async sendOne(itemId, noteId, target) {
      const t = tray(itemId)
      if (t.sending) return
      if (t.pendingThreadItemId) return get().retry(itemId, target)
      const note = t.notes.find((n) => n.id === noteId)
      if (!note) return
      return sendNotes(itemId, target, [note])
    },

    async retry(itemId, target) {
      const t = tray(itemId)
      if (t.sending || !t.pendingThreadItemId) return
      const threadItemId = t.pendingThreadItemId
      const ids = t.pendingNoteIds
      patch(itemId, { sending: true, error: null })
      try {
        const r = await redeliverDesignNotes(target.scope, target.workspace, threadItemId)
        settle(itemId, ids, r.threadItemId, r.delivered)
      } catch (e) {
        patch(itemId, { sending: false, error: errorText(e) })
      }
    },
  }
})

/** This Design item's tray (stable empty tray when it has none). */
export function useDesignTray(itemId: string): DesignTray {
  return useDesignNotesStore((s) => s.trays[itemId] ?? EMPTY_TRAY)
}

/** Badge numbers follow tray order (1-based), only for picked elements. */
export function badgesFor(tray: DesignTray): { n: number; selector: string }[] {
  return tray.notes.flatMap((n, i) => (n.element?.selector ? [{ n: i + 1, selector: n.element.selector }] : []))
}
