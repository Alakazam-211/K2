// Scene helpers for the Wiki 3D view (prd-wiki-3d-view-v1 §3.6, §3.7, §6.3).
// LAZY: imported by WikiGraph3D only (it pulls in three).
//
// - Shared geometries (one sphere, one octahedron, one box) and materials
//   shared per visual state, cached by key: hover/selection swap material
//   references, never create per-node materials.
// - One radial-gradient halo texture for the glow on key notes (additive
//   on dark palettes, normal blending on light ones, where additive vanishes).
// - A label pool (three-spritetext): the always-on set plus the nearest
//   notes, never one sprite per node.
// - Alpha-aware CSS colour parsing (`three.Color` drops alpha).

import {
  AdditiveBlending,
  BoxGeometry,
  CanvasTexture,
  Color,
  Group,
  Mesh,
  MeshLambertMaterial,
  NormalBlending,
  OctahedronGeometry,
  SphereGeometry,
  Sprite,
  SpriteMaterial,
  SRGBColorSpace,
  type BufferGeometry,
  type Object3D,
} from 'three'
import SpriteText from 'three-spritetext'

// ── Pure helpers (unit-tested) ─────────────────────────────────────────

export type Rgba = { r: number; g: number; b: number; a: number }
export type Vec3 = { x: number; y: number; z: number }

function clamp01(n: number): number {
  return n < 0 ? 0 : n > 1 ? 1 : n
}

function parseAlpha(raw: string | undefined): number {
  if (raw === undefined) return 1
  const t = raw.trim()
  if (t.endsWith('%')) return clamp01(parseFloat(t) / 100)
  const n = parseFloat(t)
  return Number.isFinite(n) ? clamp01(n) : 1
}

function parseChannel(raw: string): number {
  const t = raw.trim()
  if (t.endsWith('%')) return clamp01(parseFloat(t) / 100)
  const n = parseFloat(t)
  return Number.isFinite(n) ? clamp01(n / 255) : NaN
}

/**
 * Parse `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb()`/`rgba()` (comma or
 * space syntax, `/ alpha`, % channels) into 0–1 channels, KEEPING alpha.
 * Anything else (named colours, `color-mix()`, empty) → null.
 */
export function parseCssColor(input: string | null | undefined): Rgba | null {
  if (!input) return null
  const s = input.trim().toLowerCase()
  if (s.startsWith('#')) {
    const hex = s.slice(1)
    if (!/^[0-9a-f]+$/.test(hex)) return null
    const full =
      hex.length === 3 || hex.length === 4
        ? hex
            .split('')
            .map((c) => c + c)
            .join('')
        : hex
    if (full.length !== 6 && full.length !== 8) return null
    const ch = (i: number) => parseInt(full.slice(i, i + 2), 16) / 255
    return { r: ch(0), g: ch(2), b: ch(4), a: full.length === 8 ? ch(6) : 1 }
  }
  const m = /^rgba?\(\s*([^)]*)\)$/.exec(s)
  if (!m) return null
  const body = m[1]
  let parts: string[]
  let alpha: string | undefined
  if (body.includes(',')) {
    parts = body.split(',').map((p) => p.trim())
    if (parts.length === 4) alpha = parts.pop()
  } else {
    const [chs, a] = body.split('/')
    parts = chs.trim().split(/\s+/)
    alpha = a?.trim()
  }
  if (parts.length !== 3) return null
  const [r, g, b] = parts.map(parseChannel)
  if ([r, g, b].some((v) => Number.isNaN(v))) return null
  return { r, g, b, a: parseAlpha(alpha) }
}

/** `rgba(r, g, b, a)` with 0–255 channels; alpha multiplied by `alphaMul`. */
export function rgbaCss(c: Rgba, alphaMul = 1): string {
  const to255 = (v: number) => Math.round(clamp01(v) * 255)
  const a = Math.round(clamp01(c.a * alphaMul) * 1000) / 1000
  return `rgba(${to255(c.r)}, ${to255(c.g)}, ${to255(c.b)}, ${a})`
}

/** WCAG relative luminance (0–1). */
export function relativeLuminance(c: Rgba): number {
  const lin = (v: number) => (v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4))
  return 0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

/** Light palette (porcelain, veil…) when `--color-bg` is bright. */
export function isLightBackground(bg: Rgba | null): boolean {
  return bg ? relativeLuminance(bg) > 0.4 : false
}

export type LabelCandidate = { id: string; x?: number; y?: number; z?: number }

/**
 * Which nodes get a label: every always-on id that is on the graph (in node
 * order), then up to `max` other nodes nearest the camera within `maxDist`
 * (nearest first, ties by id). Nodes without a position are skipped.
 */
