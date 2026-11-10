// Settings → Context Catalog: the page's plain words and the rules for a new
// pack's short name. Pure (no React), so the rules are testable on their own.
//
// A context pack is a block of standing instructions that K2 adds to a
// workspace agent's always-on instructions (its AGENTS.md) once someone adds
// the pack to that workspace. The catalog is this server's library of them:
// the ones K2 ships, and the ones people here write. Adding one to a
// workspace happens in Workspaces / Agents → a workspace → Context.

import type { ContextCatalogEntry } from '@/lib/context-stack'

/** One line for each pack K2 ships, for a person reading Settings. The
 *  server's own description is written for agents (file paths, commands);
 *  it is the fallback for a pack this app doesn't know yet. */
const BUILTIN_SUMMARY: Record<string, string> = {
  'wiki:index': 'Gives agents the map of this workspace’s wiki, so they can find its notes.',
  'wiki:home': 'Gives agents this workspace’s wiki home page.',
  'wiki:hygiene': 'Asks agents to keep the wiki tidy: link new notes, keep the index current, leave no note unlinked.',
  'subagents:pack': 'Asks agents to hand heavy work to helper agents in their own copy of the code, then review it.',
  'manager:pack': 'Short standing orders for an agent that coordinates other workspaces.',
  'k2:pack': 'Short standing orders for K2’s own planning agent.',
  'connections:roster': 'Lists the agents connected to this workspace, on this computer and on others.',
  'heartbeats:roster': 'Lists this workspace’s scheduled wake-ups and how often each one runs.',
  'skills:roster': 'Lists the skills agents here can load when they need one.',
  'users:roster': 'Lists the people who can sign in to this server, and their roles.',
  'apps:roster': 'Lists the guests who use this server’s Apps.',
  'apps:maintained': 'Lists the Apps this agent looks after, with the previous agent’s notes after a handover.',
}

export function isUserPack(p: ContextCatalogEntry): boolean {
  return p.source.startsWith('catalog:user:') || p.id.startsWith('user:')
}

/** A list K2 rebuilds from current data each time it rewrites the agent's
 *  instructions (the rosters), rather than fixed text. */
export function isLivePack(p: ContextCatalogEntry): boolean {
  return (
    p.kind === 'live' ||
    (p.tags ?? []).some((t) => t.toLowerCase() === 'live') ||
    p.source.endsWith('-roster') ||
    p.id.endsWith(':roster')
  )
}

/** What a row says under the pack's name. */
export function packSummary(p: ContextCatalogEntry): string | null {
  if (!isUserPack(p) && BUILTIN_SUMMARY[p.id]) return BUILTIN_SUMMARY[p.id]
  const d = p.description?.trim()
  return d ? d : null
}

/** Names, summaries, ids and tags, for the search field. */
export function packMatches(p: ContextCatalogEntry, query: string): boolean {
  const q = query.trim().toLowerCase()
  if (!q) return true
  return [p.label, packSummary(p) ?? '', p.id, ...(p.tags ?? [])].join(' ').toLowerCase().includes(q)
}

export const SHORT_NAME_MAX = 64
export const PACK_NAME_MAX = 60

/** A short name from the pack's name: "Weekly review" → "weekly-review". */
export function shortNameFrom(name: string): string {
  return name
    .toLowerCase()
    .normalize('NFKD')
    .replace(/[\u0300-\u036f]/g, '')
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, SHORT_NAME_MAX)
    .replace(/-+$/, '')
}

/** Why a pack's name can't be used, or null. The server writes it into the
 *  pack's details as quoted text, so quotes and backslashes are refused. */
export function packNameProblem(name: string): string | null {
  if (!name.trim()) return null
  if (/["\\]/.test(name)) return 'A name can’t contain " or \\.'
  return null
}

/** Why a short name can't be used, or null. Mirrors the server's rule
 *  (lowercase letters, numbers, - and _, starting with a letter or number);
 *  the server still decides, and K2's own names are checked there. */
export function shortNameProblem(slug: string, taken: readonly string[]): string | null {
  if (!slug) return null
  if (slug.length > SHORT_NAME_MAX) return `Use ${SHORT_NAME_MAX} characters or fewer.`
  if (!/^[a-z0-9][a-z0-9_-]*$/.test(slug)) {
    return 'Use lowercase letters, numbers, - and _, starting with a letter or number.'
  }
  if (taken.includes(`user:${slug}`)) return 'A pack with this short name is already in the catalog. Change the short name.'
  return null
}

/** The next step after a create the server turned down, from its reason. */
export function createFailureNext(reason: string): string {
  const r = reason.toLowerCase()
  if (r.includes('already exists')) return 'A pack with this short name is already in the catalog. Pick another short name.'
  if (r.includes('reserved')) return 'K2 keeps this short name for its own packs. Pick another short name.'
  if (r.includes('filesystem-safe')) return 'Use only lowercase letters, numbers, - and _ in the short name.'
  return 'Nothing was added. Try again in a moment.'
}
