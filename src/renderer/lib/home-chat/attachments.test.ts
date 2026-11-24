// Home chat files, the pure halves (contract-home-chat-attachments-v1):
// the upload loop (whole chunks, resume after a cut from the server's
// committed bytes, stop), the attachment shape, the attachments_updated
// frame, and the Thread door's answers. Fail loud.

import { describe, expect, it, vi } from 'vitest'

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
  daemonCliPostBytes: vi.fn(),
  daemonCliGetBlob: vi.fn(),
}))

import type { ServerScope } from '@/kessel/server-scope'
import { attachmentsOf, parseHomeChatFileLimits, parseHomeChatMessage, type HomeChatAttachment, type HomeChatUpload } from './api'
import { applyAttachmentsFrame } from './chat'
import { parseHomeChatFrame } from './events-socket'
import { homeChatAttachmentReader, homeChatFileObjects, pictureVariant } from './file-objects'
import { UPLOAD_RETRIES, UploadAbortedError, uploadHomeChatFile, type HomeChatUploadApi, type HomeChatUploadSource } from './uploads'

const scope = { hostKey: 'local' } as unknown as ServerScope
const MIB = 1 << 20

function source(size: number): HomeChatUploadSource & { reads: Array<[number, number]> } {
  const reads: Array<[number, number]> = []
  return {
    name: 'clip.mov',
    size,
    type: 'video/quicktime',
    reads,
    read: async (offset, len) => {
      reads.push([offset, len])
      return new Uint8Array(len)
    },
  }
}

function up(over: Partial<HomeChatUpload> = {}): HomeChatUpload {
  return { id: '5001', committed: 0, size: 0, complete: false, chunkBytes: MIB, maxRequestBytes: 8 * MIB, ...over }
}

describe('uploadHomeChatFile', () => {
  it('sends whole 4 MiB ranges from offset 0 and reports progress', async () => {
    const size = 9 * MIB + 5
    const src = source(size)
    let committed = 0
    const api: HomeChatUploadApi = {
      create: vi.fn(async () => up({ size })),
      send: vi.fn(async (_s, _id, offset, bytes) => {
        expect(offset).toBe(committed)
        committed += bytes.length
        return up({ committed, size, complete: committed === size })
      }),
      status: vi.fn(async () => up()),
      sleep: async () => undefined,
    }
    const seen: number[] = []
    const id = await uploadHomeChatFile(scope, '600', src, { onProgress: (r) => seen.push(r) }, api)
    expect(id).toBe('5001')
    expect(api.create).toHaveBeenCalledWith(scope, { conv: '600', filename: 'clip.mov', size, contentType: 'video/quicktime' })
    expect(src.reads).toEqual([
      [0, 4 * MIB],
      [4 * MIB, 4 * MIB],
      [8 * MIB, MIB + 5],
    ])
    expect(seen[0]).toBe(0)
    expect(seen[seen.length - 1]).toBe(1)
    expect(api.status).not.toHaveBeenCalled()
  })

  it('a cut resumes from what the server has, never from zero', async () => {
    const size = 6 * MIB
    const src = source(size)
    let server = 0
    let cut = true
    const api: HomeChatUploadApi = {
      create: vi.fn(async () => up({ size })),
      send: vi.fn(async (_s, _id, offset, bytes) => {
        if (offset !== server) throw new Error(`range from ${offset}, server has ${server}`)
        server += bytes.length
        if (cut && server === 4 * MIB) {
          // The server stored the range; its answer never came back.
          cut = false
          throw new TypeError('Load failed')
        }
        return up({ committed: server, size, complete: server === size })
      }),
      status: vi.fn(async () => up({ committed: server, size, complete: server === size })),
      sleep: vi.fn(async () => undefined),
    }
    await uploadHomeChatFile(scope, '600', src, {}, api)
    expect(api.status).toHaveBeenCalledTimes(1)
    expect(src.reads).toEqual([
      [0, 4 * MIB],
      [4 * MIB, 2 * MIB],
    ])
    expect(server).toBe(size)
  })

  it(`gives up after ${UPLOAD_RETRIES} failed tries in a row, with the error`, async () => {
    const api: HomeChatUploadApi = {
      create: vi.fn(async () => up({ size: 10 })),
      send: vi.fn(async () => {
        throw new Error('offline')
      }),
      status: vi.fn(async () => up({ size: 10 })),
      sleep: vi.fn(async () => undefined),
    }
    await expect(uploadHomeChatFile(scope, '600', source(10), {}, api)).rejects.toThrow('offline')
    expect(api.send).toHaveBeenCalledTimes(UPLOAD_RETRIES + 1)
  })

  it('stops when its signal aborts, after telling the caller the id', async () => {
    const ctl = new AbortController()
    const ids: string[] = []
    const api: HomeChatUploadApi = {
      create: vi.fn(async () => up({ size: 10 * MIB })),
      send: vi.fn(async (_s, _id, offset, bytes) => {
        ctl.abort()
        return up({ committed: offset + bytes.length, size: 10 * MIB })
      }),
      status: vi.fn(async () => up()),
      sleep: async () => undefined,
    }
    await expect(uploadHomeChatFile(scope, '600', source(10 * MIB), { signal: ctl.signal, onCreated: (id) => ids.push(id) }, api)).rejects.toBeInstanceOf(
      UploadAbortedError,
    )
    expect(ids).toEqual(['5001'])
    expect(api.send).toHaveBeenCalledTimes(1)
  })
})

