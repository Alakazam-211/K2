import React, { useState, useRef, useCallback, useEffect } from 'react'
import { TabBar } from '@/components/TabBar/TabBar'
import { PaneLayout } from '@/components/PaneLayout/PaneLayout'
import { PresetsBar } from '@/components/PresetsBar/PresetsBar'
import { useTabsStore } from '@/stores/tabs'
import { useResolvedAgentCommand } from '@/hooks/useResolvedAgentCommand'
import { useTerminalShortcuts } from '@/hooks/useTerminalShortcuts'
import { KeyCombo } from '@/components/KeySymbol'
import { TabVisibilityContext } from '@/contexts/TabVisibilityContext'
import { applyColumnResize } from './columnResize'

interface TerminalAreaProps {
  cwd: string
}

// ── Global drag state ────────────────────────────────────────────────────
// Uses mousedown/mousemove/mouseup instead of HTML5 drag-and-drop
// because terminals swallow drag events and the overlay timing is unreliable.

interface TabDragState {
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

export function startTabDrag(data: { groupIndex: number; tabId: string; tabTitle: string; mouseX: number; mouseY: number }): void {
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
      if (attr !== undefined) {
        const targetGroup = parseInt(attr, 10)
        if (targetGroup !== globalDrag.groupIndex) {
          useTabsStore.getState().moveTabToGroup(globalDrag.groupIndex, targetGroup, globalDrag.tabId)
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
  onDrag
}: {
  onDrag: (clientX: number) => void
}): React.JSX.Element {
  const draggingRef = useRef(false)

  const endDrag = useCallback(() => {
    draggingRef.current = false
    document.body.style.cursor = ''
    document.body.style.userSelect = ''
  }, [])

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
  const tabs = useTabsStore((s) => groupIndex === 0 ? s.tabs : s.extraGroups[groupIndex - 1]?.tabs ?? [])
  const activeTabId = useTabsStore((s) => groupIndex === 0 ? s.activeTabId : s.extraGroups[groupIndex - 1]?.activeTabId ?? null)
  const dragState = useTabDragState()

  // Show drop highlight when dragging a tab from a different group
  const showDropHighlight = dragState !== null && dragState.groupIndex !== groupIndex

  return (
    <div
      // ST3: tab strip ↔ pane content breathing room (0 in Square)
      className="relative flex h-full flex-col overflow-hidden gap-[var(--gap-section)]"
      style={style}
      onMouseDown={onFocus}
      data-tab-group-index={groupIndex}
    >
      <TabBar cwd={cwd} groupIndex={groupIndex} />
      <div className="relative flex-1 overflow-hidden">
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
        {tabs.length === 0 && <EmptyWorkspaceHints />}

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

// ── Empty workspace hints ────────────────────────────────────────────────

function EmptyWorkspaceHints(): React.JSX.Element {
  // Resolved through the one default-agent seam (id-first, legacy-token
  // tolerant, first-enabled fallback) so the hint names what ⇧⌘T launches.
  const resolved = useResolvedAgentCommand()
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
  const splitCount = useTabsStore((s) => s.splitCount)
  const activeGroupIndex = useTabsStore((s) => s.activeGroupIndex)
  const setActiveGroup = useTabsStore((s) => s.setActiveGroup)

  const [flexes, setFlexes] = useState([50, 25, 25])
  const containerRef = useRef<HTMLDivElement>(null)

  useTerminalShortcuts(cwd)

  const handleResize = useCallback((handleIndex: number, clientX: number) => {
    const container = containerRef.current
    if (!container) return
    const rowRect = container.getBoundingClientRect()
    setFlexes((prev) => applyColumnResize({ clientX, rowRect, handleIndex, flexes: prev }))
  }, [])

  const prevSplitCountRef = useRef(splitCount)
  if (splitCount !== prevSplitCountRef.current) {
    prevSplitCountRef.current = splitCount
    if (splitCount === 2) setFlexes([50, 50, 0])
    else if (splitCount === 3) setFlexes([34, 33, 33])
    else setFlexes([100, 0, 0])
  }

  return (
    <div className="flex h-full w-full flex-col overflow-hidden">
      <PresetsBar cwd={cwd} />
      {/* ST3: inter-column pane gap (0 in Square; resize handles keep their own width) */}
      <div ref={containerRef} className="flex flex-1 overflow-hidden gap-[var(--gap-pane)]">
        {Array.from({ length: splitCount }, (_, i) => (
          <React.Fragment key={i}>
            {i > 0 && (
              <ColumnResizeHandle
                onDrag={(clientX) => handleResize(i - 1, clientX)}
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
