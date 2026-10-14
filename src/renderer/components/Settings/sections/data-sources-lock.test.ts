// Locking a Garden offers to pin the sources it shows (GV65; B2).

import { describe, it, expect, vi, beforeEach } from 'vitest'

const daemonCliGet = vi.hoisted(() => vi.fn())
const daemonCliPost = vi.hoisted(() => vi.fn())
const scopes = vi.hoisted(() => [] as string[])
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet, daemonCliPost }))
vi.mock('@/lib/zen/zen-api', () => ({ zenLocalScope: () => 'local-scope' }))
vi.mock('@/kessel/server-scope', () => ({
  primaryScope: () => ({ hostKey: 'local' }),
  scopeForHost: (h: string) => {
    scopes.push(h)
    return { hostKey: h }
  },
}))

import { gardenSourceAddresses, offerPinOnLock, pinAddresses, splitAddress } from './data-sources-lock'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'

const PAGE = {
  page: {
    widgets: [
      { kind: 'custom', sourceAddresses: ['fleet::akzm.k2.dev', 'latest::akzm.k2.dev'] },
      { kind: 'custom', sourceAddresses: ['fleet::akzm.k2.dev', 'money'] },
      { kind: 'agents' },
    ],
  },
}

beforeEach(() => {
  daemonCliGet.mockReset()
  daemonCliPost.mockReset()
  scopes.length = 0
})

describe('data-sources-lock', () => {
  it('collects every address a page shows, once', () => {
    expect(gardenSourceAddresses(PAGE.page)).toEqual(['fleet::akzm.k2.dev', 'latest::akzm.k2.dev', 'money'])
    expect(gardenSourceAddresses({ widgets: [{}] })).toEqual([])
    expect(gardenSourceAddresses(null)).toEqual([])
    expect(splitAddress('money')).toEqual({ name: 'money', host: 'local' })
    expect(splitAddress('fleet::akzm.k2.dev')).toEqual({ name: 'fleet', host: 'akzm.k2.dev' })
  })

  it('pins each source on its own server and reports failures', async () => {
    const seen: string[] = []
    const r = await pinAddresses(['fleet::akzm.k2.dev', 'money'], async (host, name) => {
      seen.push(`${host}/${name}`)
      if (name === 'money') throw new Error('403: {"error":{"code":"person_only","hint":"x"}}')
    })
    expect(seen).toEqual(['akzm.k2.dev/fleet', 'local/money'])
    expect(r.pinned).toEqual(['fleet::akzm.k2.dev'])
    expect(r.failed).toEqual([{ address: 'money', text: 'This needs a person (Owner or Admin) on this server.' }])
  })

  it('asks once, then pins through each server’s scope', async () => {
    daemonCliGet.mockResolvedValue(PAGE)
    daemonCliPost.mockResolvedValue({ pinned: true })
    const confirm = vi.fn(async (_o: { title: string }) => true)
    useConfirmDialogStore.setState({ confirm } as never)
    const r = await offerPinOnLock('g1', 'Fleet')
    expect(daemonCliGet).toHaveBeenCalledWith('local-scope', 'zen/get', { garden: 'g1' })
    expect(confirm.mock.calls[0]?.[0].title).toBe('Also pin the 3 sources it shows?')
    expect(r?.pinned).toEqual(['fleet::akzm.k2.dev', 'latest::akzm.k2.dev', 'money'])
    expect(scopes).toEqual(['akzm.k2.dev', 'akzm.k2.dev', 'local'])
    expect(daemonCliPost.mock.calls.map((c) => [c[0].hostKey, c[1], c[2]])).toEqual([
      ['akzm.k2.dev', 'sources/pin', { name: 'fleet' }],
      ['akzm.k2.dev', 'sources/pin', { name: 'latest' }],
      ['local', 'sources/pin', { name: 'money' }],
    ])
  })

  it('offers nothing for a Garden without sources, and pins nothing on No', async () => {
    daemonCliGet.mockResolvedValue({ page: { widgets: [] } })
    const confirm = vi.fn(async () => false)
    useConfirmDialogStore.setState({ confirm } as never)
    expect(await offerPinOnLock('g1', 'Plain')).toBeNull()
    expect(confirm).not.toHaveBeenCalled()
    daemonCliGet.mockResolvedValue(PAGE)
    expect(await offerPinOnLock('g1', 'Fleet')).toBeNull()
    expect(daemonCliPost).not.toHaveBeenCalled()
  })
})
