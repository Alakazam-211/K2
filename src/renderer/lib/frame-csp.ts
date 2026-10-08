// Per-frame Content Security Policy for every K2 surface that renders HTML
// K2 did not author (HTML file tabs, Projects dashboard htmlDoc panes,
// Inbox HTML mail, custom Garden widgets). PRD: .k2/prds/prd-html-frame-csp-v1.md
// (F1–F12) and prd-zen-user-widgets-v2 (UW13, UW46, UWB14).
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
//
// The `widget` profile (custom Garden widgets, agent-written) is stricter on
// scripts and wider on compute (UWB14; S0 spike, §15.6):
//   - `script-src 'nonce-<N>' 'wasm-unsafe-eval'`: only the scripts K2 and
//     the daemon's bundler nonced run (K2's runtime, the stdlib, the widget's
//     own files). Injected `<script>`, inline handlers, `eval` and
//     `new Function` don't. wasm compiles;
//   - `connect-src data: blob:` and `worker-src blob:`: `fetch('data:')`,
//     blob workers and wasm in them (three.js, PixiJS, physics); no network;
//   - everything else as the base: no frames, objects, forms or base.
// The nonce is fresh per bundle response (UW9), so a page can't guess it.

/**
 * `scripted` — HTML file tabs and dashboard htmlDoc panes. Inline JS runs
 *   (interactive dashboards, `.k2/wiki/roadmap-board.html`).
 * `inert` — Inbox HTML mail. No JS at all.
 * `widget` — a custom Garden widget: nonced scripts only, wasm and blob
 *   workers, no network (prd-zen-user-widgets-v2 UW13, UWB14).
 */
export type FrameProfile = 'scripted' | 'inert' | 'widget'

/** What a profile needs besides its name: the `widget` profile's nonce. */
export interface FrameCspOptions {
  nonce?: string
}

/** Directives shared by every profile, in emit order. */
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

/** UWB14: what the `widget` profile widens (never network). */
const WIDGET_OVERRIDES: Readonly<Record<string, string>> = {
  'connect-src': 'data: blob:',
  'worker-src': 'blob:',
}

/** A nonce is CSP source text: base64 / base64url / hex only. */
const NONCE_RE = /^[A-Za-z0-9+/_-]{16,128}={0,2}$/

function scriptSrc(profile: FrameProfile, opts: FrameCspOptions | undefined): string {
  if (profile === 'scripted') return "'unsafe-inline'"
  if (profile === 'inert') return "'none'"
  const nonce = opts?.nonce
  if (!nonce || !NONCE_RE.test(nonce)) throw new Error('frame-csp: the widget profile needs a well-formed nonce')
  return `'nonce-${nonce}' 'wasm-unsafe-eval'`
}

/**
 * `sandbox` tokens per profile. Never `allow-same-origin` (F5): with
 * `allow-scripts` that would let the frame reach `window.parent`, the owner
 * token in the renderer, and remove its own sandbox. A widget gets exactly
 * `allow-scripts` (UW13): no popups, modals, forms or top navigation.
 */
const SANDBOX: Record<FrameProfile, string> = {
  scripted: 'allow-scripts',
  inert: '',
  widget: 'allow-scripts',
}

/** The CSP string for a profile. Pure; same input, same output. */
export function frameCsp(profile: FrameProfile, opts?: FrameCspOptions): string {
  const parts: string[] = []
  for (const [name, value] of BASE_DIRECTIVES) {
    const v = profile === 'widget' ? (WIDGET_OVERRIDES[name] ?? value) : value
    parts.push(`${name} ${v}`)
    if (name === 'default-src') parts.push(`script-src ${scriptSrc(profile, opts)}`)
  }
  return parts.join('; ')
}

/** The iframe `sandbox` attribute value for a profile. */
export function frameSandbox(profile: FrameProfile): string {
  return SANDBOX[profile]
}

/** The `<meta>` tag the helper injects. Exported for tests. */
export function frameCspMeta(profile: FrameProfile, opts?: FrameCspOptions): string {
  return `<meta http-equiv="Content-Security-Policy" content="${frameCsp(profile, opts)}">`
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
 *
 * `prelude` (the `widget` profile: K2's theme style, runtime and library
 * scripts, each nonced by the caller) goes right after the meta and before
 * any of `html`, so K2's runtime exists before the widget's first script.
 */
export function frameSrcDoc(html: string, profile: FrameProfile, opts?: FrameCspOptions & { prelude?: string }): string {
  const head = frameCspMeta(profile, opts) + (opts?.prelude ?? '')
  const m = LEADING_DOCTYPE.exec(html)
  if (m) return m[0] + head + html.slice(m[0].length)
  return head + html
}
