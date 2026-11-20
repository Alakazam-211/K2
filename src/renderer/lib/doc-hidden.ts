// `data-doc-hidden` on <html> while the page is hidden (window minimised,
// on another Space, the lid shut). globals.css pauses the always-on
// spinners (the braille spinner, the working grid) under it, so a hidden
// window spends nothing animating them. Installed once at startup
// (index.tsx).

export const DOC_HIDDEN_ATTR = 'data-doc-hidden'

type Doc = Pick<Document, 'visibilityState' | 'addEventListener' | 'removeEventListener' | 'documentElement'>

/** Mirrors the page's visibility onto <html> now and on every change.
 *  Returns the uninstaller (tests). */
export function installDocHiddenAttr(doc: Doc = document): () => void {
  const sync = (): void => {
    if (doc.visibilityState === 'hidden') doc.documentElement.setAttribute(DOC_HIDDEN_ATTR, '')
    else doc.documentElement.removeAttribute(DOC_HIDDEN_ATTR)
  }
  sync()
  doc.addEventListener('visibilitychange', sync)
  return () => doc.removeEventListener('visibilitychange', sync)
}
