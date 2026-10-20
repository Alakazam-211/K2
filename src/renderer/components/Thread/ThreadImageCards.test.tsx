// @vitest-environment jsdom
// Thread image attachments as pictures (S3, `thread-images-v1`). Fail loud.
//
//   1. Every image type (png, jpeg, gif, webp, heic, heif, svg, bmp, tiff)
//      draws as an <img> of the daemon's thumbnail (`mode=thumb&px=512`,
//      through the Thread's own server scope), name and size under it.
//   2. `hidden` and `gone` stay chips and never fetch.
//   3. 415 (no thumbnail) falls back to today's chip.
//   4. Only <img src> with a blob URL: no inline SVG, no innerHTML.
//   5. A click opens through the file actions provider; without one, the
//      full-size viewer (`mode=preview`), Esc closes it.
//   6. Without the feature, or without a source: chips only, no fetch.
//   7. Cached: a remount doesn't fetch again; `n` is the index on the message.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: h.daemonCliGet,
  daemonCliPost: vi.fn(async () => ({})),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))

import type { JSX } from 'react'
import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react'
import type { ServerScope } from '@/kessel/server-scope'
import type { OverlayThreadItem, ThreadFileObject } from '@/components/SessionView/overlayThread'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { fakeScope } from '@/test-utils/fake-scope'
import { __resetThreadThumbsForTests, __threadThumbCacheSize, THREAD_IMAGE_MIMES } from '@/lib/thread-image-thumbs'
import { ThreadFileActionsProvider, ThreadFileObjects, type ThreadFileActions } from './ThreadFileObjects'
import { ThreadAttachmentSourceProvider, type ThreadAttachmentSource } from './ThreadImageObjects'
import { ThreadBubble } from './ThreadZenKit'

const PNG_B64 = btoa('\x89PNG fake bytes')
let urlSeq = 0
const created: Blob[] = []
const revoked: string[] = []

