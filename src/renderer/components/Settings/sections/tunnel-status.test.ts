// Settings → Tunnel status card: every state says whether this server can be
// reached, where, and offers the one next step.
import { describe, it, expect } from 'vitest'
import { summarizeTunnel, type TunnelFacts } from './tunnel-status'

const base: TunnelFacts = {
  loaded: true,
  readError: null,
  running: false,
  enabled: true,
  released: false,
  frpcInstalled: true,
  tokenSet: true,
  subdomain: 'alex',
  signedIn: true,
  hasAddresses: true,
  starting: false,
  startError: null,
  listenerName: null,
}

describe('summarizeTunnel', () => {
  it('loading, then a failed read with Try again and the reason', () => {
    expect(summarizeTunnel({ ...base, loaded: false }).state).toBe('loading')
    const failed = summarizeTunnel({ ...base, loaded: false, readError: 'database is locked' })
    expect(failed).toMatchObject({ state: 'read-failed', tone: 'error', action: 'try-again', detail: 'database is locked' })
  })

  it('reachable offers Turn off', () => {
    const s = summarizeTunnel({ ...base, running: true })
    expect(s).toMatchObject({ state: 'reachable', tone: 'ok', word: 'Reachable', action: 'turn-off' })
  })

  it('a running tunnel whose listener holds another name needs a repair', () => {
    const s = summarizeTunnel({ ...base, running: true, listenerName: 'old' })
    expect(s).toMatchObject({ state: 'repair', tone: 'warn', action: 'repair' })
    expect(s.hint).toContain('old.k2.dev')
  })

  it('connecting while a Turn on is in flight', () => {
    expect(summarizeTunnel({ ...base, starting: true })).toMatchObject({ state: 'connecting', action: null })
  })

  it('a failed Turn on says what failed, the reason, and Try again', () => {
    const s = summarizeTunnel({ ...base, startError: 'relay refused the connection' })
    expect(s).toMatchObject({ state: 'failed', tone: 'error', action: 'try-again', detail: 'relay refused the connection' })
    expect(s.hint).toContain('alex.k2.dev')
  })

  it('a missing helper program says how to fix it', () => {
    const s = summarizeTunnel({ ...base, frpcInstalled: false })
    expect(s).toMatchObject({ state: 'missing-helper', tone: 'error', action: 'check-again' })
    expect(s.hint).toContain('frpc')
  })

  it('not set up points at the next step for each account state', () => {
    const fresh = { ...base, tokenSet: false, subdomain: '' }
    expect(summarizeTunnel({ ...fresh, signedIn: false }).hint).toContain('Sign in')
    expect(summarizeTunnel(fresh).hint).toContain('Choose')
    expect(summarizeTunnel({ ...fresh, hasAddresses: false }).hint).toContain('no addresses')
    expect(summarizeTunnel(fresh)).toMatchObject({ state: 'not-set-up', action: null })
  })

  it('off by choice and ready both offer Turn on', () => {
    expect(summarizeTunnel({ ...base, enabled: false })).toMatchObject({ state: 'paused', action: 'turn-on' })
    expect(summarizeTunnel(base)).toMatchObject({ state: 'ready', action: 'turn-on' })
  })

  it('released asks for a new address', () => {
    expect(summarizeTunnel({ ...base, tokenSet: false, released: true })).toMatchObject({ state: 'released', tone: 'warn', action: null })
  })
})
