import type { JSX } from 'react'
import { ThreadViewIdentity } from './ThreadViewIdentity'
import { ThreadFloatSearch } from './ThreadFloatSearch'
import { THREAD_FLOATS_GRID, ThreadFloatScrim, threadFloatShown, useThreadFloats, type ThreadFloatId } from './threadFloats'
import { useSplitColumnFloatsSource } from './splitChrome'

/** Whether split view's floats column draws `id`: SPLIT_VIEW_THREAD_FLOATS,
 *  and in a Chatter column only what Chatter view shows too (no bell). */
export function splitColumnFloatShown(id: ThreadFloatId, kind: 'thread' | 'chatter'): boolean {
  return threadFloatShown(id, 'split') && (kind === 'thread' || threadFloatShown(id, 'chatter'))
}

/**
 * Split view's Thread column (else its Chatter column): Thread view's
 * floating objects that belong to the Thread, floating over the top of the
 * column, under the split bar. The bell and search at the top left, the
 * picture and name in the middle; Thread view's look, scrim and narrow rules
 * (`data-thread-floats`), and its search, which searches this column. The
 * column is the floats' host: the list pads by their height and scrolls
 * under them. First in the column, so first in its tab order.
 */
export function SplitColumnFloats({ kind }: { kind: 'thread' | 'chatter' }): JSX.Element | null {
  const source = useSplitColumnFloatsSource()
  const ref = useThreadFloats(source !== null)
  if (!source) return null
  const shows = (id: ThreadFloatId): boolean => splitColumnFloatShown(id, kind)
  return (
    <div
      ref={ref}
      className={THREAD_FLOATS_GRID}
      data-testid="split-column-floats"
      data-split-column-floats={kind}
      data-thread-floats=""
    >
      <ThreadFloatScrim />
      <div className="relative z-[1] flex min-w-0 items-center gap-1.5 thread-float-group">
        {shows('bell') ? source.bell : null}
        {shows('search') ? <ThreadFloatSearch /> : null}
      </div>
      <div className="relative z-[1] min-w-0">
        {shows('identity') ? <ThreadViewIdentity name={source.name} addr={source.addr} /> : null}
      </div>
      <div className="relative z-[1] flex min-w-0 items-center justify-end gap-1.5 thread-float-group" />
    </div>
  )
}
