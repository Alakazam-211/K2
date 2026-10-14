// Garden v3 (PRD §9.5, Q12): a Garden's "Review N pending" groups the
// proposed sources its cards name by server, for B2's batch dialog.
import { afterEach, describe, expect, it } from 'vitest'
import { createZenSourcesReader } from './zen-sources'
import { fakeSources, fixtureView } from './zen-sources-fixtures'
import { registerZenSourcesBatchReview, zenPendingCount, zenPendingSources, zenSourcesBatchOpener } from './zen-sources-review'

afterEach(() => registerZenSourcesBatchReview(null))

describe('zenPendingSources', () => {
  it('groups proposed sources by server; active, unreadable and repeated ones are left out', async () => {
    const f = fakeSources(['akzm.k2.dev'])
    f.servers.get('local').views.set('fleet', fixtureView({ status: 'proposed', phase: 'pending', display: 'pending' }))
    f.servers.get('local').views.set('money', fixtureView({ name: 'money' }))
    f.servers.get('akzm.k2.dev').views.set('deploys', fixtureView({ name: 'deploys', status: 'proposed' }))
    f.servers.get('akzm.k2.dev').views.set('alpha', fixtureView({ name: 'alpha', status: 'proposed' }))
    const r = createZenSourcesReader(f.deps)
    const by = await zenPendingSources(
      ['fleet', 'money', 'deploys::akzm.k2.dev', 'alpha::AKZM.k2.dev', 'fleet', 'gone::nope.k2.dev', 'Bad Address'],
      r,
    )
    expect(by).toEqual({ local: ['fleet'], 'akzm.k2.dev': ['alpha', 'deploys'] })
    expect(zenPendingCount(by)).toBe(3)
  })

  it('the opener is whatever the dialog registered', () => {
    expect(zenSourcesBatchOpener()).toBeNull()
    const calls: unknown[] = []
    registerZenSourcesBatchReview((a) => void calls.push(a))
    zenSourcesBatchOpener()?.({ gardenName: 'Fleet', byServer: { local: ['fleet'] } })
    expect(calls).toEqual([{ gardenName: 'Fleet', byServer: { local: ['fleet'] } }])
  })
})
