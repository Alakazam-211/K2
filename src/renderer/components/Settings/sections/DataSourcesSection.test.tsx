// @vitest-environment jsdom
// Settings → Gardens → External Data (contract-sources-v1 §8; B2).

import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup, within } from '@testing-library/react'

const daemonCliGet = vi.hoisted(() => vi.fn())
const daemonCliPost = vi.hoisted(() => vi.fn())
const scopeState = vi.hoisted(() => ({ isRemote: true, username: 'ada' as string | undefined }))
const sourcesChanged = vi.hoisted(() => ({ fn: null as null | ((rev: number) => void) }))

vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet, daemonCliPost }))
vi.mock('@/kessel/server-scope', () => {
  const scope = {
    id: 'primary',
    isPrimary: true,
    get hostKey() {
      return 'akzm.k2.dev'
    },
    get label() {
      return 'akzm.k2.dev'
    },
    get isRemote() {
      return scopeState.isRemote
    },
    connectHost: () => (scopeState.isRemote ? { username: scopeState.username } : null),
  }
  return { primaryScope: () => scope, scopeForHost: () => scope }
})
vi.mock('@/stores/session-events', () => ({
  onSourcesChanged: (_s: unknown, fn: (rev: number) => void) => {
    sourcesChanged.fn = fn
    return () => {
      sourcesChanged.fn = null
    }
  },
}))
vi.mock('@/lib/desktop-chrome', () => ({ isMacPlatform: () => true }))
vi.mock('@/stores/settings', () => ({ useSettingsStore: { getState: () => ({ setSection: () => {} }) } }))
vi.mock('@/stores/projects', () => ({
  useProjectsStore: (sel: (s: unknown) => unknown) =>
    sel({ projects: [{ id: 'p-web', name: 'k2-dev-web', handle: 'k2-dev-web', path: '/w/web' }] }),
}))

import { DataSourcesSection, pollerLine } from './DataSourcesSection'
import { stateText } from './DataSourceRow'
import { secretFitsOrigin } from './DataSourceReview'
import { ago, batchHash, countsLine, maskQuery, sourcesErrorOf, sourcesErrorText } from './data-sources-ui'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import type { SourceListRow, SourceResult } from '@/lib/sources/sources-types'

function view(over: Partial<SourceResult> = {}): SourceResult {
  return {
    name: 'fleet',
    label: 'Fleet boxes',
    kind: 'http-json',
    status: 'active',
    health: 'ok',
    phase: 'live',
    display: 'ok',
    reason: null,
    reasonText: null,
    httpStatus: null,
    soft: { count: 0, of: 3 },
    serverNow: 1_760_000_100,
    updatedAt: 1_760_000_060,
    checkedAt: 1_760_000_060,
    nextAt: 1_760_000_120,
    hardSince: null,
    recoveredAt: null,
    gap: null,
    muted: null,
    simulated: null,
    pinned: false,
    keep: 'values',
    masked: false,
    link: null,
    fields: {},
    items: [],
    counts: { ok: 18, warn: 0, crit: 0, unknown: 2 },
    rev: 9,
    ...over,
  }
}

function row(over: Partial<SourceListRow> = {}, v: Partial<SourceResult> = {}): SourceListRow {
  return {
    name: 'fleet',
    label: 'Fleet boxes',
    status: 'active',
    madeBy: 'agent:p-web',
    secret: null,
    pinned: true,
    keep: 'values',
    viewers: 'member',
    agentAccess: 'values',
    everySecs: 60,
    targets: 20,
    triggers: 1,
    view: view(v),
    ...over,
  }
}

const PENDING = {
  id: 'src-deploys',
  name: 'deploys',
  label: 'Vercel deploys',
  madeBy: 'agent:p-web',
  maker: { kind: 'agent', projectId: 'p-web', handle: 'k2-dev-web' },
  origins: ['https://api.vercel.com:443'],
  everySecs: 180,
  targets: 1,
  secret: null as string | null,
  urlSecret: null,
  lan: false,
  specHash: 'h'.repeat(64),
  approval: 'one_click',
}

let rows: SourceListRow[] = []
let pending = { sources: [] as unknown[], triggers: [] as unknown[] }
let posts: Array<{ route: string; body: Record<string, unknown> }> = []
let postHandler: (route: string, body: Record<string, unknown>) => unknown = () => ({})

