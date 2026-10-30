// "Save to stash" for every box a person types to an agent in (Rosson
// 2026-10-30: the phone's "+" menu everywhere). The Thread box and the
// terminal bar have their own wiring in TerminalComposeBar; the other boxes
// (the Zen Conversation widget, project chat, a ticket's discussion, a Grok
// Bot chat, the Inbox's "Chat about this") use these:
//   - `useComposeStash`: save a draft (text, then its files) to the stash,
//     with the source stamp; a toast says it landed or why it didn't;
//   - `useComposePlusMenu`: the "+" menu (an attach row when the box has
//     one, then "Save to stash" with its chord, dimmed while there is
//     nothing to save), opened above the "+";
//   - `isComposeStashChord`: Cmd+S / Ctrl+S, as in the Thread box.
// The caller clears its own box after a save (it knows its draft).

import { useCallback, useRef, useState } from 'react'
import { scopeForHost, type ServerScope } from '@/kessel/server-scope'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { LOCAL_HOME_HOST } from '@/lib/host-key'
import { isStashChord, readComposeFiles, stashChordLabel, stashSourceFor } from '@/lib/prompt-stash/compose'
import { orNull, type StashFileInput, type StashSource } from '@/lib/prompt-stash/record'
import { saveToStash } from '@/stores/prompt-stash'
import { clientToCssPx, useContextMenuStore } from '@/stores/context-menu'
import { useToastStore } from '@/stores/toast'
import { composePlusMenuHeight, composePlusMenuItems } from './PromptStash'

/** Cmd+S on macOS, Ctrl+S elsewhere (the Thread box's chord). */
export function isComposeStashChord(e: {
  key: string
  metaKey: boolean
  ctrlKey: boolean
  shiftKey: boolean
  altKey: boolean
  nativeEvent?: { isComposing?: boolean }
}): boolean {
  if (e.nativeEvent?.isComposing) return false
  return isStashChord(e, isMacPlatform())
}

export interface ComposeStashSave {
  text: string
  /** The draft's files, read when the save runs (a read that fails keeps
   *  the draft, never half-saved). */
  files?: () => Promise<StashFileInput[]>
}

export interface ComposeStash {
  /** "⌘S" / "Ctrl+S". */
  chord: string
  saving: boolean
  /** Save `draft`; true when it landed (the caller then clears its box). */
  save: (draft: ComposeStashSave) => Promise<boolean>
}

/** Files picked or dropped on this computer (paths here), read for the
 *  stash through this computer's own daemon. */
export function readLocalComposeFiles(paths: readonly string[]): Promise<StashFileInput[]> {
  return readComposeFiles(scopeForHost(LOCAL_HOME_HOST), paths)
}

/** The source stamp: `scope`'s server and the person on it; with no scope
 *  (a Zen widget, which holds none), only the agent. */
async function sourceOf(scope: ServerScope | null, agent: string | null, agentName: string | null): Promise<StashSource> {
  if (scope) return stashSourceFor(scope, agent, agentName)
  return { server_id: null, server_label: null, user: null, agent: orNull(agent), agent_name: orNull(agentName) }
}

/** Save drafts from a box to the stash, stamped with where they were
 *  written (`scope`'s server, the person, `agent`). */
export function useComposeStash({
  scope,
  agent,
  agentName,
}: {
  scope: ServerScope | null
  agent: string | null
  agentName?: string | null
}): ComposeStash {
  const [saving, setSaving] = useState(false)
  const busy = useRef(false)
  const save = useCallback(
    async (draft: ComposeStashSave): Promise<boolean> => {
      const text = draft.text.trim()
      if (busy.current) return false
      busy.current = true
      setSaving(true)
      try {
        const files = draft.files ? await draft.files() : []
        if (!text && files.length === 0) return false
        const r = await saveToStash({ text, source: await sourceOf(scope, agent, agentName ?? agent) }, files)
        if (r.status !== 'saved') return false
        useToastStore.getState().addToast('Saved to your stash', 'success', 2500)
        return true
      } catch (err) {
        useToastStore
          .getState()
          .addToast(`Couldn't stash: ${err instanceof Error ? err.message : String(err)}. Your draft is still here.`, 'error')
        return false
      } finally {
        busy.current = false
        setSaving(false)
      }
    },
    [agent, agentName, scope],
  )
  return { chord: stashChordLabel(isMacPlatform()), saving, save }
}

export interface ComposePlusAttach {
  onPick: () => void
  /** Off (dimmed) with `why` as its tooltip. */
  enabled?: boolean
  why?: string | null
}

/** The "+" menu for a box: open it above `anchor`. `menuOpen` lights the
 *  "+" while it is up. */
export function useComposePlusMenu(): {
  menuOpen: boolean
  open: (
    anchor: HTMLElement | null,
    opts: { attach?: ComposePlusAttach | null; canStash: boolean; chord: string; onStash: () => void },
  ) => void
} {
  const [menuOpen, setMenuOpen] = useState(false)
  const open = useCallback(
    (
      anchor: HTMLElement | null,
      opts: { attach?: ComposePlusAttach | null; canStash: boolean; chord: string; onStash: () => void },
    ) => {
      const items = composePlusMenuItems({ attach: !!opts.attach, canStash: opts.canStash, chord: opts.chord })
      const attachRow = items.find((i) => i.id === 'attach')
      if (attachRow && opts.attach && opts.attach.enabled === false) {
        attachRow.enabled = false
        if (opts.attach.why) attachRow.tooltip = opts.attach.why
      }
      const r = anchor?.getBoundingClientRect()
      const x = r ? clientToCssPx(r.left) : 0
      const y = r ? Math.max(4, clientToCssPx(r.top) - composePlusMenuHeight(items) - 4) : 0
      setMenuOpen(true)
      void useContextMenuStore
        .getState()
        .show(x, y, items)
        .then((picked) => {
          setMenuOpen(false)
          if (picked === 'attach') opts.attach?.onPick()
          else if (picked === 'stash') opts.onStash()
        })
    },
    [],
  )
  return { menuOpen, open }
}
