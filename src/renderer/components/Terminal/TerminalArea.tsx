import React, { useState, useRef, useCallback, useEffect } from 'react'
import { TabBar } from '@/components/TabBar/TabBar'
import { PaneLayout } from '@/components/PaneLayout/PaneLayout'
import { PresetsBar } from '@/components/PresetsBar/PresetsBar'
import { useRoom, useRoomTabs } from '@/components/Room/RoomContext'
import type { Room } from '@/stores/room'
import { useRoomResolvedAgentCommand } from '@/hooks/useResolvedAgentCommand'
import { useTerminalShortcuts } from '@/hooks/useTerminalShortcuts'
import { KeyCombo } from '@/components/KeySymbol'
import { TabVisibilityContext } from '@/contexts/TabVisibilityContext'
import { applyColumnResize } from './columnResize'
import { readColumnFlexes, writeColumnFlexes } from '@/lib/column-sizes'
import { scopedKey } from '@/kessel/server-scope'

interface TerminalAreaProps {
  cwd: string
}

// ── Global drag state ────────────────────────────────────────────────────
// Uses mousedown/mousemove/mouseup instead of HTML5 drag-and-drop
// because terminals swallow drag events and the overlay timing is unreliable.

interface TabDragState {
  /** The room the drag started in. A tab never moves to another room
   *  (MS4): a drop on another room's column does nothing. */
  room: Room
  groupIndex: number
  tabId: string
  tabTitle: string
  mouseX: number
  mouseY: number
}

let globalDrag: TabDragState | null = null
const dragListeners = new Set<() => void>()

function notifyDragListeners(): void {
  dragListeners.forEach((fn) => fn())
}

export function startTabDrag(data: { room: Room; groupIndex: number; tabId: string; tabTitle: string; mouseX: number; mouseY: number }): void {
  globalDrag = data
  notifyDragListeners()

  const handleMouseMove = (e: MouseEvent): void => {
    if (!globalDrag) return
    globalDrag = { ...globalDrag, mouseX: e.clientX, mouseY: e.clientY }
    notifyDragListeners()
  }

  const handleMouseUp = (e: MouseEvent): void => {
    // Use the event's coordinates rather than globalDrag.mouseX/Y so the
    // drop hit-test always reflects the exact release position even if the
    // last mousemove arrived slightly stale.
    if (globalDrag) {
      const el = document.elementFromPoint(e.clientX, e.clientY) as HTMLElement | null
      const col = el?.closest('[data-tab-group-index]') as HTMLElement | null
      const attr = col?.dataset?.tabGroupIndex
      const sameRoom = col?.closest('[data-room-key]')?.getAttribute('data-room-key') === globalDrag.room.key
      if (attr !== undefined && sameRoom) {
        const targetGroup = parseInt(attr, 10)
        if (targetGroup !== globalDrag.groupIndex) {
          globalDrag.room.tabs.getState().moveTabToGroup(globalDrag.groupIndex, targetGroup, globalDrag.tabId)
        }
      }
    }

    globalDrag = null
    notifyDragListeners()
    document.removeEventListener('mousemove', handleMouseMove)
    document.removeEventListener('mouseup', handleMouseUp)
    document.body.style.cursor = ''
    document.body.style.userSelect = ''
  }

  document.addEventListener('mousemove', handleMouseMove)
  document.addEventListener('mouseup', handleMouseUp)
  document.body.style.cursor = 'grabbing'
  document.body.style.userSelect = 'none'
}

function useTabDragState(): TabDragState | null {
  const [state, setState] = useState(globalDrag)
  useEffect(() => {
    const handler = (): void => setState(globalDrag ? { ...globalDrag } : null)
    dragListeners.add(handler)
    return () => { dragListeners.delete(handler) }
  }, [])
  return state
}

// ── Resize Handle between columns ────────────────────────────────────────

function ColumnResizeHandle({
  onDrag,
  onDragEnd,
}: {
  onDrag: (clientX: number) => void
  /** Once per drag, when the pointer lets go (or capture is lost). */
  onDragEnd: () => void
}): React.JSX.Element {
  const draggingRef = useRef(false)

  const endDrag = useCallback(() => {
    if (!draggingRef.current) return
    draggingRef.current = false
    document.body.style.cursor = ''
    document.body.style.userSelect = ''
    onDragEnd()
  }, [onDragEnd])

  const handlePointerDown = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return
    e.preventDefault()
    draggingRef.current = true
    e.currentTarget.setPointerCapture(e.pointerId)
    document.body.style.cursor = 'col-resize'
    document.body.style.userSelect = 'none'
  }, [])

  const handlePointerMove = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    if (!draggingRef.current) return
    onDrag(e.clientX)
  }, [onDrag])

  const handlePointerUp = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    if (e.currentTarget.hasPointerCapture(e.pointerId)) {
      e.currentTarget.releasePointerCapture(e.pointerId)
    }
    endDrag()
  }, [endDrag])

  return (
    <div
      className="flex-shrink-0 hover:bg-[var(--color-accent)] transition-colors"
      data-column-resize-handle=""
      style={{
        width: 4,
        cursor: 'col-resize',
        backgroundColor: 'var(--color-border)',
        touchAction: 'none',
      }}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerUp}
      onPointerCancel={handlePointerUp}
      onLostPointerCapture={endDrag}
    />
  )
}

