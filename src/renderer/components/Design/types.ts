// Design mode shared types (prd-browser-design-mode-v1 §00, D-A9–D-A13).
// `NoteElement` mirrors k2-core `browser_notes::NoteElement` (camelCase);
// the daemon clamps every field again, so these are only the wire shape.

export interface NoteRect {
  x: number
  y: number
  width: number
  height: number
}

/** What the in-page picker captured about one element. Untrusted page data. */
export interface NoteElement {
  selector: string
  tag: string
  id?: string
  classes?: string[]
  role?: string
  /** Accessible name. */
  name?: string
  text: string
  html: string
  styles?: Record<string, string>
  /** CSS px in the guest page's viewport. */
  rect?: NoteRect
  nearbyText?: string[]
  /** `file:line` from a dev build's React source info. */
  source?: string
}

export type NoteIntent = 'change' | 'question'

export interface GuestViewport {
  width: number
  height: number
  scrollX: number
  scrollY: number
  dpr: number
}

/** A picked element as the picker IPC reports it. */
export interface PickResult {
  type: 'pick'
  pickId: string
  url: string
  title: string
  viewport: GuestViewport
  element: NoteElement
}

/** One event the picker IPC drains from the page (sibling src-tauri work). */
export type PickEvent = PickResult | { type: 'key'; key: 'escape' }

export type PickPoll = { events: PickEvent[]; navSeq: number } | null

export interface DesignBadge {
  n: number
  selector: string
}

/**
 * The Tauri picker commands, injected so tests use a fake and integration
 * passes `@/lib/design-picker-ipc` (built by the src-tauri slice).
 */
export interface DesignPickerIpc {
  pickArm(itemId: string, parentWindow?: string): Promise<void>
  pickDisarm(itemId: string, parentWindow?: string): Promise<void>
  pickPoll(itemId: string, parentWindow?: string): Promise<PickPoll>
  setDesignBadges(itemId: string, parentWindow: string | undefined, badges: DesignBadge[]): Promise<void>
}

/** One pinned note in the tray (per viewer, never serialized). */
export interface DesignNoteDraft {
  /** Local id (stable while the note lives in the tray). */
  id: string
  comment: string
  intent: NoteIntent
  /** Absent for a whole-page note. */
  element?: NoteElement
  /** Page the element was picked on. */
  url: string
  title: string
  viewport?: { width: number; height: number }
}

/** `POST /cli/browser/notes` and `/redeliver` 200 body. */
export interface DesignNotesResult {
  ok: true
  threadItemId: string
  delivered: boolean
  targetSessionId: string | null
  addr: string
  notes?: number
}
