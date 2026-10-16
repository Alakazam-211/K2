// prd-wiki-3d-view-v1 §6.2 / §8.1: the graph adapter shared by the 2D and
// 3D renderers. Pure (node env).

import { describe, expect, it } from 'vitest'
import { WIKI_LOCAL_DEPTH, type WikiIndex, type WikiLink, type WikiNode } from './wiki-api'
import {
  buildWikiGraphData,
  countWikiGraphNodes,
  localCenterIdFor,
  ORG_EDGE_FALLBACK,
  type WikiGraphOpts,
  type WikiSimNode,
} from './wiki-graph-data'

function note(id: string, extra: Partial<WikiNode> = {}): WikiNode {
  return { id, title: id, aliases: [], tags: [], path: `${id}.md`, exists: true, ...extra }
}

function link(source: string, target: string, extra: Partial<WikiLink> = {}): WikiLink {
  return { source, target, ...extra }
}

const OPTS: WikiGraphOpts = {
  mode: 'global',
  k2Lens: 'projects',
  focusGroupFilter: 'all',
  projectFilter: 'all',
  search: '',
  localCenterId: null,
}

function opts(over: Partial<WikiGraphOpts>): WikiGraphOpts {
  return { ...OPTS, ...over }
}

/** One workspace: Home → A → B → C, A → Ghost (missing). */
function workspaceIndex(): WikiIndex {
  return {
    workspacePath: '/w/alpha',
    generatedAt: '2026-10-16T00:00:00Z',
    nodes: [note('Home', { title: 'Home' }), note('A', { tags: ['infra'] }), note('B'), note('C')],
    links: [link('Home', 'A'), link('A', 'B'), link('B', 'C'), link('A', 'Ghost', { missing: true })],
    noteCount: 4,
    scope: 'workspace',
  }
}

/**
 * Fleet map: workspaces w1 (group g1, project p1), w2 (group g2, project p1),
 * w3 (ungrouped, no project). Each has a hub and one note.
 */
function fleetIndex(): WikiIndex {
  const ws = (wid: string, fg: string | null, fgColor: string | null, pid: string | null) => [
    note(`${wid}::__workspace__`, { kind: 'workspaceHub', workspaceId: wid, focusGroupId: fg, focusGroupColor: fgColor, projectId: pid }),
    note(`${wid}::Home.md`, { title: 'Home', workspaceId: wid, focusGroupId: fg, focusGroupColor: fgColor, projectId: pid }),
  ]
  return {
    workspacePath: '',
    generatedAt: '2026-10-16T00:00:00Z',
    nodes: [
      ...ws('w1', 'g1', '#ff0000', 'p1'),
      ...ws('w2', 'g2', '#00ff00', 'p1'),
      ...ws('w3', null, null, null),
      note('__focusgroup__::g1', { kind: 'focusGroup', focusGroupId: 'g1', focusGroupColor: '#ff0000' }),
      note('__focusgroup__::g2', { kind: 'focusGroup', focusGroupId: 'g2', focusGroupColor: '#00ff00' }),
      note('__project__::p1', { kind: 'project', projectId: 'p1', projectColor: '#0000ff' }),
    ],
    links: [
      link('w1::__workspace__', 'w1::Home.md', { kind: 'workspaceHub' }),
      link('w2::__workspace__', 'w2::Home.md', { kind: 'workspaceHub' }),
      link('w3::__workspace__', 'w3::Home.md', { kind: 'workspaceHub' }),
      link('__focusgroup__::g1', 'w1::__workspace__', { kind: 'focusGroup' }),
      link('__focusgroup__::g2', 'w2::__workspace__', { kind: 'focusGroup' }),
      link('__project__::p1', 'w1::__workspace__', { kind: 'project' }),
      link('__project__::p1', 'w2::__workspace__', { kind: 'project' }),
      // Cross-workspace wikilink (search neighbours escape filters, W7).
      link('w1::Home.md', 'w3::Home.md', { kind: 'wikilink' }),
    ],
    noteCount: 3,
    scope: 'k2',
    groups: [
      { id: 'g1', name: 'Work', color: '#ff0000', workspaceIds: ['w1'] },
      { id: 'g2', name: 'Play', color: '#00ff00', workspaceIds: ['w2'] },
    ],
    projects: [{ id: 'p1', name: 'Proj', color: '#0000ff', workspaceIds: ['w1', 'w2'] }],
  }
}

function ids(index: WikiIndex, o: WikiGraphOpts, dims: 2 | 3 = 2): string[] {
  return buildWikiGraphData(index, o, new Map<string, WikiSimNode>(), dims).nodes.map((n) => n.id)
}

