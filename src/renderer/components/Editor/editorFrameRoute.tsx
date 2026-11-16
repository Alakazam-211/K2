// Where a file tab opens while its editor's preview switch is on ("New
// markdown editor" or "New code editor", lib/editor/preview-switch.ts): the
// new editor (EditorTab), or today's view. A live file (its multiplayer core
// on) opens in the new editor too (E4a, `live`: the kit's Live editor inside
// it); any-file and deck files keep today's view, and a locked (not text)
// file keeps its lock.

import { useSettingsStore } from '@/stores/settings'
import type { FileCategory } from '@/components/FileViewerPane/fileCategory'
import { liveNoticeView } from '@/lib/editor/live/live-notices'
import { newEditorKind, useEditorPreview, type EditorKind } from '@/lib/editor/preview-switch'
import { SaveNoticeBar } from './SaveNoticeBar'

/** The new editor for this file while its switch is on, else null. Follows the switch live. */
export function useEditorFrameKind(category: FileCategory): EditorKind | null {
  const kind = newEditorKind(category)
  const on = useEditorPreview(kind ?? 'code')
  return kind && on ? kind : null
}

export type EditorFrameRoute = 'frame' | 'live' | 'today'

/** `kind` is {@link useEditorFrameKind}'s answer; the rest is the file tab's state. */
export function editorFrameRoute(
  kind: EditorKind | null,
  s: { locked: boolean; liveOn: boolean; anyFile: boolean; deck: boolean },
): EditorFrameRoute {
  if (!kind || s.locked || s.anyFile || s.deck) return 'today'
  return s.liveOn ? 'live' : 'frame'
}

/** Live editing turned off mid-session: the same editor carries on, saving on its own. */
export function LiveOffNotice({ onDismiss }: { onDismiss: () => void }): React.JSX.Element {
  const openSettings = useSettingsStore((s) => s.openSettings)
  return (
    <SaveNoticeBar
      view={liveNoticeView({ kind: 'live-off' }, '')}
      kind="live:live-off"
      onAction={(id) => (id === 'open-settings' ? openSettings('multiplayer') : onDismiss())}
    />
  )
}
