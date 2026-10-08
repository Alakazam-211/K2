// zenLibInlineSafe keeps a script's meaning while removing what would end
// or confuse an inline <script> element (UWB13).
import { runInNewContext } from 'node:vm'
import { describe, expect, it } from 'vitest'
import { zenLibInlineProblem, zenLibInlineSafe } from './zen-lib-inline'

const SOURCES = [
  'out = "a</script>b"',
  'out = "A</SCRIPT >b".length',
  "out = 'x<!--y'",
  'out = `t<!--${1 + 1}</script>`',
  'out = /<!--|<\\/script/u.test("<!--")',
  'out = /a<!--b/.test("xa<!--b")',
  'out = /[<]\\/script/.test("</script")',
  'out = "esc\\<!--"',
  'out = "esc\\</script>"',
  'out = "two\\\\</script>"',
  '/* <!-- </script> */ out = 7',
]

function run(code: string): unknown {
  const ctx: { out?: unknown } = {}
  runInNewContext(code, ctx)
  return ctx.out
}

describe('zenLibInlineSafe', () => {
  it.each(SOURCES)('keeps the meaning of %s and removes the problem', (src) => {
    const safe = zenLibInlineSafe(src)
    expect(zenLibInlineProblem(safe)).toBeNull()
    expect(run(safe)).toEqual(run(src))
  })

  it('reports problems', () => {
    expect(zenLibInlineProblem('a</script>')).toMatch(/<\/script/)
    expect(zenLibInlineProblem('a<!--')).toMatch(/<!--/)
    expect(zenLibInlineProblem('a<script>')).toBeNull()
  })

  it('is idempotent', () => {
    for (const src of SOURCES) expect(zenLibInlineSafe(zenLibInlineSafe(src))).toBe(zenLibInlineSafe(src))
  })
})
