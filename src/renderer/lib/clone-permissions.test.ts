// Clone menu permissions from whoami (prd-clone-carries-threads-and-vault-v1 §4.2).

import { describe, it, expect, beforeEach, vi } from 'vitest'

import { clearClonePermsCache, clonePermsFromWhoami, fetchClonePerms, pullAllowed, pushAllowed } from './clone-permissions'
import type { ServerScope } from '@/kessel/server-scope'

const SCOPE = 'remote' as unknown as ServerScope

beforeEach(() => clearClonePermsCache())

describe('clone menu permissions', () => {
  it('hides the pull when Can clone or the master switch is off; the push only on Can clone', () => {
    const off = clonePermsFromWhoami({ canClone: true, clonePersonalComputers: false })
    expect(pullAllowed(off)).toBe(false)
    expect(pushAllowed(off)).toBe(true)
    const noClone = clonePermsFromWhoami({ canClone: false, clonePersonalComputers: true })
    expect(pullAllowed(noClone)).toBe(false)
    expect(pushAllowed(noClone)).toBe(false)
  })

  it('an older daemon (no fields) reads as allowed', () => {
    const p = clonePermsFromWhoami({ role: 'member' })
    expect(pullAllowed(p)).toBe(true)
    expect(pushAllowed(p)).toBe(true)
  })

  it('fetches whoami once per server key within the cache window; a failure reads as allowed', async () => {
    const get = vi.fn(async () => ({ canClone: false, clonePersonalComputers: false }))
    expect(await fetchClonePerms(SCOPE, 'h1', get as never)).toEqual({ canClone: false, personal: false })
    expect(await fetchClonePerms(SCOPE, 'h1', get as never)).toEqual({ canClone: false, personal: false })
    expect(get).toHaveBeenCalledTimes(1)
    expect(get).toHaveBeenCalledWith(SCOPE, 'auth/whoami')
    const failing = vi.fn(async () => Promise.reject(new Error('offline')))
    expect(await fetchClonePerms(SCOPE, 'h2', failing as never)).toEqual({ canClone: true, personal: true })
  })
})
