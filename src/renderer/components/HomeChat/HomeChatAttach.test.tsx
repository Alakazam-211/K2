// @vitest-environment jsdom
// Files in Home DMs and channels (contract-home-chat-attachments-v1). Fail loud.
//
//   1. The "+" is the agent chat's own (`ComposeAttachButton`): its glyph,
//      its label, its menu (Attach a file or image, Save to stash).
//   2. A pasted image is a chip with its picture and its upload's progress;
//      the bytes go to `uploads/data` from `offset`; Send waits for them and
//      then posts `attachments` (no text needed); the chips clear.
//   3. A file over the server's limit is refused before any upload.
//   4. A message's files draw as the Thread's file objects (a chip with
//      Download; a picture card), and `attachments_updated` replaces them.
//   5. A server without `home-chat-attachments-v1`: the attach row is off,
//      a paste is text, no upload.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))

const h = vi.hoisted(() => ({
  get: vi.fn(async (_s: unknown, _r: string, _p?: Record<string, unknown>): Promise<unknown> => ({})),
  post: vi.fn(async (_s: unknown, _r: string, _b?: unknown): Promise<unknown> => ({ ok: true })),
  bytes: vi.fn(async (_s: unknown, _r: string, _p: Record<string, unknown>, _b: Uint8Array): Promise<unknown> => ({})),
  blob: vi.fn(async (_s: unknown, _r: string, _p?: Record<string, unknown>): Promise<Blob> => new Blob([])),
  frames: null as null | ((f: unknown) => void),
  maxBytes: null as number | null,
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (s: unknown, r: string, p?: Record<string, unknown>) => h.get(s, r, p),
  daemonCliPost: (s: unknown, r: string, b?: unknown) => h.post(s, r, b),
  daemonCliPostBytes: (s: unknown, r: string, p: Record<string, unknown>, b: Uint8Array) => h.bytes(s, r, p, b),
  daemonCliGetBlob: (s: unknown, r: string, p?: Record<string, unknown>) => h.blob(s, r, p),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
vi.mock('@/lib/home-chat/events-socket', async (orig) => ({
  ...(await orig<typeof import('@/lib/home-chat/events-socket')>()),
  subscribeHomeChatEvents: (_scope: unknown, fn: (f: unknown) => void) => {
    h.frames = fn
    return () => {
      h.frames = null
    }
  },
}))

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import HomeChatView from './HomeChatView'
import { resetHomeChatForTests } from '@/stores/home-chat'
import { resetHomeChatViewCacheForTests } from '@/lib/home-chat/chat'
import { noteServerVersion } from '@/kessel/server-scope'
import { useContextMenuStore } from '@/stores/context-menu'
import type { HomeChatTarget } from '@/lib/home-chat/address'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const CHANNEL: HomeChatTarget = { kind: 'channel', hostKey: 'local', conv: '600', address: '600::local' }
const ME = '1'
const ALEX = '100'

function m(id: string, author: string, body: string, extra: Record<string, unknown> = {}): Record<string, unknown> {
  return { id, channel_id: '600', author_id: author, body, seq: Number(id), created_at: '2026-11-23T10:00:00Z', reactions: [], ...extra }
}

const PDF = { id: '7001', filename: 'plan.pdf', content_type: 'application/pdf', size: 2048, kind: 'document', url: '/cli/home-chat/file?id=7001' }
const CAT = {
  id: '7002', filename: 'cat.png', content_type: 'image/png', size: 900, kind: 'image', width: 400, height: 300,
  url: '/cli/home-chat/file?id=7002', thumbnail_url: '/cli/home-chat/file?id=7002&v=thumb',
}
const NEWEST = [m('20', ALEX, 'have a look', { attachments: [PDF] }), m('21', ME, 'mine')]

function setFiles(on: boolean): void {
  noteServerVersion('local', '0.46.18', on ? ['home-chat-v1', 'home-chat-attachments-v1'] : ['home-chat-v1'])
}

beforeEach(() => {
  // jsdom draws no blob: URLs.
  let n = 0
  URL.createObjectURL = vi.fn(() => `blob:test-${++n}`)
  URL.revokeObjectURL = vi.fn()
  resetHomeChatForTests()
  resetHomeChatViewCacheForTests()
  setFiles(true)
  h.maxBytes = null
  h.get.mockReset()
  h.post.mockReset()
  h.bytes.mockReset()
  h.blob.mockReset()
  h.get.mockImplementation(async (_s, route, p) => {
    switch (route) {
      case 'home-chat/status':
        return { ok: true, on: true, reason: null, flags: {}, files: { maxBytes: h.maxBytes, perMessage: 10, chunkBytes: 1048576, maxRequestBytes: 8388608 } }
      case 'home-chat/summary':
        return { ok: true, chat: 'live', convs: [] }
      case 'home-chat/people':
        return {
          ok: true,
          me: { ext: 'k2:connect:me', memberId: ME },
          people: [{ ext: 'k2:connect:a', username: 'alex', displayName: 'Alex', ready: true, memberId: ALEX }],
        }
      case 'home-chat/channels/members':
        return { ok: true, conv: '600', slug: 'c-0123456789', state: 'active', name: 'design', open: false, creator: 'k2:connect:me', people: [], agents: [] }
      case 'home-chat/messages':
        return { ok: true, conv: '600', page: p?.before ? { items: [], has_more: false } : { items: NEWEST, has_more: false } }
      default:
        throw new Error(`unexpected GET ${route}`)
    }
  })
  h.post.mockImplementation(async (_s, route, b) => {
    if (route === 'home-chat/uploads/create') {
      const body = b as { size: number }
      return { ok: true, upload: { id: '5001', conv: '600', committed: 0, size: body.size, complete: false, chunkBytes: 1048576, maxRequestBytes: 8388608 } }
    }
    if (route === 'home-chat/send') {
      const body = b as { text: string }
      return { ok: true, message: m('30', ME, body.text, { attachments: [CAT] }) }
    }
    return { ok: true }
  })
  h.blob.mockImplementation(async () => new Blob([new Uint8Array([137, 80, 78, 71])], { type: 'image/png' }))
})

afterEach(() => {
  cleanup()
  resetHomeChatForTests()
})

async function open(): Promise<void> {
  render(<HomeChatView target={CHANNEL} />)
  await screen.findByText('mine')
}

function plus(): HTMLElement {
  return screen.getByTestId('home-chat-plus')
}

async function paste(files: File[]): Promise<void> {
  const input = screen.getByTestId('home-chat-input')
  await act(async () => {
    fireEvent.paste(input, { clipboardData: { files, items: [], getData: () => '' } })
  })
}

function chipNames(): string[] {
  return Array.from(document.querySelectorAll('[data-testid="home-chat-compose-files"] [data-zen-attachment]')).map(
    (el) => el.getAttribute('data-zen-attachment') ?? '',
  )
}

describe('Home chat attachments', () => {
  it("the + is the agent chat's: its glyph, its label and its menu", async () => {
    await open()
    const b = plus()
    expect(b.getAttribute('data-compose-attach')).toBe('plus')
    expect(b.getAttribute('aria-label')).toBe('Attach or save')
    expect(b.querySelector('[data-compose-glyph="plus"]')).not.toBeNull()
    await act(async () => {
      fireEvent.click(b)
    })
    const items = useContextMenuStore.getState().items
    expect(items.map((i) => i.id)).toEqual(['attach', 'sep', 'stash'])
    expect(items[0].label).toBe('Attach a file or image')
    expect(items[0].enabled).not.toBe(false)
    await act(async () => {
      useContextMenuStore.getState().selectItem('sep')
    })
  })

  it('a pasted image uploads with progress, then Send posts its upload id and clears the chips', async () => {
    let finish: (v: unknown) => void = () => undefined
    h.bytes.mockImplementation(() => new Promise((r) => (finish = r)))
    await open()
    const cat = new File([new Uint8Array(900)], 'cat.png', { type: 'image/png' })
    await paste([cat])
    await waitFor(() => expect(chipNames()).toEqual(['cat.png']))
    // The image chip draws its picture, and shows how far its upload is.
    expect(document.querySelector('[data-testid="home-chat-compose-files"] [data-compose-chip-thumb]')).not.toBeNull()
    await waitFor(() => expect(document.querySelector('[data-zen-attachment-progress]')?.textContent).toBe('0%'))
    const create = h.post.mock.calls.find((c) => c[1] === 'home-chat/uploads/create')
    expect(create?.[2]).toEqual({ conv: '600', filename: 'cat.png', size: 900, contentType: 'image/png' })
    await waitFor(() => expect(h.bytes).toHaveBeenCalledTimes(1))
    const [, route, params, body] = h.bytes.mock.calls[0]
    expect(route).toBe('home-chat/uploads/data')
    expect(params).toEqual({ id: '5001', offset: 0 })
    expect(body.byteLength).toBe(900)
    // Send waits while the file is on its way.
    const send = screen.getByTestId('home-chat-send')
    expect(send.hasAttribute('disabled')).toBe(true)
    await act(async () => {
      finish({ ok: true, upload: { id: '5001', committed: 900, size: 900, complete: true } })
    })
    await waitFor(() => expect(document.querySelector('[data-zen-attachment-progress]')).toBeNull())
    expect(send.hasAttribute('disabled')).toBe(false)
    await act(async () => {
      fireEvent.click(send)
    })
    const sent = h.post.mock.calls.find((c) => c[1] === 'home-chat/send')
    expect(sent?.[2]).toMatchObject({ conv: '600', text: '', attachments: ['5001'] })
    await waitFor(() => expect(chipNames()).toEqual([]))
    // The sent message draws its picture card through the Home chat door.
    await waitFor(() => expect(h.blob.mock.calls.some((c) => c[1] === 'home-chat/file' && c[2]?.id === '7002' && c[2]?.v === 'thumb')).toBe(true))
  })

  it('a file over the server limit is refused before any upload', async () => {
    h.maxBytes = 100
    await open()
    await waitFor(() => expect(h.get.mock.calls.some((c) => c[1] === 'home-chat/status')).toBe(true))
    await paste([new File([new Uint8Array(500)], 'big.bin', { type: 'application/octet-stream' })])
    expect((await screen.findByTestId('home-chat-file-error')).textContent).toMatch(/^big\.bin is larger than this server takes/)
    expect(chipNames()).toEqual([])
    expect(h.post.mock.calls.some((c) => c[1] === 'home-chat/uploads/create')).toBe(false)
  })

  it("a message's files are the Thread's file objects, and attachments_updated replaces them", async () => {
    await open()
    const row = document.querySelector('[data-zen-message="20"]') as HTMLElement
    expect(row.querySelector('[data-thread-file="plan.pdf"]')).not.toBeNull()
    expect(row.querySelector('[aria-label="Download plan.pdf"]')).not.toBeNull()
    await act(async () => {
      h.frames?.({ type: 'attachments_updated', conv: '600', msg: '20', attachments: [PDF, CAT] })
    })
    await waitFor(() => expect(h.blob.mock.calls.some((c) => c[2]?.id === '7002' && c[2]?.v === 'thumb')).toBe(true))
  })

  it('without home-chat-attachments-v1: the attach row is off and a paste uploads nothing', async () => {
    setFiles(false)
    await open()
    await act(async () => {
      fireEvent.click(plus())
    })
    const attach = useContextMenuStore.getState().items.find((i) => i.id === 'attach')
    expect(attach?.enabled).toBe(false)
    await act(async () => {
      useContextMenuStore.getState().selectItem('sep')
    })
    await paste([new File([new Uint8Array(4)], 'x.png', { type: 'image/png' })])
    expect(chipNames()).toEqual([])
    expect(h.post.mock.calls.some((c) => c[1] === 'home-chat/uploads/create')).toBe(false)
  })
})
