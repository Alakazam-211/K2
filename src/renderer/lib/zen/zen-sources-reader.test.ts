// Garden v3 data sources (contract-sources-v1 §12.3 "Reader") — the Garden's
// reader against fake servers: local reads use the local scope, `::host`
// only saved servers, 8 source servers per Garden (local not counted), the
// socket for the window's server and 20 s polls for the rest while visible,
// `unreachable` with the last view greyed, `removed`.
import { describe, expect, it } from 'vitest'
import { createZenSourcesReader, parseSourceAddress, zenSourceErrorCode, ZenSourceError, ZEN_SOURCES_POLL_MS, type ZenSourceView } from './zen-sources'
import { fakeSources, fixtureView, settle } from './zen-sources-fixtures'

async function refusal(p: Promise<unknown>): Promise<ZenSourceError> {
  try {
    await p
  } catch (err) {
    if (err instanceof ZenSourceError) return err
    throw err
  }
  throw new Error('expected a refusal')
}

describe('addresses', () => {
  it('parses name and name::host like the Rust Address', () => {
    expect(parseSourceAddress('fleet')).toEqual({ name: 'fleet', host: null })
    expect(parseSourceAddress('fleet::ACME.k2.dev')).toEqual({ name: 'fleet', host: 'acme.k2.dev' })
    expect(parseSourceAddress('fleet::box.local:8443')).toEqual({ name: 'fleet', host: 'box.local:8443' })
    for (const bad of ['', 'Fleet', 'a_b', 'fleet::', 'fleet::https://x', 'fleet::a..b', 'fleet::x:0', 'x'.repeat(41)]) {
      expect(parseSourceAddress(bad), bad).toBeNull()
    }
  })

  it('maps daemon errors to bridge codes; no daemon answer is unreachable', () => {
    const env = (code: string, extra = {}) => new Error(JSON.stringify({ error: { code, hint: 'h', ...extra } }))
    expect(zenSourceErrorCode(env('removed')).code).toBe('removed')
    expect(zenSourceErrorCode(env('cap_not_granted', { role: 'admin' }))).toMatchObject({ code: 'cap_not_granted', role: 'admin' })
    expect(zenSourceErrorCode(env('role_required')).code).toBe('cap_not_granted')
    expect(zenSourceErrorCode(env('no_history')).code).toBe('no_history')
    expect(zenSourceErrorCode(env('rate_limited')).code).toBe('rate_limited')
    expect(zenSourceErrorCode(env('invalid_token')).code).toBe('unreachable')
    expect(zenSourceErrorCode(env('source_not_found')).code).toBe('failed')
    expect(zenSourceErrorCode(new TypeError('Failed to fetch')).code).toBe('unreachable')
    const removed = new Error('gone')
    removed.name = 'ServerRemovedError'
    expect(zenSourceErrorCode(removed).code).toBe('server_not_saved')
  })
})

