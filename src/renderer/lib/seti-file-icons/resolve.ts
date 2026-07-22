// Resolve a file basename to a Seti icon definition (glyph + color).
// Matching order mirrors VS Code's Seti theme: exact file name → compound
// extensions from the right → default file glyph.
// Light scheme uses *_light colors from the same font.

import {
  SETI_DEFAULT_ID,
  SETI_DEFAULT_ID_LIGHT,
  SETI_FILE_EXTENSIONS,
  SETI_FILE_EXTENSIONS_LIGHT,
  SETI_FILE_NAMES,
  SETI_FILE_NAMES_LIGHT,
  SETI_ICON_DEFS,
  type SetiIconDef,
} from './seti-theme-data'

export type SetiScheme = 'dark' | 'light'

const FALLBACK_DARK: SetiIconDef = SETI_ICON_DEFS[SETI_DEFAULT_ID] ?? {
  code: 0xe023,
  color: '#d4d7d6',
}
const FALLBACK_LIGHT: SetiIconDef = SETI_ICON_DEFS[SETI_DEFAULT_ID_LIGHT] ?? {
  code: FALLBACK_DARK.code,
  color: '#41535b',
}

/**
 * Extensionless basenames VS Code maps via language-id, not fileNames.
 * Keep this tiny — prefer theme data when a key exists.
 */
const EXTENSIONLESS_BASENAMES: Record<string, string> = {
  dockerfile: '_docker',
  makefile: '_makefile',
  gemfile: '_ruby',
  rakefile: '_ruby',
  procfile: '_heroku',
  jenkinsfile: '_jenkins',
  brewfile: '_ruby',
  vagrantfile: '_ruby',
}

function mapsFor(scheme: SetiScheme): {
  names: Record<string, string>
  exts: Record<string, string>
  defaultId: string
} {
  if (scheme === 'light') {
    return {
      names: SETI_FILE_NAMES_LIGHT as Record<string, string>,
      exts: SETI_FILE_EXTENSIONS_LIGHT as Record<string, string>,
      defaultId: SETI_DEFAULT_ID_LIGHT,
    }
  }
  return {
    names: SETI_FILE_NAMES as Record<string, string>,
    exts: SETI_FILE_EXTENSIONS as Record<string, string>,
    defaultId: SETI_DEFAULT_ID,
  }
}

/**
 * Resolve Seti icon id for a file basename (not a full path).
 * @internal exported for tests
 */
export function resolveSetiIconId(
  fileName: string,
  scheme: SetiScheme = 'dark',
): string {
  const { names, exts, defaultId } = mapsFor(scheme)
  const base = fileName.split(/[/\\]/).pop() || fileName
  if (!base) return defaultId

  const byName = names[base] ?? names[base.toLowerCase()]
  if (byName) return byName

  const lowerBase = base.toLowerCase()
  const extless = EXTENSIONLESS_BASENAMES[lowerBase]
  if (extless) {
    // Prefer scheme-specific id when present in defs
    if (scheme === 'light') {
      const lightId = `${extless}_light`
      if (SETI_ICON_DEFS[lightId]) return lightId
    }
    return extless
  }

  let rest = lowerBase
  while (rest.includes('.')) {
    const i = rest.indexOf('.')
    rest = rest.slice(i + 1)
    const byExt = exts[rest]
    if (byExt) return byExt
  }

  return defaultId
}

/** Glyph + color for a file name under dark or light scheme. */
export function resolveSetiIcon(
  fileName: string | undefined | null,
  scheme: SetiScheme = 'dark',
): SetiIconDef {
  const fallback = scheme === 'light' ? FALLBACK_LIGHT : FALLBACK_DARK
  if (!fileName) return fallback
  const id = resolveSetiIconId(fileName, scheme)
  return SETI_ICON_DEFS[id] ?? fallback
}
