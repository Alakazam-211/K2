// Settings uses K2's square controls. A native <input type="checkbox|radio">
// paints the browser's rounded control, so this source walk fails with
// file:line on any that is not Checkbox / Radio. The only
// exception is a custom-drawn square: an `sr-only` peer input whose visible
// box is a sibling span.
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative, sep } from 'node:path'
import { describe, expect, it } from 'vitest'

function tsxFiles(dir: string): string[] {
  const out: string[] = []
  for (const name of readdirSync(dir)) {
    const p = join(dir, name)
    if (statSync(p).isDirectory()) out.push(...tsxFiles(p))
    else if (name.endsWith('.tsx') && !name.includes('.test.')) out.push(p)
  }
  return out
}

const ROOT = __dirname
const FILES = tsxFiles(ROOT)

/** Every native checkbox/radio in `src`, as `file:line native <type>`. */
function nativeChoiceInputs(file: string, src: string): string[] {
  const hits: string[] = []
  // Up to the self-closing `/>`: a bare `>` also ends `=>` in handlers.
  const re = /<input\b[\s\S]*?\/>/g
  for (const m of src.matchAll(re)) {
    const type = /\btype=["'{]+(checkbox|radio)\b/.exec(m[0])
    if (!type) continue
    if (/\bsr-only\b/.test(m[0])) continue
    const line = src.slice(0, m.index).split('\n').length
    hits.push(`${file}:${line} native ${type[1]}`)
  }
  return hits
}

describe('Settings square controls', () => {
  it('walks the Settings sources', () => {
    expect(FILES.length).toBeGreaterThan(10)
    expect(FILES.some((f) => f.endsWith('WakeSchedulerSection.tsx'))).toBe(true)
  })

  it('the guard catches a native checkbox and radio, and skips sr-only squares', () => {
    const sample = [
      '<input type="checkbox" checked={a} />',
      '<input\n  type="radio"\n  name="x"\n/>',
      '<input type="checkbox" className="peer sr-only" />',
      '<Checkbox checked={b} />',
    ].join('\n')
    expect(nativeChoiceInputs('sample.tsx', sample)).toEqual([
      'sample.tsx:1 native checkbox',
      'sample.tsx:2 native radio',
    ])
  })

  it('has no native rounded checkbox or radio', () => {
    const hits = FILES.flatMap((f) => nativeChoiceInputs(relative(ROOT, f), readFileSync(f, 'utf8')))
    expect(hits).toEqual([])
  })

  it('Wake Scheduler "Also on battery" is the shared Toggle, like the wake switch above it', () => {
    const src = readFileSync(join(ROOT, 'sections', 'WakeSchedulerSection.tsx'), 'utf8')
    const at = src.indexOf('Also on battery')
    expect(at).toBeGreaterThan(-1)
    const after = src.slice(at)
    const toggle = after.indexOf('<Toggle')
    expect(toggle).toBeGreaterThan(-1)
    expect(after.slice(toggle, toggle + 300)).toContain('handleSetWake(true, !onBattery)')
    expect(src).not.toContain('<input')
  })
})

// Rosson 2026-10-18: a raw radio anywhere in the app's components paints
// the browser's round control. Every `type="radio"` must be the shared
// Radio (components/ui/Radio.tsx), the one file allowed to
// spell it. Tests are skipped; no sr-only exception for radios.
const COMPONENTS = join(__dirname, '..')

/** Every raw radio under `src/renderer/components`, as `file:line`. */
function rawRadios(): string[] {
  const allowed = join(COMPONENTS, 'ui', 'Radio.tsx')
  const hits: string[] = []
  for (const f of tsxFiles(COMPONENTS)) {
    if (f === allowed) continue
    const src = readFileSync(f, 'utf8')
    for (const m of src.matchAll(/<input\b[\s\S]*?\/>/g)) {
      if (!/\btype=["'{]+radio\b/.test(m[0])) continue
      hits.push(`${relative(COMPONENTS, f)}:${src.slice(0, m.index).split('\n').length}`)
    }
  }
  return hits
}

describe('square radios across the app', () => {
  it('walks every component source, including the Owner account prompt and the Clone dialog', () => {
    const files = tsxFiles(COMPONENTS).map((f) => relative(COMPONENTS, f))
    expect(files).toEqual(expect.arrayContaining([join('OwnerAccountPrompt', 'OwnerAccountPrompt.tsx'), join('CloneToDialog', 'CloneOptionsBlocks.tsx')]))
  })

  it('has no raw type="radio" outside ui/Radio.tsx', () => {
    expect(rawRadios()).toEqual([])
  })
})

// A style can only restyle a control that uses the shared set, so every
// `role="switch"` and `type="range"` under components/ must be Toggle / Slider
// (components/ui/). Gardens (components/Zen/) own their look. The bell buttons
// below are icon buttons announced as switches, with no track to restyle; each
// file may hold only that many.
const ICON_SWITCHES: Record<string, number> = {
  [join('common', 'UnreadBadge.tsx')]: 1,
  [join('WorkspacePanel', 'WorkspaceCompletionSoundToggle.tsx')]: 1,
  [join('WorkspacePanel', 'WorkspacePhonePushToggle.tsx')]: 1,
}

/** Hand-built switches and sliders in `src`, as `file:line <kind>`. */
function handBuiltControls(file: string, src: string, iconSwitches = 0): string[] {
  const at = (index: number): string => `${file}:${src.slice(0, index).split('\n').length}`
  const switches = [...src.matchAll(/\brole=["'{]+switch\b/g)].map((m) => `${at(m.index)} switch`)
  const sliders = [...src.matchAll(/\btype=["'{]+range\b/g)].map((m) => `${at(m.index)} slider`)
  // A drawn track and sliding knob with no switch role.
  const drawn = [...src.matchAll(/translate-x-(?:3\.5|4\.5)\b/g)].map((m) => `${at(m.index)} switch`)
  return [...(switches.length > iconSwitches ? switches : []), ...drawn, ...sliders]
}

describe('shared switches and sliders across the app', () => {
  const inScope = (rel: string): boolean => !rel.startsWith(`ui${sep}`) && !rel.startsWith(`Zen${sep}`)

  it('the guard catches a hand-built switch and slider, and allows a file its icon switches', () => {
    const sample = '<button role="switch" />\n<input\n  type="range"\n/>\n<Toggle checked={a} />'
    expect(handBuiltControls('sample.tsx', sample)).toEqual(['sample.tsx:1 switch', 'sample.tsx:3 slider'])
    expect(handBuiltControls('sample.tsx', sample, 1)).toEqual(['sample.tsx:3 slider'])
    expect(handBuiltControls('sample.tsx', '<a role="switch" /><a role="switch" />', 1)).toHaveLength(2)
  })

  it('has every listed icon switch still in place', () => {
    for (const [rel, count] of Object.entries(ICON_SWITCHES)) {
      expect([rel, handBuiltControls(rel, readFileSync(join(COMPONENTS, rel), 'utf8')).length]).toEqual([rel, count])
    }
  })

  it('has no hand-built role="switch" or type="range" outside components/ui', () => {
    const hits = tsxFiles(COMPONENTS)
      .map((f) => relative(COMPONENTS, f))
      .filter(inScope)
      .flatMap((rel) => handBuiltControls(rel, readFileSync(join(COMPONENTS, rel), 'utf8'), ICON_SWITCHES[rel] ?? 0))
    expect(hits).toEqual([])
  })
})
