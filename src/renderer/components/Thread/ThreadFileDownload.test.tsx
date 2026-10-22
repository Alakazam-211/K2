// @vitest-environment jsdom
// Thread file downloads (0.46.3, Rosson 2026-10-22). Fail loud.
//
//   1. The Download button shows only when the daemon said `canDownload`
//      (true) for this viewer: never without it, never for hidden or gone.
//   2. It sits on the tile's right: the last child of the chip, the list
//      row and the image card's caption.
//   3. A click saves through the daemon route that opens the file today:
//      `GET thread/attachment` with `mode=download`, the Thread's address,
//      the message id and the attachment's index `n`, on the Thread's own
//      server. It never opens the file.
//   4. Desktop Thread (open actions + source), Zen (source, no open
//      actions) and the shared bubble (`ThreadBubble`, which both mount)
//      all draw it; without an attachment source (ticket, project chat)
//      there is no button.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  downloads: [] as { label: string; first: unknown }[],
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
// The download lifecycle (transfer card, ~/Downloads) is fs-transfer's own
// and tested there; here it reads the first slice through the reader the
// Thread hands it, so the test sees the exact daemon call.
vi.mock('@/lib/fs-transfer', async (orig) => {
  const real = (await orig()) as Record<string, unknown>
  return {
    ...real,
    downloadChunks: async (label: string, read: (offset: number, len: number) => Promise<unknown>) => {
      const first = await read(0, 8)
      h.downloads.push({ label, first })
      return label
    },
  }
})

import type { JSX } from 'react'
import { cleanup, fireEvent, render, waitFor } from '@testing-library/react'
import type { ServerScope } from '@/kessel/server-scope'
import type { OverlayThreadItem, ThreadFileObject } from '@/components/SessionView/overlayThread'
import { coerceThreadFileObjects } from '@/components/SessionView/overlayThread'
import { fakeScope } from '@/test-utils/fake-scope'
import { __resetThreadThumbsForTests } from '@/lib/thread-image-thumbs'
import { threadFileDownloadable } from '@/lib/thread-attachment-download'
import { ThreadFileActionsProvider, ThreadFileObjects, type ThreadFileActions } from './ThreadFileObjects'
import { ThreadAttachmentSourceProvider, type ThreadAttachmentSource } from './ThreadImageObjects'
import { ThreadBubble } from './ThreadZenKit'

const ON = { ...fakeScope('box.example'), serverSupports: (f: string) => ['thread-attachments-v1', 'thread-images-v1'].includes(f) } as ServerScope
const SOURCE: ThreadAttachmentSource = { scope: ON, addr: 'sales' }

const CHUNK = { ok: true, base64: btoa('%PDF'), len: 4, offset: 0, size: 4, eof: true, name: 'report.pdf', mime: 'application/pdf' }

function file(name: string, extra: Partial<ThreadFileObject> = {}): ThreadFileObject {
  return { kind: 'file', state: 'ok', path: `docs/${name}`, name, size: 2048, mime: 'application/pdf', canDownload: true, ...extra }
}

function renderFiles(
  files: ThreadFileObject[],
  opts: { source?: ThreadAttachmentSource | null; actions?: ThreadFileActions } = {},
): ReturnType<typeof render> {
  const source = opts.source === undefined ? SOURCE : opts.source
  let node: JSX.Element = <ThreadFileObjects files={files} mine={false} docId="doc-9" />
  if (opts.actions) node = <ThreadFileActionsProvider value={opts.actions}>{node}</ThreadFileActionsProvider>
  return render(<ThreadAttachmentSourceProvider value={source}>{node}</ThreadAttachmentSourceProvider>)
}

const buttons = (c: HTMLElement): HTMLElement[] => [...c.querySelectorAll<HTMLElement>('[data-thread-file-download]')]

