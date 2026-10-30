// @vitest-environment jsdom
// Ticket attachments in the rail (0.46.6, prd-ticket-attachments-v1
// TK18–TK22). Fail loud.
//
//   1. Each message draws its files as the Thread does, keyed by the
//      COMMENT id (TA12): pictures read through the ticket door
//      `feedback/attachment {id, commentId, n}`; chips for the rest.
//   2. Without `ticket-attachments-v1`: chips only, nothing read, no "+".
//   3. The reader is the source's (TA13): a ticket picture and a Thread
//      picture with the same message id and index never share a cache key.
//   4. The message box: "+" picks, a paste or a drop attaches; chips above
//      the box (× removes, at most 10); they survive a remount (the draft);
//      Send posts them (text optional with files), an option pick too; a
//      refusal reads as words; off with the reason when the ticket is
//      unlinked or the server lacks the feature.
//   5. Download: a file someone else attached saves quarantined (TK22),
//      a Thread's too; your own doesn't.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  gets: [] as { path: string; params: Record<string, unknown> }[],
  posts: [] as { path: string; body: unknown }[],
  postImpl: null as null | ((path: string, body: unknown) => Promise<unknown>),
  picked: [] as string[],
  downloads: [] as { label: string; opts: unknown; first: unknown }[],
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'window-test' }) }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: async (_scope: unknown, path: string, params: Record<string, unknown> = {}) => {
    h.gets.push({ path, params })
    if (path === 'sessions/list-for-workspace') return []
    if (path === 'feedback/attachment' || path === 'thread/attachment') {
      if (params.mode === 'thumb' || params.mode === 'preview') {
        return { ok: true, kind: 'image', mime: 'image/png', base64: btoa('png-bytes'), width: 64, height: 32 }
      }
      return { ok: true, base64: btoa('%PDF'), len: 4, offset: 0, size: 4, eof: true, name: 'x', mime: 'application/pdf' }
    }
    return {}
  },
  daemonCliPost: async (_scope: unknown, path: string, body?: unknown) => {
    h.posts.push({ path, body })
    if (h.postImpl) return h.postImpl(path, body)
    return { ok: true, id: 'T', attachments: [] }
  },
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
vi.mock('@/kessel-term/TerminalPane', () => ({ TerminalPane: () => <div data-testid="stub-terminal-pane" /> }))
vi.mock('@/lib/pick-compose-files', () => ({
  composeAttachPlan: () => ({ kind: 'native' }),
  pickLocalComposeFiles: async () => [...h.picked],
  pickRemoteComposeFile: async () => null,
}))
// Uploads (a file outside the workspace, a pasted screenshot) land in the
// workspace's `.k2/downloads/`, as the real upload path does.
vi.mock('@/lib/handle-remote-drop', () => ({
  executeRemoteDrop: async (_s: unknown, paths: string[], _t: unknown, _c: unknown, build: (r: string[]) => string) =>
    build(paths.map((p) => `/ws/.k2/downloads/${p.split('/').pop()}`)),
  executeBrowserFileDrop: async (_s: unknown, files: File[], _t: unknown, _c: unknown, build: (r: string[]) => string) =>
    build(files.map((f) => `/ws/.k2/downloads/${f.name}`)),
}))
vi.mock('@/lib/fs-transfer', async (orig) => {
  const real = (await orig()) as Record<string, unknown>
  return {
    ...real,
    downloadChunks: async (label: string, read: (offset: number, len: number) => Promise<unknown>, opts?: unknown) => {
      const first = await read(0, 8)
      h.downloads.push({ label, opts: opts ?? {}, first })
      return label
    },
  }
})

import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react'
import { TicketAgentRail } from './TicketAgentRail'
import type { FeedbackShow, QuickAnswerOption } from './feedback-api'
import { fetchFeedbackShow } from './feedback-api'
import { primaryScope, type ServerScope } from '@/kessel/server-scope'
import { fakeScope } from '@/test-utils/fake-scope'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { getTicketDraftFiles, setTicketDraftFiles } from '@/lib/composer-drafts'
import {
  __resetThreadThumbsForTests,
  threadThumbKey,
  ticketAttachmentReader,
} from '@/lib/thread-image-thumbs'
import { ThreadFileObjects } from '@/components/Thread/ThreadFileObjects'
import {
  ThreadAttachmentSourceProvider,
  attachmentImageRef,
  ticketAttachmentSource,
} from '@/components/Thread/ThreadImageObjects'
import type { ThreadFileObject } from '@/components/SessionView/overlayThread'
import { FileCountBadge } from './badges'
import { useContextMenuStore } from '@/stores/context-menu'
import { usePromptStashStore, type StashSaveResult } from '@/stores/prompt-stash'
import { isMacPlatform } from '@/lib/desktop-chrome'

