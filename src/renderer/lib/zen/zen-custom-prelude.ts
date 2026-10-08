// prd-zen-user-widgets-v2 UW13, UW39, UW46, UWA5, UWB13 — what K2 puts at
// the top of a sealed widget frame, before any of the widget's own HTML:
//
//   <meta CSP>                      (frame-csp, the `widget` profile)
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

/** TUWA6: the runtime text the frame gets is at most this big. */
export const K2_FRAME_MAX_BYTES = 16 * 1024

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

export function zenFrameThemeCss(theme: ZenFrameTheme): string {
  const lines: string[] = []
  for (const [k, v] of Object.entries(theme.vars)) {
    if (!/^--zen-[a-zA-Z0-9-]+$/.test(k)) continue
    const val = safeCssValue(String(v))
    if (val !== null) lines.push(`${k}:${val};`)
  }
  return `<style data-k2-theme="">:root{${lines.join('')}color-scheme:${theme.scheme};background:transparent;}html,body{background:transparent;}</style>`
}

/** Everything K2 puts after the CSP meta (see the file comment). */
export function zenFramePrelude(opts: { nonce: string; theme: ZenFrameTheme; libs: readonly string[] }): string {
  const scheme = `document.documentElement.setAttribute('data-zen-scheme', ${JSON.stringify(opts.theme.scheme)});`
  return [
    zenFrameThemeCss(opts.theme),
    nonced(opts.nonce, scheme),
    nonced(opts.nonce, k2FrameScript()),
    ...opts.libs.map((text) => nonced(opts.nonce, text)),
  ].join('')
}
