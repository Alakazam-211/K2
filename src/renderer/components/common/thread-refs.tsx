// `#N` references in a Thread message (thread-reply-to-v1, 0.46.18): a `#N`
// that names a message or card of THIS Thread (a card's number is its seq)
// is a link that jumps to it; any other `#N` stays plain text.
//
// The rehype plugin below marks every candidate once (it never depends on
// what is loaded, so a new message doesn't re-parse every bubble); each
// marked span asks the provider's resolver whether its seq is real:
//   - a seq among the messages in memory: yes;
//   - older than the oldest loaded one while there is more history: the
//     server is asked once (`probe`), the answer kept;
//   - anything else (between loaded messages but not one, newer than the
//     newest): no, until a message with that seq arrives.

import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useSyncExternalStore,
  type JSX,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
} from 'react'

/** The server's answer for a seq older than everything loaded. */
export type ThreadRefProbe = (seq: number) => Promise<boolean>

/** At most this many server probes per open Thread. */
const MAX_PROBES = 50

export class ThreadRefResolver {
  private known = new Set<number>()
  private oldest = Number.POSITIVE_INFINITY
  private hasMore = false
  private probed = new Map<number, boolean | 'pending'>()
  private listeners = new Set<() => void>()
  private probes = 0

  constructor(private readonly probe: ThreadRefProbe | null) {}

  /** The messages in memory (their seqs) and whether older ones exist. */
  setLoaded(seqs: readonly number[], hasMore: boolean): void {
    const next = new Set<number>()
    let oldest = Number.POSITIVE_INFINITY
    for (const s of seqs) {
      if (!Number.isSafeInteger(s) || s <= 0) continue
      next.add(s)
      if (s < oldest) oldest = s
    }
    const same = next.size === this.known.size && hasMore === this.hasMore && [...next].every((s) => this.known.has(s))
    if (same) return
    this.known = next
    this.oldest = oldest
    this.hasMore = hasMore
    this.emit()
  }

  subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn)
    return () => this.listeners.delete(fn)
  }

  private emit(): void {
    for (const fn of [...this.listeners]) fn()
  }

  /** Is `seq` a message or card of this Thread? `ask`: older than
   *  everything loaded, the server can say ([`request`]); `pending` while
   *  it is asked. */
  resolves(seq: number): boolean | 'pending' | 'ask' {
    if (this.known.has(seq)) return true
    const asked = this.probed.get(seq)
    if (asked !== undefined) return asked
    if (this.probe && seq < this.oldest && this.hasMore && this.probes < MAX_PROBES) return 'ask'
    return false
  }

  /** Ask the server about `seq` when it is older than everything loaded. */
  request(seq: number): void {
    if (this.resolves(seq) !== 'ask' || !this.probe) return
    this.probes += 1
    this.probed.set(seq, 'pending')
    this.emit()
    this.probe(seq).then(
      (yes) => {
        this.probed.set(seq, yes)
        this.emit()
      },
      () => {
        this.probed.set(seq, false)
        this.emit()
      },
    )
  }
}

interface ThreadRefContextValue {
  resolver: ThreadRefResolver
  jump: (seq: number) => void
}

const ThreadRefContext = createContext<ThreadRefContextValue | null>(null)

/** Turn on `#N` links for the messages below: `seqs` are the messages in
 *  memory, `probe` asks the server about an older one, `jump` goes there. */
export function ThreadRefProvider({
  seqs,
  hasMore,
  probe,
  jump,
  children,
}: {
  seqs: readonly number[]
  hasMore: boolean
  probe: ThreadRefProbe | null
  jump: (seq: number) => void
  children: ReactNode
}): JSX.Element {
  const resolver = useMemo(() => new ThreadRefResolver(probe), [probe])
  useEffect(() => resolver.setLoaded(seqs, hasMore), [resolver, seqs, hasMore])
  const value = useMemo(() => ({ resolver, jump }), [resolver, jump])
  return <ThreadRefContext.Provider value={value}>{children}</ThreadRefContext.Provider>
}

/** True under a ThreadRefProvider (the plugin runs only there). */
export function useThreadRefsOn(): boolean {
  return useContext(ThreadRefContext) !== null
}

// ── Marking candidates in the rendered tree (hast) ───────────────────────

interface HastText {
  type: 'text'
  value: string
}
interface HastElement {
  type: 'element'
  tagName: string
  properties?: Record<string, unknown>
  children: HastNode[]
}
type HastNode = HastText | HastElement | { type: string; children?: HastNode[] }