export function pickLabelIds(
  nodes: readonly LabelCandidate[],
  camera: Vec3,
  always: ReadonlySet<string>,
  opts: { max: number; maxDist: number },
): string[] {
  const out: string[] = []
  const near: { id: string; d: number }[] = []
  for (const n of nodes) {
    if (always.has(n.id)) {
      out.push(n.id)
      continue
    }
    if (n.x == null || n.y == null || n.z == null) continue
    const dx = n.x - camera.x
    const dy = n.y - camera.y
    const dz = n.z - camera.z
    const d = Math.sqrt(dx * dx + dy * dy + dz * dz)
    if (d <= opts.maxDist) near.push({ id: n.id, d })
  }
  near.sort((a, b) => a.d - b.d || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))
  for (let i = 0; i < near.length && i < opts.max; i++) out.push(near[i].id)
  return out
}

/**
 * Camera position for a fly-to: `distance` from the node along the current
 * view direction (target → camera), so the angle you were looking from is
 * kept. A degenerate direction falls back to +z.
 */
export function flyToPosition(node: Vec3, camera: Vec3, target: Vec3, distance: number): Vec3 {
  let dx = camera.x - target.x
  let dy = camera.y - target.y
  let dz = camera.z - target.z
  const len = Math.sqrt(dx * dx + dy * dy + dz * dz)
  if (!Number.isFinite(len) || len < 1e-6) {
    dx = 0
    dy = 0
    dz = 1
  } else {
    dx /= len
    dy /= len
    dz /= len
  }
  return { x: node.x + dx * distance, y: node.y + dy * distance, z: node.z + dz * distance }
}

// ── Theme ──────────────────────────────────────────────────────────────

export type Wiki3dTheme = {
  accent: Rgba
  text: Rgba
  muted: Rgba
  bg: Rgba | null
  homeSoft: Rgba
  light: boolean
  /** Computed body font family for labels. */
  font: string
}

const FALLBACK = {
  accent: '#6c8cff',
  text: '#e8e8e8',
  muted: '#6b7280',
  homeSoft: '#8fa8ff',
}

/** Read the palette tokens from `<html>` (alpha kept). */
export function readWiki3dTheme(doc: Document = document): Wiki3dTheme {
  const cs = getComputedStyle(doc.documentElement)
  const tok = (name: string, fb: string): Rgba =>
    parseCssColor(cs.getPropertyValue(name).trim()) ?? (parseCssColor(fb) as Rgba)
  const bg = parseCssColor(cs.getPropertyValue('--color-bg').trim())
  const font = (doc.body ? getComputedStyle(doc.body).fontFamily : '') || 'sans-serif'
  return {
    accent: tok('--color-accent', FALLBACK.accent),
    text: tok('--color-text-primary', FALLBACK.text),
    muted: tok('--color-text-muted', FALLBACK.muted),
    bg,
    homeSoft: parseCssColor(FALLBACK.homeSoft) as Rgba,
    light: isLightBackground(bg),
    font,
  }
}

export function themeKey(t: Wiki3dTheme): string {
  return [rgbaCss(t.accent), rgbaCss(t.text), rgbaCss(t.muted), t.bg ? rgbaCss(t.bg) : 'none', t.font].join('|')
}

// ── Node visuals ───────────────────────────────────────────────────────

export type NodeShape = 'sphere' | 'wire' | 'octa' | 'box'

export type NodeVisual = {
  shape: NodeShape
  color: Rgba
  opacity: number
  /** Radius in graph units. */
  radius: number
  glow: boolean
  /** Label colour. */
  labelColor: Rgba
}

/** Radii (graph units), same as 2D. */
export const R_NOTE = 4
export const R_SELECTED = 6
export const R_ORG = 7
export const R_HUB = 5

/** Hover dims everything outside the hovered node's 1-hop set to this. */
export const DIM_FACTOR = 0.2

export type NodeKind = {
  missing: boolean
  home: boolean
  hub: boolean
  org: boolean
  /** Group / project colour (css) for hubs, org nodes and Home. */
  orgColor: string | null
}

export function nodeVisual(
  kind: NodeKind,
  state: { selected: boolean; hovered: boolean; dimmed: boolean },
  theme: Wiki3dTheme,
): NodeVisual {
  const org = kind.orgColor ? parseCssColor(kind.orgColor) : null
  let shape: NodeShape = 'sphere'
  let color = theme.text
  let opacity = 0.85
  let radius = R_NOTE
  let glow = false
  let labelColor = theme.text

  if (kind.missing) {
    shape = 'wire'
    color = theme.muted
    opacity = 0.45
    labelColor = theme.muted
  } else if (kind.org) {
    shape = 'box'
    color = org ?? theme.homeSoft
    radius = R_ORG
    glow = true
    labelColor = org ?? theme.homeSoft
  } else if (kind.hub) {
    shape = 'octa'
    color = org ?? theme.muted
    opacity = 0.9
    radius = R_HUB
    glow = true
  } else if (kind.home) {
    color = org ?? theme.homeSoft
    opacity = 0.9
    glow = true
    labelColor = org ?? theme.homeSoft
  }

  if (state.selected) {
    color = theme.accent
    opacity = 1
    radius = kind.org ? R_ORG : kind.hub ? R_HUB + 1 : R_SELECTED
    glow = true
    if (!kind.org) labelColor = theme.text
  } else if (state.hovered) {
    color = theme.accent
    opacity = 0.75
  }
  if (state.dimmed) opacity *= DIM_FACTOR
  return { shape, color, opacity, radius, glow, labelColor }
}

