// 0.46.3 Thread parity in Gardens (Rosson 2026-10-20): K2's own impls of
// the catalog v16 widget Thread rows, against a faked daemon. Fixtures are
// made up (example.test, /w/scout).
import { beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  calls: [] as Array<{ method: string; route: string; data: Record<string, unknown> }>,
  get: new Map<string, (p: Record<string, unknown>) => unknown>(),
  post: new Map<string, (b: Record<string, unknown>) => unknown>(),
  drops: [] as Array<{ paths: string[]; workspacePath?: string }>,
  browserDrops: [] as Array<{ names: string[]; workspacePath?: string }>,
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string, params?: Record<string, unknown>) => {
    h.calls.push({ method: 'GET', route, data: params ?? {} })
    const fn = h.get.get(route)
    if (!fn) throw new Error(`unexpected GET ${route}`)
    return fn(params ?? {})
  }),
  daemonCliPost: vi.fn(async (_scope: unknown, route: string, body?: Record<string, unknown>) => {
    h.calls.push({ method: 'POST', route, data: body ?? {} })
    const fn = h.post.get(route)
    if (!fn) throw new Error(`unexpected POST ${route}`)
    return fn(body ?? {})
  }),
}))

vi.mock('@/lib/handle-remote-drop', () => ({
  executeRemoteDrop: vi.fn(
    async (_s: unknown, paths: string[], _t: unknown, ctx: { workspacePath?: string }, build: (p: string[]) => string) => {
      h.drops.push({ paths, workspacePath: ctx.workspacePath })
      return build(paths.map((p) => `${ctx.workspacePath}/.k2/downloads/${p.split('/').pop()}`))
    },
  ),
  executeBrowserFileDrop: vi.fn(
    async (_s: unknown, files: File[], _t: unknown, ctx: { workspacePath?: string }, build: (p: string[]) => string) => {
      h.browserDrops.push({ names: files.map((f) => f.name), workspacePath: ctx.workspacePath })
      return build(files.map((f) => `${ctx.workspacePath}/.k2/downloads/${f.name}`))
    },
  ),
}))

import { DaemonHttpError } from '@/lib/daemon-http-error'
import type { ServerScope } from '@/kessel/server-scope'
import {
  ZenThreadVerbError,
  projectZenThreadFileData,
  zenThreadAttachment,
  zenThreadAttachments,
  zenThreadDelta,
  zenThreadLinks,
  zenThreadMarkRead,
  zenThreadOpenTarget,
  zenThreadPickFiles,
  zenThreadPostAttachments,
  type ZenThreadTarget,
} from './zen-thread-files'

const ALL = ['thread-attachments-v1', 'thread-images-v1', 'thread-read-v1']

function scope(features: string[] = ALL, over: Partial<ServerScope> = {}): ServerScope {
  return {
    id: 'host:box.example.test',
    isPrimary: false,
    hostKey: 'box.example.test',
    connectionKey: 'box.example.test',
    isRemote: true,
    label: 'box',
    serverSupports: (f: string) => features.includes(f),
    isWindowHost: () => false,
    ...over,
  } as unknown as ServerScope
}

function target(s: ServerScope = scope()): ZenThreadTarget {
  return { address: 'scout::box.example.test', scope: s, threadAddr: 'scout/chat', workspacePath: '/w/scout' }
}

/** The daemon's resolve-links: a path under /w/scout/ is a file there. */
function resolveLinks(): void {
  h.post.set('fs/resolve-links', (b) => ({
    root: '/w/scout',
    prefixes: ['/w/scout'],
    links: (b.paths as string[]).map((p) => {
      const rel = p.startsWith('/w/scout/') ? p.slice('/w/scout/'.length) : p.startsWith('/') ? null : p
      return rel && !rel.includes('missing') ? { input: p, linkable: true, kind: 'file', rel, path: `/w/scout/${rel}` } : { input: p, linkable: false }
    }),
  }))
}

async function code(p: Promise<unknown>): Promise<string> {
  try {
    await p
  } catch (e) {
    if (e instanceof ZenThreadVerbError) return e.code
    throw e
  }
  throw new Error('expected a refusal')
}

