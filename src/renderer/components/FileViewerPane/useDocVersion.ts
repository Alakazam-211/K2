// Live re-read for the any-file previews (3D model, image, PDF, large DXF,
// Word, zip, audio/video). The daemon sends `doc_version_changed` when an
// any-file doc (tier 0) gets a new version from any writer: `k2 doc write`
// or `restore`, or another program's save. A preview puts the version this
// hook returns in its load effect's deps, so it re-reads the file's bytes
// (keeping its camera, page or zoom where it can).
//
// Text tabs (DXF included) follow their file by FileViewerPane's own read,
// so they don't need this.

import { useEffect, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { onDocVersionChanged, type DocVersionChangedEvent } from '@/stores/session-events'

/** The event's file, as an absolute path (`workspace` + `rel`). */
export function docEventFilePath(e: Pick<DocVersionChangedEvent, 'workspace' | 'rel'>): string {
  return `${e.workspace.replace(/\/+$/, '')}/${e.rel.replace(/^\/+/, '')}`
}

/**
 * The newest version announced for `filePath` since the preview opened, or
 * `null` before any. The same version twice (the bus repeating it) is no
 * change, so a preview re-reads once per version. A new path starts over.
 */
export function useDocVersion(scope: ServerScope, filePath: string): number | null {
  const [seen, setSeen] = useState<{ path: string; version: number } | null>(null)

  useEffect(() => {
    return onDocVersionChanged(scope, (e) => {
      if (docEventFilePath(e) !== filePath) return
      setSeen((prev) => (prev && prev.path === filePath && prev.version === e.version ? prev : { path: filePath, version: e.version }))
    })
  }, [scope, filePath])

  return seen && seen.path === filePath ? seen.version : null
}
