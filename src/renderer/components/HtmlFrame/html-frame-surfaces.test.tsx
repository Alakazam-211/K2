// @vitest-environment jsdom
// prd-html-frame-csp-v1.md F6/F11: every surface that renders HTML K2 did not
// author must go through HtmlFrame (per-frame CSP + sandbox). This scan fails
// loudly if a new `<iframe>` / `srcDoc` / `.srcdoc =` / createElement('iframe')
// appears anywhere else in the shipped renderer.
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative, resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { render } from '@testing-library/react'
import { HtmlFrame } from './HtmlFrame'
import { frameCsp } from '@/lib/frame-csp'

const RENDERER = resolve(__dirname, '../..')

/** The one file allowed to create an iframe for untrusted HTML. */
const OWNER = 'components/HtmlFrame/HtmlFrame.tsx'

/** Other iframes, with the reason they are not an untrusted-HTML surface. */
const ALLOWED_OTHER_FRAMES: Record<string, string> = {
  'dev/room-frame-probe.ts': 'DEV-only P1.5 probe (import.meta.env.DEV); same-app URL, never user HTML',
}

/** Surfaces that must use HtmlFrame, and the profile each must ask for. */
const SURFACES: ReadonlyArray<{ file: string; profile: 'scripted' | 'inert' }> = [
  { file: 'components/FileViewerPane/FileViewerPane.tsx', profile: 'scripted' },
  { file: 'components/Projects/ProjectDashboard.tsx', profile: 'scripted' },
  { file: 'components/AgentPane/AgentInboxPane.tsx', profile: 'inert' },
]

const FRAME_PATTERNS: ReadonlyArray<[string, RegExp]> = [
  ['<iframe', /<iframe\b/i],
  ['srcDoc attribute', /\bsrcdoc\s*=/i],
  ['.srcdoc assignment', /\.srcdoc\b/i],
  ["createElement('iframe')", /createElement\(\s*['"`]iframe['"`]/i],
  ['<frame>/<object>/<embed>', /<(?:frame|object|embed)\b/i],
]

function walk(dir: string, out: string[]): string[] {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name)
    if (statSync(p).isDirectory()) walk(p, out)
    else if (/\.(ts|tsx)$/.test(name) && !/\.test\.(ts|tsx)$/.test(name)) out.push(p)
  }
  return out
}

/** Drop // line comments and /* block comments *\/ so prose can mention iframes. */
function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:'"`\\])\/\/.*$/gm, '$1')
}

describe('HTML frame surfaces — source scan', () => {
  const files = walk(RENDERER, []).map((p) => relative(RENDERER, p).split('\\').join('/'))

  it('finds the renderer sources', () => {
    expect(files.length).toBeGreaterThan(100)
    expect(files).toContain(OWNER)
  })

  it('no file but HtmlFrame (and the listed dev probe) creates an iframe or srcdoc', () => {
    const offenders: string[] = []
    for (const rel of files) {
      if (rel === OWNER) continue
      const code = stripComments(readFileSync(join(RENDERER, rel), 'utf8'))
      for (const [label, re] of FRAME_PATTERNS) {
        if (!re.test(code)) continue
        if (rel in ALLOWED_OTHER_FRAMES && label === "createElement('iframe')") continue
        offenders.push(`${rel}: ${label}`)
      }
    }
    expect(offenders, 'route untrusted HTML through components/HtmlFrame/HtmlFrame.tsx').toEqual([])
  })

  it('every allow-listed other frame still exists (stale allow-list fails)', () => {
    for (const rel of Object.keys(ALLOWED_OTHER_FRAMES)) {
      const code = stripComments(readFileSync(join(RENDERER, rel), 'utf8'))
      expect(code, rel).toMatch(/createElement\(\s*['"`]iframe['"`]/)
      expect(code, `${rel} must stay DEV-gated`).not.toMatch(/srcdoc/i)
    }
    const index = readFileSync(join(RENDERER, 'index.tsx'), 'utf8')
    expect(index).toMatch(/import\.meta\.env\.DEV &&[\s\S]{0,400}room-frame-probe/)
  })

  it('nothing in the renderer asks for allow-same-origin', () => {
    const hits = files.filter((rel) =>
      /allow-same-origin/.test(stripComments(readFileSync(join(RENDERER, rel), 'utf8'))),
    )
    expect(hits).toEqual([])
  })

  for (const { file, profile } of SURFACES) {
    it(`${file} renders HTML through HtmlFrame profile="${profile}"`, () => {
      const code = stripComments(readFileSync(join(RENDERER, file), 'utf8'))
      expect(code).toMatch(/import \{ HtmlFrame \} from '@\/components\/HtmlFrame\/HtmlFrame'/)
      const uses = [...code.matchAll(/<HtmlFrame\b[\s\S]*?\/>/g)].map((m) => m[0])
      expect(uses.length, `${file}: expected an <HtmlFrame /> use`).toBeGreaterThan(0)
      for (const use of uses) expect(use).toContain(`profile="${profile}"`)
    })
  }
})

describe('HtmlFrame — rendered attributes', () => {
  it('scripted: srcdoc carries the CSP meta first, sandbox is allow-scripts, no referrer', () => {
    const { container } = render(
      <HtmlFrame title="doc" html="<!DOCTYPE html><script>1</script>" profile="scripted" />,
    )
    const f = container.querySelector('iframe')
    expect(f).not.toBeNull()
    expect(f!.getAttribute('sandbox')).toBe('allow-scripts')
    expect(f!.getAttribute('referrerpolicy')).toBe('no-referrer')
    expect(f!.getAttribute('src')).toBeNull()
    expect(f!.getAttribute('srcdoc')).toBe(
      `<!DOCTYPE html><meta http-equiv="Content-Security-Policy" content="${frameCsp('scripted')}"><script>1</script>`,
    )
  })

  it('inert: empty sandbox and the no-script CSP', () => {
    const { container } = render(<HtmlFrame title="mail" html="<p>hi</p>" profile="inert" testId="m" />)
    const f = container.querySelector('iframe[data-testid="m"]')
    expect(f).not.toBeNull()
    expect(f!.getAttribute('sandbox')).toBe('')
    expect(f!.getAttribute('srcdoc')).toBe(
      `<meta http-equiv="Content-Security-Policy" content="${frameCsp('inert')}"><p>hi</p>`,
    )
  })
})