const item = (seq: number, extra: Record<string, unknown> = {}) => ({
  collection: 'thread',
  seq,
  id: `m${seq}`,
  doc: { id: `m${seq}`, kind: 'text', from: 'scout', body: `line ${seq}`, via: 'agent', ...extra },
})

beforeEach(() => {
  h.calls.length = 0
  h.get.clear()
  h.post.clear()
  h.drops.length = 0
  h.browserDrops.length = 0
})

describe('thread.read afterSeq (thread-delta-v1)', () => {
  it('reads items after the seq, changed and removed; a voided card counts as removed', async () => {
    h.get.set('thread', () => ({
      ok: true,
      after_seq: 4,
      has_more: true,
      items: [item(6), item(5)],
      changed: [
        item(3, { kind: 'choice', choice: { prompt: 'Go?', options: [], allow_custom: false, status: 'answered', answer: 'Yes' } }),
        item(2, { kind: 'choice', choice: { prompt: 'Old?', options: [], allow_custom: false, status: 'voided' }, hitl_status: 'voided' }),
      ],
      removed: [{ id: 'm1', seq: 1 }, { id: 7 }],
    }))
    const d = await zenThreadDelta(target(), 4, 50)
    expect(h.calls).toEqual([{ method: 'GET', route: 'thread', data: { addr: 'scout/chat', after_seq: 4, limit: 50 } }])
    expect(d.items.map((i) => i.seq)).toEqual([5, 6])
    expect(d.hasMore).toBe(true)
    expect(d.changed.map((i) => i.id)).toContain('m3')
    // Only well-formed removals cross.
    expect(d.removed[0]).toEqual({ id: 'm1', seq: 1 })
    expect(d.removed.some((r) => (r.id as unknown) === 7)).toBe(false)
  })

  it('refuses a bad seq and an older server, before any request', async () => {
    expect(await code(zenThreadDelta(target(), -1))).toBe('bad_args')
    expect(await code(zenThreadDelta(target(), 1.5))).toBe('bad_args')
    expect(await code(zenThreadDelta(target(scope([])), 0))).toBe('verb_unavailable')
    expect(h.calls).toEqual([])
  })
})

describe('thread.markRead (thread-read-v1)', () => {
  it('posts the read cursor by the Thread address and answers the catalog shape', async () => {
    h.post.set('thread/read', () => ({ ok: true, kind: 'thread', id: 'c-1', changed: true, lastReadSeq: 9, lastSeq: 12, unread: 3, waiting: 1, total: 4 }))
    expect(await zenThreadMarkRead(target(), 9)).toEqual({ changed: true, lastReadSeq: 9, lastSeq: 12, unread: 3 })
    await zenThreadMarkRead(target())
    expect(h.calls.map((c) => c.data)).toEqual([{ addr: 'scout/chat', seq: 9 }, { addr: 'scout/chat' }])
  })

  it('refuses without the feature, on a view-only room, and a bad seq', async () => {
    expect(await code(zenThreadMarkRead(target(scope([])), 1))).toBe('verb_unavailable')
    expect(await code(zenThreadMarkRead(target(scope(ALL, { viewOnly: true } as Partial<ServerScope>)), 1))).toBe('verb_unavailable')
    expect(await code(zenThreadMarkRead(target(), -2))).toBe('bad_args')
    expect(h.calls).toEqual([])
  })
})

