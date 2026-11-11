// The Desktop tab's toolbar (prd-remote-desktop-tab-v1 §8). The controls
// that need the live stream are disabled until it is open, with a title
// saying so. The machine's name is always shown (vs-live R22: never a
// silent switch to another machine).
//
//   [machine] [display ▾] [Fit|1:1] [View only] [Capture keys] [Mac↔PC] [Keys ▾]
//   [Send clipboard] [Copy theirs] [Take control | Hand back] … [driver] [viewers] [latency]

import type { DesktopFit } from './desktop-serialize'
import { ESCAPE_CHORD_LABEL, type KeysMenuEntry } from './desktop-keymap'
import type { Display } from './desktop-wire'

export const NEEDS_STREAM_TITLE = 'Waiting for the live picture.'
export const DISPLAY_CONTROLLER_TITLE = 'Only the person in control can switch the display (it moves everyone watching).'

export interface DesktopToolbarProps {
  machine: string
  displays: Display[]
  displayId: string | null
  fit: DesktopFit
  viewOnly: boolean
  /** This viewer may drive (Owner / Admin). */
  mayControl: boolean
  capture: boolean
  flipModifiers: boolean
  /** The stream is open. */
  live: boolean
  /** This viewer holds control. */
  controlling?: boolean
  /** Who drives, when someone else does. */
  controller?: string | null
  /** Someone at the machine is using it (remote input paused). */
  localHold?: boolean
  keys?: readonly KeysMenuEntry[]
  viewers: number | null
  latencyMs: number | null
  onDisplay(id: string): void
  onFit(fit: DesktopFit): void
  onViewOnly(on: boolean): void
  onCapture(on: boolean): void
  onFlipModifiers(on: boolean): void
  onTakeControl?(): void
  onHandBack?(): void
  onClipSend?(): void
  onClipCopy?(): void
  onKeys?(entry: KeysMenuEntry): void
}

const BTN =
  'h-6 flex-shrink-0 cursor-pointer border border-[var(--color-border)] px-2 text-xs text-[var(--color-text-primary)] transition-colors enabled:hover:border-[var(--color-text-muted)] disabled:cursor-default disabled:opacity-50'
const ON = 'border-[var(--color-accent)] text-[var(--color-accent)]'
const SELECT = 'h-6 flex-shrink-0 border border-[var(--color-border)] bg-[var(--color-bg)] px-1 text-xs text-[var(--color-text-primary)] disabled:opacity-50'