function linkPairs(index: WikiIndex, o: WikiGraphOpts): string[] {
  return buildWikiGraphData(index, o, new Map<string, WikiSimNode>(), 2).links.map(
    (l) => `${String(l.source)}>${String(l.target)}:${l.kind ?? ''}`,
  )
}

describe('buildWikiGraphData: modes', () => {
  it('global shows every note plus missing stubs, sorted by id', () => {
    expect(ids(workspaceIndex(), OPTS)).toEqual(['A', 'B', 'C', 'Ghost', 'Home'])
  })

  it('creates a missing stub for an unknown link endpoint, flagged __missing', () => {
    const data = buildWikiGraphData(workspaceIndex(), OPTS, new Map(), 2)
    const ghost = data.nodes.find((n) => n.id === 'Ghost')
    if (!ghost) throw new Error('no Ghost stub')
    expect([ghost.exists, ghost.__missing, ghost.title, ghost.path]).toEqual([false, true, 'Ghost', ''])
    const ghostLink = data.links.find((l) => l.target === 'Ghost')
    if (!ghostLink) throw new Error('no Ghost link')
    expect(ghostLink.missing).toBe(true)
    expect(data.links.find((l) => l.target === 'A')?.missing).toBe(false)
  })

  it('local shows the centre\'s neighbourhood, a fixed 2 hops deep', () => {
    expect(WIKI_LOCAL_DEPTH).toBe(2)
    expect(ids(workspaceIndex(), opts({ mode: 'local', localCenterId: 'C' }))).toEqual(['A', 'B', 'C'])
    expect(ids(workspaceIndex(), opts({ mode: 'local', localCenterId: 'B' }))).toEqual(['A', 'B', 'C', 'Ghost', 'Home'])
  })

  it('local with no centre (or an unknown one) shows everything', () => {
    expect(ids(workspaceIndex(), opts({ mode: 'local', localCenterId: null }))).toHaveLength(5)
    expect(ids(workspaceIndex(), opts({ mode: 'local', localCenterId: 'Nope' }))).toHaveLength(5)
  })

  it('k2 projects lens: all, project:<id>, and a single workspace id', () => {
    const fleet = fleetIndex()
    const all = ids(fleet, opts({ mode: 'k2', k2Lens: 'projects', projectFilter: 'all' }))
    expect(all).toEqual([
      '__project__::p1',
      'w1::Home.md', 'w1::__workspace__',
      'w2::Home.md', 'w2::__workspace__',
      'w3::Home.md', 'w3::__workspace__',
    ])
    expect(ids(fleet, opts({ mode: 'k2', k2Lens: 'projects', projectFilter: 'project:p1' }))).toEqual([
      '__project__::p1', 'w1::Home.md', 'w1::__workspace__', 'w2::Home.md', 'w2::__workspace__',
    ])
    expect(ids(fleet, opts({ mode: 'k2', k2Lens: 'projects', projectFilter: 'w3' }))).toEqual([
      'w3::Home.md', 'w3::__workspace__',
    ])
  })

  it('k2 groups lens: all, ungrouped, and a group id', () => {
    const fleet = fleetIndex()
    expect(ids(fleet, opts({ mode: 'k2', k2Lens: 'groups', focusGroupFilter: 'all' }))).toEqual([
      '__focusgroup__::g1', '__focusgroup__::g2',
      'w1::Home.md', 'w1::__workspace__',
      'w2::Home.md', 'w2::__workspace__',
      'w3::Home.md', 'w3::__workspace__',
    ])
    expect(ids(fleet, opts({ mode: 'k2', k2Lens: 'groups', focusGroupFilter: 'ungrouped' }))).toEqual([
      'w3::Home.md', 'w3::__workspace__',
    ])
    expect(ids(fleet, opts({ mode: 'k2', k2Lens: 'groups', focusGroupFilter: 'g2' }))).toEqual([
      '__focusgroup__::g2', 'w2::Home.md', 'w2::__workspace__',
    ])
  })

  it('lens edge isolation: no focusGroup edges on projects, no project edges on groups', () => {
    const fleet = fleetIndex()
    const proj = linkPairs(fleet, opts({ mode: 'k2', k2Lens: 'projects' }))
    expect(proj.filter((l) => l.endsWith(':focusGroup'))).toEqual([])
    expect(proj.filter((l) => l.endsWith(':project'))).toHaveLength(2)
    const groups = linkPairs(fleet, opts({ mode: 'k2', k2Lens: 'groups' }))
    expect(groups.filter((l) => l.endsWith(':project'))).toEqual([])
    expect(groups.filter((l) => l.endsWith(':focusGroup'))).toHaveLength(2)
  })

  it('org edges take the source group / project colour, else the fallback', () => {
    const fleet = fleetIndex()
    const groups = buildWikiGraphData(fleet, opts({ mode: 'k2', k2Lens: 'groups' }), new Map(), 2)
    expect(groups.links.filter((l) => l.kind === 'focusGroup').map((l) => l.color)).toEqual(['#ff0000', '#00ff00'])
    const proj = buildWikiGraphData(fleet, opts({ mode: 'k2', k2Lens: 'projects' }), new Map(), 2)
    expect(proj.links.filter((l) => l.kind === 'project').map((l) => l.color)).toEqual(['#0000ff', '#0000ff'])
    fleet.nodes = fleet.nodes.map((n) => (n.id === '__project__::p1' ? { ...n, projectColor: null } : n))
    const fallback = buildWikiGraphData(fleet, opts({ mode: 'k2', k2Lens: 'projects' }), new Map(), 2)
    expect(fallback.links.filter((l) => l.kind === 'project').map((l) => l.color)).toEqual([ORG_EDGE_FALLBACK, ORG_EDGE_FALLBACK])
    expect(fallback.links.filter((l) => l.kind === 'workspaceHub').map((l) => l.color)).toEqual([null, null, null])
  })
})