describe('thread.attachment (thread-images-v1)', () => {
  it('projects the answer key by key: never the path or the hash', async () => {
    h.get.set('thread/attachment', () => ({
      ok: true,
      kind: 'image',
      mime: 'image/jpeg',
      width: 512,
      height: 384,
      base64: 'AAAA',
      path: 'shots/a.heic',
      sha256: 'f'.repeat(64),
      name: 'a.heic',
      state: 'ok',
      secretExtra: 'x',
    }))
    const t = await zenThreadAttachment(target(), 'm4', 0, { mode: 'thumb', px: 400 })
    expect(t).toEqual({ kind: 'image', mime: 'image/jpeg', width: 512, height: 384, base64: 'AAAA', name: 'a.heic', state: 'ok' })
    // px rounds to the two sizes the daemon keeps.
    expect(h.calls[0].data).toEqual({ addr: 'scout/chat', id: 'm4', n: 0, mode: 'thumb', px: 512 })
    expect(projectZenThreadFileData({ kind: 'file', state: 'hidden', name: 'secret.txt', size: 9, path: 'x' })).toEqual({ kind: 'file', state: 'hidden' })
  })

  it('413 is too_large, 415 and 404 fail with the daemon’s code; bad args and older servers never ask', async () => {
    h.get.set('thread/attachment', (p) => {
      if (p.mode === 'preview') throw new DaemonHttpError('too big', 413, 'preview_too_large')
      if (p.n === 1) throw new DaemonHttpError('gone', 404, 'attachment_gone')
      throw new DaemonHttpError('no thumb', 415, 'no_thumbnail')
    })
    expect(await code(zenThreadAttachment(target(), 'm1', 0, { mode: 'preview' }))).toBe('too_large')
    await expect(zenThreadAttachment(target(), 'm1', 1, { mode: 'meta' })).rejects.toThrow(/attachment_gone/)
    await expect(zenThreadAttachment(target(), 'm1', 0, { mode: 'thumb' })).rejects.toThrow(/no_thumbnail/)
    const before = h.calls.length
    expect(await code(zenThreadAttachment(target(), '', 0))).toBe('bad_args')
    expect(await code(zenThreadAttachment(target(), 'm1', -1))).toBe('bad_args')
    expect(await code(zenThreadAttachment(target(), 'm1', 0, { mode: 'raw' as never }))).toBe('bad_args')
    expect(await code(zenThreadAttachment(target(), 'm1', 0, { px: 9999 }))).toBe('bad_args')
    expect(await code(zenThreadAttachment(target(scope(['thread-attachments-v1'])), 'm1', 0))).toBe('verb_unavailable')
    expect(h.calls.length).toBe(before)
  })
})

describe('thread.links', () => {
  it('answers {input, linkable, kind, rel}: the host path and root never cross; :line stays in input', async () => {
    resolveLinks()
    const r = await zenThreadLinks(target(), ['src/app.ts:12', '/w/scout/README.md', '/etc/hosts', 'not a path at all', 'src/missing.ts'])
    expect(r).toEqual({
      links: [
        { input: 'src/app.ts:12', linkable: true, kind: 'file', rel: 'src/app.ts' },
        { input: '/w/scout/README.md', linkable: true, kind: 'file', rel: 'README.md' },
        { input: '/etc/hosts', linkable: false },
        { input: 'not a path at all', linkable: false },
        { input: 'src/missing.ts', linkable: false },
      ],
    })
    expect(JSON.stringify(r)).not.toMatch(/"path"|"root"|prefixes/)
    // One batch, by path without the line; text that isn't path-shaped is
    // never asked about.
    expect(h.calls).toHaveLength(1)
    expect(h.calls[0].data).toEqual({
      workspace: 'scout/chat',
      paths: ['src/app.ts', '/w/scout/README.md', '/etc/hosts', 'src/missing.ts'],
    })
  })

  it('more than 200 candidates or a non-string is bad_args', async () => {
    expect(await code(zenThreadLinks(target(), Array.from({ length: 201 }, (_, i) => `a/${i}.ts`)))).toBe('bad_args')
    expect(await code(zenThreadLinks(target(), [7 as unknown as string]))).toBe('bad_args')
    expect(h.calls).toEqual([])
  })
})

describe('thread.open targets', () => {
  it('takes {docId, n} or {rel, line?}, nothing else', () => {
    expect(zenThreadOpenTarget({ docId: 'm1', n: 0 })).toEqual({ kind: 'attachment', docId: 'm1', n: 0 })
    expect(zenThreadOpenTarget({ rel: 'src/a.ts', line: 3 })).toEqual({ kind: 'rel', rel: 'src/a.ts', line: 3 })
    for (const bad of [null, {}, { docId: 'm1' }, { docId: 'm1', n: -1 }, { rel: '' }, { rel: 'a', line: 0 }, { rel: 'a', path: '/etc' }, { docId: 'm', n: 0, path: '/x' }]) {
      expect(() => zenThreadOpenTarget(bad), JSON.stringify(bad)).toThrow(ZenThreadVerbError)
    }
  })
})

