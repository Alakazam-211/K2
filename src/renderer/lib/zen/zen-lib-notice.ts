// NOTICE.md's "Zen widget library" section, generated from
// `src/shared/zen-lib.json` (prd-zen-user-widgets-v2 UWB16).
//
// One pure renderer used by both `scripts/vendor-zen-lib.sh` (which writes
// the section) and `zen-lib.test.ts` (which fails when NOTICE.md and the
// manifest disagree). No imports beyond types, so the vendor script (bun)
// can load it directly.

import type { ZenLibManifest } from './zen-lib-loader'

export const ZEN_LIB_NOTICE_BEGIN =
  '<!-- zen-lib:begin (generated from src/shared/zen-lib.json by scripts/vendor-zen-lib.sh; do not edit by hand) -->'
export const ZEN_LIB_NOTICE_END = '<!-- zen-lib:end -->'

function cell(s: string): string {
  return s.replace(/\|/g, '\\|').replace(/\s+/g, ' ').trim()
}

/** The text between the markers (markers included). */
export function renderZenLibNotice(m: ZenLibManifest): string {
  const rows = m.libs.map((l) => {
    const shipped =
      l.source === 'bundled'
        ? `shipped in the app, \`src/renderer/zen-lib/${l.id}@${l.version}/\``
        : `downloaded on first use by this computer's K2 (not shipped); licence kept at \`src/renderer/zen-lib/${l.id}@${l.version}/${l.licenseFile}\``
    return `| ${cell(l.title)} (\`${l.id}\`) | ${cell(l.version)} | ${cell(l.license)} | ${cell(l.copyright)} | ${shipped} |`
  })
  return [
    ZEN_LIB_NOTICE_BEGIN,
    '',
    'Libraries a Garden widget can ask for with `requires.libs` (the Zen widget',
    'standard library). K2 inlines each one into the sealed widget frame; the frame',
    'itself never touches the network. Each library folder keeps its full licence',
    'text in `LICENSE.txt` (Apache-2.0 libraries include their upstream NOTICE).',
    '',
    '| Library | Version | Licence | Copyright | Where |',
    '|--|--|--|--|--|',
    ...rows,
    '',
    'K2 code remains **FSL-1.1-Apache-2.0**. These libraries keep their own',
    'licences and do **not** relicense K2.',
    '',
    ZEN_LIB_NOTICE_END,
  ].join('\n')
}

/** `notice` with its generated section replaced (or appended under a
 *  heading when it has none yet). */
export function replaceZenLibNotice(notice: string, m: ZenLibManifest): string {
  const section = renderZenLibNotice(m)
  const a = notice.indexOf(ZEN_LIB_NOTICE_BEGIN)
  const b = notice.indexOf(ZEN_LIB_NOTICE_END)
  if (a >= 0 && b > a) return notice.slice(0, a) + section + notice.slice(b + ZEN_LIB_NOTICE_END.length)
  const base = notice.endsWith('\n') ? notice : `${notice}\n`
  return `${base}\n## Third-party: Zen widget library\n\n${section}\n`
}

/** The generated section currently in `notice`, or null. */
export function currentZenLibNotice(notice: string): string | null {
  const a = notice.indexOf(ZEN_LIB_NOTICE_BEGIN)
  const b = notice.indexOf(ZEN_LIB_NOTICE_END)
  if (a < 0 || b <= a) return null
  return notice.slice(a, b + ZEN_LIB_NOTICE_END.length)
}
