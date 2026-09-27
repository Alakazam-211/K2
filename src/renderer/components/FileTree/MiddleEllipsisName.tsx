import type { JSX } from 'react'
import { middleEllipsis } from './middleEllipsis'

/**
 * Files-drawer name. Display only — callers must keep using the real
 * name and path for rename, copy, and open.
 * No `truncate`: that class paints a second ellipsis at the end.
 */
export function MiddleEllipsisName({
  name,
  maxWidthPx,
  measure,
  directory = false,
  title,
  className,
}: {
  name: string
  maxWidthPx: number
  measure: (text: string) => number
  directory?: boolean
  /** Full basename tooltip. Omit so a parent path tooltip stays visible. */
  title?: string
  className?: string
}): JSX.Element {
  const shown = middleEllipsis(name, maxWidthPx, measure, { directory })
  return (
    <span
      className={['min-w-0 flex-1 overflow-hidden whitespace-nowrap', className].filter(Boolean).join(' ')}
      title={title}
    >
      {shown}
    </span>
  )
}
