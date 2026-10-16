// W0 (0.46.0) — wiki hosting (Serve / Public chat) is hidden in the desktop
// Wiki page until it is rebuilt on the K2 runtime. The daemon enforces the
// security fixes; the UI only hides the buttons behind one flag.
//
// Source-inspection: rendering WikiPage needs the whole page/store mock
// surface; what matters is that every hosting control sits inside the one
// flag guard and the flag is off.

import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { WIKI_HOSTING_UI_ENABLED } from './wiki-api'

const here = dirname(fileURLToPath(import.meta.url))
const pageSrc = readFileSync(join(here, 'WikiPage.tsx'), 'utf8')

const GUARD_OPEN = '{WIKI_HOSTING_UI_ENABLED && ('

/** The JSX inside the guard: from the opener to its matching `)}`. */
function guardedBlock(src: string): string {
  const start = src.indexOf(GUARD_OPEN)
  if (start < 0) throw new Error('missing WIKI_HOSTING_UI_ENABLED guard in WikiPage.tsx')
  let depth = 0
  for (let i = start + GUARD_OPEN.length - 1; i < src.length; i++) {
    const ch = src[i]
    if (ch === '(') depth++
    else if (ch === ')') {
      depth--
      if (depth === 0) return src.slice(start, i + 1)
    }
  }
  throw new Error('unterminated WIKI_HOSTING_UI_ENABLED guard')
}

describe('Wiki page hosting controls (W0)', () => {
  it('the hosting flag is off', () => {
    expect(WIKI_HOSTING_UI_ENABLED).toBe(false)
  })

  it('has exactly one hosting guard', () => {
    expect(pageSrc.split(GUARD_OPEN).length - 1).toBe(1)
  })

  it('Serve, Public chat and the served URL are only inside the guard', () => {
    const block = guardedBlock(pageSrc)
    const outside = pageSrc.replace(block, '')
    for (const needle of [
      'onClick={() => void onToggleServe()}',
      'onClick={() => void onTogglePublicChat()}',
      "'Serve: ON'",
      "'Public chat: OFF'",
      'href={serve.url}',
    ]) {
      expect(block, needle).toContain(needle)
      expect(outside, needle).not.toContain(needle)
    }
  })

  it('there is no Publish button in the page', () => {
    expect(pageSrc).not.toMatch(/>\s*Publish\b/)
  })
})
