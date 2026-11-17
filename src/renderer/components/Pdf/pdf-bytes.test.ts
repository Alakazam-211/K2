// A PDF tab's bytes: the desktop reads fs/read-binary as before; the hosted
// web client reads the raw bytes over the transfer door (with progress) and
// falls back to fs/read-binary when the server can't make a link.

import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ServerScope } from '@/kessel/server-scope'

const web = vi.hoisted(() => ({ on: false }))
const daemonCliGet = vi.fn()
const mintFileLink = vi.fn()
const fetchLinkBytes = vi.fn()

vi.mock('@/lib/is-web', () => ({ isWebClient: () => web.on }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (...a: unknown[]) => daemonCliGet(...a),
  isHostSwitchedError: (e: unknown) => e instanceof Error && e.name === 'HostSwitchedError',
}))
vi.mock('@/lib/file-link', () => ({
  VIEW_LINK_TTL_SECS: 120,
  mintFileLink: (...a: unknown[]) => mintFileLink(...a),
  fetchLinkBytes: (...a: unknown[]) => fetchLinkBytes(...a),
}))

import { base64Bytes, bytesToBinaryString, loadPdfBytes } from './pdf-bytes'

const scope = { id: 'test' } as unknown as ServerScope
const link = { url: 'https://rosson.app.k2.dev/_k2/transfers/downloads/x?token=k2dn_x.y', name: 'a.pdf', size: 3, mime: 'application/pdf' }

beforeEach(() => {
  web.on = false
  daemonCliGet.mockReset()
  mintFileLink.mockReset()
  fetchLinkBytes.mockReset()
})

describe('loadPdfBytes', () => {
  it('desktop: fs/read-binary, decoded (no link, no fetch)', async () => {
    daemonCliGet.mockResolvedValue({ base64: btoa('%PDF') })
    const bytes = await loadPdfBytes(scope, '/w/a.pdf')
    expect(new TextDecoder().decode(bytes)).toBe('%PDF')
    expect(daemonCliGet).toHaveBeenCalledWith(scope, 'fs/read-binary', { path: '/w/a.pdf' })
    expect(mintFileLink).not.toHaveBeenCalled()
    expect(fetchLinkBytes).not.toHaveBeenCalled()
  })

  it('hosted web: a view link, then the raw bytes with progress (no fs/read-binary)', async () => {
    web.on = true
    mintFileLink.mockResolvedValue(link)
    fetchLinkBytes.mockImplementation(async (_l: unknown, o: { onProgress?: (r: number) => void }) => {
      o.onProgress?.(0.5)
      o.onProgress?.(1)
      return new Uint8Array([37, 80, 68])
    })
    const seen: number[] = []
    const ac = new AbortController()
    const bytes = await loadPdfBytes(scope, '/w/big plans.pdf', { onProgress: (r) => seen.push(r), signal: ac.signal })
    expect(Array.from(bytes)).toEqual([37, 80, 68])
    expect(mintFileLink).toHaveBeenCalledWith(scope, '/w/big plans.pdf', 120)
    expect(fetchLinkBytes).toHaveBeenCalledWith(link, expect.objectContaining({ signal: ac.signal }))
    expect(seen).toEqual([0.5, 1])
    expect(daemonCliGet).not.toHaveBeenCalled()
  })

  it('hosted web, a server without links: falls back to fs/read-binary', async () => {
    web.on = true
    mintFileLink.mockRejectedValue(Object.assign(new Error('resumable uploads and download tokens are an App guest’s'), { status: 400, code: 'usage' }))
    daemonCliGet.mockResolvedValue({ base64: btoa('%PDF-1.7') })
    const bytes = await loadPdfBytes(scope, '/w/a.pdf')
    expect(new TextDecoder().decode(bytes)).toBe('%PDF-1.7')
    expect(fetchLinkBytes).not.toHaveBeenCalled()
  })

  it('hosted web: the fallback’s own refusal is the error shown', async () => {
    web.on = true
    mintFileLink.mockRejectedValue(new Error('File not found'))
    daemonCliGet.mockRejectedValue(new Error('File not found'))
    await expect(loadPdfBytes(scope, '/w/none.pdf')).rejects.toThrow('File not found')
  })

  it('hosted web: a door failure is not retried through the 50 MB route', async () => {
    web.on = true
    mintFileLink.mockResolvedValue(link)
    fetchLinkBytes.mockRejectedValue(new Error('the download stopped early (1 of 3 bytes)'))
    await expect(loadPdfBytes(scope, '/w/a.pdf')).rejects.toThrow('the download stopped early')
    expect(daemonCliGet).not.toHaveBeenCalled()
  })

  it('a host switch mid-mint is not papered over', async () => {
    web.on = true
    mintFileLink.mockRejectedValue(Object.assign(new Error('switched'), { name: 'HostSwitchedError' }))
    await expect(loadPdfBytes(scope, '/w/a.pdf')).rejects.toThrow('switched')
    expect(daemonCliGet).not.toHaveBeenCalled()
  })

  it('an answer with no base64 fails loudly', async () => {
    daemonCliGet.mockResolvedValue({})
    await expect(loadPdfBytes(scope, '/w/a.pdf')).rejects.toThrow('fs/read-binary answered no file')
  })
})

describe('bytesToBinaryString', () => {
  it('is atob’s binary string, byte for byte, across slice edges', () => {
    const bytes = new Uint8Array(0x8000 * 2 + 17)
    for (let i = 0; i < bytes.length; i++) bytes[i] = (i * 7 + 3) & 0xff
    const bin = bytesToBinaryString(bytes)
    expect(bin.length).toBe(bytes.length)
    for (const i of [0, 1, 0x7fff, 0x8000, 0x8001, 0xffff, 0x10000, bytes.length - 1]) expect(bin.charCodeAt(i)).toBe(bytes[i])
    // Round trip through base64, as the desktop path reads it.
    expect(Array.from(base64Bytes(btoa(bin)))).toEqual(Array.from(bytes))
  })

  it('an empty file is an empty string', () => {
    expect(bytesToBinaryString(new Uint8Array(0))).toBe('')
  })
})
