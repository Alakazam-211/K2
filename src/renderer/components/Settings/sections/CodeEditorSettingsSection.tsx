import React from 'react'
import { useCallback, useMemo, useState } from 'react'
import { Button, Surface, Tag, Toggle } from '@/components/ui'
import { useSettingsStore } from '@/stores/settings'
import { useCustomThemesStore } from '@/stores/custom-themes'
import type { CustomTheme } from '@/stores/custom-themes'
import { EDITOR_THEMES, CodeEditor } from '@/components/FileViewerPane/CodeEditor'
import { isMacPlatform } from '@/lib/desktop-chrome'
import type { EditorSettingsBackend } from '@shared/types'
import { AppFontRows } from '../shared/AppFontRows'
import { KeyChip } from '../shared/KeyChip'
import { CustomThemeCreator } from '../CustomThemeCreator'
import { SectionErrorBoundary } from '../SectionErrorBoundary'
import {
  SaveError,
  SaveNote,
  SETTING_ROW_HINT,
  SettingDropdown,
  SettingRow,
  SettingsGroup,
  useSaveStatus,
} from '../controls/SettingControls'
import { AccessSheet, FailedNote } from './AccessDialogs'
import type { SettingEntry } from '../searchManifest'

export const CODE_EDITOR_MANIFEST: SettingEntry[] = [
  // Appearance
  { id: 'code-editor.theme', section: 'code-editor', group: 'Appearance', label: 'Theme', description: 'Colours for code in the editor', keywords: ['theme', 'colors', 'colours', 'appearance', 'dark mode', 'light mode', 'custom theme', 'new theme'] },
  { id: 'code-editor.font-family', section: 'code-editor', group: 'Appearance', label: 'Font family', description: 'For code here and for text across K2', keywords: ['font', 'typeface', 'family'] },
  { id: 'code-editor.font-size', section: 'code-editor', group: 'Appearance', label: 'Font size', description: 'For code, the message box, chats and tickets', keywords: ['font', 'size', 'zoom'] },
  { id: 'code-editor.font-ligatures', section: 'code-editor', group: 'Appearance', label: 'Font ligatures', description: 'Joins pairs like => into one symbol', keywords: ['ligatures', 'font', 'arrow', 'fira', 'jetbrains'] },
  { id: 'code-editor.cursor-style', section: 'code-editor', group: 'Appearance', label: 'Cursor shape', description: 'Bar, block or underline', keywords: ['cursor', 'caret', 'style'] },
  { id: 'code-editor.cursor-blink', section: 'code-editor', group: 'Appearance', label: 'Blinking cursor', description: 'The cursor blinks while the editor has focus', keywords: ['cursor', 'blink'] },
  // Editing
  { id: 'code-editor.tab-size', section: 'code-editor', group: 'Editing', label: 'Tab size', description: 'Spaces per indent', keywords: ['tab', 'indent', 'spaces', 'size'] },
  { id: 'code-editor.word-wrap', section: 'code-editor', group: 'Editing', label: 'Wrap long lines', description: 'Long lines wrap instead of scrolling sideways', keywords: ['wrap', 'word wrap', 'lines'] },
  { id: 'code-editor.autocomplete', section: 'code-editor', group: 'Editing', label: 'Suggest words as you type', description: 'Suggestions from words already in the file', keywords: ['autocomplete', 'complete', 'suggestions', 'intellisense'] },
  { id: 'code-editor.bracket-matching', section: 'code-editor', group: 'Editing', label: 'Highlight matching brackets', description: 'Shows the partner of the bracket at the cursor', keywords: ['brackets', 'match', 'pair'] },
  { id: 'code-editor.format-on-save', section: 'code-editor', group: 'Editing', label: 'Format on save', description: 'Tidies the file with its formatter when you save', keywords: ['format', 'save', 'prettier', 'rustfmt', 'black'] },
  { id: 'code-editor.show-whitespace', section: 'code-editor', group: 'Editing', label: 'Show spaces and tabs', description: 'Marks each space and tab', keywords: ['whitespace', 'spaces', 'tabs', 'invisible'] },
  // Display
  { id: 'code-editor.line-numbers', section: 'code-editor', group: 'Display', label: 'Line numbers', description: 'In the margin on the left', keywords: ['line numbers', 'gutter', 'numbers'] },
  { id: 'code-editor.indent-guides', section: 'code-editor', group: 'Display', label: 'Indent guides', description: 'Faint lines at each indent level', keywords: ['indent', 'guides', 'rulers'] },
  { id: 'code-editor.fold-gutter', section: 'code-editor', group: 'Display', label: 'Code folding', description: 'Arrows in the margin to collapse blocks', keywords: ['fold', 'collapse', 'gutter'] },
  { id: 'code-editor.highlight-active-line', section: 'code-editor', group: 'Display', label: 'Highlight the current line', description: 'A faint band behind the line with the cursor', keywords: ['highlight', 'active', 'current line'] },
  { id: 'code-editor.scroll-past-end', section: 'code-editor', group: 'Display', label: 'Scroll past the end', description: 'The last line can scroll to the top', keywords: ['scroll', 'past end', 'bottom'] },
  { id: 'code-editor.minimap', section: 'code-editor', group: 'Display', label: 'Minimap', description: 'A small overview of the file on the right', keywords: ['minimap', 'overview', 'scroll preview'] },
  { id: 'code-editor.diff-style', section: 'code-editor', group: 'Display', label: 'Show changes', description: 'In the margin, or inline like a pull request', keywords: ['diff', 'diff style', 'changes', 'pr view', 'inline'] },
  { id: 'code-editor.scrollbar-annotations', section: 'code-editor', group: 'Display', label: 'Mark changes on the scrollbar', description: 'Coloured marks where the file changed', keywords: ['scrollbar', 'annotations', 'markers'] },
  { id: 'code-editor.sticky-scroll', section: 'code-editor', group: 'Display', label: 'Sticky scroll', description: 'Keeps the current function name at the top', keywords: ['sticky', 'scroll', 'breadcrumbs', 'header'] },
  // Keyboard
  { id: 'code-editor.vim-mode', section: 'code-editor', group: 'Keyboard', label: 'Vim mode', description: 'Vim keys and modes in the editor', keywords: ['vim', 'modal', 'hjkl', 'keybindings'] },
]

