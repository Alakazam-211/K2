import { type ReactNode, useCallback, useRef } from 'react'
import {
  SIDEBAR_MIN_WIDTH,
  SIDEBAR_MAX_WIDTH,
  SIDEBAR_DEFAULT_WIDTH
} from '../../../shared/constants'
import { usePanelsStore } from '../../stores/panels'
import { showContextMenu } from '../../lib/context-menu'

type PanelTab = 'files' | 'changes' | 'history' | 'workspace'

const TAB_LABELS: Record<PanelTab, string> = {
  files: 'Files',
  changes: 'Changes',
  history: 'Chats',
  workspace: 'Workspace'
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

  const handleTabContextMenu = useCallback(async (e: React.MouseEvent, tab: PanelTab) => {
    e.preventDefault()

    const clickedId = await showContextMenu([
      { id: 'move', label: `Move to ${oppositeLabel} Panel` },
      { id: 'separator', label: '', type: 'separator' },
      { id: 'close', label: 'Close Panel' }
    ])

    if (clickedId === 'move') {
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
  }, [oppositeSide, oppositeLabel, thisSide])

  return (
    <div
      className="relative flex flex-col h-full bg-[var(--color-bg-surface)] overflow-hidden"
      style={{ width }}
    >
      <PanelResizeHandle side={resizeSide} onWidthChange={onWidthChange} currentWidth={width} />

      {/* Optional header above tabs (e.g. workspace info in focus mode) */}
      {header}

      {/* Tab strip */}
      <div className="flex border-b border-[var(--color-border)] flex-shrink-0">
        {tabs.map((tab) => (
          <button
            key={tab}
            className={`no-drag flex-1 px-3 py-1.5 text-[11px] font-medium tracking-wide uppercase transition-colors ${
              activeTab === tab
                ? 'text-[var(--color-text-primary)] border-b border-[var(--color-accent)]'
                : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]'
            }`}
            onClick={() => onTabChange(tab)}
            onContextMenu={(e) => handleTabContextMenu(e, tab)}
          >
            {TAB_LABELS[tab]}
          </button>
        ))}
      </div>

      {/* Tab content */}
      <div className="flex-1 overflow-hidden">{children}</div>
    </div>
  )
}
