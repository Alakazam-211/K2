// Force-directed brain map for the workspace wiki (react-force-graph-2d).
//
// Live updates merge into existing simulation state: we keep x/y/vx/vy for
// known nodes so new notes appear without re-exploding the whole graph.
// Home stays identifiable with a soft blue fill when not selected.
// K2 map: focus-group hubs + dashed membership edges to workspace hubs.

import React, { useCallback, useEffect, useMemo, useRef } from 'react'
import ForceGraph2D, { type ForceGraphMethods, type NodeObject } from 'react-force-graph-2d'
import type { WikiIndex, WikiLink, WikiNode } from './wiki-api'
import {
  focusGroupFilterWorkspaceIds,
  isWikiFocusGroupNode,
  isWikiHomeNode,
  isWikiProjectNode,
  isWikiWorkspaceHubNode,
  neighborhoodIds,
  nodeMatchesSearch,
  projectFilterWorkspaceIds,
} from './wiki-api'

type GraphNode = WikiNode & {
  id: string
  __missing?: boolean
  x?: number
  y?: number
  vx?: number
  vy?: number
}

type GraphLink = {
  source: string | GraphNode
  target: string | GraphNode
  missing?: boolean
  kind?: string | null
  color?: string | null
}

/** Default node radius in graph units (unselected). */
const NODE_R_GRAPH = 4
/** Selected node radius in graph units. */
const NODE_R_SELECTED_GRAPH = 6
/** Focus-group hub radius. */
const NODE_R_FOCUS_GROUP = 7
/** Workspace hub radius. */
const NODE_R_WORKSPACE_HUB = 5

/** Full opacity at this on-screen node diameter (CSS px). */
export const LABEL_MIN_NODE_DIAMETER_PX = 25
/** Labels start fading in from this diameter (CSS px). */
export const LABEL_FADE_START_DIAMETER_PX = 20

/** Soft blue for Home when it is not the active selection. */
const HOME_SOFT_BLUE = '#8fa8ff'
/** Fallback color for focus-group / project membership edges. */
const ORG_EDGE_FALLBACK = '#6b8cff'

function readCssColor(varName: string, fallback: string): string {
  if (typeof document === 'undefined') return fallback
  const v = getComputedStyle(document.documentElement).getPropertyValue(varName).trim()
  return v || fallback
}

function nodeScreenDiameterPx(rGraph: number, globalScale: number): number {
  return 2 * rGraph * globalScale
}

/** Smoothstep 0→1 between LABEL_FADE_START_DIAMETER_PX and LABEL_MIN_NODE_DIAMETER_PX. */
function labelOpacityForDiameter(diameterPx: number): number {
  const start = LABEL_FADE_START_DIAMETER_PX
  const end = LABEL_MIN_NODE_DIAMETER_PX
  if (diameterPx <= start) return 0
  if (diameterPx >= end) return 1
  const t = (diameterPx - start) / (end - start)
  return t * t * (3 - 2 * t)
}

