// The native self-view (0.46.17, macOS, behind videoRoomsPreview): your own
// camera in a small window of its own beside the call, drawn by the OS from
// the camera's own frames (Sew's `local:camera` native sink into an
// AVSampleBufferDisplayLayer; src-tauri/src/native_voice/self_view.rs)
// instead of this page's self-view tile (the webview sink, a VP8 encode, the
// bridge and a WebCodecs decode).
//
// The page says, from the window that shows the call, whether it wants the
// self-view: your camera is on or paused there (`voice_self_view {want,
// anchor}`, anchor: the call view's rect, for where the window sits). Rust
// answers every window with `self-view:state {available, showing, reason}`:
// while `showing`, the page's own self-view tile hides (the stage's grid
// leaves it out; a floating card that would show it shows your picture
// instead), and whenever it isn't (another OS, the sink refused, the window
// closed by the person, the camera paused) the page's tile is there as
// before. Never a blank tile.

import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { useEffect, useRef } from 'react'
import { create } from 'zustand'
import { isWebClient } from '@/lib/is-web'
import { peopleTiles } from './call-share'
import { videoLive } from './call-tile-picture'
import { LOCAL_CAMERA_TRACK, type LocalVideoState } from './video-engine'
import type { CallPerson, VideoTile } from './video-labels'

/** Rust's event to every window. */
export const SELF_VIEW_EVENT = 'self-view:state'

/** A rect in this window, in CSS pixels from its top left. */
export interface SelfViewAnchor {
  x: number
  y: number
  width: number
  height: number
}

export interface NativeSelfViewState {
  /** This app draws the self-view natively (macOS). */
  available: boolean
  /** It is up now: the page's own self-view tile hides. */
  showing: boolean
  /** Why it isn't up while wanted: `closed`, `open_refused:<code>`,
   *  `no_pixel_buffer`, `window:<why>`. Null otherwise. */
  reason: string | null
}

const NOT_SHOWING: NativeSelfViewState = { available: false, showing: false, reason: null }

export const useNativeSelfView = create<NativeSelfViewState>(() => NOT_SHOWING)

/** The event's payload, or null when it isn't one. */
export function parseSelfViewState(v: unknown): NativeSelfViewState | null {
  if (!v || typeof v !== 'object' || Array.isArray(v)) return null
  const r = v as Record<string, unknown>
  if (typeof r.available !== 'boolean' || typeof r.showing !== 'boolean') return null
  if (r.reason !== null && r.reason !== undefined && typeof r.reason !== 'string') return null
  return { available: r.available, showing: r.showing, reason: typeof r.reason === 'string' ? r.reason : null }
}

/** This window wants the self-view: the call shows here and your camera is
 *  on or paused (paused keeps the sink; the window hides until it runs). */
export function selfViewWanted(camera: LocalVideoState, callHere: boolean): boolean {
  return callHere && (camera === 'on' || camera === 'paused')
}

/** Your own camera's tile, which the native self-view draws instead. */
export function isOwnCameraTile(t: VideoTile): boolean {
  return t.self && t.source === 'camera' && t.key === LOCAL_CAMERA_TRACK
}

/** The stage's tiles while the native self-view shows: yours left out. With
 *  nobody else's video then, everyone as their picture (you included), as
 *  the stage draws a call with no video, so it is never empty. */
export function stageTilesForNativeSelf(tiles: readonly VideoTile[], people: readonly CallPerson[], showing: boolean): VideoTile[] {
  if (!showing || !tiles.some(isOwnCameraTile)) return [...tiles]
  const rest = tiles.filter((t) => !isOwnCameraTile(t))
  return videoLive(rest) ? rest : peopleTiles(rest, people)
}

/** The call view's rect in this window, for where the self-view sits: the
 *  stage's grid or its share, when one is on screen. */
export function callViewAnchor(doc: Document | null = typeof document === 'undefined' ? null : document): SelfViewAnchor | null {
  const el = doc?.querySelector('[data-testid="video-grid-box"], [data-testid="call-share-main"]')
  if (!el) return null
  const r = el.getBoundingClientRect()
  if (r.width <= 0 || r.height <= 0) return null
  return { x: r.left, y: r.top, width: r.width, height: r.height }
}

export interface SelfViewOps {
  /** Rust: this window wants it or not; answers whether it draws it. */
  tell(want: boolean, anchor: SelfViewAnchor | null): Promise<boolean>
  /** Rust's state events; returns the unlisten. */
  listen(onState: (payload: unknown) => void): Promise<() => void>
}

function inApp(): boolean {
  return !isWebClient() && typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

const nativeOps: SelfViewOps = {
  tell: async (want, anchor) => {
    if (!inApp()) return false
    return (await invoke<unknown>('voice_self_view', { want, anchor })) === true
  },
  listen: async (onState) => {
    if (!inApp()) return () => {}
    return listen(SELF_VIEW_EVENT, (e) => onState(e.payload))
  },
}

let ops: SelfViewOps = nativeOps
let listening: Promise<() => void> | null = null

function ensureListening(): void {
  if (listening) return
  listening = ops
    .listen((payload) => {
      const s = parseSelfViewState(payload)
      if (s) useNativeSelfView.setState(s)
    })
    .catch((err: unknown) => {
      console.warn('[call] self-view state not followed:', err)
      return () => {}
    })
}

/** While mounted in the window that shows the call: tell Rust whether this
 *  window wants the self-view (on each change, and no longer on unmount),
 *  and follow its state. */
export function useNativeSelfViewWant(want: boolean): void {
  const wanted = useRef(false)
  useEffect(() => {
    ensureListening()
  }, [])
  useEffect(() => {
    if (!want && !wanted.current) return
    wanted.current = want
    ops.tell(want, want ? callViewAnchor() : null).catch((err: unknown) => {
      // The page keeps its own tile: nothing else to do.
      console.warn('[call] native self-view not asked:', err)
    })
  }, [want])
  useEffect(
    () => () => {
      if (!wanted.current) return
      wanted.current = false
      ops.tell(false, null).catch(() => {})
    },
    [],
  )
}

export function __setSelfViewOpsForTests(next: SelfViewOps | null): void {
  ops = next ?? nativeOps
  listening = null
  useNativeSelfView.setState(NOT_SHOWING)
}