/** The box's "+" opens its menu (attach, then Save to stash). */
async function openPlus(c: HTMLElement): Promise<void> {
  const plus = c.querySelector<HTMLButtonElement>('[data-testid="ticket-compose-plus"]')
  if (!plus) throw new Error('no "+" in the ticket box')
  await act(async () => {
    fireEvent.click(plus)
  })
  if (!useContextMenuStore.getState().isOpen) throw new Error('"+" did not open its menu')
}

function plusRow(id: string) {
  const r = useContextMenuStore.getState().items.find((i) => i.id === id)
  if (!r) throw new Error(`the "+" menu has no ${id} row`)
  return r
}

async function pickAttach(c: HTMLElement): Promise<void> {
  await openPlus(c)
  expect(plusRow('attach').enabled).not.toBe(false)
  await act(async () => {
    useContextMenuStore.getState().selectItem('attach')
  })
}

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const NOW = 1_759_241_200
const TICKET = 'bbbbbbbb-0000-4000-8000-000000000002'
const SEED = 'c0000000-0000-4000-8000-0000000000a1'
const REPLY = 'c0000000-0000-4000-8000-0000000000a2'

const png = (name: string, extra: Partial<ThreadFileObject> = {}): ThreadFileObject => ({
  kind: 'file',
  state: 'ok',
  path: `out/${name}`,
  name,
  size: 2048,
  mime: 'image/png',
  modifiedAt: 1000,
  canDownload: true,
  ...extra,
})
const pdf: ThreadFileObject = { kind: 'file', state: 'ok', path: 'docs/r.pdf', name: 'r.pdf', size: 10, mime: 'application/pdf', canDownload: true }

function item(over: Partial<FeedbackShow> = {}): FeedbackShow {
  return {
    id: TICKET,
    projectId: 'p1',
    sessionId: null,
    sessionKind: null,
    agentName: 'scout',
    kind: 'question',
    title: 'Which logo?',
    body: null,
    options: ['A', 'B'],
    priority: 3,
    status: 'waiting',
    answer: null,
    createdAt: NOW - 120,
    updatedAt: NOW - 120,
    answeredAt: null,
    commentCount: 2,
    assignees: ['owner'],
    workspace: 'Alpha',
    projectPath: '/ws',
    canonicalSessionId: null,
    comments: [
      { id: SEED, author: 'scout', body: 'Which logo?', at: NOW - 120, attachments: [png('a.png'), pdf] },
      { id: REPLY, author: 'owner', body: 'thinking', at: NOW - 60 },
    ],
    ...over,
  }
}

const OPTIONS: QuickAnswerOption[] = [
  { answer: 'A', label: 'A', detail: 'A' },
  { answer: 'B', label: 'B', detail: 'B' },
]

let features = new Set<string>(['ticket-attachments-v1', 'thread-images-v1'])

function rail(over: Partial<FeedbackShow> = {}, projectPath: string | null = '/ws'): HTMLElement {
  const it = item({ projectPath, ...over })
  return render(
    <TicketAgentRail
      feedbackId={TICKET}
      item={it}
      error={null}
      nowSec={NOW}
      options={OPTIONS}
      optionsLive
      onChanged={() => {}}
      projectId="p1"
      projectPath={projectPath}
      sessionId={null}
      sessionKind={null}
      canonicalSessionId={null}
    />,
  ).container
}

const commentPosts = (): { path: string; body: Record<string, unknown> }[] =>
  h.posts.filter((p) => p.path === 'feedback/comment') as { path: string; body: Record<string, unknown> }[]

