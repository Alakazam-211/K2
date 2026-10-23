// Sanitize mammoth's DOCX → HTML output before it reaches the K2 webview.
//
// mammoth does not sanitize: a hyperlink in a .docx keeps whatever target
// the author typed, `javascript:` included. The viewer renders into the main
// webview (CSP allows inline script there), so the HTML goes through
// DOMPurify with a strict allowlist first:
// - links: http, https, mailto, or an in-document `#fragment` only;
// - images: inline `data:image/...` only (mammoth embeds them that way);
// - no event handlers, no style, no script/iframe/object/embed/form tags.

import DOMPurify from 'dompurify'

// Tags mammoth emits, plus a few harmless structural ones.
const ALLOWED_TAGS = [
  'a', 'b', 'blockquote', 'br', 'caption', 'code', 'col', 'colgroup', 'del',
  'div', 'em', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'hr', 'i', 'img', 'ins',
  'li', 'ol', 'p', 'pre', 's', 'span', 'strong', 'sub', 'sup', 'table',
  'tbody', 'td', 'tfoot', 'th', 'thead', 'tr', 'u', 'ul',
]

const ALLOWED_ATTR = ['href', 'id', 'src', 'alt', 'title', 'colspan', 'rowspan']

const FORBID_TAGS = ['script', 'iframe', 'object', 'embed', 'style', 'form', 'svg', 'math', 'base', 'link', 'meta']

// http, https, mailto, or a same-document fragment. Anything else is dropped.
const SAFE_HREF = /^(?:https?:|mailto:|#)/i
const SAFE_IMG_SRC = /^data:image\//i

let purifier: ReturnType<typeof DOMPurify> | null = null

function getPurifier(): ReturnType<typeof DOMPurify> {
  if (purifier) return purifier
  const p = DOMPurify(window)
  p.addHook('uponSanitizeAttribute', (node, data) => {
    const name = data.attrName
    const value = data.attrValue.trim()
    if (name === 'href' && !SAFE_HREF.test(value)) {
      data.keepAttr = false
    } else if (name === 'src' && !(node.nodeName === 'IMG' && SAFE_IMG_SRC.test(value))) {
      data.keepAttr = false
    }
  })
  purifier = p
  return p
}

/** Strip everything in mammoth's HTML that could run code or load remote content. */
export function sanitizeDocxHtml(html: string): string {
  return getPurifier().sanitize(html, {
    ALLOWED_TAGS,
    ALLOWED_ATTR,
    FORBID_TAGS,
    ALLOW_DATA_ATTR: false,
    ALLOW_ARIA_ATTR: false,
    ALLOWED_URI_REGEXP: /^(?:https?:|mailto:|#|data:image\/)/i,
    RETURN_TRUSTED_TYPE: false,
  }) as string
}
