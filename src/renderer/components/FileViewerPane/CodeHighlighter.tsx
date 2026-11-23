import { useState, useEffect, useRef } from 'react'
import { createHighlighterCore, type HighlighterCore } from 'shiki/core'
import { createOnigurumaEngine } from 'shiki/engine/oniguruma'
import { loadAllowedShikiLang, resolveShikiLang, shikiThemeLoader } from '@/lib/shiki-allowlist'

// Grammars and themes come only from the allowlist (lib/shiki-allowlist.ts),
// loaded one by one on first use. A language that isn't on it, or an alias of
// one, renders as plain, escaped text through the fallbacks below.

// ── Language detection ──────────────────────────────────────────────

// Every name below goes through the allowlist, so an entry whose grammar
// isn't allowed (toml, terraform, hcl, nginx...) detects nothing and
// the file renders as plain text. It highlights again on its own once the
// grammar is allowed.
const EXT_TO_LANG: Record<string, string> = {
  // Web
  ts: 'typescript', tsx: 'tsx', js: 'javascript', jsx: 'jsx',
  mjs: 'javascript', cjs: 'javascript', mts: 'typescript', cts: 'typescript',
  html: 'html', htm: 'html', css: 'css', scss: 'scss', less: 'less',
  vue: 'vue', svelte: 'svelte', astro: 'astro',
  // Data / config
  json: 'json', jsonc: 'jsonc', json5: 'json5',
  yaml: 'yaml', yml: 'yaml', toml: 'toml', xml: 'xml',
  csv: 'csv', ini: 'ini', properties: 'ini',
  // Systems
  rs: 'rust', go: 'go', c: 'c', cpp: 'cpp', h: 'c', hpp: 'cpp',
  zig: 'zig', swift: 'swift', m: 'objective-c',
  // Scripting
  py: 'python', rb: 'ruby', lua: 'lua', perl: 'perl', pl: 'perl',
  sh: 'bash', bash: 'bash', zsh: 'bash', fish: 'fish',
  ps1: 'powershell', psm1: 'powershell',
  // JVM
  java: 'java', kt: 'kotlin', kts: 'kotlin', scala: 'scala',
  groovy: 'groovy', gradle: 'groovy',
  // .NET
  cs: 'csharp', fs: 'fsharp', vb: 'vb',
  // Docs / markup
  md: 'markdown', mdx: 'mdx', tex: 'latex', rst: 'rst',
  // Config / DevOps
  dockerfile: 'dockerfile', tf: 'terraform', hcl: 'hcl',
  nginx: 'nginx', graphql: 'graphql', gql: 'graphql',
  prisma: 'prisma', proto: 'proto',
  // Shell / misc
  sql: 'sql', r: 'r', dart: 'dart', elixir: 'elixir', ex: 'elixir',
  exs: 'elixir', erl: 'erlang', hs: 'haskell', clj: 'clojure',
  lisp: 'lisp', rkt: 'scheme', ml: 'ocaml', nim: 'nim',
  php: 'php', twig: 'twig',
  // Misc
  makefile: 'makefile', cmake: 'cmake',
  diff: 'diff', patch: 'diff', log: 'log',
  env: 'dotenv',
}

const FILENAME_TO_LANG: Record<string, string> = {
  'Dockerfile': 'dockerfile',
  'Makefile': 'makefile',
  'CMakeLists.txt': 'cmake',
  'Gemfile': 'ruby',
  'Rakefile': 'ruby',
  'Vagrantfile': 'ruby',
  'Justfile': 'just',
  // NOTE: .gitignore / .gitattributes have no Shiki grammar — they render
  // via the plain-text fallback.
  '.editorconfig': 'ini',
  '.eslintrc': 'json',
  '.prettierrc': 'json',
  'tsconfig.json': 'jsonc',
  'jsconfig.json': 'jsonc',
}

/** The allowed grammar id for a file path, or undefined (plain text). */
export function detectLanguage(filePath: string): string | undefined {
  const name = filePath.split('/').pop() || ''

  // Check full filename first
  if (FILENAME_TO_LANG[name]) return resolveShikiLang(FILENAME_TO_LANG[name])

  // .env files (.env, .env.local, .env.production, etc.)
  if (name === '.env' || name.startsWith('.env.')) return resolveShikiLang('dotenv')

  // Check extension (handle multi-part extensions like .test.ts)
  const parts = name.split('.')
  if (parts.length > 1) {
    const ext = parts.pop()!.toLowerCase()
    const lang = EXT_TO_LANG[ext]
    if (lang) return resolveShikiLang(lang)
  }

  return undefined
}

