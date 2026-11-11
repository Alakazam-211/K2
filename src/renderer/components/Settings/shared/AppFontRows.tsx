// The app font family and font size (0.45.7, Rosson: show the most general
// font settings at the top of Settings → General too). One control, two
// places: Settings → General (top) and Editors / Fonts → Code. Both read and
// write the same setting (`editor.fontFamily`, `editor.fontSize`), so a
// change in one shows in the other. The size also sets the text in the
// message box, chats, Threads and tickets.

import React from 'react'
import { useSettingsStore } from '@/stores/settings'
import { EDITOR_FONTS } from '@/components/FileViewerPane/CodeEditor'
import { SaveError, SaveNote, SettingDropdown, SettingRow, useSaveStatus } from '../controls/SettingControls'

export const APP_FONT_SIZES = [10, 11, 12, 13, 14, 15, 16, 18, 20]

const COPY = {
  general: {
    family: 'Font family',
    familyHint: 'The font K2 uses for code and text.',
    size: 'Font size',
    sizeHint: 'For code, the message box, chats and tickets.',
  },
  'code-editor': {
    family: 'Font family',
    familyHint: 'For code here and for text across K2: the same setting as in General.',
    size: 'Font size',
    sizeHint: 'For code, the message box, chats and tickets.',
  },
} as const

export function AppFontRows({
  where,
}: {
  /** Which page: sets the search ids and the words. Both sit in a card. */
  where: 'general' | 'code-editor'
}): React.JSX.Element {
  const fontFamily = useSettingsStore((s) => s.editor.fontFamily)
  const fontSize = useSettingsStore((s) => s.editor.fontSize)
  const updateEditorSettings = useSettingsStore((s) => s.updateEditorSettings)
  const copy = COPY[where]
  // Each row says when a change saved, or that it didn't. The store
  // puts the old value back when the server refuses the write.
  const familySave = useSaveStatus()
  const sizeSave = useSaveStatus()
  const saveEditor = async (
    partial: { fontFamily: string } | { fontSize: number },
    save: ReturnType<typeof useSaveStatus>,
  ): Promise<void> => {
    save.reset()
    await Promise.resolve(updateEditorSettings(partial))
    const now = useSettingsStore.getState().editor
    const kept = 'fontFamily' in partial ? now.fontFamily === partial.fontFamily : now.fontSize === partial.fontSize
    if (kept) save.saved()
    else save.failed('')
  }
  const familyOptions = EDITOR_FONTS.map((f) => ({ value: f.id, label: f.label }))
  const sizeOptions = APP_FONT_SIZES.map((n) => ({ value: String(n), label: `${n}px` }))
  // Field rows of a card: the dropdowns sit at the shared field width on the
  // card's right edge, like every other Settings field.
  return (
    <>
      <SettingRow
        label={copy.family}
        hint={
          <>
            {copy.familyHint} <SaveNote state={familySave.state} />
            <SaveError state={familySave.state} />
          </>
        }
        settingId={`${where}.font-family`}
        field
      >
        <SettingDropdown
          fullWidth
          value={fontFamily}
          ariaLabel={copy.family}
          options={familyOptions}
          onChange={(v) => saveEditor({ fontFamily: v }, familySave)}
        />
      </SettingRow>
      <SettingRow
        label={copy.size}
        hint={
          <>
            {copy.sizeHint} <SaveNote state={sizeSave.state} />
            <SaveError state={sizeSave.state} />
          </>
        }
        settingId={`${where}.font-size`}
        field
      >
        <SettingDropdown
          fullWidth
          value={String(fontSize)}
          ariaLabel={copy.size}
          options={sizeOptions}
          onChange={(v) => saveEditor({ fontSize: Number(v) }, sizeSave)}
        />
      </SettingRow>
    </>
  )
}
