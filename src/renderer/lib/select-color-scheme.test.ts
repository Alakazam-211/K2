// @vitest-environment jsdom
// Dark-theme native <select> off macOS (0.45.6 Linux check): the rules in
// globals.css apply on Linux/Windows only, follow the app scheme, and leave
// macOS untouched.
import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

const css = readFileSync(resolve(__dirname, '../globals.css'), 'utf8')
const entry = readFileSync(resolve(__dirname, '../index.tsx'), 'utf8')

/** The selector list in front of the first `{` that contains `needle`. */
function ruleFor(needle: string): { selector: string; body: string } {
  const at = css.indexOf(needle)
  expect(at, `globals.css has a rule with ${needle}`).toBeGreaterThan(-1)
  const open = css.indexOf('{', at)
  const close = css.indexOf('}', open)
  const start = css.lastIndexOf('}', at) + 1
  const selector = css.slice(start, open).replace(/\/\*[\s\S]*?\*\//g, '').trim()
  return { selector, body: css.slice(open + 1, close) }
}

function matches(selector: string, os: string, scheme: string, el: 'select' | 'option'): boolean {
  document.documentElement.setAttribute('data-os', os)
  document.documentElement.setAttribute('data-scheme', scheme)
  document.body.innerHTML = '<select><option>a</option></select>'
  const node = document.querySelector(el)
  if (!node) throw new Error(`no ${el}`)
  return node.matches(selector)
}

describe('native select theming off macOS', () => {
  it('index.tsx stamps data-os from the shared OS check', () => {
    expect(entry).toContain("setAttribute('data-os', desktopOsFromNavigator(navigator))")
  })

  it('dark scheme gives selects color-scheme dark on Linux and Windows, never on macOS', () => {
    const dark = ruleFor('[data-scheme="dark"] select')
    expect(dark.body).toContain('color-scheme: dark')
    expect(matches(dark.selector, 'linux', 'dark', 'select')).toBe(true)
    expect(matches(dark.selector, 'windows', 'dark', 'select')).toBe(true)
    expect(matches(dark.selector, 'mac', 'dark', 'select')).toBe(false)
    expect(matches(dark.selector, 'linux', 'light', 'select')).toBe(false)

    const light = ruleFor('[data-scheme="light"] select')
    expect(light.body).toContain('color-scheme: light')
    expect(matches(light.selector, 'linux', 'light', 'select')).toBe(true)
    expect(matches(light.selector, 'mac', 'light', 'select')).toBe(false)
  })

  it('options take theme tokens off macOS only', () => {
    const opt = ruleFor('select option')
    expect(opt.body).toContain('background-color: var(--color-bg-elevated)')
    expect(opt.body).toContain('color: var(--color-text-primary)')
    expect(matches(opt.selector, 'linux', 'dark', 'option')).toBe(true)
    expect(matches(opt.selector, 'mac', 'dark', 'option')).toBe(false)
  })
})
