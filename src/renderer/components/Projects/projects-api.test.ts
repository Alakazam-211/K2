// Pure helpers of the project-group API layer (P4). The wire calls
// themselves are exercised against the daemon (P2's route tests +
// curl-in-dev discipline); here we pin the error-recovery + nav
// partition logic the page renders from.

import { describe, it, expect, vi } from 'vitest'

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
}))

import {
  createErrorMessage,
  createProjectGroupDashboard,
  daemonErrorInfo,
  fetchProjectGroupIcon,
  normalizeHexColor,
  partitionPinned,
  reorderProjectGroupDashboards,
  setProjectGroupColor,
  setProjectGroupIcon,
} from './projects-api'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'

const get = vi.mocked(daemonCliGet)
const post = vi.mocked(daemonCliPost)

describe('daemonErrorInfo', () => {
  it('recovers code + hint from the project-group error contract', () => {
    // daemon-cli surfaces the RAW body as the message when `error` is an
    // object (its string-only fast path doesn't match) — that raw body
    // IS the contract shape.
    const err = new Error(
      '{"ok":false,"error":{"code":"name_taken","hint":"a project named \'release\' already exists"}}',
    )
    expect(daemonErrorInfo(err)).toEqual({
      code: 'name_taken',
      hint: "a project named 'release' already exists",
    })
  })

  it('yields nothing for non-JSON messages', () => {
    expect(daemonErrorInfo(new Error('fetch failed'))).toEqual({})
    expect(daemonErrorInfo('plain string')).toEqual({})
  })
})

describe('createErrorMessage', () => {
  it('surfaces name_taken with the daemon hint', () => {
    const err = new Error('{"ok":false,"error":{"code":"name_taken","hint":"taken!"}}')
    expect(createErrorMessage(err)).toBe('taken!')
  })

  it('falls back to a generic name_taken message without a hint', () => {
    const err = new Error('{"ok":false,"error":{"code":"name_taken"}}')
    expect(createErrorMessage(err)).toBe('A project with that name already exists.')
  })

  it('passes through other errors verbatim', () => {
    expect(createErrorMessage(new Error('connection refused'))).toBe('connection refused')
  })
})

describe('dashboard route wrappers (§6.7.6 contract shapes)', () => {
  it('create posts {group, name} and unwraps the {ok, dashboard} envelope', async () => {
    const dashboard = { id: 'd2', groupId: 'g1', name: 'Ops', position: 1 }
    post.mockResolvedValueOnce({ ok: true, dashboard })
    await expect(createProjectGroupDashboard('g1', 'Ops')).resolves.toBe(dashboard)
    expect(post).toHaveBeenCalledWith('project-group/dashboard/create', {
      group: 'g1',
      name: 'Ops',
    })
  })

  it('reorder posts the FULL id order and unwraps {ok, dashboards}', async () => {
    const dashboards = [{ id: 'd2' }, { id: 'd1' }]
    post.mockResolvedValueOnce({ ok: true, dashboards })
    await expect(reorderProjectGroupDashboards('g1', ['d2', 'd1'])).resolves.toBe(dashboards)
    expect(post).toHaveBeenCalledWith('project-group/dashboard/reorder', {
      group: 'g1',
      order: ['d2', 'd1'],
    })
  })

  it('reorder degrades a dashboards-less body to an empty list', async () => {
    post.mockResolvedValueOnce({ ok: true })
    await expect(reorderProjectGroupDashboards('g1', ['d1'])).resolves.toEqual([])
  })
})

describe('icon + color route wrappers (§6.7.7 contract shapes)', () => {
  it('icon GETs {group} and unwraps the {ok, found, dataUrl} envelope', async () => {
    get.mockResolvedValueOnce({ ok: true, found: true, dataUrl: 'data:image/png;base64,x' })
    await expect(fetchProjectGroupIcon('g1')).resolves.toEqual({
      found: true,
      dataUrl: 'data:image/png;base64,x',
    })
    expect(get).toHaveBeenCalledWith('project-group/icon', { group: 'g1' })
  })

  it('icon degrades a fieldless body to not-found (advisory decoration)', async () => {
    get.mockResolvedValueOnce({ ok: true })
    await expect(fetchProjectGroupIcon('g1')).resolves.toEqual({ found: false, dataUrl: null })
  })

  it('set-icon posts {group, dataUrl} and null clears', async () => {
    post.mockResolvedValueOnce({ ok: true })
    await setProjectGroupIcon('g1', 'data:image/png;base64,x')
    expect(post).toHaveBeenCalledWith('project-group/set-icon', {
      group: 'g1',
      dataUrl: 'data:image/png;base64,x',
    })
    post.mockResolvedValueOnce({ ok: true })
    await setProjectGroupIcon('g1', null)
    expect(post).toHaveBeenCalledWith('project-group/set-icon', { group: 'g1', dataUrl: null })
  })

  it('set-color posts {group, color} and null clears', async () => {
    post.mockResolvedValueOnce({ ok: true })
    await setProjectGroupColor('g1', '#61afef')
    expect(post).toHaveBeenCalledWith('project-group/set-color', {
      group: 'g1',
      color: '#61afef',
    })
    post.mockResolvedValueOnce({ ok: true })
    await setProjectGroupColor('g1', null)
    expect(post).toHaveBeenCalledWith('project-group/set-color', { group: 'g1', color: null })
  })
})

describe('normalizeHexColor', () => {
  it('accepts #rrggbb, folds case, and tolerates a missing #', () => {
    expect(normalizeHexColor('#61afef')).toBe('#61afef')
    expect(normalizeHexColor('61AFEF')).toBe('#61afef')
    expect(normalizeHexColor('  #E06C75  ')).toBe('#e06c75')
  })

  it('expands 3-digit shorthand', () => {
    expect(normalizeHexColor('#abc')).toBe('#aabbcc')
    expect(normalizeHexColor('F0a')).toBe('#ff00aa')
  })

  it('rejects everything else', () => {
    expect(normalizeHexColor('')).toBeNull()
    expect(normalizeHexColor('#12345')).toBeNull()
    expect(normalizeHexColor('#1234567')).toBeNull()
    expect(normalizeHexColor('red')).toBeNull()
    expect(normalizeHexColor('#gggggg')).toBeNull()
  })
})

describe('partitionPinned', () => {
  it('splits pinned-first sections preserving order within each', () => {
    const groups = [
      { id: 'a', pinned: false },
      { id: 'b', pinned: true },
      { id: 'c', pinned: false },
      { id: 'd', pinned: true },
    ]
    const { pinned, unpinned } = partitionPinned(groups)
    expect(pinned.map((g) => g.id)).toEqual(['b', 'd'])
    expect(unpinned.map((g) => g.id)).toEqual(['a', 'c'])
  })
})