// ── Single Tab Group Column ──────────────────────────────────────────────

function TabGroupColumn({
  groupIndex,
  cwd,
  isActive,
  onFocus,
  style
}: {
  groupIndex: number
  cwd: string
  isActive: boolean
  onFocus: () => void
  style?: React.CSSProperties
}): React.JSX.Element {
  const room = useRoom()
  const tabs = useRoomTabs((s) => groupIndex === 0 ? s.tabs : s.extraGroups[groupIndex - 1]?.tabs ?? [])
  const activeTabId = useRoomTabs((s) => groupIndex === 0 ? s.activeTabId : s.extraGroups[groupIndex - 1]?.activeTabId ?? null)
  const dragState = useTabDragState()

  // Show drop highlight when dragging a tab from a different group
  const showDropHighlight =
    dragState !== null && dragState.room === room && dragState.groupIndex !== groupIndex

  return (
    <div
      // ST3: tab strip ↔ pane content breathing room (0 in Square)
      className="relative flex h-full flex-col overflow-hidden gap-[var(--gap-section)]"
      style={style}
      onMouseDown={onFocus}
      data-tab-group-index={groupIndex}
    >
      <TabBar cwd={cwd} groupIndex={groupIndex} />
      <div className="relative flex-1 overflow-hidden" data-toast-host="workspace">
        {/*
          Retained-view model (see Zed/VS Code): every open tab's tree
          stays mounted so scroll/cursor/focus state lives naturally on
          the DOM across tab switches. Only the active tab is visible;
          the rest are hidden via display:none, which preserves their
          state without consuming paint cycles.
        */}
        {tabs.map((tab) => {
          const isActiveTab = tab.id === activeTabId
          return (
            <TabVisibilityContext.Provider key={tab.id} value={isActiveTab}>
              <div
                data-tab-id={tab.id}
                className="absolute inset-0"
                style={{ display: isActiveTab ? 'block' : 'none' }}
                aria-hidden={!isActiveTab}
              >
                <PaneLayout tabId={tab.id} />
              </div>
            </TabVisibilityContext.Provider>
          )
        })}
        {tabs.length === 0 && (groupIndex > 0 ? <EmptySplitColumnHint /> : <EmptyWorkspaceHints />)}

        {/* Drop highlight */}
        {showDropHighlight && (
          <div
            className="absolute inset-0 pointer-events-none"
            style={{
              zIndex: 10,
              backgroundColor: 'color-mix(in srgb, var(--color-accent) 8%, transparent)',
              border: '2px solid var(--color-accent)',
            }}
          />
        )}
      </div>
    </div>
  )
}

// ── Empty split column ───────────────────────────────────────────────────

/** A split column with no tabs (the split button on the pinned tab leaves the
 *  new column empty): tell the user how to fill it. */
export function EmptySplitColumnHint(): React.JSX.Element {
  return (
    <div
      className="flex h-full flex-col items-center justify-center gap-1.5 px-4 text-center text-[var(--color-text-muted)]"
      data-empty-split-column=""
    >
      <span className="text-xs text-[var(--color-text-secondary)]">Drag a tab here to see it side by side</span>
      <span className="text-[11px]">or press + for a new tab</span>
    </div>
  )
}

// ── Empty workspace hints ────────────────────────────────────────────────

function EmptyWorkspaceHints(): React.JSX.Element {
  // Resolved through the one default-agent seam (id-first, legacy-token
  // tolerant, first-enabled fallback) so the hint names what ⇧⌘T launches.
  const resolved = useRoomResolvedAgentCommand()
  const agentLabel = resolved?.preset.label || 'AI Agent'

  return (
    <div className="flex h-full flex-col items-center justify-center gap-3 text-[var(--color-text-muted)]">
      <div className="flex flex-col items-center gap-2.5">
        <span className="text-xs">
          <kbd className="px-1.5 py-0.5 bg-white/[0.06] text-[var(--color-text-secondary)] font-mono text-[11px]">
            <KeyCombo combo="⌘" />T
          </kbd>
          <span className="ml-2">Terminal</span>
        </span>
        <span className="text-xs">
          <kbd className="px-1.5 py-0.5 bg-white/[0.06] text-[var(--color-text-secondary)] font-mono text-[11px]">
            <KeyCombo combo="⌘" /><KeyCombo combo="⇧" />T
          </kbd>
          <span className="ml-2">{agentLabel}</span>
        </span>
        <span className="text-xs">
          <kbd className="px-1.5 py-0.5 bg-white/[0.06] text-[var(--color-text-secondary)] font-mono text-[11px]">
            <KeyCombo combo="⌘" />N
          </kbd>
          <span className="ml-2">New file</span>
        </span>
      </div>
    </div>
  )
}

