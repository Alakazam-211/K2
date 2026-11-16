// @vitest-environment jsdom
// The editors' default theme, "Follow app" (build-editors.md E1a): every
// colour comes from the look's --color-* tokens, so it follows Terminal and
// Glass, dark and light, with no reconfigure. The eight named themes stay as
// opt-in choices (in their own file), the old default id moves to Follow app,
// and the live editor and the editor that replaces it when live editing ends
// draw with the same theme.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, waitFor } from '@testing-library/react'
import { EditorView } from '@codemirror/view'
import { EditorState, type Extension } from '@codemirror/state'
import { defaultHighlightStyle } from '@codemirror/language'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => {
    throw new Error('not a git repo')
  }),
  daemonCliPost: vi.fn(async () => ({})),
  daemonCliPostQuery: vi.fn(async () => ({})),
}))
// The daemon's settings route echoes what it stored.
const daemon = vi.hoisted(() => ({ updates: [] as Record<string, unknown>[] }))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({})),
  settingsReset: vi.fn(async () => ({})),
  settingsUpdate: vi.fn(async (updates: Record<string, unknown>) => {
    daemon.updates.push(updates)
    return { keybindings: {}, ...updates }
  }),
}))
// The live view's kit is not under test: capture what the pane hands it.
const live = vi.hoisted(() => ({ extensions: null as unknown }))
vi.mock('@/components/FileViewerPane/LiveDocView', () => ({
  LiveDocView: (props: { extensions: unknown }) => {
    live.extensions = props.extensions
    return <div data-testid="live-doc-view" />
  },
}))

import {
  EDITOR_THEMES,
  FOLLOW_APP_HIGHLIGHT,
  FOLLOW_APP_THEME_ID,
  FOLLOW_APP_THEME_SPEC,
  getHighlightExtension,
  getThemeExtension,
  resolveEditorThemeId,
} from './theme'
import { NAMED_EDITOR_THEMES } from './named-themes'
import { useSettingsStore } from '@/stores/settings'
import { CodeEditor } from '@/components/FileViewerPane/CodeEditor'
import { LiveDocPane } from '@/components/FileViewerPane/LiveDocPane'
import { CSS_VARS, LIVE_BASE_THEME_SPEC } from '@/components/LiveEditor/theme'
import { primaryScope } from '@/kessel/server-scope'

const RENDERER = join(__dirname, '..', '..')

/** Every string value in a theme spec, with the selector path that holds it. */
function values(spec: Record<string, unknown>, path = ''): [string, string][] {
  const out: [string, string][] = []
  for (const [k, v] of Object.entries(spec)) {
    if (typeof v === 'string') out.push([`${path} ${k}`, v])
    else if (v && typeof v === 'object') out.push(...values(v as Record<string, unknown>, `${path} ${k}`))
  }
  return out
}

const HARD_COLOUR = /#[0-9a-f]{3,8}\b|rgba?\(|hsla?\(|\b(white|black|Canvas|CanvasText)\b/i

/** The theme classes an extension puts on the editor (beyond a bare editor's). */
function themeClasses(ext: Extension): string[] {
  const bare = new EditorView({ state: EditorState.create({ doc: '' }) })
  const themed = new EditorView({ state: EditorState.create({ doc: '', extensions: ext }) })
  const out = [...themed.dom.classList].filter((c) => !bare.dom.classList.contains(c))
  bare.destroy()
  themed.destroy()
  return out
}

/** The class CodeMirror's highlighter gives `const` in a TypeScript line. */
function keywordClass(root: ParentNode): string {
  const span = Array.from(root.querySelectorAll('.cm-line span')).find((s) => s.textContent === 'const')
  return span?.className ?? '(no keyword span)'
}

// jsdom has no layout: CodeMirror's measuring asks a Range for its boxes.
const noRects = (): DOMRectList => Object.assign([], { item: () => null }) as unknown as DOMRectList
Range.prototype.getClientRects ??= noRects
Range.prototype.getBoundingClientRect ??= () => new DOMRect()

beforeEach(() => {
  live.extensions = null
})
afterEach(() => cleanup())

