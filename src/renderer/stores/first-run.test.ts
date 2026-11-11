// @vitest-environment jsdom
//
// Setup checklist store: reads `first-run/status`, re-reads on every event
// the contract lists, takes the body hide/show/skip return, and drops row
// ids and `opens` values it doesn't know.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
  handlers: new Map<string, Array<() => void>>(),
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
  }
})

function sub(name: string) {
  return (_scope: unknown, fn: () => void) => {
    const list = h.handlers.get(name) ?? []
    list.push(fn)
    h.handlers.set(name, list)
    return () => {
      list.splice(list.indexOf(fn), 1)
    }
  }
}

vi.mock('@/stores/session-events', () => ({
  onFirstRunChanged: sub('first_run_changed'),
  onLlmAccountsChanged: sub('llm_accounts_changed'),
  onProjectsChanged: sub('projects_changed'),
  onTunnelStatusChanged: sub('tunnel_status_changed'),
  onTunnelIdentityChanged: sub('tunnel_identity_changed'),
  onPairCodeChanged: sub('pair_code_changed'),
  onAppHello: sub('hello'),
}))

import { primaryScope } from '@/kessel/server-scope'
import {
  parseFirstRunDoc,
  resetFirstRunForTests,
  rowsLeft,
  useFirstRunStore,
  visibleRows,
  watchFirstRun,
} from './first-run'

const BODY = {
  enabled: true,
  hidden: false,
  finished: false,
  rows: [
    { id: 'llm_sign_in', state: 'todo', opens: 'llm_sign_in' },
    { id: 'first_agent', state: 'done', opens: 'add_workspace' },
    { id: 'tunnel', state: 'todo', opens: 'tunnel' },
    { id: 'phone', state: 'todo', opens: 'tunnel', note: 'needs_tunnel' },
    { id: 'own_sign_in', state: 'todo', opens: 'owner_account_prompt' },
  ],
}

function emit(name: string): void {
  for (const fn of [...(h.handlers.get(name) ?? [])]) fn()
}

function entry() {
  return useFirstRunStore.getState().entries[primaryScope().id]
}

describe('useFirstRunStore', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    h.daemonCliGet.mockReset()
    h.daemonCliPost.mockReset()
    h.daemonCliGet.mockResolvedValue(BODY)
    h.handlers.clear()
    resetFirstRunForTests()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('reads first-run/status on watch', async () => {
    const stop = watchFirstRun(primaryScope())
    await vi.advanceTimersByTimeAsync(0)
    expect(h.daemonCliGet).toHaveBeenCalledTimes(1)
    expect(h.daemonCliGet).toHaveBeenCalledWith('first-run/status')
    expect(entry()?.doc?.rows.map((r) => r.id)).toEqual(['llm_sign_in', 'first_agent', 'tunnel', 'phone', 'own_sign_in'])
    expect(entry()?.doc?.rows[3]).toEqual({ id: 'phone', state: 'todo', opens: 'tunnel', note: 'needs_tunnel' })
    stop()
  })

  it.each([
    'first_run_changed',
    'llm_accounts_changed',
    'projects_changed',
    'tunnel_status_changed',
    'tunnel_identity_changed',
    'pair_code_changed',
  ])('re-reads on %s', async (kind) => {
    const stop = watchFirstRun(primaryScope())
    await vi.advanceTimersByTimeAsync(0)
    expect(h.daemonCliGet).toHaveBeenCalledTimes(1)
    emit(kind)
    await vi.advanceTimersByTimeAsync(0)
    expect(h.daemonCliGet).toHaveBeenCalledTimes(2)
    stop()
    emit(kind)
    await vi.advanceTimersByTimeAsync(0)
    expect(h.daemonCliGet).toHaveBeenCalledTimes(2)
  })

  it('reads once more when an event lands during a read', async () => {
    let release: (v: unknown) => void = () => {}
    h.daemonCliGet.mockImplementationOnce(() => new Promise((r) => (release = r)))
    const stop = watchFirstRun(primaryScope())
    emit('projects_changed')
    emit('projects_changed')
    expect(h.daemonCliGet).toHaveBeenCalledTimes(1)
    release(BODY)
    await vi.advanceTimersByTimeAsync(0)
    expect(h.daemonCliGet).toHaveBeenCalledTimes(2)
    stop()
  })

  it('hide POSTs and takes the returned body', async () => {
    h.daemonCliPost.mockResolvedValue({ ...BODY, hidden: true })
    await useFirstRunStore.getState().hide()
    expect(h.daemonCliPost).toHaveBeenCalledWith('first-run/hide', {})
    expect(entry()?.doc?.hidden).toBe(true)
    expect(visibleRows(entry()?.doc)).toBeNull()
  })

  it('show POSTs and takes the returned body', async () => {
    h.daemonCliPost.mockResolvedValue(BODY)
    await useFirstRunStore.getState().show()
    expect(h.daemonCliPost).toHaveBeenCalledWith('first-run/show', {})
    expect(visibleRows(entry()?.doc)?.length).toBe(5)
  })

  it('skip POSTs the row and takes the returned body', async () => {
    h.daemonCliPost.mockResolvedValue({
      ...BODY,
      rows: BODY.rows.map((r) => (r.id === 'tunnel' ? { ...r, state: 'skipped' } : r)),
    })
    await useFirstRunStore.getState().skip('tunnel')
    expect(h.daemonCliPost).toHaveBeenCalledWith('first-run/skip', { row: 'tunnel' })
    expect(entry()?.doc?.rows.find((r) => r.id === 'tunnel')?.state).toBe('skipped')
  })

  it('a failed gesture keeps the list and says what failed', async () => {
    const stop = watchFirstRun(primaryScope())
    await vi.advanceTimersByTimeAsync(0)
    h.daemonCliPost.mockRejectedValue(new Error('role_required'))
    await useFirstRunStore.getState().hide()
    expect(entry()?.doc?.hidden).toBe(false)
    expect(entry()?.error).toBe('role_required')
    stop()
  })

  it('a read that lands after a server switch lands nowhere', async () => {
    let release: (v: unknown) => void = () => {}
    h.daemonCliGet.mockImplementationOnce(() => new Promise((r) => (release = r)))
    const p = useFirstRunStore.getState().load()
    resetFirstRunForTests()
    release(BODY)
    await p
    expect(entry()).toBeUndefined()
  })
})

