// Per-frame Content Security Policy for every K2 surface that renders HTML
// K2 did not author (HTML file tabs, Projects dashboard htmlDoc panes,
// Inbox HTML mail). PRD: .k2/prds/prd-html-frame-csp-v1.md (F1–F12).
//
// Why: an `<iframe srcdoc>` inherits the app CSP (`src-tauri/tauri.conf.json`),
// whose connect-src allows `http://127.0.0.1:*`, `http://*:*` and
// `ipc://localhost` because the renderer itself needs them. So a script in a
// user's HTML file could fetch the local daemon, scan loopback/LAN services,
// and post to any plain-http host. CSP policies stack: the `<meta>` we put at
// the top of the srcdoc is enforced IN ADDITION to the inherited app policy,
// so it can only narrow, never widen (F2).
//
// The policy keeps exactly what these frames could do in a shipped build
// (app CSP ∩ sandbox): inline scripts where scripts run today, inline styles,
// data:/blob: images, media and fonts. Nothing else (F3/F4).

/**
 * `scripted` — HTML file tabs and dashboard htmlDoc panes. Inline JS runs
 *   (interactive dashboards, `.k2/wiki/roadmap-board.html`).
 * `inert` — Inbox HTML mail. No JS at all.
 */
export type FrameProfile = 'scripted' | 'inert'

/** Directives shared by both profiles, in emit order. */
const BASE_DIRECTIVES: ReadonlyArray<readonly [string, string]> = [
  ['default-src', "'none'"],
  ['style-src', "'unsafe-inline'"],
  ['img-src', 'data: blob:'],
  ['media-src', 'data: blob:'],
  ['font-src', 'data:'],
  ['connect-src', "'none'"],
  ['frame-src', "'none'"],
  ['child-src', "'none'"],
  ['worker-src', "'none'"],
  ['object-src', "'none'"],
  ['manifest-src', "'none'"],
  ['form-action', "'none'"],
  ['base-uri', "'none'"],
]

const SCRIPT_SRC: Record<FrameProfile, string> = {
  scripted: "'unsafe-inline'",
  inert: "'none'",
}

/**
 * `sandbox` tokens per profile. Never `allow-same-origin` (F5): with
 * `allow-scripts` that would let the frame reach `window.parent`, the owner
 * token in the renderer, and remove its own sandbox.
 */
const SANDBOX: Record<FrameProfile, string> = {
  scripted: 'allow-scripts',
  inert: '',
}

/** The CSP string for a profile. Pure; same input, same output. */
export function frameCsp(profile: FrameProfile): string {
  const parts: string[] = []
  for (const [name, value] of BASE_DIRECTIVES) {
    parts.push(`${name} ${value}`)
    if (name === 'default-src') parts.push(`script-src ${SCRIPT_SRC[profile]}`)
  }
  return parts.join('; ')
}

/** The iframe `sandbox` attribute value for a profile. */
export function frameSandbox(profile: FrameProfile): string {
  return SANDBOX[profile]
}

/** The `<meta>` tag the helper injects. Exported for tests. */
export function frameCspMeta(profile: FrameProfile): string {
  return `<meta http-equiv="Content-Security-Policy" content="${frameCsp(profile)}">`
}

// A leading doctype, and nothing else. The HTML tokenizer ends a DOCTYPE token
// at the first `>` in every doctype state, so `[^>]*>` matches exactly what the
// browser consumes. We deliberately do NOT skip leading comments: `<!-->` and
// `--!>` end a comment early, and a regex that guessed wrong would place the
// meta after attacker markup. A doc that starts with a comment gets the meta
// prepended (safe; at worst quirks mode for that doc).
const LEADING_DOCTYPE = /^﻿?[\t\n\f\r ]*<!doctype\b[^>]*>/i

/**
 * Wrap untrusted HTML for `<iframe srcdoc>`: the CSP meta becomes the first
 * element the parser sees, before any script, style or resource in `html`.
 * Placed after a leading `<!doctype …>` so standards-mode docs stay in
 * standards mode (a meta before the doctype would force quirks).
 */
export function frameSrcDoc(html: string, profile: FrameProfile): string {
  const meta = frameCspMeta(profile)
  const m = LEADING_DOCTYPE.exec(html)
  if (m) return m[0] + meta + html.slice(m[0].length)
  return meta + html
}
