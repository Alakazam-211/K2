// A dashboard pane's chrome (ProjectDashboard): the 32px bar (title, close,
// drag to move) over the pane's body.

import type React from 'react'
import { Surface } from '@/components/ui'
import type { TileView } from './tile-view'

export function PaneChrome({
  title,
  hint,
  shortcutNum,
  readOnly,
  focused,
  onClose,
  onHeaderMouseDown,
  lead,
  picker,
  tileView,
  children,
}: {
  title: string
  hint?: string
  /** Before the title: an agent tile's 16px picture. */
  lead?: React.ReactNode
  /** An agent tile's view picker, in the bar's centre (prd-projects-threads-v1
   *  §3.1). Its segments are buttons, so pressing one never starts a drag. */
  picker?: React.ReactNode
  /** An agent tile's view: `data-tile-view` on the tile, read by focus
   *  (⌘1–9, Esc) and Ctrl+`. */
  tileView?: TileView
  /** ⌘N badge — reading-order pane number (first 9 panes only). */
  shortcutNum?: number
  readOnly: boolean
  focused: boolean
  onClose: () => void
  onHeaderMouseDown?: (e: React.MouseEvent) => void
  children: React.ReactNode
}): React.JSX.Element {
  const name = (
    <>
      {lead}
      <span className="min-w-0 text-[11px] font-semibold text-[var(--color-text-secondary)] truncate" data-tile-title="">
        {title}
      </span>
      {hint && <span className="text-[9px] text-[var(--color-text-muted)] truncate opacity-70">{hint}</span>}
    </>
  )
  const controls = (
    <>
      {/* ⌘N pane-switch badge (the ActiveBar shortcutNum look),
          immediately left of the close button. */}
      {shortcutNum !== undefined && (
        <span
          className="text-[10px] font-mono text-[var(--color-text-muted)] tabular-nums flex-shrink-0"
          data-tile-shortcut=""
          title={`⌘${shortcutNum} focuses this pane`}
        >
          <span className="key-symbol">⌘</span>
          {shortcutNum}
        </span>
      )}
      {!readOnly && (
        <button
          type="button"
          onClick={onClose}
          onMouseDown={(e) => e.stopPropagation()}
          className="flex items-center justify-center w-4 h-4 flex-shrink-0 text-[var(--color-text-muted)] hover:text-[var(--color-status-error-soft)] transition-colors cursor-pointer"
          title="Close pane"
          aria-label="Close pane"
        >
          <svg width="8" height="8" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5">
            <line x1="2" y1="2" x2="10" y2="10" />
            <line x1="10" y1="2" x2="2" y2="10" />
          </svg>
        </button>
      )}
    </>
  )
  const bar = `h-8 gap-2 pl-2 pr-1.5 border-b border-[var(--color-border)] flex-shrink-0 select-none ${
    readOnly ? '' : 'cursor-grab'
  }`
  return (
    <Surface
      role2="surface" material="tile"
      bordered={false}
      data-tile-view={tileView}
      className={`flex flex-col h-full min-w-0 min-h-0 border transition-colors ${
        focused ? 'border-[var(--color-accent)]' : 'border-[var(--color-border)]'
      }`}
    >
      {/* The tile's bar, 32px: it holds the picker's cells. */}
      {picker ? (
        // Three columns, the picker truly centred (Rosson, 2026-11-11). The
        // name's column gives way first (it truncates, then clips); the
        // controls' column never shrinks below its content, so the picker
        // may shift left but never covers the name or the buttons. A narrow
        // tile puts the picker on its own row (globals.css, agent-tile).
        <div
          data-tile-bar=""
          data-tile-centred=""
          className={`grid grid-cols-[minmax(0,1fr)_auto_minmax(max-content,1fr)] items-center ${bar}`}
          onMouseDown={readOnly ? undefined : onHeaderMouseDown}
          title={readOnly ? undefined : 'Drag to move'}
        >
          <div className="flex min-w-0 items-center gap-2 overflow-hidden" data-tile-start="">
            {name}
          </div>
          {/* The capsule's own padding is not a button: keep it off the drag. */}
          <span className="flex cursor-default justify-self-center" data-tile-picker="" onMouseDown={(e) => e.stopPropagation()}>
            {picker}
          </span>
          <div className="flex items-center justify-end gap-2" data-tile-end="">
            {controls}
          </div>
        </div>
      ) : (
        <div
          data-tile-bar=""
          className={`flex items-center ${bar}`}
          onMouseDown={readOnly ? undefined : onHeaderMouseDown}
          title={readOnly ? undefined : 'Drag to move'}
        >
          {name}
          <span className="flex-1" data-tile-spacer="" />
          {controls}
        </div>
      )}
      <div className="flex-1 min-h-0 min-w-0">{children}</div>
    </Surface>
  )
}
