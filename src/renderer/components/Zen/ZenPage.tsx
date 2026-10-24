// prd-zen-mode-v1 Z10, Z27, Z28, Z33 and prd-zen-gardens-v1 G24, G29, G38,
// G52 — one Garden's resolved page on screen.
//
// Draws the page from data (prd-zen-freeform-chrome S2): the top band (or,
// with nothing in it, the title strip), the layout host (one column per
// `layout.split` entry, each with its top and bottom edges and the widgets
// placed in it), then the bottom band. Where K2's controls sit is
// `page.placement` (`ZenBands.tsx`). Every widget gets its own
// `ZenWidgetBridge` built with the caps its source declared (Z33); K2's
// controls get the template bridge (no-cap verbs plus `agents.add` and
// `gardens:manage`, FC31). The page owns the control registry (Z27, G24:
// the Zen toggle and Garden switcher are checked, through their menu's
// button when they sit in a menu, FC25; the drag areas are bound only),
// and, outside safe mode, the check schedule (Z28). While it is on screen,
// ⌘1–9 opens row N of its first Agents widget (G52).
//
// S5 / S6 plug in without touching this file: widgets and chrome by kind
// (`zen-registry.tsx`), data verbs on the bridge (`registerZenVerb`), and
// the theme on the root (`registerZenThemeEngine`).
//
// A Garden with a nav rail draws the view its rail picked in this window
// (Rosson 2026-10-04: My Home, Agents, Projects, Tickets; `zen-rail-views`).
// The view changes the widgets only; the page's controls stay where they
// are (a one-column view moves column-edge items to column 0, FC48), so a
// view switch never fails the required-controls check. Arriving at My Home
// or Agents (a rail switch, or entering Zen) puts the caret in the selected
// agent's message box (Rosson 2026-10-04; `zen-compose-focus`).
//
// When the window is narrower than the columns' min widths, they scale
// down so every column (and every control at its edge) stays on screen
// (FC27, `zenColumnMinWidthCss`).
//
// Every column box is a shared Zen glass tile (`data-zen-glass`,
// `lib/zen/zen-glass.ts`; Rosson 2026-10-04).
//
// prd-garden-column-collapse A1: a `collapsible` column has a chevron in its
// top outer corner; folded, it is an edge tab (`ZenColumnEdgeTab`) and its
// widgets stay mounted but hidden, so a custom frame keeps running. Where
// each column is comes from `zen-columns` (this computer, per Garden: the
// person's click, a widget's unsaved `k2.layout.*` fold, the narrow-window
// fold). Folding moves the focus to the tab, the person's unfold to the
// column's first control, and a polite live region says which. The page
// hands its custom frames its own controller (`ZenColumnsContext`) and no
// other page's.