beforeEach(() => {
  __resetThreadThumbsForTests()
  h.daemonCliGet.mockReset()
  h.daemonCliGet.mockResolvedValue(CHUNK)
  h.downloads.length = 0
  let seq = 0
  ;(URL as unknown as { createObjectURL: (b: Blob) => string }).createObjectURL = () => `blob:t-${++seq}`
  ;(URL as unknown as { revokeObjectURL: (u: string) => void }).revokeObjectURL = () => undefined
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

describe('the daemon decides', () => {
  it('coerce carries canDownload only as a boolean', () => {
    const [a, b, c] = coerceThreadFileObjects([
      { kind: 'file', state: 'ok', path: 'a', name: 'a', canDownload: true },
      { kind: 'file', state: 'ok', path: 'b', name: 'b', canDownload: false },
      { kind: 'file', state: 'ok', path: 'c', name: 'c', canDownload: 'yes' },
    ])
    expect(a.canDownload).toBe(true)
    expect(b.canDownload).toBe(false)
    expect('canDownload' in c).toBe(false)
  })

  it('only canDownload: true on a visible, present file is downloadable', () => {
    expect(threadFileDownloadable(file('a.pdf'))).toBe(true)
    expect(threadFileDownloadable(file('a.pdf', { canDownload: false }))).toBe(false)
    expect(threadFileDownloadable(file('a.pdf', { canDownload: undefined }))).toBe(false)
    expect(threadFileDownloadable(file('a.pdf', { state: 'gone' }))).toBe(false)
    expect(threadFileDownloadable({ kind: 'file', state: 'hidden', canDownload: true })).toBe(false)
    expect(threadFileDownloadable(file('a.pdf', { state: 'changed' }))).toBe(true)
  })

  it('no canDownload, false, hidden or gone: no button anywhere', () => {
    const { container } = renderFiles([
      file('a.pdf', { canDownload: undefined }),
      file('b.pdf', { canDownload: false }),
    ])
    expect(buttons(container)).toHaveLength(0)
    cleanup()
    const r = renderFiles([file('c.pdf', { state: 'gone' }), { kind: 'file', state: 'hidden', canDownload: true }, file('d.pdf', { canDownload: false })])
    expect(r.container.querySelector('[data-thread-file-list]')).not.toBeNull()
    expect(buttons(r.container)).toHaveLength(0)
  })
})

describe('the button', () => {
  it('a chip: on the right, and a click saves through thread/attachment mode=download', async () => {
    const open = vi.fn()
    const { container } = renderFiles([file('a.pdf'), file('b.pdf')], { actions: { open } })
    const bs = buttons(container)
    expect(bs).toHaveLength(2)
    const tile = container.querySelector('[data-thread-file="docs/b.pdf"]') as HTMLElement
    expect(tile.lastElementChild).toBe(bs[1])
    expect(bs[1].getAttribute('aria-label')).toBe('Download b.pdf')
    fireEvent.click(bs[1])
    await waitFor(() => expect(h.downloads).toHaveLength(1))
    expect(h.daemonCliGet).toHaveBeenCalledTimes(1)
    expect(h.daemonCliGet.mock.calls[0][0]).toBe(ON)
    expect(h.daemonCliGet.mock.calls[0][1]).toBe('thread/attachment')
    expect(h.daemonCliGet.mock.calls[0][2]).toEqual({ addr: 'sales', id: 'doc-9', n: 1, mode: 'download', offset: 0, len: 8 })
    expect(h.downloads[0].label).toBe('b.pdf')
    expect(open).not.toHaveBeenCalled()
    // Opening still works beside it (the desktop Thread).
    fireEvent.click(tile.querySelector('[data-thread-file-open]') as HTMLElement)
    expect(open).toHaveBeenCalledTimes(1)
    expect(open.mock.calls[0][0].path).toBe('docs/b.pdf')
  })

  it('a list card row: on the row’s right, the row’s own index', async () => {
    const files = [file('a.pdf'), file('b.pdf', { canDownload: false }), file('c.pdf')]
    const { container } = renderFiles(files)
    expect(container.querySelector('[data-thread-file-list]')).not.toBeNull()
    const bs = buttons(container)
    expect(bs).toHaveLength(2)
    const row = container.querySelector('[data-thread-file="docs/c.pdf"]') as HTMLElement
    expect(row.lastElementChild).toBe(bs[1])
    fireEvent.click(bs[1])
    await waitFor(() => expect(h.downloads).toHaveLength(1))
    expect(h.daemonCliGet.mock.calls[0][2]).toEqual({ addr: 'sales', id: 'doc-9', n: 2, mode: 'download', offset: 0, len: 8 })
  })

  it('an image card: in the caption’s right, its index on the message (Zen: no open actions)', async () => {
    h.daemonCliGet.mockImplementation(async (_s: unknown, _r: string, q: { mode: string }) =>
      q.mode === 'thumb'
        ? { ok: true, kind: 'image', mime: 'image/png', width: 10, height: 10, base64: btoa('png'), name: 'hero.png', size: 3, state: 'ok' }
        : CHUNK,
    )
    const { container } = renderFiles([file('a.pdf'), file('hero.png', { mime: 'image/png' })])
    await waitFor(() => expect(container.querySelector('[data-thread-image] img')).not.toBeNull())
    const caption = container.querySelector('[data-thread-image-caption]') as HTMLElement
    const b = caption.querySelector('[data-thread-file-download]') as HTMLElement
    expect(b).not.toBeNull()
    expect(caption.lastElementChild).toBe(b)
    fireEvent.click(b)
    await waitFor(() => expect(h.downloads).toHaveLength(1))
    const call = h.daemonCliGet.mock.calls.find((c) => (c[2] as { mode: string }).mode === 'download')
    expect(call?.[2]).toEqual({ addr: 'sales', id: 'doc-9', n: 1, mode: 'download', offset: 0, len: 8 })
    expect(container.querySelector('[data-thread-image-lightbox], [role="dialog"]')).toBeNull()
  })

  it('without an attachment source (ticket, project chat): no button', () => {
    const { container } = renderFiles([file('a.pdf')], { source: null })
    expect(container.querySelector('[data-thread-file="docs/a.pdf"]')).not.toBeNull()
    expect(buttons(container)).toHaveLength(0)
  })

  it('the shared bubble (desktop Thread and Zen both mount it) draws it and saves its own message', async () => {
    const item: OverlayThreadItem = {
      collection: 'thread',
      seq: 4,
      id: 'msg-77',
      doc: { id: 'msg-77', kind: 'text', from: 'sales', body: 'The report', attachments: [file('report.pdf')] },
    }
    const { container } = render(
      <ThreadAttachmentSourceProvider value={{ scope: ON, addr: 'sales/reviewer' }}>
        <ThreadBubble item={item} mine={false} author="sales" timeLabel="now" viewer={null} />
      </ThreadAttachmentSourceProvider>,
    )
    const bs = buttons(container)
    expect(bs).toHaveLength(1)
    fireEvent.click(bs[0])
    await waitFor(() => expect(h.downloads).toHaveLength(1))
    expect(h.daemonCliGet.mock.calls[0][2]).toEqual({ addr: 'sales/reviewer', id: 'msg-77', n: 0, mode: 'download', offset: 0, len: 8 })
  })
})