beforeEach(() => {
  cleanup()
  posts = []
  scopeState.isRemote = true
  scopeState.username = 'ada'
  rows = [row()]
  pending = { sources: [], triggers: [] }
  postHandler = () => ({})
  daemonCliGet.mockReset()
  daemonCliPost.mockReset()
  daemonCliGet.mockImplementation(async (_scope: unknown, route: string, params?: Record<string, unknown>) => {
    switch (route) {
      case 'sources/list':
        return { sources: rows, serverNow: 1_760_000_100, rev: 9 }
      case 'sources/status':
        return { sources: rows.length, active: 1, targets: 20, inFlight: 0, behindSecs: 0, parkedOrigins: [], lastGap: null, airgap: false }
      case 'sources/pending':
        return pending
      case 'sources/show':
        throw new Error('{"error":{"code":"not_built","hint":"x"}}')
      case 'sources/triggers/list':
        return {
          triggers: [
            {
              id: 't-1',
              source: params?.source ?? 'fleet',
              on: { kind: 'item_went_crit' },
              agent: { projectId: 'p-web', handle: 'k2-dev-web', missing: false },
              note: null,
              cooldownSecs: 1800,
              dailyCap: 12,
              status: 'proposed',
              madeBy: 'agent:p-web',
              lastFireAt: null,
              firesToday: 0,
            },
          ],
        }
      case 'sources/triggers/log':
        return { fires: [{ id: 'f1', at: Math.floor(Date.now() / 1000) - 120, incidentId: 'i', itemKey: 'ohana', outcome: 'suppressed', suppressReason: 'muted', deliveredAt: null, woke: null, noteBytes: null, detail: null }] }
      case 'vault/list':
        return { secrets: [] }
      default:
        throw new Error(`unexpected GET ${route}`)
    }
  })
  daemonCliPost.mockImplementation(async (_scope: unknown, route: string, body: Record<string, unknown>) => {
    posts.push({ route, body })
    return postHandler(route, body)
  })
  useConfirmDialogStore.setState({ confirm: vi.fn(async () => true) } as never)
})

describe('helpers', () => {
  it('formats state, counts, ages and masks queries', () => {
    expect(countsLine({ ok: 18, warn: 0, crit: 1, unknown: 2 })).toBe('1 red, 18 ok, 2 unknown')
    expect(ago(40)).toBe('40 s ago')
    expect(ago(3700)).toBe('1 h ago')
    expect(maskQuery('https://api.vercel.com/v6/deployments?teamId=team_x&limit=5')).toBe(
      'https://api.vercel.com/v6/deployments?teamId=•••&limit=•••',
    )
    expect(stateText(row({}, { display: 'ok', soft: { count: 2, of: 3 } }))).toBe('ok · retrying 2/3')
    expect(stateText(row({}, { display: 'warn', soft: { count: 2, of: 3, level: 'crit' } }))).toBe('warn · soft crit 2/3')
    expect(stateText(row({}, { display: 'stale', reasonText: 'The site did not answer in time.' }))).toBe(
      'stale · The site did not answer in time.',
    )
    expect(pollerLine({ sources: 23, active: 21, targets: 61, inFlight: 0, behindSecs: 0, parkedOrigins: [], lastGap: null, airgap: false })).toBe(
      'Poller: 23 sources · 61 targets · on time',
    )
    expect(sourcesErrorOf(new Error('409: {"error":{"code":"root_key","hint":"h","rootKey":{"code":"stripe_secret"}}}'))?.rootKey).toEqual({
      code: 'stripe_secret',
    })
    expect(secretFitsOrigin({ hosts: ['https://api.stripe.com:443'], status: 'active' }, 'https://api.stripe.com:443')).toBe(true)
    expect(secretFitsOrigin({ hosts: ['https://api.stripe.com:443'], status: 'active' }, 'https://evil.example:443')).toBe(false)
  })

  it('reads the one confirm code, and the 0.45.x spelling for one release', () => {
    const want = 'This needs your confirm (Touch ID, your login or the vault password). Try again and confirm when asked.'
    expect(sourcesErrorText(new Error('403: {"error":{"code":"vault_confirm_required","hint":"h"}}'))).toBe(want)
    expect(sourcesErrorText(new Error('403: {"error":{"code":"confirm_required","hint":"h"}}'))).toBe(want)
  })

  it('hashes a batch the way the daemon does', async () => {
    // sha256("a:X\nb:Y"), sorted lines.
    const h = await batchHash([
      { id: 'b', secret: 'Y' },
      { id: 'a', secret: 'X' },
    ])
    // The same vector is pinned in approve_routes.rs (batch_hash).
    expect(h).toBe('6976188b7ee15f6eef1614e1c09a81e74d9915020a7b0801b2b276e368652074')
    const again = await batchHash([
      { id: 'a', secret: 'X' },
      { id: 'b', secret: 'Y' },
    ])
    expect(again).toBe(h)
  })
})

