// The Live view as the file tab mounts it (lazy, like CodeEditor): the Live
// editor with the app's editor theme and, for code, the file's language,
// both from CodeEditor's own tables. Markdown uses the editor's Markdown
// mode; any other text the plain mode with the language on top.

import React, { useMemo } from 'react'
import type { Extension } from '@codemirror/state'
import type { ServerScope } from '@/kessel/server-scope'
import { useSettingsStore } from '@/stores/settings'
import { getHighlightExtension, getLanguageExtension, getThemeExtension } from './CodeEditor'
import { LiveDocView, type LiveDocClosed } from './LiveDocView'

export interface LiveDocPaneProps {
  scope: ServerScope
  filePath: string
  /** The file's core, as the daemon named it (`markdown`, `text`). */
  core: string
  onCoreOff?: (closed: LiveDocClosed) => void
}

export function LiveDocPane({ scope, filePath, core, onCoreOff }: LiveDocPaneProps): React.JSX.Element {
  const theme = useSettingsStore((s) => s.editor.theme) || 'k2so-dark'
  const markdown = core === 'markdown'
  const extensions = useMemo<Extension>(() => {
    const lang = markdown ? null : getLanguageExtension(filePath)
    return [getThemeExtension(theme), getHighlightExtension(theme), ...(lang ? [lang] : [])]
  }, [theme, filePath, markdown])
  return <LiveDocView scope={scope} filePath={filePath} markdown={markdown} extensions={extensions} onCoreOff={onCoreOff} />
}

export default LiveDocPane
