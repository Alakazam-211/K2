// Markdown with GFM (tables, task lists, strikethrough, autolinks) for the
// file editors (build-editors.md E2). One shared instance, so CodeEditor's
// own language for a .md file and Write mode's bundle are the same extension
// and CodeMirror keeps one copy. Code fences highlight through the languages
// CodeEditor already ships, in the token syntax theme.

import { markdown, markdownLanguage } from '@codemirror/lang-markdown'
import { StreamLanguage, type Language, type LanguageSupport } from '@codemirror/language'
import { javascript } from '@codemirror/lang-javascript'
import { python } from '@codemirror/lang-python'
import { rust } from '@codemirror/lang-rust'
import { css } from '@codemirror/lang-css'
import { html } from '@codemirror/lang-html'
import { json } from '@codemirror/lang-json'
import { cpp } from '@codemirror/lang-cpp'
import { java } from '@codemirror/lang-java'
import { go } from '@codemirror/lang-go'
import { sql } from '@codemirror/lang-sql'
import { xml } from '@codemirror/lang-xml'
import { yaml } from '@codemirror/lang-yaml'
import { php } from '@codemirror/lang-php'
import { shell } from '@codemirror/legacy-modes/mode/shell'
import { toml } from '@codemirror/legacy-modes/mode/toml'
import { diff } from '@codemirror/legacy-modes/mode/diff'

const FENCE: Record<string, () => Language> = {
  ts: () => javascript({ typescript: true }).language,
  tsx: () => javascript({ typescript: true, jsx: true }).language,
  js: () => javascript().language,
  jsx: () => javascript({ jsx: true }).language,
  json: () => json().language,
  py: () => python().language,
  rs: () => rust().language,
  go: () => go().language,
  css: () => css().language,
  html: () => html().language,
  sql: () => sql().language,
  xml: () => xml().language,
  yaml: () => yaml().language,
  java: () => java().language,
  cpp: () => cpp().language,
  php: () => php().language,
  sh: () => StreamLanguage.define(shell),
  toml: () => StreamLanguage.define(toml),
  diff: () => StreamLanguage.define(diff),
}
const ALIAS: Record<string, string> = {
  typescript: 'ts', javascript: 'js', mjs: 'js', cjs: 'js', jsonc: 'json', json5: 'json', python: 'py', rust: 'rs', golang: 'go',
  yml: 'yaml', c: 'cpp', 'c++': 'cpp', h: 'cpp', hpp: 'cpp', bash: 'sh', shell: 'sh', zsh: 'sh', console: 'sh', svg: 'xml', patch: 'diff',
}

/** The fence's language name, as typed after the backticks (lowercased, first word). */
export function fenceLanguageId(info: string): string {
  const id = info.trim().split(/[\s{]/)[0].toLowerCase()
  return ALIAS[id] ?? id
}

const loaded = new Map<string, Language>()
/** The highlighter for a fence's info string, or null for plain text. */
export function fenceLanguage(info: string): Language | null {
  const id = fenceLanguageId(info)
  const make = FENCE[id]
  if (!make) return null
  let lang = loaded.get(id)
  if (!lang) loaded.set(id, (lang = make()))
  return lang
}

/** How a fence's language reads in its label ("TypeScript"), or the info as typed. */
const LABELS: Record<string, string> = {
  ts: 'TypeScript', tsx: 'TSX', js: 'JavaScript', jsx: 'JSX', json: 'JSON', py: 'Python', rs: 'Rust', go: 'Go', css: 'CSS', html: 'HTML',
  sql: 'SQL', xml: 'XML', yaml: 'YAML', java: 'Java', cpp: 'C++', php: 'PHP', sh: 'Shell', toml: 'TOML', diff: 'Diff',
}
export function fenceLabel(info: string): string {
  const raw = info.trim().split(/[\s{]/)[0]
  return LABELS[fenceLanguageId(info)] ?? raw
}

let support: LanguageSupport | null = null
/** Markdown with GFM and highlighted fences: one instance for every editor. */
export function gfmMarkdown(): LanguageSupport {
  return (support ??= markdown({ base: markdownLanguage, codeLanguages: fenceLanguage }))
}