describe('resolveEditorThemeId', () => {
  it('moves the old default and an unset theme to Follow app', () => {
    expect(FOLLOW_APP_THEME_ID).toBe('follow-app')
    for (const id of [undefined, null, '', 'k2so-dark', 'follow-app']) expect(resolveEditorThemeId(id)).toBe('follow-app')
  })

  it('keeps a named theme, the renamed K2 Dark and a custom theme', () => {
    for (const id of ['k2-dark', 'one-dark', 'dracula', 'nord', 'github-dark', 'gruvbox-dark', 'ayu-mirage', 'parchment', 'custom:midnight']) {
      expect(resolveEditorThemeId(id)).toBe(id)
    }
  })

  it('an id K2 no longer knows follows the app', () => {
    expect(resolveEditorThemeId('solarized-ish')).toBe('follow-app')
  })

  it('a fresh client starts on Follow app', () => {
    expect(useSettingsStore.getState().editor.theme).toBe('follow-app')
  })

  it('the choice persists through the daemon and comes back as stored', async () => {
    await useSettingsStore.getState().updateEditorSettings({ theme: 'nord' })
    expect(daemon.updates.at(-1)).toEqual({ editor: expect.objectContaining({ theme: 'nord' }) })
    expect(useSettingsStore.getState().editor.theme).toBe('nord')
    await useSettingsStore.getState().updateEditorSettings({ theme: 'follow-app' })
    expect(daemon.updates.at(-1)).toEqual({ editor: expect.objectContaining({ theme: 'follow-app' }) })
    expect(useSettingsStore.getState().editor.theme).toBe('follow-app')
  })
})

