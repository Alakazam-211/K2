// Tier 0 kept both versions of this file (kit v0.14.0): another program saved
// over an agent's change (or a version saved under someone's lock), and the
// replaced version is a conflict copy. The daemon sends `doc_conflict_copy`
// once per copy with the text to show (the same line goes to the workspace's
// Thread); this tab shows it once, until dismissed.

import React, { useEffect, useState } from 'react'
import { Button, Callout } from '@/components/ui'
import type { ServerScope } from '@/kessel/server-scope'
import { onDocConflictCopy, type DocConflictCopyEvent } from '@/stores/session-events'

/** The event's file, as an absolute path (`workspace` + `rel`). */
export function conflictFilePath(e: Pick<DocConflictCopyEvent, 'workspace' | 'rel'>): string {
  return `${e.workspace.replace(/\/+$/, '')}/${e.rel.replace(/^\/+/, '')}`
}

interface Shown {
  key: string
  text: string
}

export function DocConflictNotice({ scope, filePath }: { scope: ServerScope; filePath: string }): React.JSX.Element | null {
  const [shown, setShown] = useState<Shown[]>([])

  useEffect(() => {
    setShown([])
    return onDocConflictCopy(scope, (e) => {
      if (conflictFilePath(e) !== filePath) return
      const key = `${e.projectId}:${e.current}:${e.kept}`
      // Once per copy, even if the bus repeats it.
      setShown((prev) => (prev.some((s) => s.key === key) ? prev : [...prev, { key, text: e.text }]))
    })
  }, [scope, filePath])

  if (shown.length === 0) return null
  return (
    <div className="flex flex-col gap-1 border-b border-[var(--color-border)] px-2 py-1 flex-shrink-0" data-testid="doc-conflict-notices">
      {shown.map((s) => (
        <Callout key={s.key} tone="info" role="status" className="flex items-center gap-2" data-testid="doc-conflict-notice">
          <span className="min-w-0 flex-1 break-words">{s.text}</span>
          <Button variant="quiet" onClick={() => setShown((prev) => prev.filter((p) => p.key !== s.key))}>
            Dismiss
          </Button>
        </Callout>
      ))}
    </div>
  )
}
