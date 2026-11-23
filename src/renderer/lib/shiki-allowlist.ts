// The renderer's one shiki entry. Every grammar and theme the app highlights
// with comes through here: the ids in shiki-allowlist.generated.ts (made by
// scripts/gen-shiki-allowlist.mjs) minus anything on the left-out list, read
// from the same file the build plugin uses (SHIKI_EXCLUDED_FILE in
// shiki-vite-plugin.ts). Nothing else in src/renderer imports shiki's
// grammars or themes; shiki-allowlist.test.ts enforces that.
//
// A language that isn't allowed (or an alias of one) has no loader, so the
// caller renders the code as plain text.

import excludedText from './shiki-excluded.txt?raw'
import { parseShikiExcluded } from './shiki-excluded'
import {
  GENERATED_LANG_ALIASES,
  GENERATED_LANG_LOADERS,
  GENERATED_THEME_LOADERS,
  type ShikiLangLoader,
  type ShikiThemeLoader,
} from './shiki-allowlist.generated'
import type { LanguageRegistration } from 'shiki/core'

export const SHIKI_EXCLUDED = parseShikiExcluded(excludedText)

/** Raw text of the left-out list, as this module read it (for the test). */
export const SHIKI_EXCLUDED_TEXT: string = excludedText

const own = (o: object, k: string): boolean => Object.prototype.hasOwnProperty.call(o, k)

/** Grammar ids the code viewer may load. */
export const SHIKI_LANG_IDS: readonly string[] = Object.keys(GENERATED_LANG_LOADERS).filter(
  (id) => !SHIKI_EXCLUDED.langs.has(id),
)
/** Theme ids the code viewer may load. */
export const SHIKI_THEME_IDS: readonly string[] = Object.keys(GENERATED_THEME_LOADERS).filter(
  (id) => !SHIKI_EXCLUDED.themes.has(id),
)

const LANGS = new Set(SHIKI_LANG_IDS)
const THEMES = new Set(SHIKI_THEME_IDS)

/** Resolve a language id or alias (any case) to an allowed grammar id. */
export function resolveShikiLang(name: string): string | undefined {
  const l = name.toLowerCase().trim()
  if (LANGS.has(l)) return l
  if (own(GENERATED_LANG_ALIASES, l)) {
    const id = GENERATED_LANG_ALIASES[l]
    if (LANGS.has(id)) return id
  }
  return undefined
}

export function isShikiLangAllowed(id: string): boolean {
  return LANGS.has(id)
}

export function shikiLangLoader(id: string): ShikiLangLoader | undefined {
  return LANGS.has(id) && own(GENERATED_LANG_LOADERS, id) ? GENERATED_LANG_LOADERS[id] : undefined
}

export function shikiThemeLoader(id: string): ShikiThemeLoader | undefined {
  return THEMES.has(id) && own(GENERATED_THEME_LOADERS, id) ? GENERATED_THEME_LOADERS[id] : undefined
}

/**
 * Load an allowed grammar's registrations, with every left-out grammar taken
 * out: the registrations a grammar module carries for its embedded languages,
 * and the names in each `embeddedLangs` list (shiki refuses a grammar whose
 * embedded language is missing). Undefined when the id isn't allowed.
 */
export async function loadAllowedShikiLang(id: string): Promise<LanguageRegistration[] | undefined> {
  const loader = shikiLangLoader(id)
  if (!loader) return undefined
  const regs = (await loader()).default
  const keep = (n: string) => !SHIKI_EXCLUDED.langs.has(n)
  return regs
    .filter((r) => keep(r.name))
    .map((r) => {
      const embedded = r.embeddedLangs?.filter(keep)
      const lazy = r.embeddedLangsLazy?.filter(keep)
      return { ...r, embeddedLangs: embedded, embeddedLangsLazy: lazy }
    })
}
