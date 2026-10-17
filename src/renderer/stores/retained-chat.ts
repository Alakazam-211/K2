// useRetainedChatStore — session state for pinned-chat background
// retention (design: .k2/notes/pinned-chat-background-render-design.md).
//
// Holds the three inputs the per-window PinnedChatRetainer derives its
// render from:
//   - `mruOrder`   — visit order (most-recently-foregrounded first),
//                    fed by PinnedChatSlot mounts (real visits), pruned
//                    on Active-section leave. seedBoot may record
//                    display order but does NOT attach panes. The
//                    retained set itself is NEVER stored — it's
//                    `computeRetainedSet(...)` over this order
//                    (kessel-term/retainedChat.ts).
//   - `entries`    — the AgentChatPane props for each visited workspace
//                    (projectPath/agentName/restoredSessionId), so the
//                    retainer can render an instance for a workspace
//                    whose tab tree is currently unmounted.
//   - `slots`      — the live portal targets: the foreground workspace's
//                    pinned-chat pane slot registers its element +
//                    visibility here; the retainer re-parents the
//                    retained instance's container into it (or into the
//                    off-screen hidden host when absent).
//                    A workspace can have MORE THAN ONE slot (the Chat tab,
//                    plus a Design tab's Chat column — prd-browser-design-
//                    mode-v1 D-A7). `slotRecords` keeps every one; `slots`
//                    is the one the instance lives in (`resolveSlot`): a
//                    visible Chat tab wins, then a visible Design slot,
//                    then the (hidden) Chat tab. One instance, never two
//                    AgentChatPanes.
//
// Module-level zustand store: it deliberately SURVIVES the
// `<App key={hostKey}>` remount so the retainer can be unmounted/
// remounted by layout-mode switches without dropping state — the HOST
// boundary is enforced explicitly via `onActiveHostChange` (background
// attachments never span hosts; the old host's sockets are unreachable
// anyway). Same pattern as ActiveBar's `_activeBarMemory`.

import { create } from 'zustand'
import { onActiveHostChange } from '@/stores/connect-host'
import {
  pruneOrderToActive,
  recordVisitOrder,
  seedBootOrder,
} from '@/kessel-term/retainedChat'

export interface RetainedChatEntry {
  projectId: string
  projectPath: string
  agentName: string
  restoredSessionId?: string
}

export interface RetainedChatSlot {
  el: HTMLElement
  /** The slot's tab visibility (chat tab is display:block in the
   *  foreground workspace). Drives the portal'd pane's
   *  TabVisibilityContext. */
  visible: boolean
}

/** Who owns a slot: the pinned Chat tab, or a Design tab's Chat column. */
export type RetainedChatSlotKind = 'chat' | 'design'

export interface RetainedChatSlotRecord extends RetainedChatSlot {
  kind: RetainedChatSlotKind
}

/** The slot the retained instance lives in, newest registration first
 *  within a rule: a visible Chat tab, else any visible slot (a Design
 *  tab's column), else the Chat tab (hidden), else any slot. */
export function resolveSlot(records: readonly RetainedChatSlotRecord[]): RetainedChatSlot | null {
  const pick = (pred: (r: RetainedChatSlotRecord) => boolean): RetainedChatSlotRecord | null => {
    for (let i = records.length - 1; i >= 0; i--) if (pred(records[i])) return records[i]
    return null
  }
  const r =
    pick((x) => x.kind === 'chat' && x.visible) ??
    pick((x) => x.visible) ??
    pick((x) => x.kind === 'chat') ??
    pick(() => true)
  return r ? { el: r.el, visible: r.visible } : null
}

/** True while this workspace's pinned Chat TAB is visible in this window
 *  (a Design tab then shows the page only, D-A7). */
export function chatTabSlotVisible(state: Pick<RetainedChatState, 'slotRecords'>, projectId: string): boolean {
  return (state.slotRecords.get(projectId) ?? []).some((r) => r.kind === 'chat' && r.visible)
}

interface RetainedChatState {
  entries: ReadonlyMap<string, RetainedChatEntry>
  mruOrder: readonly string[]
  /** The slot each workspace's instance lives in (`resolveSlot` over
   *  `slotRecords`). */
  slots: ReadonlyMap<string, RetainedChatSlot>
  /** Every registered slot per workspace, in registration order. */
  slotRecords: ReadonlyMap<string, readonly RetainedChatSlotRecord[]>
  /** Display-only boot seeding ran for this host session (one-shot). */
  bootSeeded: boolean

  /** A workspace's pinned chat was foregrounded (its slot mounted).
   *  Upserts the entry props and moves the workspace to MRU front. */
  recordVisit: (entry: RetainedChatEntry) => void
  /** One-shot display-only boot seeding: records Active-list order
   *  behind any real visits, bounded by `cap`. Does NOT upsert pane
   *  entries — those would mount AgentChatPane → ensure-pinned-chat.
   *  No-ops after the first call for this host session. */
  seedBoot: (entries: RetainedChatEntry[], cap: number) => void
  /** Active-section membership tracking: drop visits for workspaces no
   *  longer in the canonical Active set (leave ⇒ evict; a later re-join
   *  does not auto-attach — only a fresh visit does). */
  pruneToActive: (activeProjectIds: ReadonlySet<string>) => void
  /** Hard-evict one workspace (daemon session removed while
   *  backgrounded). Re-visiting re-attaches fresh. */
  evict: (projectId: string) => void

