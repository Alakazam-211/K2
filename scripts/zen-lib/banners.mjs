// Licence banners for the Zen widget library files the daemon serves (the
// Kessel Steward's ruling: every served file starts with one). Shared by
// scripts/zen-lib/vendor.ts, which writes them, and
// scripts/zen-lib/check-banners.mjs, which checks them, so the two always
// agree on what a banner is.
//
// A banner is a legal comment in the file's first lines: one of the
// comments the file starts with (nothing but whitespace and other comments
// before it), starting in its first BANNER_WINDOW bytes, that is a `/*!` or
// `//!` comment, or a block or line comment with `@license` or `@preserve`
// (what minifiers keep). That one comment must name the library's id from
// src/shared/zen-lib.json as a whole word (any case), every SPDX id of its
// licence literally, and a copyright ("copyright", "(c)" or "©", any case).
// A bare `/*!`, one with a URL and no SPDX id, or K2's old "pre-bundled for
// K2" header (no copyright) is not a banner. This is the partner build's
// licence-gate rule, made stricter: there the comment may be anywhere in
// the file, any allowlisted id counts and no copyright is needed.
//
// scripts/zen-lib/banner-cases.json holds the cases every port of this rule
// must pass (check-banners.mjs --self-test runs them; the partner build's
// licence-allow.py reads the same file).

/** A banner starts within this many bytes (UTF-8) of the file's start. */
export const BANNER_WINDOW = 4096

/** The comments a text starts with: [{ block, bang, body, at }], `at` the
 *  comment's start in UTF-8 bytes. Stops at the first character that is
 *  neither whitespace nor a comment. */
export function leadingComments(text) {
  const out = []
  let i = text.charCodeAt(0) === 0xfeff ? 1 : 0
  const at = (j) => Buffer.byteLength(text.slice(0, j), 'utf8')
  for (;;) {
    while (i < text.length && /\s/.test(text[i])) i++
    if (text.startsWith('/*', i)) {
      const end = text.indexOf('*/', i + 2)
      if (end < 0) return out
      out.push({ block: true, bang: text[i + 2] === '!', body: text.slice(i + 2, end), at: at(i) })
      i = end + 2
    } else if (text.startsWith('//', i)) {
      const end = text.indexOf('\n', i)
      out.push({ block: false, bang: text[i + 2] === '!', body: text.slice(i + 2, end < 0 ? text.length : end), at: at(i) })
      if (end < 0) return out
      i = end + 1
    } else {
      return out
    }
  }
}

