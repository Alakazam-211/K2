// prd-zen-freeform-chrome FC7–FC12, FC27, FC43 — the page's bands, title
// strip and column edges, drawn from data (`page.placement`: the daemon's
// `page.chrome`, `page.bands`, `page.edges`, `page.menus`).
//
//   - The TOP BAND (when it holds something) runs across the window above
//     the columns. It keeps the window buttons' insets: it starts after
//     `--zen-stoplight-safe-left` and ends before `--zen-stoplight-safe-right`
//     (FC11), and is at least as tall as the macOS lights' band.
//   - With no top band, K2 draws the TITLE STRIP instead (FC10): an empty
//     strip as tall as the window buttons (`--zen-stoplight-safe-top`). It
//     drags the window and holds nothing, so the macOS lights never sit on
//     the page.
//   - The BOTTOM BAND is drawn only when it holds something. No window
//     buttons there on any platform.
//   - A COLUMN EDGE is a short row at the top or bottom of one column,
//     outside its box (where a row rail always sat). Not a drag area.
//
// Every band (and the title strip) is a drag area: K2 binds `drag-region`
// on the band's own element (FC12), and clicks on its buttons never drag
// (`titlebar-drag.ts`). Each band keeps at least `ZEN_DRAG_MIN_WIDTH_PX` of
// empty space: the spacers between its start, center and end groups.
//
// Each row's items come in three groups (`start`, `center`, `end`), in the
// daemon's order (the last `end` item sits in the corner). A chrome item is
// drawn by the chrome registry (`zen-registry.tsx`) inside a
// `display: contents` wrapper (`data-zen-chrome-item`); a run of content
// widgets (the nav rail as a row) goes in one `ZenBandSlot`, which shrinks
// and clips first. When the row is still too narrow, usage hides, then the
// theme control, then the Garden switcher's name truncates (FC27,
// `zenRowOverflow`). Required controls and menu buttons never shrink.

import { useCallback, useContext, useEffect, useLayoutEffect, useRef, useState } from 'react'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import { ZEN_DRAG_MIN_WIDTH_PX } from '@/lib/zen/zen-controls'
import { zenRowOverflow, ZEN_OVERFLOW_ORDER, type ZenOverflowKind, type ZenRowOverflow } from '@/lib/zen/zen-overflow'
import type { ZenAlign, ZenChromeItem, ZenColumnEdge, ZenRowGroups, ZenWidgetDecl } from '@/lib/zen/zen-page'
import { zenChromeFor } from './zen-registry'
import { ZenBandSlot } from './ZenBand'
import {
  TEXTING_BAR_HEIGHT_PX,
  useZenBind,
  ZenChromePlaceContext,
  ZenK2ChromeContext,
  ZenRowCompactContext,
  type ZenChromePlace,
} from './ZenTemplateControls'

/** What the rows draw from: the page's content widgets and chrome items by
 *  id, and the template bridge K2's controls bind through. */
export interface ZenRowSource {
  widget(id: string): ZenWidgetDecl | null
  drawWidget(w: ZenWidgetDecl): React.JSX.Element
  item(id: string): ZenChromeItem | null
  /** A `menu`'s items, in file order. */
  menuItems(id: string): ZenChromeItem[]
  bridge: ZenWidgetBridge
}

/** Rows' shared look: K2 menu rows highlight on hover and keyboard focus. */
export const ZEN_CHROME_CSS = `
[data-zen-root] [data-zen-menu-list] [role="menuitem"]:hover,
[data-zen-root] [data-zen-menu-list] [role="menuitemradio"]:hover,
[data-zen-root] [data-zen-menu-list] [role="menuitem"]:focus-visible,
[data-zen-root] [data-zen-menu-list] [role="menuitemradio"]:focus-visible,
[data-zen-root] [data-zen-garden-menu] [role="menuitemradio"]:hover,
[data-zen-root] [data-zen-garden-menu] [role="menuitemradio"]:focus-visible {
  background: var(--zen-surface);
  outline: none;
}
`

const ROW_GAP_PX = 8
const GROUP_GAP_PX = 12

/** FC27: what gives way in a row, measured after every render and on
 *  resize. Hidden items keep their last natural width, so a row that grows
 *  again shows them again. Without layout (width 0) nothing hides. */
