import { type ReactNode, useCallback, useEffect, useId, useMemo, useRef, useState } from 'react'
import {
  SIDEBAR_MIN_WIDTH,
  SIDEBAR_MAX_WIDTH,
  SIDEBAR_DEFAULT_WIDTH
} from '../../../shared/constants'
import { usePanelsStore, type PanelTab } from '../../stores/panels'
import { showContextMenu } from '../../lib/context-menu'
import { Surface } from '@/components/ui'
import DrawerTabBarToggle from '@/components/Layout/DrawerTabBarToggle'
import { TAB_STRIP_HEIGHT } from '@/lib/tab-strip'
import {
  DRAWER_TAB_DRAG_THRESHOLD_PX,
  applyTabOrder,
  dropSlotAt,
  isCustomOrder,
  moveByStep,
  moveToSlot,
} from '@/lib/drawer-tab-order'

const TAB_LABELS: Record<PanelTab, string> = {
  files: 'Files',
  agent: 'Agent',
  history: 'Chats',
  workspace: 'Workspace'
}

export const DRAWER_TAB_REORDER_HELP =
  'Drag a tab to reorder it, or press Alt+Left Arrow or Alt+Right Arrow to move it.'

/** What a screen reader hears after a move ("Files moved to position 2 of 2"). */
export function drawerTabMovedText(tab: PanelTab, order: readonly PanelTab[]): string {
  return `${TAB_LABELS[tab]} moved to position ${order.indexOf(tab) + 1} of ${order.length}`
}

interface TabbedPanelProps {
  tabs: PanelTab[]
  activeTab: PanelTab
  onTabChange: (tab: PanelTab) => void
  width: number
  onWidthChange: (width: number) => void
  /** Which side this panel is on */
  resizeSide: 'left' | 'right'
  /** Optional header rendered above the tab strip (e.g. workspace header in focus mode) */
  header?: ReactNode
  children: ReactNode
}

function PanelResizeHandle({
  side,
  onWidthChange,
  currentWidth
}: {
  side: 'left' | 'right'
  onWidthChange: (width: number) => void
  currentWidth: number
}): React.JSX.Element {
  const isDragging = useRef(false)

  const handleMouseDown = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault()
      isDragging.current = true

      const startX = e.clientX
      const startWidth = currentWidth

      const handleMouseMove = (moveEvent: MouseEvent): void => {
        if (!isDragging.current) return
        const delta = moveEvent.clientX - startX
        const newWidth = side === 'right'
          ? Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, startWidth + delta))
          : Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, startWidth - delta))
        onWidthChange(newWidth)
      }

      const handleMouseUp = (): void => {
        isDragging.current = false
        document.removeEventListener('mousemove', handleMouseMove)
        document.removeEventListener('mouseup', handleMouseUp)
        document.body.style.cursor = ''
        document.body.style.userSelect = ''
      }

      document.addEventListener('mousemove', handleMouseMove)
      document.addEventListener('mouseup', handleMouseUp)
      document.body.style.cursor = 'col-resize'
      document.body.style.userSelect = 'none'
    },
    [onWidthChange, currentWidth, side]
  )

  const handleDoubleClick = useCallback(() => {
    onWidthChange(SIDEBAR_DEFAULT_WIDTH)
  }, [onWidthChange])

  return (
    <div
      data-resize-edge=""
      className={`absolute top-0 bottom-0 w-1 cursor-col-resize hover:bg-[var(--color-accent)] transition-colors duration-150 z-10 no-drag ${
        side === 'left' ? 'left-0' : 'right-0'
      }`}
      onMouseDown={handleMouseDown}
      onDoubleClick={handleDoubleClick}
    />
  )
}

