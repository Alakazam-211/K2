// Rosson's bug (2026-11-04): from Settings → Workspaces, Home (⌘O) or the
// sidebar, picking a new folder made the folder but no workspace. The Add
// Workspace and Initialize git dialogs that follow the folder picker rendered
// at z-50, under Settings (200) and Home (150), so they opened hidden and the
// add never happened. These tests pin both dialogs to a layer above Settings
// and Home, the same way RemoveWorkspaceDialog is.

import { describe, it, expect } from 'vitest'
import { readFileSync } from 'fs'
import { resolve } from 'path'
import { ADD_WORKSPACE_Z, SETTINGS_OVERLAY_Z, HOST_PICKER_Z } from '@/lib/overlay-layers'

const src = (rel: string): string => readFileSync(resolve(__dirname, '..', rel), 'utf8')

describe('Add Workspace / Initialize git stacking', () => {
  it('the layer sits above Settings and Home, level with the folder picker', () => {
    expect(ADD_WORKSPACE_Z).toBeGreaterThan(SETTINGS_OVERLAY_Z)
    // Home (ZenRoot) paints at 150; read it from the source so a change there is caught.
    const zen = src('Zen/ZenRoot.tsx')
    const homeZ = [...zen.matchAll(/zIndex:\s*(\d+)/g)].map((m) => Number(m[1]))
    expect(homeZ.length).toBeGreaterThan(0)
    for (const z of homeZ) expect(ADD_WORKSPACE_Z).toBeGreaterThan(z)
    expect(ADD_WORKSPACE_Z).toBe(HOST_PICKER_Z)
  })

  for (const file of ['AddWorkspaceDialog/AddWorkspaceDialog.tsx', 'GitInitDialog/GitInitDialog.tsx']) {
    it(`${file} uses ADD_WORKSPACE_Z, not a hard-coded layer`, () => {
      const s = src(file)
      expect(s).toMatch(/zIndex=\{ADD_WORKSPACE_Z\}/)
      expect(s).not.toMatch(/zIndex=\{\d+\}/)
    })
  }
})
