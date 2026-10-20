// 0.46.3 Thread parity in Gardens (Rosson 2026-10-20): `thread.open` in Zen.
// A Garden has no tab strip, so a message's attachment or a linked
// workspace file opens here instead: a sheet over the Garden with the file
// full size (an image as an <img> from a data: URL, text as plain text,
// anything else by its name), Escape / the backdrop / Close to dismiss, and
// "Open in K2" to leave Zen and open it as a file tab (when the agent's
// room is on this window). Built with DOM nodes and textContent only:
// nothing a file holds is ever parsed as markup (an SVG shows through
// <img>, which runs no script).

export type ZenFilePreviewBody =
  | { kind: 'image'; mime: string; base64: string; fallback?: () => Promise<{ mime: string; base64: string } | null> }
  | { kind: 'text'; text: string; truncated: boolean }
  | { kind: 'none'; reason: string }

export interface ZenFilePreview {
  /** The file's name (title). */
  name: string
  /** `:line` it was linked at, for the title. */
  line?: number
  /** The content, loaded after the sheet shows ("Loading…" until then). */
  load(): Promise<ZenFilePreviewBody>
  /** Leave Zen and open the file as a tab; absent = no button. */
  openInK2?: () => void
}

const SHEET_ID = 'zen-file-preview'
const TEXT_MAX = 512 * 1024

let closeCurrent: (() => void) | null = null

/** Is a preview sheet showing (tests)? */
export function zenFilePreviewShown(): boolean {
  return typeof document !== 'undefined' && document.getElementById(SHEET_ID) !== null
}

/** Close the sheet, if one shows. */
export function closeZenFilePreview(): void {
  closeCurrent?.()
}

function node<K extends keyof HTMLElementTagNameMap>(tag: K, style: Partial<CSSStyleDeclaration>, text?: string): HTMLElementTagNameMap[K] {
  const n = document.createElement(tag)
  Object.assign(n.style, style)
  if (text !== undefined) n.textContent = text
  return n
}

/** An image data: URL for base64 bytes of an image type (never anything else). */
export function zenImageDataUrl(mime: string, base64: string): string | null {
  if (!/^image\/[a-z0-9.+-]+$/i.test(mime) || !/^[A-Za-z0-9+/=\s]*$/.test(base64)) return null
  return `data:${mime};base64,${base64.replace(/\s+/g, '')}`
}

