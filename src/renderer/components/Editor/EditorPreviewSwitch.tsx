// Settings → Multiplayer: "New editors (preview)", beside the live views'
// preview switches, and on a server without multiplayer cores the page's only card. Two rows, one per editor, each off by default and kept
// per computer (lib/editor/preview-switch.ts). Anyone can turn them on: they
// change how files look on this computer, not the server.

import { BetaTag, Toggle } from '@/components/ui'
import { SETTING_ROW_HINT, SettingsGroup, SettingSwitchRow } from '@/components/Settings/controls/SettingControls'
import type { SettingEntry } from '@/components/Settings/searchManifest'
import { setEditorPreview, useEditorPreview, type EditorKind } from '@/lib/editor/preview-switch'

export const EDITOR_PREVIEW_ROWS: readonly { kind: EditorKind; id: string; name: string; line: string; testId: string }[] = [
  {
    kind: 'markdown',
    id: 'multiplayer.new-markdown-editor',
    name: 'New markdown editor',
    line: 'Markdown files open in the new editor: Write, Read and Source, with the word count, Full width and Wrap. Saving works as it does today. Live files keep the live editor for now. On this computer only.',
    testId: 'new-markdown-editor-toggle',
  },
  {
    kind: 'code',
    id: 'multiplayer.new-code-editor',
    name: 'New code editor',
    line: 'Code, config, plain text and logs open in the new editor, and CSV, HTML, Mermaid, diagram and DXF files keep their preview beside Source. Saving works as it does today. Live files keep the live editor for now. On this computer only.',
    testId: 'new-code-editor-toggle',
  },
]

/** Settings search: both switches, on every server (Settings → Multiplayer is always there). */
export const EDITOR_PREVIEW_MANIFEST: SettingEntry[] = [
  { id: 'multiplayer.new-markdown-editor', section: 'multiplayer', group: 'New editors (preview)', label: 'New markdown editor', description: 'Markdown files open in the new editor: Write, Read and Source', keywords: ['editor', 'new editor', 'preview', 'write', 'read', 'source', 'markdown', 'beta'] },
  { id: 'multiplayer.new-code-editor', section: 'multiplayer', group: 'New editors (preview)', label: 'New code editor', description: 'Code, config, text and logs open in the new editor', keywords: ['editor', 'new editor', 'preview', 'code', 'source', 'json', 'yaml', 'csv', 'html', 'beta'] },
]

function Row({ row }: { row: (typeof EDITOR_PREVIEW_ROWS)[number] }): React.JSX.Element {
  const on = useEditorPreview(row.kind)
  return (
    <SettingSwitchRow settingId={row.id} control={<Toggle checked={on} onChange={(v: boolean) => setEditorPreview(row.kind, v)} aria-label={`${row.name} (Beta)`} data-testid={row.testId} />}>
      <div className="type-row flex items-center gap-2 text-[var(--color-text-primary)]">
        <span>{row.name}</span>
        <BetaTag />
      </div>
      <div className={SETTING_ROW_HINT}>{row.line}</div>
    </SettingSwitchRow>
  )
}

export function EditorPreviewSwitch(): React.JSX.Element {
  return (
    <SettingsGroup title="New editors (preview)" card testId="editor-preview-switches">
      {EDITOR_PREVIEW_ROWS.map((row) => (
        <Row key={row.kind} row={row} />
      ))}
    </SettingsGroup>
  )
}
