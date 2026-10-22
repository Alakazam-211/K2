// prd-zen-user-widgets-v2 UW13, UW39, UW46, UWA5, UWB13 — what K2 puts at
// the top of a sealed widget frame, before any of the widget's own HTML:
//
//   <meta x-dns-prefetch-control off> <meta CSP>   (frame-csp, `widget`)
//   <script nonce> the sealed-frame guard: no WebRTC, link resource hints
//                  or <a ping> (src/shared/sealed-frame-guard.js). Best
//                  effort for the web client; the desktop app also runs it
//                  as an all-frames init script.
//   <style>  --zen-* variables, transparent background   (UW39)
//   <script nonce> k2-frame.js: the verb table + K2's runtime   (UWA5)
//   <script nonce> each `requires.libs` library, in order       (UWB13)
//   …the daemon's bundle (its own scripts already carry the nonce)
//
// The runtime and libraries never count toward the widget's 256 KB
// (UW8). Every script text has `</script` escaped, so no text can close
// its element early.
//
// k2-frame.js is generated (`contract-gen`, UWA11): the catalog's widget
// verb table as `K2_CONTRACT`, then `sdk/k2-runtime.js`, in one classic
// script. Never edited by hand; the freshness tests check it.

import k2Frame from '../../../../sdk/generated/k2-frame.js?raw'
import { SEALED_FRAME_GUARD } from '../sealed-frame-guard'

/** TUWA6: the runtime text the frame gets is at most this big (16 KB
 *  until Garden data's verbs and SDK helpers, contract-garden-data-v1 §7;
 *  32 KB with 0.46.1's SDK and its garden-local App rows; 40 KB with
 *  catalog v20's Garden columns and agent list rows, 0.46.3 r10; the Rust
 *  cap is `FRAME_MAX_BYTES`). */
export const K2_FRAME_MAX_BYTES = 40 * 1024

/** k2-frame.js: the table, then the runtime. */
export function k2FrameScript(): string {
  return k2Frame
}

/** Script text that can sit inside `<script>…</script>` as-is: `</script`
 *  becomes `<\/script`, which means the same in a string, a template or a
 *  regex. (`<!--` is left alone: rewriting it would change regexes; the
 *  stdlib's per-library smoke test, UWB17, covers what ships.) */
export function zenScriptText(text: string): string {
  return text.replace(/<\/(script)/gi, '<\\/$1')
}

function nonced(nonce: string, text: string): string {
  return `<script nonce="${nonce}">${zenScriptText(text)}</script>`
}

/** A theme value that can sit inside a style rule (no breaking out). */
function safeCssValue(v: string): string | null {
  return /[<>{};\\]|\/\*/.test(v) ? null : v
}

/** The `--zen-*` variables and scheme the frame starts with (UW39). */
export interface ZenFrameTheme {
  vars: Record<string, string>
  scheme: 'light' | 'dark'
}

export function zenFrameThemeCss(theme: ZenFrameTheme, ownScheme = false): string {
  const lines: string[] = []
  for (const [k, v] of Object.entries(theme.vars)) {
    if (!/^--zen-[a-zA-Z0-9-]+$/.test(k)) continue
    const val = safeCssValue(String(v))
    if (val !== null) lines.push(`${k}:${val};`)
  }
  // The Garden's scheme wins over the widget's own (0.46.3): a widget
  // stylesheet's `color-scheme: light dark` (K2's card kits, @1 and @2) let
  // the OS pick, so a dark Garden on a Mac in Light mode painted its cards
  // white. `!important` beats any widget rule without changing a widget
  // file (hashes, versions and own copies stay as they are); the runtime
  // keeps it in step on a theme switch. A manifest's `"colorScheme":
  // "own"` opts out: the Garden's scheme is only the starting value.
  const scheme = theme.scheme === 'dark' ? 'dark' : 'light'
  return `<style data-k2-theme="">:root{${lines.join('')}color-scheme:${scheme}${ownScheme ? '' : '!important'};background:transparent;}html,body{background:transparent;}</style>`
}

/** Everything K2 puts after the CSP meta (see the file comment). */
export function zenFramePrelude(opts: { nonce: string; theme: ZenFrameTheme; libs: readonly string[]; ownScheme?: boolean }): string {
  const own = opts.ownScheme ? `document.documentElement.setAttribute('data-k2-scheme', 'own');` : ''
  const scheme = `document.documentElement.setAttribute('data-zen-scheme', ${JSON.stringify(opts.theme.scheme)});${own}`
  return [
    nonced(opts.nonce, SEALED_FRAME_GUARD),
    zenFrameThemeCss(opts.theme, opts.ownScheme === true),
    nonced(opts.nonce, scheme),
    nonced(opts.nonce, k2FrameScript()),
    ...opts.libs.map((text) => nonced(opts.nonce, text)),
  ].join('')
}