describe('the reader', () => {
  it('reads this computer through the local scope and a saved server through its own', async () => {
    const f = fakeSources(['akzm.k2.dev'])
    f.servers.get('local').views.set('fleet', fixtureView())
    f.servers.get('akzm.k2.dev').views.set('deploys', fixtureView({ name: 'deploys', label: 'Deploys', fields: { list: ['a', 'b'] } }))
    const r = createZenSourcesReader(f.deps)
    const local = await r.read('fleet')
    expect(local).toMatchObject({ address: 'fleet', reach: 'ok', serverNow: 1_760_423_520, phase: 'live' })
    const remote = await r.read('deploys::AKZM.k2.dev')
    expect(remote.address).toBe('deploys::akzm.k2.dev')
    expect(remote.fields.list).toBe('["a","b"]')
    expect(f.servers.get('local').reads).toBe(1)
    expect(f.servers.get('akzm.k2.dev').reads).toBe(1)
  })

  it('an unsaved host is server_not_saved, never read', async () => {
    const f = fakeSources([])
    const r = createZenSourcesReader(f.deps)
    const e = await refusal(r.read('fleet::other.k2.dev'))
    expect(e.code).toBe('server_not_saved')
    expect(e.message).toContain('Add other.k2.dev to your servers')
  })

  it('at most 8 source servers per Garden, this computer not counted', async () => {
    const hosts = Array.from({ length: 9 }, (_, i) => `b${i}.k2.dev`)
    const f = fakeSources(hosts)
    for (const h of hosts) f.servers.get(h).views.set('fleet', fixtureView())
    f.servers.get('local').views.set('fleet', fixtureView())
    const r = createZenSourcesReader(f.deps)
    for (const h of hosts.slice(0, 8)) await r.read(`fleet::${h}`, 'g-1')
    await r.read('fleet', 'g-1')
    expect((await refusal(r.read(`fleet::${hosts[8]}`, 'g-1'))).code).toBe('rate_limited')
    await r.read(`fleet::${hosts[8]}`, 'g-2')
    await r.read(`fleet::${hosts[0]}`, 'g-1')
  })

  it('unreachable hands back the last view greyed (phase stale); removed likewise', async () => {
    const f = fakeSources(['akzm.k2.dev'])
    const s = f.servers.get('akzm.k2.dev')
    s.views.set('fleet', fixtureView({ health: 'crit', display: 'crit' }))
    const r = createZenSourcesReader(f.deps)
    await r.read('fleet::akzm.k2.dev')
    s.down = true
    const e = await refusal(r.read('fleet::akzm.k2.dev'))
    expect(e.code).toBe('unreachable')
    expect(e.last).toMatchObject({ reach: 'unreachable', phase: 'stale', display: 'stale', health: 'crit' })
    s.down = false
    s.removed.add('fleet')
    const gone = await refusal(r.read('fleet::akzm.k2.dev'))
    expect(gone.code).toBe('removed')
    expect(gone.last?.reach).toBe('removed')
    // Never read before: no last.
    expect((await refusal(r.read('other::akzm.k2.dev'))).last).toBeUndefined()
  })

  it('the window’s server rides its socket; others poll every 20 s only while visible', async () => {
    const f = fakeSources(['akzm.k2.dev'], 'local')
    const local = f.servers.get('local')
    const remote = f.servers.get('akzm.k2.dev')
    local.views.set('fleet', fixtureView())
    remote.views.set('fleet', fixtureView())
    const r = createZenSourcesReader(f.deps)
    const seenLocal: ZenSourceView[] = []
    const seenRemote: ZenSourceView[] = []
    const offL = r.subscribe('fleet', (v) => seenLocal.push(v), () => undefined, 'g-1')
    const offR = r.subscribe('fleet::akzm.k2.dev', (v) => seenRemote.push(v), () => undefined, 'g-1')
    await settle()
    expect(seenLocal).toHaveLength(1)
    expect(seenRemote).toHaveLength(1)
    expect(f.timers).toBe(1)
    expect(ZEN_SOURCES_POLL_MS).toBe(20_000)

    // A change on this computer arrives on the socket.
    local.views.set('fleet', fixtureView({ health: 'crit', display: 'crit', rev: 11 }))
    local.rev = 11
    f.socket(11)
    await settle()
    expect(seenLocal.at(-1)?.health).toBe('crit')
    expect(local.changes).toEqual([10])

    // The remote server is polled, but not while hidden.
    remote.views.set('fleet', fixtureView({ health: 'warn', display: 'warn', rev: 12 }))
    remote.rev = 12
    f.visible.v = false
    f.tick()
    await settle()
    expect(remote.changes).toEqual([])
    f.visible.v = true
    f.tick()
    await settle()
    expect(remote.changes).toEqual([10])
    expect(seenRemote.at(-1)?.health).toBe('warn')

    // It goes away: the last view comes back greyed, once.
    remote.down = true
    f.tick()
    await settle()
    f.tick()
    await settle()
    expect(seenRemote.filter((v) => v.reach === 'unreachable')).toHaveLength(1)
    expect(seenRemote.at(-1)).toMatchObject({ phase: 'stale', reach: 'unreachable' })

    offR()
    offL()
    expect(f.timers).toBe(0)
  })

  it('a subscription to a source that never answered reports the error once', async () => {
    const f = fakeSources(['akzm.k2.dev'])
    f.servers.get('akzm.k2.dev').error = { code: 'cap_not_granted', role: 'admin' }
    const r = createZenSourcesReader(f.deps)
    const errors: ZenSourceError[] = []
    const off = r.subscribe('money::akzm.k2.dev', () => undefined, (e) => errors.push(e))
    await settle()
    expect(errors.map((e) => [e.code, e.role])).toEqual([['cap_not_granted', 'admin']])
    off()
  })

  it('list gives each bound address a reach', async () => {
    const f = fakeSources(['akzm.k2.dev'])
    f.servers.get('local').views.set('fleet', fixtureView())
    const r = createZenSourcesReader(f.deps)
    const rows = await r.list(['fleet', 'deploys::nope.k2.dev', 'money::akzm.k2.dev'])
    expect(rows).toEqual([
      { address: 'fleet', label: 'Fleet boxes', health: 'ok', phase: 'live', display: 'ok', reach: 'ok' },
      { address: 'deploys::nope.k2.dev', label: null, health: null, phase: null, display: null, reach: 'server_not_saved' },
      { address: 'money::akzm.k2.dev', label: null, health: null, phase: null, display: null, reach: 'unreachable' },
    ])
  })

  it('history and refresh pass through; no_history and rate_limited are refusals', async () => {
    const f = fakeSources()
    f.servers.get('local').views.set('fleet', fixtureView())
    f.servers.get('local').views.set('money', fixtureView({ name: 'money', keep: 'states', fields: {} }))
    const r = createZenSourcesReader(f.deps)
    expect((await r.history('fleet', 'count')).points).toEqual([{ t: 1, v: 2 }, { t: 2, v: null }])
    expect((await refusal(r.history('money', 'pastDue'))).code).toBe('no_history')
    expect(await r.refresh('fleet')).toEqual({ queued: true, nextAt: 1_760_423_521 })
    expect((await refusal(r.refresh('fleet'))).code).toBe('rate_limited')
  })
})