beforeEach(() => {
  __resetThreadThumbsForTests()
  urlSeq = 0
  created.length = 0
  revoked.length = 0
  h.daemonCliGet.mockReset()
  ;(URL as unknown as { createObjectURL: (b: Blob) => string }).createObjectURL = (b: Blob) => {
    created.push(b)
    return `blob:thumb-${++urlSeq}`
  }
  ;(URL as unknown as { revokeObjectURL: (u: string) => void }).revokeObjectURL = (u: string) => {
    revoked.push(u)
  }
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

function scopeWith(features: string[]): ServerScope {
  return { ...fakeScope('box.example'), serverSupports: (f: string) => features.includes(f) } as ServerScope
}

const ON = scopeWith(['thread-attachments-v1', 'thread-images-v1'])
const OFF = scopeWith(['thread-attachments-v1'])

function image(name: string, mime: string, extra: Partial<ThreadFileObject> = {}): ThreadFileObject {
  return { kind: 'file', state: 'ok', path: `out/${name}`, name, size: 421888, mime, modifiedAt: 1760790000000, ...extra }
}

function thumbAnswer(mime = 'image/jpeg', base64 = PNG_B64): Record<string, unknown> {
  return { ok: true, kind: 'image', mime, width: 1600, height: 900, base64, name: 'x', size: 1, state: 'ok' }
}

function renderFiles(
  files: ThreadFileObject[],
  opts: { source?: ThreadAttachmentSource | null; actions?: ThreadFileActions; docId?: string } = {},
): ReturnType<typeof render> {
  const source = opts.source === undefined ? { scope: ON, addr: 'sales' } : opts.source
  let node: JSX.Element = <ThreadFileObjects files={files} mine={false} docId={opts.docId ?? 'doc-1'} />
  if (opts.actions) node = <ThreadFileActionsProvider value={opts.actions}>{node}</ThreadFileActionsProvider>
  return render(<ThreadAttachmentSourceProvider value={source}>{node}</ThreadAttachmentSourceProvider>)
}

const EXT: Record<string, string> = {
  'image/png': 'png',
  'image/jpeg': 'jpg',
  'image/gif': 'gif',
  'image/webp': 'webp',
  'image/heic': 'heic',
  'image/heif': 'heif',
  'image/svg+xml': 'svg',
  'image/bmp': 'bmp',
  'image/tiff': 'tiff',
}

describe('S3: image attachments draw as pictures', () => {
  it('covers every image type the daemon names', () => {
    expect([...THREAD_IMAGE_MIMES].sort()).toEqual(Object.keys(EXT).sort())
  })

  for (const mime of Object.keys(EXT)) {
    it(`${mime}: an <img> of the thumbnail, fetched through the Thread's server`, async () => {
      h.daemonCliGet.mockResolvedValue(thumbAnswer(mime === 'image/svg+xml' ? 'image/svg+xml' : 'image/jpeg'))
      const name = `hero.${EXT[mime]}`
      const { container } = renderFiles([image(name, mime)])
      await waitFor(() => expect(container.querySelector('[data-thread-image] img')).not.toBeNull())
      const img = container.querySelector('[data-thread-image] img') as HTMLImageElement
      expect(img.getAttribute('src')).toMatch(/^blob:thumb-\d+$/)
      expect(img.getAttribute('alt')).toBe(name)
      expect(h.daemonCliGet).toHaveBeenCalledTimes(1)
      const [scope, route, params] = h.daemonCliGet.mock.calls[0]
      expect(scope).toBe(ON)
      expect(route).toBe('thread/attachment')
      expect(params).toEqual({ addr: 'sales', id: 'doc-1', n: 0, mode: 'thumb', px: 512 })
      // Name and size, muted, under the picture; no chip.
      expect(container.querySelector('[data-thread-image-caption] [data-thread-file-name]')?.textContent).toBe(name)
      expect(container.querySelector('[data-thread-image-caption] [data-thread-file-meta]')?.textContent).toBe('412 KB')
      expect(container.querySelector('[data-thread-file]')).toBeNull()
    })
  }

  it('a skeleton with the name while the thumbnail loads', async () => {
    let resolve!: (v: unknown) => void
    h.daemonCliGet.mockReturnValue(new Promise((r) => (resolve = r)))
    const { container } = renderFiles([image('hero.png', 'image/png')])
    const card = container.querySelector('[data-thread-image]') as HTMLElement
    expect(card.hasAttribute('data-thread-image-loading')).toBe(true)
    expect(card.querySelector('[data-thread-image-skeleton]')?.textContent).toBe('hero.png')
    expect(card.querySelector('img')).toBeNull()
    await act(async () => resolve(thumbAnswer()))
    expect(card.querySelector('img')).not.toBeNull()
    expect(card.hasAttribute('data-thread-image-loading')).toBe(false)
  })

  it('changed: the picture with the amber dot and "changed since sent"', async () => {
    h.daemonCliGet.mockResolvedValue(thumbAnswer())
    const { container } = renderFiles([image('hero.png', 'image/png', { state: 'changed' })])
    await waitFor(() => expect(container.querySelector('[data-thread-image] img')).not.toBeNull())
    expect(container.querySelector('[data-thread-image]')?.getAttribute('data-state')).toBe('changed')
    expect(container.querySelector('[data-thread-image] [data-thread-file-changed]')).not.toBeNull()
    expect(container.querySelector('[data-thread-image-caption]')?.textContent).toContain('changed since sent')
  })

  it('hidden and gone stay chips and never fetch', () => {
    const { container } = renderFiles([
      { kind: 'file', state: 'hidden' },
      image('old.png', 'image/png', { state: 'gone' }),
    ])
    expect(h.daemonCliGet).not.toHaveBeenCalled()
    expect(container.querySelector('[data-thread-image]')).toBeNull()
    expect(container.querySelector('[data-thread-file="hidden"]')).not.toBeNull()
    expect(container.querySelector('[data-thread-file="out/old.png"]')?.getAttribute('data-state')).toBe('gone')
  })

  it('415 (no thumbnail, e.g. HEIC with no converter) falls back to the chip', async () => {
    const warn = vi.spyOn(console, 'warn')
    h.daemonCliGet.mockRejectedValue(new DaemonHttpError('no thumbnail', 415, 'no_thumbnail'))
    const { container } = renderFiles([image('shot.heic', 'image/heic')])
    await waitFor(() => expect(container.querySelector('[data-thread-file="out/shot.heic"]')).not.toBeNull())
    expect(container.querySelector('[data-thread-image]')).toBeNull()
    expect(container.querySelector('img')).toBeNull()
    // Expected, not noise.
    expect(warn).not.toHaveBeenCalled()
    // Remembered: a remount does not ask again.
    cleanup()
    renderFiles([image('shot.heic', 'image/heic')])
    await waitFor(() => expect(document.querySelector('[data-thread-file="out/shot.heic"]')).not.toBeNull())
    expect(h.daemonCliGet).toHaveBeenCalledTimes(1)
  })

  it('any other error falls back to the chip too (and a later mount retries)', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    h.daemonCliGet.mockRejectedValueOnce(new Error('network down'))
    const { container } = renderFiles([image('hero.png', 'image/png')])
    await waitFor(() => expect(container.querySelector('[data-thread-file="out/hero.png"]')).not.toBeNull())
    cleanup()
    h.daemonCliGet.mockResolvedValue(thumbAnswer())
    const again = renderFiles([image('hero.png', 'image/png')])
    await waitFor(() => expect(again.container.querySelector('[data-thread-image] img')).not.toBeNull())
    expect(h.daemonCliGet).toHaveBeenCalledTimes(2)
  })

  it('an answer that is not a picture is refused (chip), never drawn', async () => {
    h.daemonCliGet.mockResolvedValue({ ok: true, kind: 'text', mime: 'text/html', base64: btoa('<b>x</b>') })
    const { container } = renderFiles([image('hero.png', 'image/png')])
    await waitFor(() => expect(container.querySelector('[data-thread-file="out/hero.png"]')).not.toBeNull())
    expect(created).toHaveLength(0)
  })

  it('an SVG only ever reaches the page as <img src="blob:…">, never as markup', async () => {
    const svg = '<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script><rect width="9" height="9"/></svg>'
    h.daemonCliGet.mockResolvedValue(thumbAnswer('image/svg+xml', btoa(svg)))
    const { container } = renderFiles([image('logo.svg', 'image/svg+xml')])
    await waitFor(() => expect(container.querySelector('[data-thread-image] img')).not.toBeNull())
    expect(container.querySelector('svg rect, script')).toBeNull()
    expect(container.innerHTML).not.toContain('<script')
    expect(container.innerHTML).not.toContain('alert(1)')
    expect(created[0].type).toBe('image/svg+xml')
    // And the code never reaches for innerHTML.
    for (const file of ['ThreadFileObjects.tsx', 'ThreadImageObjects.tsx']) {
      const src = readFileSync(join(__dirname, file), 'utf8')
      expect(src, file).not.toContain('dangerouslySetInnerHTML')
      expect(src, file).not.toContain('innerHTML')
    }
  })

  it('several images wrap in a grid; files that are not images stay chips beside them', async () => {
    h.daemonCliGet.mockResolvedValue(thumbAnswer())
    const pdf: ThreadFileObject = { kind: 'file', state: 'ok', path: 'docs/a.pdf', name: 'a.pdf', size: 10, mime: 'application/pdf' }
    const { container } = renderFiles([pdf, image('a.png', 'image/png'), image('b.png', 'image/png')])
    await waitFor(() => expect(container.querySelectorAll('[data-thread-image] img')).toHaveLength(2))
    const grid = container.querySelector('[data-thread-images]') as HTMLElement
    expect(grid.querySelectorAll('[data-thread-image]')).toHaveLength(2)
    expect((grid.querySelector('[data-thread-image]') as HTMLElement).style.width).toBe('160px')
    expect(container.querySelector('[data-thread-file="docs/a.pdf"]')).not.toBeNull()
    // `n` is each image's place on the message: 1 and 2 (the pdf is 0).
    const ns = h.daemonCliGet.mock.calls.map((c) => (c[2] as { n: number }).n).sort()
    expect(ns).toEqual([1, 2])
  })

  it('one image is a 240 px card', async () => {
    h.daemonCliGet.mockResolvedValue(thumbAnswer())
    const { container } = renderFiles([image('a.png', 'image/png')])
    await waitFor(() => expect(container.querySelector('[data-thread-image] img')).not.toBeNull())
    expect((container.querySelector('[data-thread-image]') as HTMLElement).style.width).toBe('240px')
  })

  it('cached: a re-render and a remount draw from the cache, no second fetch', async () => {
    h.daemonCliGet.mockResolvedValue(thumbAnswer())
    const files = [image('a.png', 'image/png')]
    const first = renderFiles(files)
    await waitFor(() => expect(first.container.querySelector('[data-thread-image] img')).not.toBeNull())
    first.rerender(
      <ThreadAttachmentSourceProvider value={{ scope: ON, addr: 'sales' }}>
        <ThreadFileObjects files={[...files]} mine={false} docId="doc-1" />
      </ThreadAttachmentSourceProvider>,
    )
    cleanup()
    const second = renderFiles(files)
    // Drawn at once from the cache: no loading state at all.
    expect(second.container.querySelector('[data-thread-image] img')?.getAttribute('src')).toBe('blob:thumb-1')
    expect(h.daemonCliGet).toHaveBeenCalledTimes(1)
    expect(__threadThumbCacheSize()).toBe(1)
    // A new modifiedAt is a new picture.
    cleanup()
    renderFiles([image('a.png', 'image/png', { modifiedAt: 1760799999999 })])
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledTimes(2))
  })
})