// ── Scene objects (three) ─────────────────────────────────────────────

const HALO_SIZE = 4.6 // × radius
const LABEL_HEIGHT = 3.2
const LABEL_HEIGHT_KEY = 4

type NodeEntry = {
  group: Group
  mesh: Mesh
  halo: Sprite | null
}

function makeHaloTexture(): CanvasTexture | null {
  if (typeof document === 'undefined') return null
  const c = document.createElement('canvas')
  c.width = 128
  c.height = 128
  const ctx = c.getContext('2d')
  if (!ctx) return null
  const g = ctx.createRadialGradient(64, 64, 0, 64, 64, 64)
  g.addColorStop(0, 'rgba(255,255,255,1)')
  g.addColorStop(0.35, 'rgba(255,255,255,0.45)')
  g.addColorStop(1, 'rgba(255,255,255,0)')
  ctx.fillStyle = g
  ctx.fillRect(0, 0, 128, 128)
  const tex = new CanvasTexture(c)
  tex.colorSpace = SRGBColorSpace
  return tex
}

function toColor(c: Rgba): Color {
  return new Color().setRGB(c.r, c.g, c.b, SRGBColorSpace)
}

/**
 * Owns everything the 3D view creates in three: shared geometries, the
 * material cache, the halo texture, node groups and the label pool.
 * `dispose()` frees all of it.
 */
export class Wiki3dScene {
  readonly geometries: Record<'sphere' | 'octa' | 'box', BufferGeometry> = {
    sphere: new SphereGeometry(1, 16, 12),
    octa: new OctahedronGeometry(1.15),
    box: new BoxGeometry(1.35, 1.35, 1.35),
  }
  private materials = new Map<string, MeshLambertMaterial>()
  private haloMaterials = new Map<string, SpriteMaterial>()
  private haloTexture: CanvasTexture | null = null
  private nodes = new Map<string, NodeEntry>()
  private labels = new Map<string, SpriteText>()
  readonly labelGroup = new Group()
  theme: Wiki3dTheme
  glowEnabled = true
  private disposed = false

  constructor(theme: Wiki3dTheme) {
    this.theme = theme
    this.labelGroup.name = 'wiki3d-labels'
  }

  /** Swap palettes: drop cached materials and label sprites (re-made on demand). */
  setTheme(theme: Wiki3dTheme): void {
    this.theme = theme
    for (const m of this.materials.values()) m.dispose()
    this.materials.clear()
    for (const m of this.haloMaterials.values()) m.dispose()
    this.haloMaterials.clear()
    this.clearLabels()
  }

  material(shape: NodeShape, color: Rgba, opacity: number): MeshLambertMaterial {
    const o = Math.round(clamp01(opacity * color.a) * 100) / 100
    const key = `${shape === 'wire' ? 'w' : 's'}|${rgbaCss({ ...color, a: 1 })}|${o}`
    let m = this.materials.get(key)
    if (!m) {
      m = new MeshLambertMaterial({
        color: toColor(color),
        transparent: o < 1,
        opacity: o,
        wireframe: shape === 'wire',
        depthWrite: o >= 0.5,
      })
      this.materials.set(key, m)
    }
    return m
  }

  haloMaterial(color: Rgba): SpriteMaterial | null {
    if (!this.haloTexture) this.haloTexture = makeHaloTexture()
    if (!this.haloTexture) return null
    const light = this.theme.light
    const key = `${rgbaCss({ ...color, a: 1 })}|${light ? 'l' : 'd'}`
    let m = this.haloMaterials.get(key)
    if (!m) {
      m = new SpriteMaterial({
        map: this.haloTexture,
        color: toColor(color),
        transparent: true,
        opacity: light ? 0.25 : 0.35,
        blending: light ? NormalBlending : AdditiveBlending,
        depthWrite: false,
      })
      this.haloMaterials.set(key, m)
    }
    return m
  }