  /** `kind` defaults to the Chat tab's slot. */
  registerSlot: (projectId: string, el: HTMLElement, kind?: RetainedChatSlotKind) => void
  /** `el` names the slot; without it, the workspace's Chat tab slot(s). */
  setSlotVisible: (projectId: string, visible: boolean, el?: HTMLElement) => void
  /** Unregisters ONLY that element's slot — guards the unmount(old)/
   *  mount(new) interleave across slot remounts. */
  unregisterSlot: (projectId: string, el: HTMLElement) => void
}

/** `slotRecords` with `projectId`'s list replaced, and `slots` re-resolved. */
function withSlotRecords(
  s: Pick<RetainedChatState, 'slots' | 'slotRecords'>,
  projectId: string,
  records: readonly RetainedChatSlotRecord[],
): Pick<RetainedChatState, 'slots' | 'slotRecords'> {
  const slotRecords = new Map(s.slotRecords)
  const slots = new Map(s.slots)
  if (records.length === 0) {
    slotRecords.delete(projectId)
    slots.delete(projectId)
  } else {
    slotRecords.set(projectId, records)
    const resolved = resolveSlot(records)
    const prev = s.slots.get(projectId)
    if (resolved && prev && prev.el === resolved.el && prev.visible === resolved.visible) {
      // Same slot as before: keep its identity.
    } else if (resolved) {
      slots.set(projectId, resolved)
    }
  }
  return { slotRecords, slots }
}

export const useRetainedChatStore = create<RetainedChatState>((set, get) => ({
  entries: new Map(),
  mruOrder: [],
  slots: new Map(),
  slotRecords: new Map(),
  bootSeeded: false,

  recordVisit: (entry) => {
    set((s) => {
      const prev = s.entries.get(entry.projectId)
      const entryUnchanged =
        prev !== undefined &&
        prev.projectPath === entry.projectPath &&
        prev.agentName === entry.agentName &&
        prev.restoredSessionId === entry.restoredSessionId
      const orderUnchanged = s.mruOrder[0] === entry.projectId
      if (entryUnchanged && orderUnchanged) return s
      const entries = new Map(s.entries)
      entries.set(entry.projectId, entry)
      return {
        entries,
        mruOrder: orderUnchanged
          ? s.mruOrder
          : recordVisitOrder(s.mruOrder, entry.projectId),
      }
    })
  },

  seedBoot: (entries, cap) => {
    if (get().bootSeeded) return
    set((s) => ({
      bootSeeded: true,
      // Display-only: do not upsert entries. Entries are what the
      // retainer mounts as AgentChatPane → ensure-pinned-chat. A seed
      // is not a visit (prd-active-window-wake-and-reap-v1 §3.4).
      mruOrder: seedBootOrder(
        s.mruOrder,
        entries.map((e) => e.projectId),
        cap,
      ),
    }))
  },

  pruneToActive: (activeProjectIds) => {
    set((s) => {
      const pruned = pruneOrderToActive(s.mruOrder, activeProjectIds)
      if (pruned.length === s.mruOrder.length) return s
      const entries = new Map(s.entries)
      for (const id of s.mruOrder) {
        if (!activeProjectIds.has(id)) entries.delete(id)
      }
      return { mruOrder: pruned, entries }
    })
  },

  evict: (projectId) => {
    set((s) => {
      if (!s.mruOrder.includes(projectId) && !s.entries.has(projectId)) return s
      const entries = new Map(s.entries)
      entries.delete(projectId)
      return {
        entries,
        mruOrder: s.mruOrder.filter((id) => id !== projectId),
      }
    })
  },

  registerSlot: (projectId, el, kind = 'chat') => {
    set((s) => {
      const records = (s.slotRecords.get(projectId) ?? []).filter((r) => r.el !== el)
      return withSlotRecords(s, projectId, [...records, { el, visible: false, kind }])
    })
  },

  setSlotVisible: (projectId, visible, el) => {
    set((s) => {
      const records = s.slotRecords.get(projectId)
      if (!records) return s
      const hit = (r: RetainedChatSlotRecord): boolean => (el ? r.el === el : r.kind === 'chat')
      if (!records.some((r) => hit(r) && r.visible !== visible)) return s
      return withSlotRecords(
        s,
        projectId,
        records.map((r) => (hit(r) ? { ...r, visible } : r)),
      )
    })
  },

  unregisterSlot: (projectId, el) => {
    set((s) => {
      const records = s.slotRecords.get(projectId)
      if (!records || !records.some((r) => r.el === el)) return s
      return withSlotRecords(
        s,
        projectId,
        records.filter((r) => r.el !== el),
      )
    })
  },
}))

/** Reset to a clean session (host switch / tests). */
export function resetRetainedChatStore(): void {
  useRetainedChatStore.setState({
    entries: new Map(),
    mruOrder: [],
    slots: new Map(),
    slotRecords: new Map(),
    bootSeeded: false,
  })
}

// Host boundary: background attachments never span hosts. Fires only on
// a real active-host change, AFTER `activeHost` flipped (the `<App
// key={hostKey}>` remount tears the instances down; this clears the
// module-level memory so the new host starts cold and re-seeds).
onActiveHostChange(() => {
  resetRetainedChatStore()
})
