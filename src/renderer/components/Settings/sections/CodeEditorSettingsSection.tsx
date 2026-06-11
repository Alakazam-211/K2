import React from 'react'
import { useCallback, useMemo, useState } from 'react'
import { useSettingsStore } from '@/stores/settings'
import { useCustomThemesStore } from '@/stores/custom-themes'
import { EDITOR_THEMES, EDITOR_FONTS, CodeEditor } from '@/components/FileViewerPane/CodeEditor'
import { CustomThemeCreator } from '../CustomThemeCreator'
import { SectionErrorBoundary } from '../SectionErrorBoundary'
import { SettingDropdown } from '../controls/SettingControls'
import type { SettingEntry } from '../searchManifest'

export const CODE_EDITOR_MANIFEST: SettingEntry[] = [
  // Appearance
  { id: 'code-editor.theme', section: 'code-editor', group: 'Appearance', label: 'Theme', description: 'Color theme for the code editor', keywords: ['theme', 'colors', 'appearance', 'dark mode', 'light mode'] },
  { id: 'code-editor.font-family', section: 'code-editor', group: 'Appearance', label: 'Font Family', description: 'Monospace font for code editing', keywords: ['font', 'typeface', 'family'] },
  { id: 'code-editor.font-size', section: 'code-editor', group: 'Appearance', label: 'Font Size', description: 'Editor text size in pixels', keywords: ['font', 'size', 'zoom'] },
  { id: 'code-editor.font-ligatures', section: 'code-editor', group: 'Appearance', label: 'Font Ligatures', description: 'Programming ligatures (e.g. => becomes arrow)', keywords: ['ligatures', 'font', 'arrow', 'fira', 'jetbrains'] },
  { id: 'code-editor.cursor-style', section: 'code-editor', group: 'Appearance', label: 'Cursor Style', description: 'Bar / block / underline', keywords: ['cursor', 'caret'] },
  { id: 'code-editor.cursor-blink', section: 'code-editor', group: 'Appearance', label: 'Cursor Blink', description: 'Animate the cursor blinking', keywords: ['cursor', 'blink'] },
  // Editing
  { id: 'code-editor.tab-size', section: 'code-editor', group: 'Editing', label: 'Tab Size', description: 'Default spaces per indentation', keywords: ['tab', 'indent', 'spaces', 'size'] },
  { id: 'code-editor.word-wrap', section: 'code-editor', group: 'Editing', label: 'Word Wrap', description: 'Wrap long lines instead of horizontal scroll', keywords: ['wrap', 'word', 'lines'] },
  { id: 'code-editor.autocomplete', section: 'code-editor', group: 'Editing', label: 'Autocomplete', description: 'Word-based completion suggestions as you type', keywords: ['autocomplete', 'complete', 'suggestions', 'intellisense'] },
  { id: 'code-editor.bracket-matching', section: 'code-editor', group: 'Editing', label: 'Bracket Matching', description: 'Highlight matching brackets', keywords: ['brackets', 'match', 'pair'] },
  { id: 'code-editor.format-on-save', section: 'code-editor', group: 'Editing', label: 'Format on Save', description: 'Auto-format with Prettier/rustfmt/black on ⌘S', keywords: ['format', 'save', 'prettier', 'rustfmt', 'black'] },
  { id: 'code-editor.show-whitespace', section: 'code-editor', group: 'Editing', label: 'Show Whitespace', description: 'Render spaces and tabs as visible dots', keywords: ['whitespace', 'spaces', 'tabs', 'invisible'] },
  // Gutter & Display
  { id: 'code-editor.line-numbers', section: 'code-editor', group: 'Gutter & Display', label: 'Line Numbers', description: 'Show line numbers in the gutter', keywords: ['line numbers', 'gutter', 'numbers'] },
  { id: 'code-editor.indent-guides', section: 'code-editor', group: 'Gutter & Display', label: 'Indent Guides', description: 'Vertical indentation guide lines', keywords: ['indent', 'guides', 'rulers'] },
  { id: 'code-editor.fold-gutter', section: 'code-editor', group: 'Gutter & Display', label: 'Code Folding', description: 'Show fold/unfold arrows in the gutter', keywords: ['fold', 'collapse', 'gutter'] },
  { id: 'code-editor.highlight-active-line', section: 'code-editor', group: 'Gutter & Display', label: 'Highlight Active Line', description: 'Subtle background on the current line', keywords: ['highlight', 'active', 'current line'] },
  { id: 'code-editor.scroll-past-end', section: 'code-editor', group: 'Gutter & Display', label: 'Scroll Past End', description: 'Allow scrolling beyond the last line', keywords: ['scroll', 'past end', 'bottom'] },
  { id: 'code-editor.minimap', section: 'code-editor', group: 'Gutter & Display', label: 'Minimap', description: 'Miniature overview of the file on the right', keywords: ['minimap', 'overview', 'scroll preview'] },
  { id: 'code-editor.diff-style', section: 'code-editor', group: 'Gutter & Display', label: 'Diff Style', description: 'Gutter marks vs inline PR-style diffs', keywords: ['diff', 'changes', 'pr view'] },
  { id: 'code-editor.scrollbar-annotations', section: 'code-editor', group: 'Gutter & Display', label: 'Scrollbar Annotations', description: 'Colored markers on the scrollbar where code changed', keywords: ['scrollbar', 'annotations', 'markers'] },
  { id: 'code-editor.sticky-scroll', section: 'code-editor', group: 'Gutter & Display', label: 'Sticky Scroll', description: 'Pin current function/class header at the top', keywords: ['sticky', 'scroll', 'breadcrumbs', 'header'] },
  // Keybindings & Modes
  { id: 'code-editor.vim-mode', section: 'code-editor', group: 'Keybindings', label: 'Vim Mode', description: 'Full vim keybinding emulation (hjkl, modes, commands)', keywords: ['vim', 'modal', 'hjkl', 'keybindings'] },
]

