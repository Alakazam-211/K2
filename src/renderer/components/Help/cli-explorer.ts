// The Help explorer's model: CLI tools → noun families → nouns → verbs,
// plus pages (`k2 study`, and the daemon glossary when it answers), and
// one search across all of it.
//
// Sources:
//   - @shared/cli-catalog.generated.json — written by
//     scripts/gen-cli-explorer-catalog.mjs from `k2 --schema`, `k2 study`
//     and k2-node's USAGE (a drift test keeps it current).
//   - k2Nouns.ts — families, friendly blurbs and examples.
//   - GET /cli/glossary (+ /cli/glossary/get) — live from the daemon.

import { K2_NODE_BLURBS, K2_NODE_INTRO, K2_EXPLORER_INTRO, K2_NOUN_GROUPS, type K2NounNote } from './k2Nouns'

export interface CliCatalog {
  k2: {
    description: string
    commands: Array<{ name: string; usage: string; description: string }>
    help: Record<string, string>
  }
  study: Array<{ id: string; title: string; summary: string; body: string }>
  k2Node: {
    tagline: string
    footer: string
    commands: Array<{ noun: string; usage: string; note: string; section: string }>
  }
}

export interface ExplorerVerb {
  /** Display name (`k2 agent hire`). */
  name: string
  usage: string
  description: string
}

export interface ExplorerNoun {
  /** `<tool>:<noun>`. */
  key: string
  toolId: ToolId
  groupId: string
  noun: string
  also: string[]
  blurb: string
  example?: string
  note?: K2NounNote
  verbs: ExplorerVerb[]
  /** `--help` text for a noun `--schema` does not list yet. */
  help?: string
}

export interface ExplorerGroup {
  id: string
  title: string
  nouns: ExplorerNoun[]
}

export type ToolId = 'k2' | 'k2-node'

export interface ExplorerTool {
  id: ToolId
  name: string
  intro: string
  groups: ExplorerGroup[]
  footer?: string
}

export type PageSetId = 'study' | 'glossary'

export interface ExplorerPage {
  key: string
  set: PageSetId
  id: string
  title: string
  summary: string
  /** Study pages carry their body; glossary bodies load on open. */
  body?: string
}

export interface ExplorerModel {
  tools: ExplorerTool[]
  study: ExplorerPage[]
}

export interface GlossaryEntry {
  term: string
  summary: string
}

/** The daemon glossary predates the rename; show it as K2. */
export function fixK2Name(text: string): string {
  return text.replace(/\bK2SO\b/g, 'K2').replace(/\bk2so\b/g, 'k2')
}

