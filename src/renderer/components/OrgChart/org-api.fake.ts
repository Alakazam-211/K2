// A stand-in daemon for the Org Chart's tests: the `OrgApi` over an
// in-memory chart, keeping the daemon's rules (one org per agent, the head is
// a member, line rules, stale revisions) and recording every call by route.
// Only tests import it.

import type { OrgApi, OrgChart, OrgLineKind, OrgRef } from './org-api'
import { checkLine } from './org-rules'
import * as ops from './org-ops'

export interface FakeCall {
  route: string
  body: Record<string, unknown>
}

/** Shaped like `DaemonHttpError` (a `code` beside the message). */
export class FakeDaemonError extends Error {
  readonly code: string
  constructor(code: string, message: string) {
    super(message)
    this.code = code
  }
}

export interface FakeOrgApi extends OrgApi {
  calls: FakeCall[]
  state: OrgChart
  /** Calls to one route, in order. */
  callsTo(route: string): FakeCall[]
  /** The next call to `route` fails once with this code. */
  failNext(route: string, code: string, message?: string): void
}

export function fakeOrgApi(initial: OrgChart): FakeOrgApi {
  let seq = 0
  const fails = new Map<string, { code: string; message: string }>()
  const api: FakeOrgApi = {
    calls: [],
    state: structuredClone(initial),
    callsTo: (route) => api.calls.filter((c) => c.route === route),
    failNext: (route, code, message = code) => void fails.set(route, { code, message }),
    chart: async () => {
      api.calls.push({ route: 'chart', body: {} })
      return structuredClone(api.state)
    },
    create: async (name, purpose) => write('create', { name, purpose }, (c) => ops.createOrg(c, `org-${++seq}`, name, purpose ?? '')),
    rename: async (org, name) => write('rename', { org, name }, (c) => ops.renameOrg(c, org, name)),
    setPurpose: async (org, purpose) => write('purpose', { org, purpose }, (c) => ops.setPurpose(c, org, purpose)),
    deleteOrg: async (org) => write('delete', { org }, (c) => ops.deleteOrg(c, org)),
    addMember: async (org, agent) =>
      write('member/add', { org, agent }, (c) => {
        const cur = ops.orgOfAgent(c, agent)
        if (cur && cur !== org) throw new FakeDaemonError('agent_in_another_org', `${agent} is already in ${cur}.`)
        return ops.joinOrg(c, org, agent)
      }),
    moveMember: async (agent, org) => write('member/move', { agent, org }, (c) => ops.joinOrg(c, org, agent)),
    removeMember: async (org, agent) => write('member/remove', { org, agent }, (c) => ops.leaveOrg(c, agent)),
    setHead: async (org, agent) =>
      write('head', { org, agent }, (c) => {
        if (ops.orgOfAgent(c, agent) !== org) throw new FakeDaemonError('head_not_member', 'The head must be a member.')
        return ops.makeHead(c, org, agent)
      }),
    addLine: async (kind: OrgLineKind, a: OrgRef, b: OrgRef) =>
      write('line/add', { kind, a, b }, (c) => {
        const no = checkLine(c, kind, a, b)
        if (no) throw new FakeDaemonError(no.code, no.message)
        return ops.addLine(c, `line-${++seq}`, kind, a, b)
      }),
    removeLine: async (id) => write('line/remove', { id }, (c) => ops.removeLine(c, id)),
    move: async (node, x, y, revision) =>
      write('layout/move', { kind: node.kind, id: node.id, x, y, revision }, (c) => {
        if (revision !== c.revision) throw new FakeDaemonError('stale_revision', 'The chart changed; reload and try again.')
        return ops.place(c, node, x, y)
      }),
    tidy: async (revision) => write('layout/tidy', { revision }, (c) => ops.tidy(c)),
    setBulletin: async (org, lines) => write('bulletin/set', { org, lines }, (c) => ops.setBulletin(c, org, lines, 'person:owner', 1)),
  }
  async function write(route: string, body: Record<string, unknown>, apply: (c: OrgChart) => OrgChart): Promise<void> {
    api.calls.push({ route, body })
    const f = fails.get(route)
    if (f) {
      fails.delete(route)
      throw new FakeDaemonError(f.code, f.message)
    }
    const next = apply(api.state)
    api.state = { ...next, revision: api.state.revision + 1 }
  }
  return api
}
