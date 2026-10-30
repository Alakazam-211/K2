import { describe, expect, it } from 'vitest'
import { DaemonHttpError } from './daemon-http-error'
import { accessLossOf, appsBlockingRemovalOf, lossParts } from './app-access-loss'

describe('app access loss (0.46.7 uninstall refusals)', () => {
  it('words only the counts above zero, singular and plural', () => {
    expect(
      lossParts({ logins: 2, chatGrants: 1, otherGrants: 0, ownerGrants: 1, secretShares: null, spaces: 3, total: 7 }),
    ).toEqual(['2 App logins', '1 chat grant the App gave', '1 owner grant to the App', '3 chat Spaces bound to it'])
    expect(lossParts({ secretShares: 1 })).toEqual(['1 secret shared with it'])
    expect(lossParts(null)).toEqual([])
  })

  it('reads a single uninstall refusal (wouldDrop) and a release refusal (apps)', () => {
    const one = new DaemonHttpError('uninstalling jobs would drop 1 App login…', 409, 'uninstall_drops_access', {
      code: 'uninstall_drops_access',
      wouldDrop: { logins: 1, total: 1 },
    })
    expect(accessLossOf(one, 'jobs')).toEqual([{ app: 'jobs', parts: ['1 App login'] }])
    const all = new DaemonHttpError('nothing was released…', 409, 'uninstall_drops_access', {
      code: 'uninstall_drops_access',
      released: [],
      apps: [
        { app: 'jobs', wouldDrop: { logins: 40, spaces: 1 } },
        { app: 'crm', wouldDrop: { secretShares: 2 } },
      ],
    })
    expect(accessLossOf(all)).toEqual([
      { app: 'jobs', parts: ['40 App logins', '1 chat Space bound to it'] },
      { app: 'crm', parts: ['2 secrets shared with it'] },
    ])
    // The body-in-the-message shape some callers throw.
    const raw = new Error(JSON.stringify({ error: { code: 'uninstall_drops_access', apps: [{ app: 'x', wouldDrop: { logins: 1 } }] } }))
    expect(accessLossOf(raw)).toEqual([{ app: 'x', parts: ['1 App login'] }])
  })

  it('is null for any other refusal', () => {
    expect(accessLossOf(new DaemonHttpError('no', 403, 'owner_only', { code: 'owner_only' }), 'jobs')).toBeNull()
    expect(accessLossOf(new Error('network down'))).toBeNull()
    expect(appsBlockingRemovalOf(new DaemonHttpError('poc', 409, 'poc_of_projects', null))).toBeNull()
  })

  it("reads a removal's workspace_has_apps refusal", () => {
    const e = new DaemonHttpError('Sales maintains Apps…', 409, 'workspace_has_apps', {
      code: 'workspace_has_apps',
      apps: [
        { app: 'jobs', state: 'running' },
        { app: 'crm', state: 'stopped' },
      ],
    })
    expect(appsBlockingRemovalOf(e)).toEqual(['jobs', 'crm'])
  })
})
