// Pin-to-size menu — shared helper (S7c, PRD presence-multiplayer §5.5)
//
// S7b built this menu inside PaneTabBar, but PaneTabBar only renders
// when a tab HAS SPLITS. Single-pane tabs — the common case, including
// the pinned agent chat — render via the workspace-level TabBar. This
// module extracts the item→sessionId resolution, the menu construction
// and the pin/unpin action so both tab bars offer the identical menu.
//
// Split of responsibilities:
// - resolvePinSessionId / buildPinSizeMenuItems are PURE (unit-tested).
// - openPinSizeMenu drives the context-menu store + the daemon POST.
// - The transient error hint stays COMPONENT-LOCAL: callers pass
//   `onError` and render the message however their bar renders hints.

import { usePinnedSizeStore, type PinnedSize } from '@/stores/pinned-size'
import { useContextMenuStore, type ContextMenuItemDef } from '@/stores/context-menu'
import { agentChatId } from '@/lib/terminal-id'
import { daemonCliPost } from '@/lib/daemon-cli'

// ── Types ────────────────────────────────────────────────────────────────

/** The preset grid sizes offered by the pin-size menu. */
export const PIN_PRESETS: ReadonlyArray<{ cols: number; rows: number }> = [
  { cols: 80, rows: 24 },
  { cols: 100, rows: 30 },
  { cols: 120, rows: 36 },
  { cols: 160, rows: 48 },
]

/** What the daemon's /cli/terminal/pin-size answers with. */
interface PinSizeResponse {
  success: boolean
  pinned: { cols: number; rows: number; setBy?: string | null } | null
  persisted: boolean
}

/** Structural view of a tab item — matches both PaneTabBar's local
 *  Item shape and the tabs store's `Item` without importing either. */
export interface PinnableItem {
  type: 'terminal' | 'file-viewer' | 'agent'
  data: unknown
}

// ── Resolution (pure) ────────────────────────────────────────────────────

/** Daemon session UUID for a tab item, or null when the item has no
 *  live Kessel session (file viewers, work boards, legacy panes,
 *  not-yet-spawned terminals). Terminal items key straight off
 *  their terminalId (`-shell` covers the agent-exit fallback pane);
 *  agent chat items reconstruct the AgentChatPane's terminal id
 *  (`agent-chat:<projectId>`, see terminal-id.ts) via the projects
 *  store's path→id mapping. Resolution success doubles as the
 *  "offer the pin button at all" gate. */
export function resolvePinSessionId(
  item: PinnableItem,
  sessions: Record<string, string>,
  projects: ReadonlyArray<{ id: string; path: string }>,
): string | null {
  if (item.type === 'terminal') {
    const tid = (item.data as { terminalId: string }).terminalId
    return sessions[tid] ?? sessions[`${tid}-shell`] ?? null
  }
  if (item.type === 'agent') {
    const data = item.data as { agentName: string; projectPath: string; section?: string }
    if (data.section !== 'chat') return null
    const project = projects.find((p) => p.path === data.projectPath)
    if (!project) return null
    return sessions[agentChatId(project.id, data.agentName)] ?? null
  }
  return null
}

// ── Menu construction (pure) ─────────────────────────────────────────────

/** Build the pin-size ContextMenu item list for the current pin state:
 *  disabled "Pin size" header, the four presets (✓ on the active one),
 *  "Match my window now", and — only while pinned — a separator +
 *  Unpin. NBSP-ish two-space padding keeps unchecked labels
 *  column-aligned with the checked one (the menu renders in a
 *  monospace stack). */
export function buildPinSizeMenuItems(
  pin: Pick<PinnedSize, 'cols' | 'rows'> | null,
): ContextMenuItemDef[] {
  const mark = (on: boolean): string => (on ? '✓ ' : '  ')
  return [
    { id: 'pin-header', label: 'Pin size', enabled: false },
    ...PIN_PRESETS.map(({ cols, rows }) => ({
      id: `preset:${cols}x${rows}`,
      label: `${mark(!!pin && pin.cols === cols && pin.rows === rows)}${cols}×${rows}`,
    })),
    { id: 'match', label: `${mark(false)}Match my window now` },
    ...(pin
      ? [
          { id: 'pin-sep', label: '', type: 'separator' },
          { id: 'unpin', label: `${mark(false)}Unpin` },
        ]
      : []),
  ]
}

// ── Menu driver (context-menu store + daemon POST) ───────────────────────

/** Open the pin-size menu at (x, y) for a resolved daemon session and
 *  perform whatever the user picks: preset/match → POST pin-size,
 *  unpin → POST clear. Failures (and "match" with no measured dims)
 *  are reported through `onError` — the caller renders the hint. */
export async function openPinSizeMenu(opts: {
  x: number
  y: number
  sessionId: string
  onError: (msg: string) => void
}): Promise<void> {
  const { x, y, sessionId, onError } = opts
  const pin = usePinnedSizeStore.getState().pins[sessionId] ?? null
  const selected = await useContextMenuStore
    .getState()
    .show(x, y, buildPinSizeMenuItems(pin))
  if (!selected) return
  try {
    if (selected === 'unpin') {
      await daemonCliPost<PinSizeResponse>('terminal/pin-size', {
        session: sessionId,
        clear: true,
      })
      usePinnedSizeStore.getState().setPin(sessionId, null)
      return
    }
    let cols: number
    let rows: number
    if (selected === 'match') {
      // Freeze the size THIS window last measured for the pane
      // (recorded by TerminalPane's sendResize even while pinned).
      const dims = usePinnedSizeStore.getState().dims[sessionId]
      if (!dims) {
        onError('Pin failed: current window size unknown')
        return
      }
      cols = dims.cols
      rows = dims.rows
    } else {
      const m = /^preset:(\d+)x(\d+)$/.exec(selected)
      if (!m) return
      cols = Number(m[1])
      rows = Number(m[2])
    }
    const res = await daemonCliPost<PinSizeResponse>('terminal/pin-size', {
      session: sessionId,
      cols,
      rows,
    })
    // Mirror the authoritative answer immediately so the menu's
    // checkmark is right on reopen; the pane itself converges via
    // its `pin_changed` frame.
    if (res.pinned) {
      usePinnedSizeStore.getState().setPin(sessionId, {
        cols: res.pinned.cols,
        rows: res.pinned.rows,
        setBy: res.pinned.setBy ?? null,
      })
    }
  } catch (err) {
    onError(`Pin failed: ${err instanceof Error ? err.message : String(err)}`)
  }
}
