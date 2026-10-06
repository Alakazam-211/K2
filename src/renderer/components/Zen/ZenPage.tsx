// prd-zen-mode-v1 Z10, Z27, Z28, Z33 and prd-zen-gardens-v1 G24, G29, G38,
// G52 — one Garden's resolved page on screen.
//
// Draws the template's controls, then the layout host: one column per
// `layout.split` entry, each holding the widgets placed in it. Every widget
// gets its own `ZenWidgetBridge` built with the caps its source declared
// (Z33); the template's controls get the template bridge (no-cap verbs plus
// `agents.add` and `gardens:manage`). The page owns the control registry
// (Z27, G24: the Zen toggle and Garden switcher are checked; the drag area
// is bound only), and, outside safe mode, the check schedule (Z28). While it is on screen, ⌘1–9
// opens row N of its first Agents widget (G52).
//
// S5 / S6 plug in without touching this file: widgets by kind and template
// controls by template id (`zen-registry.tsx`), data verbs on the bridge
// (`registerZenVerb`), and the theme on the root (`registerZenThemeEngine`).
//
// A Garden with a nav rail draws the view its rail picked in this window
// (Rosson 2026-10-04: My Home, Agents, Projects, Tickets; `zen-rail-views`).
// The view changes the widgets only; the template's controls, and so the
// required-controls check, stay the same. Arriving at My Home or Agents (a
// rail switch, or entering Zen) puts the caret in the selected agent's
// message box (Rosson 2026-10-04; `zen-compose-focus`).
//
// A widget with a band slot (`[[widget]] slot = "top"`, Rosson 2026-10-06)
// is drawn here too, with its own bridge, and handed to the template's
// `ZenBandSlot` (`ZenBand.tsx`) instead of a column; the template's
// controls stay its own, so the required-controls check is unchanged.
//
// Every column box is a shared Zen glass tile (`data-zen-glass`,
// `lib/zen/zen-glass.ts`; Rosson 2026-10-04).

import { useEffect, useMemo, useRef } from 'react'
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
import { zenPageForView, zenPageHasRail, zenRailOrientation } from '@/lib/zen/zen-rail-views'
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
import { zenWidgetInBand, zenWidgetSlot, type ZenResolvedPage, type ZenWidgetDecl } from '@/lib/zen/zen-page'
import { exitZen, registerZenRowSelect } from '@/lib/zen/zen-view'
import {
  ZEN_RAIL_KINDS,
  ZEN_TEMPLATE_CONTROL_CAPS,
  ZEN_UNBOXED_KINDS,
  zenTemplateControlsFor,
  zenWidgetFor,
} from './zen-registry'
import { zenConversationRow } from './widgets/ZenConversationWidget'
import { ZenBandContext } from './ZenBand'

type ControlFailure = Extract<ZenControlCheck, { ok: false }>

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
  // G52: ⌘1–9 / ⌘0 opens row N of this page's first Agents widget.
  useEffect(
    () => registerZenRowSelect((index) => selectZenRowOnPage(pageRef.current, currentZenGardenId() ?? '', index)),
    [],
  )
  const widgetBridges = useMemo(() => {
    const m = new Map<string, ZenWidgetBridge>()
    for (const w of page.widgets) m.set(w.id, createZenBridge(host, { id: w.id, caps: w.caps }))
    return m
  }, [host, page.widgets])
  const bridgesRef = useRef(widgetBridges)
  bridgesRef.current = widgetBridges

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

  const Controls = zenTemplateControlsFor(page.template)
  const { layout } = page
  const draw = (w: ZenWidgetDecl): React.JSX.Element => {
    const Widget = zenWidgetFor(w.kind)
    const bridge = widgetBridges.get(w.id)
    if (!bridge) throw new Error(`zen page: no bridge for widget ${w.id}`)
    return <Widget key={w.id} decl={w} bridge={bridge} />
  }
  // Band widgets (`slot = "top"`, Rosson 2026-10-06): drawn here with their
  // own bridges, placed by the template's `ZenBandSlot`.
  const bands = new Map<string, React.ReactNode>()
  for (const w of page.widgets) {
    if (!zenWidgetInBand(w)) continue
    const slot = zenWidgetSlot(w)
    bands.set(slot, [...((bands.get(slot) as React.ReactNode[] | undefined) ?? []), draw(w)])
  }
  const columnWidgets = page.widgets.filter((w) => !zenWidgetInBand(w))
  return (
    <div ref={rootRef} className="flex h-full min-h-0 w-full flex-col" data-zen-page={page.template} data-zen-view={view}>
      {Controls && (
        <ZenBandContext.Provider value={bands}>
          <Controls bridge={controlsBridge} />
        </ZenBandContext.Provider>
      )}
      {banner}
      <div
        className="flex min-h-0 flex-1"
        data-zen-layout={layout.kind}
        style={{ gap: 'var(--zen-gap)', padding: '0 var(--zen-gap) var(--zen-gap)' }}
      >
        {layout.split.map((pct, col) => {
          const inCol = columnWidgets.filter((w) => w.column === col)
          const rails = inCol.filter((w) => ZEN_RAIL_KINDS.has(w.kind))
          const boxed = inCol.filter((w) => !ZEN_RAIL_KINDS.has(w.kind))
          // A rail drawn as a row sits across the top of its column instead
          // of down its left edge.
          const rowRail = rails.some((w) => zenRailOrientation(w) === 'row')
          // K2 views that draw their own panels (Tickets' glass) get no box.
          const bare = boxed.length > 0 && boxed.every((w) => ZEN_UNBOXED_KINDS.has(w.kind))
          return (
            <div
              key={col}
              data-zen-column-slot={col}
              className={`flex min-h-0 min-w-0 ${rowRail ? 'flex-col' : 'flex-row'}`}
              style={{ flex: `${pct} 1 0%`, minWidth: layout.minWidths[col] ?? 0, gap: 'var(--zen-gap)' }}
            >
              {rails.map(draw)}
              {(boxed.length > 0 || rails.length === 0) && (
                <div
                  data-zen-column={col}
                  className="flex min-h-0 min-w-0 flex-col"
                  data-zen-column-bare={bare ? '' : undefined}
                  {...(bare ? {} : ZEN_GLASS_PROPS)}
                  style={
                    bare
                      ? { flex: '1 1 0%' }
                      : { flex: '1 1 0%', borderRadius: 'var(--zen-radius)', overflow: 'hidden' }
                  }
                >
                  {boxed.map(draw)}
                </div>
              )}
            </div>
          )
        })}
      </div>
    </div>
  )
}
