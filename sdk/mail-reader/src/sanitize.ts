// Mail HTML, cleaned a second time in the client (the daemon cleans it first
// with ammonia), then handed to the host's frame. Adapted from OCmail's
// lib/email-sanitization.ts (MIT, Copyright (c) 2025 Matthieu MALVACHE,
// Copyright (c) 2026 Alakazam Labs; see ../NOTICE). DOMPurify is used under
// its Apache-2.0 option.
//
// K2's additions over OCmail's config: nothing remote survives. `src` keeps
// only inline `data:image/…` (or `blob:`), `srcset`/`background`/`poster` go,
// `url(…)` in a style becomes `none`, and links lose `href` (the target moves
// to `title`, so hovering shows where it goes; the frame can't navigate).

import DOMPurify from 'dompurify'

/** OCmail's unified email config (script vectors out, formatting kept). */
export const EMAIL_SANITIZE_CONFIG = {
  ADD_ATTR: ['style', 'class', 'width', 'height', 'align', 'valign', 'bgcolor', 'color', 'title'],
  ALLOW_DATA_ATTR: false,
  FORCE_BODY: true,
  FORBID_TAGS: [
    'script', 'iframe', 'object', 'embed', 'form',
    'input', 'button', 'meta', 'link', 'base',
    'svg', 'math', 'style',
    // K2: nothing that loads media from elsewhere.
    'video', 'audio', 'source', 'track', 'picture', 'frame', 'frameset', 'portal',
  ],
  FORBID_ATTR: [
    'onerror', 'onload', 'onclick', 'onmouseover',
    'onfocus', 'onblur', 'onchange', 'onsubmit',
    'onkeydown', 'onkeyup', 'onmousedown', 'onmouseup',
    // K2: remote fetches and navigation.
    'srcset', 'background', 'poster', 'action', 'formaction', 'ping', 'target',
  ],
}

const SAFE_SRC = /^(?:data:image\/|blob:)/i
const CSS_URL = /url\s*\(\s*(['"]?)(.*?)\1\s*\)/gi
const CSS_BAD = /expression\s*\(|@import|javascript:|behavior\s*:|-moz-binding/i

let purifier: ReturnType<typeof DOMPurify> | null = null

function mailPurifier(): ReturnType<typeof DOMPurify> {
  if (purifier) return purifier
  if (typeof window === 'undefined') throw new Error('@k2/mail-reader: sanitizing mail needs a DOM window')
  const p = DOMPurify(window)
  p.addHook('uponSanitizeAttribute', (node, data) => {
    const name = data.attrName
    const value = String(data.attrValue ?? '').trim()
    if (name === 'src' && !SAFE_SRC.test(value)) {
      data.keepAttr = false
      return
    }
    if (name === 'style') {
      if (CSS_BAD.test(value)) {
        data.keepAttr = false
        return
      }
      data.attrValue = value.replace(CSS_URL, (_m, _q, url: string) => (SAFE_SRC.test(url.trim()) ? `url("${url.trim()}")` : 'none'))
      return
    }
    if (name === 'href') {
      // The frame can't follow a link; show the target on hover instead.
      const el = node as Element
      if (value && typeof el.setAttribute === 'function' && !el.getAttribute('title')) el.setAttribute('title', value)
      data.keepAttr = false
    }
  })
  purifier = p
  return p
}

/** Clean mail HTML for an inert frame (no script, nothing remote, no links). */
export function sanitizeEmailHtml(html: string): string {
  return mailPurifier().sanitize(html, { ...EMAIL_SANITIZE_CONFIG, RETURN_TRUSTED_TYPE: false }) as string
}

/** True when cleaned HTML still names a remote resource in a src (a bug). */
export function hasRemoteSrc(html: string): boolean {
  return /\s(?:src|srcset|background|poster)\s*=\s*["']?\s*(?:https?:)?\/\//i.test(html) || /url\(\s*["']?\s*(?:https?:)?\/\//i.test(html)
}

/**
 * The document the host's frame shows: the cleaned mail on a light card.
 * Mail is designed for a light page, so the frame keeps the system's light
 * colours (`Canvas` / `CanvasText`) whatever K2's theme is.
 */
export function mailFrameDocument(cleanHtml: string): string {
  return (
    '<!doctype html><html><head><meta charset="utf-8"><meta name="color-scheme" content="light">' +
    '<style>html,body{margin:0;background:Canvas;color:CanvasText}' +
    'body{padding:20px 24px;font:14px/1.5 system-ui,sans-serif;overflow-wrap:anywhere}' +
    'img{max-width:100%;height:auto}table{max-width:100%}a[title]{text-decoration:underline;cursor:help}</style>' +
    `</head><body>${cleanHtml}</body></html>`
  )
}

/** OCmail's escape: all five HTML-significant characters, `&` first. */
export function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;')
}
