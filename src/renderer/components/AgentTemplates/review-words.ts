// What the two reviews (Create an agent, Save as template) call each thing:
// a plain name first, and the raw path or id as a quiet second line, never
// as the main label. The daemon's rows carry paths and ids
// (`.k2/agent/ROLE.md`, `wiki:index`, `compute.use`); a person reads
// "Instructions (charter)", "Wiki index (context)", "Use compute
// (permission)".

import type { HireRow } from '@/lib/agent-templates-api'
import { PRODUCT_NAME } from '@/lib/brand'

/** A plain name, and the raw path or id it stands for (shown quietly). */
export interface PlainItem {
  what: string
  raw?: string
}

/** `draft-review` → "Draft review"; `weekly_digest` → "Weekly digest". */
export function humanize(id: string): string {
  const words = id
    .replace(/\.[a-z0-9]+$/i, '')
    .replace(/[-_]+/g, ' ')
    .trim()
  return words ? words[0].toUpperCase() + words.slice(1) : id
}

const PERMISSIONS: Record<string, string> = {
  'permission-cards': 'Can ask you to change one of its settings, on a card in its Thread',
}

/** `compute.use` → "Use compute"; `vault.manage` → "Manage vault". */
export function permissionWords(id: string): string {
  if (PERMISSIONS[id]) return PERMISSIONS[id]
  const [area, verb] = id.split('.')
  if (!verb) return humanize(id)
  return `${verb[0].toUpperCase()}${verb.slice(1).replace(/[-_]+/g, ' ')} ${area.replace(/[-_]+/g, ' ')}`
}

/** A context entry: `wiki:index` → "Wiki index"; `skills` → "Skills". */
export function contextWords(id: string): string {
  return humanize(id.replace(/:/g, ' '))
}

/** A tool by its program name: `claude` → "Claude". */
export function toolWords(tool: string): string {
  return humanize(tool)
}

const base = (path: string): string => path.split('/').filter(Boolean).pop() ?? path

/** A file a template holds or an agent gets. */
export function fileWords(path: string): PlainItem {
  const name = base(path)
  if (/^ROLE\.md$/i.test(name)) return { what: 'Instructions (charter)', raw: path }
  if (/^template\.(toml|lock)$/i.test(name)) return { what: 'Template record', raw: path }
  if (/^_Index\.md$/i.test(name)) return { what: 'Wiki index', raw: path }
  const skill = /(?:^|\/)skills\/([^/]+)\/SKILL\.md$/i.exec(path)
  if (skill) return { what: `${humanize(skill[1])} (skill)`, raw: path }
  if (/(?:^|\/)wiki\//.test(path) && /\.md$/i.test(name)) return { what: `${name.replace(/\.md$/i, '')} (wiki note)`, raw: path }
  return { what: path }
}

/** One row of the Create review's dry run. */
export function hireRowWords(r: Pick<HireRow, 'step' | 'target'>): PlainItem {
  switch (r.step) {
    case 'folder':
      return { what: 'Its folder', raw: r.target }
    case 'register':
      // The folder row above already shows the path.
      return { what: `Adds it to your agents in ${PRODUCT_NAME}` }
    case 'role':
    case 'wiki':
    case 'wiki-index':
    case 'lock':
      return fileWords(r.target)
    case 'catalog':
      return { what: `${contextWords(r.target)} (context)`, raw: r.target }
    case 'layer':
      return { what: `${humanize(r.target)} (context layer)`, raw: r.target }
    case 'skill':
      return { what: `${humanize(r.target)} (skill)`, raw: r.target }
    case 'heartbeat':
      return { what: `${humanize(r.target)} (heartbeat)`, raw: r.target }
    case 'permission':
      return r.target in PERMISSIONS ? { what: PERMISSIONS[r.target] } : { what: `${permissionWords(r.target)} (permission)`, raw: r.target }
    case 'hello':
      return { what: 'Says hello in its Thread' }
    default:
      return { what: `${humanize(r.step)}: ${r.target}` }
  }
}

/** A row's state, said once: the daemon's reason when it gives one (it says
 *  more than the state), else the state. */
export function rowNote(statusLabel: string, reason?: string | null): string {
  const why = reason?.trim()
  if (!why) return statusLabel
  return why[0].toUpperCase() + why.slice(1)
}
