// Design mode picker IPC (prd-browser-design-mode-v1 D-A11). Thin typed
// wrappers over the Tauri commands in src-tauri/src/commands/browser_webviews.rs.
//
// Only Design views have the picker: the Design pane must create its page
// with `invoke('browser_create', { …, design: true })`. A plain Browser tab
// (no flag) gets "not a design view" from every call here.
//
// Every value in a PickEvent came from the page and was clamped in Rust
// (k2_core::browser_notes); treat it as display data, never markup.
import { invoke } from '@tauri-apps/api/core'

export interface NoteRect {
  x: number
  y: number
  width: number
  height: number
}

/** Mirrors k2_core::browser_notes::NoteElement (camelCase). */
export interface NoteElement {
  selector: string
  tag: string
  id?: string
  classes?: string[]
  role?: string
  name?: string
  text: string
  html: string
  styles?: Record<string, string>
  rect?: NoteRect
  nearbyText?: string[]
  source?: string
}

export interface PickViewport {
  width: number
  height: number
  scrollX: number
  scrollY: number
  dpr: number
}

export interface PickPayload {
  type: 'pick'
  pickId: string
  url: string
  title: string
  viewport: PickViewport
  element: NoteElement
}

export interface PickKey {
  type: 'key'
  key: 'escape'
}

export type PickEvent = PickPayload | PickKey

export type PickPoll = { events: PickEvent[]; navSeq: number } | null

export interface DesignBadge {
  n: number
  selector: string
}

interface Target {
  itemId: string
  parentWindow?: string
}

export function pickArm({ itemId, parentWindow }: Target): Promise<void> {
  return invoke<void>('browser_pick_arm', { itemId, parentWindow })
}

export function pickDisarm({ itemId, parentWindow }: Target): Promise<void> {
  return invoke<void>('browser_pick_disarm', { itemId, parentWindow })
}

/** Drain picks and Esc presses. Poll ~120 ms while armed. Do not gate on
 * the pane being visible: a frozen pane is hidden but still answers. */
export function pickPoll({ itemId, parentWindow }: Target): Promise<PickPoll> {
  return invoke<PickPoll>('browser_pick_poll', { itemId, parentWindow })
}

export function setDesignBadges(
  { itemId, parentWindow }: Target,
  badges: DesignBadge[],
): Promise<void> {
  return invoke<void>('browser_design_badges', { itemId, parentWindow, badges })
}
