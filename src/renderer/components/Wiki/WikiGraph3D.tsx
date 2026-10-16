// Wiki 3D map (prd-wiki-3d-view-v1 §3, §6.3). LAZY: WikiPage loads this
// chunk only when the 3D view opens; it carries react-force-graph-3d and
// shares `three` with the CAD viewer.
//
// Same data as 2D (wiki-graph-data.ts) with its OWN sim map (W9). On top
// of the library we do: CAD-parity orbit controls (W8), render on demand
// (the library renders every frame forever, W11), label level of detail,
// the glow halo, theme tokens with alpha + light palettes (W13), an fps
// watchdog, and a dispose that hands the GL context back (W10).

import React, { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import ForceGraph3D, { type ForceGraphMethods, type NodeObject } from 'react-force-graph-3d'
import type { OrbitControls } from 'three/addons/controls/OrbitControls.js'
import type { WebGLRenderer, Scene } from 'three'
import type { WikiIndex } from './wiki-api'
import {
  isWikiFocusGroupNode,
  isWikiHomeNode,
  isWikiProjectNode,
  isWikiWorkspaceHubNode,
} from './wiki-api'
import {
  buildWikiGraphData,
  localCenterIdFor,
  type WikiGraphLink,
  type WikiSimNode,
} from './wiki-graph-data'
import {
  flyToPosition,
  nodeVisual,
  parseCssColor,
  pickLabelIds,
  readWiki3dTheme,
  rgbaCss,
  themeKey,
  Wiki3dScene,
  type NodeKind,
  type Vec3,
} from './wiki-3d-scene'
import { classifyFps, FPS_SAMPLE_MS, type Wiki3dFallbackReason } from './wiki-3d-guard'

type Node3 = WikiSimNode
type Link3 = WikiGraphLink
type FG = ForceGraphMethods<NodeObject<Node3>, Link3>

/** Pause the render loop this long after the layout settles and input stops. */
export const IDLE_PAUSE_MS = 1000
/** Fly-to duration and distance from the node (graph units). */
export const FLY_MS = 800
export const FLY_DISTANCE = 140
/** Search framing waits for typing to stop. */
export const SEARCH_FIT_DEBOUNCE_MS = 300
/** Labels: nearest-N cap (15 after the degrade step), refresh rate. */
export const LABEL_MAX = 40
export const LABEL_MAX_DEGRADED = 15
const LABEL_THROTTLE_MS = 250

export const CANVAS_LABEL = 'Wiki graph, 3D. Use the search box and note panel to read notes.'
export const HINT_TEXT =
  'Drag to orbit · right-drag or Shift-drag to pan · scroll to zoom · double-click to reset'

/** Labels are our sprites; the library's hover tooltip stays empty. */
const NO_TOOLTIP = (): string => ''

export type WikiFocusRequest = { id: string; seq: number }

export type WikiGraph3DProps = {
  index: WikiIndex
  selectedId: string | null
  search: string
  mode: 'k2' | 'local' | 'global'
  k2Lens?: 'projects' | 'groups'
  focusGroupFilter?: string
  projectFilter?: string
  onSelect: (id: string | null) => void
  reducedMotion: boolean
  /** Bumped by a wikilink click in the note panel: fly to that note. */
  focusRequest: WikiFocusRequest | null
  /** 3D can't keep going here (slow frames, context lost): WikiPage switches to 2D. */
  onFallback: (reason: Extract<Wiki3dFallbackReason, 'slow' | 'crashed'>) => void
}

function endId(x: string | Node3): string {
  return typeof x === 'object' ? String(x.id) : String(x)
}

function nodeKind(n: Node3): NodeKind {
  const missing = !!n.__missing || !n.exists
  const org = isWikiFocusGroupNode(n) || isWikiProjectNode(n)
  const hub = isWikiWorkspaceHubNode(n)
  const home = !missing && isWikiHomeNode(n)
  const orgColor = isWikiProjectNode(n) ? n.projectColor || null : n.focusGroupColor || null
  return { missing, org, hub, home, orgColor }
}

function posOf(n: Node3 | undefined): Vec3 | null {
  if (!n || n.x == null || n.y == null || n.z == null) return null
  return { x: n.x, y: n.y, z: n.z }
}

export default function WikiGraph3D({
  index,
  selectedId,
  search,
  mode,
  k2Lens = 'groups',
  focusGroupFilter = 'all',
  projectFilter = 'all',
  onSelect,
  reducedMotion,
  focusRequest,
  onFallback,
}: WikiGraph3DProps): React.JSX.Element {
  const containerRef = useRef<HTMLDivElement>(null)
  const fgRef = useRef<FG | undefined>(undefined)
  const [size, setSize] = useState({ w: 400, h: 400 })
  const simNodesRef = useRef<Map<string, Node3>>(new Map())

  // three objects we own (theme read once here, re-read on palette changes).
  const sceneRef = useRef<Wiki3dScene | null>(null)
  if (sceneRef.current === null) sceneRef.current = new Wiki3dScene(readWiki3dTheme())
  const scene = sceneRef.current

  // Live values read by stable callbacks.
  const selectedRef = useRef(selectedId)
  const hoveredRef = useRef<string | null>(null)
  const hoverSetRef = useRef<Set<string> | null>(null)
  const rmRef = useRef(reducedMotion)
  rmRef.current = reducedMotion
  const onSelectRef = useRef(onSelect)
  onSelectRef.current = onSelect
  const onFallbackRef = useRef(onFallback)
  onFallbackRef.current = onFallback
  const labelMaxRef = useRef(LABEL_MAX)
  const [linkVersion, setLinkVersion] = useState(0)

  // Render-on-demand bookkeeping.
  const pausedRef = useRef(false)
  const engineStoppedRef = useRef(false)
  const holdUntilRef = useRef(0)
  const idleTimerRef = useRef<number | null>(null)
  const disposedRef = useRef(false)

  // Camera bookkeeping.
  const pendingFitRef = useRef<'all' | 'search' | null>('all')
  const pendingFocusRef = useRef<string | null>(null)
  const matchedRef = useRef<Set<string> | null>(null)

  // ── data ────────────────────────────────────────────────────────────
  const localCenterId = localCenterIdFor(mode, selectedId)
  const data = useMemo(
    () =>
      buildWikiGraphData(
        index,
        { mode, k2Lens, focusGroupFilter, projectFilter, search, localCenterId },
        simNodesRef.current,
        3,
      ),
    [index, localCenterId, search, mode, k2Lens, focusGroupFilter, projectFilter],
  )
  matchedRef.current = data.matchedIds
  const graphData = useMemo(() => ({ nodes: data.nodes, links: data.links }), [data])
  const adjacency = useMemo(() => {
    const adj = new Map<string, Set<string>>()
    for (const l of data.links) {
      const s = endId(l.source)
      const t = endId(l.target)
      if (!adj.has(s)) adj.set(s, new Set())
      if (!adj.has(t)) adj.set(t, new Set())
      adj.get(s)!.add(t)
      adj.get(t)!.add(s)
    }
    return adj
  }, [data])
  const adjacencyRef = useRef(adjacency)
  adjacencyRef.current = adjacency

  // ── render on demand (W11) ──────────────────────────────────────────
  const schedulePause = useCallback((): void => {
    if (idleTimerRef.current !== null) window.clearTimeout(idleTimerRef.current)
    const wait = Math.max(IDLE_PAUSE_MS, holdUntilRef.current - Date.now())
    idleTimerRef.current = window.setTimeout(() => {
      idleTimerRef.current = null
      if (disposedRef.current) return
      if (!engineStoppedRef.current || Date.now() < holdUntilRef.current) {
        schedulePause()
        return
      }
      fgRef.current?.pauseAnimation()
      pausedRef.current = true
    }, wait)
  }, [])

  /** Resume the render loop (input, data, selection, camera) for at least `holdMs`. */
  const wake = useCallback(
    (holdMs = 0): void => {
      if (disposedRef.current) return
      if (holdMs > 0) holdUntilRef.current = Math.max(holdUntilRef.current, Date.now() + holdMs + 100)
      if (pausedRef.current) {
        pausedRef.current = false
        fgRef.current?.resumeAnimation()
      }
      schedulePause()
    },
    [schedulePause],
  )

  // ── visuals ─────────────────────────────────────────────────────────
  const visualFor = useCallback(
    (n: Node3) => {
      const hs = hoverSetRef.current
      return nodeVisual(
        nodeKind(n),
        {
          selected: n.id === selectedRef.current,
          hovered: n.id === hoveredRef.current,
          dimmed: !!hs && !hs.has(n.id),
        },
        scene.theme,
      )
    },
    [scene],
  )

  const restyle = useCallback(
    (ids?: Iterable<string>): void => {
      const sim = simNodesRef.current
      for (const id of ids ?? sim.keys()) {
        const n = sim.get(id)
        if (n) scene.applyVisual(id, visualFor(n))
      }
    },
    [scene, visualFor],
  )

  const nodeThreeObject = useCallback(
    (n: NodeObject<Node3>) => scene.createNode(String(n.id), visualFor(n as Node3)),
    [scene, visualFor],
  )

  const linkColor = useCallback(
    (l: Link3): string => {
      const t = scene.theme
      let base = t.muted
      let alpha = 0.35
      if (l.kind === 'focusGroup' || l.kind === 'project') {
        base = parseCssColor(l.color ?? null) ?? t.homeSoft
        alpha = 0.6
      } else if (l.missing) {
        alpha = 0.15
      }
      const hs = hoverSetRef.current
      const h = hoveredRef.current
      if (hs && h) {
        const touches = endId(l.source) === h || endId(l.target) === h
        alpha = touches ? Math.min(1, alpha * 2) : alpha * 0.2
      }
      return rgbaCss(base, alpha)
    },
    // linkVersion: a cheap prop change that makes the library re-read colours.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [scene, linkVersion],
  )

  // ── labels (§3.6) ───────────────────────────────────────────────────
  const labelTimerRef = useRef<number | null>(null)
  const labelLastRef = useRef(0)
  const refreshLabels = useCallback((): void => {
    const fg = fgRef.current
    if (!fg || disposedRef.current) return
    const sim = simNodesRef.current
    const always = new Set<string>()
    for (const n of sim.values()) {
      const k = nodeKind(n)
      if (k.home || k.hub || k.org) always.add(n.id)
    }
    if (selectedRef.current) always.add(selectedRef.current)
    if (hoveredRef.current) {
      always.add(hoveredRef.current)
      for (const id of adjacencyRef.current.get(hoveredRef.current) ?? []) always.add(id)
    }
    const cam = fg.camera().position
    const controls = fg.controls() as OrbitControls
    const t = controls.target
    const camDist = Math.hypot(cam.x - t.x, cam.y - t.y, cam.z - t.z)
    const ids = pickLabelIds([...sim.values()], cam, always, {
      max: labelMaxRef.current,
      maxDist: Math.max(120, camDist * 1.1),
    })
    scene.setLabels(ids, (id) => {
      const n = sim.get(id)
      if (!n) return null
      const v = visualFor(n)
      const k = nodeKind(n)
      return { text: n.title || n.id, color: v.labelColor, key: k.home || k.hub || k.org, pos: posOf(n), radius: v.radius }
    })
  }, [scene, visualFor])

  const scheduleLabels = useCallback((): void => {
    const now = Date.now()
    const since = now - labelLastRef.current
    if (since >= LABEL_THROTTLE_MS) {
      labelLastRef.current = now
      refreshLabels()
      return
    }
    if (labelTimerRef.current !== null) return
    labelTimerRef.current = window.setTimeout(() => {
      labelTimerRef.current = null
      labelLastRef.current = Date.now()
      refreshLabels()
    }, LABEL_THROTTLE_MS - since)
  }, [refreshLabels])

  const syncLabels = useCallback((): void => {
    scene.syncLabelPositions((id) => posOf(simNodesRef.current.get(id)))
  }, [scene])

  // ── camera ──────────────────────────────────────────────────────────
  const fitAll = useCallback(
    (ms: number): void => {
      const fg = fgRef.current
      if (!fg) return
      wake(ms)
      fg.zoomToFit(rmRef.current ? 0 : ms, 40)
    },
    [wake],
  )

  const runPendingFit = useCallback((): void => {
    const fg = fgRef.current
    const pending = pendingFitRef.current
    if (!fg || !pending) return
    pendingFitRef.current = null
    const matched = matchedRef.current
    if (pending === 'search' && matched && matched.size > 0) {
      wake(600)
      fg.zoomToFit(rmRef.current ? 0 : 600, 40, (n: NodeObject<Node3>) => matched.has(String(n.id)))
    } else {
      fitAll(pending === 'search' ? 600 : 400)
    }
  }, [fitAll, wake])

  const flyTo = useCallback(
    (id: string): boolean => {
      const fg = fgRef.current
      const pos = posOf(simNodesRef.current.get(id))
      if (!fg || !pos) return false
      const ms = rmRef.current ? 0 : FLY_MS
      wake(ms)
      const cam = fg.camera().position
      const target = (fg.controls() as OrbitControls).target
      fg.cameraPosition(flyToPosition(pos, cam, target, FLY_DISTANCE), pos, ms)
      return true
    },
    [wake],
  )

  // ── engine callbacks ────────────────────────────────────────────────
  const fpsStartedRef = useRef(false)
  const startFpsWatchdog = useCallback((): void => {
    if (fpsStartedRef.current || typeof requestAnimationFrame !== 'function') return
    fpsStartedRef.current = true
    const samples: number[] = []
    const start = performance.now()
    let last = start
    const step = (now: number): void => {
      if (disposedRef.current) return
      const dt = now - last
      last = now
      if (dt > 0 && typeof document !== 'undefined' && document.visibilityState === 'visible') {
        samples.push(1000 / dt)
      }
      if (now - start < FPS_SAMPLE_MS) {
        requestAnimationFrame(step)
        return
      }
      const verdict = classifyFps(samples)
      if (verdict === 'fallback') {
        onFallbackRef.current('slow')
      } else if (verdict === 'degrade') {
        scene.glowEnabled = false
        labelMaxRef.current = LABEL_MAX_DEGRADED
        restyle()
        refreshLabels()
        wake()
      }
    }
    requestAnimationFrame(step)
  }, [scene, restyle, refreshLabels, wake])

  const onEngineTick = useCallback((): void => {
    engineStoppedRef.current = false
    startFpsWatchdog()
    syncLabels()
    scheduleLabels()
    if (pendingFocusRef.current && flyTo(pendingFocusRef.current)) pendingFocusRef.current = null
  }, [startFpsWatchdog, syncLabels, scheduleLabels, flyTo])

  const onEngineStop = useCallback((): void => {
    engineStoppedRef.current = true
    syncLabels()
    refreshLabels()
    runPendingFit()
    if (pendingFocusRef.current && flyTo(pendingFocusRef.current)) pendingFocusRef.current = null
    schedulePause()
  }, [syncLabels, refreshLabels, runPendingFit, flyTo, schedulePause])

  // ── effects ─────────────────────────────────────────────────────────
  // Data changed: the library re-heats; keep the loop running, forget
  // disposed node objects.
  const firstDataRef = useRef(true)
  useEffect(() => {
    scene.prune(new Set(data.nodes.map((n) => n.id)))
    if (firstDataRef.current) {
      firstDataRef.current = false
      return
    }
    engineStoppedRef.current = false
    wake()
  }, [data, scene, wake])

  // Mode / workspace / lens / filter change → fit the new graph once it settles.
  const scopeKey = `${mode}|${index.scope ?? ''}|${index.workspacePath}|${k2Lens}|${focusGroupFilter}|${projectFilter}`
  const prevScopeRef = useRef(scopeKey)
  useEffect(() => {
    if (prevScopeRef.current === scopeKey) return
    prevScopeRef.current = scopeKey
    pendingFitRef.current = 'all'
  }, [scopeKey])

  // Search change → frame the matches ~300 ms after typing stops (once).
  const prevSearchRef = useRef(search)
  useEffect(() => {
    if (prevSearchRef.current === search) return
    prevSearchRef.current = search
    const t = window.setTimeout(() => {
      pendingFitRef.current = 'search'
      if (engineStoppedRef.current) runPendingFit()
    }, SEARCH_FIT_DEBOUNCE_MS)
    return () => window.clearTimeout(t)
  }, [search, runPendingFit])

  // Selection change → restyle only the two affected nodes.
  useEffect(() => {
    const prev = selectedRef.current
    selectedRef.current = selectedId
    const ids = [prev, selectedId].filter((x): x is string => !!x)
    if (ids.length === 0) return
    restyle(ids)
    wake()
    scheduleLabels()
  }, [selectedId, restyle, wake, scheduleLabels])

  // Wikilink click in the note panel → fly once the node has a position.
  const lastFocusSeqRef = useRef<number | null>(focusRequest?.seq ?? null)
  useEffect(() => {
    if (!focusRequest || focusRequest.seq === lastFocusSeqRef.current) return
    lastFocusSeqRef.current = focusRequest.seq
    if (!flyTo(focusRequest.id)) {
      pendingFocusRef.current = focusRequest.id
      wake()
    }
  }, [focusRequest, flyTo, wake])

  // Size follows the pane.
  useEffect(() => {
    const el = containerRef.current
    if (!el) return
    const measure = (): void =>
      setSize({ w: Math.max(1, Math.floor(el.clientWidth)), h: Math.max(1, Math.floor(el.clientHeight)) })
    measure()
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(() => {
      measure()
      wake()
    })
    ro.observe(el)
    return () => ro.disconnect()
  }, [wake])

  // Palette changes on <html> → re-read tokens, rebuild materials (W13).
  useEffect(() => {
    if (typeof MutationObserver === 'undefined') return
    let key = themeKey(scene.theme)
    const mo = new MutationObserver(() => {
      const next = readWiki3dTheme()
      const k = themeKey(next)
      if (k === key) return
      key = k
      scene.setTheme(next)
      restyle()
      refreshLabels()
      setLinkVersion((v) => v + 1)
      wake()
    })
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-style', 'data-palette', 'style', 'class'] })
    return () => mo.disconnect()
  }, [scene, restyle, refreshLabels, wake])

  // Controls + canvas setup after mount, and the dispose (W8, W10). A
  // layout effect: its cleanup runs before the library's own destructor
  // (a passive effect in react-kapsule), while our refs are still live.
  useLayoutEffect(() => {
    const fg = fgRef.current
    if (!fg) return
    disposedRef.current = false
    const renderer = fg.renderer() as WebGLRenderer
    const threeScene = fg.scene() as Scene
    const canvas = renderer.domElement
    const controls = fg.controls() as OrbitControls
    controls.enableDamping = true
    controls.dampingFactor = 0.12
    controls.screenSpacePanning = true
    controls.zoomToCursor = true
    if (typeof controls.listenToKeyEvents === 'function') controls.listenToKeyEvents(canvas)
    canvas.tabIndex = 0
    canvas.style.outline = 'none'
    canvas.style.display = 'block'
    canvas.setAttribute('aria-label', CANVAS_LABEL)
    canvas.setAttribute('data-wiki-3d-canvas', '')
    threeScene.add(scene.labelGroup)

    const stop = (e: Event): void => e.preventDefault()
    const onInput = (): void => wake()
    const onDbl = (): void => {
      if (hoveredRef.current) return // a node, not the background
      fitAll(400)
    }
    const onLost = (e: Event): void => {
      e.preventDefault()
      if (disposedRef.current) return
      onFallbackRef.current('crashed')
    }
    const onControlsStart = (): void => wake()
    const onControlsChange = (): void => scheduleLabels()
    const onVisibility = (): void => {
      if (document.hidden) {
        fgRef.current?.pauseAnimation()
        pausedRef.current = true
      } else {
        wake()
      }
    }
    canvas.addEventListener('gesturestart', stop)
    canvas.addEventListener('dblclick', onDbl)
    canvas.addEventListener('webglcontextlost', onLost)
    for (const t of ['pointermove', 'pointerdown', 'wheel', 'keydown'] as const) {
      canvas.addEventListener(t, onInput, { passive: true })
    }
    controls.addEventListener('start', onControlsStart)
    controls.addEventListener('change', onControlsChange)
    document.addEventListener('visibilitychange', onVisibility)

    return () => {
      disposedRef.current = true
      if (idleTimerRef.current !== null) window.clearTimeout(idleTimerRef.current)
      if (labelTimerRef.current !== null) window.clearTimeout(labelTimerRef.current)
      canvas.removeEventListener('gesturestart', stop)
      canvas.removeEventListener('dblclick', onDbl)
      canvas.removeEventListener('webglcontextlost', onLost)
      for (const t of ['pointermove', 'pointerdown', 'wheel', 'keydown'] as const) {
        canvas.removeEventListener(t, onInput)
      }
      controls.removeEventListener('start', onControlsStart)
      controls.removeEventListener('change', onControlsChange)
      document.removeEventListener('visibilitychange', onVisibility)
      fg.pauseAnimation()
      scene.dispose()
      // The library's destructor (next, in react-kapsule's passive cleanup)
      // only dispose()s the renderer; hand the GL context back now — WebKit
      // caps live contexts. Calls after this are no-ops on a lost context.
      renderer.forceContextLoss()
      const w = window as unknown as { scene?: unknown }
      if (w.scene === threeScene) delete w.scene
    }
    // Mount-only: the library is created once per mount.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  return (
    <div
      ref={containerRef}
      className="relative w-full h-full min-h-0 min-w-0 overflow-hidden"
      style={{ position: 'relative', width: '100%', height: '100%', overflow: 'hidden' }}
      data-wiki-graph-3d
    >
      {/* Absolutely placed so the canvas can never grow the pane it measures. */}
      <div style={{ position: 'absolute', inset: 0 }}>
      <ForceGraph3D<Node3, Link3>
        ref={fgRef as React.MutableRefObject<FG | undefined>}
        controlType="orbit"
        rendererConfig={{ antialias: true, alpha: true, powerPreference: 'default' }}
        width={size.w}
        height={size.h}
        graphData={graphData}
        backgroundColor="rgba(0,0,0,0)"
        showNavInfo={false}
        nodeId="id"
        linkSource="source"
        linkTarget="target"
        nodeLabel={NO_TOOLTIP}
        nodeThreeObject={nodeThreeObject}
        linkColor={linkColor as (l: unknown) => string}
        linkOpacity={1}
        linkWidth={0}
        enableNodeDrag
        cooldownTicks={reducedMotion ? 0 : 40}
        cooldownTime={2_000}
        d3AlphaDecay={0.06}
        d3VelocityDecay={0.35}
        warmupTicks={reducedMotion ? 80 : 0}
        onEngineTick={onEngineTick}
        onEngineStop={onEngineStop}
        onNodeHover={(node) => {
          const id = node ? String((node as Node3).id) : null
          if (id === hoveredRef.current) return
          hoveredRef.current = id
          if (id) {
            const set = new Set(adjacencyRef.current.get(id) ?? [])
            set.add(id)
            hoverSetRef.current = set
          } else {
            hoverSetRef.current = null
          }
          restyle()
          setLinkVersion((v) => v + 1)
          scheduleLabels()
          wake()
        }}
        onNodeClick={(node) => {
          const id = String((node as Node3).id)
          if (!id) return
          onSelectRef.current(id)
          flyTo(id)
        }}
        onNodeDrag={() => {
          wake()
          syncLabels()
        }}
        onBackgroundClick={() => {
          if (hoveredRef.current) {
            hoveredRef.current = null
            hoverSetRef.current = null
            restyle()
            setLinkVersion((v) => v + 1)
          }
          onSelectRef.current(null)
        }}
      />
      </div>
      <div className="pointer-events-none absolute left-2 bottom-2 flex items-center gap-2 text-[10px] text-[var(--color-text-muted)] select-none">
        <span data-wiki-3d-hint>{HINT_TEXT}</span>
        <button
          type="button"
          className="pointer-events-auto px-1.5 py-0.5 border border-[var(--color-border)] bg-[var(--color-bg)]/80 text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] cursor-pointer"
          onClick={() => fitAll(400)}
        >
          Reset view
        </button>
      </div>
    </div>
  )
}