export function buildExplorerModel(catalog: CliCatalog): ExplorerModel {
  const byHead = new Map<string, CliCatalog['k2']['commands']>()
  for (const c of catalog.k2.commands) {
    const head = c.name.split(' ')[0]
    const list = byHead.get(head) ?? []
    list.push(c)
    byHead.set(head, list)
  }
  const k2Groups: ExplorerGroup[] = K2_NOUN_GROUPS.map((g) => ({
    id: g.id,
    title: g.title,
    nouns: g.items.map((item) => {
      const heads = [item.noun, ...(item.also ?? [])]
      const verbs = heads.flatMap((h) =>
        (byHead.get(h) ?? []).map((c) => ({ name: `k2 ${c.name}`, usage: c.usage, description: c.description })),
      )
      return {
        key: `k2:${item.noun}`,
        toolId: 'k2' as const,
        groupId: g.id,
        noun: item.noun,
        also: item.also ?? [],
        blurb: item.blurb,
        example: item.example,
        note: item.note,
        verbs,
        help: catalog.k2.help[item.noun],
      }
    }),
  }))

  const nodeNouns: ExplorerNoun[] = []
  const nodeByNoun = new Map<string, ExplorerNoun>()
  for (const c of catalog.k2Node.commands) {
    let n = nodeByNoun.get(c.noun)
    if (!n) {
      n = {
        key: `k2-node:${c.noun}`,
        toolId: 'k2-node',
        groupId: 'k2-node',
        noun: c.noun,
        also: [],
        blurb: K2_NODE_BLURBS[c.noun] ?? c.note,
        verbs: [],
      }
      nodeByNoun.set(c.noun, n)
      nodeNouns.push(n)
    }
    const name = c.usage.replace(/^sudo /, '').split(/ [[<(-]|  /)[0]
    n.verbs.push({ name, usage: c.usage, description: c.note })
  }

  return {
    tools: [
      { id: 'k2', name: 'k2', intro: K2_EXPLORER_INTRO, groups: k2Groups },
      {
        id: 'k2-node',
        name: 'k2-node',
        intro: K2_NODE_INTRO,
        groups: [{ id: 'k2-node', title: 'Commands', nouns: nodeNouns }],
        footer: catalog.k2Node.footer,
      },
    ],
    study: catalog.study.map((p) => ({
      key: `study:${p.id}`,
      set: 'study',
      id: p.id,
      title: p.title,
      summary: p.summary,
      body: p.body,
    })),
  }
}

export function glossaryPages(entries: GlossaryEntry[]): ExplorerPage[] {
  return entries.map((e) => ({
    key: `glossary:${e.term}`,
    set: 'glossary',
    id: e.term,
    title: e.term,
    summary: fixK2Name(e.summary),
  }))
}

export function allNouns(model: ExplorerModel): ExplorerNoun[] {
  return model.tools.flatMap((t) => t.groups.flatMap((g) => g.nouns))
}

// ── Search ──────────────────────────────────────────────────────────────

export type SearchKind = 'tool' | 'noun' | 'command' | 'page' | 'term'

export interface SearchResult {
  key: string
  kind: SearchKind
  label: string
  detail: string
  score: number
  target: ExplorerTarget
}

/** Where a result jumps to. */
export type ExplorerTarget =
  | { kind: 'tool'; toolId: ToolId; groupId?: string; nounKey?: string; verbName?: string }
  | { kind: 'pages'; set: PageSetId; pageId?: string }

export const SEARCH_BADGES: Record<SearchKind, string> = {
  tool: 'Tool',
  noun: 'Noun',
  command: 'Command',
  page: 'Page',
  term: 'Term',
}

function scoreText(q: string, words: string[], name: string, ...rest: string[]): number {
  const n = name.toLowerCase()
  let score = 0
  if (n === q) score = 100
  else if (n.startsWith(q)) score = 80
  else if (n.split(/[\s/:-]+/).some((w) => w.startsWith(q))) score = 60
  else if (n.includes(q)) score = 45
  const hay = rest.join('\n').toLowerCase()
  if (!score) {
    if (hay.includes(q)) score = 20
    else if (words.length > 1 && words.every((w) => n.includes(w) || hay.includes(w))) score = 10
  }
  return score
}

const KIND_ORDER: Record<SearchKind, number> = { tool: 0, noun: 1, page: 2, term: 3, command: 4 }

export function searchExplorer(
  model: ExplorerModel,
  glossary: ExplorerPage[],
  query: string,
  limit = 60,
): SearchResult[] {
  const q = query.trim().toLowerCase()
  if (!q) return []
  const words = q.split(/\s+/)
  // `k2 msg` searches as `msg` for nouns and verbs.
  const bare = q.replace(/^(sudo )?k2(-node)? /, '')
  const out: SearchResult[] = []
  for (const tool of model.tools) {
    const s = scoreText(q, words, tool.name, tool.intro)
    if (s >= 45) out.push({ key: `tool:${tool.id}`, kind: 'tool', label: tool.name, detail: tool.intro, score: s + 5, target: { kind: 'tool', toolId: tool.id } })
    for (const g of tool.groups) {
      for (const n of g.nouns) {
        const label = `${tool.name} ${n.noun}`
        const s2 = Math.max(
          scoreText(bare, words, n.noun, n.blurb, n.example ?? '', g.title),
          ...n.also.map((a) => scoreText(bare, words, a) - 5),
          scoreText(q, words, label),
        )
        if (s2) {
          out.push({
            key: n.key,
            kind: 'noun',
            label,
            detail: n.blurb,
            score: s2 + 2,
            target: { kind: 'tool', toolId: tool.id, groupId: g.id, nounKey: n.key },
          })
        }
        for (const v of n.verbs) {
          const vName = v.name.replace(/^(sudo )?k2(-node)? /, '')
          const s3 = Math.max(scoreText(bare, words, vName, v.usage, v.description), scoreText(q, words, v.name))
          if (s3) {
            out.push({
              key: `${n.key}::${v.name}::${v.usage}`,
              kind: 'command',
              label: v.name,
              detail: v.description || v.usage,
              score: s3 - 3,
              target: { kind: 'tool', toolId: tool.id, groupId: g.id, nounKey: n.key, verbName: v.name },
            })
          }
        }
      }
    }
  }
  for (const p of model.study) {
    const s = Math.max(scoreText(q, words, p.title, p.summary, p.body ?? ''), scoreText(q, words, p.id), scoreText(q, words, `k2 study ${p.id}`))
    if (s) out.push({ key: p.key, kind: 'page', label: p.title, detail: `k2 study ${p.id} · ${p.summary}`, score: s, target: { kind: 'pages', set: 'study', pageId: p.id } })
  }
  for (const p of glossary) {
    const s = scoreText(q, words, p.title, p.summary)
    if (s) out.push({ key: p.key, kind: 'term', label: p.title, detail: p.summary, score: s - 1, target: { kind: 'pages', set: 'glossary', pageId: p.id } })
  }
  const seen = new Set<string>()
  return out
    .sort((a, b) => b.score - a.score || KIND_ORDER[a.kind] - KIND_ORDER[b.kind] || a.label.localeCompare(b.label))
    .filter((r) => (seen.has(r.key) ? false : (seen.add(r.key), true)))
    .slice(0, limit)
}