/** Fonts with programming ligatures among the bundled ones. */
const LIGATURE_FONTS = new Set(['Fira Code', 'JetBrains Mono', 'Lilex'])
const DEFAULT_THEME = 'k2so-dark'

type BoolKey = { [K in keyof EditorSettingsBackend]: EditorSettingsBackend[K] extends boolean ? K : never }[keyof EditorSettingsBackend]

/** Saves one editor setting and reports it on the row. */
function useEditorSave(): {
  status: ReturnType<typeof useSaveStatus>
  save: (partial: Partial<EditorSettingsBackend>) => Promise<void>
} {
  const status = useSaveStatus()
  const update = useSettingsStore((s) => s.updateEditorSettings)
  const save = async (partial: Partial<EditorSettingsBackend>): Promise<void> => {
    status.reset()
    await Promise.resolve(update(partial))
    const now = useSettingsStore.getState().editor
    const kept = (Object.keys(partial) as (keyof EditorSettingsBackend)[]).every((k) => now[k] === partial[k])
    if (kept) status.saved()
    else status.failed('')
  }
  return { status, save }
}

function RowHint({ status, children }: { status: ReturnType<typeof useSaveStatus>; children: React.ReactNode }): React.JSX.Element {
  return (
    <>
      {children} <SaveNote state={status.state} />
      <SaveError state={status.state} />
    </>
  )
}

/** A switch row for one on/off editor setting. */
function EditorSwitch({
  id,
  field,
  label,
  hint,
  disabled = false,
}: {
  id: string
  field: BoolKey
  label: string
  hint: React.ReactNode
  disabled?: boolean
}): React.JSX.Element {
  const on = useSettingsStore((s) => Boolean(s.editor[field]))
  const { status, save } = useEditorSave()
  return (
    <SettingRow settingId={id} label={label} hint={<RowHint status={status}>{hint}</RowHint>}>
      <Toggle
        size="sm"
        checked={on && !disabled}
        disabled={disabled}
        aria-label={label}
        onChange={() => void save({ [field]: !on } as Partial<EditorSettingsBackend>)}
      />
    </SettingRow>
  )
}

/** A dropdown row for one editor setting with a few choices. */
function EditorChoice({
  id,
  label,
  hint,
  value,
  options,
  onPick,
}: {
  id: string
  label: string
  hint: React.ReactNode
  value: string
  options: { value: string; label: string }[]
  onPick: (v: string) => Partial<EditorSettingsBackend>
}): React.JSX.Element {
  const { status, save } = useEditorSave()
  return (
    <SettingRow settingId={id} label={label} hint={<RowHint status={status}>{hint}</RowHint>} field>
      <SettingDropdown fullWidth ariaLabel={label} value={value} options={options} onChange={(v) => save(onPick(v))} />
    </SettingRow>
  )
}

