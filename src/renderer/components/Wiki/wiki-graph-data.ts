// Pure graph adapter shared by the 2D (WikiGraph) and 3D (WikiGraph3D)
// renderers (prd-wiki-3d-view-v1 §6.2). Moved verbatim from WikiGraph.tsx:
// catalog with missing stubs, K2 lens/filter, Local neighbourhood, search
// plus neighbours, sim-node reuse, neighbour-adjacent spawn, sorted nodes,
// lens edge isolation and org edge colours.
//
// Each renderer passes ITS OWN sim map (W9): three-forcegraph writes
// `__threeObj` onto node objects, so sharing them with 2D would keep
// disposed meshes alive.

import type { WikiIndex, WikiLink, WikiNode } from './wiki-api'
import {
  focusGroupFilterWorkspaceIds,
  isWikiFocusGroupNode,
  isWikiProjectNode,
  neighborhoodIds,
  nodeMatchesSearch,
  projectFilterWorkspaceIds,
} from './wiki-api'

/** Fallback color for focus-group / project membership edges. */
export const ORG_EDGE_FALLBACK = '#6b8cff'

export type WikiSimNode = WikiNode & {
  id: string
  __missing?: boolean
  x?: number
  y?: number
  z?: number
  vx?: number
  vy?: number
  vz?: number
}

export type WikiGraphLink = {
  source: string | WikiSimNode
  target: string | WikiSimNode
  missing?: boolean
  kind?: string | null
  color?: string | null
}

export type WikiGraphOpts = {
  mode: 'k2' | 'local' | 'global'
  depth: 1 | 2
  k2Lens: 'projects' | 'groups'
  focusGroupFilter: string
  projectFilter: string
  search: string
  /** Only Local mode's neighbourhood depends on selection (see `localCenterIdFor`). */
  localCenterId: string | null
}

export type WikiGraphData<N> = {
  nodes: N[]
  links: WikiGraphLink[]
  /** Search matches (before neighbours are added); null when the search is empty. */
  matchedIds: Set<string> | null
}

/**
 * Only Local mode's neighbourhood depends on selection. Including the
 * selection in the data for Global/K2 rebuilt the whole graph object on
 * every click, which restarts the force sim → layout shift.
 */
export function localCenterIdFor(mode: WikiGraphOpts['mode'], selectedId: string | null): string | null {
  return mode === 'local' ? selectedId : null
}

function stub(id: string): WikiNode {
  return { id, title: id, aliases: [], tags: [], path: '', exists: false }
}

/** Every node id (index nodes plus missing stubs for unknown link endpoints). */
function buildCatalog(index: WikiIndex): Map<string, WikiNode> {
  const catalog = new Map<string, WikiNode>()
  for (const n of index.nodes) {
    catalog.set(n.id, n)
  }
  for (const l of index.links) {
    if (!catalog.has(l.source)) catalog.set(l.source, stub(l.source))
    if (!catalog.has(l.target)) catalog.set(l.target, stub(l.target))
  }
  return catalog
}

function visibleIds(
  index: WikiIndex,
  catalog: Map<string, WikiNode>,
  opts: WikiGraphOpts,
): { visible: Set<string>; matchedIds: Set<string> | null } {
  const { mode, k2Lens, projectFilter, focusGroupFilter, localCenterId, depth, search } = opts
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

  let matchedIds: Set<string> | null = null
  if (search.trim()) {
    const matched = new Set<string>()
    for (const n of catalog.values()) {
      if (visible.has(n.id) && nodeMatchesSearch(n, search)) matched.add(n.id)
    }
    // Pre-existing (W7, pinned by a test): neighbours come from ALL
    // index.links, so they can fall outside the K2 filter / Local set.
    const withNeighbors = new Set(matched)
    for (const l of index.links) {
      if (matched.has(l.source)) withNeighbors.add(l.target)
      if (matched.has(l.target)) withNeighbors.add(l.source)
    }
    visible = withNeighbors
    matchedIds = matched
  }

  return { visible, matchedIds }
}

/** How many nodes the graph would show (the 3D node cap, F3). No sim side effects. */
export function countWikiGraphNodes(index: WikiIndex, opts: WikiGraphOpts): number {
  return visibleIds(index, buildCatalog(index), opts).visible.size
}

/**
 * The ids the graph would show plus the search matches (null when the
 * search is empty). The standard Wiki view lists from the same set so the
 * three views agree. No sim side effects.
 */
export function wikiGraphVisibleIds(
  index: WikiIndex,
  opts: WikiGraphOpts,
): { visible: Set<string>; matchedIds: Set<string> | null } {
  return visibleIds(index, buildCatalog(index), opts)
}

/**
 * Build `{nodes, links}` for a force graph. Reuses node objects from `sim`
 * (keeping x/y/z and velocities) so polls don't re-explode the layout; new
 * nodes spawn next to a known neighbour. Mutates `sim`.
 */
export function buildWikiGraphData<N extends WikiSimNode>(
  index: WikiIndex,
  opts: WikiGraphOpts,
  sim: Map<string, N>,
  dims: 2 | 3,
): WikiGraphData<N> {
  const { mode, k2Lens } = opts
  const catalog = buildCatalog(index)
  const { visible, matchedIds } = visibleIds(index, catalog, opts)

  for (const id of [...sim.keys()]) {
    if (!visible.has(id)) sim.delete(id)
  }

  const nodes: N[] = []
  for (const id of visible) {
    const meta = catalog.get(id)!
    let node = sim.get(id)
    if (!node) {
      let x = (Math.random() - 0.5) * 40
      let y = (Math.random() - 0.5) * 40
      let z = dims === 3 ? (Math.random() - 0.5) * 40 : undefined
      for (const l of index.links) {
        const other =
          l.source === id ? l.target : l.target === id ? l.source : null
        if (!other) continue
        const n = sim.get(other)
        if (n && n.x != null && n.y != null) {
          x = n.x + (Math.random() - 0.5) * 36
          y = n.y + (Math.random() - 0.5) * 36
          if (dims === 3) z = (n.z ?? 0) + (Math.random() - 0.5) * 36
          break
        }
      }
      const fresh: WikiSimNode = {
        ...meta,
        id,
        __missing: !meta.exists,
        x,
        y,
        vx: 0,
        vy: 0,
      }
      if (dims === 3) {
        fresh.z = z
        fresh.vz = 0
      }
      node = fresh as N
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

  const links: WikiGraphLink[] = index.links
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

  return { nodes, links, matchedIds }
}
