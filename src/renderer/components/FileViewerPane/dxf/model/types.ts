// The CAD pack's drawing model, as the viewer reads it: one header per
// version (layers, blocks, layouts, settings) and pages of entities in draw
// order, each with its style already resolved and its geometry in the
// drawing's world plan (angles in radians, counter-clockwise). Inside a block
// definition a value that depends on the placement is the word "insert": the
// referencing INSERT's value. The same shapes serve DXF and DWG.

export type XY = [number, number]
export type RGB = [number, number, number]
/** `[x0, y0, x1, y1]`. */
export type Box4 = [number, number, number, number]
/** A value, or "insert": the referencing INSERT's. */
export type OrInsert<T> = T | 'insert'

export type ModelStyle = {
  rgb: OrInsert<RGB>
  /** The theme's foreground (the format's colour 7): drawn black on light, white on dark. */
  fg: boolean
  aci?: number | null
  alpha: OrInsert<number>
  ltype: OrInsert<string>
  lt_scale?: number
  lw_mm: OrInsert<number>
}

export type ModelRow = {
  /** `@h:<HANDLE>`. */
  h: string
  kind: string
  layer: string
  /** `model`, a layout's `@h:`, or a block's `@h:`. */
  owner: string
  z: number
  style?: ModelStyle
  visible: boolean | 'insert'
  drawn: boolean
  /** Why it isn't drawn: one of the header's `whys`. */
  why?: string
  bbox: Box4 | null
  bbox_q?: string
  /** Why the edit verbs won't change it (`grouped`, `locked-layer`, …), when the model says; absent or null when they will. */
  edit?: string | null
  // Each kind's own fields; read by the renderer kind by kind.
  geom?: Record<string, unknown> | null
}

export type ModelPage = {
  schema: 'cad.drawing.page/1'
  version: number
  space: string
  entities: ModelRow[]
  not_drawn?: number
  /** The next call's arguments, or null on the last page. */
  next: string[] | null
}

export type ModelLayer = {
  handle: string
  name: string
  rgb: RGB
  fg: boolean
  alpha: number
  ltype?: string
  lw_mm?: number
  on: boolean
  frozen: boolean
  locked?: boolean
}

/** A linetype: its pattern in drawing units (positive a dash, negative a gap, 0 a dot). */
export type ModelLinetype = {
  handle?: string
  name: string
  length?: number
  dashes: number[]
  /** Text or shapes placed along the line (complex linetypes). */
  complex?: unknown[]
}

export type ModelBlock = {
  handle: string
  name: string
  base: XY
  entities: number
  bbox: Box4 | null
  digest: string
  anon: boolean
  xref: boolean
}

export type ModelLayout = {
  handle: string
  name: string
  kind: 'model' | 'paper'
  space: string
  entities: number
  tab?: number
}

export type ModelHeader = {
  schema: 'cad.drawing.header/1'
  version: number
  /** `dxf` or `dwg`. */
  format?: string
  /** The file's format version (`AC1032`); AC1015 is DXF 2000. */
  acadver?: string
  /** The host can save this file's edits (false: it opens read only, with `read_only_reason` when given). */
  writer?: boolean
  read_only_reason?: string
  units?: { word?: string; insunits?: number }
  extents?: { stated: Box4 | null; computed: Box4 | null }
  settings?: { pdmode?: number; pdsize?: number; fillmode?: boolean; lwdisplay?: boolean; ltscale?: number; lwdefault_mm?: number }
  layers: ModelLayer[]
  linetypes?: ModelLinetype[]
  blocks: ModelBlock[]
  /** Model space's own block record (`@h:`): a hold on it covers all of model space. Null for an old drawing without handles. */
  model_space_block?: string | null
  layouts: ModelLayout[]
  whys?: string[]
}

export const isHeader = (v: unknown): v is ModelHeader =>
  typeof v === 'object' && v !== null && (v as { schema?: unknown }).schema === 'cad.drawing.header/1' && Array.isArray((v as ModelHeader).layers)

export const isPage = (v: unknown): v is ModelPage =>
  typeof v === 'object' && v !== null && (v as { schema?: unknown }).schema === 'cad.drawing.page/1' && Array.isArray((v as ModelPage).entities)