import { useEffect, useId, useMemo, useRef, useState } from 'react'
import { useHomesStore } from '@/stores/homes'
import { useWindowFocusStore } from '@/stores/window-focus'
import {
  createZenGarden,
  currentZenGardenId,
  deleteZenGarden,
  renameZenGarden,
  setZenGardenTemplate,
  switchZenGarden,
  useZenGardensStore,
} from '@/lib/zen/zen-gardens'
import { selectZenRowOnPage, ZEN_TEMPLATE_CONTROLS_ID } from '@/lib/zen/zen-data'
import { useZenWindowStore } from '@/lib/zen/zen-window'
import { zenPageForView, zenPageHasRail } from '@/lib/zen/zen-rail-views'
import { cancelZenComposeFocus, requestZenComposeFocus, takeZenEntered } from '@/lib/zen/zen-compose-focus'
import { ZEN_GLASS_PROPS } from '@/lib/zen/zen-glass'
import type { ZenAgentRow } from '@/lib/zen/zen-data'
import type { ZenRailView } from '@/lib/zen/zen-window'
import { createZenBridge, ZenBridgeError, type ZenBridgeHost, type ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import {
  checkZenControls,
  createControlRegistry,
  createControlStreak,
  type ZenControlCheck,
} from '@/lib/zen/zen-controls'
import {
  ZEN_CHECK_INTERVAL_MS,
  ZEN_CHECK_SECOND_MS,
  registerZenControlCheck,
  zenGeometry,
  zenReservedRects,
} from '@/lib/zen/zen-monitor'
import { zenBodyWidgets, zenFillReload, ZenFillReloadContext } from '@/lib/zen/zen-fill-reload'
import {
  zenMenuRequiredControls,
  type ZenResolvedPage,
  type ZenWidgetDecl,
} from '@/lib/zen/zen-page'
import { ZEN_COLUMN_TAB_PX, zenColumnMinWidthCss } from '@/lib/zen/zen-overflow'
import { useZenColumnFolds, zenColumnLabel, ZenColumnsContext } from '@/lib/zen/zen-columns'
import { ZEN_FULL_CANVAS_BAND_CSS, ZenColumnChevron, ZenColumnEdgeTab, zenColumnBadgeSources, zenColumnSide } from './ZenColumnEdgeTab'
import { exitZen, registerZenRowSelect, registerZenViewSwitch } from '@/lib/zen/zen-view'
import { ZEN_RAIL_KINDS, ZEN_TEMPLATE_CONTROL_CAPS, ZEN_UNBOXED_KINDS, zenWidgetFor } from './zen-registry'
import { zenConversationRow } from './widgets/ZenConversationWidget'
import { ZenWidgetStyles } from './widgets/zen-widget-kit'
import { ZenWidgetBoundary } from './ZenWidgetBoundary'
import { useZenWidgetsRunningMarker } from '@/lib/zen/zen-custom-run'
import { ZEN_CUSTOM_KIND, zenColumnFlex } from '@/lib/zen/zen-page'
import { ZEN_CHROME_CSS, ZenBottomBand, ZenEdgeRow, ZenTopBand, type ZenRowSource } from './ZenBands'
import { ZenNewGardenHost } from './widgets/ZenNewGardenModal'
import { ZenSourceAskHost, ZEN_SOURCE_ASK_CAPS, ZEN_SOURCE_ASK_ID } from './widgets/ZenSourceAsk'
import { ZenSourcesPending } from './widgets/ZenSourcesPending'

type ControlFailure = Extract<ZenControlCheck, { ok: false }>

/** What can take the focus when a column opens (its first control). */
const ZEN_FOCUSABLE =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), iframe, [tabindex]:not([tabindex="-1"])'

/** The live region's box: read aloud, never seen. */
const ZEN_VISUALLY_HIDDEN: React.CSSProperties = {
  position: 'absolute',
  width: 1,
  height: 1,
  padding: 0,
  margin: -1,
  overflow: 'hidden',
  clip: 'rect(0, 0, 0, 0)',
  whiteSpace: 'nowrap',
  border: 0,
}

/** How long the window must stop resizing before the check runs (ms). */
export const ZEN_RESIZE_SETTLE_MS = 300

function homeSummaries(): Array<{ id: string; name: string }> {
  return useHomesStore.getState().homes.map((h) => ({ id: h.id, name: h.name }))
}

function gardenSummaries(): Array<{ id: string; name: string; index: number }> {
  return useZenGardensStore.getState().gardens.map((g, i) => ({ id: g.id, name: g.name, index: i + 1 }))
}

function gardenIds(): string[] {
  return useZenGardensStore.getState().gardens.map((g) => g.id)
}

/** The agent whose message box an arrival focuses: the row the page's first
 *  Conversation (with a message box) shows, or null when none is selected. */
export function zenSelectedConversationAddress(
  page: ZenResolvedPage,
  bridges: ReadonlyMap<string, ZenWidgetBridge>,
): string | null {
  for (const w of page.widgets) {
    if (w.kind !== 'conversation' || w.props.compose === false) continue
    const bridge = bridges.get(w.id)
    if (!bridge) throw new Error(`zen page: no bridge for widget ${w.id}`)
    // A conversation that can't read agents (not granted, or no data verbs
    // installed) has no row to focus; the caret never crashes the page.
    if (!bridge.caps.has('agents:read')) continue
    let rows: ZenAgentRow[]
    try {
      rows = bridge.call('agents.list') as ZenAgentRow[]
    } catch (err) {
      if (err instanceof ZenBridgeError && err.code === 'verb_unavailable') continue
      throw err
    }
    const row = zenConversationRow(rows, w.props)
    if (row) return row.address
  }
  return null
}

