// The Live view as the file tab mounts it (lazy, like CodeEditor): the Live
// editor with the same editor theme as CodeEditor (lib/editor/theme, Follow
// app by default) and, for code, the file's language from CodeEditor's table. Markdown uses the editor's Markdown
// mode; any other text the plain mode with the language on top.

import React, { useMemo } from 'react'
import type { Extension } from '@codemirror/state'
import type { ServerScope } from '@/kessel/server-scope'
import { useSettingsStore } from '@/stores/settings'
import { getLanguageExtension } from './CodeEditor'
import { getHighlightExtension, getThemeExtension, resolveEditorThemeId } from '@/lib/editor/theme'
import { LiveDocView, type LiveDocClosed } from './LiveDocView'

export interface LiveDocPaneProps {
  scope: ServerScope
  filePath: string
  /** The file's core, as the daemon named it (`markdown`, `text`). */
  core: string
  onCoreOff?: (closed: LiveDocClosed) => void
}

export function LiveDocPane({ scope, filePath, core, onCoreOff }: LiveDocPaneProps): React.JSX.Element {
  const theme = resolveEditorThemeId(useSettingsStore((s) => s.editor.theme))
  const markdown = core === 'markdown'
  const extensions = useMemo<Extension>(() => {
    const lang = markdown ? null : getLanguageExtension(filePath)
    return [getThemeExtension(theme), getHighlightExtension(theme), ...(lang ? [lang] : [])]
  }, [theme, filePath, markdown])
  return <LiveDocView scope={scope} filePath={filePath} markdown={markdown} extensions={extensions} onCoreOff={onCoreOff} />
}

export default LiveDocPane
