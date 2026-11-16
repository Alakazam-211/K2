// Settings → Code: "New file editor (preview)", off by default, per computer
// (lib/editor/preview-switch.ts). One card at the top of the page.

import { BetaTag, Toggle } from '@/components/ui'
import { SETTING_ROW_HINT, SettingsGroup, SettingSwitchRow } from '@/components/Settings/controls/SettingControls'
import { setNewEditorPreview, useNewEditorPreview } from '@/lib/editor/preview-switch'

export function EditorPreviewSwitch(): React.JSX.Element {
  const on = useNewEditorPreview()
  return (
    <SettingsGroup title="Preview" card testId="code-preview-switches">
      <SettingSwitchRow
        settingId="code-editor.new-editor-preview"
        control={<Toggle checked={on} onChange={(v: boolean) => setNewEditorPreview(v)} aria-label="New file editor (preview) (Beta)" data-testid="new-editor-preview-toggle" />}
      >
        <div className="type-row flex items-center gap-2 text-[var(--color-text-primary)]">
          <span>New file editor (preview)</span>
          <BetaTag />
        </div>
        <div className={SETTING_ROW_HINT}>
          Markdown and code files open in the new editor: Write, Read and Source for markdown, and a status line with the word count, Full width and Wrap. Saving works as it does today. On this computer only.
        </div>
      </SettingSwitchRow>
    </SettingsGroup>
  )
}