// Shiki's built-in plain renderer (no grammar), for fences that say "text".
const PLAIN_FENCES = new Set(['text', 'txt', 'plain', 'plaintext'])

/**
 * Resolve a markdown fence language string to an allowed Shiki language
 * ('plaintext' for text fences), or undefined for the unstyled fallback.
 */
export function resolveFenceLang(lang: string): string | undefined {
  const l = lang.toLowerCase().trim()
  if (PLAIN_FENCES.has(l)) return 'plaintext'
  return resolveShikiLang(l)
}

// ── Theme ───────────────────────────────────────────────────────────

const THEME = 'github-dark-default'

// ── Highlighter (one per page, grammars loaded on first use) ────────

let highlighter: Promise<HighlighterCore> | null = null
const langLoads = new Map<string, Promise<boolean>>()

function getHighlighter(): Promise<HighlighterCore> {
  if (!highlighter) {
    highlighter = (async () => {
      const themeLoader = shikiThemeLoader(THEME)
      if (!themeLoader) throw new Error(`code viewer theme ${THEME} is not on the shiki allowlist`)
      return createHighlighterCore({
        themes: [(await themeLoader()).default],
        langs: [],
        engine: createOnigurumaEngine(import('shiki/wasm')),
      })
    })()
    // A failed start is retried on the next request (this one renders plain).
    highlighter.catch(() => { highlighter = null })
  }
  return highlighter
}

function ensureLang(hl: HighlighterCore, id: string): Promise<boolean> {
  let p = langLoads.get(id)
  if (!p) {
    p = (async () => {
      const regs = await loadAllowedShikiLang(id)
      if (!regs) return false
      await hl.loadLanguage(...regs)
      return true
    })()
    langLoads.set(id, p)
    p.catch(() => { langLoads.delete(id) })
  }
  return p
}

/**
 * Highlight `code` as `lang` (an allowed grammar id, or 'plaintext').
 * Resolves to null when the language isn't allowed: render plain text.
 */
export async function highlightToHtml(code: string, lang: string): Promise<string | null> {
  const hl = await getHighlighter()
  if (lang !== 'plaintext' && !(await ensureLang(hl, lang))) return null
  return hl.codeToHtml(code, { lang, theme: THEME })
}

// ── Highlighted code component (for file viewer) ────────────────────

interface CodeViewerProps {
  code: string
  language?: string
  className?: string
}

export function CodeViewer({ code, language, className }: CodeViewerProps): React.JSX.Element {
  const [html, setHtml] = useState<string | null>(null)
  const containerRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    let cancelled = false

    if (!language) {
      setHtml(null)
      return
    }

    highlightToHtml(code, language).then((result) => {
      if (!cancelled) setHtml(result)
    }).catch(() => {
      if (!cancelled) setHtml(null)
    })

    return () => { cancelled = true }
  }, [code, language])

  if (!html || !language) {
    // Fallback: plain text
    return (
      <pre className={`p-4 text-xs text-[var(--color-text-secondary)] font-mono leading-5 whitespace-pre overflow-auto ${className || ''}`}>
        <code>{code}</code>
      </pre>
    )
  }

  return (
    <div
      ref={containerRef}
      className={`code-highlighted overflow-auto ${className || ''}`}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  )
}

// ── Highlighted code block (for markdown fenced blocks) ─────────────

interface CodeBlockProps {
  code: string
  language?: string
}

export function HighlightedCodeBlock({ code, language }: CodeBlockProps): React.JSX.Element {
  const [html, setHtml] = useState<string | null>(null)
  const lang = language ? resolveFenceLang(language) : undefined

  useEffect(() => {
    let cancelled = false

    if (!lang) {
      setHtml(null)
      return
    }

    highlightToHtml(code, lang).then((result) => {
      if (!cancelled) setHtml(result)
    }).catch(() => {
      if (!cancelled) setHtml(null)
    })

    return () => { cancelled = true }
  }, [code, lang])

  if (html) {
    return (
      <div
        className="code-highlighted"
        dangerouslySetInnerHTML={{ __html: html }}
      />
    )
  }

  // Fallback: unstyled code block
  return (
    <pre className="bg-[var(--color-bg-inset)] border border-[var(--color-border)] p-3 overflow-x-auto">
      <code className="text-xs text-[var(--color-text-secondary)] font-mono">{code}</code>
    </pre>
  )
}