/** A fixed editor shortcut: what it does, and its keys. */
function ShortcutRow({ label, hint, keys }: { label: string; hint: string; keys: string[] }): React.JSX.Element {
  return (
    <SettingRow label={label} hint={hint}>
      {keys.map((k) => (
        <KeyChip key={k} combo={k} />
      ))}
    </SettingRow>
  )
}

/** Ask before deleting a custom theme; say why if the server refuses. */
function DeleteThemeDialog({
  theme,
  inUse,
  onClose,
  onDeleted,
}: {
  theme: CustomTheme
  inUse: boolean
  onClose: () => void
  onDeleted: () => void
}): React.JSX.Element {
  const deleteCustomTheme = useCustomThemesStore((s) => s.deleteCustomTheme)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const run = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      await deleteCustomTheme(theme.id)
      onDeleted()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err ?? ''))
      setBusy(false)
    }
  }
  return (
    <AccessSheet
      title={`Delete ${theme.name}?`}
      intro={
        inUse
          ? 'The theme file is removed from this server. The editor uses it now, so it goes back to K2 Dark.'
          : 'The theme file is removed from this server. Other themes stay as they are.'
      }
      onClose={onClose}
      busy={busy}
      testId="code-theme-delete"
    >
      {error !== null && (
        <FailedNote what={`${theme.name} wasn’t deleted.`} next="The theme is still there. Try again in a moment." detail={error || 'no reason given'} />
      )}
      <div className="flex justify-end gap-2 pt-1">
        <Button variant="ghost" size="md" onClick={onClose} disabled={busy}>
          Cancel
        </Button>
        <Button variant="danger-muted" size="md" onClick={() => void run()} disabled={busy}>
          {busy ? 'Deleting…' : 'Delete theme'}
        </Button>
      </div>
    </AccessSheet>
  )
}

