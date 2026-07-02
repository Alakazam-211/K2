// useRetainedChatStore — session state for pinned-chat background
// retention (design: .k2/notes/pinned-chat-background-render-design.md).
//
// Holds the three inputs the per-window PinnedChatRetainer derives its
// render from:
//   - `mruOrder`   — visit order (most-recently-foregrounded first),
//                    fed by PinnedChatSlot mounts + boot seeding, pruned
//                    on Active-section leave. The retained set itself is
//                    NEVER stored — it's `computeRetainedSet(...)` over
//                    this order (kessel-term/retainedChat.ts).
//   - `entries`    — the AgentChatPane props for each visited workspace
//                    (projectPath/agentName/restoredSessionId), so the
//                    retainer can render an instance for a workspace
//                    whose tab tree is currently unmounted.
//   - `slots`      — the live portal targets: the foreground workspace's
//                    pinned-chat pane slot registers its element +
//                    visibility here; the retainer re-parents the
//                    retained instance's container into it (or into the
//                    off-screen hidden host when absent).
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

interface RetainedChatState {
  entries: ReadonlyMap<string, RetainedChatEntry>
  mruOrder: readonly string[]
  slots: ReadonlyMap<string, RetainedChatSlot>
  /** Eager-boot seeding ran for this host session (one-shot). */
  bootSeeded: boolean

  /** A workspace's pinned chat was foregrounded (its slot mounted).
   *  Upserts the entry props and moves the workspace to MRU front. */
  recordVisit: (entry: RetainedChatEntry) => void
  /** One-shot eager-boot seeding: append Active-list-ordered entries
   *  behind any real visits, bounded by `cap` (retainedCap at the call
   *  site). No-ops after the first call for this host session. */
  seedBoot: (entries: RetainedChatEntry[], cap: number) => void
  /** Active-section membership tracking: drop visits for workspaces no
   *  longer in the canonical Active set (leave ⇒ evict; a later re-join
   *  does not auto-attach). */
  pruneToActive: (activeProjectIds: ReadonlySet<string>) => void
  /** Hard-evict one workspace (daemon session removed while
   *  backgrounded). Re-visiting re-attaches fresh. */
  evict: (projectId: string) => void

  registerSlot: (projectId: string, el: HTMLElement) => void
  setSlotVisible: (projectId: string, visible: boolean) => void
  /** Unregisters ONLY if `el` is still the registered element — guards
   *  the unmount(old)/mount(new) interleave across slot remounts. */
  unregisterSlot: (projectId: string, el: HTMLElement) => void
}

export const useRetainedChatStore = create<RetainedChatState>((set, get) => ({
  entries: new Map(),
  mruOrder: [],
  slots: new Map(),
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
    set((s) => {
      const nextEntries = new Map(s.entries)
      for (const e of entries) {
        // Seeds never overwrite a real visit's entry props.
        if (!nextEntries.has(e.projectId)) nextEntries.set(e.projectId, e)
      }
      return {
        bootSeeded: true,
        entries: nextEntries,
        mruOrder: seedBootOrder(
          s.mruOrder,
          entries.map((e) => e.projectId),
          cap,
        ),
      }
    })
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

  registerSlot: (projectId, el) => {
    set((s) => {
      const slots = new Map(s.slots)
      slots.set(projectId, { el, visible: false })
      return { slots }
    })
  },

  setSlotVisible: (projectId, visible) => {
    set((s) => {
      const slot = s.slots.get(projectId)
      if (!slot || slot.visible === visible) return s
      const slots = new Map(s.slots)
      slots.set(projectId, { el: slot.el, visible })
      return { slots }
    })
  },

  unregisterSlot: (projectId, el) => {
    set((s) => {
      const slot = s.slots.get(projectId)
      if (!slot || slot.el !== el) return s
      const slots = new Map(s.slots)
      slots.delete(projectId)
      return { slots }
    })
  },
}))

/** Reset to a clean session (host switch / tests). */
export function resetRetainedChatStore(): void {
  useRetainedChatStore.setState({
    entries: new Map(),
    mruOrder: [],
    slots: new Map(),
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