export default function WikiGraph({
  index,
  selectedId,
  search,
  mode,
  depth,
  k2Lens = 'groups',
  focusGroupFilter = 'all',
  projectFilter = 'all',
  onSelect,
}: {
  index: WikiIndex
  selectedId: string | null
  search: string
  mode: 'k2' | 'local' | 'global'
  depth: 1 | 2
  /** K2 only: Projects map vs Focus Groups map. */
  k2Lens?: 'projects' | 'groups'
  /** K2 groups lens: `all` | `ungrouped` | focus group id. */
  focusGroupFilter?: string
  /** K2 projects lens: Feedback dropdown value (`all` | ws id | `project:id`). */
  projectFilter?: string
  onSelect: (id: string | null) => void
}): React.JSX.Element {
  const containerRef = useRef<HTMLDivElement>(null)
  const fgRef = useRef<ForceGraphMethods<NodeObject<GraphNode>, GraphLink> | undefined>(undefined)
  const [size, setSize] = React.useState({ w: 400, h: 400 })
  const [hoveredId, setHoveredId] = React.useState<string | null>(null)

  // Live node objects the simulation mutates — stable identity across polls.
  const simNodesRef = useRef<Map<string, GraphNode>>(new Map())
  const firstLayoutRef = useRef(true)

  useEffect(() => {
    const el = containerRef.current
    if (!el) return
    const ro = new ResizeObserver((entries) => {
      const cr = entries[0]?.contentRect
      if (!cr) return
      setSize({ w: Math.max(1, Math.floor(cr.width)), h: Math.max(1, Math.floor(cr.height)) })
    })
    ro.observe(el)
    setSize({
      w: Math.max(1, Math.floor(el.clientWidth)),
      h: Math.max(1, Math.floor(el.clientHeight)),
    })
    return () => ro.disconnect()
  }, [])

  // Wheel / two-finger trackpad → pan. Pinch (browsers synthesize ctrl+wheel) → zoom.
  useEffect(() => {
    const el = containerRef.current
    if (!el) return
    const onWheel = (e: WheelEvent): void => {
      e.preventDefault()
      e.stopPropagation()
      const fg = fgRef.current
      if (!fg) return

      if (e.ctrlKey || e.metaKey) {
        const k = typeof fg.zoom === 'function' ? Number(fg.zoom()) : 1
        const current = Number.isFinite(k) && k > 0 ? k : 1
        const factor = Math.exp(-e.deltaY * 0.01)
        const next = Math.min(8, Math.max(0.15, current * factor))
        fg.zoom(next, 0)
        return
      }

      const k = typeof fg.zoom === 'function' ? Number(fg.zoom()) : 1
      const scale = Number.isFinite(k) && k > 0 ? k : 1
      const cur = (fg.centerAt as unknown as () => { x: number; y: number } | undefined)()
      const cx = cur && Number.isFinite(cur.x) ? cur.x : 0
      const cy = cur && Number.isFinite(cur.y) ? cur.y : 0
      fg.centerAt(cx + e.deltaX / scale, cy + e.deltaY / scale, 0)
    }
    el.addEventListener('wheel', onWheel, { passive: false })
    return () => el.removeEventListener('wheel', onWheel)
  }, [])

  const colors = useMemo(
    () => ({
      accent: readCssColor('--color-accent', '#6c8cff'),
      homeSoft: HOME_SOFT_BLUE,
      text: readCssColor('--color-text-primary', '#e8e8e8'),
      muted: readCssColor('--color-text-muted', '#6b7280'),
      border: readCssColor('--color-border', '#333'),
      bg: readCssColor('--color-bg', '#0d0d0d'),
    }),
    [],
  )

  // Only Local mode's neighborhood depends on selection. Including
  // selectedId in graphData for Global/K2 rebuilt the whole graphData
  // object on every click, which restarts force-graph's sim → layout shift.
  const localCenterId = mode === 'local' ? selectedId : null

  const graphData = useMemo(() => {
    const catalog = new Map<string, WikiNode>()
    for (const n of index.nodes) {
      catalog.set(n.id, n)
    }
    for (const l of index.links) {
      if (!catalog.has(l.source)) {
        catalog.set(l.source, {
          id: l.source,
          title: l.source,
          aliases: [],
          tags: [],
          path: '',
          exists: false,
        })
      }
      if (!catalog.has(l.target)) {
        catalog.set(l.target, {
          id: l.target,
          title: l.target,
          aliases: [],
          tags: [],
          path: '',
          exists: false,
        })
      }
    }

    let visible = new Set(catalog.keys())

    // K2: pick lens (projects vs groups) and apply org filter.
    if (mode === 'k2') {
      const next = new Set<string>()
      if (k2Lens === 'projects') {
        const wsOk = projectFilterWorkspaceIds(index, projectFilter)
        const projectIdFilter =
          projectFilter.startsWith('project:') ? projectFilter.slice('project:'.length) : null
        for (const n of catalog.values()) {
          if (isWikiFocusGroupNode(n)) continue // hide FG layer on Projects lens
          if (isWikiProjectNode(n)) {
            if (projectFilter === 'all') {
              next.add(n.id)
            } else if (projectIdFilter && (n.projectId === projectIdFilter || n.id === `__project__::${projectIdFilter}`)) {
              next.add(n.id)
            }
            // Single-workspace filter: no project square unless that ws is a member (optional omit)
            continue
          }
          if (!wsOk) {
            next.add(n.id)
            continue
          }
          if (n.workspaceId && wsOk.has(n.workspaceId)) next.add(n.id)
        }
      } else {
        // Groups lens — hide project layer
        const wsOk = focusGroupFilterWorkspaceIds(index, focusGroupFilter)
        for (const n of catalog.values()) {
          if (isWikiProjectNode(n)) continue
          if (isWikiFocusGroupNode(n)) {
            if (!focusGroupFilter || focusGroupFilter === 'all') {
              next.add(n.id)
            } else if (
              focusGroupFilter !== 'ungrouped' &&
              (n.focusGroupId === focusGroupFilter || n.id === `__focusgroup__::${focusGroupFilter}`)
            ) {
              next.add(n.id)
            }
            continue
          }
          if (!wsOk) {
            next.add(n.id)
            continue
          }
          if (n.workspaceId && wsOk.has(n.workspaceId)) next.add(n.id)
        }
      }
      visible = next
    }

    if (mode === 'local' && localCenterId && catalog.has(localCenterId)) {
      visible = neighborhoodIds(index.links, localCenterId, depth)
    }

    if (search.trim()) {
      const matched = new Set<string>()
      for (const n of catalog.values()) {
        if (visible.has(n.id) && nodeMatchesSearch(n, search)) matched.add(n.id)
      }
      const withNeighbors = new Set(matched)
      for (const l of index.links) {
        if (matched.has(l.source)) withNeighbors.add(l.target)
        if (matched.has(l.target)) withNeighbors.add(l.source)
      }
      visible = withNeighbors
    }

    const sim = simNodesRef.current
    for (const id of [...sim.keys()]) {
      if (!visible.has(id)) sim.delete(id)
    }

    const nodes: GraphNode[] = []
    for (const id of visible) {
      const meta = catalog.get(id)!
      let node = sim.get(id)
      if (!node) {
        let x = (Math.random() - 0.5) * 40
        let y = (Math.random() - 0.5) * 40
        for (const l of index.links) {
          const other =
            l.source === id ? l.target : l.target === id ? l.source : null
          if (!other) continue
          const n = sim.get(other)
          if (n && n.x != null && n.y != null) {
            x = n.x + (Math.random() - 0.5) * 36
            y = n.y + (Math.random() - 0.5) * 36
            break
          }
        }
        node = {
          ...meta,
          id,
          __missing: !meta.exists,
          x,
          y,
          vx: 0,
          vy: 0,
        }
        sim.set(id, node)
      } else {
        node.title = meta.title
        node.aliases = meta.aliases
        node.tags = meta.tags
        node.path = meta.path
        node.exists = meta.exists
        node.__missing = !meta.exists
        node.workspaceId = meta.workspaceId
        node.workspaceName = meta.workspaceName
        node.workspacePath = meta.workspacePath
        node.kind = meta.kind
        node.focusGroupId = meta.focusGroupId
        node.focusGroupName = meta.focusGroupName
        node.focusGroupColor = meta.focusGroupColor
        node.projectId = meta.projectId
        node.projectName = meta.projectName
        node.projectColor = meta.projectColor
      }
      nodes.push(node)
    }

    // Stable order so force-graph doesn't thrash identity when sets match.
    nodes.sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))

    const links: GraphLink[] = index.links
      .filter((l: WikiLink) => {
        if (!visible.has(l.source) || !visible.has(l.target)) return false
        // Lens isolation: don't draw the other org layer's edges.
        if (mode === 'k2' && k2Lens === 'projects' && l.kind === 'focusGroup') return false
        if (mode === 'k2' && k2Lens === 'groups' && l.kind === 'project') return false
        return true
      })
      .map((l) => {
        const src = catalog.get(l.source)
        let edgeColor: string | null = null
        if (l.kind === 'focusGroup') {
          edgeColor = src?.focusGroupColor || ORG_EDGE_FALLBACK
        } else if (l.kind === 'project') {
          edgeColor = src?.projectColor || ORG_EDGE_FALLBACK
        }
        return {
          source: l.source,
          target: l.target,
          missing: l.missing || !catalog.get(l.target)?.exists,
          kind: l.kind ?? null,
          color: edgeColor,
        }
      })

    return { nodes, links }
  }, [index, localCenterId, search, mode, depth, k2Lens, focusGroupFilter, projectFilter])

  const structureKey = `${graphData.nodes.length}:${graphData.links.length}`
  const prevStructureRef = useRef(structureKey)
  useEffect(() => {
    const fg = fgRef.current
    if (!fg) return
    const prev = prevStructureRef.current
    prevStructureRef.current = structureKey
    if (firstLayoutRef.current) {
      firstLayoutRef.current = false
      return
    }
    if (structureKey === prev) return
    try {
      fg.d3ReheatSimulation()
    } catch {
      /* ignore */
    }
  }, [structureKey])

  // Camera is user-controlled only (pan + pinch-zoom). We intentionally do
  // NOT auto centerAt/zoom on selection or graph growth — that re-fired on
  // every node add / re-select while the map was building and felt like
  // constant zoom creep.

  const paintNode = useCallback(
    (node: NodeObject<GraphNode>, ctx: CanvasRenderingContext2D, globalScale: number) => {
      const n = node as GraphNode
      const x = n.x ?? 0
      const y = n.y ?? 0
      const isSelected = n.id === selectedId
      const isHovered = n.id === hoveredId
      const missing = n.__missing || !n.exists
      const isHome = !missing && isWikiHomeNode(n)
      const isFg = isWikiFocusGroupNode(n)
      const isProject = isWikiProjectNode(n)
      const isOrgSquare = isFg || isProject
      const isHub = isWikiWorkspaceHubNode(n)
      const orgColor = isProject
        ? n.projectColor || null
        : n.focusGroupColor || null

      let r = NODE_R_GRAPH
      if (isSelected) r = NODE_R_SELECTED_GRAPH
      else if (isOrgSquare) r = NODE_R_FOCUS_GROUP
      else if (isHub) r = NODE_R_WORKSPACE_HUB

      const diameterPx = nodeScreenDiameterPx(NODE_R_GRAPH, globalScale)
      // Labels: zoom fade; selected / hover / Home / hubs / org squares always on.
      const zoomLabelAlpha =
        isSelected || isHovered || isHome || isOrgSquare || isHub
          ? 1
          : labelOpacityForDiameter(diameterPx)

      if (isOrgSquare) {
        // Rounded square for focus-group / project hubs.
        const s = r * 1.35
        ctx.beginPath()
        const rr = 1.5
        ctx.moveTo(x - s + rr, y - s)
        ctx.arcTo(x + s, y - s, x + s, y + s, rr)
        ctx.arcTo(x + s, y + s, x - s, y + s, rr)
        ctx.arcTo(x - s, y + s, x - s, y - s, rr)
        ctx.arcTo(x - s, y - s, x + s, y - s, rr)
        ctx.closePath()
        ctx.fillStyle = isSelected || isHovered ? colors.accent : orgColor || colors.homeSoft
        ctx.globalAlpha = isSelected ? 1 : 0.85
        ctx.fill()
        ctx.globalAlpha = 1
        ctx.strokeStyle = orgColor || colors.accent
        ctx.lineWidth = 1.4 / globalScale
        ctx.stroke()
      } else {
        ctx.beginPath()
        ctx.arc(x, y, r, 0, 2 * Math.PI)
        if (missing) {
          ctx.fillStyle = colors.muted
          ctx.globalAlpha = 0.45
        } else if (isSelected) {
          ctx.fillStyle = colors.accent
          ctx.globalAlpha = 1
        } else if (isHovered) {
          ctx.fillStyle = colors.accent
          ctx.globalAlpha = 0.75
        } else if (isHub) {
          ctx.fillStyle = orgColor || colors.muted
          ctx.globalAlpha = 0.9
        } else if (isHome) {
          ctx.fillStyle = orgColor || colors.homeSoft
          ctx.globalAlpha = 0.9
        } else {
          ctx.fillStyle = colors.text
          ctx.globalAlpha = 0.85
        }
        ctx.fill()
        ctx.globalAlpha = 1

        if (isSelected || isHovered || isHome || isHub) {
          ctx.strokeStyle =
            isHome && !isSelected && !orgColor
              ? colors.homeSoft
              : orgColor && (isHome || isHub) && !isSelected
                ? orgColor
                : colors.accent
          ctx.lineWidth = (isSelected ? 1.5 : isHome || isHub ? 1.1 : 1) / globalScale
          ctx.globalAlpha = isHome && !isSelected && !isHovered ? 0.65 : 1
          ctx.beginPath()
          ctx.arc(x, y, r + 3, 0, 2 * Math.PI)
          ctx.stroke()
          ctx.globalAlpha = 1
        }
      }

      if (zoomLabelAlpha <= 0.01) return

      const label = n.title || n.id
      const fontSize = Math.max(11 / globalScale, 2.8)
      ctx.font = isOrgSquare ? `600 ${fontSize}px sans-serif` : `${fontSize}px sans-serif`
      ctx.textAlign = 'center'
      ctx.textBaseline = 'top'
      ctx.fillStyle = missing
        ? colors.muted
        : isOrgSquare
          ? orgColor || colors.homeSoft
          : isHome && !isSelected
            ? orgColor || colors.homeSoft
            : colors.text
      const base =
        missing ? 0.55 : isHovered || isSelected || isHome || isOrgSquare || isHub ? 1 : 0.9
      ctx.globalAlpha = base * zoomLabelAlpha
      ctx.fillText(label, x, y + r + 2)
      ctx.globalAlpha = 1
    },
    [selectedId, hoveredId, colors],
  )

  const paintPointer = useCallback(
    (node: NodeObject<GraphNode>, color: string, ctx: CanvasRenderingContext2D) => {
      const n = node as GraphNode
      let r = NODE_R_GRAPH + 2
      if (n.id === selectedId) r = NODE_R_SELECTED_GRAPH + 2
      else if (isWikiFocusGroupNode(n) || isWikiProjectNode(n)) r = NODE_R_FOCUS_GROUP + 2
      else if (isWikiWorkspaceHubNode(n)) r = NODE_R_WORKSPACE_HUB + 2
      ctx.beginPath()
      ctx.arc(n.x ?? 0, n.y ?? 0, r, 0, 2 * Math.PI)
      ctx.fillStyle = color
      ctx.fill()
    },
    [selectedId],
  )

  return (
    <div ref={containerRef} className="w-full h-full min-h-0 min-w-0">
      <ForceGraph2D<GraphNode, GraphLink>
        ref={fgRef as React.MutableRefObject<ForceGraphMethods<NodeObject<GraphNode>, GraphLink>>}
        width={size.w}
        height={size.h}
        graphData={graphData}
        backgroundColor="transparent"
        nodeId="id"
        linkSource="source"
        linkTarget="target"
        nodeLabel={() => ''}
        nodeCanvasObject={paintNode}
        nodeCanvasObjectMode={() => 'replace'}
        nodePointerAreaPaint={paintPointer}
        linkColor={(l) => {
          const link = l as GraphLink
          if (link.kind === 'focusGroup' || link.kind === 'project') {
            return link.color || ORG_EDGE_FALLBACK
          }
          if (link.kind === 'workspaceHub') return colors.muted
          return link.missing ? colors.muted : colors.border
        }}
        linkWidth={(l) => {
          const link = l as GraphLink
          if (link.kind === 'focusGroup' || link.kind === 'project') return 1.6
          if (link.missing) return 0.5
          return 1
        }}
        linkLineDash={(l) => {
          const link = l as GraphLink
          if (link.kind === 'focusGroup' || link.kind === 'project') return [4, 3]
          if (link.missing) return [2, 2]
          return null
        }}
        cooldownTicks={40}
        cooldownTime={2_000}
        d3AlphaDecay={0.06}
        d3VelocityDecay={0.35}
        warmupTicks={0}
        enableZoomInteraction={false}
        enablePanInteraction={true}
        onNodeHover={(node) => {
          const id = node ? String((node as GraphNode).id) : null
          setHoveredId(id)
        }}
        onNodeClick={(node) => {
          const id = String((node as GraphNode).id)
          if (id) onSelect(id)
        }}
        onBackgroundClick={() => {
          setHoveredId(null)
          onSelect(null)
        }}
      />
    </div>
  )
}