// Sample code for the live preview — K2 coding in a galaxy far, far away
const PREVIEW_CODE = `import { useState, useEffect, useCallback } from 'react'

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


const DEMO_CHANGES: Map<number, 'added' | 'modified' | 'deleted'> = new Map([
  [11, 'added'], [14, 'modified'], [21, 'added'], [22, 'added'], [23, 'added'], [24, 'added'], [25, 'added'],
  [30, 'modified'], [31, 'modified'], [37, 'added'], [38, 'added'], [39, 'added'], [40, 'added'], [55, 'deleted'],
  [73, 'modified'], [74, 'modified'], [75, 'modified'], [76, 'modified'], [79, 'added'],
])

export function CodeEditorSettingsSection(): React.JSX.Element {
  const editor = useSettingsStore((s) => s.editor)
  const updateEditorSettings = useSettingsStore((s) => s.updateEditorSettings)
  const customThemes = useCustomThemesStore((s) => s.customThemes)
  const [showCreator, setShowCreator] = useState(false)
  const [editingThemePath, setEditingThemePath] = useState<string | undefined>(undefined)
  const [deleting, setDeleting] = useState<CustomTheme | null>(null)
  const [themeNote, setThemeNote] = useState<string | null>(null)
  const isCustomTheme = editor.theme.startsWith('custom:')
  const fontSupportsLigatures = LIGATURE_FONTS.has(editor.fontFamily)
  const mac = isMacPlatform()
  const themeSave = useEditorSave()

  // Built-in themes, then yours.
  const themeOptions = useMemo(
    () => [
      ...EDITOR_THEMES.map((t) => ({ value: t.id, label: t.label })),
      ...customThemes.map((t) => ({ value: t.id, label: t.name })),
    ],
    [customThemes],
  )

  const openCreator = useCallback((path?: string) => {
    setEditingThemePath(path)
    setShowCreator(true)
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

  const activeCustom = isCustomTheme ? customThemes.find((t) => t.id === editor.theme) : undefined

  return (
    <div className="w-full space-y-6 pb-6" data-testid="code-page">
      <header className="min-w-0">
        <h2 className="type-title text-[var(--color-text-primary)]">Code</h2>
        <p className={SETTING_ROW_HINT}>How files look and behave in K2’s code editor. The preview shows each change as you make it.</p>
      </header>

      <div className="flex gap-6 items-start">
        <div className="flex-1 min-w-0 max-w-[40rem] space-y-6">
          <SettingsGroup
            card
            title="Appearance"
            action={
              <Button variant="quiet" onClick={() => openCreator(undefined)}>
                New theme
              </Button>
            }
            testId="code-appearance"
          >
            <SettingRow
              settingId="code-editor.theme"
              label="Theme"
              hint={
                <RowHint status={themeSave.status}>
                  Colours for code in the editor and in diffs.
                  {activeCustom && (
                    <>
                      {' '}
                      <button
                        type="button"
                        data-k2-focus=""
                        className="relative px-0.5 -mx-0.5 underline hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
                        onClick={() => openCreator(activeCustom.path)}
                      >
                        Edit {activeCustom.name}
                      </button>
                    </>
                  )}
                </RowHint>
              }
              field
            >
              <SettingDropdown
                fullWidth
                ariaLabel="Theme"
                value={editor.theme}
                options={themeOptions}
                onChange={(v) => themeSave.save({ theme: v })}
              />
            </SettingRow>
            {/* The same rows as the top of Settings → General (one setting). */}
            <AppFontRows where="code-editor" />
            <EditorSwitch
              id="code-editor.font-ligatures"
              field="fontLigatures"
              label="Font ligatures"
              disabled={!fontSupportsLigatures}
              hint={
                fontSupportsLigatures
                  ? 'Joins pairs like => and != into one symbol.'
                  : `${editor.fontFamily} has no ligatures. Pick Fira Code, JetBrains Mono or Lilex above to use them.`
              }
            />
            <EditorChoice
              id="code-editor.cursor-style"
              label="Cursor shape"
              hint="The shape of the text cursor."
              value={editor.cursorStyle}
              options={[
                { value: 'bar', label: 'Bar' },
                { value: 'block', label: 'Block' },
                { value: 'underline', label: 'Underline' },
              ]}
              onPick={(v) => ({ cursorStyle: v as EditorSettingsBackend['cursorStyle'] })}
            />
            <EditorSwitch id="code-editor.cursor-blink" field="cursorBlink" label="Blinking cursor" hint="The cursor blinks while the editor has focus." />
          </SettingsGroup>

          {customThemes.length > 0 && (
            <SettingsGroup
              card
              title="Your themes"
              hint={
                themeNote ? (
                  <span role="status" data-testid="code-theme-note">
                    {themeNote}
                  </span>
                ) : (
                  'Themes you made. Edit one to change its colours.'
                )
              }
              testId="code-custom-themes"
            >
              {customThemes.map((t) => (
                <div key={t.id} className="px-4 py-3 flex items-center justify-between gap-6" data-testid={`code-theme-${t.id}`}>
                  <div className="flex items-center gap-3 min-w-0">
                    <span
                      aria-hidden="true"
                      data-k2-chamfer="chip"
                      className="w-5 h-5 flex-shrink-0 border border-[var(--color-control-edge)] rounded-[var(--radius-field)]"
                      style={{ backgroundColor: t.colors.bg }}
                    />
                    <span className="type-row text-[var(--color-text-primary)] truncate">{t.name}</span>
                    {editor.theme === t.id && <Tag tone="accent">In use</Tag>}
                  </div>
                  <div className="flex items-center gap-2 flex-shrink-0">
                    <Button variant="quiet" aria-label={`Edit ${t.name}`} onClick={() => openCreator(t.path)}>
                      Edit
                    </Button>
                    <Button variant="danger-quiet" aria-label={`Delete ${t.name}`} onClick={() => setDeleting(t)}>
                      Delete…
                    </Button>
                  </div>
                </div>
              ))}
            </SettingsGroup>
          )}

          <SettingsGroup card title="Editing" testId="code-editing">
            <EditorChoice
              id="code-editor.tab-size"
              label="Tab size"
              hint="Spaces per indent. Some languages use their own."
              value={String(editor.tabSize)}
              options={[
                { value: '2', label: '2 spaces' },
                { value: '4', label: '4 spaces' },
                { value: '8', label: '8 spaces' },
              ]}
              onPick={(v) => ({ tabSize: Number(v) })}
            />
            <EditorSwitch id="code-editor.word-wrap" field="wordWrap" label="Wrap long lines" hint="Long lines wrap instead of scrolling sideways." />
            <EditorSwitch id="code-editor.autocomplete" field="autocomplete" label="Suggest words as you type" hint="Suggestions come from words already in the file." />
            <EditorSwitch id="code-editor.bracket-matching" field="bracketMatching" label="Highlight matching brackets" hint="Shows the partner of the bracket at the cursor." />
            <EditorSwitch
              id="code-editor.format-on-save"
              field="formatOnSave"
              label="Format on save"
              hint={`Tidies the file with its formatter (Prettier, rustfmt or black) when you press ${mac ? '⌘S' : 'Ctrl+S'}.`}
            />
            <EditorSwitch id="code-editor.show-whitespace" field="showWhitespace" label="Show spaces and tabs" hint="Marks each space and tab so you can tell them apart." />
          </SettingsGroup>

          <SettingsGroup card title="Display" testId="code-display">
            <EditorSwitch id="code-editor.line-numbers" field="lineNumbers" label="Line numbers" hint="In the margin on the left." />
            <EditorSwitch id="code-editor.indent-guides" field="indentGuides" label="Indent guides" hint="Faint vertical lines at each indent level." />
            <EditorSwitch id="code-editor.fold-gutter" field="foldGutter" label="Code folding" hint="Arrows in the margin to collapse and expand blocks." />
            <EditorSwitch id="code-editor.highlight-active-line" field="highlightActiveLine" label="Highlight the current line" hint="A faint band behind the line with the cursor." />
            <EditorSwitch id="code-editor.scroll-past-end" field="scrollPastEnd" label="Scroll past the end" hint="The last line can scroll up to the top of the editor." />
            <EditorSwitch id="code-editor.minimap" field="minimap" label="Minimap" hint="A small overview of the whole file on the right." />
            <EditorChoice
              id="code-editor.diff-style"
              label="Show changes"
              hint="How lines you changed are marked."
              value={editor.diffStyle}
              options={[
                { value: 'gutter', label: 'In the margin' },
                { value: 'inline', label: 'Inline, like a pull request' },
              ]}
              onPick={(v) => ({ diffStyle: v as EditorSettingsBackend['diffStyle'] })}
            />
            <EditorSwitch id="code-editor.scrollbar-annotations" field="scrollbarAnnotations" label="Mark changes on the scrollbar" hint="Coloured marks show where the file changed." />
            <EditorSwitch id="code-editor.sticky-scroll" field="stickyScroll" label="Sticky scroll" hint="Keeps the current function or class name pinned at the top." />
          </SettingsGroup>

          <SettingsGroup card title="Keyboard" hint="Shortcuts in the editor. Change K2’s own shortcuts in Key Bindings." testId="code-keyboard">
            <EditorSwitch id="code-editor.vim-mode" field="vimMode" label="Vim mode" hint="Vim keys and modes: hjkl, insert and visual mode, : commands." />
            <ShortcutRow label="Select the next match" hint="Adds a cursor at the next match of the selected word." keys={[mac ? '⌘D' : 'Ctrl+D']} />
            <ShortcutRow label="Find and replace" hint="Opens search at the top of the file, with a replace field." keys={[mac ? '⌘F' : 'Ctrl+F']} />
            <ShortcutRow label="Fold and unfold" hint="The first collapses the block at the cursor; the second opens it again." keys={mac ? ['⌥⌘[', '⌥⌘]'] : ['Ctrl+Shift+[', 'Ctrl+Shift+]']} />
          </SettingsGroup>
        </div>

        {/* Live preview: stays in view while the settings scroll. */}
        <section className="flex-1 min-w-[400px] sticky top-0 self-start" aria-label="Preview" data-testid="code-preview">
          <div className="mb-2">
            <div className="flex items-center min-h-[1.5rem]">
              <h3 className="type-heading text-[var(--color-text-primary)]">Preview</h3>
            </div>
            <p className={SETTING_ROW_HINT}>A sample file, with changed lines marked.</p>
          </div>
          <Surface chamfer material="tile" className="h-[calc(100vh-13rem)] min-h-[20rem] overflow-hidden">
            <CodeEditor code={PREVIEW_CODE} filePath="preview.tsx" onSave={() => {}} onChange={() => {}} readOnly demoLineChanges={DEMO_CHANGES} />
          </Surface>
        </section>
      </div>

      {deleting && (
        <DeleteThemeDialog
          theme={deleting}
          inUse={editor.theme === deleting.id}
          onClose={() => setDeleting(null)}
          onDeleted={() => {
            const name = deleting.name
            const wasInUse = editor.theme === deleting.id
            if (wasInUse) void updateEditorSettings({ theme: DEFAULT_THEME })
            setDeleting(null)
            setThemeNote(wasInUse ? `${name} is deleted. The editor is back on K2 Dark.` : `${name} is deleted.`)
          }}
        />
      )}
    </div>
  )
}
