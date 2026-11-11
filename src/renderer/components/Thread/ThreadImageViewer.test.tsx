// @vitest-environment jsdom
// The Thread image viewer (0.46.11, Rosson: "display it in a little modal
// that has an 'open in tab' option … show the number of images sent and
// give the user the ability to look through them"). Fail loud.
//
//   1. A click on a picture opens the viewer at THAT picture; focus moves in.
//   2. Several pictures: "2 of 5" (polite), labelled previous / next
//      buttons, ←/→, a swipe; the arrows turn off at the ends.
//   3. From three pictures a strip of thumbnails (not at two); a thumbnail
//      shows its picture.
//   4. "Open in tab" closes the viewer and opens the file through the
//      surface's opener (a file tab, shown); none without an opener.
//   5. Esc closes it and focus goes back to the card that opened it.
//   6. Download saves through the attachment door (`mode=download`).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  downloads: [] as string[],
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
vi.mock('@/lib/fs-transfer', () => ({
  downloadChunks: vi.fn(async (name: string, read: (offset: number, len: number) => Promise<unknown>) => {
    await read(0, 8)
    h.downloads.push(name)
    return name
  }),
}))

import type { JSX } from 'react'
import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react'
import type { ServerScope } from '@/kessel/server-scope'
import type { ThreadFileObject } from '@/components/SessionView/overlayThread'
import { fakeScope } from '@/test-utils/fake-scope'
import { __resetThreadThumbsForTests } from '@/lib/thread-image-thumbs'
import { openThreadAttachment } from '@/lib/thread-file-link-open'
import { ThreadFileActionsProvider, ThreadFileObjects, type ThreadFileActions } from './ThreadFileObjects'
import { ThreadAttachmentSourceProvider, THREAD_VIEWER_STRIP_AT } from './ThreadImageObjects'

const ON = { ...fakeScope('box.example'), serverSupports: (f: string) => ['thread-attachments-v1', 'thread-images-v1'].includes(f) } as ServerScope

