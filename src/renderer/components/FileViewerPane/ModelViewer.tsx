// 3D model preview for FileViewerPane: STL / OBJ / 3MF / PLY / GLB / glTF
// in a three.js canvas. Ported from the K2 Companion
// (`src/zen/files/ModelView.tsx`).
//
// Bytes come from the room's daemon through `loadHostBinary`
// (`fs/read-binary`, ranged `fs/read-range` assembly past its 50 MB cap),
// so remote rooms and the web client work like the other binary previews.
// three.js and the loaders are dynamic imports (`./modelScene`, then one
// loader chunk per format): nothing here is in the start-up bundle.

import { useEffect, useRef, useState } from 'react'
import { useRoom } from '@/components/Room/RoomContext'
import { isHostSwitchedError } from '@/lib/daemon-cli'
import { loadHostBinary } from '@/lib/load-host-binary'
import { isRemoteMacTmpError } from '@/lib/remote-mac-tmp'
import { getFileExtension } from './fileCategory'
import type { ModelStats } from './modelParse'
import { ViewerNote } from './ViewerNote'

/** Desktop preview cap (the phone stops at the daemon's 50 MB). */
export const MODEL_MAX_BYTES = 200 * 1024 * 1024

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / (1024 * 1024)).toFixed(1)} MB`
}

/** "Too large" note for a model of `size` bytes, or null if it fits. */
export function modelSizeError(size: number): string | null {
  return size > MODEL_MAX_BYTES
    ? `${formatBytes(size)} is over the ${formatBytes(MODEL_MAX_BYTES)} preview limit.`
    : null
}

const fmtDim = (v: number): string => {
  const a = Math.abs(v)
  if (a === 0) return '0'
  // Trailing zeros dropped: "60", not "60.0".
  return String(
    Number(a >= 100 ? v.toFixed(0) : a >= 10 ? v.toFixed(1) : a >= 0.1 ? v.toFixed(2) : v.toPrecision(2)),
  )
}

export function statsLine(s: ModelStats): string {
  const tris = `${s.triangles.toLocaleString()} triangle${s.triangles === 1 ? '' : 's'}`
  return `${tris} · ${s.size.map(fmtDim).join(' × ')} units`
}

type Phase =
  | { k: 'fetching' }
  | { k: 'building' }
  | { k: 'ready' }
  | { k: 'nogl' }
  | { k: 'error'; title: string; detail: string }

export function ModelViewer({ filePath }: { filePath: string }): React.JSX.Element {
  // Home M4: the room's server (a viewer in B's room reads B's file).
  const scope = useRoom().scope
  const [stats, setStats] = useState<ModelStats | null>(null)
  const [phase, setPhase] = useState<Phase>({ k: 'fetching' })
  const [progress, setProgress] = useState<number | null>(null)
  const hostRef = useRef<HTMLDivElement>(null)
  const resetRef = useRef<(() => void) | null>(null)
  const ext = getFileExtension(filePath)

  useEffect(() => {
    let cancelled = false
    let dispose: (() => void) | null = null
    const ac = new AbortController()
    // The room's connection identity: a server switch mid-read drops it.
    const startedKey = scope.connectionKey
    setStats(null)
    setProgress(null)
    setPhase({ k: 'fetching' })
    resetRef.current = null

    void (async () => {
      let bytes: Uint8Array
      try {
        bytes = await loadHostBinary(scope, filePath, {
          allowRangeAssembly: true,
          signal: ac.signal,
          onProgress: (ratio) => {
            if (!cancelled) setProgress(ratio)
          },
        })
      } catch (e) {
        if (cancelled || isHostSwitchedError(e)) return
        if (e instanceof Error && e.name === 'AbortError') return
        if (isRemoteMacTmpError(e)) {
          setPhase({ k: 'error', title: 'Not available on this server', detail: filePath })
          return
        }
        const msg = e instanceof Error ? e.message : String(e)
        const big = /too large|50mb/i.test(msg)
        setPhase({ k: 'error', title: big ? 'Too large to preview' : 'Could not load the model', detail: msg })
        return
      }
      if (cancelled || scope.connectionKey !== startedKey) return
      const tooBig = modelSizeError(bytes.byteLength)
      if (tooBig) {
        setPhase({ k: 'error', title: 'Too large to preview', detail: tooBig })
        return
      }
      setPhase({ k: 'building' })
      try {
        const m = await import('./modelScene')
        const parsed = await m.parseModel(bytes, ext)
        if (cancelled) {
          m.disposeObject(parsed.root)
          return
        }
        setStats(parsed.stats)
        const host = hostRef.current
        if (!host || !m.hasWebGL2()) {
          m.disposeObject(parsed.root)
          setPhase({ k: 'nogl' })
          return
        }
        try {
          const viewer = m.createViewer(host, parsed.root)
          dispose = viewer.dispose
          resetRef.current = viewer.reset
          setPhase({ k: 'ready' })
        } catch {
          m.disposeObject(parsed.root)
          setPhase({ k: 'nogl' })
        }
      } catch (e) {
        if (!cancelled) {
          setPhase({ k: 'error', title: 'Could not open the model', detail: e instanceof Error ? e.message : String(e) })
        }
      }
    })()

    return () => {
      cancelled = true
      ac.abort()
      resetRef.current = null
      dispose?.()
    }
    // `scope` is read live; its connection key re-runs the load on a switch.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filePath, ext, scope.connectionKey])

  const note =
    phase.k === 'error' ? (
      <ViewerNote tone="error" title={phase.title} detail={phase.detail} testId="model-error" />
    ) : phase.k === 'nogl' ? (
      <ViewerNote title="3D preview needs WebGL" detail="This web view did not start WebGL 2." testId="model-nogl" />
    ) : phase.k === 'fetching' ? (
      <ViewerNote
        title="Loading model…"
        detail={progress !== null && progress < 1 ? `${Math.round(progress * 100)}%` : null}
      />
    ) : phase.k === 'building' ? (
      <ViewerNote title="Building the model…" />
    ) : null

  return (
    <div className="flex h-full w-full flex-col" data-testid="file-model">
      {stats ? (
        <div className="flex shrink-0 items-center gap-2 px-3 py-1 text-[11px] text-[var(--color-text-muted)]">
          <span className="min-w-0 flex-1 truncate" data-testid="model-stats">
            {statsLine(stats)}
          </span>
          {phase.k === 'ready' ? (
            <button
              type="button"
              className="px-1.5 py-0.5 text-[10px] font-medium border border-[var(--color-border)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors"
              onClick={() => resetRef.current?.()}
              title="Reset the view (double-click)"
              data-testid="model-reset"
            >
              Reset view
            </button>
          ) : null}
        </div>
      ) : null}
      <div className="relative min-h-0 flex-1 overflow-hidden">
        <div ref={hostRef} className="absolute inset-0" data-testid="model-host" />
        {note ? <div className="absolute inset-0">{note}</div> : null}
        {phase.k === 'ready' ? (
          <div className="pointer-events-none absolute bottom-2.5 left-3 right-3 text-center text-[11px] text-[var(--color-text-muted)]">
            Drag to orbit · right-drag or Shift-drag to pan · scroll to zoom · double-click to reset
          </div>
        ) : null}
      </div>
    </div>
  )
}
