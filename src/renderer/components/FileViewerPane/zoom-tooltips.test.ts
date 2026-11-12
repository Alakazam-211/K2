// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest'
import { EditorState } from '@codemirror/state'
import { EditorView, showTooltip } from '@codemirror/view'
import { applyAppZoomForTest, resetAppZoomForTest } from '@/test-utils/app-zoom'
import { zoomAwareTooltips, zoomTooltipPosition } from './zoom-tooltips'

let view: EditorView | null = null
afterEach(() => {
  view?.destroy()
  view = null
  document.body.replaceChildren()
  resetAppZoomForTest()
})

function editor(): EditorView {
  const parent = document.createElement('div')
  document.body.appendChild(parent)
  const tip = showTooltip.of({ pos: 0, create: () => ({ dom: document.createElement('div') }) })
  view = new EditorView({ state: EditorState.create({ doc: 'abc', extensions: [zoomAwareTooltips(), tip] }), parent })
  return view
}
const position = (v: EditorView): string => (v.dom.ownerDocument.querySelector('.cm-tooltip') as HTMLElement).style.position

describe('CodeMirror tooltips under the app zoom', () => {
  it('are fixed at 100 % and absolute (placed and scaled by CodeMirror) at any other zoom', () => {
    expect(zoomTooltipPosition(1)).toBe('fixed')
    for (const z of [0.9, 1.1, 1.25, 1.5, 1.75]) expect(zoomTooltipPosition(z)).toBe('absolute')
  })

  it('an editor opened at 150 % places its tooltips absolutely', () => {
    applyAppZoomForTest(1.5)
    expect(position(editor())).toBe('absolute')
  })

  it('follow a zoom change while the editor is open, and back', async () => {
    const v = editor()
    expect(position(v)).toBe('fixed')
    applyAppZoomForTest(1.25)
    await new Promise((r) => setTimeout(r, 0))
    expect(position(v)).toBe('absolute')
    applyAppZoomForTest(1)
    await new Promise((r) => setTimeout(r, 0))
    expect(position(v)).toBe('fixed')
  })
})
