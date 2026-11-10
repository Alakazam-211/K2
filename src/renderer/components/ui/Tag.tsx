import { cx } from './cx'

/** What a tag says, by colour. `accent` is BETA or NEW, `muted` is SOON or
 *  OFF (an outline, quieter than the row it sits on), `neutral` is a plain
 *  label, and `ok`, `warn` and `danger` are states. */
export type TagTone = 'accent' | 'muted' | 'neutral' | 'ok' | 'warn' | 'danger'

export interface TagProps extends React.HTMLAttributes<HTMLSpanElement> {
  tone?: TagTone
}

/** The small uppercase tag (BETA, SOON, ON…). A pill in Glass and a chip with
 *  cut corners in Terminal; colours and shape come from `.k2-tag` in
 *  globals.css, where every tone's text clears 4.5:1 in every palette. Set at
 *  the hint size (never below it), medium weight, so it reads without
 *  shouting; `.k2-tag` keeps the line tight. */
export function Tag({ tone = 'neutral', className, ...rest }: TagProps): React.JSX.Element {
  return <span data-k2-chamfer="chip" data-tone={tone} className={cx('k2-tag type-hint font-medium', className)} {...rest} />
}
