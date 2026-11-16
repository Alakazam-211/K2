// Write mode's small widgets (build-editors.md E2): the list bullet, the
// task checkbox, a code fence's header (language and Copy) and the image
// figure. Each one hangs in the line's left margin or sits in a line of its
// own, so the text beside it never moves when the caret reveals the marks.
// Styles are in components/Editor/writeModeCss.ts, all on tokens.

import { EditorView, ViewPlugin, WidgetType } from '@codemirror/view'
import { syntaxTree } from '@codemirror/language'
import { isExternal, resolveLinkPath, writeLinks } from './links'
import { toggleTask } from './tasks'

const el = <K extends keyof HTMLElementTagNameMap>(tag: K, cls?: string): HTMLElementTagNameMap[K] => {
  const e = document.createElement(tag)
  if (cls) e.className = cls
  return e
}

/** A bullet: a dot, a ring one level down, then a dot again (squares in Terminal, by CSS). */
export class BulletWidget extends WidgetType {
  constructor(readonly level: number) {
    super()
  }
  eq(o: BulletWidget): boolean {
    return o.level === this.level
  }
  toDOM(): HTMLElement {
    const b = el('span', 'cm-md-bullet')
    b.dataset.shape = this.level % 2 ? 'ring' : 'dot'
    b.setAttribute('aria-hidden', 'true')
    return b
  }
}

/** K2's checkbox for a task. Click or Space toggles the box in the text. */
export class CheckboxWidget extends WidgetType {
  constructor(readonly checked: boolean, readonly readOnly: boolean) {
    super()
  }
  eq(o: CheckboxWidget): boolean {
    return o.checked === this.checked && o.readOnly === this.readOnly
  }
  toDOM(view: EditorView): HTMLElement {
    const wrap = el('span', 'cm-md-check')
    const box = el('input', 'k2-checkbox')
    box.type = 'checkbox'
    box.dataset.k2Chamfer = 'chip'
    box.checked = this.checked
    const label = (): void => box.setAttribute('aria-label', box.checked ? 'Done task' : 'Task')
    label()
    if (this.readOnly) box.setAttribute('aria-disabled', 'true')
    const toggle = (): boolean => toggleTask(view, view.posAtDOM(wrap))
    // Keep the caret where it is: a click on the box isn't a click in the text.
    box.addEventListener('mousedown', (e) => e.preventDefault())
    box.addEventListener('click', (e) => {
      if (!toggle()) e.preventDefault()
      label()
    })
    box.addEventListener('keydown', (e) => {
      if (e.key !== ' ') return
      e.preventDefault()
      toggle()
    })
    box.addEventListener('keyup', (e) => {
      if (e.key === ' ') e.preventDefault()
    })
    wrap.appendChild(box)
    return wrap
  }
  // Same box, new state: keep the element so keyboard focus stays on it.
  updateDOM(dom: HTMLElement, _view: EditorView, from: CheckboxWidget): boolean {
    const box = dom.querySelector('input')
    if (!box || from.readOnly !== this.readOnly) return false
    box.checked = this.checked
    box.setAttribute('aria-label', this.checked ? 'Done task' : 'Task')
    return true
  }
  ignoreEvent(): boolean {
    return true
  }
}

const COPY_SVG = '<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" aria-hidden="true"><rect x="5.5" y="5.5" width="8" height="8" rx="1.2"/><path d="M10.5 5.5V3.7c0-.66-.54-1.2-1.2-1.2H3.7c-.66 0-1.2.54-1.2 1.2v5.6c0 .66.54 1.2 1.2 1.2h1.8"/></svg>'

/** The text of the fence whose opening line holds `pos`. */
export function fenceText(view: EditorView, pos: number): string {
  let n = syntaxTree(view.state).resolveInner(pos, 1)
  while (n.name !== 'FencedCode' && n.parent) n = n.parent
  if (n.name !== 'FencedCode') return ''
  const first = view.state.doc.lineAt(n.from)
  const last = n.lastChild?.name === 'CodeMark' && n.lastChild.from > first.to ? view.state.doc.lineAt(n.lastChild.from).from - 1 : n.to
  return last > first.to ? view.state.sliceDoc(first.to + 1, last) : ''
}

/** A fence's opening line, rendered: the language, and Copy. */
export class FenceHeaderWidget extends WidgetType {
  constructor(readonly label: string) {
    super()
  }
  eq(o: FenceHeaderWidget): boolean {
    return o.label === this.label
  }
  toDOM(view: EditorView): HTMLElement {
    const head = el('span', 'cm-md-fence-head')
    const name = el('span', 'cm-md-fence-lang')
    name.textContent = this.label
    const copy = el('button', 'cm-md-copy')
    copy.type = 'button'
    copy.innerHTML = COPY_SVG
    copy.setAttribute('aria-label', 'Copy code')
    copy.title = 'Copy code'
    copy.addEventListener('mousedown', (e) => e.preventDefault())
    copy.addEventListener('click', () => {
      void navigator.clipboard?.writeText(fenceText(view, view.posAtDOM(head))).then(() => {
        copy.dataset.copied = ''
        copy.title = 'Copied'
        setTimeout(() => {
          delete copy.dataset.copied
          copy.title = 'Copy code'
        }, 1500)
      })
    })
    head.append(name, copy)
    return head
  }
  ignoreEvent(e: Event): boolean {
    return (e.target as Element | null)?.closest?.('.cm-md-copy') != null
  }
}

/** Object URLs for the images this view shows, revoked when it closes. */
export const imageCache = ViewPlugin.fromClass(
  class {
    urls = new Map<string, Promise<string>>()
    destroy(): void {
      for (const p of this.urls.values()) void p.then((u) => u.startsWith('blob:') && URL.revokeObjectURL(u)).catch(() => {})
    }
  },
)

/** An image, resolved against the file's folder, shown above its line (the line is its caption). */
export class ImageWidget extends WidgetType {
  constructor(readonly src: string, readonly alt: string) {
    super()
  }
  eq(o: ImageWidget): boolean {
    return o.src === this.src && o.alt === this.alt
  }
  get estimatedHeight(): number {
    return 240
  }
  toDOM(view: EditorView): HTMLElement {
    const fig = el('div', 'cm-md-figure')
    const img = el('img')
    img.alt = this.alt
    img.draggable = false
    const missing = (): void => {
      fig.dataset.missing = ''
      img.remove()
      fig.textContent = `Can't show ${this.src}`
      view.requestMeasure()
    }
    img.addEventListener('load', () => view.requestMeasure())
    img.addEventListener('error', missing)
    fig.appendChild(img)
    const links = view.state.facet(writeLinks)
    if (isExternal(this.src) || /^data:image\//i.test(this.src)) {
      img.src = this.src
    } else {
      const path = resolveLinkPath(links.filePath, this.src)
      const cache = view.plugin(imageCache)?.urls
      if (!path || !links.loadImage) {
        missing()
      } else {
        let url = cache?.get(path)
        if (!url) {
          url = links.loadImage(path)
          cache?.set(path, url)
        }
        fig.dataset.path = path
        url.then((u) => (img.src = u)).catch(missing)
      }
    }
    return fig
  }
}
