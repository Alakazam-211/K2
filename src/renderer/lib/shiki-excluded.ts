// Parser for the code viewer's left-out shiki list (shiki-excluded.txt, or
// the notices list file that replaces it). Pure: the Vite plugin reads the
// file with fs at build time, the renderer reads it through `?raw`, and both
// parse it here, so the two always agree on the set.
//
// Line format (anything else, e.g. `#` comments, is ignored):
//   shiki <lang|theme> <id>: <licence> (<source>): <note>

export interface ShikiExcluded {
  langs: ReadonlySet<string>
  themes: ReadonlySet<string>
}

const LINE = /^shiki (lang|theme) ([A-Za-z0-9][A-Za-z0-9._+#-]*):/

export function parseShikiExcluded(text: string): ShikiExcluded {
  const langs = new Set<string>()
  const themes = new Set<string>()
  for (const raw of text.split('\n')) {
    const line = raw.trim()
    if (!line || line.startsWith('#')) continue
    const m = LINE.exec(line)
    if (!m) throw new Error(`shiki-excluded: unreadable line: ${line}`)
    ;(m[1] === 'lang' ? langs : themes).add(m[2])
  }
  if (langs.size === 0 && themes.size === 0) throw new Error('shiki-excluded: the list is empty')
  return { langs, themes }
}