/** Views whose page has the conversation an arrival focuses. */
const ZEN_CONVERSATION_VIEWS: ReadonlySet<ZenRailView> = new Set(['home', 'agents'])

export function ZenPage({
  page: garden,
  safe,
  banner,
  onControlFailure,
}: {
  page: ZenResolvedPage
  /** Safe mode: the built-in page, no control checks. */
  safe: boolean
  /** K2's banner (safe mode, config error), under the template's band. */
  banner: React.ReactNode
  onControlFailure(failure: ControlFailure): void
}): React.JSX.Element {
  // Re-render on Garden changes: the template's switcher lists them.
  useZenGardensStore((s) => s.gardens)
  useZenWindowStore((s) => s.garden)
  // The rail's view in this window: the Garden's page, or a K2 view drawn
  // inside it (the same controls either way).
  const picked = useZenWindowStore((s) => s.view)
  const view = zenPageHasRail(garden) ? picked : 'home'
  const page = useMemo(() => zenPageForView(garden, view), [garden, view])
  const rootRef = useRef<HTMLDivElement | null>(null)
  const pageRef = useRef(page)
  pageRef.current = page
  const failRef = useRef(onControlFailure)
  failRef.current = onControlFailure

  const registry = useMemo(
    () =>
      createControlRegistry({
        exit: () => exitZen(),
        selectGarden: (id) => switchZenGarden(id),
        gardenIds,
        // FC25: what a menu must bind after its button is activated.
        menuHolds: (id) => zenMenuRequiredControls(pageRef.current.placement, id),
      }),
    [],
  )
  useEffect(() => () => registry.dispose(), [registry])
  // A pick's pending caret never outlives the page (a Garden switch, exit).
  useEffect(() => () => cancelZenComposeFocus(), [])

  const host = useMemo<ZenBridgeHost>(
    () => ({
      gardens: gardenSummaries,
      currentGardenId: () => currentZenGardenId() ?? '',
      switchGarden: (id) => switchZenGarden(id),
      createGarden: async (name, template, opts) => {
        const g = await createZenGarden(name, template, opts)
        return { id: g.id, name: g.name, index: g.index }
      },
      useGardenTemplate: (id, template, force) => setZenGardenTemplate(id, template, force),
      renameGarden: (id, name) => renameZenGarden(id, name),
      deleteGarden: (id) => deleteZenGarden(id),
      homes: homeSummaries,
      exit: () => exitZen(),
      controls: registry,
      page: () => pageRef.current,
    }),
    [registry],
  )

  const controlsBridge = useMemo(
    () => createZenBridge(host, { id: ZEN_TEMPLATE_CONTROLS_ID, caps: ZEN_TEMPLATE_CONTROL_CAPS }),
    [host],
  )
  // Garden v3: K2's own Ask box for data-source cards (never a widget's bridge).
  const sourceAskBridge = useMemo(() => createZenBridge(host, { id: ZEN_SOURCE_ASK_ID, caps: ZEN_SOURCE_ASK_CAPS }), [host])
  // G52: ⌘1–9 / ⌘0 opens row N of this page's first Agents widget.
  useEffect(
    () => registerZenRowSelect((index) => selectZenRowOnPage(pageRef.current, currentZenGardenId() ?? '', index)),
    [],
  )
  // 0.46.0: ⌃⌘1–4 (Ctrl+Alt+1–4) switch the rail's view, as a rail click
  // does. A Garden without a rail has no views: the chord does nothing.
  // (⌃⌘5 Wiki and ⌃⌘6 Settings never reach here: lib/page-shortcuts.ts.)
  const gardenRef = useRef(garden)
  gardenRef.current = garden
  useEffect(
    () =>
      registerZenViewSwitch((v) => {
        if (zenPageHasRail(gardenRef.current)) useZenWindowStore.getState().setView(v)
      }),
    [],
  )
  const widgetBridges = useMemo(() => {
    const m = new Map<string, ZenWidgetBridge>()
    for (const w of page.widgets) m.set(w.id, createZenBridge(host, { id: w.id, caps: w.caps }))
    return m
  }, [host, page.widgets])
  const bridgesRef = useRef(widgetBridges)
  bridgesRef.current = widgetBridges
  // UW32: while this Garden's custom widgets are mounted, the window
  // remembers it (a freeze then restarts them paused). Not in safe mode.
  useZenWidgetsRunningMarker(
    garden.garden?.id ?? currentZenGardenId() ?? '',
    !safe && page.widgets.some((w) => w.kind === ZEN_CUSTOM_KIND),
  )

  // Arriving at My Home / Agents (a rail switch, or this window entering
  // Zen): focus the selected agent's message box once it can be typed in.
  // A Garden switch, a page reload or a remote update is not an arrival.
  const arrivedView = useRef<ZenRailView | null>(null)
  useEffect(() => {
    const prev = arrivedView.current
    arrivedView.current = view
    const arrived = prev === null ? takeZenEntered() : prev !== view
    if (!arrived || !ZEN_CONVERSATION_VIEWS.has(view)) return
    const address = zenSelectedConversationAddress(pageRef.current, bridgesRef.current)
    if (address) requestZenComposeFocus(address)
  }, [view])

  // Z28: the check schedule (not in safe mode: K2's own page needs none).
  const focused = useWindowFocusStore((s) => s.isFocused)
  const streak = useMemo(() => createControlStreak((f) => failRef.current(f)), [])
  const runCheck = useMemo(
    () => () =>
      streak.record(
        checkZenControls({
          declared: pageRef.current.controls,
          registry,
          geometry: zenGeometry(),
          reserved: zenReservedRects(),
          gardenIds: gardenIds(),
          root: rootRef.current,
          // FC25: a control in a closed menu is checked through its button.
          placement: pageRef.current.placement,
        }),
      ),
    [registry, streak],
  )
  useEffect(() => {
    if (safe) return
    return registerZenControlCheck(runCheck)
  }, [safe, runCheck])
  // First paint and 1.5 s later, again for every new page version (the
  // Garden's: a rail view switch is not a new page).
  useEffect(() => {
    if (safe) return
    let raf: number | null = null
    if (typeof requestAnimationFrame === 'function') raf = requestAnimationFrame(() => runCheck())
    const second = setTimeout(runCheck, ZEN_CHECK_SECOND_MS)
    return () => {
      if (raf !== null && typeof cancelAnimationFrame === 'function') cancelAnimationFrame(raf)
      clearTimeout(second)
    }
  }, [safe, runCheck, garden.version])
  // After a resize settles: a burst of resize events mid-animation (full
  // screen, a window snap) is many frames, not two separate checks.
  useEffect(() => {
    if (safe) return
    let timer: ReturnType<typeof setTimeout> | null = null
    const onResize = (): void => {
      if (timer !== null) clearTimeout(timer)
      timer = setTimeout(() => {
        timer = null
        runCheck()
      }, ZEN_RESIZE_SETTLE_MS)
    }
    window.addEventListener('resize', onResize)
    return () => {
      if (timer !== null) clearTimeout(timer)
      window.removeEventListener('resize', onResize)
    }
  }, [safe, runCheck])
  useEffect(() => {
    if (safe || !focused) return
    const id = setInterval(runCheck, ZEN_CHECK_INTERVAL_MS)
    return () => clearInterval(id)
  }, [safe, focused, runCheck])

  const { layout, placement } = page
  // prd-garden-column-collapse A1: where each column is (this computer, per
  // Garden), the narrow-window fold, and the page's `k2.layout.*` controller.
  const layoutRef = useRef<HTMLDivElement | null>(null)
  const foldGarden = garden.garden?.id ?? currentZenGardenId() ?? ''
  const cols = useZenColumnFolds(foldGarden, layout, layoutRef)
  const foldIds = useId()
  const tabRefs = useRef(new Map<number, HTMLButtonElement>())
  const slotRefs = useRef(new Map<number, HTMLDivElement>())
  const pendingFocus = useRef<{ index: number; to: 'tab' | 'body' } | null>(null)
  const [announce, setAnnounce] = useState('')
  const foldsRef = useRef(layout.folds)
  foldsRef.current = layout.folds
  const onMove = cols.onMove
  useEffect(() => {
    onMove((index, collapsed, by) => {
      const label = zenColumnLabel(foldsRef.current, index)
      setAnnounce(collapsed ? `${label} collapsed` : `${label} expanded`)
      const slot = slotRefs.current.get(index)
      const inside = !!slot && typeof document !== 'undefined' && slot.contains(document.activeElement)
      // Folding moves the focus from inside the column to its tab; the
      // person's unfold moves it to the column's first control.
      if (collapsed && inside) pendingFocus.current = { index, to: 'tab' }
      else if (!collapsed && by === 'person') pendingFocus.current = { index, to: 'body' }
    })
    return () => onMove(null)
  }, [onMove])
  useEffect(() => {
    const p = pendingFocus.current
    if (!p) return
    if (p.to === 'tab' && !cols.collapsed[p.index]) return
    if (p.to === 'body' && cols.collapsed[p.index]) return
    pendingFocus.current = null
    if (p.to === 'tab') {
      tabRefs.current.get(p.index)?.focus()
      return
    }
    const body = document.getElementById(`${foldIds}-col-${p.index}`)
    const first = body?.querySelector<HTMLElement>(ZEN_FOCUSABLE)
    const target = first ?? slotRefs.current.get(p.index)?.querySelector<HTMLElement>('[data-zen-column-chevron]')
    target?.focus()
  })
  const draw = (w: ZenWidgetDecl): React.JSX.Element => {
    const Widget = zenWidgetFor(w.kind)
    const bridge = widgetBridges.get(w.id)
    if (!bridge) throw new Error(`zen page: no bridge for widget ${w.id}`)
    // UW31: each widget crashes alone (its own card), never the page.
    return (
      <ZenWidgetBoundary key={w.id} widgetId={w.id} kind={w.kind}>
        <Widget decl={w} bridge={bridge} />
      </ZenWidgetBoundary>
    )
  }
  // Content widgets the rows draw (a band, a column edge) by id, with
  // their own bridges. Everything else fills its column's body.
  const byId = new Map(page.widgets.map((w) => [w.id, w]))
  const items = new Map(placement.items.map((i) => [i.id, i]))
  const src: ZenRowSource = {
    widget: (id) => byId.get(id) ?? null,
    drawWidget: draw,
    item: (id) => items.get(id) ?? null,
    menuItems: (id) => (placement.menus[id] ?? []).flatMap((m) => items.get(m) ?? []),
    bridge: controlsBridge,
  }
  const bodyWidgets = zenBodyWidgets(page)
  // `[layout] canvas = "full"` (the Diary): the page is the whole window.
  const full = layout.canvas === 'full'
  // The rows a folded column's tab counts (memoized: a new list resubscribes).
  const badgeSources = useMemo(
    () =>
      layout.split.map((_, col) =>
        layout.folds?.[col]?.collapsible ? zenColumnBadgeSources(page.widgets.filter((w) => w.column === col), widgetBridges) : [],
      ),
    [layout, page.widgets, widgetBridges],
  )
  // A custom widget that fills a full canvas: its Reload goes in the
  // Garden's menu, not a floating ⋯ (0.45.2; `zen-fill-reload`).
  const fillReload = zenFillReload(page)
  return (
    <ZenFillReloadContext.Provider value={fillReload}>
      <ZenColumnsContext.Provider value={cols.controller}>
        <div
          ref={rootRef}
          className="relative flex h-full min-h-0 w-full flex-col"
          data-zen-page={page.template}
          data-zen-view={view}
          data-zen-chrome-from={placement.from}
          data-zen-canvas={full ? 'full' : undefined}
        >
          <ZenWidgetStyles />
          <style data-zen-chrome-styles="">{ZEN_CHROME_CSS}</style>
          <ZenTopBand groups={placement.bands.top} src={src} float={full} />
          {banner}
          <div
            ref={layoutRef}
            className="flex min-h-0 min-w-0 flex-1"
            data-zen-layout={layout.kind}
            style={{
              gap: full ? 0 : 'var(--zen-gap)',
              padding: full ? 0 : placement.bands.bottom ? '0 var(--zen-gap)' : '0 var(--zen-gap) var(--zen-gap)',
            }}
          >
            {layout.split.map((pct, col) => {
              const inCol = bodyWidgets.filter((w) => w.column === col)
              const rails = inCol.filter((w) => ZEN_RAIL_KINDS.has(w.kind))
              const boxed = inCol.filter((w) => !ZEN_RAIL_KINDS.has(w.kind))
              const boxedFlex = zenColumnFlex(boxed)
              // K2 views that draw their own panels (Tickets' glass) get no box;
              // nor does anything on a full canvas (it draws its own surface).
              const bare = full || (boxed.length > 0 && boxed.every((w) => ZEN_UNBOXED_KINDS.has(w.kind)))
              // 0.46.4 (Rosson 2026-10-23): widgets stacked in a column are
              // separate glass tiles with the theme's gap between (`weight`
              // still shares the height); `joined = true` keeps one panel.
              // One widget, a bare column or a full canvas: as before.
              const tiles = !bare && boxed.length > 1 && layout.joined?.[col] !== true
              // On a full canvas the top band floats over every column. A
              // custom widget makes room for it itself (and cuts holes in
              // it); K2's own widgets (the Kanban's Thread, 0.46.4) start
              // below it, as the column tabs do.
              const underBand = full && boxed.length > 0 && !boxed.some((w) => w.kind === ZEN_CUSTOM_KIND)
              // FC9: a column's edges (a row rail, a Zen control) run across its
              // top and bottom, outside its box.
              const top = placement.edges.find((e) => e.column === col && e.edge === 'top')
              const bottom = placement.edges.find((e) => e.column === col && e.edge === 'bottom')
              const body = (
                <>
                  {rails.map(draw)}
                  {tiles && (
                    <div
                      data-zen-column={col}
                      data-zen-column-tiles=""
                      className="flex min-h-0 min-w-0 flex-col"
                      style={{ flex: '1 1 0%', gap: 'var(--zen-gap)' }}
                    >
                      {boxed.map((w) => (
                        <div
                          key={w.id}
                          data-zen-tile={w.id}
                          data-zen-weight={boxedFlex === null ? undefined : (w.weight ?? 1)}
                          {...ZEN_GLASS_PROPS}
                          className="flex min-h-0 min-w-0 flex-col"
                          style={{ flex: boxedFlex?.get(w.id) ?? '1 1 0%', borderRadius: 'var(--zen-radius)', overflow: 'hidden' }}
                        >
                          {draw(w)}
                        </div>
                      ))}
                    </div>
                  )}
                  {!tiles && (boxed.length > 0 || rails.length === 0) && (
                    <div
                      data-zen-column={col}
                      className="flex min-h-0 min-w-0 flex-col"
                      data-zen-column-bare={bare ? '' : undefined}
                      data-zen-column-under-band={underBand ? '' : undefined}
                      {...(bare ? {} : ZEN_GLASS_PROPS)}
                      style={
                        bare
                          ? { flex: '1 1 0%', ...(underBand ? { paddingTop: ZEN_FULL_CANVAS_BAND_CSS } : {}) }
                          : { flex: '1 1 0%', borderRadius: 'var(--zen-radius)', overflow: 'hidden' }
                      }
                    >
                      {boxedFlex === null
                        ? boxed.map(draw)
                        : boxed.map((w) => (
                            // 0.46.1 (gap 6): `weight` shares the column's height.
                            <div
                              key={w.id}
                              data-zen-weight={w.weight ?? 1}
                              className="flex min-h-0 min-w-0 flex-col"
                              style={{ flex: boxedFlex.get(w.id) }}
                            >
                              {draw(w)}
                            </div>
                          ))}
                    </div>
                  )}
                </>
              )
              // FC27: min widths scale down when they don't all fit (A1: a
              // folded column takes no share and reserves its tab).
              const slotStyle = {
                flex: `${pct} 1 0%`,
                minWidth: zenColumnMinWidthCss(layout.minWidths, col, cols.collapsed),
                gap: 'var(--zen-gap)',
              }
              if (layout.folds?.[col]?.collapsible) {
                // A1: one stable tree whether folded or open, so folding never
                // remounts the column's widgets (a custom frame keeps running).
                const folded = cols.collapsed[col] === true
                const label = zenColumnLabel(layout.folds, col)
                const side = zenColumnSide(col, layout.split.length)
                const bodyId = `${foldIds}-col-${col}`
                return (
                  <div
                    key={col}
                    ref={(el) => {
                      if (el) slotRefs.current.set(col, el)
                      else slotRefs.current.delete(col)
                    }}
                    data-zen-column-slot={col}
                    data-zen-column-collapsible=""
                    data-zen-column-folded={folded ? (cols.auto[col] ? 'auto' : '') : undefined}
                    className="relative flex min-h-0 min-w-0 flex-row"
                    style={
                      folded
                        ? { flex: `0 0 ${ZEN_COLUMN_TAB_PX}px`, width: ZEN_COLUMN_TAB_PX, minWidth: ZEN_COLUMN_TAB_PX }
                        : slotStyle
                    }
                  >
                    {folded ? (
                      <ZenColumnEdgeTab
                        index={col}
                        label={label}
                        side={side}
                        controls={bodyId}
                        sources={badgeSources[col]}
                        full={full}
                        onExpand={() => cols.personExpand(col)}
                        buttonRef={(el) => {
                          if (el) tabRefs.current.set(col, el)
                          else tabRefs.current.delete(col)
                        }}
                      />
                    ) : null}
                    <div
                      id={bodyId}
                      data-zen-column-region={col}
                      aria-label={label}
                      role="region"
                      className={`min-h-0 min-w-0 flex-1 ${top || bottom ? 'flex-col' : 'flex-row'}`}
                      style={{ display: folded ? 'none' : 'flex', gap: 'var(--zen-gap)' }}
                    >
                      {top && <ZenEdgeRow edge={top} src={src} />}
                      {top || bottom ? (
                        <div data-zen-column-body={col} className="flex min-h-0 min-w-0 flex-1 flex-row" style={{ gap: 'var(--zen-gap)' }}>
                          {body}
                        </div>
                      ) : (
                        body
                      )}
                      {bottom && <ZenEdgeRow edge={bottom} src={src} />}
                    </div>
                    {folded ? null : (
                      <ZenColumnChevron
                        index={col}
                        label={label}
                        side={side}
                        controls={bodyId}
                        disabled={!cols.canFold(col)}
                        full={full}
                        onFold={() => cols.personFold(col)}
                      />
                    )}
                  </div>
                )
              }
              if (!top && !bottom) {
                return (
                  <div key={col} data-zen-column-slot={col} className="flex min-h-0 min-w-0 flex-row" style={slotStyle}>
                    {body}
                  </div>
                )
              }
              return (
                <div key={col} data-zen-column-slot={col} className="flex min-h-0 min-w-0 flex-col" style={slotStyle}>
                  {top && <ZenEdgeRow edge={top} src={src} />}
                  <div data-zen-column-body={col} className="flex min-h-0 min-w-0 flex-1 flex-row" style={{ gap: 'var(--zen-gap)' }}>
                    {body}
                  </div>
                  {bottom && <ZenEdgeRow edge={bottom} src={src} />}
                </div>
              )
            })}
          </div>
          {placement.bands.bottom && <ZenBottomBand groups={placement.bands.bottom} src={src} />}
          <ZenNewGardenHost bridge={controlsBridge} />
          <ZenSourceAskHost bridge={sourceAskBridge} />
          <ZenSourcesPending gardenName={garden.garden?.name ?? 'this Garden'} widgets={page.widgets} />
          {layout.folds && (
            <div data-zen-column-announce="" aria-live="polite" role="status" style={ZEN_VISUALLY_HIDDEN}>
              {announce}
            </div>
          )}
        </div>
      </ZenColumnsContext.Provider>
    </ZenFillReloadContext.Provider>
  )
}
