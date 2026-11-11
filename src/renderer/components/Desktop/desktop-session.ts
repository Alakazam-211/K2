// One viewer's side of a desktop stream (prd-remote-desktop-tab-v1 §6, §8):
// who drives, which display is shown, the rate, and what each gesture
// sends. The daemon decides everything (the lease, view-only, the display);
// this keeps what it said and turns a gesture into wire messages, so a
// refusal is rare and the toolbar shows the truth.
//
// Pure (R14): no React, no DOM, no `window`. The phone (P2) lifts it.
//
//  - Take control: `take-control`; the daemon answers with `control`.
//  - Hand back: every key and button this viewer holds is released first
//    (R3), then `hand-back`.
//  - Display: the controller's only (v1: it moves every viewer).
//  - Hidden tab: `fps 0` (pause; the socket stays); shown again: the rate
//    it had, then `keyframe` (its decoder lost its place while paused).

import { HeldInput } from './desktop-input'
import type { Display, Mode, ServerMsg, ViewerMsg } from './desktop-wire'

export interface ControlState {
  /** Who drives (a name), or nobody. */
  controller: string | null
  /** This viewer drives. */
  you: boolean
  /** Someone at the machine is using it: remote input is dropped. */
  localHold: boolean
}

/** Why a gesture sent nothing. */
export type GestureRefusal = 'not_controller' | 'watch_only' | 'already' | 'hidden'

export type Gesture = { send: ViewerMsg[] } | { refused: GestureRefusal }

export class DesktopSession {
  mode: Mode = 'view'
  control: ControlState = { controller: null, you: false, localHold: false }
  displays: Display[] = []
  display: string | null = null
  /** The rate the daemon gave while shown (what a shown tab asks again). */
  fps: number | null = null
  hidden = false
  /** The viewer may drive at all (Owner / Admin; `status.you.mayControl`). */
  mayControl: boolean
  readonly held = new HeldInput()

  constructor(opts: { mayControl: boolean }) {
    this.mayControl = opts.mayControl
  }

  /** A daemon message. Returns true when something the toolbar shows moved. */
  onServer(m: ServerMsg): boolean {
    switch (m.t) {
      case 'hello':
        this.mode = m.mode
        this.displays = m.displays
        this.display = m.display
        if (!this.hidden) this.fps = m.fps
        this.control = { controller: m.controller, you: m.mode === 'control', localHold: this.control.localHold }
        return true
      case 'control':
        this.control = { controller: m.controller, you: m.you, localHold: m.localHold }
        this.mode = m.you ? 'control' : 'view'
        return true
      case 'displays':
        this.displays = m.displays
        if (m.display) this.display = m.display
        return true
      case 'fps':
        // The answer to a pause is the daemon's floor, not a rate to keep.
        if (this.hidden) return false
        this.fps = m.fps
        return true
      default:
        return false
    }
  }

  /** May this viewer's input go now? */
  drives(viewOnly: boolean): boolean {
    return this.control.you && !viewOnly && !this.hidden
  }

  takeControl(): Gesture {
    if (!this.mayControl) return { refused: 'watch_only' }
    if (this.control.you) return { refused: 'already' }
    return { send: [{ t: 'take-control' }] }
  }

  /** Release what is held, then hand back. */
  handBack(): Gesture {
    if (!this.control.you) return { refused: 'not_controller' }
    return { send: [...this.held.releaseAll(), { t: 'hand-back' }] }
  }

  /** Every key and button held, released (blur, hide, view-only). */
  releaseHeld(): ViewerMsg[] {
    return this.held.releaseAll()
  }

  /** Switch display: the controller's only. */
  selectDisplay(id: string): Gesture {
    if (!this.control.you) return { refused: 'not_controller' }
    if (id === this.display || !this.displays.some((d) => d.id === id)) return { refused: 'already' }
    return { send: [...this.held.releaseAll(), { t: 'display', id }] }
  }

  clipboardSet(text: string): Gesture {
    if (!this.control.you) return { refused: 'not_controller' }
    return { send: [{ t: 'clipboard-set', text }] }
  }

  clipboardGet(): Gesture {
    if (!this.control.you) return { refused: 'not_controller' }
    return { send: [{ t: 'clipboard-get' }] }
  }

  /** View only on or off (the daemon drops a controller to watching). */
  setViewOnly(on: boolean): ViewerMsg[] {
    return on ? [...this.held.releaseAll(), { t: 'view', on: true }] : [{ t: 'view', on: false }]
  }

  /** The tab was hidden or shown. Hidden: release what is held, pause
   *  (`fps 0`). Shown: the rate it had, then a keyframe. */
  setHidden(hidden: boolean): ViewerMsg[] {
    if (hidden === this.hidden) return []
    this.hidden = hidden
    if (hidden) return [...this.held.releaseAll(), { t: 'fps', fps: 0 }]
    const out: ViewerMsg[] = []
    if (this.fps && this.fps > 0) out.push({ t: 'fps', fps: this.fps })
    out.push({ t: 'keyframe' })
    return out
  }

  /** The current display's facts, if known. */
  currentDisplay(): Display | null {
    return this.displays.find((d) => d.id === this.display) ?? null
  }

  /** Note an input that went (so a drop can release it, R3). */
  sent(m: ViewerMsg): void {
    this.held.note(m)
  }
}