let urlSeq = 0
beforeEach(() => {
  __resetThreadThumbsForTests()
  urlSeq = 0
  h.downloads.length = 0
  h.daemonCliGet.mockReset()
  // Thumbs and previews answer the same shape; the URL names which one.
  h.daemonCliGet.mockImplementation(async (_s: unknown, _r: string, p: { mode: string; n: number }) =>
    p.mode === 'download'
      ? { ok: true, offset: 0, len: 3, eof: true, size: 3, base64: btoa('png'), name: 'x.png', mime: 'image/png' }
      : { ok: true, kind: 'image', mime: 'image/png', width: 800, height: 600, base64: btoa(`${p.mode}-${p.n}`) },
  )
  ;(URL as unknown as { createObjectURL: (b: Blob) => string }).createObjectURL = () => `blob:img-${++urlSeq}`
  ;(URL as unknown as { revokeObjectURL: (u: string) => void }).revokeObjectURL = () => undefined
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

function image(name: string, extra: Partial<ThreadFileObject> = {}): ThreadFileObject {
  return { kind: 'file', state: 'ok', path: `shots/${name}`, name, size: 2048, mime: 'image/png', modifiedAt: 1, ...extra }
}

const FIVE = ['one.png', 'two.png', 'three.png', 'four.png', 'five.png'].map((n) => image(n))

function renderFiles(files: ThreadFileObject[], actions?: ThreadFileActions): ReturnType<typeof render> {
  let node: JSX.Element = <ThreadFileObjects files={files} mine={false} docId="doc-5" />
  if (actions) node = <ThreadFileActionsProvider value={actions}>{node}</ThreadFileActionsProvider>
  return render(<ThreadAttachmentSourceProvider value={{ scope: ON, addr: 'design' }}>{node}</ThreadAttachmentSourceProvider>)
}

async function openAt(files: ThreadFileObject[], index: number, actions?: ThreadFileActions): Promise<{ card: HTMLElement; viewer: HTMLElement }> {
  const { container } = renderFiles(files, actions)
  await waitFor(() => expect(container.querySelectorAll('[data-thread-image] img')).toHaveLength(files.length))
  const card = container.querySelectorAll<HTMLElement>('[data-thread-image-open]')[index]
  fireEvent.click(card)
  const viewer = document.querySelector('[data-thread-image-viewer]') as HTMLElement
  expect(viewer).not.toBeNull()
  return { card, viewer }
}

const q = (sel: string): HTMLElement | null => document.querySelector(sel)
const caption = (): string | null | undefined => q('[data-thread-image-viewer-caption]')?.textContent
const count = (): string | null | undefined => q('[data-thread-image-viewer-count]')?.textContent

describe('the Thread image viewer', () => {
  it('one picture: the viewer with its name, no count, arrows or strip; focus moves in', async () => {
    await openAt([image('hero.png')], 0)
    const dialog = q('[role="dialog"]') as HTMLElement
    expect(dialog.getAttribute('aria-modal')).toBe('true')
    expect(document.getElementById(dialog.getAttribute('aria-labelledby') ?? '')?.textContent).toBe('hero.png')
    expect(document.activeElement).toBe(dialog)
    expect(q('[data-thread-image-viewer-picture]')?.getAttribute('alt')).toBe('hero.png')
    expect(q('[data-thread-image-viewer-count]')).toBeNull()
    expect(q('[data-thread-image-viewer-prev]')).toBeNull()
    expect(q('[data-thread-image-viewer-strip]')).toBeNull()
    // The shared × of every dialog.
    expect(q('[data-thread-image-viewer] [data-k2-dialog-close]')?.getAttribute('aria-label')).toBe('Close (Esc)')
  })

  it('five pictures: starts at the one clicked, "2 of 5" announced politely', async () => {
    await openAt(FIVE, 1)
    expect(caption()).toBe('two.png')
    expect(count()).toBe('2 of 5')
    expect(q('[data-thread-image-viewer-count]')?.getAttribute('aria-live')).toBe('polite')
    // It reads the whole picture of the one clicked (n = 1).
    await waitFor(() => expect(q('[data-thread-image-viewer-picture]')?.hasAttribute('data-full')).toBe(true))
    const previews = h.daemonCliGet.mock.calls.filter((c) => (c[2] as { mode: string }).mode === 'preview')
    expect(previews.map((c) => c[2])).toEqual([{ addr: 'design', id: 'doc-5', n: 1, mode: 'preview' }])
  })

  it('previous and next are labelled buttons; they turn off at the ends', async () => {
    await openAt(FIVE, 0)
    const prev = q('[data-thread-image-viewer-prev]') as HTMLButtonElement
    const next = q('[data-thread-image-viewer-next]') as HTMLButtonElement
    expect(prev.getAttribute('aria-label')).toBe('Previous image')
    expect(next.getAttribute('aria-label')).toBe('Next image')
    expect(prev.disabled).toBe(true)
    fireEvent.click(next)
    expect(count()).toBe('2 of 5')
    expect(caption()).toBe('two.png')
    fireEvent.click(prev)
    expect(count()).toBe('1 of 5')
    for (let i = 0; i < 6; i++) fireEvent.click(next)
    expect(count()).toBe('5 of 5')
    expect(next.disabled).toBe(true)
  })

  it('← and → step through; not with a modifier', async () => {
    await openAt(FIVE, 2)
    expect(count()).toBe('3 of 5')
    fireEvent.keyDown(window, { key: 'ArrowRight' })
    expect(count()).toBe('4 of 5')
    fireEvent.keyDown(window, { key: 'ArrowLeft' })
    fireEvent.keyDown(window, { key: 'ArrowLeft' })
    expect(count()).toBe('2 of 5')
    fireEvent.keyDown(window, { key: 'ArrowLeft', metaKey: true })
    expect(count()).toBe('2 of 5')
  })

  it('a swipe on touch turns the picture; a short or vertical one does not', async () => {
    await openAt(FIVE, 2)
    const stage = q('[data-thread-image-viewer-stage]') as HTMLElement
    const swipe = (x0: number, x1: number, y1 = 100): void => {
      fireEvent.touchStart(stage, { touches: [{ clientX: x0, clientY: 100 }] })
      fireEvent.touchEnd(stage, { changedTouches: [{ clientX: x1, clientY: y1 }] })
    }
    swipe(300, 200)
    expect(count()).toBe('4 of 5')
    swipe(200, 300)
    expect(count()).toBe('3 of 5')
    swipe(300, 280)
    swipe(300, 240, 300)
    expect(count()).toBe('3 of 5')
  })

  it(`a strip of thumbnails from ${THREAD_VIEWER_STRIP_AT} pictures; a thumbnail shows its picture`, async () => {
    await openAt(FIVE, 0)
    const thumbs = document.querySelectorAll<HTMLElement>('[data-thread-image-viewer-thumb]')
    expect(thumbs).toHaveLength(5)
    expect(thumbs[0].getAttribute('aria-current')).toBe('true')
    expect(thumbs[3].getAttribute('aria-label')).toBe('Image 4 of 5: four.png')
    // The strip draws the cards' cached thumbnails: no new thumbnail reads.
    expect(h.daemonCliGet.mock.calls.filter((c) => (c[2] as { mode: string }).mode === 'thumb')).toHaveLength(5)
    fireEvent.click(thumbs[3])
    expect(count()).toBe('4 of 5')
    expect(caption()).toBe('four.png')
    expect(thumbs[3].getAttribute('aria-current')).toBe('true')
    expect(thumbs[0].hasAttribute('aria-current')).toBe(false)
  })

  it('two pictures: arrows, no strip', async () => {
    await openAt(FIVE.slice(0, 2), 1)
    expect(count()).toBe('2 of 2')
    expect(q('[data-thread-image-viewer-next]')).not.toBeNull()
    expect(q('[data-thread-image-viewer-strip]')).toBeNull()
  })

  it('"Open in tab" closes the viewer and opens the picture shown, through the surface\'s opener', async () => {
    const open = vi.fn()
    const { card } = await openAt(FIVE, 1, { open })
    fireEvent.keyDown(window, { key: 'ArrowRight' })
    const button = q('[data-thread-image-viewer-open-tab]') as HTMLElement
    expect(button.textContent).toBe('Open in tab')
    expect(button.getAttribute('aria-label')).toBe('Open three.png in a tab')
    fireEvent.click(button)
    expect(q('[data-thread-image-viewer]')).toBeNull()
    expect(open).toHaveBeenCalledTimes(1)
    // A plain open: the tab is shown (not the background open).
    expect(open.mock.calls[0]).toEqual([FIVE[2]])
    // Focus is the tab's, not handed back to the card.
    await new Promise((r) => setTimeout(r, 5))
    expect(document.activeElement).not.toBe(card)
  })

  it('"Open in tab" is the Files drawer\'s opener: a file tab of that path, shown (an open one is found, not duplicated)', async () => {
    const openFileAsTab = vi.fn(() => 'existing' as const)
    const room = { scope: ON, isPrimary: false, tabs: { getState: () => ({ openFileAsTab }) } } as never
    const transport = vi.fn(async (_ws: string, paths: string[]) => ({
      links: paths.map((p) => ({ text: p, linkable: true, path: `/ws/${p}`, kind: 'file' as const })),
    }))
    const actions: ThreadFileActions = { open: (file, opts) => void openThreadAttachment(room, transport as never, 'design', file, opts) }
    await openAt(FIVE, 4, actions)
    fireEvent.click(q('[data-thread-image-viewer-open-tab]') as HTMLElement)
    await waitFor(() => expect(openFileAsTab).toHaveBeenCalledWith('/ws/shots/five.png'))
    expect(transport).toHaveBeenCalledWith('design', ['shots/five.png'])
  })

  it('no file opener on this surface: no "Open in tab"', async () => {
    await openAt(FIVE, 0)
    expect(q('[data-thread-image-viewer-open-tab]')).toBeNull()
  })

  it('Esc closes it and focus goes back to the card it opened from', async () => {
    const { card } = await openAt(FIVE, 3)
    fireEvent.keyDown(q('[role="dialog"]') as HTMLElement, { key: 'Escape' })
    expect(q('[data-thread-image-viewer]')).toBeNull()
    await waitFor(() => expect(document.activeElement).toBe(card))
  })

  it('the × closes it too, focus back on the card', async () => {
    const { card } = await openAt([image('hero.png')], 0)
    fireEvent.click(q('[data-thread-image-viewer] [data-k2-dialog-close]') as HTMLElement)
    expect(q('[data-thread-image-viewer]')).toBeNull()
    await waitFor(() => expect(document.activeElement).toBe(card))
  })

  it('Download saves the picture shown through the attachment door (only when the daemon allows it)', async () => {
    const files = [image('a.png', { canDownload: true }), image('b.png', { canDownload: false })]
    await openAt(files, 0)
    const save = q('[data-thread-image-viewer-download]') as HTMLElement
    expect(save.getAttribute('aria-label')).toBe('Download a.png')
    await act(async () => {
      fireEvent.click(save)
    })
    await waitFor(() => expect(h.downloads).toEqual(['a.png']))
    const call = h.daemonCliGet.mock.calls.find((c) => (c[2] as { mode: string }).mode === 'download')
    expect(call?.[2]).toEqual({ addr: 'design', id: 'doc-5', n: 0, mode: 'download', offset: 0, len: 8 })
    // Still open; the next picture may not be saved.
    fireEvent.keyDown(window, { key: 'ArrowRight' })
    expect(q('[data-thread-image-viewer-download]')).toBeNull()
  })

  it('a click in the viewer does not reach the message under it', async () => {
    const bubbleClick = vi.fn()
    const { container } = render(
      <div onClick={bubbleClick}>
        <ThreadAttachmentSourceProvider value={{ scope: ON, addr: 'design' }}>
          <ThreadFileObjects files={FIVE} mine={false} docId="doc-5" />
        </ThreadAttachmentSourceProvider>
      </div>,
    )
    await waitFor(() => expect(container.querySelectorAll('[data-thread-image] img')).toHaveLength(5))
    fireEvent.click(container.querySelector('[data-thread-image-open]') as HTMLElement)
    bubbleClick.mockClear()
    fireEvent.click(q('[data-thread-image-viewer-next]') as HTMLElement)
    expect(count()).toBe('2 of 5')
    expect(bubbleClick).not.toHaveBeenCalled()
  })
})