/** Show `p` over the Garden (one at a time: a new one replaces the last). */
export function showZenFilePreview(p: ZenFilePreview): void {
  if (typeof document === 'undefined') return
  closeCurrent?.()
  const backdrop = node('div', {
    position: 'fixed',
    inset: '0',
    zIndex: '400',
    background: 'rgba(0,0,0,0.55)',
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    padding: '32px',
  })
  backdrop.id = SHEET_ID
  backdrop.setAttribute('data-zen-file-preview', '')
  const sheet = node('div', {
    background: 'var(--zen-surface, #1c1c1e)',
    color: 'var(--zen-text, #f2f2f7)',
    borderRadius: '14px',
    boxShadow: '0 20px 60px rgba(0,0,0,0.45)',
    maxWidth: 'min(1100px, 100%)',
    maxHeight: '100%',
    minWidth: '320px',
    display: 'flex',
    flexDirection: 'column',
    overflow: 'hidden',
  })
  sheet.setAttribute('role', 'dialog')
  sheet.setAttribute('aria-modal', 'true')
  const title = p.line ? `${p.name}:${p.line}` : p.name
  sheet.setAttribute('aria-label', title)
  const head = node('div', { display: 'flex', alignItems: 'center', gap: '8px', padding: '10px 12px', borderBottom: '1px solid rgba(127,127,127,0.25)' })
  const name = node('div', { flex: '1', minWidth: '0', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', fontWeight: '600', fontSize: '13px' }, title)
  name.setAttribute('data-zen-preview-name', '')
  head.appendChild(name)
  const button = (label: string, onClick: () => void): HTMLButtonElement => {
    const b = node('button', {
      border: '1px solid rgba(127,127,127,0.35)',
      background: 'transparent',
      color: 'inherit',
      borderRadius: '8px',
      padding: '4px 10px',
      fontSize: '12px',
      cursor: 'pointer',
    }, label)
    b.type = 'button'
    b.addEventListener('click', onClick)
    return b
  }
  if (p.openInK2) {
    const go = p.openInK2
    const b = button('Open in K2', () => {
      close()
      go()
    })
    b.setAttribute('data-zen-preview-open', '')
    head.appendChild(b)
  }
  const closeBtn = button('Close', () => close())
  closeBtn.setAttribute('data-zen-preview-close', '')
  head.appendChild(closeBtn)
  sheet.appendChild(head)
  const body = node('div', { overflow: 'auto', flex: '1', minHeight: '0', display: 'flex', alignItems: 'center', justifyContent: 'center' })
  body.setAttribute('data-zen-preview-body', '')
  body.appendChild(node('div', { padding: '40px', opacity: '0.7', fontSize: '13px' }, 'Loading…'))
  sheet.appendChild(body)
  backdrop.appendChild(sheet)

  const note = (text: string): void => {
    body.replaceChildren(node('div', { padding: '40px', opacity: '0.75', fontSize: '13px', textAlign: 'center' }, text))
  }
  const onKey = (e: KeyboardEvent): void => {
    if (e.key !== 'Escape') return
    e.preventDefault()
    e.stopPropagation()
    close()
  }
  backdrop.addEventListener('mousedown', (e) => {
    if (e.target === backdrop) close()
  })
  let open = true
  function close(): void {
    if (!open) return
    open = false
    document.removeEventListener('keydown', onKey, true)
    backdrop.remove()
    if (closeCurrent === close) closeCurrent = null
  }
  closeCurrent = close
  document.addEventListener('keydown', onKey, true)
  document.body.appendChild(backdrop)
  closeBtn.focus()

  void p
    .load()
    .then((content) => {
      if (!open) return
      if (content.kind === 'none') return note(content.reason)
      if (content.kind === 'text') {
        const pre = node('pre', {
          margin: '0',
          padding: '14px 16px',
          alignSelf: 'stretch',
          whiteSpace: 'pre-wrap',
          wordBreak: 'break-word',
          fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace',
          fontSize: '12px',
          lineHeight: '1.5',
          minWidth: 'min(720px, 80vw)',
        })
        const text = content.text.length > TEXT_MAX ? content.text.slice(0, TEXT_MAX) : content.text
        pre.textContent = content.truncated || content.text.length > TEXT_MAX ? `${text}\n…` : text
        body.style.alignItems = 'flex-start'
        body.replaceChildren(pre)
        return
      }
      const url = zenImageDataUrl(content.mime, content.base64)
      if (!url) return note('This image can’t be shown here.')
      const img = node('img', { maxWidth: '100%', maxHeight: 'calc(100vh - 140px)', objectFit: 'contain', display: 'block' })
      img.alt = p.name
      let triedFallback = false
      img.addEventListener('error', () => {
        // A type this window can't draw (HEIC, TIFF): K2's thumbnail.
        if (triedFallback || !content.fallback) return note('This image can’t be shown here.')
        triedFallback = true
        void content.fallback().then(
          (t) => {
            const next = t ? zenImageDataUrl(t.mime, t.base64) : null
            if (!open) return
            if (next) img.src = next
            else note('This image can’t be shown here.')
          },
          () => note('This image can’t be shown here.'),
        )
      })
      img.src = url
      body.replaceChildren(img)
    })
    .catch((err: unknown) => {
      if (open) note(`Couldn’t open ${p.name}: ${err instanceof Error ? err.message : String(err)}`)
    })
}