export function DesktopToolbar(props: DesktopToolbarProps): React.JSX.Element {
  const streamTitle = props.live ? undefined : NEEDS_STREAM_TITLE
  const controlling = !!props.controlling
  const drives = props.live && controlling && !props.viewOnly
  const controlTitle = !props.live
    ? NEEDS_STREAM_TITLE
    : !props.mayControl
      ? 'You can only watch this screen'
      : controlling
        ? 'Hand control back; keep watching'
        : props.controller
          ? `${props.controller} is in control; take it when they've stopped`
          : 'Take control of the keyboard and mouse'
  const clipTitle = (what: string): string | undefined => streamTitle ?? (controlling ? what : 'Take control first')
  return (
    <div
      className="flex h-10 flex-shrink-0 items-center gap-1.5 overflow-x-auto border-b border-[var(--color-border)] px-2 text-xs"
      data-desktop-toolbar=""
    >
      <span className="flex-shrink-0 truncate font-medium text-[var(--color-text-primary)]" data-desktop-machine="">
        {props.machine}
      </span>
      <select
        className={SELECT}
        aria-label="Display"
        data-desktop-display-picker=""
        value={props.displayId ?? ''}
        disabled={props.displays.length === 0 || (props.live && !controlling)}
        title={props.live && !controlling ? DISPLAY_CONTROLLER_TITLE : undefined}
        onChange={(e) => props.onDisplay(e.target.value)}
      >
        {props.displays.length === 0 ? <option value="">No display yet</option> : null}
        {props.displays.map((d) => (
          <option key={d.id} value={d.id}>
            {d.name}
            {d.main ? ' (main)' : ''}
          </option>
        ))}
      </select>
      <button
        type="button"
        className={BTN}
        data-desktop-fit={props.fit}
        title={props.fit === 'fit' ? 'Show at 1:1' : 'Fit the picture to the pane'}
        onClick={() => props.onFit(props.fit === 'fit' ? '1:1' : 'fit')}
      >
        {props.fit === 'fit' ? 'Fit' : '1:1'}
      </button>
      <button
        type="button"
        className={`${BTN} ${props.viewOnly ? ON : ''}`}
        data-desktop-view-only={String(props.viewOnly)}
        disabled={!props.mayControl}
        title={props.mayControl ? 'Only watch: send no input' : 'You can only watch this screen'}
        onClick={() => props.onViewOnly(!props.viewOnly)}
      >
        View only
      </button>
      <button
        type="button"
        className={`${BTN} ${props.capture ? ON : ''}`}
        data-desktop-capture={String(props.capture)}
        disabled={!props.mayControl || props.viewOnly}
        title={`Send K2's own shortcuts to the remote desktop too; ${ESCAPE_CHORD_LABEL} leaves`}
        onClick={() => props.onCapture(!props.capture)}
      >
        Capture keys
      </button>
      <button
        type="button"
        className={`${BTN} ${props.flipModifiers ? ON : ''}`}
        data-desktop-flip={String(props.flipModifiers)}
        disabled={!props.mayControl || props.viewOnly}
        title="Swap ⌘ and Ctrl between a Mac and a PC"
        onClick={() => props.onFlipModifiers(!props.flipModifiers)}
      >
        Mac ↔ PC
      </button>
      <select
        className={SELECT}
        aria-label="Keys"
        data-desktop-keys-menu=""
        value=""
        disabled={!drives}
        title={streamTitle ?? (drives ? 'Send a chord K2 or your computer would take' : 'Take control first')}
        onChange={(e) => {
          const entry = props.keys?.find((k) => k.id === e.target.value)
          if (entry) props.onKeys?.(entry)
        }}
      >
        <option value="">Keys</option>
        {(props.keys ?? []).map((k) => (
          <option key={k.id} value={k.id}>
            {k.label}
          </option>
        ))}
      </select>
      <button type="button" className={BTN} data-desktop-clip-send="" disabled={!drives} title={clipTitle('Send your clipboard text')} onClick={() => props.onClipSend?.()}>
        Send clipboard
      </button>
      <button type="button" className={BTN} data-desktop-clip-copy="" disabled={!drives} title={clipTitle("Copy the remote desktop's clipboard text")} onClick={() => props.onClipCopy?.()}>
        Copy theirs
      </button>
      <button
        type="button"
        className={`${BTN} ${controlling ? ON : ''}`}
        data-desktop-take-control={controlling ? 'hand-back' : 'take'}
        disabled={!props.live || !props.mayControl || props.viewOnly}
        title={controlTitle}
        onClick={() => (controlling ? props.onHandBack?.() : props.onTakeControl?.())}
      >
        {controlling ? 'Hand back' : 'Take control'}
      </button>
      <span className="flex-1" />
      {props.live && props.localHold ? (
        <span className="flex-shrink-0 text-[var(--color-text-muted)]" data-desktop-local-hold="">
          Someone at the screen is using it
        </span>
      ) : props.live && !controlling && props.controller ? (
        <span className="flex-shrink-0 truncate text-[var(--color-text-muted)]" data-desktop-controller="">
          {props.controller} is in control
        </span>
      ) : null}
      <span className="flex-shrink-0 text-[var(--color-text-muted)]" data-desktop-viewers="">
        {props.viewers === null ? '' : `${props.viewers} watching`}
      </span>
      <span className="flex-shrink-0 text-[var(--color-text-muted)]" data-desktop-latency="" title={streamTitle ?? 'Capture to screen (the daemon’s part plus this side’s decode and draw)'}>
        {props.latencyMs === null ? '— ms' : `${Math.round(props.latencyMs)} ms`}
      </span>
    </div>
  )
}
