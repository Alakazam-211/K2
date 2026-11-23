// Build-time half of the code viewer's shiki allowlist (shiki-allowlist.ts is
// the runtime half). A kept grammar can still import a left-out one as an
// embedded language (cpp and elm embed glsl; ruby, handlebars and rst embed
// yaml), and a plain import would carry the left-out grammar into the bundle.
// This plugin swaps every left-out grammar or theme module, however it is
// reached, for an empty module, so none of them can land in a built chunk.
// The highlighter drops the matching `embeddedLangs` names, so the kept
// grammar loads and the embedded region renders unhighlighted.
//
// Used by vite.config.ts and vite.config.web.ts (production builds). Vitest
// runs node_modules natively, past Vite plugins, so the runtime half also
// drops left-out registrations itself.

import { readFileSync } from 'fs'
import { resolve } from 'path'
import type { Plugin } from 'vite'
import { parseShikiExcluded, type ShikiExcluded } from './shiki-excluded'

/**
 * The left-out list, relative to the repo root. One source: shiki-allowlist.ts
 * imports the same file with `?raw`, and scripts/gen-shiki-allowlist.mjs reads
 * this constant. Repoint both together (the allowlist test checks they match).
 */
export const SHIKI_EXCLUDED_FILE = 'src/renderer/lib/shiki-excluded.txt'

const STUB_PREFIX = '\0k2-shiki-left-out:'

export function readShikiExcluded(repoRoot: string): ShikiExcluded {
  return parseShikiExcluded(readFileSync(resolve(repoRoot, SHIKI_EXCLUDED_FILE), 'utf8'))
}

/**
 * The left-out id a module specifier names, or undefined. Covers the shiki
 * re-export paths (`shiki/langs/<id>.mjs`), the package paths
 * (`@shikijs/langs/<id>`) and the relative imports one grammar file makes of
 * another inside `@shikijs/langs/dist` (`./<id>.mjs`).
 */
export function leftOutShikiModule(
  excluded: ShikiExcluded,
  source: string,
  importer: string | undefined,
): { kind: 'lang' | 'theme'; id: string } | undefined {
  const s = source.replace(/\\/g, '/')
  const imp = (importer ?? '').replace(/\\/g, '/')
  const pick = (kind: 'lang' | 'theme', id: string) =>
    (kind === 'lang' ? excluded.langs : excluded.themes).has(id) ? { kind, id } : undefined
  let m = /^(?:shiki\/(langs|themes)\/|@shikijs\/(langs|themes)\/(?:dist\/)?)([^/]+?)(?:\.mjs)?$/.exec(s)
  if (m) return pick((m[1] ?? m[2]) === 'langs' ? 'lang' : 'theme', m[3])
  m = /^\.\/([^/]+?)\.mjs$/.exec(s)
  if (m) {
    if (/\/@shikijs\/langs\/dist\/[^/]+$/.test(imp) || /\/shiki\/dist\/langs\/[^/]+$/.test(imp)) return pick('lang', m[1])
    if (/\/@shikijs\/themes\/dist\/[^/]+$/.test(imp) || /\/shiki\/dist\/themes\/[^/]+$/.test(imp)) return pick('theme', m[1])
  }
  return undefined
}

export function shikiLeftOutPlugin(repoRoot: string): Plugin {
  const excluded = readShikiExcluded(repoRoot)
  return {
    name: 'k2-shiki-left-out',
    enforce: 'pre',
    resolveId(source, importer) {
      const hit = leftOutShikiModule(excluded, source, importer)
      return hit ? `${STUB_PREFIX}${hit.kind}:${hit.id}` : null
    },
    load(id) {
      if (!id.startsWith(STUB_PREFIX)) return null
      // A grammar module's default export is a registration array; a theme's
      // is one registration. Both stubs carry no grammar or theme content.
      return id.slice(STUB_PREFIX.length).startsWith('lang:') ? 'export default []' : 'export default undefined'
    },
  }
}
