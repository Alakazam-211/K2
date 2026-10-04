// prd-zen-mode-v1 Z10, Z27, Z28, Z33 — one resolved Zen page on screen.
//
// Draws the template's controls, then the layout host: one column per
// `layout.split` entry, each holding the widgets placed in it. Every widget
// gets its own `ZenWidgetBridge` built with the caps its source declared
// (Z33); the template's controls get the template bridge (no-cap verbs plus
// `agents.add`). The template's footer, if any, sits under the first column
// (the bottom-left corner). The page owns the control registry (Z27) and,
// outside safe mode, the check schedule (Z28).
//
// S5 / S6 plug in without touching this file: widgets by kind and template
// controls by template id (`zen-registry.tsx`), data verbs on the bridge
// (`registerZenVerb`), and the theme on the root (`registerZenThemeEngine`).

import { useEffect, useMemo, useRef } from 'react'
import { useHomesStore, selectedHome } from '@/stores/homes'
import { useWindowFocusStore } from '@/stores/window-focus'
import { createZenBridge, type ZenBridgeHost, type ZenWidgetBridge } from '@/lib/zen/zen-bridge'
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
import type { ZenResolvedPage } from '@/lib/zen/zen-page'
import { exitZen } from '@/lib/zen/zen-view'
import { ZEN_TEMPLATE_CONTROL_CAPS, zenTemplateControlsFor, zenTemplateFooterFor, zenWidgetFor } from './zen-registry'

type ControlFailure = Extract<ZenControlCheck, { ok: false }>

function homeSummaries(): Array<{ id: string; name: string }> {
  return useHomesStore.getState().homes.map((h) => ({ id: h.id, name: h.name }))
}

export function ZenPage({
  page,
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
  // Re-render on Home changes: the template's switcher lists them.
  useHomesStore((s) => s.homes)
  useHomesStore((s) => s.selectedId)
  const rootRef = useRef<HTMLDivElement | null>(null)
  const pageRef = useRef(page)
  pageRef.current = page
  const failRef = useRef(onControlFailure)
  failRef.current = onControlFailure

  const registry = useMemo(
    () =>
      createControlRegistry({
        exit: () => exitZen(),
        selectHome: (id) => useHomesStore.getState().selectHome(id),
        homeIds: () => useHomesStore.getState().homes.map((h) => h.id),
      }),
    [],
  )
  useEffect(() => () => registry.dispose(), [registry])

  const host = useMemo<ZenBridgeHost>(
    () => ({
      homes: homeSummaries,
      selectedHomeId: () => selectedHome(useHomesStore.getState()).id,
      selectHome: (id) => useHomesStore.getState().selectHome(id),
      exit: () => exitZen(),
      controls: registry,
      page: () => pageRef.current,
    }),
    [registry],
  )

  const controlsBridge = useMemo(() => createZenBridge(host, { id: 'template-controls', caps: ZEN_TEMPLATE_CONTROL_CAPS }), [host])
  const widgetBridges = useMemo(() => {
    const m = new Map<string, ZenWidgetBridge>()
    for (const w of page.widgets) m.set(w.id, createZenBridge(host, { id: w.id, caps: w.caps }))
    return m
  }, [host, page.widgets])

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
          homeIds: useHomesStore.getState().homes.map((h) => h.id),
          root: rootRef.current,
        }),
      ),
    [registry, streak],
  )
  useEffect(() => {
    if (safe) return
    return registerZenControlCheck(runCheck)
  }, [safe, runCheck])
  // First paint and 1.5 s later, again for every new page version.
  useEffect(() => {
    if (safe) return
    let raf: number | null = null
    if (typeof requestAnimationFrame === 'function') raf = requestAnimationFrame(() => runCheck())
    const second = setTimeout(runCheck, ZEN_CHECK_SECOND_MS)
    return () => {
      if (raf !== null && typeof cancelAnimationFrame === 'function') cancelAnimationFrame(raf)
      clearTimeout(second)
    }
  }, [safe, runCheck, page.version])
  useEffect(() => {
    if (safe) return
    const onResize = (): void => runCheck()
    window.addEventListener('resize', onResize)
    return () => window.removeEventListener('resize', onResize)
  }, [safe, runCheck])
  useEffect(() => {
    if (safe || !focused) return
    const id = setInterval(runCheck, ZEN_CHECK_INTERVAL_MS)
    return () => clearInterval(id)
  }, [safe, focused, runCheck])

  const Controls = zenTemplateControlsFor(page.template)
  const Footer = zenTemplateFooterFor(page.template)
  const { layout } = page
  return (
    <div ref={rootRef} className="flex h-full min-h-0 w-full flex-col" data-zen-page={page.template}>
      {Controls && <Controls bridge={controlsBridge} />}
      {banner}
      <div
        className="flex min-h-0 flex-1"
        data-zen-layout={layout.kind}
        style={{ gap: 'var(--zen-gap)', padding: '0 var(--zen-gap) var(--zen-gap)' }}
      >
        {layout.split.map((pct, col) => (
          <div
            key={col}
            data-zen-column-slot={col}
            className="flex min-h-0 min-w-0 flex-col"
            style={{ flex: `${pct} 1 0%`, minWidth: layout.minWidths[col] ?? 0 }}
          >
            <div
              data-zen-column={col}
              className="flex min-h-0 min-w-0 flex-col"
              style={{
                flex: '1 1 0%',
                background: 'var(--zen-surface)',
                border: '1px solid var(--zen-border)',
                borderRadius: 'var(--zen-radius)',
                overflow: 'hidden',
              }}
            >
              {page.widgets
                .filter((w) => w.column === col)
                .map((w) => {
                  const Widget = zenWidgetFor(w.kind)
                  const bridge = widgetBridges.get(w.id)
                  if (!bridge) throw new Error(`zen page: no bridge for widget ${w.id}`)
                  return <Widget key={w.id} decl={w} bridge={bridge} />
                })}
            </div>
            {col === 0 && Footer && <Footer bridge={controlsBridge} />}
          </div>
        ))}
      </div>
    </div>
  )
}
