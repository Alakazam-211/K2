// The Desktop tab's toolbar (prd-remote-desktop-tab-v1 §8), without video
// (D7a). Every control the pane will have is here; the ones that need the
// live stream are disabled until it arrives (D7b), with a title saying so.
// The machine's name is always shown (vs-live R22: never a silent switch
// to another machine).
//
//   [machine] [display ▾] [Fit|1:1] [View only] [Capture keys] [Mac↔PC] [Keys ▾]
//   [Send clipboard] [Copy theirs] [Take control] ……… [viewers] [latency]

import type { DesktopFit } from './desktop-serialize'
import { ESCAPE_CHORD_LABEL } from './desktop-keymap'
import type { Display } from './desktop-wire'

export const NEEDS_STREAM_TITLE = 'The live picture comes in a later K2 build.'

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
  /** The stream is open (D7b). False in this build. */
  live: boolean
  viewers: number | null
  latencyMs: number | null
  onDisplay(id: string): void
  onFit(fit: DesktopFit): void
  onViewOnly(on: boolean): void
  onCapture(on: boolean): void
  onFlipModifiers(on: boolean): void
}

const BTN =
  'h-6 flex-shrink-0 cursor-pointer border border-[var(--color-border)] px-2 text-xs text-[var(--color-text-primary)] transition-colors enabled:hover:border-[var(--color-text-muted)] disabled:cursor-default disabled:opacity-50'
const ON = 'border-[var(--color-accent)] text-[var(--color-accent)]'

export function DesktopToolbar(props: DesktopToolbarProps): React.JSX.Element {
  const streamTitle = props.live ? undefined : NEEDS_STREAM_TITLE
  return (
    <div
      className="flex h-10 flex-shrink-0 items-center gap-1.5 overflow-x-auto border-b border-[var(--color-border)] px-2 text-xs"
      data-desktop-toolbar=""
    >
      <span className="flex-shrink-0 truncate font-medium text-[var(--color-text-primary)]" data-desktop-machine="">
        {props.machine}
      </span>
      <select
        className="h-6 flex-shrink-0 border border-[var(--color-border)] bg-[var(--color-bg)] px-1 text-xs text-[var(--color-text-primary)]"
        aria-label="Display"
        data-desktop-display-picker=""
        value={props.displayId ?? ''}
        disabled={props.displays.length === 0}
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
      <button type="button" className={BTN} data-desktop-keys-menu="" disabled={!props.live} title={streamTitle ?? 'Send a chord K2 or your computer would take'}>
        Keys
      </button>
      <button type="button" className={BTN} data-desktop-clip-send="" disabled={!props.live} title={streamTitle ?? 'Send your clipboard text'}>
        Send clipboard
      </button>
      <button type="button" className={BTN} data-desktop-clip-copy="" disabled={!props.live} title={streamTitle ?? "Copy the remote desktop's clipboard text"}>
        Copy theirs
      </button>
      <button type="button" className={BTN} data-desktop-take-control="" disabled={!props.live || !props.mayControl} title={streamTitle ?? 'Take control'}>
        Take control
      </button>
      <span className="flex-1" />
      <span className="flex-shrink-0 text-[var(--color-text-muted)]" data-desktop-viewers="">
        {props.viewers === null ? '' : `${props.viewers} watching`}
      </span>
      <span className="flex-shrink-0 text-[var(--color-text-muted)]" data-desktop-latency="" title={streamTitle}>
        {props.latencyMs === null ? '— ms' : `${Math.round(props.latencyMs)} ms`}
      </span>
    </div>
  )
}
