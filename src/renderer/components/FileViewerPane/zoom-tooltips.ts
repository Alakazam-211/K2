// CodeMirror's tooltips (the completion list, hovers) under the app zoom.
//
// By default CodeMirror places a tooltip with `position: fixed` from window-px
// coordinates. Under the app zoom (CSS `zoom` on <html>, lib/app-zoom) those
// read as CSS px, so Chromium draws the list zoom times too far right and
// down. WebKit notices and turns its tooltips absolute by itself. Absolute
// tooltips sit inside the editor and CodeMirror scales them, so they land
// under the caret at any zoom. At 100 % they stay fixed, where no scroller or
// pane edge can clip them.

import { Compartment, type Extension } from '@codemirror/state'
import { ViewPlugin, tooltips, type EditorView } from '@codemirror/view'
import { appZoom } from '@/lib/app-zoom'

/** Tooltip placement for zoom `z`: fixed at 100 %, absolute otherwise. */
export function zoomTooltipPosition(z: number): 'fixed' | 'absolute' {
  return z === 1 ? 'fixed' : 'absolute'
}

/** The editor's tooltip config, kept in step with the app zoom while the
 *  editor is open (the zoom lands on <html>'s style and class). */
export function zoomAwareTooltips(): Extension {
  const slot = new Compartment()
  const config = (): Extension => tooltips({ position: zoomTooltipPosition(appZoom()) })
  const follow = ViewPlugin.fromClass(
    class {
      private position = zoomTooltipPosition(appZoom())
      private readonly observer: MutationObserver | null
      constructor(view: EditorView) {
        this.observer =
          typeof MutationObserver === 'function'
            ? new MutationObserver(() => {
                const next = zoomTooltipPosition(appZoom())
                if (next === this.position) return
                this.position = next
                view.dispatch({ effects: slot.reconfigure(config()) })
              })
            : null
        this.observer?.observe(document.documentElement, { attributes: true, attributeFilter: ['style', 'class'] })
      }
      destroy(): void {
        this.observer?.disconnect()
      }
    },
  )
  return [slot.of(config()), follow]
}
