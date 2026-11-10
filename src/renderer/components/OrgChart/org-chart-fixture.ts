// A small chart for the Org Chart's tests: three agents here and one on
// another server, two orgs (Billing empty, Support with Bo as head), one
// connection (Ada ↔ Bo). Only tests import it.

import type { OrgChart } from './org-api'

export function orgChartFixture(over: Partial<OrgChart> = {}): OrgChart {
  return {
    revision: 1,
    canEdit: true,
    agents: [
      { id: 'ada', name: 'Ada', color: null, iconUrl: null, server: null, role: 'Invoices and refunds' },
      { id: 'bo', name: 'Bo', color: null, iconUrl: null, server: null, role: null },
      { id: 'cy', name: 'Cy', color: null, iconUrl: null, server: null, role: null },
      { id: 'far', name: 'Far', color: null, iconUrl: null, server: 'other.k2.dev', role: null },
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
