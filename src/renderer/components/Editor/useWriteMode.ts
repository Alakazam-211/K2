// The Write and Read extension for the editor frame (build-editors.md E2):
// the file's own room loads its images and opens its file links, and an
// http, https or mailto link opens outside K2. Source gets no extension.

import { useMemo } from 'react'
import type { Extension } from '@codemirror/state'
import { useOptionalRoom } from '@/components/Room/RoomContext'
import { loadHostImageObjectUrl } from '@/lib/load-host-binary'
import { openExternalLink } from '@/lib/open-external-link'
import { writeMode, type WriteLinks } from '@/lib/editor/write-mode'
import type { EditorMode } from '@/lib/editor/modes'

const NONE: Extension = []

export function useWriteMode(mode: EditorMode, filePath: string): Extension {
  const room = useOptionalRoom()
  const links = useMemo<WriteLinks>(
    () => ({
      filePath,
      loadImage: room ? (p) => loadHostImageObjectUrl(room.scope, p).then((r) => r.url) : undefined,
      openFile: room ? (p) => void room.tabs.getState().openFileAsTab(p) : undefined,
      openExternal: (url) => void openExternalLink(url),
    }),
    [filePath, room],
  )
  return useMemo(() => (mode === 'source' ? NONE : writeMode(mode, links)), [mode, links])
}
