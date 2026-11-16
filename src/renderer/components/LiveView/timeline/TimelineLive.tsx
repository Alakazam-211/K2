// The live timeline tab on a real file (T7): the shared shell over the real
// source, the kit's keyed watch and presence on K2's docs socket
// (connect.ts). Chosen only when the daemon says it serves the doc (its
// timeline core is on: live-doc-core `timeline`); otherwise the tab plays
// the sample, with its banner, as before. One connection per mount, made in
// an effect so every one made is stopped (StrictMode mounts twice in dev).

import { useEffect, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { daemonCliPost } from '@/lib/daemon-cli'
import { createDaemonLiveTransport } from '@/lib/docs-live-daemon'
import type { LiveTransport } from '@/lib/docs-live-transport'
import { connectTimeline } from '@/lib/live-view/timeline/connect'
import type { KeyedWatchTimelineSource } from '@/lib/live-view/timeline/keyed-source'
import { LiveShellView } from '../shell/LiveShellView'
import { TIMELINE_SHELL } from './timeline-shell'

export interface TimelineLiveProps {
  scope: ServerScope
  /** The file, absolute (the daemon finds its workspace). */
  filePath: string
  width?: number
  /** Tests and shots: the socket (default: the daemon's, through the dial queue). */
  createTransport?: (scope: ServerScope, doc: { path: string }) => LiveTransport
}

const nameOf = (p: string): string => p.split(/[\\/]/).pop() || p

export default function TimelineLive({ scope, filePath, width, createTransport }: TimelineLiveProps): React.JSX.Element | null {
  const [source, setSource] = useState<KeyedWatchTimelineSource | null>(null)
  useEffect(() => {
    const t = (createTransport ?? createDaemonLiveTransport)(scope, { path: filePath })
    const c = connectTimeline(t, { post: (body) => daemonCliPost(scope, 'docs/say', { path: filePath, ...body }) })
    setSource(c.source)
    return () => c.stop()
  }, [scope, filePath, createTransport])
  if (!source) return null
  return <LiveShellView source={source} config={TIMELINE_SHELL} width={width} fileName={nameOf(filePath)} />
}