/** Never inside code, a link, or a file link's candidate. */
const SKIP_TAGS = new Set(['pre', 'code', 'a', 'script', 'style'])
const MAX_REFS_PER_MESSAGE = 50

/** `#N`: not after a letter, digit, `_`, `&`, `#`, `/` or `\` (a URL's
 *  fragment, `GH#22`, `&#39;`), and not followed by a letter, digit or `_`. */
const REF = /(^|[^\p{L}\p{N}_&#/\\])#(\d{1,9})(?![\p{L}\p{N}_])/gu

/** The `#N` candidates in `text`: where each starts and ends, and its seq. */
export function findThreadRefs(text: string): { start: number; end: number; seq: number }[] {
  const out: { start: number; end: number; seq: number }[] = []
  for (const m of text.matchAll(REF)) {
    const seq = Number(m[2])
    if (!Number.isSafeInteger(seq) || seq <= 0) continue
    const start = (m.index ?? 0) + m[1].length
    out.push({ start, end: start + 1 + m[2].length, seq })
  }
  return out
}

/** rehype plugin: wrap each `#N` candidate in a marked `span`. */
export function rehypeThreadRefs() {
  return (tree: HastNode): void => {
    let budget = MAX_REFS_PER_MESSAGE
    const walk = (parent: { children?: HastNode[] }): void => {
      const kids = parent.children
      if (!kids) return
      const next: HastNode[] = []
      for (const node of kids) {
        if (budget <= 0) {
          next.push(node)
          continue
        }
        if (node.type === 'element') {
          const el = node as HastElement
          if (!SKIP_TAGS.has(el.tagName) && el.properties?.dataK2FileCandidate === undefined) walk(el)
          next.push(el)
          continue
        }
        if (node.type === 'text') {
          const value = (node as HastText).value
          const refs = findThreadRefs(value).slice(0, budget)
          if (refs.length === 0) {
            next.push(node)
            continue
          }
          let at = 0
          for (const r of refs) {
            budget--
            if (r.start > at) next.push({ type: 'text', value: value.slice(at, r.start) })
            next.push({
              type: 'element',
              tagName: 'span',
              properties: { dataK2ThreadRef: String(r.seq) },
              children: [{ type: 'text', value: value.slice(r.start, r.end) }],
            })
            at = r.end
          }
          if (at < value.length) next.push({ type: 'text', value: value.slice(at) })
          continue
        }
        next.push(node)
      }
      parent.children = next
    }
    walk(tree as { children?: HastNode[] })
  }
}

// ── The link ─────────────────────────────────────────────────────────────

const noopSubscribe = (): (() => void) => () => {}
const never = (): false => false

function selectingText(): boolean {
  const sel = typeof window !== 'undefined' ? window.getSelection?.() : null
  return !!sel && !sel.isCollapsed && sel.toString().length > 0
}

/** A marked `#N`: a link when its seq is a message or card of this Thread. */
export function ThreadRef({ seq, children }: { seq: number; children: ReactNode }): JSX.Element {
  const ctx = useContext(ThreadRefContext)
  const real = useSyncExternalStore(
    ctx?.resolver.subscribe ?? noopSubscribe,
    ctx ? () => ctx.resolver.resolves(seq) : never,
  )
  // `ask` once the provider knows what is loaded (its effect runs after
  // this one's on the first paint), so the request follows the state.
  useEffect(() => {
    if (real === 'ask') ctx?.resolver.request(seq)
  }, [ctx, seq, real])
  if (!ctx || real !== true) return <>{children}</>
  const go = (): void => ctx.jump(seq)
  return (
    <span
      role="link"
      tabIndex={0}
      className="k2-thread-ref"
      data-k2-thread-ref-link={seq}
      title={`Show #${seq} in this Thread`}
      style={{ textDecoration: 'underline', textUnderlineOffset: 2, cursor: 'pointer' }}
      onClick={(e: MouseEvent) => {
        if (e.button !== 0 || selectingText()) return
        e.preventDefault()
        e.stopPropagation()
        go()
      }}
      onKeyDown={(e: KeyboardEvent) => {
        if (e.key !== 'Enter' && e.key !== ' ') return
        e.preventDefault()
        e.stopPropagation()
        go()
      }}
    >
      {children}
    </span>
  )
}
