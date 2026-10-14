// Garden v3 (contract-sources-v1 §12.3 "Ask") — the draft K2 writes: the
// address, label, state, reason sentence, failing keys and
// `k2 sources show <name>`; never a field value; another server named.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { zenSourceView } from './zen-sources'
import { fixtureItem, fixtureView } from './zen-sources-fixtures'
import {
  openZenSourceAsk,
  rememberZenSourceAskAgent,
  useZenSourceAskStore,
  zenSourceAskDraft,
  zenSourceAskLastAgent,
  zenSourceMakerProject,
} from './zen-source-ask'

const RED = fixtureView({
  health: 'crit',
  display: 'crit',
  reasonText: 'Some boxes are down.',
  fields: { newest: 'v9.9.9-secretish', total: 4242 },
  items: [
    fixtureItem('alpha'),
    fixtureItem('ohana', { health: 'crit', reason: 'timeout', fields: { version: '0.0.1-value' } }),
    fixtureItem('luzz', { health: 'unknown', reason: null }),
  ],
})

describe('zenSourceAskDraft', () => {
  it('names the source, state, reason, failing keys and the CLI; no values', () => {
    const d = zenSourceAskDraft({
      gardenId: 'g-1',
      gardenName: 'Fleet',
      address: 'fleet::akzm.k2.dev',
      itemKey: null,
      view: zenSourceView('fleet::akzm.k2.dev', RED),
      madeBy: null,
    })
    expect(d).toContain('"Fleet boxes" (fleet::akzm.k2.dev)')
    expect(d).toContain('red (crit)')
    expect(d).toContain('Some boxes are down.')
    expect(d).toContain('ohana (crit, timeout)')
    expect(d).toContain('luzz (unknown)')
    expect(d).not.toContain('alpha')
    expect(d).toContain('This check runs on akzm.k2.dev.')
    expect(d).toContain('k2 sources show fleet')
    for (const value of ['v9.9.9-secretish', '4242', '0.0.1-value', '0.45.6']) expect(d).not.toContain(value)
  })

  it('a tile ask names the item; a local source says nothing about hosts', () => {
    const d = zenSourceAskDraft({ gardenId: 'g', gardenName: 'G', address: 'fleet', itemKey: 'ohana', view: zenSourceView('fleet', RED), madeBy: null })
    expect(d).toContain("I'm asking about ohana (crit, timeout).")
    expect(d).not.toContain('This check runs on')
  })

  it('unreachable and removed say so; no view still drafts', () => {
    const v = { ...zenSourceView('fleet::akzm.k2.dev', RED), reach: 'unreachable' as const, display: 'stale' as const, phase: 'stale' as const }
    expect(zenSourceAskDraft({ gardenId: 'g', gardenName: 'G', address: v.address, itemKey: null, view: v, madeBy: null })).toContain(
      'akzm.k2.dev isn’t answering',
    )
    const d = zenSourceAskDraft({ gardenId: 'g', gardenName: 'G', address: 'money', itemKey: null, view: null, madeBy: null })
    expect(d).toContain('(money)')
    expect(d).toContain('k2 sources show money')
  })
})

describe('which agent', () => {
  beforeEach(() => {
    const m = new Map<string, string>()
    vi.stubGlobal('localStorage', {
      getItem: (k: string) => m.get(k) ?? null,
      setItem: (k: string, v: string) => void m.set(k, v),
      removeItem: (k: string) => void m.delete(k),
    })
  })
  afterEach(() => {
    vi.unstubAllGlobals()
    useZenSourceAskStore.getState().close()
  })

  it('the maker is agent:<projectId>; people makers are not agents', () => {
    expect(zenSourceMakerProject('agent:7c1e')).toBe('7c1e')
    expect(zenSourceMakerProject('login:rosson')).toBeNull()
    expect(zenSourceMakerProject(null)).toBeNull()
  })

  it('remembers the last chosen agent per Garden (best effort)', () => {
    rememberZenSourceAskAgent('g-1', 'nora::local')
    expect(zenSourceAskLastAgent('g-1')).toBe('nora::local')
    expect(zenSourceAskLastAgent('g-2')).toBeNull()
    vi.stubGlobal('localStorage', {
      getItem: () => {
        throw new Error('blocked')
      },
    })
    expect(zenSourceAskLastAgent('g-1'), 'blocked storage reads as nothing remembered').toBeNull()
  })

  it('openZenSourceAsk puts one request in the store', () => {
    expect(openZenSourceAsk({ gardenId: 'g', gardenName: 'G', address: 'fleet', itemKey: null, view: null, madeBy: null })).toBe(true)
    expect(useZenSourceAskStore.getState().request?.address).toBe('fleet')
  })
})