describe('DataSourcesSection', () => {
  it('lists sources with state, counts, age by the server clock, triggers and badges', async () => {
    render(<DataSourcesSection />)
    const r = await screen.findByTestId('sources-row')
    expect(r.textContent).toContain('Fleet boxes')
    expect(r.textContent).toContain('ok · 18 ok, 2 unknown · 40 s ago · 1 trigger')
    expect(r.textContent).toContain('pinned')
    expect(screen.getByTestId('sources-runs-on').textContent).toBe('Runs on akzm.k2.dev · always on')
    expect(screen.queryByTestId('sources-mac-hint')).toBeNull()
    expect(await screen.findByTestId('sources-poller')).toHaveProperty('textContent', 'Poller: 1 source · 20 targets · on time')
  })

  it('says checks pause while this Mac sleeps, and warns about a legacy token', async () => {
    scopeState.isRemote = false
    const { unmount } = render(<DataSourcesSection />)
    expect(await screen.findByTestId('sources-mac-hint')).toBeTruthy()
    unmount()
    scopeState.isRemote = true
    scopeState.username = undefined
    render(<DataSourcesSection />)
    expect(await screen.findByTestId('sources-legacy-token')).toBeTruthy()
  })

  it('re-reads on sources_changed', async () => {
    render(<DataSourcesSection />)
    await screen.findByTestId('sources-row')
    const before = daemonCliGet.mock.calls.filter((c) => c[1] === 'sources/list').length
    rows = [row({ label: 'Fleet (renamed)' })]
    sourcesChanged.fn?.(10)
    await waitFor(() => expect(screen.getByTestId('sources-row').textContent).toContain('Fleet (renamed)'))
    expect(daemonCliGet.mock.calls.filter((c) => c[1] === 'sources/list').length).toBeGreaterThan(before)
  })

  it('approves a secretless proposal in one click with the hash it showed', async () => {
    rows = [row({ name: 'deploys', label: 'Vercel deploys', status: 'proposed', pinned: false }, { status: 'proposed', display: 'pending' })]
    pending = { sources: [PENDING], triggers: [] }
    postHandler = (route) => (route === 'sources/approve' ? { name: 'deploys', status: 'active', secretBound: false, nextAt: null } : {})
    render(<DataSourcesSection />)
    expect((await screen.findByTestId('sources-pending-banner')).textContent).toContain('1 waiting for approval')
    fireEvent.click(await screen.findByTestId('sources-row-review'))
    const rev = await screen.findByTestId('sources-review')
    expect(rev.textContent).toContain('Proposed by k2-dev-web · every 3 min · 1 target · https://api.vercel.com:443')
    fireEvent.click(within(rev).getByTestId('sources-review-approve'))
    await waitFor(() => expect(posts.find((p) => p.route === 'sources/approve')).toBeTruthy())
    const body = posts.find((p) => p.route === 'sources/approve')!.body
    expect(body).toMatchObject({ name: 'deploys', specHash: 'h'.repeat(64), viewers: 'member', agentAccess: 'values', keep: 'values' })
    expect(body.confirmToken).toBeUndefined()
    expect(posts.some((p) => p.route.startsWith('vault/confirm'))).toBe(false)
  })

  it('a secret goes through the vault confirm bound to the source id, then root-key Use anyway', async () => {
    rows = [row({ name: 'deploys', status: 'proposed' }, { status: 'proposed' })]
    pending = { sources: [{ ...PENDING, secret: 'STRIPE_SK', approval: 'needs_confirm' }], triggers: [] }
    let approves = 0
    postHandler = (route, body) => {
      if (route === 'vault/confirm/start') return { methods: ['connect-password'], expiresIn: 60 }
      if (route === 'vault/confirm/finish') return { token: `tok-${String(body.secretId)}`, expiresIn: 60 }
      if (route === 'sources/approve') {
        approves += 1
        if (approves === 1) throw new Error('409: {"error":{"code":"root_key","hint":"Make a restricted key","rootKey":{"code":"stripe_secret","hint":"Make a restricted key"}}}')
        return { name: 'deploys', status: 'active', secretBound: true, nextAt: null }
      }
      return {}
    }
    render(<DataSourcesSection />)
    fireEvent.click(await screen.findByTestId('sources-row-review'))
    const rev = await screen.findByTestId('sources-review')
    expect((within(rev).getByTestId('sources-review-secret') as HTMLSelectElement).value).toBe('STRIPE_SK')
    // Secret-bound defaults: admins only, agents see states.
    expect(within(rev).getByTestId('sources-review-viewers').querySelectorAll('input')[1]).toHaveProperty('checked', true)
    fireEvent.click(within(rev).getByTestId('sources-review-approve'))
    const pw = await screen.findByTestId('sources-confirm-password')
    fireEvent.change(pw, { target: { value: 'hunter2' } })
    fireEvent.submit(pw.closest('form')!)
    await screen.findByTestId('sources-review-rootkey')
    const finish = posts.find((p) => p.route === 'vault/confirm/finish')!.body
    expect(finish).toMatchObject({ action: 'source-secret', secretId: 'src-deploys', method: 'connect-password', password: 'hunter2' })
    fireEvent.click(screen.getByTestId('sources-review-use-anyway'))
    await waitFor(() => expect(approves).toBe(2))
    const last = posts.filter((p) => p.route === 'sources/approve')[1].body
    expect(last).toMatchObject({ secret: 'STRIPE_SK', confirmToken: 'tok-src-deploys', useAnyway: true })
  })

  it('row menu: pin, mute 1 h, remove with the held-name warning', async () => {
    rows = [row({ pinned: false })]
    render(<DataSourcesSection />)
    const open = async (): Promise<void> => {
      fireEvent.click(await screen.findByTestId('sources-row-menu'))
    }
    await open()
    fireEvent.click(screen.getByTestId('sources-menu-pin'))
    await waitFor(() => expect(posts.at(-1)).toEqual({ route: 'sources/pin', body: { name: 'fleet' } }))
    await open()
    fireEvent.click(screen.getByTestId('sources-menu-mute'))
    await waitFor(() => expect(posts.at(-1)?.route).toBe('sources/mute'))
    expect(posts.at(-1)?.body).toMatchObject({ name: 'fleet', forSecs: 3600 })
    await open()
    fireEvent.click(screen.getByTestId('sources-menu-remove'))
    await waitFor(() => expect(posts.at(-1)?.route).toBe('sources/remove'))
    const confirmArgs = (useConfirmDialogStore.getState().confirm as ReturnType<typeof vi.fn>).mock.calls[0][0]
    expect(confirmArgs.message).toContain('The name is held for 7 days')
  })

  it('shows not-built routes plainly instead of crashing', async () => {
    daemonCliGet.mockImplementation(async () => {
      throw new Error('501: {"error":{"code":"not_built","hint":"x"}}')
    })
    render(<DataSourcesSection />)
    expect((await screen.findByTestId('sources-error')).textContent).toBe('This part of External Data is not on this server yet.')
  })
})

