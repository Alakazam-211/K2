// Video in K2's voice rooms: the app's video controller over Sew's plugin. The
// logic and its tests live in `stores/video-core.ts`.
//
// Dark behind the server's `videoRoomsPreview` setting (off by default): with it
// off, nothing loads and no video command is ever sent.

import { useStore } from 'zustand'
import { Channel, invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { isWebClient } from '@/lib/is-web'
import { NativeVideoEngine, type NativeVideoBridge } from '@/lib/voice/native-video-engine'
import type { NativeStatus } from '@/lib/voice/native-voice-engine'
import { webCodecsEnv } from '@/lib/voice/video-decoder'
import type { PrivacyPane } from '@/lib/voice/video-labels'
import { useSettingsStore } from '@/stores/settings'
import { useToastStore } from '@/stores/toast'
import { voice } from '@/stores/voice'
import { createVideoController, type TileBox, type VideoController, type VideoPrefs, type VideoState } from './video-core'

const PREFS_KEY = 'k2.voice.videoPrefs.v1'

function loadPrefs(): VideoPrefs {
  try {
    const raw = localStorage.getItem(PREFS_KEY)
    const p = raw ? (JSON.parse(raw) as Partial<VideoPrefs>) : {}
    return { cameraId: typeof p.cameraId === 'string' ? p.cameraId : null }
  } catch {
    return { cameraId: null }
  }
}

function savePrefs(p: VideoPrefs): void {
  try {
    localStorage.setItem(PREFS_KEY, JSON.stringify(p))
  } catch {
    // Per-device convenience only: the call uses the default camera without it.
  }
}

const bridge: NativeVideoBridge = {
  invoke: (command, args) => invoke(command, args),
  listen: (event, handler) => listen(event, (e) => handler(e.payload)),
  channel: (onMessage) => new Channel<unknown>(onMessage),
}

async function loadEngine(): Promise<NativeVideoEngine | null> {
  if (isWebClient()) return null
  const status = await invoke<NativeStatus>('voice_native_status')
  return status.available ? new NativeVideoEngine(bridge) : null
}

/** A tile's size (ResizeObserver), whether it's on screen (IntersectionObserver)
 *  and whether the window is showing (visibilitychange). */
export function watchTileBox(el: HTMLElement, onBox: (box: TileBox) => void): () => void {
  let width = el.clientWidth
  let height = el.clientHeight
  let intersecting = true
  const shown = (): boolean => typeof document === 'undefined' || document.visibilityState !== 'hidden'
  const tell = (): void => onBox({ width, height, visible: intersecting && shown() })
  const ro =
    typeof ResizeObserver === 'function'
      ? new ResizeObserver((entries) => {
          const r = entries[entries.length - 1]?.contentRect
          if (!r) return
          width = r.width
          height = r.height
          tell()
        })
      : null
  ro?.observe(el)
  const io =
    typeof IntersectionObserver === 'function'
      ? new IntersectionObserver((entries) => {
          const e = entries[entries.length - 1]
          if (!e) return
          intersecting = e.isIntersecting
          tell()
        })
      : null
  io?.observe(el)
  document.addEventListener('visibilitychange', tell)
  tell()
  return () => {
    ro?.disconnect()
    io?.disconnect()
    document.removeEventListener('visibilitychange', tell)
  }
}

const PRIVACY_COMMAND: Record<PrivacyPane, string> = {
  camera: 'permissions_open_camera_settings',
  screen: 'permissions_open_screen_settings',
}

export const video: VideoController = createVideoController({
  loadEngine,
  webCodecs: webCodecsEnv,
  watchBox: watchTileBox,
  devicePixelRatio: () => (typeof window === 'undefined' ? 1 : window.devicePixelRatio || 1),
  loadPrefs,
  savePrefs,
  toast: (text, kind) => useToastStore.getState().addToast(text, kind === 'error' ? 'error' : 'info'),
  openPrivacySettings: (pane) => invoke<boolean>(PRIVACY_COMMAND[pane]),
  now: () => Date.now(),
})

function sync(): void {
  video.setActive({ enabled: useSettingsStore.getState().videoRoomsPreview === true, inCall: voice.store.getState().call !== null })
}

let retains = 0
let release: (() => void) | null = null

/** Follow the setting and the call while a headset button is mounted. */
export function retainVideo(): () => void {
  retains += 1
  if (retains === 1) {
    const a = useSettingsStore.subscribe((s, prev) => {
      if (s.videoRoomsPreview !== prev.videoRoomsPreview) sync()
    })
    const b = voice.store.subscribe((s, prev) => {
      if ((s.call === null) !== (prev.call === null)) sync()
    })
    sync()
    release = () => {
      a()
      b()
      video.stop()
    }
  }
  return () => {
    retains -= 1
    if (retains === 0 && release) {
      release()
      release = null
    }
  }
}

export function useVideo<T>(sel: (s: VideoState) => T): T {
  return useStore(video.store, sel)
}