export function CodeEditorSettingsSection(): React.JSX.Element {
  const editor = useSettingsStore((s) => s.editor)
  const updateEditorSettings = useSettingsStore((s) => s.updateEditorSettings)
  const customThemes = useCustomThemesStore((s) => s.customThemes)
  const [showCreator, setShowCreator] = useState(false)
  const [editingThemePath, setEditingThemePath] = useState<string | undefined>(undefined)
  const [showThemeManager, setShowThemeManager] = useState(false)
  const isCustomTheme = editor.theme.startsWith('custom:')
  const deleteCustomTheme = useCustomThemesStore((s) => s.deleteCustomTheme)

  const LIGATURE_FONTS = new Set(['Fira Code', 'JetBrains Mono', 'Lilex'])
  const fontSupportsLigatures = LIGATURE_FONTS.has(editor.fontFamily)

  // Build combined theme list: built-in + custom
  const allThemeOptions = useMemo(() => {
    const builtIn = EDITOR_THEMES.map(t => ({ value: t.id, label: t.label }))
    const custom = customThemes.map(t => ({ value: t.id, label: `${t.name}` }))
    if (custom.length > 0) {
      return [...builtIn, { value: '__divider__', label: '── Custom ──' }, ...custom]
    }
    return builtIn
  }, [customThemes])

  // Open creator for an existing custom theme
  const handleCustomize = useCallback(() => {
    const theme = customThemes.find((t) => t.id === editor.theme)
    setEditingThemePath(theme?.path)
    setShowCreator(true)
  }, [customThemes, editor.theme])

  // Open creator for a brand new theme
  const handleNewTheme = useCallback(() => {
    setEditingThemePath(undefined)
    setShowCreator(true)
  }, [])

  const toggleRow = (label: string, description: string, key: keyof typeof editor, isLast = false, disabled = false) => (
    <div key={key} className={`flex items-center justify-between px-3 py-2.5 ${!isLast ? 'border-b border-[var(--color-border)]' : ''} ${disabled ? 'opacity-40' : ''}`}>
      <div>
        <div className="text-xs text-[var(--color-text-secondary)]">{label}</div>
        <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">{description}</div>
      </div>
      <button
        onClick={() => { if (!disabled) updateEditorSettings({ [key]: !editor[key] }) }}
        className={`w-7 h-3.5 flex items-center transition-colors flex-shrink-0 ${disabled ? 'cursor-default' : 'no-drag cursor-pointer'} ${
          editor[key] && !disabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
        }`}
      >
        <span className={`w-2.5 h-2.5 bg-white block transition-transform ${
          editor[key] && !disabled ? 'translate-x-3.5' : 'translate-x-0.5'
        }`} />
      </button>
    </div>
  )

  const dropdownRow = (label: string, description: string, value: string, options: { value: string; label: string }[], onChange: (v: string) => void, isLast = false) => (
    <div className={`flex items-center justify-between px-3 py-2.5 ${!isLast ? 'border-b border-[var(--color-border)]' : ''}`}>
      <div>
        <div className="text-xs text-[var(--color-text-secondary)]">{label}</div>
        <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">{description}</div>
      </div>
      <SettingDropdown value={value} options={options} onChange={onChange} />
    </div>
  )

  // Sample code for the live preview — K2 coding in a galaxy far, far away
  const previewCode = `import { useState, useEffect, useCallback } from 'react'

// K2 Security Droid — Imperial Data Vault Access Module
// "I find that answer vague and unconvincing." — K2

interface SecurityProtocol {
  clearanceLevel: 'rebel' | 'imperial' | 'classified'
  accessCode: string
  probabilityOfSuccess: number
  isStealthMode?: boolean
}

type MissionStatus = 'infiltrating' | 'compromised' | 'success' | 'told-you-so'

/**
 * Calculates the survival odds for a given mission.
 * Spoiler: they're never good enough for K2's standards.
 */
export function calculateSurvivalOdds(
  crew: string[],
  hasForceSensitive: boolean,
  imperialPresence: number
): { odds: number; commentary: string } {
  const baseOdds = 100 - (imperialPresence * 12.7)
  const crewBonus = crew.length * 3.2
  const forceMultiplier = hasForceSensitive ? 1.47 : 0.89

  const finalOdds = Math.min(
    97.6,
    Math.max(0, (baseOdds + crewBonus) * forceMultiplier)
  )

  // K2 always has something to say about the odds
  const commentary =
    finalOdds > 80 ? "Acceptable. I still don't like it." :
    finalOdds > 50 ? "I have a bad feeling about this." :
    finalOdds > 20 ? "Would you like to know the probability of failure?" :
    "I'm not very optimistic about our chances."

  return { odds: Math.round(finalOdds * 100) / 100, commentary }
}

export function useImperialVault(protocol: SecurityProtocol) {
  const [status, setStatus] = useState<MissionStatus>('infiltrating')
  const [dataStolen, setDataStolen] = useState<string[]>([])
  const [alarmTriggered, setAlarmTriggered] = useState(false)

  // Attempt to slice into the Imperial network
  const sliceTerminal = useCallback(async (terminalId: string) => {
    if (protocol.probabilityOfSuccess < 32.5) {
      setStatus('told-you-so')
      return { success: false, message: "I told you this would happen." }
    }

    try {
      const deathStarPlans = await fetchClassifiedData(terminalId)
      setDataStolen((prev) => [...prev, ...deathStarPlans])
      setStatus('success')
      return { success: true, message: "The plans are in the droid." }
    } catch {
      setAlarmTriggered(true)
      setStatus('compromised')
      return { success: false, message: "There are a lot of them." }
    }
  }, [protocol.probabilityOfSuccess])

  // Monitor for Stormtroopers (they never check behind crates)
  useEffect(() => {
    if (!protocol.isStealthMode) return

    const patrol = setInterval(() => {
      const detected = Math.random() > 0.85
      if (detected && !alarmTriggered) {
        setAlarmTriggered(true)
        setStatus('compromised')
        console.warn('[K2] Congratulations. You are being rescued.')
      }
    }, 5000)

    return () => clearInterval(patrol)
  }, [protocol.isStealthMode, alarmTriggered])

  return { status, dataStolen, alarmTriggered, sliceTerminal }
}

async function fetchClassifiedData(id: string): Promise<string[]> {
  // "Quiet! And there is a fresh one if you mouth off again."
  const response = await fetch(\`/api/imperial/\${id}/plans\`)
  if (!response.ok) throw new Error('Access denied. Probably.')
  return response.json()
}
`

  // Demo diff data: K2's latest code review changes
  const demoChanges = useMemo(() => {
    const m = new Map<number, 'added' | 'modified' | 'deleted'>()
    m.set(11, 'added')
    m.set(14, 'modified')
    m.set(21, 'added')
    m.set(22, 'added')
    m.set(23, 'added')
    m.set(24, 'added')
    m.set(25, 'added')
    m.set(30, 'modified')
    m.set(31, 'modified')
    m.set(37, 'added')
    m.set(38, 'added')
    m.set(39, 'added')
    m.set(40, 'added')
    m.set(55, 'deleted')
    m.set(73, 'modified')
    m.set(74, 'modified')
    m.set(75, 'modified')
    m.set(76, 'modified')
    m.set(79, 'added')
    return m
  }, [])

  if (showCreator) {
    return (
      <SectionErrorBoundary>
        <div className="absolute inset-0 overflow-hidden bg-[var(--color-bg)]">
          <CustomThemeCreator
            currentThemeId={editor.theme}
            existingThemePath={editingThemePath}
            onClose={() => setShowCreator(false)}
          />
        </div>
      </SectionErrorBoundary>
    )
  }

  return (
    <div className="flex gap-6">
      {/* Settings panel */}
      <div className="max-w-xl flex-1 space-y-6 min-w-0">
        {/* ── Appearance ── */}
        <div>
          <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-3">Appearance</h2>
          <div className="border border-[var(--color-border)]">
            <div className="flex items-center justify-between px-3 py-2.5 border-b border-[var(--color-border)]">
              <div>
                <div className="text-xs text-[var(--color-text-secondary)]">Theme</div>
                <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">Color theme for the code editor</div>
              </div>
              <div className="flex items-center gap-1.5">
                {customThemes.length > 0 && (
                  <button
                    onClick={() => setShowThemeManager(!showThemeManager)}
                    className="px-2 py-1 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-secondary)] transition-colors cursor-pointer no-drag"
                  >
                    Manage
                  </button>
                )}
                {isCustomTheme && (
                  <button
                    onClick={handleCustomize}
                    className="px-2 py-1 text-[10px] text-[var(--color-accent)] border border-[var(--color-accent)]/30 hover:bg-[var(--color-accent)]/10 transition-colors cursor-pointer no-drag"
                  >
                    Customize
                  </button>
                )}
                <SettingDropdown
                  value={editor.theme}
                  options={allThemeOptions.filter(o => o.value !== '__divider__')}
                  onChange={(v) => updateEditorSettings({ theme: v })}
                />
                <button
                  onClick={handleNewTheme}
                  className="px-2 py-1 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-secondary)] transition-colors cursor-pointer no-drag whitespace-nowrap"
                >
                  + New
                </button>
              </div>
            </div>
            {showThemeManager && customThemes.length > 0 && (
              <div className="border-b border-[var(--color-border)] bg-[var(--color-bg)]/50">
                <div className="px-3 py-2">
                  <div className="text-[10px] text-[var(--color-text-muted)] uppercase tracking-wider mb-2">Custom Themes</div>
                  {customThemes.map((t) => (
                    <div key={t.id} className="flex items-center justify-between py-1.5 group">
                      <div className="flex items-center gap-2 min-w-0">
                        <span className="w-3 h-3 flex-shrink-0 border border-[var(--color-border)]" style={{ backgroundColor: t.colors.bg }} />
                        <span className="text-xs text-[var(--color-text-primary)] truncate">{t.name}</span>
                        {editor.theme === t.id && (
                          <span className="text-[9px] text-[var(--color-accent)] flex-shrink-0">active</span>
                        )}
                      </div>
                      <div className="flex items-center gap-1.5 opacity-0 group-hover:opacity-100 transition-opacity">
                        <button
                          onClick={() => {
                            setEditingThemePath(t.path)
                            setShowCreator(true)
                            setShowThemeManager(false)
                          }}
                          className="px-1.5 py-0.5 text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] cursor-pointer no-drag"
                        >
                          Edit
                        </button>
                        <button
                          onClick={async () => {
                            if (editor.theme === t.id) {
                              updateEditorSettings({ theme: 'k2so-dark' })
                            }
                            await deleteCustomTheme(t.id)
                          }}
                          className="px-1.5 py-0.5 text-[10px] text-[var(--color-text-muted)] hover:text-red-400 cursor-pointer no-drag"
                        >
                          Delete
                        </button>
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            )}
            {dropdownRow('Font Family', 'Monospace font for code editing', editor.fontFamily,
              EDITOR_FONTS.map(f => ({ value: f.id, label: f.label })),
              (v) => updateEditorSettings({ fontFamily: v })
            )}
            {dropdownRow('Font Size', 'Editor text size in pixels', String(editor.fontSize),
              [10, 11, 12, 13, 14, 15, 16, 18, 20].map(n => ({ value: String(n), label: `${n}px` })),
              (v) => updateEditorSettings({ fontSize: Number(v) })
            )}
            {toggleRow('Font Ligatures', fontSupportsLigatures ? 'Enable programming ligatures (e.g. => becomes arrow)' : 'Requires Fira Code, JetBrains Mono, or Lilex', 'fontLigatures', false, !fontSupportsLigatures)}
            {dropdownRow('Cursor Style', 'Shape of the text cursor', editor.cursorStyle,
              [{ value: 'bar', label: 'Bar' }, { value: 'block', label: 'Block' }, { value: 'underline', label: 'Underline' }],
              (v) => updateEditorSettings({ cursorStyle: v as 'bar' | 'block' | 'underline' })
            )}
            {toggleRow('Cursor Blink', 'Animate the cursor blinking', 'cursorBlink', true)}
          </div>
        </div>

        {/* ── Editing ── */}
        <div>
          <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-3">Editing</h2>
          <div className="border border-[var(--color-border)]">
            {dropdownRow('Tab Size', 'Default spaces per indentation (languages may override)', String(editor.tabSize),
              [{ value: '2', label: '2' }, { value: '4', label: '4' }, { value: '8', label: '8' }],
              (v) => updateEditorSettings({ tabSize: Number(v) })
            )}
            {toggleRow('Word Wrap', 'Wrap long lines instead of horizontal scrolling', 'wordWrap')}
            {toggleRow('Autocomplete', 'Show word-based completion suggestions as you type', 'autocomplete')}
            {toggleRow('Bracket Matching', 'Highlight matching brackets', 'bracketMatching')}
            {toggleRow('Format on Save', 'Auto-format with Prettier, rustfmt, or black on Cmd+S', 'formatOnSave')}
            {toggleRow('Show Whitespace', 'Render spaces and tabs as visible dots', 'showWhitespace', true)}
          </div>
        </div>

        {/* ── Gutter & Display ── */}
        <div>
          <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-3">Gutter & Display</h2>
          <div className="border border-[var(--color-border)]">
            {toggleRow('Line Numbers', 'Show line numbers in the gutter', 'lineNumbers')}
            {toggleRow('Indent Guides', 'Show vertical indentation guide lines', 'indentGuides')}
            {toggleRow('Code Folding', 'Show fold/unfold arrows in the gutter', 'foldGutter')}
            {toggleRow('Highlight Active Line', 'Subtle background highlight on the current line', 'highlightActiveLine')}
            {toggleRow('Scroll Past End', 'Allow scrolling beyond the last line', 'scrollPastEnd')}
            {toggleRow('Minimap', 'Show a miniature overview of the file on the right', 'minimap')}
            {dropdownRow('Diff Style', 'How changed lines appear in the editor', editor.diffStyle,
              [{ value: 'gutter', label: 'Gutter' }, { value: 'inline', label: 'Inline (PR view)' }],
              (v) => updateEditorSettings({ diffStyle: v as 'gutter' | 'inline' })
            )}
            {toggleRow('Scrollbar Annotations', 'Show colored markers on the scrollbar where code was changed', 'scrollbarAnnotations')}
            {toggleRow('Sticky Scroll', 'Pin current function/class header at the top', 'stickyScroll', true)}
          </div>
        </div>

        {/* ── Keybindings & Modes ── */}
        <div>
          <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-3">Keybindings & Modes</h2>
          <div className="border border-[var(--color-border)]">
            {toggleRow('Vim Mode', 'Full vim keybinding emulation (hjkl, modes, commands)', 'vimMode')}
            <div className="px-3 py-2.5 border-b border-[var(--color-border)]">
              <div className="flex items-center justify-between">
                <div className="text-xs text-[var(--color-text-secondary)]">Select Next Occurrence</div>
                <span className="text-[10px] text-[var(--color-text-muted)] font-mono bg-[var(--color-bg)] px-2 py-0.5 border border-[var(--color-border)]">Cmd+D</span>
              </div>
              <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">Add a cursor at the next match of the selected word</div>
            </div>
            <div className="px-3 py-2.5 border-b border-[var(--color-border)]">
              <div className="flex items-center justify-between">
                <div className="text-xs text-[var(--color-text-secondary)]">Find & Replace</div>
                <span className="text-[10px] text-[var(--color-text-muted)] font-mono bg-[var(--color-bg)] px-2 py-0.5 border border-[var(--color-border)]">Cmd+F / Cmd+H</span>
              </div>
              <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">Search with regex, case-sensitive, and replace support</div>
            </div>
            <div className="px-3 py-2.5">
              <div className="flex items-center justify-between">
                <div className="text-xs text-[var(--color-text-secondary)]">Fold / Unfold</div>
                <span className="text-[10px] text-[var(--color-text-muted)] font-mono bg-[var(--color-bg)] px-2 py-0.5 border border-[var(--color-border)]">Cmd+Shift+[ / ]</span>
              </div>
              <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">Collapse or expand code blocks at the cursor</div>
            </div>
          </div>
        </div>
      </div>

      {/* ── Live Preview (sticky) ── */}
      <div className="flex-1 min-w-[400px] sticky top-0 self-start">
        <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-3">Preview</h2>
        <div className="border border-[var(--color-border)] h-[calc(100vh-120px)] overflow-hidden">
          <CodeEditor
            code={previewCode}
            filePath="preview.tsx"
            onSave={() => {}}
            onChange={() => {}}
            readOnly
            demoLineChanges={demoChanges}
          />
        </div>
      </div>
    </div>
  )
}