describe('parseFirstRunDoc', () => {
  it('drops unknown row ids and unknown opens values', () => {
    const doc = parseFirstRunDoc({
      enabled: true,
      rows: [
        { id: 'meet_k5', state: 'todo', opens: 'k5_chat' },
        { id: 'llm_sign_in', state: 'todo', opens: 'something_new' },
        { id: 'k2_cli', state: 'todo', opens: 'general' },
        { id: 'tunnel', state: 'blocked', opens: 'tunnel', note: 'other' },
      ],
    })
    expect(doc.rows).toEqual([
      { id: 'llm_sign_in', state: 'todo', opens: null, note: null },
      { id: 'k2_cli', state: 'todo', opens: null, note: null },
      { id: 'tunnel', state: 'todo', opens: 'tunnel', note: null },
    ])
  })

  it('knows the k2 command row and its cli_settings opener', () => {
    const doc = parseFirstRunDoc({ enabled: true, rows: [{ id: 'k2_cli', state: 'done', opens: 'cli_settings' }] })
    expect(doc.rows).toEqual([{ id: 'k2_cli', state: 'done', opens: 'cli_settings', note: null }])
  })

  it('draws the k2 command row only on the server\'s own machine', () => {
    const doc = parseFirstRunDoc({
      enabled: true,
      rows: [
        { id: 'phone', state: 'todo', opens: 'phone' },
        { id: 'k2_cli', state: 'todo', opens: 'cli_settings' },
      ],
    })
    expect(visibleRows(doc)?.map((r) => r.id)).toEqual(['phone', 'k2_cli'])
    expect(visibleRows(doc, { thisComputer: false })?.map((r) => r.id)).toEqual(['phone'])
    const only = { ...doc, rows: doc.rows.filter((r) => r.id === 'k2_cli') }
    expect(visibleRows(only, { thisComputer: false })).toBeNull()
  })

  it('a malformed body is a switched-off checklist', () => {
    expect(parseFirstRunDoc(null)).toEqual({ enabled: false, hidden: false, finished: false, rows: [] })
    expect(parseFirstRunDoc({ enabled: 'yes', rows: 'x' })).toEqual({
      enabled: false,
      hidden: false,
      finished: false,
      rows: [],
    })
  })

  it('shows nothing when off, hidden or finished', () => {
    const base = parseFirstRunDoc(BODY)
    expect(visibleRows(base)?.length).toBe(5)
    expect(visibleRows({ ...base, enabled: false })).toBeNull()
    expect(visibleRows({ ...base, hidden: true })).toBeNull()
    expect(visibleRows({ ...base, finished: true })).toBeNull()
    expect(rowsLeft(base.rows)).toBe(4)
  })
})