describe('S3: opening a picture', () => {
  it('with the file actions provider: opens the file tab (the provider), no viewer', async () => {
    h.daemonCliGet.mockResolvedValue(thumbAnswer())
    const open = vi.fn()
    const file = image('hero.png', 'image/png')
    const { container } = renderFiles([file], { actions: { open } })
    await waitFor(() => expect(container.querySelector('[data-thread-image] img')).not.toBeNull())
    fireEvent.click(container.querySelector('[data-thread-image-open]') as HTMLElement)
    expect(open).toHaveBeenCalledTimes(1)
    expect(open.mock.calls[0][0]).toEqual(file)
    expect(document.querySelector('[data-thread-image-lightbox]')).toBeNull()
  })

  it('without one (Zen, Home): the full-size viewer loads mode=preview; Esc closes it', async () => {
    h.daemonCliGet.mockImplementation(async (_s: unknown, _r: string, p: { mode: string }) =>
      p.mode === 'preview' ? thumbAnswer('image/png') : thumbAnswer(),
    )
    const { container } = renderFiles([image('hero.png', 'image/png')])
    await waitFor(() => expect(container.querySelector('[data-thread-image] img')).not.toBeNull())
    fireEvent.click(container.querySelector('[data-thread-image-open]') as HTMLElement)
    const box = document.querySelector('[data-thread-image-lightbox]') as HTMLElement
    expect(box).not.toBeNull()
    // The thumbnail at once, then the whole image.
    await waitFor(() => expect(box.hasAttribute('data-full')).toBe(true))
    expect(box.querySelector('img')?.getAttribute('src')).toBe('blob:thumb-2')
    const preview = h.daemonCliGet.mock.calls.find((c) => (c[2] as { mode: string }).mode === 'preview')
    expect(preview?.[2]).toEqual({ addr: 'sales', id: 'doc-1', n: 0, mode: 'preview' })
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(document.querySelector('[data-thread-image-lightbox]')).toBeNull()
    // The whole image's URL is let go; the cached thumbnail is kept.
    expect(revoked).toEqual(['blob:thumb-2'])
  })

  it('a click on the viewer closes it; a failed preview keeps the thumbnail up', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    h.daemonCliGet.mockImplementation(async (_s: unknown, _r: string, p: { mode: string }) => {
      if (p.mode === 'preview') throw new DaemonHttpError('too large', 413, 'preview_too_large')
      return thumbAnswer()
    })
    const { container } = renderFiles([image('hero.png', 'image/png')])
    await waitFor(() => expect(container.querySelector('[data-thread-image] img')).not.toBeNull())
    fireEvent.click(container.querySelector('[data-thread-image-open]') as HTMLElement)
    const box = document.querySelector('[data-thread-image-lightbox]') as HTMLElement
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledTimes(2))
    expect(box.querySelector('img')?.getAttribute('src')).toBe('blob:thumb-1')
    expect(box.hasAttribute('data-full')).toBe(false)
    fireEvent.click(box)
    expect(document.querySelector('[data-thread-image-lightbox]')).toBeNull()
  })
})