describe('triggers and simulate', () => {
  it('approves a proposed trigger, shows fires, and adds one for an agent', async () => {
    render(<DataSourcesSection />)
    fireEvent.click(await screen.findByTestId('sources-row-menu'))
    fireEvent.click(screen.getByTestId('sources-menu-triggers'))
    const t = await screen.findByTestId('sources-trigger-row')
    expect(t.textContent).toContain('When it an item goes red, wake k2-dev-web')
    expect(t.textContent).toContain('proposed by k2-dev-web')
    expect((await screen.findByTestId('sources-trigger-fires')).textContent).toContain('held (muted)')
    fireEvent.click(within(t).getByTestId('sources-trigger-approve'))
    await waitFor(() => expect(posts.at(-1)).toEqual({ route: 'sources/triggers/approve', body: { id: 't-1' } }))
    fireEvent.click(screen.getByTestId('sources-trigger-add'))
    const form = await screen.findByTestId('sources-trigger-form')
    fireEvent.change(within(form).getByTestId('sources-trigger-on'), { target: { value: 'stale_for' } })
    fireEvent.change(within(form).getByTestId('sources-trigger-agent'), { target: { value: 'p-web' } })
    fireEvent.change(within(form).getByTestId('sources-trigger-note'), { target: { value: 'check the relay' } })
    fireEvent.click(within(form).getByTestId('sources-trigger-save'))
    await waitFor(() => expect(posts.at(-1)?.route).toBe('sources/triggers/add'))
    expect(posts.at(-1)?.body).toEqual({
      source: 'fleet',
      on: { kind: 'stale_for', secs: 1800 },
      agent: 'p-web',
      note: 'check the relay',
      cooldownSecs: 1800,
      dailyCap: 12,
    })
  })

  it('sends a test wake and says how it went', async () => {
    postHandler = (route) => (route === 'sources/triggers/test' ? { outcome: 'delivered', woke: true } : {})
    render(<DataSourcesSection />)
    fireEvent.click(await screen.findByTestId('sources-row-menu'))
    fireEvent.click(screen.getByTestId('sources-menu-triggers'))
    fireEvent.click(await screen.findByTestId('sources-trigger-test'))
    expect(await screen.findByText('Test sent (woke the agent).')).toBeTruthy()
  })

  it('simulates a state for a chosen time, and turns it off', async () => {
    rows = [row({}, { simulated: { state: 'crit', until: 1_760_003_600, by: 'login:ada' } })]
    render(<DataSourcesSection />)
    expect((await screen.findByTestId('sources-row')).textContent).toContain('TEST crit')
    fireEvent.click(screen.getByTestId('sources-row-menu'))
    fireEvent.click(screen.getByTestId('sources-menu-simulate'))
    const d = await screen.findByTestId('sources-simulate')
    fireEvent.click(within(d).getByLabelText('Amber'))
    fireEvent.change(within(d).getByTestId('sources-simulate-for'), { target: { value: '600' } })
    fireEvent.click(within(d).getByTestId('sources-simulate-start'))
    await waitFor(() => expect(posts.at(-1)).toEqual({ route: 'sources/simulate', body: { name: 'fleet', state: 'warn', forSecs: 600 } }))
    fireEvent.click(screen.getByTestId('sources-row-menu'))
    fireEvent.click(screen.getByTestId('sources-menu-simulate'))
    fireEvent.click(await screen.findByTestId('sources-simulate-off'))
    await waitFor(() => expect(posts.at(-1)?.body).toMatchObject({ name: 'fleet', state: 'off' }))
  })
})

