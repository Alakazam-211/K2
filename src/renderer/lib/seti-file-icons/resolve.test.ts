import { describe, it, expect } from 'vitest'
import { resolveSetiIconId, resolveSetiIcon } from './resolve'
import { SETI_DEFAULT_ID, SETI_DEFAULT_ID_LIGHT, SETI_ICON_DEFS } from './seti-theme-data'

describe('resolveSetiIconId', () => {
  it('matches special file names (case-insensitive)', () => {
    expect(resolveSetiIconId('README.md')).toBe('_info')
    expect(resolveSetiIconId('package.json')).toBe('_npm_1')
    expect(resolveSetiIconId('Cargo.toml')).toBe('_rust')
  })

  it('matches compound then simple extensions', () => {
    expect(resolveSetiIconId('app.spec.ts')).toBe('_typescript_1')
    expect(resolveSetiIconId('app.ts')).toBe('_typescript')
    expect(resolveSetiIconId('styles.css.map')).toBe('_css')
    expect(resolveSetiIconId('main.rs')).toBe('_rust')
    expect(resolveSetiIconId('App.tsx')).toBe('_react')
  })

  it('falls back to default for unknown types', () => {
    expect(resolveSetiIconId('mystery.zzzzunknown')).toBe(SETI_DEFAULT_ID)
  })

  it('maps extensionless Docker/Make basenames', () => {
    expect(resolveSetiIconId('Dockerfile')).toBe('_docker')
    expect(resolveSetiIconId('Makefile')).toBe('_makefile')
  })

  it('uses light icon ids when scheme is light', () => {
    expect(resolveSetiIconId('app.ts', 'light')).toBe('_typescript_light')
    expect(resolveSetiIconId('README.md', 'light')).toBe('_info_light')
    expect(resolveSetiIconId('mystery.zzzzunknown', 'light')).toBe(
      SETI_DEFAULT_ID_LIGHT,
    )
  })
})

describe('resolveSetiIcon', () => {
  it('returns a glyph code and color', () => {
    const def = resolveSetiIcon('main.ts', 'dark')
    expect(def.code).toBeGreaterThan(0)
    expect(def.color).toMatch(/^#/)
  })

  it('light scheme uses different colors than dark for the same glyph', () => {
    const dark = resolveSetiIcon('main.ts', 'dark')
    const light = resolveSetiIcon('main.ts', 'light')
    expect(dark.code).toBe(light.code)
    expect(dark.color).not.toBe(light.color)
    expect(SETI_ICON_DEFS._typescript_light?.color).toBe(light.color)
  })

  it('handles empty name with default', () => {
    expect(resolveSetiIcon(undefined, 'dark').code).toBe(
      resolveSetiIcon('nope.zzzzunknown', 'dark').code,
    )
  })
})
