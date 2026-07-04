import { useCallback, useEffect, useRef, useState } from 'react'
import { useTabsStore } from '@/stores/tabs'
import { useContextMenuStore, type ContextMenuItemDef } from '@/stores/context-menu'
import { usePinnedSizeStore } from '@/stores/pinned-size'
import { useProjectsStore } from '@/stores/projects'
import { agentChatId } from '@/lib/terminal-id'
import { daemonCliPost } from '@/lib/daemon-cli'

// ── Types (from tabs store — will be available after store refactor) ──

interface TerminalItemData {
  terminalId: string
  cwd: string
  command?: string
  args?: string[]
}

interface FileViewerItemData {
  filePath: string
}

interface AgentItemData {
  agentName: string
  projectPath: string
  /** 'inbox' = work board (no PTY of its own); 'chat' = the pinned
   *  chat, whose AgentChatPane hosts a Kessel TerminalPane. */
  section?: string
}

interface Item {
  id: string
  type: 'terminal' | 'file-viewer' | 'agent'
  data: TerminalItemData | FileViewerItemData | AgentItemData
  pinned?: boolean
}

// ── Helpers ──────────────────────────────────────────────────────────────

function getTabLabel(item: Item): string {
  if (item.type === 'terminal') {
    const data = item.data as TerminalItemData
    if (data.command) {
      const name = data.command.split('/').pop() || data.command
      return name
    }
    return 'Terminal'
  }
  if (item.type === 'file-viewer') {
    const data = item.data as FileViewerItemData
    return data.filePath.split('/').pop() || data.filePath
  }
  if (item.type === 'agent') {
    const data = item.data as AgentItemData
    return data.agentName === '__workspace__' ? 'Work Board' : `Agent: ${data.agentName}`
  }
  return 'Unknown'
}

// ── Pin-to-size (S7b, PRD presence-multiplayer §5.5) ─────────────────────

