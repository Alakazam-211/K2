// Thread file links (Rosson 2026-10-18): "file paths that lead to somewhere
// in this workspace are clickable. If they're outside the workspace then
// they're not clickable."
//
//   - `rehypeFileLinks` marks path-shaped text in a rendered message
//     (prose, and inline code that is a path and nothing else; never inside
//     a fenced code block or an existing link) as candidates.
//   - The daemon decides (daemon-first): `POST /cli/fs/resolve-links`
//     resolves each candidate against the message's workspace root,
//     follows symlinks, and answers linkable (kind + path) or a bare
//     "not linkable" for anything missing or outside. Candidates are
//     batched (one request per tick per Thread) and cached per Thread.
//   - `FileLink` draws a linkable candidate as a keyboard-reachable link;
//     anything else stays plain text.
//
// Only a message rendered under a `FileLinkProvider` gets links; every
// other `ChatMessageBody` (tickets, project chat, Grok Bots, Zen) is
// unchanged.

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
import { findFilePaths, matchWholeFilePath } from '@/lib/file-link-match'
import { daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'

/** One daemon answer (`k2_core::file_links::LinkAnswer`). */
export interface FileLinkAnswer {
  input: string
  linkable: boolean
  kind?: 'file' | 'directory'
  rel?: string
  path?: string
}

interface ResolveLinksResponse {
  root: string
  prefixes: string[]
  links: FileLinkAnswer[]
}

/** What a click does with a linkable answer. */
export type FileLinkOpen = (answer: FileLinkAnswer & { linkable: true; path: string }, line?: number, col?: number) => void

/** Sends one batch to the daemon. Injectable for tests. */
export type FileLinkTransport = (workspace: string, paths: string[]) => Promise<ResolveLinksResponse>

const MAX_BATCH = 200

/** Per-Thread resolver: a batched, cached view of the daemon's answers. */
export class FileLinkResolver {
  private answers = new Map<string, FileLinkAnswer | 'pending'>()
  private queue: string[] = []
  private flushScheduled = false
  private listeners = new Set<() => void>()
  private prefixList: string[] = []
  private dead = false
  private primed = false

  constructor(
    private readonly workspace: string,
    private readonly transport: FileLinkTransport,
  ) {}

  /** Root spellings (from the daemon) the matcher may take with spaces. */
  prefixes(): string[] {
    return this.prefixList
  }

  /** Learn the root's spellings before the first message needs them. */
  prime(): void {
    if (this.primed) return
    this.primed = true
    this.send([])
  }

  get(path: string): FileLinkAnswer | 'pending' | undefined {
    return this.answers.get(path)
  }

  /** Ask for `path` (no-op when asked before or the workspace is unknown). */
  request(path: string): void {
    if (this.dead || this.answers.has(path)) return
    this.answers.set(path, 'pending')
    this.queue.push(path)
    if (this.flushScheduled) return
    this.flushScheduled = true
    queueMicrotask(() => this.flush())
  }

  subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn)
    return () => this.listeners.delete(fn)
  }

  private bump(): void {
    for (const fn of this.listeners) fn()
  }

  private flush(): void {
    this.flushScheduled = false
    while (this.queue.length > 0) this.send(this.queue.splice(0, MAX_BATCH))
  }

  private send(batch: string[]): void {
    this.transport(this.workspace, batch)
      .then((res) => {
        const byInput = new Map((res.links ?? []).map((a) => [a.input, a]))
        for (const p of batch) {
          const a = byInput.get(p)
          this.answers.set(p, a ?? { input: p, linkable: false })
        }
        const prefixes = Array.isArray(res.prefixes) ? res.prefixes.filter((p) => typeof p === 'string') : []
        if (prefixes.join('\n') !== this.prefixList.join('\n')) this.prefixList = prefixes
        this.bump()
      })
      .catch(() => {
        // An unknown workspace (an imported or Grok Bot Thread), a server
        // without the route, or no connection: every candidate stays plain.
        this.dead = true
        for (const p of batch) this.answers.set(p, { input: p, linkable: false })
        this.bump()
      })
  }
}

/** The real transport: the room's own server. */
export function scopeFileLinkTransport(scope: ServerScope): FileLinkTransport {
  return (workspace, paths) => daemonCliPost<ResolveLinksResponse>(scope, 'fs/resolve-links', { workspace, paths })
}

interface FileLinkContextValue {
  resolver: FileLinkResolver
  open: FileLinkOpen
}

const FileLinkContext = createContext<FileLinkContextValue | null>(null)

/** Turn on file links for the messages below, resolved against
 *  `workspace` (a Thread address, a workspace path or id) on the server
 *  `transport` talks to. */
export function FileLinkProvider({
  workspace,
  transport,
  open,
  children,
}: {
  workspace: string
  transport: FileLinkTransport
  open: FileLinkOpen
  children: ReactNode
}): JSX.Element {
  const resolver = useMemo(() => new FileLinkResolver(workspace, transport), [workspace, transport])
  useEffect(() => resolver.prime(), [resolver])
  const value = useMemo(() => ({ resolver, open }), [resolver, open])
  return <FileLinkContext.Provider value={value}>{children}</FileLinkContext.Provider>
}

