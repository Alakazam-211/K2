// The live 3D scene tab on a real file: the shared shell over the real
// source (the kit's keyed watch and presence on K2's docs socket, kept
// connected by shared/live-connect.ts). Chosen only when the daemon says it
// serves the doc live (live-doc-core `scene`); otherwise the tab plays the
// sample, with its banner, as before. With the real source and the real
// picture there is no Sample data banner at all. One connection per mount,
// made in an effect so every one made is stopped (StrictMode mounts twice).

import { useEffect, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { daemonCliPost } from '@/lib/daemon-cli'
import { createDaemonLiveTransport } from '@/lib/docs-live-daemon'
import type { LiveTransport } from '@/lib/docs-live-transport'
import { KeyedWatchSceneSource } from '@/lib/live-view/scene/keyed-source'
import { keepConnected } from '@/lib/live-view/shared/live-connect'
import type { Transport } from '@k2/mpc-client'
import { LiveFileContext } from '../live-file'
import { LiveShellView } from '../shell/LiveShellView'
import { SCENE_SHELL } from './scene-shell'

export interface SceneLiveProps {
  scope: ServerScope
  /** The file, absolute (the daemon finds its workspace). */
  filePath: string
  width?: number
  /** Tests and shots: the socket (default: the daemon's). */
  createTransport?: (scope: ServerScope, doc: { path: string }) => LiveTransport
}

const nameOf = (p: string): string => (p.split(/[\\/]/).pop() || p).replace(/\.blend$/i, '')

export default function SceneLive({ scope, filePath, width, createTransport }: SceneLiveProps): React.JSX.Element | null {
  const [source, setSource] = useState<KeyedWatchSceneSource | null>(null)
  useEffect(() => {
    const t = (createTransport ?? createDaemonLiveTransport)(scope, { path: filePath })
    const src = new KeyedWatchSceneSource(t as unknown as Transport, { post: (body) => daemonCliPost(scope, 'docs/say', { path: filePath, ...body }), fileName: nameOf(filePath) })
    const c = keepConnected(t, src)
    setSource(src)
    return () => c.stop()
  }, [scope, filePath, createTransport])
  if (!source) return null
  return (
    <LiveFileContext.Provider value={{ path: filePath }}>
      <LiveShellView source={source} config={SCENE_SHELL} width={width} fileName={nameOf(filePath)} />
    </LiveFileContext.Provider>
  )
}