function escapeRe(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

/** The SPDX ids an expression names (`MIT AND Apache-2.0` -> MIT, Apache-2.0). */
export function licenceIds(expr) {
  return expr
    .replace(/[()]/g, ' ')
    .split(/\s+/)
    .filter((t) => t && t !== 'AND' && t !== 'OR' && t !== 'WITH')
}

/** Whether `body` names `id` as a whole word (any case). */
export function namesPackage(body, id) {
  return new RegExp(`(?<![A-Za-z0-9_-])${escapeRe(id)}(?![A-Za-z0-9_-])`, 'i').test(body)
}

/** Whether `body` names the SPDX id `spdx` literally, as a whole word. */
export function namesLicence(body, spdx) {
  return new RegExp(`(?<![A-Za-z0-9.+-])${escapeRe(spdx)}(?![A-Za-z0-9+-])`).test(body)
}

/** Whether `body` names a copyright: "copyright", "(c)" or "©", any case. */
export function namesCopyright(body) {
  return /\bcopyright\b|\(c\)|©/i.test(body)
}

/** A legal comment (what minifiers keep: `/*!`, `//!`, or `@license` or
 *  `@preserve` in a block or line comment) inside the window. */
function legalComments(text) {
  return leadingComments(text).filter(
    (c) => c.at < BANNER_WINDOW && (c.bang || c.body.includes('@license') || c.body.includes('@preserve')),
  )
}

/** The banner `text` starts with that names library `id`, every id of its
 *  licence `expr` and a copyright, as one line, or null. */
export function bannerOf(text, id, expr) {
  const ids = licenceIds(expr)
  if (!ids.length) return null
  const c = legalComments(text).find(
    (c) => namesPackage(c.body, id) && ids.every((spdx) => namesLicence(c.body, spdx)) && namesCopyright(c.body),
  )
  if (!c) return null
  return c.block ? `/*${c.body.replace(/\s*\n\s*\*?\s*/g, ' ').trimEnd()} */` : `//${c.body.trimEnd()}`
}

/** Whether `text` starts with a banner naming library `id` and every id of
 *  its licence `expr`. */
export function hasBanner(text, id, expr) {
  return bannerOf(text, id, expr) !== null
}

/** Why `text` has no banner, in a few words (for the check's report). */
export function bannerProblem(text, id, expr) {
  if (hasBanner(text, id, expr)) return null
  const legal = legalComments(text)
  if (!legal.length) return `no /*!, //!, @license or @preserve comment among the comments in its first ${BANNER_WINDOW} bytes`
  const named = legal.filter((c) => namesPackage(c.body, id))
  if (!named.length) return `its legal comment does not name ${id}`
  const missing = licenceIds(expr).filter((spdx) => !named.some((c) => namesLicence(c.body, spdx)))
  if (missing.length) return `its legal comment does not name ${missing.join(', ')} (the SPDX id, literally)`
  return 'its legal comment names no copyright'
}

/** Copyright lines of a licence text: a line that starts (after any
 *  comment `*` or `#`) with "Copyright" then "(c)", "©" or a digit, or
 *  with "(c)" or "©" then a digit; and, in an upstream banner line split
 *  by " | " (DOMPurify's), a part that starts with "(c)" or "©". The
 *  Apache template line ("Copyright [yyyy] ...") and prose ("copyright
 *  notice", "Copyright Holder", "(c) You must retain") are not copyright
 *  lines. Each is kept as written, trimmed, in order, once. */
export function copyrightLines(licenceText) {
  const out = []
  const keep = (s) => {
    if (!out.includes(s)) out.push(s)
  }
  for (const raw of licenceText.split(/\r?\n/)) {
    const line = raw.replace(/^\s*(?:[*#]\s*)?/, '').trim()
    if (/^Copyright\s*(?:\(c\)|\(C\)|©|\d)/.test(line) || /^(?:\(c\)|©)\s*\d/.test(line)) keep(line)
    else if (line.includes(' | ')) {
      for (const part of line.split(' | ').map((p) => p.trim())) if (/^(?:\(c\)|©)\s+\S/.test(part)) keep(part)
    }
  }
  return out
}

/** The copyright part of a banner: the licence's copyright lines joined
 *  with "; ", or "Copyright (c) the <project> contributors" when it has
 *  none. */
export function copyrightPart(licenceText, project) {
  const lines = copyrightLines(licenceText)
  return lines.length ? lines.join('; ') : `Copyright (c) the ${project} contributors`
}

/** The banner of a file that carries an upstream library (a copied build
 *  with no banner of its own, or K2's esbuild bundle of it). */
export function libraryBanner({ id, version, license, copyright }) {
  return checkedComment(`/*! ${id} ${version} | ${license} | ${copyright} | full text: Third-party notices */`)
}

/** The banner of K2's glue or style file next to an upstream library: it
 *  names the upstream it carries; K2's part is MIT. */
export function glueBanner({ file, id, version, title, license, copyright }) {
  const stem = file.replace(/\.js$/, '')
  const name = (title ?? '').replace(/\s*\(.*\)$/, '')
  const named = name && name.toLowerCase() !== id.toLowerCase() ? `${id} ${version}, ${name}` : `${id} ${version}`
  return checkedComment(`/*! ${stem}: ${named} (${license}, ${copyright}); K2 additions MIT | full text: Third-party notices */`)
}

function checkedComment(c) {
  if (c.slice(2, -2).includes('*/') || c.includes('\n')) throw new Error(`a banner can't hold "*/" or a newline: ${c}`)
  return c
}
