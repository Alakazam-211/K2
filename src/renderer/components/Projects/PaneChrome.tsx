// A dashboard pane's chrome (ProjectDashboard): the 32px bar (title, close,
// drag to move) over the pane's body.

import type React from 'react'
import { useLayoutEffect, useRef, useState } from 'react'
import { DialogCloseButton, Surface } from '@/components/ui'
import type { TileView } from './tile-view'
import { fitTileBar, measureTileBar, sameTileBarFit, TILE_BAR_FIT_DEFAULT, type TileBarFit } from './tile-bar-fit'

/** The bar keeps its name: it gives way in the order `fitTileBar` sets, as
 *  the tile's width, the name or the Style's fonts change. */
function useTileBarFit(deps: unknown[]): [React.RefObject<HTMLDivElement | null>, TileBarFit] {
  const ref = useRef<HTMLDivElement>(null)
  const [fit, setFit] = useState<TileBarFit>(TILE_BAR_FIT_DEFAULT)
  useLayoutEffect(() => {
    const bar = ref.current
    if (!bar) return
    let live = true
    const run = (): void => {
      if (!live) return
      const sizes = measureTileBar(bar)
      if (!sizes) return
      const next = fitTileBar(sizes)
      setFit((prev) => (sameTileBarFit(prev, next) ? prev : next))
    }
    run()
    // The bar's width (the tile), and the picker's (a Style switch changes
    // its cells and the fonts together).
    const ro = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(() => run())
    ro?.observe(bar)
    const picker = bar.querySelector('[data-tile-picker]')
    if (picker) ro?.observe(picker)
    void document.fonts?.ready.then(run)
    return () => {
      live = false
      ro?.disconnect()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps)
  return [ref, fit]
}

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
  /** An agent tile's view picker, in the bar's centre while the name fits
   *  beside it (prd-projects-threads-v1 §3.1). Its segments are buttons, so
   *  pressing one never starts a drag. */
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
  const [barRef, fit] = useTileBarFit([title, hint, shortcutNum, readOnly, !!picker, !!lead])
  const name = (
    <>
      {lead && (
        <span className="flex flex-none mr-2" data-tile-lead="">
          {lead}
        </span>
      )}
      <span className="min-w-0 type-name text-[var(--color-text-primary)] truncate" data-tile-title="">
        {title}
      </span>
      {hint && (
        <span className="min-w-0 ml-2 type-hint text-[var(--color-text-muted)] truncate" data-tile-hint="">
          {hint}
        </span>
      )}
    </>
  )
  const controls = (
    <>
      {/* ⌘N pane-switch badge, left of the close button: a shortcut hint,
          so it shows on the bar's hover and on keyboard focus in the tile
          (globals.css). */}
      {shortcutNum !== undefined && (
        <span
          className="flex-none whitespace-nowrap overflow-hidden mr-2 type-hint text-[var(--color-text-muted)] tabular-nums"
          data-tile-shortcut=""
          title={`⌘${shortcutNum} focuses this pane`}
        >
          <span className="key-symbol">⌘</span>
          {shortcutNum}
        </span>
      )}
      {!readOnly && (
        // The shared quiet ×: closing a tile only takes it off the
        // dashboard; an agent keeps running.
        <DialogCloseButton
          size={12}
          onClick={onClose}
          onMouseDown={(e) => e.stopPropagation()}
          title={picker ? 'Close pane. The agent keeps running.' : 'Close pane'}
          aria-label="Close pane"
          data-tile-close=""
        />
      )}
    </>
  )
  // No tooltip on the bar: it is the drag handle (the grab pointer says so),
  // and a tooltip over the name only got in the way.
  const barProps = {
    ref: barRef,
    'data-tile-bar': '',
    'data-tile-hint': hint ? (fit.hint ? 'on' : 'off') : undefined,
    'data-tile-keys': shortcutNum !== undefined ? (fit.keys ? 'on' : 'off') : undefined,
    onMouseDown: readOnly ? undefined : onHeaderMouseDown,
  }
  const bar = `h-8 gap-2 pl-2 pr-1 border-b border-[var(--color-border)] flex-shrink-0 select-none ${
    readOnly ? '' : 'cursor-grab'
  }`
  return (
    <Surface
      role2="surface" material="tile"
      bordered={false}
      data-tile=""
      data-tile-view={tileView}
      className={`flex flex-col h-full min-w-0 min-h-0 border transition-colors ${
        focused ? 'border-[var(--color-accent)]' : 'border-[var(--color-border)]'
      }`}
    >
      {/* The tile's bar, 32px: it holds the picker's cells. */}
      {picker ? (
        // Three columns: name | picker | controls. While the name fits
        // beside it, the picker is truly centred (Rosson, 2026-11-11: equal
        // side columns); when it doesn't, the picker follows the name and
        // the controls keep the right end (`data-tile-fit`, globals.css). A
        // narrow tile puts the picker on its own row (agent-tile).
        <div {...barProps} data-tile-fit={fit.centred ? 'centred' : 'follow'} className={`grid items-center ${bar}`}>
          <div className="flex min-w-0 items-center overflow-hidden" data-tile-start="">
            {name}
          </div>
          {/* The capsule's own padding is not a button: keep it off the drag. */}
          <span className="flex cursor-default justify-self-center" data-tile-picker="" onMouseDown={(e) => e.stopPropagation()}>
            {picker}
          </span>
          <div className="flex items-center justify-end" data-tile-end="">
            {controls}
          </div>
        </div>
      ) : (
        <div {...barProps} className={`flex items-center ${bar}`}>
          <div className="flex min-w-0 flex-1 items-center overflow-hidden" data-tile-start="">
            {name}
          </div>
          <div className="flex flex-none items-center justify-end" data-tile-end="">
            {controls}
          </div>
        </div>
      )}
      <div className="flex-1 min-h-0 min-w-0">{children}</div>
    </Surface>
  )
}
