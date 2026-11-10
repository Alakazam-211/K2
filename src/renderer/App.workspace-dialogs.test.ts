// Add and Remove Workspace go through their dialogs. The dialogs are
// mounted once, in App, and every "Add Workspace" / "Remove Workspace" in
// the sidebar, the icon rail and Settings opens them through their stores
// (the dialogs then add or remove; their own tests cover that). Source
// inspection: a render of App would need its whole mock surface to show
// that a JSX mount point exists.

import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = dirname(fileURLToPath(import.meta.url))
const src = (rel: string): string => readFileSync(join(root, rel), 'utf8')
const count = (s: string, needle: string): number => s.split(needle).length - 1

describe('Add / Remove Workspace dialogs', () => {
  it('App mounts each dialog exactly once', () => {
    const app = src('App.tsx')
    expect(count(app, '<AddWorkspaceDialog />')).toBe(1)
    expect(count(app, '<RemoveWorkspaceDialog />')).toBe(1)
  })

  it('the sidebar and the icon rail open both dialogs through their stores', () => {
    for (const file of ['components/Sidebar/Sidebar.tsx', 'components/Sidebar/IconRail.tsx']) {
      const s = src(file)
      expect(s, file).toContain('useAddWorkspaceDialogStore.getState().open({')
      expect(s, file).toContain('useRemoveWorkspaceDialogStore.getState().open({')
      // The removal itself is the dialog's, after the person picks a mode.
      expect(s, file).not.toMatch(/\bremoveProject\(/)
    }
  })

  it('Settings → Workspaces removes through the Remove dialog too', () => {
    const s = src('components/Settings/sections/ProjectsSection.tsx')
    expect(s).toContain('useRemoveWorkspaceDialogStore.getState().open({')
    expect(s).not.toMatch(/\bremoveProject\(/)
  })
})
