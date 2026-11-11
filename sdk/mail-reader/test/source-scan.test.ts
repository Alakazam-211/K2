// T12 (prd-ocmail-admin-in-k2-v1 §12) and the licence rule: the package
// talks to no server and frames nothing itself, has no JMAP client, never
// marks mail read on its own, and keeps both copyright lines and the MIT text.
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative, resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const PKG = resolve(__dirname, '..')
const SRC = join(PKG, 'src')

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name)
    if (statSync(p).isDirectory()) walk(p, out)
    else if (/\.(ts|tsx)$/.test(name)) out.push(p)
  }
  return out
}

/** Comments out, so prose may name what the code must not do. */
function code(path: string): string {
  return readFileSync(path, 'utf8')
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/(^|[^:'"`\\])\/\/.*$/gm, '$1')
}

const BANNED: ReadonlyArray<[string, RegExp]> = [
  ['fetch(', /\bfetch\s*\(/],
  ['XMLHttpRequest', /\bXMLHttpRequest\b/],
  ['WebSocket', /\bWebSocket\b/],
  ['EventSource', /\bEventSource\b/],
  ['a JMAP client', /jmap\/client|JMAPClient|JmapClient|\/jmap\//i],
  ['markAsRead', /\bmarkAsRead\b/],
  ['uploadBlob', /\buploadBlob\b/],
  ['sendEmail', /\bsendEmail\b/],
  ['moveToMailbox', /\bmoveToMailbox\b/],
  ['an iframe', /<iframe\b|createElement\(\s*['"`]iframe/i],
  ['srcdoc', /\bsrcdoc\b/i],
  ['allow-same-origin', /allow-same-origin/],
  ['next/*', /from\s+['"]next(\/|['"])/],
  ['next-intl', /next-intl/],
  ['lucide-react', /lucide-react/],
  ['@tanstack/react-virtual', /@tanstack\/react-virtual/],
  ['date-fns', /date-fns/],
  ['the renderer', /from\s+['"]@\//],
]

describe('@k2/mail-reader source scan', () => {
  const files = walk(SRC)

  it('finds the package sources', () => {
    expect(files.map((f) => relative(SRC, f))).toEqual(
      expect.arrayContaining(['index.ts', 'source.ts', 'sanitize.ts', 'views/MailReader.tsx', 'views/EmailViewer.tsx']),
    )
  })

  it('no network call, JMAP client, frame, auto mark-read or OCmail-only dependency', () => {
    const hits: string[] = []
    for (const f of files) {
      const c = code(f)
      for (const [label, re] of BANNED) if (re.test(c)) hits.push(`${relative(SRC, f)}: ${label}`)
    }
    expect(hits).toEqual([])
  })

  it('imports only react, dompurify and its own files', () => {
    const outside: string[] = []
    for (const f of files) {
      for (const m of code(f).matchAll(/from\s+['"]([^'"]+)['"]/g)) {
        const spec = m[1]
        if (spec.startsWith('.') || spec === 'react' || spec === 'dompurify') continue
        outside.push(`${relative(SRC, f)}: ${spec}`)
      }
    }
    expect(outside).toEqual([])
  })

  it('NOTICE keeps both copyright lines and the MIT permission text; UPSTREAM names the upstream', () => {
    const notice = readFileSync(join(PKG, 'NOTICE'), 'utf8')
    expect(notice).toContain('Copyright (c) 2025 Matthieu MALVACHE')
    expect(notice).toContain('Copyright (c) 2026 Alakazam Labs')
    expect(notice).toContain('Permission is hereby granted, free of charge, to any person obtaining a copy')
    expect(notice).toContain('THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND')
    const upstream = readFileSync(join(PKG, 'UPSTREAM.md'), 'utf8')
    expect(upstream).toContain('root-fr/jmap-webmail')
    expect(upstream).toContain('v1.7.3')
    expect(upstream).toContain('`tauri`')
    expect(upstream).not.toMatch(/Alakazam-211\//)
    const pkg = JSON.parse(readFileSync(join(PKG, 'package.json'), 'utf8')) as { license: string; name: string }
    expect(pkg).toMatchObject({ name: '@k2/mail-reader', license: 'MIT' })
  })

  it('every adapted file credits OCmail and points at the NOTICE', () => {
    for (const rel of ['types.ts', 'thread-utils.ts', 'sanitize.ts', 'format.ts', 'views/FolderList.tsx', 'views/EmailList.tsx', 'views/ThreadListItem.tsx', 'views/ThreadConversationView.tsx', 'views/EmailViewer.tsx']) {
      // The header comment, comment markers and line breaks folded away.
      const head = readFileSync(join(SRC, rel), 'utf8').slice(0, 700).replace(/\n\s*\/\/\s?/g, ' ').replace(/\s+/g, ' ')
      expect(head, rel).toMatch(/OCmail/)
      expect(head, rel).toMatch(/NOTICE/)
      expect(head, rel).toContain('Matthieu MALVACHE')
    }
  })
})
