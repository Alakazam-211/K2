// Today's markdown renderer (react-markdown with GFM, highlighted fences):
// the file viewer's Preview, and the editor frame's Read until E2 makes Read
// the read-only Write view (build-editors.md E1b). Moved out of
// FileViewerPane unchanged.

import React from 'react'
import remarkGfm from 'remark-gfm'
import Markdown from '@/components/Markdown/Markdown'
import { HighlightedCodeBlock } from './CodeHighlighter'

/**
 * Module-level ReactMarkdown components map. Hoisted out of the
 * render function so its reference is stable across renders — if
 * we rebuilt this object inside the component body, ReactMarkdown
 * would get a new prop ref every render and rebuild its element
 * tree, which destroys any active DOM text selection (the user
 * loses their highlight every time the parent re-renders).
 */
const MARKDOWN_COMPONENTS = {
  code({ className, children, ...props }: React.HTMLAttributes<HTMLElement> & { children?: React.ReactNode }) {
    const match = /language-(\w+)/.exec(className || '')
    const codeStr = String(children).replace(/\n$/, '')
    if (match) {
      return <HighlightedCodeBlock code={codeStr} language={match[1]} />
    }
    return <code className={className} {...props}>{children}</code>
  },
}
const MARKDOWN_REMARK_PLUGINS = [remarkGfm]

/**
 * Memoized markdown body. Only re-renders when `content` actually
 * changes — without this, every parent re-render (dirty-state ticks,
 * settings polls, tab activity, etc.) rebuilt ReactMarkdown's element
 * tree, and the browser's active text selection collapsed. React.memo
 * with default shallow prop comparison stops the cascade.
 */
export const MemoizedMarkdown = React.memo(function MemoizedMarkdown({
  content,
}: {
  content: string
}): React.JSX.Element {
  return (
    <Markdown
      remarkPlugins={MARKDOWN_REMARK_PLUGINS}
      components={MARKDOWN_COMPONENTS}
    >
      {content}
    </Markdown>
  )
})
