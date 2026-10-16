// The standard Wiki view's data (prd-wiki-3d-view-v1 §12): the note list
// (grouped by workspace, folder or title category) and backlinks. Pure.

import type { WikiIndex, WikiNode } from './wiki-api'
import { isWikiArticleNode, isWikiHomeNode } from './wiki-api'
import { wikiGraphVisibleIds, type WikiGraphOpts } from './wiki-graph-data'

export type WikiNoteGroup = {
  /** Stable key (`ws:<id>`, `dir:<folder>`, `cat:<prefix>`, `home`, `other`). */
  key: string
  /** Header text; null for the unlabelled top group (Home). */
  label: string | null
  notes: WikiNode[]
}

/** Note id without the fleet `wsId::` prefix (`sub/Note.md`). */
export function bareWikiNoteId(id: string): string {
  const i = id.indexOf('::')
  return i >= 0 ? id.slice(i + 2) : id
}

/** Folder of a note inside its wiki (`''` at the root). */
export function wikiNoteFolder(node: WikiNode): string {
  const bare = bareWikiNoteId(node.id)
  const slash = bare.lastIndexOf('/')
  return slash > 0 ? bare.slice(0, slash) : ''
}

const CATEGORY = /^([^-–—]{1,32}?)\s+[-–—]\s+\S/

/** Title category: `Feature - Tickets Board` → `Feature`. Null when there is none. */
export function wikiTitleCategory(title: string): string | null {
  const m = CATEGORY.exec(title.trim())
  return m ? m[1].trim() : null
}

function byTitle(a: WikiNode, b: WikiNode): number {
  const t = (a.title || a.id).localeCompare(b.title || b.id, undefined, { sensitivity: 'base' })
  return t !== 0 ? t : a.id < b.id ? -1 : a.id > b.id ? 1 : 0
}

/** Home first, then by title. */
function homeFirst(a: WikiNode, b: WikiNode): number {
  const ha = isWikiHomeNode(a) ? 0 : 1
  const hb = isWikiHomeNode(b) ? 0 : 1
  return ha !== hb ? ha - hb : byTitle(a, b)
}

/**
 * The notes the list shows: real articles in the same visible set as the
 * graphs (mode, Local depth, K2 lens + filters). While searching, only the
 * matches (not their graph neighbours), like the Articles count.
 */
export function wikiListNotes(index: WikiIndex, opts: WikiGraphOpts): WikiNode[] {
  const { visible, matchedIds } = wikiGraphVisibleIds(index, opts)
  const keep = matchedIds ?? visible
  return index.nodes.filter((n) => isWikiArticleNode(n) && keep.has(n.id))
}

/**
 * Group the list. All workspaces: one group per workspace (Home first in
 * each). One workspace: Home on top, then by folder when the note is in
 * one, else by title category (`Feature - …`), else "Other" last. A
 * category with a single note joins "Other".
 */
export function groupWikiNotes(notes: WikiNode[], fleet: boolean): WikiNoteGroup[] {
  if (fleet) {
    const byWs = new Map<string, WikiNoteGroup>()
    for (const n of notes) {
      const key = `ws:${n.workspaceId ?? ''}`
      let g = byWs.get(key)
      if (!g) {
        g = { key, label: n.workspaceName || n.workspaceId || 'Workspace', notes: [] }
        byWs.set(key, g)
      }
      g.notes.push(n)
    }
    const groups = [...byWs.values()]
    for (const g of groups) g.notes.sort(homeFirst)
    groups.sort((a, b) => (a.label ?? '').localeCompare(b.label ?? '', undefined, { sensitivity: 'base' }))
    return groups
  }

  const home: WikiNode[] = []
  const buckets = new Map<string, WikiNoteGroup>()
  const other: WikiNode[] = []
  for (const n of notes) {
    if (isWikiHomeNode(n) && wikiNoteFolder(n) === '') {
      home.push(n)
      continue
    }
    const folder = wikiNoteFolder(n)
    const cat = folder ? null : wikiTitleCategory(n.title || bareWikiNoteId(n.id))
    const key = folder ? `dir:${folder}` : cat ? `cat:${cat.toLowerCase()}` : null
    if (!key) {
      other.push(n)
      continue
    }
    let g = buckets.get(key)
    if (!g) {
      g = { key, label: folder ? `${folder}/` : (cat as string), notes: [] }
      buckets.set(key, g)
    }
    g.notes.push(n)
  }
  const groups: WikiNoteGroup[] = []
  for (const g of buckets.values()) {
    if (g.key.startsWith('cat:') && g.notes.length < 2) other.push(...g.notes)
    else groups.push(g)
  }
  for (const g of groups) g.notes.sort(byTitle)
  // Categories first (A–Z), folders after them (A–Z).
  groups.sort((a, b) => {
    const fa = a.key.startsWith('dir:') ? 1 : 0
    const fb = b.key.startsWith('dir:') ? 1 : 0
    if (fa !== fb) return fa - fb
    return (a.label ?? '').localeCompare(b.label ?? '', undefined, { sensitivity: 'base' })
  })
  const out: WikiNoteGroup[] = []
  if (home.length) out.push({ key: 'home', label: null, notes: home.sort(byTitle) })
  out.push(...groups)
  if (other.length) out.push({ key: 'other', label: groups.length || home.length ? 'Other' : null, notes: other.sort(byTitle) })
  return out
}

/**
 * Notes that link to `id` ("Linked from"): wikilinks only (not hub /
 * focus-group / project membership), existing articles, no self-links,
 * one entry per source, by title.
 */
export function wikiBacklinks(index: WikiIndex, id: string): WikiNode[] {
  const byId = new Map(index.nodes.map((n) => [n.id, n]))
  const seen = new Set<string>()
  const out: WikiNode[] = []
  for (const l of index.links) {
    if (l.target !== id || l.source === id) continue
    if (l.kind && l.kind !== 'wikilink') continue
    if (seen.has(l.source)) continue
    const src = byId.get(l.source)
    if (!src || !isWikiArticleNode(src)) continue
    seen.add(l.source)
    out.push(src)
  }
  return out.sort(byTitle)
}