function useZenRowOverflow(inner: React.RefObject<HTMLDivElement | null>, dragMin: number): ZenRowOverflow {
  const [state, setState] = useState<ZenRowOverflow>({ hide: [], compactSwitcher: false })
  const natural = useRef(new Map<string, number>())
  const stateRef = useRef(state)
  stateRef.current = state
  const measure = useCallback(() => {
    const row = inner.current
    if (!row) return
    const available = row.clientWidth
    if (!(available > 0)) return
    let fixed = Math.max(0, row.children.length - 1) * GROUP_GAP_PX
    const optional: Partial<Record<ZenOverflowKind, number>> = {}
    for (const el of Array.from(row.querySelectorAll<HTMLElement>('[data-zen-chrome-item]'))) {
      const kind = el.getAttribute('data-zen-chrome-kind') ?? ''
      const child = el.firstElementChild
      const shown = el.style.display !== 'none' && child instanceof HTMLElement
      const width = shown ? (child as HTMLElement).offsetWidth + ROW_GAP_PX : 0
      if ((ZEN_OVERFLOW_ORDER as readonly string[]).includes(kind)) {
        if (shown && width > ROW_GAP_PX) natural.current.set(kind, width)
        const w = natural.current.get(kind)
        if (w !== undefined) optional[kind as ZenOverflowKind] = w
        continue
      }
      if (kind === 'garden-switcher') {
        if (shown && !stateRef.current.compactSwitcher) natural.current.set(kind, width)
        fixed += natural.current.get(kind) ?? width
        continue
      }
      fixed += width
    }
    const next = zenRowOverflow({ available, fixed, optional, dragMin })
    setState((prev) =>
      prev.compactSwitcher === next.compactSwitcher && prev.hide.join() === next.hide.join() ? prev : next,
    )
  }, [inner, dragMin])
  useLayoutEffect(() => {
    measure()
  })
  useEffect(() => {
    window.addEventListener('resize', measure)
    const ro = typeof ResizeObserver === 'function' ? new ResizeObserver(() => measure()) : null
    if (ro && inner.current) ro.observe(inner.current)
    return () => {
      window.removeEventListener('resize', measure)
      ro?.disconnect()
    }
  }, [measure, inner])
  return state
}

/** One group's items: chrome through the registry, content runs in a
 *  `ZenBandSlot`. */
function RowItems({
  ids,
  slot,
  src,
  hidden,
}: {
  ids: readonly string[]
  slot: string
  src: ZenRowSource
  hidden: ReadonlySet<string>
}): React.JSX.Element {
  // K2's extras are off in safe mode (FC22): no wrapper for them at all.
  const k2 = useContext(ZenK2ChromeContext)
  const off = (kind: string): boolean => (kind === 'usage' && !k2.usage) || (kind === 'theme-picker' && !k2.theme)
  const out: React.ReactNode[] = []
  let run: React.JSX.Element[] = []
  const flush = (): void => {
    if (run.length === 0) return
    out.push(
      <ZenBandSlot key={`content:${out.length}`} slot={slot}>
        {run}
      </ZenBandSlot>,
    )
    run = []
  }
  for (const id of ids) {
    const w = src.widget(id)
    if (w) {
      run.push(src.drawWidget(w))
      continue
    }
    flush()
    const item = src.item(id)
    if (!item || off(item.kind)) continue
    const Chrome = zenChromeFor(item.kind)
    if (!Chrome) continue
    out.push(
      <div
        key={id}
        data-zen-chrome-item={id}
        data-zen-chrome-kind={item.kind}
        data-zen-overflow-hidden={hidden.has(item.kind) ? '' : undefined}
        style={{ display: hidden.has(item.kind) ? 'none' : 'contents' }}
      >
        <Chrome item={item} bridge={src.bridge} menuItems={item.kind === 'menu' ? src.menuItems(item.id) : []} />
      </div>,
    )
  }
  flush()
  return <>{out}</>
}

function Group({
  name,
  ids,
  row,
  opens,
  slot,
  src,
  hidden,
  extra,
}: {
  name: ZenAlign
  ids: readonly string[]
  row: ZenChromePlace['row']
  opens: ZenChromePlace['opens']
  slot: string
  src: ZenRowSource
  hidden: ReadonlySet<string>
  extra?: Record<string, string>
}): React.JSX.Element | null {
  if (ids.length === 0) return null
  const hasContent = ids.some((id) => src.widget(id) !== null)
  const place: ZenChromePlace = { row, opens, align: name }
  return (
    <ZenChromePlaceContext.Provider value={place}>
      <div
        data-zen-row-group={name}
        {...extra}
        // Only a group with content may shrink (its content clips first);
        // controls never shrink.
        className={`flex items-center gap-2 ${hasContent ? 'min-w-0' : 'flex-shrink-0'}`}
        style={hasContent ? { flex: '0 1 auto' } : undefined}
      >
        <RowItems ids={ids} slot={slot} src={src} hidden={hidden} />
      </div>
    </ZenChromePlaceContext.Provider>
  )
}

