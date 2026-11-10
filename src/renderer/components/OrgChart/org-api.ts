// The Org Chart's one door to the daemon (prd-org-chart-v1 §6.2). Every org,
// member, head, line, placed position and bulletin is daemon state under
// `/cli/organization/*`; this file is the only place the renderer names those
// routes or reads their shapes. The tab draws `chart` and turns each gesture
// into one call here.
//
// Shapes (the renderer's side of the contract; the daemon builder owns the
// routes, so a drift fails `parseOrgChart` loudly instead of drawing junk):
//
//   GET  organization/chart → OrgChart
//   POST organization/create         {name, purpose?}
//   POST organization/rename         {org, name}
//   POST organization/purpose        {org, purpose}            purpose ≤ 400 chars (Rosson 11-10, answer 3)
//   POST organization/delete         {org}
//   POST organization/member/add     {org, agent}
//   POST organization/member/move    {agent, org}
//   POST organization/member/remove  {org, agent}
//   POST organization/head           {org, agent}
//   POST organization/line/add       {kind, a, b}              below: a sits above b
//   POST organization/line/remove    {id}
//   POST organization/layout/move    {kind, id, x, y, revision} agent in a bucket: x/y relative to it
//   POST organization/layout/tidy    {revision}
//   POST organization/bulletin/set   {org, lines}              ≤ 5 lines, ≤ 160 chars each
//
// Every write bumps the chart's revision and the daemon emits
// `org_chart_changed {revision}` on the app bus; the tab refetches on it.
//
// The daemon's side of these shapes is `crates/k2-core/src/org_chart/wire.rs`
// (mirrored in full in `lib/org-chart-types.ts`). The chart is laid out by
// the daemon: `nodes` is the truth, one row per org and agent.
//
// "Manage the org chart" (per agent) is not an organization route:
//   POST agent-access/set {toggle: 'org_chart_manage', workspace, value}
// The Owner turns it on; the Owner or an Admin turns it off. Each chart
// agent's `canManage` reads it back.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'

export type OrgNodeKind = 'agent' | 'org'

/** One end of a line, or a node on the canvas. */
export interface OrgRef {
  kind: OrgNodeKind
  id: string
}

export type OrgLineKind = 'below' | 'beside'

export interface OrgAgent {
  /** This server's agents: the workspace (project) id. Agents on other
   *  servers: `remote:<agent>@<host>`. */
  id: string
  name: string
  color: string | null
  iconUrl: string | null
  /** null: an agent on this server. Otherwise the federated server it lives
   *  on (shown only when one of this server's agents can talk to it). */
  server: string | null
  /** What the agent does, from its AGENTS.md (no separate role field);
   *  null when it has none. */
  role: string | null
  /** Its "Manage the org chart" switch. */
  canManage: boolean
}

export interface OrgBulletin {
  lines: string[]
  /** Unix seconds. */
  updatedAt: number
  /** `owner`, `login:<user>`, `agent:<handle>`. */
  updatedBy: string
}

export interface Org {
  id: string
  name: string
  purpose: string
  headId: string | null
  memberIds: string[]
  bulletin: OrgBulletin | null
}

export interface OrgLine {
  id: string
  kind: OrgLineKind
  /** For `below`, `a` sits above `b`. For `beside`, the order means nothing. */
  a: OrgRef
  b: OrgRef
}

/** Who may message whom (K2's existing connections), deduped: one pair per
 *  link. Drawn as the dotted layer. */
export interface OrgConnection {
  a: string
  b: string
}

/** One node of the daemon's layout. An org's x/y/w/h and a loose agent's
 *  x/y are canvas coordinates; a member's x/y is relative to its bucket (as
 *  `layout/move` takes it). `person`: a person dragged it there. */
export interface OrgPlacement {
  kind: OrgNodeKind
  id: string
  x: number
  y: number
  w: number | null
  h: number | null
  placedBy: 'auto' | 'person'
}

export interface OrgChart {
  revision: number
  /** An Owner or Admin person: writes any org's bulletin. A Member login
   *  sees the chart view only. */
  canEdit: boolean
  /** The caller may change orgs, members, heads and lines. */
  canManage: boolean
  /** The caller may drag and tidy (Owner and Admin people only). */
  canArrange: boolean
  /** One card's size on the canvas. */
  card: { w: number; h: number }
  agents: OrgAgent[]
  orgs: Org[]
  lines: OrgLine[]
  connections: OrgConnection[]
  nodes: OrgPlacement[]
}

