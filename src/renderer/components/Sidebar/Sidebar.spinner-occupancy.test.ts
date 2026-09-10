// Workspace-tile spinner occupancy — fail loud if idle vs working reflows
// the name row. AgentSpinner used to return null until working, then mount
// a text-[11px] braille glyph that grew the status row and jumped the name.

import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const dir = dirname(fileURLToPath(import.meta.url))
const sidebarSrc = readFileSync(join(dir, 'Sidebar.tsx'), 'utf8')
const tagsSrc = readFileSync(join(dir, 'NavProjectTags.tsx'), 'utf8')
const globalsSrc = readFileSync(join(dir, '../../globals.css'), 'utf8')

function sliceFn(src: string, name: string, until: string): string {
  const start = src.indexOf(`function ${name}`)
  if (start < 0) throw new Error(`missing function ${name}`)
  const end = src.indexOf(until, start + 1)
  if (end < 0) throw new Error(`missing terminator ${until} after ${name}`)
  return src.slice(start, end)
}

const agentSpinner = sliceFn(sidebarSrc, 'AgentSpinner', 'function NavWorktreeRow')
const singleProjectItem = sliceFn(sidebarSrc, 'SingleProjectItem', 'function WorkspaceButton')

describe('AgentSpinner occupancy', () => {
  it('always renders a 14px slot — never unmounts idle', () => {
    expect(agentSpinner).toContain('inline-flex h-3.5 w-3.5')
    expect(agentSpinner).toContain('leading-none')
    expect(agentSpinner).not.toMatch(/return null/)
    expect(agentSpinner).toMatch(/React\.JSX\.Element\s*\{/)
    expect(agentSpinner).not.toMatch(/React\.JSX\.Element\s*\|\s*null/)
  })

  it('only the glyph is conditional (working braille / review done / idle empty)', () => {
    expect(agentSpinner).toContain('braille-spinner')
    expect(agentSpinner).toContain('done')
    expect(agentSpinner).toMatch(/working \? \(/)
    expect(agentSpinner).toMatch(/review \? \(/)
  })
})

describe('SingleProjectItem reserved two-row template', () => {
  it('status row is always 14px even when empty — pin and unpin share this', () => {
    expect(singleProjectItem).toContain('h-3.5 min-h-3.5 leading-none')
    expect(singleProjectItem).not.toContain('-mt-px')
    expect(singleProjectItem).toContain('<AgentSpinner projectId={project.id} />')
  })

  it('name row uses leading-4 so it does not reflow when the spinner appears', () => {
    expect(singleProjectItem).toContain('w-full leading-4')
    expect(singleProjectItem).toContain('truncate flex-1 leading-4')
  })
})

describe('NavProjectTags chips fit the 14px status slot', () => {
  it('tagBox is h-3.5 leading-none', () => {
    expect(tagsSrc).toMatch(/const tagBox =\s*'[^']*h-3\.5[^']*leading-none/)
    expect(tagsSrc).not.toMatch(/const tagBox =\s*'[^']*leading-3/)
  })
})

describe('.braille-spinner cannot change layout across frames', () => {
  it('locks a 1em box with overflow hidden', () => {
    const block = globalsSrc.slice(globalsSrc.indexOf('.braille-spinner {'))
    const end = block.indexOf('.braille-spinner::after')
    if (end < 0) throw new Error('missing .braille-spinner element rule')
    const rule = block.slice(0, end)
    expect(rule).toMatch(/display:\s*inline-block/)
    expect(rule).toMatch(/width:\s*1em/)
    expect(rule).toMatch(/height:\s*1em/)
    expect(rule).toMatch(/line-height:\s*1/)
    expect(rule).toMatch(/overflow:\s*hidden/)
  })
})
