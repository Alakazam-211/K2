// lib/file-link: the hosted web client's link to a file's raw bytes on the
// daemon's transfer door. The URL it builds, the routes it mints with, the
// byte read (progress, refusals, a short read) and the browser handoff.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ServerScope } from '@/kessel/server-scope'

const daemonCliPost = vi.fn()
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliPost: (...args: unknown[]) => daemonCliPost(...args),
}))

import {
  DOWNLOAD_LINK_TTL_SECS,
  fetchLinkBytes,
  linkFromAnswer,
  mintAttachmentLink,
  mintFileLink,
  startBrowserDownload,
  VIEW_LINK_TTL_SECS,
  type FileLink,
} from './file-link'

const ID = '0123456789abcdef0123456789abcdef'
const TOKEN = `k2dn_${ID}.Ab-c_d9`
const answer = { ok: true, url: `/_k2/transfers/downloads/${ID}`, transferToken: TOKEN, size: 5, name: 'A-101 é.pdf', mime: 'application/pdf', etag: '"e"', expiresAt: 1 }

/** The hosted web client's scope: same origin, TLS. */
const webScope = { creds: async () => ({ host: 'rosson.app.k2.dev', port: 443, secure: true, token: '' }) } as unknown as ServerScope
/** A desktop scope on the local daemon. */
const localScope = { creds: async () => ({ host: '127.0.0.1', port: 51234, secure: false, token: 'tok' }) } as unknown as ServerScope

