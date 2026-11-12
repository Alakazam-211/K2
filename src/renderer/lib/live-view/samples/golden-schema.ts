// The goldens' view schema in the kit's form (k2-mpc-base v0.17.0 `GridSlot`,
// WIRE.md "The live grid"), so the sample sends what a kit host sends.
// Constructed, not in the goldens: the goldens' grid slot (r1.1) predates the
// kit's, and only two of its parts differ in form.

import type { XlsxViewSchema } from '../not-in-kit'
import type { GoldenViewSchema } from './goldens'

/**
 * - `cell_derived`: the goldens' `{names}` becomes the kit's list of names
 *   (a list, as a later copy of the goldens may give it, passes as it is).
 *   KeyedWatch reads it as a list on every rows page and throws on `{names}`.
 * - `style_hint`: the goldens say it in prose ("the cell register's s (0 when
 *   absent)", "Container.derived.styles[<s>]"); the kit names the field, the
 *   default style id, the container derived value and the parts. The view
 *   reads none of it (a missing table or index is the default, r1.1).
 */
export function kitViewSchema(g: GoldenViewSchema): XlsxViewSchema {
  const { cell_derived, style_hint, ...grid } = g.grid
  return {
    ...g,
    grid: {
      ...grid,
      ...(cell_derived ? { cell_derived: Array.isArray(cell_derived) ? [...cell_derived] : [...cell_derived.names] } : {}),
      style_hint: { field: grid.cell.style, default: '0', table: 'styles', parts: Object.keys(style_hint?.parts ?? { bold: '', fill: '', numfmt: '' }) },
    },
  }
}