export const PURPOSE_MAX = 400
export const BULLETIN_MAX_LINES = 5
export const BULLETIN_LINE_MAX = 160
export const ORG_NAME_MAX = 40

// ── parsing (fails loudly on a shape the renderer doesn't know) ─────────

type Obj = Record<string, unknown>

function obj(v: unknown, what: string): Obj {
  if (!v || typeof v !== 'object' || Array.isArray(v)) throw new Error(`org chart: ${what} is not an object`)
  return v as Obj
}
function str(o: Obj, k: string, what: string): string {
  const v = o[k]
  if (typeof v !== 'string') throw new Error(`org chart: ${what}.${k} is not a string`)
  return v
}
function strOrNull(o: Obj, k: string, what: string): string | null {
  const v = o[k]
  if (v === undefined || v === null) return null
  if (typeof v !== 'string') throw new Error(`org chart: ${what}.${k} is not a string`)
  return v
}
function num(o: Obj, k: string, what: string): number {
  const v = o[k]
  if (typeof v !== 'number' || !Number.isFinite(v)) throw new Error(`org chart: ${what}.${k} is not a number`)
  return v
}
function numOrNull(o: Obj, k: string, what: string): number | null {
  const v = o[k]
  if (v === undefined || v === null) return null
  if (typeof v !== 'number' || !Number.isFinite(v)) throw new Error(`org chart: ${what}.${k} is not a number`)
  return v
}
function arr(o: Obj, k: string, what: string): unknown[] {
  const v = o[k]
  if (!Array.isArray(v)) throw new Error(`org chart: ${what}.${k} is not a list`)
  return v
}
function kind(v: unknown, what: string): OrgNodeKind {
  if (v !== 'agent' && v !== 'org') throw new Error(`org chart: ${what} is not "agent" or "org"`)
  return v
}
function ref(v: unknown, what: string): OrgRef {
  const o = obj(v, what)
  return { kind: kind(o.kind, `${what}.kind`), id: str(o, 'id', what) }
}

function bool(o: Obj, k: string, what: string): boolean {
  const v = o[k]
  if (typeof v !== 'boolean') throw new Error(`org chart: ${what}.${k} is not a boolean`)
  return v
}

export function parseOrgChart(raw: unknown): OrgChart {
  const o = obj(raw, 'the chart')
  const card = obj(o.card, 'chart.card')
  return {
    revision: num(o, 'revision', 'chart'),
    canEdit: bool(o, 'canEdit', 'chart'),
    canManage: bool(o, 'canManage', 'chart'),
    canArrange: bool(o, 'canArrange', 'chart'),
    card: { w: num(card, 'w', 'chart.card'), h: num(card, 'h', 'chart.card') },
    agents: arr(o, 'agents', 'chart').map((v, i) => {
      const a = obj(v, `agents[${i}]`)
      return {
        id: str(a, 'id', `agents[${i}]`),
        name: str(a, 'name', `agents[${i}]`),
        color: strOrNull(a, 'color', `agents[${i}]`),
        iconUrl: strOrNull(a, 'iconUrl', `agents[${i}]`),
        server: strOrNull(a, 'server', `agents[${i}]`),
        role: strOrNull(a, 'role', `agents[${i}]`) || null,
        canManage: bool(a, 'canManage', `agents[${i}]`),
      }
    }),
    orgs: arr(o, 'orgs', 'chart').map((v, i) => {
      const g = obj(v, `orgs[${i}]`)
      const b = g.bulletin === null || g.bulletin === undefined ? null : obj(g.bulletin, `orgs[${i}].bulletin`)
      return {
        id: str(g, 'id', `orgs[${i}]`),
        name: str(g, 'name', `orgs[${i}]`),
        purpose: str(g, 'purpose', `orgs[${i}]`),
        headId: strOrNull(g, 'headId', `orgs[${i}]`),
        memberIds: arr(g, 'memberIds', `orgs[${i}]`).map((m, j) => {
          if (typeof m !== 'string') throw new Error(`org chart: orgs[${i}].memberIds[${j}] is not a string`)
          return m
        }),
        bulletin: b
          ? {
              lines: arr(b, 'lines', `orgs[${i}].bulletin`).map((l, j) => {
                if (typeof l !== 'string') throw new Error(`org chart: orgs[${i}].bulletin.lines[${j}] is not a string`)
                return l
              }),
              updatedAt: num(b, 'updatedAt', `orgs[${i}].bulletin`),
              updatedBy: str(b, 'updatedBy', `orgs[${i}].bulletin`),
            }
          : null,
      }
    }),
    lines: arr(o, 'lines', 'chart').map((v, i) => {
      const l = obj(v, `lines[${i}]`)
      const k = l.kind
      if (k !== 'below' && k !== 'beside') throw new Error(`org chart: lines[${i}].kind is not "below" or "beside"`)
      return { id: str(l, 'id', `lines[${i}]`), kind: k, a: ref(l.a, `lines[${i}].a`), b: ref(l.b, `lines[${i}].b`) }
    }),
    connections: arr(o, 'connections', 'chart').map((v, i) => {
      const c = obj(v, `connections[${i}]`)
      return { a: str(c, 'a', `connections[${i}]`), b: str(c, 'b', `connections[${i}]`) }
    }),
    nodes: arr(o, 'nodes', 'chart').map((v, i) => {
      const n = obj(v, `nodes[${i}]`)
      const by = n.placedBy
      if (by !== 'auto' && by !== 'person') throw new Error(`org chart: nodes[${i}].placedBy is not "auto" or "person"`)
      return {
        kind: kind(n.kind, `nodes[${i}].kind`),
        id: str(n, 'id', `nodes[${i}]`),
        x: num(n, 'x', `nodes[${i}]`),
        y: num(n, 'y', `nodes[${i}]`),
        w: numOrNull(n, 'w', `nodes[${i}]`),
        h: numOrNull(n, 'h', `nodes[${i}]`),
        placedBy: by,
      }
    }),
  }
}

