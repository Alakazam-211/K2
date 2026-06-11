import { useCallback } from 'react'
import { useTabsStore } from '@/stores/tabs'

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
