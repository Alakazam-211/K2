// A gesture's effect on the chart, applied at once while the daemon call is
// in flight (optimistic; the daemon's answer, or the next `org_chart_changed`
// refetch, replaces it). Pure: each returns a new chart.

import type { OrgChart, OrgLineKind, OrgRef } from './org-api'

const without = (list: string[], id: string): string[] => list.filter((x) => x !== id)

/** Out of whatever org it was in (one org per agent), and its placement
 *  there with it (a position inside a bucket means nothing in another). */
function leave(chart: OrgChart, agent: string): OrgChart {
  return {
    ...chart,
    orgs: chart.orgs.map((o) =>
      o.memberIds.includes(agent)
        ? { ...o, memberIds: without(o.memberIds, agent), headId: o.headId === agent ? null : o.headId }
        : o,
    ),
    nodes: chart.nodes.filter((n) => !(n.kind === 'agent' && n.id === agent)),
  }
}

export function joinOrg(chart: OrgChart, org: string, agent: string): OrgChart {
  const left = leave(chart, agent)
  return { ...left, orgs: left.orgs.map((o) => (o.id === org ? { ...o, memberIds: [...o.memberIds, agent] } : o)) }
}

export function leaveOrg(chart: OrgChart, agent: string): OrgChart {
  return leave(chart, agent)
}

export function makeHead(chart: OrgChart, org: string, agent: string): OrgChart {
  return { ...chart, orgs: chart.orgs.map((o) => (o.id === org ? { ...o, headId: agent } : o)) }
}

export function addLine(chart: OrgChart, id: string, kind: OrgLineKind, a: OrgRef, b: OrgRef): OrgChart {
  return { ...chart, lines: [...chart.lines, { id, kind, a, b }] }
}

export function removeLine(chart: OrgChart, id: string): OrgChart {
  return { ...chart, lines: chart.lines.filter((l) => l.id !== id) }
}

/** A person's drag: this node stays where it was put. */
export function place(chart: OrgChart, node: OrgRef, x: number, y: number): OrgChart {
  const old = chart.nodes.find((n) => n.kind === node.kind && n.id === node.id)
  const rest = chart.nodes.filter((n) => !(n.kind === node.kind && n.id === node.id))
  return { ...chart, nodes: [...rest, { kind: node.kind, id: node.id, x, y, w: old?.w ?? null, h: old?.h ?? null, placedBy: 'person' }] }
}

/** "Tidy up": every node back on auto. */
export function tidy(chart: OrgChart): OrgChart {
  return { ...chart, nodes: chart.nodes.filter((n) => n.placedBy !== 'person') }
}

export function createOrg(chart: OrgChart, id: string, name: string, purpose = ''): OrgChart {
  return { ...chart, orgs: [...chart.orgs, { id, name, purpose, headId: null, memberIds: [], bulletin: null }] }
}

export function renameOrg(chart: OrgChart, org: string, name: string): OrgChart {
  return { ...chart, orgs: chart.orgs.map((o) => (o.id === org ? { ...o, name } : o)) }
}

export function setPurpose(chart: OrgChart, org: string, purpose: string): OrgChart {
  return { ...chart, orgs: chart.orgs.map((o) => (o.id === org ? { ...o, purpose } : o)) }
}

export function setBulletin(chart: OrgChart, org: string, lines: string[], by: string, at: number): OrgChart {
  return {
    ...chart,
    orgs: chart.orgs.map((o) => (o.id === org ? { ...o, bulletin: { lines, updatedAt: at, updatedBy: by } } : o)),
  }
}

/** Deleting an org: its members leave, its lines and its placement go. */
export function deleteOrg(chart: OrgChart, org: string): OrgChart {
  return {
    ...chart,
    orgs: chart.orgs.filter((o) => o.id !== org),
    lines: chart.lines.filter((l) => !((l.a.kind === 'org' && l.a.id === org) || (l.b.kind === 'org' && l.b.id === org))),
    nodes: chart.nodes.filter((n) => !(n.kind === 'org' && n.id === org)),
  }
}

/** The org an agent is in, or null. */
export function orgOfAgent(chart: OrgChart, agent: string): string | null {
  return chart.orgs.find((o) => o.memberIds.includes(agent))?.id ?? null
}