// ── the client ──────────────────────────────────────────────────────────

/** Every call the tab makes. The page takes one of these (tests hand it a
 *  fake); `orgApiFor(scope)` is the daemon's. */
export interface OrgApi {
  chart(): Promise<OrgChart>
  create(name: string, purpose?: string): Promise<void>
  rename(org: string, name: string): Promise<void>
  setPurpose(org: string, purpose: string): Promise<void>
  deleteOrg(org: string): Promise<void>
  addMember(org: string, agent: string): Promise<void>
  moveMember(agent: string, org: string): Promise<void>
  removeMember(org: string, agent: string): Promise<void>
  setHead(org: string, agent: string): Promise<void>
  addLine(kind: OrgLineKind, a: OrgRef, b: OrgRef): Promise<void>
  removeLine(id: string): Promise<void>
  move(node: OrgRef, x: number, y: number, revision: number): Promise<void>
  tidy(revision: number): Promise<void>
  setBulletin(org: string, lines: string[]): Promise<void>
}

export function orgApiFor(scope: ServerScope): OrgApi {
  const post = async (route: string, body: Record<string, unknown>): Promise<void> => {
    await daemonCliPost<unknown>(scope, `organization/${route}`, body)
  }
  return {
    chart: async () => parseOrgChart(await daemonCliGet<unknown>(scope, 'organization/chart')),
    create: (name, purpose) => post('create', purpose ? { name, purpose } : { name }),
    rename: (org, name) => post('rename', { org, name }),
    setPurpose: (org, purpose) => post('purpose', { org, purpose }),
    deleteOrg: (org) => post('delete', { org }),
    addMember: (org, agent) => post('member/add', { org, agent }),
    moveMember: (agent, org) => post('member/move', { agent, org }),
    removeMember: (org, agent) => post('member/remove', { org, agent }),
    setHead: (org, agent) => post('head', { org, agent }),
    addLine: (kind, a, b) => post('line/add', { kind, a, b }),
    removeLine: (id) => post('line/remove', { id }),
    move: (node, x, y, revision) => post('layout/move', { kind: node.kind, id: node.id, x, y, revision }),
    tidy: (revision) => post('layout/tidy', { revision }),
    setBulletin: (org, lines) => post('bulletin/set', { org, lines }),
  }
}

/** "Manage the org chart" for one agent (the Owner turns it on; the Owner or
 *  an Admin turns it off). `workspace`: the agent's workspace path. */
export async function setAgentCanManageOrgChart(scope: ServerScope, workspace: string, value: boolean): Promise<void> {
  await daemonCliPost<unknown>(scope, 'agent-access/set', { toggle: 'org_chart_manage', workspace, value })
}