const CAT = {
  id: '7002', filename: 'cat.png', content_type: 'image/png', size: 900, kind: 'image', width: 400, height: 300,
  url: '/cli/home-chat/file?id=7002', thumbnail_url: '/cli/home-chat/file?id=7002&v=thumb', media_pending: true,
}

describe('attachments on messages', () => {
  it('parses the daemon shape; unknown kinds are other; no id is dropped', () => {
    const got = attachmentsOf([CAT, { id: '9', kind: 'weird' }, { filename: 'no id' }])
    expect(got).toEqual<HomeChatAttachment[]>([
      { id: '7002', filename: 'cat.png', contentType: 'image/png', size: 900, kind: 'image', width: 400, height: 300, thumb: true, preview: false, poster: false, pending: true },
      { id: '9', filename: 'file', contentType: '', size: 0, kind: 'other', width: null, height: null, thumb: false, preview: false, poster: false, pending: false },
    ])
    const msg = parseHomeChatMessage({ id: '20', channel_id: '600', author_id: '1', body: '', attachments: [CAT] })
    expect(msg?.attachments?.map((a) => a.id)).toEqual(['7002'])
    expect(parseHomeChatFileLimits({ maxBytes: 5, perMessage: 10 })).toEqual({ maxBytes: 5, perMessage: 10, chunkBytes: MIB, maxRequestBytes: 8 * MIB })
    expect(parseHomeChatFileLimits({ maxBytes: null }).maxBytes).toBeNull()
  })

  it('attachments_updated parses and replaces only its message’s list', () => {
    const f = parseHomeChatFrame(JSON.stringify({ type: 'attachments_updated', conv: '600', msg: '20', attachments: [{ ...CAT, media_pending: false }] }))
    expect(f).toMatchObject({ type: 'attachments_updated', conv: '600', msg: '20' })
    const a = parseHomeChatMessage({ id: '20', channel_id: '600', author_id: '1', body: '', attachments: [CAT] })!
    const b = parseHomeChatMessage({ id: '21', channel_id: '600', author_id: '1', body: 'x' })!
    const next = applyAttachmentsFrame([a, b], f as { msg: string; attachments: unknown })
    expect(next[0].attachments?.[0].pending).toBe(false)
    expect(next[1]).toBe(b)
    expect(applyAttachmentsFrame([b], { msg: '99', attachments: [] })[0]).toBe(b)
  })

  it('draws as Thread file objects a person may download', () => {
    expect(homeChatFileObjects(attachmentsOf([CAT]))).toEqual([
      { kind: 'file', state: 'ok', path: 'cat.png', name: 'cat.png', size: 900, mime: 'image/png', canDownload: true },
    ])
  })

  it('the door: a thumbnail card, a preview, nothing for a picture with none, ranged downloads', async () => {
    const atts = attachmentsOf([CAT, { id: '7003', filename: 'IMG_1.heic', content_type: 'image/heic', size: 50, kind: 'image' }, { id: '7004', filename: 'a.pdf', content_type: 'application/pdf', size: 10, kind: 'document' }])
    const fetchFile = vi.fn(async (_s: unknown, _id: string, opts?: { variant?: string; range?: { offset: number; len: number } }) => {
      if (opts?.range) return new Blob([new Uint8Array(opts.range.len).fill(7)])
      return new Blob([new Uint8Array([1, 2, 3])], { type: 'image/webp' })
    })
    const read = homeChatAttachmentReader(scope, (id) => (id === '20' ? atts : undefined), fetchFile as never)
    const thumb = (await read('20', 0, { mode: 'thumb', px: 512 })) as Record<string, unknown>
    expect(thumb).toMatchObject({ kind: 'image', mime: 'image/webp', base64: 'AQID', width: 400, height: 300 })
    expect(fetchFile.mock.calls[0][2]).toEqual({ variant: 'thumb' })
    expect(pictureVariant(atts[0], 'preview')).toBe('original')
    expect(await read('20', 1, { mode: 'thumb' })).toEqual({ kind: 'none' })
    const part = (await read('20', 2, { mode: 'download', offset: 4, len: 100 })) as Record<string, unknown>
    expect(part).toEqual({ base64: 'BwcHBwcH', len: 6, size: 10, eof: true })
    expect(fetchFile.mock.calls[1][2]).toEqual({ download: true, range: { offset: 4, len: 6 } })
    await expect(read('99', 0, { mode: 'thumb' })).rejects.toThrow('no longer on the message')
  })
})
