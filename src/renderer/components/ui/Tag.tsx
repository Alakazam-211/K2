import { cx } from './cx'

/** What a tag says, by colour. `accent` is NEW (and Beta, through
 *  `BetaTag`), `muted` is SOON or OFF (an outline, quieter than the row it
 *  sits on), `neutral` is a plain label, and `ok`, `warn` and `danger` are
 *  states. */
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

/** What a Beta tag says on hover. */
export const BETA_TAG_TITLE = 'This feature is in beta — interface and behavior may change'

export type BetaTagProps = Omit<TagProps, 'tone' | 'children'>

/** The one Beta mark, everywhere the app shows one: the blue (accent) tag
 *  the Settings nav draws. Put it straight after the label it marks; in a
 *  flex row the row's `gap-2` spaces it, in running text pass `ml-2`. It
 *  sits on the line's middle either way. The text is "Beta" (read as a
 *  word, found by a "beta" search); `.k2-tag` sets it in capitals. */
export function BetaTag({ title = BETA_TAG_TITLE, className, ...rest }: BetaTagProps): React.JSX.Element {
  return (
    <Tag tone="accent" data-beta-tag="" title={title} className={cx('align-middle', className)} {...rest}>
      Beta
    </Tag>
  )
}
