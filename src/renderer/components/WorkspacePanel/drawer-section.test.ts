// The drawers' collapsible section headers share one hover fill: the
// palette's wash-1 (the Apps header's). The Workspace drawer's headers use
// DRAWER_SECTION_HEADER and the Files drawer's FILES_SECTION_HEADER, both
// built on DRAWER_SECTION_HOVER, so no header can drift to its own fill.
// This source walk fails with the file name when a header goes its own way.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { DRAWER_SECTION_HEADER, DRAWER_SECTION_HOVER, FILES_SECTION_HEADER } from './drawer-section'

const COMPONENTS = join(__dirname, '..')
const read = (rel: string): string => readFileSync(join(COMPONENTS, rel), 'utf8')

// Each Workspace drawer section and its one header.
const WORKSPACE_HEADERS = [
  'HeartbeatsPanel/HeartbeatsPanel.tsx',
  'WorkspacePanel/ConnectedAgentsSection.tsx',
  'WorkspacePanel/AppsSection.tsx',
  'WorkspacePanel/UrlsPortsSection.tsx',
  'WorkspacePanel/WorkspaceApiSection.tsx',
  'WorkspacePanel/WorkspacePanel.tsx',
]

const count = (src: string, needle: string): number => src.split(needle).length - 1

describe('drawer section headers share one hover', () => {
  it('the hover is the wash-1 token', () => {
    expect(DRAWER_SECTION_HOVER).toBe('hover:bg-[var(--color-wash-1)]')
    expect(DRAWER_SECTION_HEADER.split(' ')).toContain(DRAWER_SECTION_HOVER)
    expect(FILES_SECTION_HEADER.split(' ')).toContain(DRAWER_SECTION_HOVER)
  })

  it('carries no other hover fill', () => {
    for (const cls of [DRAWER_SECTION_HEADER, FILES_SECTION_HEADER]) {
      expect(cls.split(' ').filter((c) => c.startsWith('hover:bg-'))).toEqual([DRAWER_SECTION_HOVER])
    }
  })

  for (const rel of WORKSPACE_HEADERS) {
    it(`${rel} uses the shared header`, () => {
      const src = read(rel)
      expect(count(src, 'className={DRAWER_SECTION_HEADER}'), rel).toBe(1)
      // The old one-off header fill (white at 2%) is gone.
      expect(src, rel).not.toContain('hover:bg-white/[0.02]')
      expect(src, rel).not.toContain('justify-between px-3 py-2 border-b')
    })
  }

  it('the Files drawer headers (Environment, Workspace Resources, AI Config) use the shared header', () => {
    const src = read('FileTree/FileTree.tsx')
    expect(count(src, 'className={FILES_SECTION_HEADER}')).toBe(3)
    expect(src).not.toContain('w-full h-[35px] px-3')
  })
})
