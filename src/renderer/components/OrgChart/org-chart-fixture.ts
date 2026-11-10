// A small chart for the Org Chart's tests, in the daemon's shape: three
// agents here and one on another server (`remote:far@other.k2.dev`), two
// orgs (Billing empty, Support with Bo as head), one connection (Ada ↔ Bo),
// an Owner looking (may manage and arrange). `nodes` is empty: the test
// daemon (org-api.fake.ts) lays the chart out when it is read. Only tests
// import it.

import type { OrgChart } from './org-api'

/** The agent on another server, as the daemon names it. */
export const FAR = 'remote:far@other.k2.dev'

export function orgChartFixture(over: Partial<OrgChart> = {}): OrgChart {
  return {
    revision: 1,
    canEdit: true,
    canManage: true,
    canArrange: true,
    card: { w: 220, h: 72 },
    agents: [
      { id: 'ada', name: 'Ada', color: null, iconUrl: null, server: null, role: 'Invoices and refunds', canManage: false },
      { id: 'bo', name: 'Bo', color: null, iconUrl: null, server: null, role: null, canManage: false },
      { id: 'cy', name: 'Cy', color: null, iconUrl: null, server: null, role: null, canManage: true },
      { id: FAR, name: 'far (other.k2.dev)', color: null, iconUrl: null, server: 'other.k2.dev', role: null, canManage: false },
    ],
    orgs: [
      { id: 'org-billing', name: 'Billing', purpose: 'Money in and out.', headId: null, memberIds: [], bulletin: null },
      {
        id: 'org-support',
        name: 'Support',
        purpose: 'Help for customers.',
        headId: 'bo',
        memberIds: ['bo'],
        bulletin: { lines: ['Tickets go here.', 'This week: the new inbox.'], updatedAt: 1, updatedBy: 'bo' },
      },
    ],
    lines: [],
    connections: [{ a: 'ada', b: 'bo' }],
    nodes: [],
    ...over,
  }
}
