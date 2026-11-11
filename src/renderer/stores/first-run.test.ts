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

  it('dismissOffer posts the dismissal and takes the body it returns', async () => {
    h.daemonCliPost.mockResolvedValue({ ...BODY, offer: null })
    await useFirstRunStore.getState().dismissOffer()
    expect(h.daemonCliPost).toHaveBeenCalledWith('first-run/offer/dismiss', {})
    expect(entry()?.doc?.offer).toBeNull()
  })

  it('createK5 posts the tool, name and handle, returns the seat and reads the status again', async () => {
    h.daemonCliPost.mockResolvedValue({ workspaceId: 'ws-k5', name: 'Kay', path: '/home/r/k2-agents/kay' })
    const seat = await useFirstRunStore.getState().createK5({ tool: 'codex', name: ' Kay ', handle: 'kay' })
    expect(h.daemonCliPost).toHaveBeenCalledWith('first-run/k5', { tool: 'codex', name: 'Kay', handle: 'kay' })
    expect(seat).toEqual({ workspaceId: 'ws-k5', name: 'Kay', path: '/home/r/k2-agents/kay' })
    expect(h.daemonCliGet).toHaveBeenCalledWith('first-run/status')
  })

  it('createK5 leaves out an empty name and handle, and throws the refusal', async () => {
    h.daemonCliPost.mockRejectedValue(Object.assign(new Error('K5 already exists'), { code: 'k5_exists' }))
    await expect(useFirstRunStore.getState().createK5({ tool: 'claude', name: '  ', handle: '' })).rejects.toThrow(
      'K5 already exists',
    )
    expect(h.daemonCliPost).toHaveBeenCalledWith('first-run/k5', { tool: 'claude' })
  })

  it('startChiefOfStaff posts the start and returns K5\'s workspace', async () => {
    h.daemonCliPost.mockResolvedValue({ workspaceId: 'ws-k5' })
    await expect(useFirstRunStore.getState().startChiefOfStaff()).resolves.toBe('ws-k5')
    expect(h.daemonCliPost).toHaveBeenCalledWith('first-run/chief-of-staff/start', {})
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
        // The k2 command row, dropped 11-10: an older daemon may still send it.
        { id: 'k2_cli', state: 'todo', opens: 'cli_settings' },
        { id: 'phone', state: 'todo', opens: 'cli_settings' },
        { id: 'tunnel', state: 'blocked', opens: 'tunnel', note: 'other' },
      ],
    })
    expect(doc.rows).toEqual([
      { id: 'llm_sign_in', state: 'todo', opens: null, note: null },
      { id: 'phone', state: 'todo', opens: null, note: null },
      { id: 'tunnel', state: 'todo', opens: 'tunnel', note: null },
    ])
  })

  it('a malformed body is a switched-off checklist', () => {
    const off = { enabled: false, hidden: false, finished: false, rows: [], offer: null, k5: null, k5Defaults: null }
    expect(parseFirstRunDoc(null)).toEqual(off)
    expect(parseFirstRunDoc({ enabled: 'yes', rows: 'x', offer: 7, k5: 'x', k5Defaults: [] })).toEqual(off)
  })

  it('reads the K5 fields, the chief_of_staff row, its opens values and the needs_k5 note', () => {
    const doc = parseFirstRunDoc({
      enabled: true,
      offer: 'meet_k5',
      k5: { workspaceId: 'ws-k5', name: 'K5', path: '/home/r/k2-agents/k5' },
      k5Defaults: { name: 'K5', handle: 'k5', path: '/home/r/k2-agents/k5' },
      rows: [
        { id: 'llm_sign_in', state: 'done', opens: 'llm_sign_in' },
        { id: 'chief_of_staff', state: 'todo', opens: 'k5_sheet', note: 'needs_k5' },
        { id: 'tunnel', state: 'todo', opens: 'tunnel' },
      ],
    })
    expect(doc.offer).toBe('meet_k5')
    expect(doc.k5).toEqual({ workspaceId: 'ws-k5', name: 'K5', path: '/home/r/k2-agents/k5' })
    expect(doc.k5Defaults).toEqual({ name: 'K5', handle: 'k5', path: '/home/r/k2-agents/k5' })
    expect(doc.rows.map((r) => r.id)).toEqual(['llm_sign_in', 'chief_of_staff', 'tunnel'])
    expect(doc.rows[1]).toEqual({ id: 'chief_of_staff', state: 'todo', opens: 'k5_sheet', note: 'needs_k5' })
    const cos = parseFirstRunDoc({ enabled: true, rows: [{ id: 'chief_of_staff', state: 'todo', opens: 'k5_thread_cos' }] })
    expect(cos.rows[0].opens).toBe('k5_thread_cos')
  })

  it('drops an unknown offer, a seat without an id and defaults without a name or handle', () => {
    const doc = parseFirstRunDoc({
      enabled: true,
      offer: 'meet_k6',
      k5: { name: 'K5' },
      k5Defaults: { name: 'K5', path: '/x' },
      rows: [{ id: 'chief_of_staff', state: 'todo', opens: 'k5_garden', note: 'needs_k6' }],
    })
    expect(doc.offer).toBeNull()
    expect(doc.k5).toBeNull()
    expect(doc.k5Defaults).toBeNull()
    expect(doc.rows[0]).toEqual({ id: 'chief_of_staff', state: 'todo', opens: null, note: null })
  })

  it('reads the dev_tools row first, its opener and the installing note', () => {
    const doc = parseFirstRunDoc({
      enabled: true,
      rows: [
        { id: 'dev_tools', state: 'todo', opens: 'dev_tools', note: 'installing' },
        { id: 'llm_sign_in', state: 'todo', opens: 'llm_sign_in' },
      ],
    })
    expect(doc.rows[0]).toEqual({ id: 'dev_tools', state: 'todo', opens: 'dev_tools', note: 'installing' })
    expect(doc.rows[0].command).toBeUndefined()
  })

  it("takes Linux's dev_tools note as the command, and nowhere else", () => {
    const doc = parseFirstRunDoc({
      enabled: true,
      rows: [
        { id: 'dev_tools', state: 'todo', opens: null, note: 'sudo apt install git' },
        { id: 'phone', state: 'todo', opens: null, note: 'sudo apt install git' },
      ],
    })
    expect(doc.rows[0]).toEqual({ id: 'dev_tools', state: 'todo', opens: null, note: null, command: 'sudo apt install git' })
    expect(doc.rows[1]).toEqual({ id: 'phone', state: 'todo', opens: null, note: null })
    // An unknown opens value with an unknown note: both dropped, no command.
    const odd = parseFirstRunDoc({ enabled: true, rows: [{ id: 'dev_tools', state: 'todo', opens: 'dev_tools_v2', note: 'brew' }] })
    expect(odd.rows[0]).toEqual({ id: 'dev_tools', state: 'todo', opens: null, note: null })
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