describe('thread.pickFiles and posts with attachments', () => {
  const pickers = (native: string[] | null, browser: File[] = [], web = false) => ({
    native: async () => native,
    browser: async () => browser,
    web: () => web,
  })

  it('cancel answers no files and asks nothing', async () => {
    expect(await zenThreadPickFiles(target(), pickers(null))).toEqual({ files: [] })
    expect(await zenThreadPickFiles(target(), pickers([]))).toEqual({ files: [] })
    expect(await zenThreadPickFiles(target(), pickers(null, [], true))).toEqual({ files: [] })
    expect(h.calls).toEqual([])
    expect(h.drops).toEqual([])
  })

  it('this computer: a file in the workspace stays put; others are copied into .k2/downloads; all workspace-relative', async () => {
    resolveLinks()
    const local = target(scope(ALL, { isRemote: false, hostKey: 'local' }))
    const r = await zenThreadPickFiles(local, pickers(['/w/scout/notes/plan.md', '/Users/me/shot.png']))
    expect(r).toEqual({
      files: [
        { relPath: 'notes/plan.md', name: 'plan.md' },
        { relPath: '.k2/downloads/shot.png', name: 'shot.png' },
      ],
    })
    expect(h.drops).toEqual([{ paths: ['/Users/me/shot.png'], workspacePath: '/w/scout' }])
  })

  it('another server or the browser: everything is uploaded first', async () => {
    resolveLinks()
    const r = await zenThreadPickFiles(target(), pickers(['/w/scout/in.txt']))
    expect(r.files).toEqual([{ relPath: '.k2/downloads/in.txt', name: 'in.txt' }])
    const file = new File(['hi'], 'hello.txt', { type: 'text/plain' })
    const b = await zenThreadPickFiles(target(), pickers(null, [file], true))
    expect(b.files).toEqual([{ relPath: '.k2/downloads/hello.txt', name: 'hello.txt', size: 2, mime: 'text/plain' }])
    expect(h.browserDrops).toEqual([{ names: ['hello.txt'], workspacePath: '/w/scout' }])
  })

  it('needs thread-attachments-v1', async () => {
    expect(await code(zenThreadPickFiles(target(scope(['thread-read-v1'])), pickers(['/x'])))).toBe('verb_unavailable')
  })

  it('a post carries [{path: relPath, line?, lineEnd?}] on the compose send; bad lists are bad_args', async () => {
    h.post.set('thread/post', (b) => ({ ok: true, id: 'p1', seq: 7, body: b.text, via: 'compose', kind: 'text' }))
    const posted = await zenThreadPostAttachments(target(), '', zenThreadAttachments([{ relPath: 'a.png' }, { relPath: 'src/x.ts', line: 3, lineEnd: 9 }]))
    expect(posted?.seq).toBe(7)
    expect(h.calls.at(-1)).toEqual({
      method: 'POST',
      route: 'thread/post',
      data: { addr: 'scout/chat', text: '', via: 'compose', attachments: [{ path: 'a.png' }, { path: 'src/x.ts', line: 3, lineEnd: 9 }] },
    })
    for (const bad of [
      'a.png',
      [{ path: '/etc/hosts' }],
      [{ relPath: '' }],
      [{ relPath: 'a', line: 0 }],
      [{ relPath: 'a', files: [] }],
      Array.from({ length: 11 }, () => ({ relPath: 'a' })),
    ]) {
      expect(() => zenThreadAttachments(bad), JSON.stringify(bad)).toThrow(ZenThreadVerbError)
    }
    expect(await code(zenThreadPostAttachments(target(scope([])), 'x', [{ relPath: 'a' }]))).toBe('verb_unavailable')
  })
})