/** The empty space between groups: drag space in a band (FC12). */
function Spacer({ min, drag }: { min: number; drag: boolean }): React.JSX.Element {
  return (
    <div
      aria-hidden
      data-zen-row-spacer=""
      data-zen-drag={drag ? '' : undefined}
      className="flex-1 self-stretch"
      style={{ minWidth: min }}
    />
  )
}

function RowBody({
  groups,
  row,
  opens,
  slot,
  src,
  dragMin,
  top,
}: {
  groups: ZenRowGroups
  row: ZenChromePlace['row']
  opens: ZenChromePlace['opens']
  slot: string
  src: ZenRowSource
  dragMin: number
  top: boolean
}): React.JSX.Element {
  const innerRef = useRef<HTMLDivElement | null>(null)
  const overflow = useZenRowOverflow(innerRef, dragMin)
  const hidden = new Set<string>(overflow.hide)
  const center = groups.center.length > 0
  const spacerMin = center ? dragMin / 2 : dragMin
  const drag = row !== 'edge'
  const common = { row, opens, slot, src, hidden }
  return (
    <ZenRowCompactContext.Provider value={overflow.compactSwitcher}>
      <div
        ref={innerRef}
        data-zen-row-inner=""
        className="flex min-w-0 flex-1 items-center self-stretch"
        style={{ gap: GROUP_GAP_PX }}
      >
        <Group name="start" ids={groups.start} {...common} />
        <Spacer min={spacerMin} drag={drag} />
        {center && (
          <>
            <Group name="center" ids={groups.center} {...common} />
            <Spacer min={spacerMin} drag={drag} />
          </>
        )}
        <Group name="end" ids={groups.end} {...common} extra={top ? { 'data-zen-top-right': '' } : undefined} />
      </div>
    </ZenRowCompactContext.Provider>
  )
}

/** The top band, or (FC10) the title strip when it holds nothing. */
export function ZenTopBand({ groups, src }: { groups: ZenRowGroups | null; src: ZenRowSource }): React.JSX.Element {
  const drag = useZenBind(src.bridge, 'drag-region')
  if (!groups) {
    return (
      <div
        ref={drag}
        data-zen-title-strip=""
        aria-hidden
        className="flex-shrink-0"
        style={{ height: 'var(--zen-stoplight-safe-top, 28px)' }}
      />
    )
  }
  return (
    <div
      ref={drag}
      data-zen-band-row="top"
      data-zen-template-bar=""
      className="flex flex-shrink-0 items-center"
      style={{
        height: `max(${TEXTING_BAR_HEIGHT_PX + 8}px, var(--zen-stoplight-safe-top, 0px))`,
        paddingLeft: 'var(--zen-stoplight-safe-left, 14px)',
        paddingRight: 'calc(var(--zen-stoplight-safe-right, 0px) + 14px)',
      }}
    >
      <RowBody groups={groups} row="top" opens="down" slot="top" src={src} dragMin={ZEN_DRAG_MIN_WIDTH_PX} top />
    </div>
  )
}

/** The bottom band (FC10: only when it holds something). */
export function ZenBottomBand({ groups, src }: { groups: ZenRowGroups; src: ZenRowSource }): React.JSX.Element {
  const drag = useZenBind(src.bridge, 'drag-region')
  return (
    <div
      ref={drag}
      data-zen-band-row="bottom"
      className="flex flex-shrink-0 items-center"
      style={{ height: TEXTING_BAR_HEIGHT_PX + 8, padding: '0 14px' }}
    >
      <RowBody groups={groups} row="bottom" opens="up" slot="bottom" src={src} dragMin={ZEN_DRAG_MIN_WIDTH_PX} top={false} />
    </div>
  )
}

/** One column edge (FC9): a row at the top or bottom of a column, outside
 *  its box. Not a drag area. */
export function ZenEdgeRow({ edge, src }: { edge: ZenColumnEdge; src: ZenRowSource }): React.JSX.Element {
  return (
    <div
      data-zen-edge-row={`${edge.column}:${edge.edge}`}
      data-zen-edge={edge.edge}
      className="flex flex-shrink-0 items-center"
      style={{ minHeight: 30 }}
    >
      <RowBody
        groups={edge}
        row="edge"
        opens={edge.edge === 'bottom' ? 'up' : 'down'}
        slot={`edge-${edge.edge}`}
        src={src}
        dragMin={0}
        top={false}
      />
    </div>
  )
}
