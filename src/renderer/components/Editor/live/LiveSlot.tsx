// Where the live doc's editor shows in the frame (the editors' renderEditor):
// it moves the long-lived host element (useLiveDoc) into place and hands the
// frame's mode to its compartment: the look, the file's language in Source and
// code, Write and Read's page, line numbers, and an update listener for the
// words and the caret. Presence (E4b) puts its extensions in with them, its
// off-screen bands above and below the text, the say row under it, and the
// Follow frame around them.

import { useEffect, useLayoutEffect, useMemo, useRef } from 'react'
import { lineNumbers, EditorView } from '@codemirror/view'
import type { Extension } from '@codemirror/state'
import { getLanguageExtension } from '@/components/FileViewerPane/CodeEditor'
import { getHighlightExtension, getThemeExtension, resolveEditorThemeId } from '@/lib/editor/theme'
import { useSettingsStore } from '@/stores/settings'
import type { EditorMountOptions } from '../EditorFrame'

export interface LiveSlotProps {
  host: HTMLDivElement
  filePath: string
  opts: EditorMountOptions
  configure: (ext: Extension) => void
  /** The text and the caret, after every change (yours and others'). */
  onUpdate: (text: string, cursor: { line: number; col: number }) => void
  /** Presence on the text: flags, holds, the band's watch (made once). */
  presence?: Extension
  /** The extensions went in (a new mode): put the holds back. */
  onConfigured?: () => void
  /** The off-screen bands, and the say row under the text. */
  top?: React.ReactNode
  bottom?: React.ReactNode
  foot?: React.ReactNode
  /** Following someone: the frame's colour. */
  ring?: string | null
}

export function LiveSlot({ host, filePath, opts, configure, onUpdate, presence, onConfigured, top, bottom, foot, ring }: LiveSlotProps): React.JSX.Element {
  const box = useRef<HTMLDivElement>(null)
  const update = useRef(onUpdate)
  update.current = onUpdate
  const theme = resolveEditorThemeId(useSettingsStore((s) => s.editor.theme))
  const { mode, lineNumbers: numbers, extensions } = opts
  const ext = useMemo<Extension>(() => {
    // Write and Read bring their own markdown; Source and code take the file's language.
    const lang = mode === 'source' ? getLanguageExtension(filePath) : null
    return [
      getThemeExtension(theme),
      getHighlightExtension(theme),
      lang ?? [],
      extensions,
      presence ?? [],
      numbers ? lineNumbers() : [],
      EditorView.updateListener.of((u) => {
        if (!u.docChanged && !u.selectionSet) return
        const head = u.state.selection.main.head
        const line = u.state.doc.lineAt(head)
        update.current(u.state.doc.toString(), { line: line.number, col: head - line.from + 1 })
      }),
    ]
  }, [mode, numbers, extensions, filePath, theme, presence])

  useLayoutEffect(() => {
    const el = box.current
    if (!el) return
    el.appendChild(host)
    // Back in the page (after Compare or Preview): measure again.
    const cm = host.querySelector<HTMLElement>('.cm-editor')
    if (cm) EditorView.findFromDOM(cm)?.requestMeasure()
    return () => {
      if (host.parentElement === el) el.removeChild(host)
    }
  }, [host])

  const configured = useRef(onConfigured)
  configured.current = onConfigured
  useEffect(() => {
    configure(ext)
    configured.current?.()
  }, [configure, ext])

  return (
    <div data-mode={mode} data-following={ring ? '' : undefined} style={ring ? ({ '--fc': ring } as React.CSSProperties) : undefined} className="ed-live-slot h-full min-h-0">
      {top}
      <div ref={box} className="ed-live-box relative flex min-h-0 flex-1 flex-col" />
      {bottom}
      {foot}
    </div>
  )
}
