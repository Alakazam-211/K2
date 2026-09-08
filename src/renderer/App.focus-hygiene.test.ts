// Focus window drawer/chrome hygiene — fail loud for F2/F3/F4/F10
// (prd-focus-window-drawer-hygiene-v1 + vs-live F11–F18).
//
// Source-inspection: these locks are "do not remount / do not leak into
// shared chrome / first-caller notify" — a render tree would need the
// whole App mock surface and still miss a second JSX copy.

import { describe, expect, it } from 'vitest'
import { existsSync, readdirSync, readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const rendererRoot = dirname(fileURLToPath(import.meta.url))
const appSrc = readFileSync(join(rendererRoot, 'App.tsx'), 'utf8')
const focusLayoutSrc = readFileSync(
  join(rendererRoot, 'components/Layout/FocusLayout.tsx'),
  'utf8',
)
const desktopChromeLeftSrc = readFileSync(
  join(rendererRoot, 'components/TopBar/DesktopChromeLeft.tsx'),
  'utf8',
)
const panelsSrc = readFileSync(join(rendererRoot, 'stores/panels.ts'), 'utf8')
const pageTabsSrc = readFileSync(
  join(rendererRoot, 'components/TopBar/PageTabs.tsx'),
  'utf8',
)
const feedbackSrc = readFileSync(join(rendererRoot, 'stores/feedback.ts'), 'utf8')

function sliceFn(src: string, name: string, until: string | null): string {
  const start = src.indexOf(`function ${name}`)
  if (start < 0) throw new Error(`missing function ${name}`)
  if (until === null) return src.slice(start)
  const end = src.indexOf(until, start + 1)
  if (end < 0) throw new Error(`missing terminator ${until} after ${name}`)
  return src.slice(start, end)
}

function* walkTs(dir: string): Generator<string> {
  for (const ent of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, ent.name)
    if (ent.isDirectory()) yield* walkTs(p)
    else if (/\.(ts|tsx)$/.test(ent.name)) yield p
  }
}

describe('Focus window overlay hygiene', () => {
  it('F2: FocusModeContent does not remount overlays; FocusLayout still renders', () => {
    const focusMode = sliceFn(appSrc, 'FocusModeContent', 'function applyK2SOZoom')
    expect(focusMode).toContain('<FocusErrorBoundary>')
    expect(focusMode).toContain('<FocusLayout')
    expect(focusMode).toContain('<TerminalArea')
    const banned = [
      'RunningAgentsPanel',
      'FeedbackPage',
      'ProjectsPage',
      'WikiPage',
      'CommandPalette',
      'Toast',
      'GitInitDialog',
      'TransferProgress',
      'AssistantBar',
      'MemoDialog',
    ]
    for (const name of banned) {
      expect(focusMode, `F2: FocusModeContent must not mount ${name}`).not.toMatch(
        new RegExp(name),
      )
    }

    const appRoot = sliceFn(appSrc, 'AppRoot', null)
    expect(appRoot).toContain('<GitInitDialog />')
    expect(appRoot).toContain('<CommandPalette />')
    expect(appRoot).toContain('{!settingsOpen && <RunningAgentsPanel />}')
    expect(appRoot).toContain('{!settingsOpen && <FeedbackPage />}')
    expect(appRoot).toContain('{!settingsOpen && <ProjectsPage />}')
    expect(appRoot).toContain('{!settingsOpen && <WikiPage />}')
    expect(appRoot).toContain('<Toast />')
    expect(appRoot).toContain('<TransferProgress />')
    expect(appRoot).toContain('<AssistantBar />')
    expect(appRoot).toContain('<MemoDialog />')
    expect(appRoot).toContain("mode={settingsOpen ? 'button-only' : undefined}")
    expect(appRoot).toContain('<AgentCloseDialog')
  })

  it('F3/F18: WorktreeBar import gone and file deleted; no remaining importers', () => {
    expect(appSrc).not.toMatch(/WorktreeBar/)
    const barPath = join(rendererRoot, 'components/FocusWindow/WorktreeBar.tsx')
    expect(existsSync(barPath), 'WorktreeBar.tsx must be deleted').toBe(false)
    expect(existsSync(join(rendererRoot, 'components/FocusWindow/FocusWorkspaceHeader.tsx'))).toBe(true)

    const importers: string[] = []
    for (const file of walkTs(rendererRoot)) {
      if (file.endsWith('.test.ts') || file.endsWith('.test.tsx')) continue
      const src = readFileSync(file, 'utf8')
      if (/from ['"][^'"]*WorktreeBar['"]/.test(src) || /import WorktreeBar/.test(src)) {
        importers.push(file)
      }
    }
    expect(importers, `remaining WorktreeBar importers: ${importers.join(', ')}`).toEqual([])
  })

  it('FocusLayout still exposes left/right panel toggles; panel tab set unchanged', () => {
    expect(focusLayoutSrc).toContain('toggleLeftPanel')
    expect(focusLayoutSrc).toContain('toggleRightPanel')
    expect(focusLayoutSrc).toContain('title="Toggle left panel"')
    expect(focusLayoutSrc).toContain('title="Toggle right panel"')
    expect(panelsSrc).toMatch(/type PanelTab = 'files' \| 'changes' \| 'history' \| 'workspace'/)
  })

  it('F4/F13: ServerSwitcher lives in FocusLayout only, not DesktopChromeLeft; no PageTabs', () => {
    expect(focusLayoutSrc).toMatch(/import ServerSwitcher from/)
    expect(focusLayoutSrc).toContain('<ServerSwitcher />')
    expect(focusLayoutSrc).not.toMatch(/PageTabs/)
    expect(focusLayoutSrc).not.toMatch(/RunningAgents/)
    expect(focusLayoutSrc).not.toMatch(/Tickets/)
    expect(desktopChromeLeftSrc).not.toMatch(/ServerSwitcher/)
    expect(desktopChromeLeftSrc).not.toMatch(/PageTabs/)
  })

  it('F10/F12: AppRoot inits both buses; notify is label === main, never hardcoded false', () => {
    const appRoot = sliceFn(appSrc, 'AppRoot', null)
    expect(appRoot).toContain('initFeedbackEvents(notify)')
    expect(appRoot).toContain('initProjectGroupEvents()')
    expect(appRoot).toMatch(/\.label === 'main'/)
    expect(appRoot).toMatch(/let notify = true/)
    expect(appRoot).not.toMatch(/initFeedbackEvents\(false\)/)
    expect(appRoot).not.toMatch(/initFeedbackEvents\(true\)/)

    // PageTabs may keep inits as no-ops (first caller wins).
    expect(pageTabsSrc).toContain('initFeedbackEvents(isMain)')
    expect(pageTabsSrc).toContain('initProjectGroupEvents()')

    // Re-calling must not flip notify.
    expect(feedbackSrc).toMatch(/if \(eventsInitialized\) return/)
  })
})