beforeEach(() => {
  daemonCliPost.mockReset()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('linkFromAnswer', () => {
  it('builds the door URL on the scope’s own origin with the token in the query', async () => {
    const l = await linkFromAnswer(webScope, answer, 'x')
    expect(l).toEqual({
      url: `https://rosson.app.k2.dev/_k2/transfers/downloads/${ID}?token=${TOKEN}`,
      name: 'A-101 é.pdf',
      size: 5,
      mime: 'application/pdf',
    })
    const local = await linkFromAnswer(localScope, answer, 'x')
    expect(local.url).toBe(`http://127.0.0.1:51234/_k2/transfers/downloads/${ID}?token=${TOKEN}`)
  })

  it('falls back to the given name and an unknown size', async () => {
    const l = await linkFromAnswer(webScope, { url: answer.url, transferToken: TOKEN }, 'plan.pdf')
    expect(l).toMatchObject({ name: 'plan.pdf', size: null, mime: 'application/octet-stream' })
  })

  it('refuses an answer that is not a door link (never builds a URL from it)', async () => {
    await expect(linkFromAnswer(webScope, { ...answer, url: 'https://evil.example/x' }, 'x')).rejects.toThrow(/no download link/)
    await expect(linkFromAnswer(webScope, { ...answer, url: '/_k2/transfers/downloads/../../cli/x' }, 'x')).rejects.toThrow(/no download link/)
    await expect(linkFromAnswer(webScope, { ...answer, transferToken: 'k2up_abc' }, 'x')).rejects.toThrow(/no download token/)
    await expect(linkFromAnswer(webScope, { ...answer, transferToken: `${TOKEN}&x=1` }, 'x')).rejects.toThrow(/no download token/)
    await expect(linkFromAnswer(webScope, undefined, 'x')).rejects.toThrow(/no download link/)
  })
})

describe('mint', () => {
  it('a file: POST fs/download/token with its absolute path and the lifetime', async () => {
    daemonCliPost.mockResolvedValue(answer)
    const l = await mintFileLink(webScope, '/srv/ws/plans/A-101.pdf')
    expect(daemonCliPost).toHaveBeenCalledWith(webScope, 'fs/download/token', { path: '/srv/ws/plans/A-101.pdf', tokenTtlSecs: DOWNLOAD_LINK_TTL_SECS })
    expect(l.url).toContain(`?token=${TOKEN}`)
    await mintFileLink(webScope, '/srv/a.pdf', VIEW_LINK_TTL_SECS)
    expect(daemonCliPost).toHaveBeenLastCalledWith(webScope, 'fs/download/token', { path: '/srv/a.pdf', tokenTtlSecs: VIEW_LINK_TTL_SECS })
  })

  it('a Thread attachment: POST thread/attachment/token with addr, id and n', async () => {
    daemonCliPost.mockResolvedValue({ ...answer, name: undefined })
    const l = await mintAttachmentLink(webScope, { addr: 'sales', docId: 'doc-1', n: 2 }, 'notes.txt')
    expect(daemonCliPost).toHaveBeenCalledWith(webScope, 'thread/attachment/token', { addr: 'sales', id: 'doc-1', n: 2, tokenTtlSecs: DOWNLOAD_LINK_TTL_SECS })
    expect(l.name).toBe('notes.txt')
  })

  it('a refusal reaches the caller', async () => {
    daemonCliPost.mockRejectedValue(new Error('File not found'))
    await expect(mintFileLink(webScope, '/srv/none.pdf')).rejects.toThrow('File not found')
  })
})

const link: FileLink = { url: `https://rosson.app.k2.dev/_k2/transfers/downloads/${ID}?token=${TOKEN}`, name: 'a.pdf', size: 6, mime: 'application/pdf' }

function streamOf(chunks: number[][]): ReadableStream<Uint8Array> {
  return new ReadableStream({
    start(c) {
      for (const ch of chunks) c.enqueue(new Uint8Array(ch))
      c.close()
    },
  })
}

describe('fetchLinkBytes', () => {
  it('reads the raw bytes as they arrive, with progress, and sends no cookie', async () => {
    const fetchMock = vi.fn(async () => new Response(streamOf([[1, 2], [3, 4, 5], [6]]), { status: 200, headers: { 'Content-Length': '6' } }))
    vi.stubGlobal('fetch', fetchMock)
    const progress: number[] = []
    const bytes = await fetchLinkBytes(link, { onProgress: (r) => progress.push(r) })
    expect(Array.from(bytes)).toEqual([1, 2, 3, 4, 5, 6])
    expect(progress).toEqual([2 / 6, 5 / 6, 1, 1])
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    expect(url).toBe(link.url)
    expect(init).toMatchObject({ method: 'GET', credentials: 'omit', cache: 'no-store' })
  })

  it('uses the link’s size when the answer has no Content-Length', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => new Response(streamOf([[1, 2, 3], [4, 5, 6]]), { status: 200 })))
    const progress: number[] = []
    await fetchLinkBytes(link, { onProgress: (r) => progress.push(r) })
    expect(progress).toEqual([0.5, 1, 1])
  })

  it('a door refusal says the daemon’s hint', async () => {
    const body = JSON.stringify({ error: { code: 'transfer_token_expired', hint: 'this download token has expired' } })
    vi.stubGlobal('fetch', vi.fn(async () => new Response(body, { status: 401, headers: { 'Content-Type': 'application/json' } })))
    await expect(fetchLinkBytes(link)).rejects.toThrow('this download token has expired')
    vi.stubGlobal('fetch', vi.fn(async () => new Response('nope', { status: 502 })))
    await expect(fetchLinkBytes(link)).rejects.toThrow('the server answered 502')
  })

  it('a short read fails loudly instead of handing pdf.js half a file', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => new Response(streamOf([[1, 2]]), { status: 200, headers: { 'Content-Length': '6' } })))
    await expect(fetchLinkBytes(link)).rejects.toThrow('the download stopped early (2 of 6 bytes)')
  })

  it('stops when its signal aborts', async () => {
    const ac = new AbortController()
    vi.stubGlobal(
      'fetch',
      vi.fn(async (_u: string, init: RequestInit) => {
        if (init.signal?.aborted) throw new DOMException('Aborted', 'AbortError')
        return new Response(streamOf([[1]]), { status: 200 })
      }),
    )
    ac.abort()
    await expect(fetchLinkBytes(link, { signal: ac.signal })).rejects.toThrow('Aborted')
  })
})

describe('startBrowserDownload', () => {
  it('clicks a hidden <a download> to the link and removes it', () => {
    const click = vi.fn()
    const remove = vi.fn()
    const el = { href: '', download: '', rel: '', style: { display: '' }, click, remove }
    const appendChild = vi.fn()
    vi.stubGlobal('document', { createElement: vi.fn(() => el), body: { appendChild } })
    startBrowserDownload(link)
    expect(el).toMatchObject({ href: link.url, download: 'a.pdf', rel: 'noopener', style: { display: 'none' } })
    expect(appendChild).toHaveBeenCalledWith(el)
    expect(click).toHaveBeenCalledTimes(1)
    expect(remove).toHaveBeenCalledTimes(1)
  })
})