describe('buildWikiGraphData: search', () => {
  it('shows matches plus their 1-hop neighbours and returns matchedIds', () => {
    const data = buildWikiGraphData(workspaceIndex(), opts({ search: 'infra' }), new Map(), 2)
    expect(data.nodes.map((n) => n.id)).toEqual(['A', 'B', 'Ghost', 'Home'])
    expect([...(data.matchedIds ?? new Set(['matchedIds was null']))]).toEqual(['A'])
  })

  it('matchedIds is null when the search is empty or blank', () => {
    expect(buildWikiGraphData(workspaceIndex(), OPTS, new Map(), 2).matchedIds).toBeNull()
    expect(buildWikiGraphData(workspaceIndex(), opts({ search: '   ' }), new Map(), 2).matchedIds).toBeNull()
  })

  it('pins W7: neighbours from index.links can fall outside the K2 filter', () => {
    // Filter to w1 only; the search matches w1's Home, whose wikilink
    // neighbour is w3's Home (outside the filter). It still shows.
    const data = buildWikiGraphData(
      fleetIndex(),
      opts({ mode: 'k2', k2Lens: 'projects', projectFilter: 'w1', search: 'home' }),
      new Map(),
      2,
    )
    expect([...(data.matchedIds ?? new Set<string>())]).toEqual(['w1::Home.md'])
    expect(data.nodes.map((n) => n.id)).toEqual(['w1::Home.md', 'w1::__workspace__', 'w3::Home.md'])
  })

  it('pins W7: neighbours can fall outside the Local set', () => {
    // Local around Ghost (2 hops) = {Ghost, A, Home, B}; searching "B" adds C (B's neighbour), outside the set.
    const data = buildWikiGraphData(
      workspaceIndex(),
      opts({ mode: 'local', localCenterId: 'Ghost', search: 'B' }),
      new Map(),
      2,
    )
    expect(data.nodes.map((n) => n.id)).toEqual(['A', 'B', 'C'])
  })
})

