// The app font family and font size (0.45.7, Rosson: show the most general
// font settings at the top of Settings → General too). One control, two
// places: Settings → General (top) and Editors / Fonts → Code. Both read and
// write the same setting (`editor.fontFamily`, `editor.fontSize`), so a
// change in one shows in the other. The size also sets the text in the
// message box, chats, Threads and tickets.

import React from 'react'
import { useSettingsStore } from '@/stores/settings'
import { EDITOR_FONTS } from '@/components/FileViewerPane/CodeEditor'
import { SettingDropdown, SettingRow } from '../controls/SettingControls'

export const APP_FONT_SIZES = [10, 11, 12, 13, 14, 15, 16, 18, 20]

const COPY = {
  general: {
    family: 'Font family',
    familyHint: 'The font K2 uses for code and text.',
    size: 'Font size',
    sizeHint: 'For code, the message box, chats and tickets.',
  },
  'code-editor': {
    family: 'Font Family',
    familyHint: 'Monospace font for code editing',
    size: 'Font Size',
    sizeHint: 'Editor text size in pixels',
  },
} as const

export function AppFontRows({
  where,
  lastHasBorder = true,
}: {
  /** Which page: sets the search ids and the words. */
  where: 'general' | 'code-editor'
  /** The size row's bottom border (off when it is the last row of a box).
   *  Code only: General's rows sit in a card, which draws the dividers. */
  lastHasBorder?: boolean
}): React.JSX.Element {
  const fontFamily = useSettingsStore((s) => s.editor.fontFamily)
  const fontSize = useSettingsStore((s) => s.editor.fontSize)
  const updateEditorSettings = useSettingsStore((s) => s.updateEditorSettings)
  const copy = COPY[where]
  const familyOptions = EDITOR_FONTS.map((f) => ({ value: f.id, label: f.label }))
  const sizeOptions = APP_FONT_SIZES.map((n) => ({ value: String(n), label: `${n}px` }))
  if (where === 'general') {
    // Field rows of General's card: the dropdowns sit at the shared field
    // width on the card's right edge, like every other Settings field.
    return (
      <>
        <SettingRow label={copy.family} hint={copy.familyHint} settingId="general.font-family" field>
          <SettingDropdown
            fullWidth
            value={fontFamily}
            ariaLabel={copy.family}
            options={familyOptions}
            onChange={(v) => updateEditorSettings({ fontFamily: v })}
          />
        </SettingRow>
        <SettingRow label={copy.size} hint={copy.sizeHint} settingId="general.font-size" field>
          <SettingDropdown
            fullWidth
            value={String(fontSize)}
            ariaLabel={copy.size}
            options={sizeOptions}
            onChange={(v) => updateEditorSettings({ fontSize: Number(v) })}
          />
        </SettingRow>
      </>
    )
  }
  const row = (border: boolean): string =>
    `flex items-center justify-between px-3 py-2.5 ${border ? 'border-b border-[var(--color-border)]' : ''}`
  return (
    <>
      <div className={row(true)} data-settings-id={`${where}.font-family`}>
        <div>
          <div className="type-row text-[var(--color-text-secondary)]">{copy.family}</div>
          <div className="type-hint text-[var(--color-text-muted)] mt-0.5">{copy.familyHint}</div>
        </div>
        <SettingDropdown
          value={fontFamily}
          ariaLabel={copy.family}
          options={familyOptions}
          onChange={(v) => updateEditorSettings({ fontFamily: v })}
        />
      </div>
      <div className={row(lastHasBorder)} data-settings-id={`${where}.font-size`}>
        <div>
          <div className="type-row text-[var(--color-text-secondary)]">{copy.size}</div>
          <div className="type-hint text-[var(--color-text-muted)] mt-0.5">{copy.sizeHint}</div>
        </div>
        <SettingDropdown
          value={String(fontSize)}
          ariaLabel={copy.size}
          options={sizeOptions}
          onChange={(v) => updateEditorSettings({ fontSize: Number(v) })}
        />
      </div>
    </>
  )
}
