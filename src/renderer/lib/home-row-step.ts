// Step through the Home nav's rows from the keyboard (Rosson 2026-11-16):
//
//   ⌥⌘↑ the row above the selected one · ⌥⌘↓ the row below   (macOS)
//   Ctrl+Alt+Up / Ctrl+Alt+Down                              (Linux / Windows)
//
// Rebindable: HOTKEYS ids `homeRowUp` / `homeRowDown` (`@shared/hotkeys`),
// listed in Settings → Key Bindings → Navigation; the handler reads the
// effective binding on every keydown. No native menu accelerator, as with
// the page chords: a fixed accelerator would keep the keys after a rebind.
//
// What a step does:
//   - Order is the nav's own (`displayRows`: pins first, then manual order
//     or most recent first), the same rows the nav and the collapsed rail
//     draw for the selected Home group. Other groups are not visited.
//   - "Selected" is what the rows paint as selected (`homeRowOnScreen`).
//     With none (or a selection from another group), ↓ takes the first row
//     and ↑ the last.
//   - The next row is opened as ⌘1–9 opens one: `homeRowOpenableNow` then
//     `openHomeRow`, the click's path. A row whose click does nothing (No
//     access, gone, another server on the web) is stepped over.
//   - The ends stop; there is no wrap (as ⌥⌘← / ⌥⌘→ on the tab strip).
//   - The row scrolls into view in the nav (or the rail, collapsed).
//
// Where the keys stay with what has focus:
//   - a code editor (CodeMirror's default keymap: Mod-Alt-ArrowUp/Down add
//     a cursor above / below, ⌥⌘↑ on macOS and Ctrl+Alt+Up elsewhere);
//   - a dialog (its own list may read arrows).
// Text fields (the Thread compose box) and the terminal step: neither uses
// these keys for editing, and the listener runs in the capture phase so a
// terminal never also sends `ESC[1;11A` to the agent behind it.
//
// Clear of the chords already on arrows: ⌥⌘← / ⌥⌘→ previous / next tab
// (useTerminalShortcuts), ⌥↑ / ⌥↓ next change in a diff (no ⌘), ⌥↑ / ⌥↓
// move a Zen Agents row or a Garden (no ⌘).

import { effectiveKeybinding, keyEventMatchesCombo, type KeyComboEventLike } from '@shared/hotkeys'
import { useHomesStore, selectedHome, type HomeRow } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { useHomeRoomsStore } from '@/stores/home-rooms'
import { useHomeChatStore } from '@/stores/home-chat'
import { useGrokBotsStore } from '@/stores/grokbots'
import { displayRows } from '@/lib/home-row-order'
import { activeHomeHostKey, findWorkspaceForRow, parseHomeAddress } from '@/lib/home-address'
import { remoteRoomsPreviewEnabled } from '@/lib/remote-rooms-preview'
import { openHomeRow } from '@/lib/home-open'
import { homeRowOpenableNow } from '@/components/Home/home-room'
import { homeRowOnScreen, type OnScreen } from '@/components/Home/home-row-on-screen'
import { isHomeChatRow } from '@/lib/home-chat/address'
import { grokBotIdFromAddress } from '@/lib/grokbots'

/** -1 the row above, 1 the row below. */
export type HomeRowStep = -1 | 1

/** HOTKEYS ids, by direction. */
export const HOME_ROW_STEP_IDS = { up: 'homeRowUp', down: 'homeRowDown' } as const

/** The step a keydown stands for (the effective bindings), or null. */
export function homeRowStepForEvent(
  e: KeyComboEventLike,
  keybindings: Record<string, string> | null | undefined,
  isMac: boolean,
): HomeRowStep | null {
  if (keyEventMatchesCombo(e, effectiveKeybinding(keybindings, HOME_ROW_STEP_IDS.up, isMac))) return -1
  if (keyEventMatchesCombo(e, effectiveKeybinding(keybindings, HOME_ROW_STEP_IDS.down, isMac))) return 1
  return null
}

/** Does the focused element keep these keys? A code editor (add a cursor
 *  above / below) or anything inside a dialog. */
export function homeRowStepKeptBy(target: EventTarget | null): boolean {
  if (!target || typeof (target as Element).closest !== 'function') return false
  const el = target as Element
  return el.closest('.cm-editor') !== null || el.closest('[role="dialog"], [role="alertdialog"]') !== null
}

/**
 * The row index a step lands on: from `current` (null = nothing selected:
 * ↓ the first row, ↑ the last) in `step`'s direction, the first row
 * `canOpen` allows. Null at the ends (no wrap) or with no such row.
 */
export function nextHomeRowIndex(
  count: number,
  current: number | null,
  step: HomeRowStep,
  canOpen: (index: number) => boolean,
): number | null {
  let i = current === null ? (step === 1 ? 0 : count - 1) : current + step
  for (; i >= 0 && i < count; i += step) {
    if (canOpen(i)) return i
  }
  return null
}

/** The selected row's index in `rows`, as the rows paint it, or null. A
 *  person, channel or Grok Bot chat covers the main area, so its row wins
 *  over a workspace row behind it. */
export function selectedHomeRowIndex(
  rows: readonly HomeRow[],
  now: OnScreen,
  workspaceFor: (row: HomeRow) => { id: string } | null,
): number | null {
  let first: number | null = null
  for (let i = 0; i < rows.length; i++) {
    const row = rows[i]
    if (!homeRowOnScreen(row, workspaceFor(row), now)) continue
    if (isHomeChatRow(row) || grokBotIdFromAddress(row.address) !== null) return i
    if (first === null) first = i
  }
  return first
}

/** What is on screen now, read from the stores (no render). */
export function onScreenNow(): OnScreen {
  return {
    activeProjectId: useProjectsStore.getState().activeProjectId,
    shown: useHomeRoomsStore.getState().shown,
    preview: remoteRoomsPreviewEnabled(),
    chat: useHomeChatStore.getState().open?.address ?? null,
    grok: useGrokBotsStore.getState().open,
  }
}

/** A row's workspace on the connected server (as the nav finds it), or null. */
function connectedWorkspaceFor(row: HomeRow): { id: string } | null {
  const host = useConnectHostStore.getState()
  if (host.connectionStatus !== 'connected') return null
  if (parseHomeAddress(row.address)?.host !== activeHomeHostKey(host.activeHost)) return null
  return findWorkspaceForRow(useProjectsStore.getState().projects, row)
}

/** Bring a row into view: the nav's row, else the collapsed rail's icon. */
export function scrollHomeRowIntoView(address: string, doc: Document = document): void {
  const match = (attr: string): Element | null =>
    Array.from(doc.querySelectorAll(`[${attr}]`)).find((el) => el.getAttribute(attr) === address) ?? null
  const el = match('data-home-row') ?? match('data-home-rail-row')
  el?.scrollIntoView?.({ block: 'nearest' })
}

/** One step in the selected Home group. Returns the row opened, or null
 *  (an end, an empty group, or no row that opens). */
export function stepHomeRow(step: HomeRowStep): HomeRow | null {
  const rows = displayRows(selectedHome(useHomesStore.getState()))
  if (rows.length === 0) return null
  const current = selectedHomeRowIndex(rows, onScreenNow(), connectedWorkspaceFor)
  const next = nextHomeRowIndex(rows.length, current, step, (i) => homeRowOpenableNow(rows[i]))
  if (next === null) return null
  const row = rows[next]
  openHomeRow(row)
  scrollHomeRowIntoView(row.address)
  return row
}
