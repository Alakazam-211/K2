// Pure helpers of the project-group API layer (P4). The wire calls
// themselves are exercised against the daemon (P2's route tests +
// curl-in-dev discipline); here we pin the error-recovery + nav
// partition logic the page renders from.

import { describe, it, expect, vi } from 'vitest'

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
}))

import { createErrorMessage, daemonErrorInfo, partitionPinned } from './projects-api'

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