describe('buildWikiGraphData: sim reuse', () => {
  it('two builds with the same index return the same node objects, positions unchanged', () => {
    const sim = new Map<string, WikiSimNode>()
    const index = workspaceIndex()
    const first = buildWikiGraphData(index, OPTS, sim, 2)
    first.nodes.forEach((n, i) => {
      n.x = i * 10
      n.y = i * -10
    })
    const second = buildWikiGraphData(index, OPTS, sim, 2)
    expect(second.nodes.length).toBe(first.nodes.length)
    second.nodes.forEach((n, i) => {
      expect(n).toBe(first.nodes[i])
      expect([n.x, n.y]).toEqual([i * 10, i * -10])
    })
  })

  it('a new node spawns within 36 units of a known neighbour (2D and 3D)', () => {
    for (const dims of [2, 3] as const) {
      const sim = new Map<string, WikiSimNode>()
      const index = workspaceIndex()
      buildWikiGraphData(index, OPTS, sim, dims)
      const c = sim.get('C')
      if (!c) throw new Error('no C')
      c.x = 1000
      c.y = 2000
      if (dims === 3) c.z = 3000
      // Remove B from the sim so it respawns; its only known neighbour with a position near C is C (A is random).
      sim.delete('B')
      const a = sim.get('A')
      if (!a) throw new Error('no A')
      sim.delete('A') // B's neighbours: A (gone) and C.
      const next = { ...index, nodes: index.nodes.filter((n) => n.id !== 'A'), links: [link('B', 'C')] }
      buildWikiGraphData(next, OPTS, sim, dims)
      const b = sim.get('B')
      if (!b || b.x == null || b.y == null) throw new Error('B did not respawn with a position')
      expect(Math.abs(b.x - 1000)).toBeLessThanOrEqual(18)
      expect(Math.abs(b.y - 2000)).toBeLessThanOrEqual(18)
      if (dims === 3) {
        if (b.z == null) throw new Error('3D spawn has no z')
        expect(Math.abs(b.z - 3000)).toBeLessThanOrEqual(18)
        expect(b.vz).toBe(0)
      } else {
        expect('z' in b).toBe(false)
      }
    }
  })

  it('a removed node leaves the sim map', () => {
    const sim = new Map<string, WikiSimNode>()
    buildWikiGraphData(workspaceIndex(), OPTS, sim, 2)
    expect(sim.has('C')).toBe(true)
    const index = workspaceIndex()
    index.nodes = index.nodes.filter((n) => n.id !== 'C')
    index.links = index.links.filter((l) => l.target !== 'C')
    buildWikiGraphData(index, OPTS, sim, 2)
    expect(sim.has('C')).toBe(false)
  })

  it('dims=3 seeds and keeps z', () => {
    const sim = new Map<string, WikiSimNode>()
    const first = buildWikiGraphData(workspaceIndex(), OPTS, sim, 3)
    for (const n of first.nodes) {
      if (typeof n.z !== 'number') throw new Error(`${n.id} has no z`)
      n.z = 42
    }
    const second = buildWikiGraphData(workspaceIndex(), OPTS, sim, 3)
    expect(second.nodes.map((n) => n.z)).toEqual(second.nodes.map(() => 42))
  })

  it('updates metadata on a reused node', () => {
    const sim = new Map<string, WikiSimNode>()
    const index = workspaceIndex()
    const first = buildWikiGraphData(index, OPTS, sim, 2)
    const a = first.nodes.find((n) => n.id === 'A')
    const renamed = { ...index, nodes: index.nodes.map((n) => (n.id === 'A' ? { ...n, title: 'Alpha' } : n)) }
    const second = buildWikiGraphData(renamed, OPTS, sim, 2)
    const a2 = second.nodes.find((n) => n.id === 'A')
    expect(a2).toBe(a)
    expect(a2?.title).toBe('Alpha')
  })
})

describe('selection and counts', () => {
  it('selecting in Global/K2 does not change the output (no localCenterId)', () => {
    expect(localCenterIdFor('global', 'A')).toBeNull()
    expect(localCenterIdFor('k2', 'A')).toBeNull()
    expect(localCenterIdFor('local', 'A')).toBe('A')
    const index = workspaceIndex()
    const a = ids(index, opts({ localCenterId: localCenterIdFor('global', 'A') }))
    const b = ids(index, opts({ localCenterId: localCenterIdFor('global', 'C') }))
    expect(a).toEqual(b)
  })

  it('countWikiGraphNodes matches the built node count and leaves no sim state', () => {
    const fleet = fleetIndex()
    for (const o of [
      OPTS,
      opts({ mode: 'k2', k2Lens: 'groups', focusGroupFilter: 'g1' }),
      opts({ mode: 'k2', k2Lens: 'projects', search: 'home' }),
    ]) {
      expect(countWikiGraphNodes(fleet, o)).toBe(ids(fleet, o).length)
    }
  })

  it('a search keystroke rebuild on a ~550-node map is fast', () => {
    const nodes: WikiNode[] = []
    const links: WikiLink[] = []
    for (let i = 0; i < 550; i++) {
      nodes.push(note(`n${i}`, { tags: i % 7 === 0 ? ['seven'] : [] }))
      if (i > 0) links.push(link(`n${i}`, `n${Math.floor(i / 3)}`))
      if (i > 10) links.push(link(`n${i}`, `n${i - 10}`))
    }
    const index: WikiIndex = { workspacePath: '/w', generatedAt: '', nodes, links, noteCount: 550, scope: 'workspace' }
    const sim = new Map<string, WikiSimNode>()
    buildWikiGraphData(index, OPTS, sim, 3)
    const t0 = performance.now()
    const data = buildWikiGraphData(index, opts({ search: 'seven' }), sim, 3)
    const ms = performance.now() - t0
    expect(data.matchedIds?.size).toBe(79)
    expect(ms).toBeLessThan(50)
  })
})