export default function TabbedPanel({
  tabs,
  activeTab,
  onTabChange,
  width,
  onWidthChange,
  resizeSide,
  header,
  children
}: TabbedPanelProps): React.JSX.Element {
  // Determine which side this panel is on based on resizeSide
  // resizeSide='right' means the resize handle is on the right edge → this is the LEFT panel
  // resizeSide='left' means the resize handle is on the left edge → this is the RIGHT panel
  const thisSide = resizeSide === 'right' ? 'left' : 'right'
  const oppositeSide = thisSide === 'left' ? 'right' : 'left'
  const oppositeLabel = oppositeSide === 'left' ? 'Left' : 'Right'
  const reorderHelpId = `drawer-tab-reorder-help-${useId()}`

  // ── Tab order (0.46.13): this client's drag order over the drawer's tabs ──
  const savedOrder = usePanelsStore((s) =>
    thisSide === 'left' ? s.leftPanelTabOrder : s.rightPanelTabOrder
  )
  const ordered = useMemo(() => applyTabOrder(tabs, savedOrder), [tabs, savedOrder])
  const customOrder = isCustomOrder(tabs, ordered)

  const stripRef = useRef<HTMLDivElement>(null)
  // The tab being dragged and the drop slot under the pointer (null = no drag).
  const [drag, setDrag] = useState<{ from: number; slot: number } | null>(null)
  const [announcement, setAnnouncement] = useState('')
  // A drag's release can fire a click on the tab; swallow that one click.
  // Cleared on the next press, so a release off the tab never eats a real click.
  const swallowClick = useRef(false)
  const refocusTab = useRef<PanelTab | null>(null)
  // Ends a drag still in progress (a new press, or the panel unmounting).
  const endDrag = useRef<(() => void) | null>(null)
  useEffect(() => () => endDrag.current?.(), [])

  const commitOrder = useCallback(
    (next: readonly PanelTab[], moved: PanelTab) => {
      usePanelsStore.getState().reorderDrawerTabs(thisSide, next)
      setAnnouncement(drawerTabMovedText(moved, next))
    },
    [thisSide]
  )

  // Focus follows a tab moved with the keyboard.
  useEffect(() => {
    const tab = refocusTab.current
    if (tab === null || !stripRef.current) return
    refocusTab.current = null
    stripRef.current.querySelector<HTMLElement>(`[data-drawer-tab-id="${tab}"]`)?.focus()
  }, [ordered])

  // Press and move past DRAWER_TAB_DRAG_THRESHOLD_PX to drag; a plain click
  // still selects. Esc cancels; the release saves the new order.
  const handleTabPointerDown = (e: React.PointerEvent, from: number): void => {
    swallowClick.current = false
    if (e.button !== 0 || ordered.length < 2) return
    endDrag.current?.()
    const startX = e.clientX
    const startY = e.clientY
    let started = false
    let slot = from
    const blockSelect = (ev: Event): void => ev.preventDefault()

    const cleanup = (): void => {
      document.removeEventListener('pointermove', onMove)
      document.removeEventListener('pointerup', onUp)
      document.removeEventListener('pointercancel', onCancel)
      document.removeEventListener('keydown', onKey, true)
      document.removeEventListener('selectstart', blockSelect)
      if (started) {
        document.body.style.cursor = ''
        document.body.style.userSelect = ''
        setDrag(null)
      }
      endDrag.current = null
    }
    const onMove = (ev: PointerEvent): void => {
      if (!started) {
        if (Math.hypot(ev.clientX - startX, ev.clientY - startY) <= DRAWER_TAB_DRAG_THRESHOLD_PX) return
        started = true
        document.body.style.cursor = 'grabbing'
        document.body.style.userSelect = 'none'
        document.addEventListener('selectstart', blockSelect)
      }
      const items = stripRef.current?.querySelectorAll<HTMLElement>('[data-drawer-tab-index]') ?? []
      slot = dropSlotAt(
        Array.from(items, (el) => {
          const r = el.getBoundingClientRect()
          return { left: r.left, width: r.width }
        }),
        ev.clientX
      )
      setDrag({ from, slot })
    }
    const onUp = (): void => {
      const wasDrag = started
      cleanup()
      if (!wasDrag) return
      swallowClick.current = true
      const next = moveToSlot(ordered, from, slot)
      if (next !== ordered) commitOrder(next, ordered[from])
    }
    const onCancel = (): void => {
      if (started) swallowClick.current = true
      cleanup()
    }
    const onKey = (ev: KeyboardEvent): void => {
      if (ev.key !== 'Escape') return
      if (started) {
        ev.preventDefault()
        ev.stopPropagation()
      }
      onCancel()
    }

    document.addEventListener('pointermove', onMove)
    document.addEventListener('pointerup', onUp)
    document.addEventListener('pointercancel', onCancel)
    document.addEventListener('keydown', onKey, true)
    endDrag.current = cleanup
  }

  // Keyboard reorder on the focused tab: Alt+Left / Alt+Right (the strip is
  // horizontal). Focus follows the tab and the move is announced.
  const handleTabKeyDown = (e: React.KeyboardEvent, index: number): void => {
    if (!e.altKey || e.metaKey || e.ctrlKey || e.shiftKey) return
    if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return
    e.preventDefault()
    const next = moveByStep(ordered, index, e.key === 'ArrowLeft' ? -1 : 1)
    if (next === ordered) return
    refocusTab.current = ordered[index]
    commitOrder(next, ordered[index])
  }

  const handleTabContextMenu = useCallback(async (e: React.MouseEvent, tab: PanelTab) => {
    e.preventDefault()

    const clickedId = await showContextMenu([
      { id: 'move', label: `Move to ${oppositeLabel} Panel` },
      // Only once this client has moved the tabs out of their base order.
      ...(customOrder ? [{ id: 'reset-order', label: 'Reset Tab Order' }] : []),
      { id: 'separator', label: '', type: 'separator' },
      { id: 'close', label: 'Close Panel' }
    ])

    if (clickedId === 'reset-order') {
      usePanelsStore.getState().resetDrawerTabOrder(thisSide)
      setAnnouncement('Tab order reset')
    } else if (clickedId === 'move') {
      const store = usePanelsStore.getState()
      if (oppositeSide === 'left') {
        store.moveTabToLeft(tab)
        // Auto-open the target panel if it's closed
        if (!store.leftPanelOpen) {
          store.toggleLeftPanel()
        }
      } else {
        store.moveTabToRight(tab)
        if (!store.rightPanelOpen) {
          store.toggleRightPanel()
        }
      }
    } else if (clickedId === 'close') {
      if (thisSide === 'left') {
        usePanelsStore.getState().toggleLeftPanel()
      } else {
        usePanelsStore.getState().toggleRightPanel()
      }
    }
  }, [oppositeSide, oppositeLabel, thisSide, customOrder])

  return (
    <Surface
      role2="surface" material="tile"
      bordered={false}
      data-drawer-panel=""
      className="relative flex flex-col h-full min-h-0 overflow-hidden"
      style={{ width }}
    >
      <PanelResizeHandle side={resizeSide} onWidthChange={onWidthChange} currentWidth={width} />

      {/* Optional header above tabs (e.g. workspace info in focus mode) */}
      {header}

      {/* Tab strip — the room TabBar's height (TAB_STRIP_HEIGHT, 36px) so
          the tabs match and the bottom border lines up across left /
          center / right. */}
      <div
        ref={stripRef}
        className={`flex ${TAB_STRIP_HEIGHT} items-center border-b border-[var(--color-border)] flex-shrink-0`}
        data-drawer-tab-strip=""
      >
        {/* The open drawer's close toggle sits at the strip's end nearest
            the room (`Layout/DrawerTabBarToggle.tsx`). */}
        {thisSide === 'right' && <DrawerTabBarToggle side="right" placement="drawer" />}
        {ordered.map((tab, index) => {
          const dragged = drag !== null && drag.from === index
          // No drop line on the tab's own spot (dropping there changes nothing).
          const lineAt =
            drag !== null && drag.slot !== drag.from && drag.slot !== drag.from + 1 ? drag.slot : null
          const canMove = ordered.length > 1
          return (
            <button
              key={tab}
              type="button"
              data-drawer-tab=""
              data-drawer-tab-id={tab}
              data-drawer-tab-index={index}
              data-selected={activeTab === tab ? '' : undefined}
              data-dragging={dragged ? '' : undefined}
              aria-describedby={canMove ? reorderHelpId : undefined}
              aria-keyshortcuts={canMove ? 'Alt+ArrowLeft Alt+ArrowRight' : undefined}
              className={`relative no-drag flex-1 h-full px-3 text-[11px] font-medium tracking-wide uppercase transition-colors flex items-center justify-center select-none ${
                activeTab === tab
                  ? 'text-[var(--color-text-primary)] border-b border-[var(--color-accent)]'
                  : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]'
              } ${dragged ? 'opacity-30' : ''}`}
              onPointerDown={(e) => handleTabPointerDown(e, index)}
              onKeyDown={(e) => handleTabKeyDown(e, index)}
              onClick={() => {
                if (swallowClick.current) {
                  swallowClick.current = false
                  return
                }
                onTabChange(tab)
              }}
              onContextMenu={(e) => handleTabContextMenu(e, tab)}
            >
              {lineAt === index && (
                <span
                  aria-hidden="true"
                  data-drawer-tab-drop="before"
                  className="absolute left-0 top-1 bottom-1 w-[2px] bg-[var(--color-accent)] z-10 pointer-events-none"
                />
              )}
              {lineAt === ordered.length && index === ordered.length - 1 && (
                <span
                  aria-hidden="true"
                  data-drawer-tab-drop="after"
                  className="absolute right-0 top-1 bottom-1 w-[2px] bg-[var(--color-accent)] z-10 pointer-events-none"
                />
              )}
              {TAB_LABELS[tab]}
            </button>
          )
        })}
        {thisSide === 'left' && <DrawerTabBarToggle side="left" placement="drawer" />}
      </div>

      <span id={reorderHelpId} className="sr-only">
        {DRAWER_TAB_REORDER_HELP}
      </span>
      <span role="status" aria-live="polite" className="sr-only" data-drawer-tab-announce="">
        {announcement}
      </span>

      {/* Tab content */}
      <div className="flex-1 min-h-0 overflow-hidden">{children}</div>
    </Surface>
  )
}