/** The provider's resolver, or null (no links here). Re-renders when its
 *  root spellings arrive. */
export function useFileLinkPrefixes(): { enabled: boolean; prefixes: string[] } {
  const ctx = useContext(FileLinkContext)
  // The snapshot is the prefix list itself (replaced only when it
  // changes), so answers arriving do not re-render every message.
  const prefixes = useSyncExternalStore(
    ctx?.resolver.subscribe ?? noopSubscribe,
    ctx ? () => ctx.resolver.prefixes() : emptyPrefixes,
  )
  return { enabled: !!ctx, prefixes }
}

const noopSubscribe = (): (() => void) => () => {}
const NO_PREFIXES: string[] = []
const emptyPrefixes = (): string[] => NO_PREFIXES

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

const SKIP_TAGS = new Set(['pre', 'a', 'script', 'style'])
const MAX_CANDIDATES_PER_MESSAGE = 100

function candidateSpan(path: string, line: number | undefined, col: number | undefined, children: HastNode[]): HastElement {
  return {
    type: 'element',
    tagName: 'span',
    properties: {
      dataK2FileCandidate: path,
      ...(line !== undefined ? { dataK2FileLine: String(line) } : {}),
      ...(col !== undefined ? { dataK2FileCol: String(col) } : {}),
    },
    children,
  }
}

function textOf(node: HastElement): string | null {
  if (node.children.length !== 1 || node.children[0].type !== 'text') return null
  return (node.children[0] as HastText).value
}

/** rehype plugin: wrap each path candidate in a marked `span`. */
export function rehypeFileLinks(options: { prefixes?: readonly string[] } = {}) {
  const prefixes = options.prefixes ?? []
  return (tree: HastNode): void => {
    let budget = MAX_CANDIDATES_PER_MESSAGE
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
          if (SKIP_TAGS.has(el.tagName)) {
            next.push(el)
            continue
          }
          if (el.tagName === 'code') {
            // Inline code (a fenced block sits in a <pre>, skipped above).
            const text = textOf(el)
            const m = text !== null ? matchWholeFilePath(text) : null
            if (m) {
              budget--
              next.push(candidateSpan(m.path, m.line, m.col, [el]))
            } else {
              next.push(el)
            }
            continue
          }
          walk(el)
          next.push(el)
          continue
        }
        if (node.type === 'text') {
          const value = (node as HastText).value
          const matches = findFilePaths(value, { prefixes }).slice(0, budget)
          if (matches.length === 0) {
            next.push(node)
            continue
          }
          let at = 0
          for (const m of matches) {
            budget--
            if (m.start > at) next.push({ type: 'text', value: value.slice(at, m.start) })
            next.push(candidateSpan(m.path, m.line, m.col, [{ type: 'text', value: value.slice(m.start, m.end) }]))
            at = m.end
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

/** Was this click the end of a text selection drag inside the message? */
function selectingText(): boolean {
  const sel = typeof window !== 'undefined' ? window.getSelection?.() : null
  return !!sel && !sel.isCollapsed && sel.toString().length > 0
}

export function FileLink({
  path,
  line,
  col,
  children,
}: {
  path: string
  line?: number
  col?: number
  children: ReactNode
}): JSX.Element {
  const ctx = useContext(FileLinkContext)
  const answer = useSyncExternalStore(
    ctx?.resolver.subscribe ?? noopSubscribe,
    ctx ? () => ctx.resolver.get(path) : () => undefined,
  )
  useEffect(() => {
    ctx?.resolver.request(path)
  }, [ctx, path])
  if (!ctx || !answer || answer === 'pending' || !answer.linkable || !answer.path) return <>{children}</>
  const target = answer as FileLinkAnswer & { linkable: true; path: string }
  const go = (): void => ctx.open(target, line, col)
  const where = `${target.rel || '.'}${line ? `:${line}${col ? `:${col}` : ''}` : ''}`
  return (
    <span
      role="link"
      tabIndex={0}
      className="k2-file-link"
      data-k2-file-link={target.kind}
      title={target.kind === 'directory' ? `Show ${where} in Files` : `Open ${where}`}
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

/** `span` for react-markdown: a marked candidate becomes a `FileLink`. */
export function MarkdownSpan(props: Record<string, unknown> & { children?: ReactNode }): JSX.Element {
  const { node: _node, children, ...rest } = props
  const candidate = rest['data-k2-file-candidate']
  if (typeof candidate !== 'string') return <span {...(rest as Record<string, string>)}>{children}</span>
  const line = Number(rest['data-k2-file-line']) || undefined
  const col = Number(rest['data-k2-file-col']) || undefined
  return (
    <FileLink path={candidate} line={line} col={col}>
      {children}
    </FileLink>
  )
}