describe('S3: gates', () => {
  it('a server without thread-images-v1: today\'s chip, no fetch', () => {
    const { container } = renderFiles([image('hero.png', 'image/png')], { source: { scope: OFF, addr: 'sales' } })
    expect(container.querySelector('[data-thread-image]')).toBeNull()
    expect(container.querySelector('[data-thread-file="out/hero.png"]')).not.toBeNull()
    expect(h.daemonCliGet).not.toHaveBeenCalled()
  })

  it('no attachment source (a ticket, the project chat): chips only, no fetch', () => {
    const { container } = renderFiles([image('hero.png', 'image/png')], { source: null })
    expect(container.querySelector('[data-thread-image]')).toBeNull()
    expect(container.querySelector('[data-thread-file="out/hero.png"]')).not.toBeNull()
    expect(h.daemonCliGet).not.toHaveBeenCalled()
  })

  it('ThreadBubble hands its message id: the picture reads that message', async () => {
    h.daemonCliGet.mockResolvedValue(thumbAnswer())
    const item: OverlayThreadItem = {
      collection: 'thread',
      seq: 4,
      id: 'msg-77',
      doc: { id: 'msg-77', kind: 'text', from: 'sales', body: 'The new hero', attachments: [image('hero.png', 'image/png')] },
    }
    const { container } = render(
      <ThreadAttachmentSourceProvider value={{ scope: ON, addr: 'sales/reviewer' }}>
        <ThreadBubble item={item} mine={false} author="sales" timeLabel="now" viewer={null} />
      </ThreadAttachmentSourceProvider>,
    )
    await waitFor(() => expect(container.querySelector('[data-thread-image] img')).not.toBeNull())
    expect(h.daemonCliGet.mock.calls[0][2]).toEqual({ addr: 'sales/reviewer', id: 'msg-77', n: 0, mode: 'thumb', px: 512 })
  })
})
