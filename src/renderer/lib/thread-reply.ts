// prd-thread-message-menu-v1 — replying to one Thread message (or a piece of
// one) from the message's menu (Rosson 2026-10-30).
//
// The reply rides the message TEXT, not a new daemon field: every server and
// every client already carries it, and the agent gets it on its PTY with the
// rest of the person's words. One fixed format, so agents read it the same way
// every time and this window can draw it back as a small quote:
//
//   Replying to Thread message 42 from k2 (id <uuid>; read it whole: k2 thread k2 --before-seq 43 --limit 1):
//   > the quoted words (the whole message up to 600 characters, else its
//   > first 600 and "…"; a selection: exactly the selection)
//
//   the person's own words
//
// A reply to a selection says "Replying to part of Thread message …". The
// read command is the existing `k2 thread <addr> --before-seq N --limit 1`
// (the newest message below seq N, which is message N-1): no new verb.

import { create } from 'zustand'
import { scopedKey, type ServerScope } from '@/kessel/server-scope'

/** A whole message is quoted in full up to this many characters. */
export const THREAD_REPLY_QUOTE_MAX = 600

/** What a reply points at. */
export interface ThreadReplyTarget {
  /** The Thread message's id. */
  id: string
  /** Its seq in the Thread (0 when unknown: no read command). */
  seq: number
  /** Who wrote it, as the agent knows them (an agent's address, a person's login). */
  from: string
  /** The Thread's address on its server (the read command's `<addr>`). */
  addr: string
  /** The words quoted: the message's text, or the selected piece. */
  text: string
  /** True: a reply to a selected piece of the message. */
  part: boolean
}

/** The quoted words: a selection exactly (outer blank lines dropped); a
 *  whole message in full up to THREAD_REPLY_QUOTE_MAX characters, else its
 *  first THREAD_REPLY_QUOTE_MAX and "…". */
export function threadReplyQuote(target: Pick<ThreadReplyTarget, 'text' | 'part'>): string {
  const text = target.text.replace(/\r\n?/g, '\n').replace(/^\s*\n/, '').replace(/\s+$/, '')
  if (target.part) return text
  const chars = Array.from(text)
  if (chars.length <= THREAD_REPLY_QUOTE_MAX) return text
  return `${chars.slice(0, THREAD_REPLY_QUOTE_MAX).join('').trimEnd()}…`
}

/** `addr` as one shell word (single-quoted unless it is plainly safe). */
function shellWord(s: string): string {
  return /^[A-Za-z0-9_./:@-]+$/.test(s) ? s : `'${s.replace(/'/g, `'\\''`)}'`
}

/** The header line (without the quote). */
export function threadReplyHeader(target: ThreadReplyTarget): string {
  const what = target.part ? 'part of Thread message' : 'Thread message'
  const num = target.seq > 0 ? ` ${target.seq}` : ''
  const from = target.from.trim() || 'someone'
  const read = target.seq > 0 ? `; read it whole: k2 thread ${shellWord(target.addr)} --before-seq ${target.seq + 1} --limit 1` : ''
  return `Replying to ${what}${num} from ${from} (id ${target.id}${read}):`
}

/** The text a reply sends: the header, the quote as `> ` lines, a blank
 *  line, then the person's words (none: the quote alone). */
export function formatThreadReply(target: ThreadReplyTarget, text: string): string {
  const quote = threadReplyQuote(target)
    .split('\n')
    .map((l) => (l ? `> ${l}` : '>'))
    .join('\n')
  const words = text.trim()
  return `${threadReplyHeader(target)}\n${quote}${words ? `\n\n${words}` : ''}`
}

/** A reply read back out of a message's text. */
export interface ParsedThreadReply {
  part: boolean
  seq: number
  from: string
  id: string
  /** The quoted words (the `> ` taken off). */
  quote: string
  /** The person's own words after the quote. */
  rest: string
}

const HEADER =
  /^Replying to (part of )?Thread message(?: (\d+))? from (.+?) \(id ([^\s;()]+)(?:; read it whole: k2 thread .+? --before-seq \d+ --limit 1)?\):$/

/** The reply at the start of `body`, else null (any other message). */
export function parseThreadReply(body: string): ParsedThreadReply | null {
  if (!body.startsWith('Replying to ')) return null
  const lines = body.replace(/\r\n?/g, '\n').split('\n')
  const m = HEADER.exec(lines[0])
  if (!m) return null
  let i = 1
  const quoted: string[] = []
  while (i < lines.length && (lines[i] === '>' || lines[i].startsWith('> '))) {
    quoted.push(lines[i] === '>' ? '' : lines[i].slice(2))
    i += 1
  }
  if (quoted.length === 0) return null
  // One blank line between the quote and the words.
  if (i < lines.length && lines[i] === '') i += 1
  return {
    part: !!m[1],
    seq: m[2] ? Number(m[2]) : 0,
    from: m[3],
    id: m[4],
    quote: quoted.join('\n'),
    rest: lines.slice(i).join('\n'),
  }
}

/** The first non-blank line of `text` (the chip's preview). */
export function threadReplyFirstLine(text: string): string {
  return text.split('\n').find((l) => l.trim())?.trim() ?? ''
}

/** Which composer a reply goes to: the Thread's address on its server. */
export function threadReplyKey(scope: ServerScope, addr: string): string {
  return scopedKey(scope, `thread-reply:${addr}`)
}

interface PendingReply extends ThreadReplyTarget {
  /** Bumped on every pick, so the composer focuses again. */
  nonce: number
}

interface ThreadReplyState {
  /** The reply waiting in each Thread's composer (this window only). */
  replies: Record<string, PendingReply>
  /** How many composers are open for each key (Reply shows only then). */
  composers: Record<string, number>
  set: (key: string, target: ThreadReplyTarget) => void
  clear: (key: string) => void
  /** A composer for `key` is open; returns its close. */
  register: (key: string) => () => void
}

let nonce = 0

export const useThreadReplyStore = create<ThreadReplyState>((set) => ({
  replies: {},
  composers: {},
  set: (key, target) => {
    nonce += 1
    set((s) => ({ replies: { ...s.replies, [key]: { ...target, nonce } } }))
  },
  clear: (key) =>
    set((s) => {
      if (!(key in s.replies)) return s
      const replies = { ...s.replies }
      delete replies[key]
      return { replies }
    }),
  register: (key) => {
    set((s) => ({ composers: { ...s.composers, [key]: (s.composers[key] ?? 0) + 1 } }))
    return () =>
      set((s) => {
        const n = (s.composers[key] ?? 0) - 1
        const composers = { ...s.composers }
        if (n > 0) composers[key] = n
        else delete composers[key]
        return { composers }
      })
  },
}))

/** Is a composer open for `key` (a Reply has somewhere to go)? */
export function threadReplyComposerOpen(key: string): boolean {
  return (useThreadReplyStore.getState().composers[key] ?? 0) > 0
}

/** Tests: forget every pending reply and composer. */
export function resetThreadReplyForTests(): void {
  useThreadReplyStore.setState({ replies: {}, composers: {} })
}