/** The preset grid sizes offered by the pin-size menu. */
const PIN_PRESETS: ReadonlyArray<{ cols: number; rows: number }> = [
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

// ── Props ────────────────────────────────────────────────────────────────

interface PaneTabBarProps {
  items: Item[]
  activeItemIndex: number
  onActivate: (index: number) => void
  onClose: (itemId: string) => void
  onClosePane?: () => void  // Close the entire pane/split
  tabId?: string
  paneGroupId?: string
}

// ── Component ────────────────────────────────────────────────────────────

export function PaneTabBar({
  items,
  activeItemIndex,
  onActivate,
  onClose,
  onClosePane,
  tabId,
  paneGroupId
}: PaneTabBarProps): React.JSX.Element {
  const handleClose = useCallback(
    (e: React.MouseEvent, itemId: string) => {
      e.stopPropagation()
      onClose(itemId)
    },
    [onClose]
  )

  // ── Pin-to-size menu (S7b) ─────────────────────────────────────────────
  //
  // Pin state arrives via the pane's grid-WS pin frames, mirrored into
  // `usePinnedSizeStore` by TerminalPane — no polling here. The same
  // store carries the terminalId→daemon-sessionId mapping (registered
  // at spawn-resolve), which is also the "is there a live session?"
  // gate for showing the button at all.
  const pinSessions = usePinnedSizeStore((s) => s.sessions)
  const pins = usePinnedSizeStore((s) => s.pins)
  const projects = useProjectsStore((s) => s.projects)
  const showContextMenu = useContextMenuStore((s) => s.show)

  // Transient inline hint for pin-size failures (cleared after 4s).
  const [pinHint, setPinHint] = useState<string | null>(null)
  const pinHintTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const flashPinHint = useCallback((msg: string) => {
    setPinHint(msg)
    if (pinHintTimerRef.current) clearTimeout(pinHintTimerRef.current)
    pinHintTimerRef.current = setTimeout(() => setPinHint(null), 4000)
  }, [])
  useEffect(
    () => () => {
      if (pinHintTimerRef.current) clearTimeout(pinHintTimerRef.current)
    },
    []
  )

  /** Daemon session UUID for a tab item, or null when the item has no
   *  live Kessel session (file viewers, work boards, legacy panes,
   *  not-yet-spawned terminals). Terminal items key straight off
   *  their terminalId (`-shell` covers the agent-exit fallback pane);
   *  agent chat tabs reconstruct the AgentChatPane's terminal id
   *  (`agent-chat:<projectId>`, see terminal-id.ts) via the projects
   *  store's path→id mapping. */
  const resolvePinSessionId = (item: Item): string | null => {
    if (item.type === 'terminal') {
      const tid = (item.data as TerminalItemData).terminalId
      return pinSessions[tid] ?? pinSessions[`${tid}-shell`] ?? null
    }
    if (item.type === 'agent') {
      const data = item.data as AgentItemData
      if (data.section !== 'chat') return null
      const project = projects.find((p) => p.path === data.projectPath)
      if (!project) return null
      return pinSessions[agentChatId(project.id, data.agentName)] ?? null
    }
    return null
  }

  const handlePinMenu = useCallback(
    async (e: React.MouseEvent, sessionId: string) => {
      e.stopPropagation()
      const anchor = (e.currentTarget as HTMLElement).getBoundingClientRect()
      const pin = usePinnedSizeStore.getState().pins[sessionId] ?? null
      // NBSP padding keeps unchecked labels column-aligned with the
      // checked one (the menu renders in a monospace stack).
      const mark = (on: boolean): string => (on ? '✓ ' : '  ')
      const menuItems: ContextMenuItemDef[] = [
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
      const selected = await showContextMenu(anchor.left, anchor.bottom + 4, menuItems)
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
            flashPinHint('Pin failed: current window size unknown')
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
        flashPinHint(
          `Pin failed: ${err instanceof Error ? err.message : String(err)}`
        )
      }
    },
    [showContextMenu, flashPinHint]
  )

  // Cross-pane drag detection: track mousedown on an item for potential drag-to-move
  const handleItemMouseDown = useCallback(
    (e: React.MouseEvent, itemId: string) => {
      if (e.button !== 0 || !tabId || !paneGroupId) return
      // Only trigger if target is not the close button
      if ((e.target as HTMLElement).closest('button')) return

      const startX = e.clientX
      const startY = e.clientY
      let dragging = false

      const onMouseMove = (ev: MouseEvent): void => {
        if (!dragging && (Math.abs(ev.clientX - startX) > 5 || Math.abs(ev.clientY - startY) > 5)) {
          dragging = true
          document.body.style.cursor = 'grabbing'
          document.body.style.userSelect = 'none'
        }
        if (!dragging) return

        // Find the pane group under the cursor
        const el = document.elementFromPoint(ev.clientX, ev.clientY)
        const paneEl = el?.closest('[data-pane-group-id]') as HTMLElement | null
        if (paneEl) {
          paneEl.style.outline = '1px solid var(--color-accent)'
        }
      }

      const onMouseUp = (ev: MouseEvent): void => {
        document.removeEventListener('mousemove', onMouseMove)
        document.removeEventListener('mouseup', onMouseUp)
        document.body.style.cursor = ''
        document.body.style.userSelect = ''

        // Clear any outlines
        document.querySelectorAll('[data-pane-group-id]').forEach((el) => {
          (el as HTMLElement).style.outline = ''
        })

        if (!dragging) return

        // Find drop target
        const el = document.elementFromPoint(ev.clientX, ev.clientY)
        const paneEl = el?.closest('[data-pane-group-id]') as HTMLElement | null
        if (!paneEl) return

        const toTabId = paneEl.dataset.tabId
        const toPaneGroupId = paneEl.dataset.paneGroupId
        if (!toTabId || !toPaneGroupId) return
        if (toTabId === tabId && toPaneGroupId === paneGroupId) return // same pane

        useTabsStore.getState().moveItemBetweenPanes(tabId, paneGroupId, itemId, toTabId, toPaneGroupId)
      }

      document.addEventListener('mousemove', onMouseMove)
      document.addEventListener('mouseup', onMouseUp)
    },
    [tabId, paneGroupId]
  )

  return (
    <div
      className="flex items-center border-b border-[var(--color-border)] overflow-x-auto"
      style={{
        height: '24px',
        minHeight: '24px',
        background: '#111',
        fontFamily: "'MesloLGM Nerd Font', Menlo, Monaco, monospace",
        fontSize: '11px'
      }}
    >
      <div className="flex flex-1 items-center overflow-x-auto">
        {items.map((item, index) => {
          const isActive = index === activeItemIndex
          // S7b — pin-size entry point, only on items with a LIVE
          // Kessel session (the store mapping doubles as the gate).
          const pinSessionId = resolvePinSessionId(item)
          const pin = pinSessionId ? pins[pinSessionId] ?? null : null
          return (
            <div
              key={item.id}
              className={`flex items-center cursor-pointer select-none shrink-0 ${
                isActive
                  ? 'text-[var(--color-text-primary)]'
                  : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]'
              }`}
              style={{
                height: '24px',
                paddingLeft: '8px',
                paddingRight: '4px',
                background: isActive ? '#1a1a1a' : 'transparent',
                borderBottom: isActive ? '1px solid var(--color-accent)' : '1px solid transparent',
                maxWidth: '160px'
              }}
              onClick={() => onActivate(index)}
              onMouseDown={(e) => handleItemMouseDown(e, item.id)}
            >
              <span className="truncate" style={{ lineHeight: '24px' }}>
                {getTabLabel(item)}
              </span>

              {pinSessionId && (
                <button
                  className="ml-1 flex items-center justify-center shrink-0 hover:bg-white/10"
                  style={{
                    width: '14px',
                    height: '14px',
                    color: pin ? 'var(--color-accent)' : undefined,
                  }}
                  onClick={(e) => handlePinMenu(e, pinSessionId)}
                  onMouseDown={(e) => e.stopPropagation()}
                  title={
                    pin
                      ? `Pinned to ${pin.cols}×${pin.rows} — click to change`
                      : 'Pin size'
                  }
                  data-pane-tab-pin-size=""
                >
                  {/* Pushpin glyph — head, shoulder, needle. */}
                  <svg
                    width="8"
                    height="8"
                    viewBox="0 0 12 12"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="1.5"
                  >
                    <path d="M4 1.5 h4 l-0.75 3.5 L9 6.5 H3 l1.75-1.5 Z" />
                    <line x1="6" y1="6.5" x2="6" y2="10.5" />
                  </svg>
                </button>
              )}

              <button
                className="ml-1.5 flex items-center justify-center shrink-0 hover:bg-white/10"
                style={{ width: '14px', height: '14px' }}
                onClick={(e) => handleClose(e, item.id)}
                title="Close tab"
              >
                <svg
                  width="7"
                  height="7"
                  viewBox="0 0 8 8"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.5"
                >
                  <line x1="1" y1="1" x2="7" y2="7" />
                  <line x1="7" y1="1" x2="1" y2="7" />
                </svg>
              </button>
            </div>
          )
        })}
      </div>

      {/* S7b — transient pin-size failure hint (auto-clears). */}
      {pinHint && (
        <span
          className="shrink-0 px-2 truncate"
          style={{ color: '#f66', fontSize: '10px', maxWidth: '260px' }}
          data-pane-tab-pin-hint=""
        >
          {pinHint}
        </span>
      )}

      {/* Close pane button — removes this split */}
      {onClosePane && (
        <button
          className="flex items-center justify-center shrink-0 text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-white/10"
          style={{ width: '24px', height: '24px' }}
          onClick={(e) => {
            e.stopPropagation()
            onClosePane()
          }}
          title="Close pane"
        >
          <svg
            width="10"
            height="10"
            viewBox="0 0 10 10"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
          >
            <line x1="1" y1="1" x2="9" y2="9" />
            <line x1="9" y1="1" x2="1" y2="9" />
          </svg>
        </button>
      )}
    </div>
  )
}
