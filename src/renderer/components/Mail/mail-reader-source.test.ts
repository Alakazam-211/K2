// OA5: the renderer's MailReaderSource only ever calls the person-facing
// reader routes (/cli/mail/reader/*), never /cli/mail/read (which marks mail
// read for the mailbox's own reader) and never with a project= claim.
import { readFileSync, readdirSync } from 'node:fs'
import { join } from 'node:path'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  gets: [] as { route: string; params?: Record<string, unknown> }[],
  posts: [] as { route: string; body: unknown }[],
  answers: {} as Record<string, unknown>,
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string, params?: Record<string, unknown>) => {
    h.gets.push({ route, params })
    if (!(route in h.answers)) throw new Error(`no fake answer for ${route}`)
    return h.answers[route]
  }),
  daemonCliPost: vi.fn(async (_scope: unknown, route: string, body: unknown) => {
    h.posts.push({ route, body })
    return { ok: true }
  }),
}))

import { daemonMailReaderSource, READER_ROUTE } from './mail-reader-source'

const scope = {} as never

beforeEach(() => {
  h.gets.length = 0
  h.posts.length = 0
  h.answers = {
    'mail/reader/inboxes': { ok: true, viewer: { actor: 'user:a', role: 'admin', opensAny: true }, inboxes: [] },
    'mail/reader/folders': { ok: true, folders: [{ name: 'Inbox', role: 'inbox' }] },
    'mail/reader/messages': { ok: true, items: [], nextOffset: 50 },
    'mail/reader/message': { ok: true, markedRead: false, message: { id: 'm_1' } },
    'mail/reader/thread': { ok: true, messages: [{ id: 'm_1' }] },
    'mail/reader/attachment': { ok: true, filename: 'a.txt', mime: 'text/plain', size: 3, dataBase64: btoa('abc') },
  }
})

describe('daemonMailReaderSource', () => {
  it('every read and write goes to /cli/mail/reader/*, with the contract params', async () => {
    const s = daemonMailReaderSource(scope)
    await s.inboxes()
    await s.folders('bot@x.example')
    const page = await s.list('bot@x.example', { folder: 'archive', offset: 50, limit: 50, query: 'invoice', unread: true })
    expect(page.nextOffset).toBe(50)
    await s.message('m_1')
    await s.thread('m_1', 'inbox')
    const file = await s.attachment('m_1', 2)
    expect(file).toMatchObject({ filename: 'a.txt', mime: 'text/plain', size: 3 })
    expect(await file.blob.text()).toBe('abc')
    await s.move('m_1', 'Archive')
    await s.flag('m_1', { read: true })
    await s.archive('m_1')
    await s.delete('m_1')
    await s.reply({ id: 'm_1', body: 'hi' })
    await s.draft({ inbox: 'bot@x.example', to: ['a@b.example'], subject: 's', body: 'b' })
    await s.send({ inbox: 'bot@x.example', to: ['a@b.example'], subject: 's', body: 'b' })

    expect(h.gets.map((g) => g.route)).toEqual([
      'mail/reader/inboxes',
      'mail/reader/folders',
      'mail/reader/messages',
      'mail/reader/message',
      'mail/reader/thread',
      'mail/reader/attachment',
    ])
    expect(h.gets[2].params).toEqual({ inbox: 'bot@x.example', folder: 'archive', offset: 50, limit: 50, query: 'invoice', from: undefined, since: undefined, before: undefined, unread: 1 })
    expect(h.gets[5].params).toEqual({ id: 'm_1', part: 2 })
    expect(h.posts).toEqual([
      { route: 'mail/reader/move', body: { id: 'm_1', folder: 'Archive' } },
      { route: 'mail/reader/flag', body: { id: 'm_1', read: true } },
      { route: 'mail/reader/archive', body: { id: 'm_1' } },
      { route: 'mail/reader/delete', body: { id: 'm_1' } },
      { route: 'mail/reader/reply', body: { id: 'm_1', body: 'hi' } },
      { route: 'mail/reader/draft', body: { inbox: 'bot@x.example', to: ['a@b.example'], subject: 's', body: 'b' } },
      { route: 'mail/reader/send', body: { inbox: 'bot@x.example', to: ['a@b.example'], subject: 's', body: 'b' } },
    ])
    for (const c of [...h.gets.map((g) => g.params ?? {}), ...h.posts.map((p) => p.body as Record<string, unknown>)]) {
      expect(Object.keys(c)).not.toContain('project')
    }
  })

  it('an answer missing its field fails loudly, never an empty list', async () => {
    h.answers['mail/reader/inboxes'] = { ok: true }
    await expect(daemonMailReaderSource(scope).inboxes()).rejects.toThrow('without the viewer')
    h.answers['mail/reader/messages'] = { ok: true }
    await expect(daemonMailReaderSource(scope).list('a@b', {})).rejects.toThrow('without the message list')
  })

  it('no Mail page file names another mail route or a project claim', () => {
    expect(READER_ROUTE).toBe('mail/reader')
    const dir = __dirname
    const hits: string[] = []
    for (const name of readdirSync(dir)) {
      if (!/\.(ts|tsx)$/.test(name) || /\.test\./.test(name)) continue
      const code = readFileSync(join(dir, name), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:'"`\\])\/\/.*$/gm, '$1')
      for (const m of code.matchAll(/['"`/]mail\/([a-z/-]+)/g)) if (!m[1].startsWith('reader')) hits.push(`${name}: mail/${m[1]}`)
      if (/\bproject\s*:/.test(code) || /project=/.test(code)) hits.push(`${name}: project claim`)
    }
    expect(hits).toEqual([])
  })
})