describe('Follow app theme', () => {
  it('reads only the look tokens: no fixed colour anywhere', () => {
    const hard = values(FOLLOW_APP_THEME_SPEC as Record<string, unknown>).filter(([, v]) => HARD_COLOUR.test(v))
    expect(hard).toEqual([])
  })

  it('takes each part of the editor from its token', () => {
    const s = FOLLOW_APP_THEME_SPEC as Record<string, Record<string, string>>
    expect(s['&'].backgroundColor).toContain('var(--color-bg)')
    expect(s['&'].color).toBe('var(--color-text-primary)')
    expect(s['.cm-gutters'].backgroundColor).toBe(s['&'].backgroundColor)
    expect(s['.cm-gutters'].color).toBe('var(--color-text-muted)')
    expect(s['.cm-activeLineGutter'].color).toBe('var(--color-text-primary)')
    expect(s['.cm-activeLine'].backgroundColor).toMatch(/var\(--color-/)
    expect(s['.cm-selectionBackground, ::selection'].backgroundColor).toContain('var(--color-selection)')
    expect(s['.cm-searchMatch'].backgroundColor).toBe('var(--color-find-match)')
    expect(s['.cm-searchMatch.cm-searchMatch-selected'].backgroundColor).toBe('var(--color-find-match-current)')
    expect(s['&'].caretColor ?? s['.cm-content'].caretColor).toContain('var(--color-accent-text)')
  })

  it('colours code from the syntax tokens', () => {
    for (const spec of FOLLOW_APP_HIGHLIGHT) {
      if (spec.color) expect(String(spec.color)).toMatch(/^var\(--color-(syntax-[a-z]+|text-[a-z]+|accent-text|code-inline|status-error-text)\)$/)
    }
    const byTag = (t: unknown): string | undefined => FOLLOW_APP_HIGHLIGHT.find((s) => ([] as unknown[]).concat(s.tag).includes(t))?.color as string | undefined
    // The defaults' coloured tags are all covered, so CodeMirror's fixed
    // fallback palette never paints in this theme.
    for (const spec of defaultHighlightStyle.specs) {
      if (!spec.color) continue
      for (const t of ([] as unknown[]).concat(spec.tag)) expect(byTag(t), String(t)).toBeDefined()
    }
  })

  it('needs no reconfigure to follow the look: one extension for every look', () => {
    expect(getThemeExtension('follow-app')).toBe(getThemeExtension('follow-app'))
    expect(getHighlightExtension('follow-app')).toBe(getHighlightExtension('k2so-dark'))
  })
})

describe('Selected code', () => {
  it('keeps its own colours: the Terminal field-selection ink leaves CodeMirror out', () => {
    const css = readFileSync(join(RENDERER, 'globals.css'), 'utf8')
    const rule = css.split('\n').filter((l) => l.startsWith('html[data-style="terminal"]') && l.includes('contenteditable') && l.includes('::selection'))
    expect(rule.length).toBeGreaterThan(0)
    for (const sel of rule) expect(sel).toContain(':not(.cm-content)')
  })
})

describe('Named themes', () => {
  it('Follow app comes first, then the eight named themes', () => {
    expect(EDITOR_THEMES[0]).toEqual({ id: 'follow-app', label: 'Follow app (default)' })
    expect(EDITOR_THEMES.slice(1).map((t) => t.id)).toEqual(['k2-dark', 'one-dark', 'dracula', 'nord', 'github-dark', 'gruvbox-dark', 'ayu-mirage', 'parchment'])
    expect(NAMED_EDITOR_THEMES).toHaveLength(8)
  })

  it('each still loads, and each draws differently from Follow app', () => {
    const follow = themeClasses(getThemeExtension('follow-app')).join(' ')
    expect(follow).not.toBe('')
    for (const { id } of NAMED_EDITOR_THEMES) {
      const cls = themeClasses(getThemeExtension(id)).join(' ')
      expect(cls, id).not.toBe('')
      expect(cls, id).not.toBe(follow)
      expect(getHighlightExtension(id), id).toBeTruthy()
    }
  })

  it('live outside CodeEditor.tsx, which no longer carries their tables', () => {
    const src = readFileSync(join(RENDERER, 'components', 'FileViewerPane', 'CodeEditor.tsx'), 'utf8')
    expect(src).not.toMatch(/THEME_COLORS|THEME_HIGHLIGHTS|'dracula'|#282a36/)
  })
})

describe('Live editor theme', () => {
  it('maps every --mpc-* colour to a look token', () => {
    for (const name of ['--mpc-bg', '--mpc-fg', '--mpc-rejected', '--mpc-banner-bg', '--mpc-banner-fg', '--mpc-focus'] as const) {
      expect(CSS_VARS[name], name).toMatch(/var\(--color-/)
      expect(CSS_VARS[name], name).not.toMatch(HARD_COLOUR)
    }
  })

  it('scopes its base theme to the live editor, so a later plain editor never inherits it', () => {
    for (const key of Object.keys(LIVE_BASE_THEME_SPEC)) expect(key, key).toMatch(/^&\.mpc-cm\b/)
  })
})

describe('The live editor and the editor that replaces it use the same theme', () => {
  it.each(['k2so-dark', 'follow-app', 'dracula'])('setting %s', async (theme) => {
    useSettingsStore.setState((s) => ({ editor: { ...s.editor, theme } }))
    const doc = 'const x = 1\n'

    // The live view, as the file tab mounts it.
    render(<LiveDocPane scope={primaryScope()} filePath="/w/src/x.ts" core="text" />)
    expect(live.extensions).not.toBeNull()
    const liveView = new EditorView({ state: EditorState.create({ doc, extensions: live.extensions as Extension }) })
    const liveClasses = [...liveView.dom.classList]
    const liveKeyword = keywordClass(liveView.dom)
    liveView.destroy()
    cleanup()

    // The editor shown once live editing is off.
    const { container } = render(<CodeEditor code={doc} filePath="/w/src/x.ts" onSave={() => {}} onChange={() => {}} />)
    await waitFor(() => expect(container.querySelector('.cm-editor')).toBeTruthy())
    const plainClasses = [...container.querySelector('.cm-editor')!.classList]

    const want = themeClasses(getThemeExtension(resolveEditorThemeId(theme)))
    for (const c of want) {
      expect(liveClasses, `live editor has ${c}`).toContain(c)
      expect(plainClasses, `live-off editor has ${c}`).toContain(c)
    }
    expect(keywordClass(container)).toBe(liveKeyword)
    expect(liveKeyword).not.toBe('(no keyword span)')
  })
})
