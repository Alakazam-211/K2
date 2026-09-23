import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const src = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), 'Settings.tsx'),
  'utf8',
)

describe('Settings nav Apps', () => {
  it('sidebar label is Apps, not Skin Access; route id stays skin-access', () => {
    expect(src).toContain("{ id: 'skin-access', label: 'Apps' }")
    expect(src).not.toContain("{ id: 'skin-access', label: 'Skin Access' }")
    expect(src).toContain("id: 'k2-access', label: 'Server Access'")
    expect(src).toContain("activeSection === 'skin-access'")
  })

  it('People comes immediately after Server Access and is not skin-access', () => {
    const serverStart = src.indexOf("title: 'K2 Server'")
    const serverEnd = src.indexOf("title: 'Editors / Fonts'")
    expect(serverStart).toBeGreaterThan(0)
    expect(serverEnd).toBeGreaterThan(serverStart)
    const server = src.slice(serverStart, serverEnd)
    expect(server).toContain("id: 'k2-access', label: 'Server Access'")
    expect(server).toContain("{ id: 'people', label: 'People' }")
    expect(server).toContain(
      "{ id: 'k2-access', label: 'Server Access' },\n        { id: 'people', label: 'People' },",
    )
    expect(server).not.toContain("id: 'skin-access'")
    expect(server).not.toContain("{ id: 'skin-access', label: 'People' }")
    expect(src).toContain("activeSection === 'people'")
    expect(src).toContain('<PeopleSection />')
    expect(src).not.toContain("{ id: 'k2-access', label: 'People' }")
  })
})
