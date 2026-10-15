// The app font family and font size (0.45.7, Rosson: show the most general
// font settings at the top of Settings → General too). One control, two
// places: Settings → General (top) and Editors / Fonts → Code. Both read and
// write the same setting (`editor.fontFamily`, `editor.fontSize`), so a
// change in one shows in the other. The size also sets the text in the
// message box, chats, Threads and tickets.

import React from 'react'
import { useSettingsStore } from '@/stores/settings'
import { EDITOR_FONTS } from '@/components/FileViewerPane/CodeEditor'
import { SettingDropdown } from '../controls/SettingControls'

export const APP_FONT_SIZES = [10, 11, 12, 13, 14, 15, 16, 18, 20]

const COPY = {
  general: {
    family: 'Font family',
    familyHint: 'Same setting as Editors / Fonts → Code',
    size: 'Font size',
    sizeHint: 'Text size in pixels for code, the message box, chats and tickets',
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
  /** The size row's bottom border (off when it is the last row of a box). */
  lastHasBorder?: boolean
}): React.JSX.Element {
  const fontFamily = useSettingsStore((s) => s.editor.fontFamily)
  const fontSize = useSettingsStore((s) => s.editor.fontSize)
  const updateEditorSettings = useSettingsStore((s) => s.updateEditorSettings)
  const copy = COPY[where]
  const row = (border: boolean): string =>
    `flex items-center justify-between px-3 py-2.5 ${border ? 'border-b border-[var(--color-border)]' : ''}`
  return (
    <>
      <div className={row(true)} data-settings-id={`${where}.font-family`}>
        <div>
          <div className="text-xs text-[var(--color-text-secondary)]">{copy.family}</div>
          <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">{copy.familyHint}</div>
        </div>
        <SettingDropdown
          value={fontFamily}
          ariaLabel={copy.family}
          options={EDITOR_FONTS.map((f) => ({ value: f.id, label: f.label }))}
          onChange={(v) => updateEditorSettings({ fontFamily: v })}
        />
      </div>
      <div className={row(lastHasBorder)} data-settings-id={`${where}.font-size`}>
        <div>
          <div className="text-xs text-[var(--color-text-secondary)]">{copy.size}</div>
          <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">{copy.sizeHint}</div>
        </div>
        <SettingDropdown
          value={String(fontSize)}
          ariaLabel={copy.size}
          options={APP_FONT_SIZES.map((n) => ({ value: String(n), label: `${n}px` }))}
          onChange={(v) => updateEditorSettings({ fontSize: Number(v) })}
        />
      </div>
    </>
  )
}
