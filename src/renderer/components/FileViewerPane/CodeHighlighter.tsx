import { useState, useEffect, useRef } from 'react'
import { type BundledLanguage, type BundledTheme, codeToHtml, bundledLanguages } from 'shiki'

// ── Language detection ──────────────────────────────────────────────

const EXT_TO_LANG: Record<string, BundledLanguage> = {
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

const FILENAME_TO_LANG: Record<string, BundledLanguage> = {
  'Dockerfile': 'dockerfile',
  'Makefile': 'makefile',
  'CMakeLists.txt': 'cmake',
  'Gemfile': 'ruby',
  'Rakefile': 'ruby',
  'Vagrantfile': 'ruby',
  'Justfile': 'just',
  '.gitignore': 'gitignore',
  '.gitattributes': 'gitattributes',
  '.editorconfig': 'ini',
  '.eslintrc': 'json',
  '.prettierrc': 'json',
  'tsconfig.json': 'jsonc',
  'jsconfig.json': 'jsonc',
}

export function detectLanguage(filePath: string): BundledLanguage | undefined {
  const name = filePath.split('/').pop() || ''

  // Check full filename first
  if (FILENAME_TO_LANG[name] && name in bundledLanguages) return FILENAME_TO_LANG[name]
  if (FILENAME_TO_LANG[name]) return FILENAME_TO_LANG[name]

  // .env files (.env, .env.local, .env.production, etc.)
  if (name === '.env' || name.startsWith('.env.')) return 'dotenv' as BundledLanguage

  // Check extension (handle multi-part extensions like .test.ts)
  const parts = name.split('.')
  if (parts.length > 1) {
    const ext = parts.pop()!.toLowerCase()
    const lang = EXT_TO_LANG[ext]
    if (lang && lang in bundledLanguages) return lang
  }

  return undefined
}

/** Resolve a markdown fence language string to a Shiki language. */
export function resolveFenceLang(lang: string): BundledLanguage | undefined {
  const l = lang.toLowerCase().trim()
  if (l in bundledLanguages) return l as BundledLanguage
  // Common aliases
  const aliases: Record<string, BundledLanguage> = {
    'js': 'javascript', 'ts': 'typescript', 'py': 'python', 'rb': 'ruby',
    'sh': 'bash', 'shell': 'bash', 'zsh': 'bash', 'yml': 'yaml',
    'cs': 'csharp', 'c++': 'cpp', 'objc': 'objective-c',
    'dockerfile': 'dockerfile', 'makefile': 'makefile',
    'text': 'plaintext', 'txt': 'plaintext', 'plain': 'plaintext',
  }
  return aliases[l]
}

// ── Theme ───────────────────────────────────────────────────────────

const THEME: BundledTheme = 'github-dark-default'

// ── Highlighted code component (for file viewer) ────────────────────

interface CodeViewerProps {
  code: string
  language?: BundledLanguage
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

    codeToHtml(code, {
      lang: language,
      theme: THEME,
    }).then((result) => {
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

    codeToHtml(code, {
      lang,
      theme: THEME,
    }).then((result) => {
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
    <pre className="bg-[#1a1a1a] border border-[var(--color-border)] p-3 overflow-x-auto">
      <code className="text-xs text-[var(--color-text-secondary)] font-mono">{code}</code>
    </pre>
  )
}