  /** Create the object for a node (called by three-forcegraph once per node). */
  createNode(id: string, visual: NodeVisual): Group {
    const group = new Group()
    const mesh = new Mesh(this.geometries.sphere, this.material(visual.shape, visual.color, visual.opacity))
    group.add(mesh)
    const entry: NodeEntry = { group, mesh, halo: null }
    this.nodes.set(id, entry)
    this.applyVisual(id, visual)
    return group
  }

  hasNode(id: string): boolean {
    return this.nodes.has(id)
  }

  applyVisual(id: string, v: NodeVisual): void {
    const e = this.nodes.get(id)
    if (!e) return
    const geo = v.shape === 'octa' ? this.geometries.octa : v.shape === 'box' ? this.geometries.box : this.geometries.sphere
    if (e.mesh.geometry !== geo) e.mesh.geometry = geo
    e.mesh.material = this.material(v.shape, v.color, v.opacity)
    e.mesh.scale.setScalar(v.radius)
    const wantHalo = v.glow && this.glowEnabled && v.opacity > 0.3
    if (wantHalo) {
      const mat = this.haloMaterial(v.color)
      if (mat) {
        if (!e.halo) {
          e.halo = new Sprite(mat)
          e.halo.renderOrder = -1
          e.halo.raycast = () => {} // the halo is not a hit target
          e.group.add(e.halo)
        }
        e.halo.material = mat
        e.halo.scale.setScalar(v.radius * HALO_SIZE)
        e.halo.visible = true
      }
    } else if (e.halo) {
      e.halo.visible = false
    }
  }

  /** Forget node entries the graph no longer has (the library disposed their objects). */
  prune(liveIds: ReadonlySet<string>): void {
    for (const id of [...this.nodes.keys()]) {
      if (!liveIds.has(id)) this.nodes.delete(id)
    }
    for (const id of [...this.labels.keys()]) {
      if (!liveIds.has(id)) this.dropLabel(id)
    }
  }

  nodeIds(): string[] {
    return [...this.nodes.keys()]
  }

  /** Show labels for exactly `ids`; reuse pooled sprites, detach the rest. */
  setLabels(
    ids: readonly string[],
    info: (id: string) => { text: string; color: Rgba; key: boolean; pos: Vec3 | null; radius: number } | null,
  ): void {
    const want = new Set(ids)
    for (const [id, s] of this.labels) {
      if (!want.has(id)) s.visible = false
    }
    for (const id of ids) {
      const i = info(id)
      if (!i || !i.pos) continue
      let s = this.labels.get(id)
      const color = rgbaCss(i.color)
      if (!s) {
        s = new SpriteText(i.text, i.key ? LABEL_HEIGHT_KEY : LABEL_HEIGHT, color)
        s.fontFace = this.theme.font
        s.fontSize = 64
        s.material.depthWrite = false
        s.renderOrder = 20
        s.raycast = () => {}
        this.labels.set(id, s)
        this.labelGroup.add(s)
      } else {
        if (s.text !== i.text) s.text = i.text
        if (s.color !== color) s.color = color
        const h = i.key ? LABEL_HEIGHT_KEY : LABEL_HEIGHT
        if (s.textHeight !== h) s.textHeight = h
      }
      s.userData.radius = i.radius
      s.position.set(i.pos.x, i.pos.y - i.radius - s.textHeight * 0.8, i.pos.z)
      s.visible = true
    }
  }

  /** Move visible labels with their nodes (engine ticks, node drags). */
  syncLabelPositions(pos: (id: string) => Vec3 | null): void {
    for (const [id, s] of this.labels) {
      if (!s.visible) continue
      const p = pos(id)
      if (!p) continue
      const r = typeof s.userData.radius === 'number' ? (s.userData.radius as number) : R_NOTE
      s.position.set(p.x, p.y - r - s.textHeight * 0.8, p.z)
    }
  }

  visibleLabelIds(): string[] {
    return [...this.labels].filter(([, s]) => s.visible).map(([id]) => id)
  }

  private dropLabel(id: string): void {
    const s = this.labels.get(id)
    if (!s) return
    this.labelGroup.remove(s)
    s.material.map?.dispose()
    s.material.dispose()
    this.labels.delete(id)
  }

  private clearLabels(): void {
    for (const id of [...this.labels.keys()]) this.dropLabel(id)
  }

  /** Free every geometry, material, texture and sprite this view made. */
  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    this.clearLabels()
    this.labelGroup.parent?.remove(this.labelGroup)
    for (const m of this.materials.values()) m.dispose()
    this.materials.clear()
    for (const m of this.haloMaterials.values()) m.dispose()
    this.haloMaterials.clear()
    this.haloTexture?.dispose()
    this.haloTexture = null
    for (const g of Object.values(this.geometries)) g.dispose()
    for (const e of this.nodes.values()) e.group.parent?.remove(e.group as Object3D)
    this.nodes.clear()
  }

  get isDisposed(): boolean {
    return this.disposed
  }
}
