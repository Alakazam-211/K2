// 0159: the Workspace panel's Connected Agents rows read
// `remoteWorkspaceGone` and paint a gone connection with its reason.
import { describe, expect, it } from 'vitest'
import { parseConnectionRows, remoteConnRowView } from './connected-agents-rows'

describe('Connected Agents federated rows', () => {
  it('a live row reads federated · host (unchanged)', () => {
    expect(remoteConnRowView({ remote: true, agent: 'ai', host: 'rpm.k2.dev', address: 'ai::rpm.k2.dev' })).toEqual({
      title: 'ai',
      detail: 'federated · rpm.k2.dev',
      gone: false,
    })
  })

  it('an older daemon without the field reads live', () => {
    const [row] = parseConnectionRows(JSON.stringify({ connections: [{ remote: true, agent: 'ai', host: 'rpm.k2.dev' }] }))
    expect(remoteConnRowView(row).gone).toBe(false)
  })

  it('a gone row says the workspace no longer exists on that server', () => {
    const [row] = parseConnectionRows({
      connections: [{ remote: true, agent: 'ai', host: 'rpm.k2.dev', address: 'ai::rpm.k2.dev', remoteWorkspaceGone: true }],
    })
    expect(remoteConnRowView(row)).toEqual({
      title: 'ai',
      detail: 'Workspace no longer exists on rpm.k2.dev. Re-add to reconnect.',
      gone: true,
    })
  })
})
