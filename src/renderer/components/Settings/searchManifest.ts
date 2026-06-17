import type { SettingsSection } from '@/stores/settings'

/**
 * One searchable entry in the Settings search palette. Each section
 * file (`sections/*.tsx`) exports a `SECTION_MANIFEST: SettingEntry[]`
 * describing its visible settings. The palette flattens them all into
 * one list and fuzzy-matches on label + description + keywords.
 *
 * `id` is stable and rendered on the matching DOM row as
 * `data-settings-id={id}` so the palette can scrollIntoView + briefly
 * highlight it after jumping to the section.
 */
export interface SettingEntry {
  /**
   * Stable, globally unique identifier. Convention:
   * `<section>.<kebab-key>`. Example: `general.cli-version`.
   * Rendered on the DOM row as `data-settings-id` for scroll-to-field.
   */
  id: string
  /** Which section in the sidebar this setting lives in. */
  section: SettingsSection
  /** Short, user-visible label (title-cased). Used as the primary match target. */
  label: string
  /**
   * Optional one-line description for context in the palette result row.
   * Short enough to sit on one line in the modal.
   */
  description?: string
  /**
   * Alternative phrasings the user might search for. Examples: a setting
   * called "Theme" might include `['color', 'appearance', 'dark mode']`.
   * Matched case-insensitively; order doesn't matter.
   */
  keywords?: string[]
  /**
   * Optional group label within the section — e.g. "Appearance" or
   * "Editing" in Code Editor. Displayed as a small breadcrumb so users
   * can disambiguate similarly-named settings across groups.
   */
  group?: string
}

/** Haystack built once per entry at index time. */
interface IndexedEntry {
  entry: SettingEntry
  haystack: string
}

/** Lightweight fuzzy-ish matcher: lowercase substring across all fields, with a simple score. */
export function scoreEntry(entry: SettingEntry, query: string): number {
  const q = query.toLowerCase().trim()
  if (!q) return 1 // every entry matches empty query
  const label = entry.label.toLowerCase()
  const desc = (entry.description ?? '').toLowerCase()
  const group = (entry.group ?? '').toLowerCase()
  const keywords = (entry.keywords ?? []).map((k) => k.toLowerCase())

  // Exact label match wins.
  if (label === q) return 1000
  // Label starts-with is very strong.
  if (label.startsWith(q)) return 500
  // Label contains query — strong.
  if (label.includes(q)) return 300
  // Group starts-with.
  if (group && group.startsWith(q)) return 200
  // Any keyword match.
  if (keywords.some((k) => k === q)) return 180
  if (keywords.some((k) => k.startsWith(q))) return 120
  if (keywords.some((k) => k.includes(q))) return 80
  // Description contains.
  if (desc.includes(q)) return 40
  // Group contains.
  if (group.includes(q)) return 30
  // Check tokenized multi-word queries: all tokens present somewhere.
  const tokens = q.split(/\s+/).filter(Boolean)
  if (tokens.length > 1) {
    const hay = `${label} ${desc} ${group} ${keywords.join(' ')}`
    if (tokens.every((t) => hay.includes(t))) return 20
  }
  return 0
}

/** Filter + rank a manifest against a query. Returns entries sorted by score desc. */
export function searchManifest(entries: SettingEntry[], query: string): SettingEntry[] {
  if (!query.trim()) return entries
  const scored: { entry: SettingEntry; score: number }[] = []
  for (const entry of entries) {
    const score = scoreEntry(entry, query)
    if (score > 0) scored.push({ entry, score })
  }
  scored.sort((a, b) => b.score - a.score)
  return scored.map((s) => s.entry)
}

/** Section label lookup — duplicated from Settings.tsx SECTIONS so manifests don't import the router. */
export const SECTION_LABELS: Record<SettingsSection, string> = {
  general: 'General',
  projects: 'Workspaces',
  'workspace-states': 'Workspace States',
  'agent-skills': 'Canonical Agent Flow',
  terminal: 'Terminal',
  'code-editor': 'Code Editor',
  'editors-agents': 'Editors & Agents',
  keybindings: 'Keybindings',
  timer: 'Timer',
  companion: 'K2 Companion',
  connections: 'Connections',
  'k2-connect': 'K2 Connect',
}

/** Suppress unused-type warning since IndexedEntry is reserved for future optimization. */
export type { IndexedEntry }