beforeEach(() => {
  features = new Set(['ticket-attachments-v1', 'thread-images-v1'])
  vi.spyOn(primaryScope(), 'serverSupports').mockImplementation((f) => features.has(f))
  __resetThreadThumbsForTests()
  h.gets.length = 0
  h.posts.length = 0
  h.postImpl = null
  h.picked = []
  h.downloads.length = 0
  setTicketDraftFiles(TICKET, [])
  let seq = 0
  ;(URL as unknown as { createObjectURL: (b: Blob) => string }).createObjectURL = () => `blob:t-${++seq}`
  ;(URL as unknown as { revokeObjectURL: (u: string) => void }).revokeObjectURL = () => undefined
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

describe('files under each message', () => {
  it('draw keyed by the comment id, pictures read through the ticket door', async () => {
    const c = rail()
    const seedRow = c.querySelector(`[data-zen-message="${SEED}"]`)
    expect(seedRow, 'the row is keyed by the comment id').not.toBeNull()
    expect(c.querySelector(`[data-zen-message="${TICKET}:0"]`)).toBeNull()
    await waitFor(() => expect(seedRow!.querySelector('[data-thread-image="out/a.png"] img')).not.toBeNull())
    expect(seedRow!.querySelector('[data-thread-file-chip], [data-thread-files]')).not.toBeNull()
    const thumbs = h.gets.filter((g) => g.path === 'feedback/attachment')
    expect(thumbs).toHaveLength(1)
    expect(thumbs[0].params).toEqual({ id: TICKET, commentId: SEED, n: 0, mode: 'thumb', px: 512 })
    expect(h.gets.some((g) => g.path === 'thread/attachment')).toBe(false)
    expect(seedRow!.textContent).toContain('r.pdf')
  })

  it('without the feature: chips only, nothing read, no "+"', async () => {
    features = new Set(['thread-images-v1'])
    const c = rail()
    const seedRow = c.querySelector(`[data-zen-message="${SEED}"]`)!
    expect(seedRow.querySelector('[data-thread-image]')).toBeNull()
    expect(seedRow.textContent).toContain('a.png')
    expect(seedRow.querySelector('[data-thread-file-download]'), 'no door, no Download').toBeNull()
    await new Promise((r) => setTimeout(r, 20))
    expect(h.gets.filter((g) => g.path === 'feedback/attachment')).toHaveLength(0)
    // The "+" offers only Save to stash: no attach choice.
    await openPlus(c)
    expect(useContextMenuStore.getState().items.map((i) => i.id)).toEqual(['stash'])
    act(() => useContextMenuStore.getState().close())
  })

  it('show coerces each comment\'s files (unknown shapes never reach a bubble)', async () => {
    const mod = await import('@/lib/daemon-cli')
    const spy = vi.spyOn(mod, 'daemonCliGet').mockResolvedValueOnce({
      ...item(),
      comments: [
        { id: SEED, author: 'scout', body: 'x', at: 1, attachments: [{ kind: 'file', state: 'ok', path: 'a', name: 'a', bogus: 1 }, 'junk'] },
        { id: REPLY, author: 'owner', body: 'y', at: 2, attachments: 'nope' },
      ],
    } as never)
    const got = await fetchFeedbackShow(TICKET)
    expect(spy).toHaveBeenCalled()
    expect(got.comments[0].attachments).toEqual([{ kind: 'file', state: 'ok', path: 'a', name: 'a' }])
    expect('attachments' in got.comments[1]).toBe(false)
  })
})

describe('the reader is the source\'s', () => {
  it('ticket reads go to feedback/attachment with commentId', async () => {
    const scope = fakeScope('box.example')
    await ticketAttachmentReader(scope, TICKET)(REPLY, 2, { mode: 'meta' })
    expect(h.gets.at(-1)).toEqual({ path: 'feedback/attachment', params: { id: TICKET, commentId: REPLY, n: 2, mode: 'meta' } })
  })

  it('a ticket picture and a Thread picture never share a cache key', () => {
    const scope = fakeScope('box.example')
    const ticket = attachmentImageRef(ticketAttachmentSource(scope, TICKET), 'doc-1', 0, 5)
    const thread = attachmentImageRef({ scope, addr: `ticket:${TICKET}` }, 'doc-1', 0, 5)
    const plainThread = attachmentImageRef({ scope, addr: 'sales' }, 'doc-1', 0, 5)
    expect(threadThumbKey(ticket)).not.toBe(threadThumbKey(thread))
    expect(threadThumbKey(ticket)).not.toBe(threadThumbKey(plainThread))
    expect(threadThumbKey(attachmentImageRef(ticketAttachmentSource(scope, 'other'), 'doc-1', 0, 5))).not.toBe(threadThumbKey(ticket))
  })
})

describe('Save to stash in the ticket box', () => {
  const realSave = usePromptStashStore.getState().save
  afterEach(() => {
    usePromptStashStore.setState({ save: realSave })
  })

  it('"+" has Save to stash (dimmed when empty); picking it saves, then clears; Cmd+S too', async () => {
    const save = vi.fn(async (): Promise<StashSaveResult> => ({ status: 'saved' }) as StashSaveResult)
    usePromptStashStore.setState({ save })
    const c = rail()
    await openPlus(c)
    expect(plusRow('stash').enabled).toBe(false)
    act(() => useContextMenuStore.getState().close())
    const input = c.querySelector<HTMLTextAreaElement>('[data-testid="ticket-compose-input"]')!
    fireEvent.change(input, { target: { value: 'ask about the build' } })
    await openPlus(c)
    expect(plusRow('stash').enabled).toBe(true)
    await act(async () => {
      useContextMenuStore.getState().selectItem('stash')
    })
    await waitFor(() => expect(input.value).toBe(''))
    expect(save).toHaveBeenCalledTimes(1)
    expect((save.mock.calls[0] as unknown[])[0]).toMatchObject({ text: 'ask about the build', source: { agent: 'ws' } })
    fireEvent.change(input, { target: { value: 'chord' } })
    const mac = isMacPlatform()
    fireEvent.keyDown(input, { key: 's', metaKey: mac, ctrlKey: !mac })
    await waitFor(() => expect(input.value).toBe(''))
    expect(save).toHaveBeenCalledTimes(2)
    expect(commentPosts()).toHaveLength(0)
  })
})

describe('the message box takes files', () => {
  it('"+" picks, chips show, Send posts them with no text', async () => {
    h.picked = ['/ws/out/shot.png', '/elsewhere/notes.txt']
    const c = rail()
    await pickAttach(c)
    await waitFor(() => expect(c.querySelectorAll('[data-testid="ticket-compose-files"] [data-zen-attachment]')).toHaveLength(2))
    expect(c.querySelector('[data-zen-attachment="shot.png"]')!.getAttribute('title')).toBe('/ws/out/shot.png')
    expect(c.querySelector('[data-zen-attachment="notes.txt"]')!.getAttribute('title')).toBe('/ws/.k2/downloads/notes.txt')
    // × removes one.
    fireEvent.click(c.querySelector('[aria-label="Remove notes.txt"]')!)
    expect(c.querySelectorAll('[data-testid="ticket-compose-files"] [data-zen-attachment]')).toHaveLength(1)
    // Text is optional with files.
    fireEvent.keyDown(c.querySelector('[data-testid="ticket-compose-input"]')!, { key: 'Enter' })
    await waitFor(() => expect(commentPosts()).toHaveLength(1))
    expect(commentPosts()[0].body).toEqual({ id: TICKET, body: '', attachments: [{ path: '/ws/out/shot.png' }] })
    await waitFor(() => expect(c.querySelector('[data-testid="ticket-compose-files"]')).toBeNull())
    expect(getTicketDraftFiles(TICKET)).toEqual([])
  })

  it('an option pick sends the chips too', async () => {
    h.picked = ['/ws/out/b.png']
    const c = rail()
    await pickAttach(c)
    await waitFor(() => expect(c.querySelector('[data-zen-attachment="b.png"]')).not.toBeNull())
    const pick = [...c.querySelectorAll<HTMLButtonElement>('[data-testid="ticket-quick-answer"]')].find((b) => b.textContent === 'B')!
    fireEvent.click(pick)
    await waitFor(() => expect(commentPosts()).toHaveLength(1))
    expect(commentPosts()[0].body).toEqual({ id: TICKET, body: 'B', optionPick: true, attachments: [{ path: '/ws/out/b.png' }] })
  })

  it('a paste and a drop attach; at most 10', async () => {
    const c = rail()
    const input = c.querySelector('[data-testid="ticket-compose-input"]')!
    const shot = new File(['x'], 'screenshot.png', { type: 'image/png' })
    fireEvent.paste(input, { clipboardData: { files: [shot], items: [], getData: () => '' } })
    await waitFor(() => expect(c.querySelector('[data-zen-attachment="screenshot.png"]')).not.toBeNull())
    expect(c.querySelector('[data-zen-attachment="screenshot.png"]')!.getAttribute('title')).toBe('/ws/.k2/downloads/screenshot.png')
    const box = c.querySelector('[data-testid="ticket-compose"]')!
    const dropped = Array.from({ length: 10 }, (_, i) => new File(['y'], `d${i}.txt`))
    fireEvent.drop(box, { dataTransfer: { files: dropped, types: ['Files'] } })
    await waitFor(() => expect(c.textContent).toContain('At most 10 files per message.'))
    expect(c.querySelectorAll('[data-testid="ticket-compose-files"] [data-zen-attachment]')).toHaveLength(10)
  })

  it('chips survive a remount (the draft) and a refusal reads as words', async () => {
    setTicketDraftFiles(TICKET, [{ path: '/ws/.env', name: '.env' }])
    h.postImpl = async () => {
      throw new DaemonHttpError('{"ok":false}', 400, 'attachment_secret')
    }
    const c = rail()
    expect(c.querySelector('[data-zen-attachment=".env"]'), 'restored from the draft').not.toBeNull()
    fireEvent.change(c.querySelector('[data-testid="ticket-compose-input"]')!, { target: { value: 'see' } })
    fireEvent.keyDown(c.querySelector('[data-testid="ticket-compose-input"]')!, { key: 'Enter' })
    await waitFor(() => expect(c.textContent).toContain('looks like a secret'))
    expect(c.textContent).toContain('the ticket')
    expect(c.querySelector('[data-zen-attachment=".env"]'), 'a refused send keeps the chips').not.toBeNull()
  })

  it('off with the reason when the ticket is unlinked', async () => {
    const c = rail({}, null)
    await openPlus(c)
    expect(plusRow('attach').enabled).toBe(false)
    expect(plusRow('attach').tooltip).toContain('workspace was removed')
    act(() => useContextMenuStore.getState().close())
    expect(c.querySelector('[data-testid="ticket-compose"]')!.hasAttribute('data-compose-bar')).toBe(false)
  })

  it('a file from the drop router (k2so:compose-insert) attaches', async () => {
    const c = rail()
    const box = c.querySelector('[data-testid="ticket-compose"]') as HTMLElement
    expect(box.getAttribute('data-compose-bar')).toBe('')
    expect(box.getAttribute('data-workspace-path')).toBe('/ws')
    await act(async () => {
      box.dispatchEvent(new CustomEvent('k2so:compose-insert', { detail: { data: 'x', paths: ['/ws/.k2/downloads/up.png'], uploaded: true } }))
    })
    await waitFor(() => expect(c.querySelector('[data-zen-attachment="up.png"]')).not.toBeNull())
  })
})

describe('Download quarantines what someone else attached', () => {
  function files(source: ReturnType<typeof ticketAttachmentSource> | { scope: ServerScope; addr: string }, mine: boolean): HTMLElement {
    return render(
      <ThreadAttachmentSourceProvider value={source}>
        <ThreadFileObjects files={[pdf]} mine={mine} docId={SEED} />
      </ThreadAttachmentSourceProvider>,
    ).container
  }

  it('a ticket file: quarantined when someone else attached it, through the ticket door', async () => {
    const scope = fakeScope('box.example')
    const c = files(ticketAttachmentSource(scope, TICKET), false)
    fireEvent.click(c.querySelector('[data-thread-file-download]')!)
    await waitFor(() => expect(h.downloads).toHaveLength(1))
    expect(h.downloads[0].opts).toEqual({ quarantine: true })
    expect(h.gets.at(-1)).toEqual({
      path: 'feedback/attachment',
      params: { id: TICKET, commentId: SEED, n: 0, mode: 'download', offset: 0, len: 8 },
    })
  })

  it('your own file is not quarantined; a Thread file from others is', async () => {
    const scope = fakeScope('box.example')
    let c = files(ticketAttachmentSource(scope, TICKET), true)
    fireEvent.click(c.querySelector('[data-thread-file-download]')!)
    await waitFor(() => expect(h.downloads).toHaveLength(1))
    expect(h.downloads[0].opts).toEqual({})
    cleanup()
    c = files({ scope, addr: 'sales' }, false)
    fireEvent.click(c.querySelector('[data-thread-file-download]')!)
    await waitFor(() => expect(h.downloads).toHaveLength(2))
    expect(h.downloads[1].opts).toEqual({ quarantine: true })
    expect(h.gets.at(-1)!.path).toBe('thread/attachment')
  })
})

describe('the board card paperclip (TK21)', () => {
  it('shows the count when files ride the ticket, nothing at 0 or from an older daemon', () => {
    let c = render(<FileCountBadge count={3} />).container
    const badge = c.querySelector('[data-testid="card-file-count"]')!
    expect(badge.textContent).toBe('3')
    expect(badge.getAttribute('aria-label')).toBe('3 files')
    cleanup()
    c = render(<FileCountBadge count={1} />).container
    expect(c.querySelector('[data-testid="card-file-count"]')!.getAttribute('aria-label')).toBe('1 file')
    cleanup()
    for (const n of [0, undefined, null, Number.NaN]) {
      c = render(<FileCountBadge count={n} />).container
      expect(c.querySelector('[data-testid="card-file-count"]'), String(n)).toBeNull()
      cleanup()
    }
  })
})
