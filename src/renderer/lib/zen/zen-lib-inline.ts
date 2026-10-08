// Making a library's text safe to put inside an HTML <script> element
// (prd-zen-user-widgets-v2 UWB13: K2 inlines each library into the sealed
// widget frame as a nonced classic script).
//
// An inline script ends at the first `</script`, and `<!--` switches the
// HTML parser into the "escaped" states, where a later `<script` can make
// it skip the real end tag. Minified JavaScript only carries these
// sequences inside strings, regular expressions, template literals and
// comments (minifiers never print `<!--` as code: it is an HTML-like
// comment), so two rewrites keep the meaning:
//   `</script` → `<\/script`   (`\/` is `/` in strings, templates and every regex mode)
//   `<!--`     → `\x3C!--`     (`\x3C` is `<` in strings, templates and every regex mode)
// A `<` that was already escaped (`\<`) has its escape replaced, not doubled.
// The one visible difference: a rewritten regular expression's `.source`
// shows the escape (esbuild does the same for `</script`); what it matches
// is unchanged.
//
// Used by scripts/vendor-zen-lib.sh on the bundled files (so they are
// checked in already safe) and by zenLib.texts on downloaded and CDN text.
// No imports, so the vendor script (bun) can load it.

export function zenLibInlineSafe(text: string): string {
  return text
    .replace(/(\\*)<\/(script)/gi, (_m, bs: string, word: string) =>
      bs.length % 2 === 1 ? `${bs.slice(1)}\\x3C\\/${word}` : `${bs}<\\/${word}`,
    )
    .replace(/(\\*)<!--/g, (_m, bs: string) => (bs.length % 2 === 1 ? `${bs.slice(1)}\\x3C!--` : `${bs}\\x3C!--`))
}

/** Why `text` can't be inlined as is, or null. */
export function zenLibInlineProblem(text: string): string | null {
  if (/<\/script/i.test(text)) return 'contains "</script"'
  if (text.includes('<!--')) return 'contains "<!--"'
  return null
}