describe('batch review', () => {
  it('approves secretless items with no confirm and secret items with one batch confirm', async () => {
    const { DataSourcesBatchDialog } = await import('./DataSourcesBatchDialog')
    pending = {
      sources: [
        { ...PENDING, name: 'fleet', id: 'src-fleet', label: 'Fleet' },
        { ...PENDING, name: 'money', id: 'src-money', label: 'Money', secret: 'STRIPE_RK', approval: 'needs_confirm' },
        { ...PENDING, name: 'nas', id: 'src-nas', label: 'NAS', lan: true, approval: 'owner_only' },
      ],
      triggers: [{ id: 't-9', source: 'fleet', on: { kind: 'item_went_crit' }, agent: { projectId: 'p-web', handle: 'k2-dev-web' }, madeBy: 'agent:p-web' }],
    }
    postHandler = (route, body) => {
      if (route === 'vault/confirm/start') return { methods: ['vault-password'], expiresIn: 60 }
      if (route === 'vault/confirm/finish') return { token: `tok-${String(body.secretId)}`, expiresIn: 60 }
      if (route === 'sources/batch-approve') return { approved: ['fleet', 'money'], triggersApproved: ['t-9'] }
      return {}
    }
    const { primaryScope } = await import('@/kessel/server-scope')
    render(<DataSourcesBatchDialog args={{ gardenName: 'Fleet', byServer: { 'akzm.k2.dev': ['fleet', 'money', 'nas'] } }} onClose={() => {}} scopeOf={() => primaryScope()} />)
    expect(screen.getByText('Review pending for Garden “Fleet”')).toBeTruthy()
    const boxes = await screen.findAllByTestId('sources-batch-item')
    expect(boxes).toHaveLength(4)
    expect((boxes[2] as HTMLInputElement).disabled).toBe(true)
    fireEvent.click(screen.getByTestId('sources-batch-approve'))
    const pw = await screen.findByTestId('sources-confirm-password')
    fireEvent.change(pw, { target: { value: 'pw' } })
    fireEvent.submit(pw.closest('form')!)
    await waitFor(() => expect(posts.find((p) => p.route === 'sources/batch-approve')).toBeTruthy())
    const want = await batchHash([{ id: 'src-money', secret: 'STRIPE_RK' }])
    expect(posts.find((p) => p.route === 'vault/confirm/finish')!.body).toMatchObject({ action: 'source-batch', secretId: want })
    expect(posts.find((p) => p.route === 'sources/batch-approve')!.body).toEqual({
      items: [
        { name: 'fleet', specHash: 'h'.repeat(64) },
        { name: 'money', specHash: 'h'.repeat(64), secret: 'STRIPE_RK' },
      ],
      triggers: ['t-9'],
      confirmToken: `tok-${want}`,
    })
    expect(await screen.findByText('Approved 3 on akzm.k2.dev.')).toBeTruthy()
  })
})