// ── Drag ghost (follows cursor) ──────────────────────────────────────────

function DragGhost(): React.JSX.Element | null {
  const dragState = useTabDragState()
  if (!dragState) return null

  return (
    <div
      className="fixed pointer-events-none"
      style={{
        left: dragState.mouseX + 8,
        top: dragState.mouseY - 12,
        zIndex: 9999,
        backgroundColor: 'var(--color-bg-inset)',
        border: '1px solid var(--color-border)',
        padding: '4px 10px',
        fontSize: '11px',
        color: 'var(--color-text-primary)',
        fontFamily: 'inherit',
        whiteSpace: 'nowrap',
        opacity: 0.9,
      }}
    >
      {dragState.tabTitle}
    </div>
  )
}

// ── Main Terminal Area ───────────────────────────────────────────────────

export function TerminalArea({ cwd }: TerminalAreaProps): React.JSX.Element {
  const room = useRoom()
  const splitCount = useRoomTabs((s) => s.splitCount)
  const activeGroupIndex = useRoomTabs((s) => s.activeGroupIndex)
  const setActiveGroup = useRoomTabs((s) => s.setActiveGroup)

  // Column widths are per-window view state (split-view D2), kept in this
  // window's `k2.windowChrome.columns.<label>` per workspace and column
  // count. Leaving a workspace stashes it (splitCount → 1, key → null) and
  // coming back restores splitCount, so the widths are re-read whenever the
  // workspace or the column count changes — never reset to an even split.
  const activeWorkspaceKey = useRoomTabs((s) => s.activeWorkspaceKey)
  const sizesKey = activeWorkspaceKey ? scopedKey(room.scope, activeWorkspaceKey) : null
  const [sized, setSized] = useState(() => ({
    key: sizesKey,
    count: splitCount,
    flexes: readColumnFlexes(sizesKey, splitCount),
  }))
  // The latest widths for the drag math and the save on release, which can
  // run before React re-renders with the last move.
  const sizedRef = useRef(sized)
  let current = sized
  if (sized.key !== sizesKey || sized.count !== splitCount) {
    current = { key: sizesKey, count: splitCount, flexes: readColumnFlexes(sizesKey, splitCount) }
    setSized(current)
  }
  sizedRef.current = current
  const flexes = current.flexes
  const containerRef = useRef<HTMLDivElement>(null)

  useTerminalShortcuts(room, cwd)

  const handleResize = useCallback((handleIndex: number, clientX: number) => {
    const container = containerRef.current
    if (!container) return
    const rowRect = container.getBoundingClientRect()
    const prev = sizedRef.current
    const next = { ...prev, flexes: applyColumnResize({ clientX, rowRect, handleIndex, flexes: prev.flexes }) }
    sizedRef.current = next
    setSized(next)
  }, [])

  const handleResizeEnd = useCallback(() => {
    const { key, count, flexes: widths } = sizedRef.current
    if (key && widths.length === count) writeColumnFlexes(key, widths)
  }, [])

  return (
    // `data-room-key` — pointer-down / focus-in here focuses this room for
    // window-level input (MS17), and tab drags stay inside it (MS4).
    <div className="flex h-full w-full flex-col overflow-hidden" data-room-key={room.key}>
      {/* The launch bar edits the WINDOW's server's presets (create, edit,
          reorder), so only the window's own room shows it. A Home room
          launches agents from the + menu and Ctrl+1–9, which spawn on the
          room's server (Home M5). */}
      {room.isPrimary && <PresetsBar cwd={cwd} />}
      {/* ST3: inter-column pane gap (0 in Square; resize handles keep their own width) */}
      <div ref={containerRef} className="flex flex-1 overflow-hidden gap-[var(--gap-pane)]">
        {Array.from({ length: splitCount }, (_, i) => (
          <React.Fragment key={i}>
            {i > 0 && (
              <ColumnResizeHandle
                onDrag={(clientX) => handleResize(i - 1, clientX)}
                onDragEnd={handleResizeEnd}
              />
            )}
            <TabGroupColumn
              groupIndex={i}
              cwd={cwd}
              isActive={i === activeGroupIndex}
              onFocus={() => setActiveGroup(i)}
              style={{ flex: `${flexes[i]} 0 0%`, minWidth: 0 }}
            />
          </React.Fragment>
        ))}
      </div>
      <DragGhost />
    </div>
  )
}
