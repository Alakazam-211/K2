// The sheet a drawing sits on: always dark, as CAD apps draw model space.
// CAD files keep their own colours, so yellow (colour 2) text all but
// vanishes on a light look's white sheet. The drawing area paints
// --color-cad-sheet: a dark sheet in the light looks, the look's own
// background in the dark ones. Colour 7 (the foreground, `currentColor` on
// both the model path and the viewer's own reading) draws white. Only the
// drawing area: the header and the stats line keep the look. `--dxf-sheet`
// is the sheet's colour for what covers the drawing (a wipeout), `--dxf-note`
// the colour of a note drawn on the sheet.

export const SHEET_STYLE = {
  '--dxf-sheet': 'var(--color-cad-sheet)',
  '--dxf-note': 'rgb(255 255 255 / 0.7)',
  background: 'var(--dxf-sheet)',
  color: 'white',
} as React.CSSProperties
