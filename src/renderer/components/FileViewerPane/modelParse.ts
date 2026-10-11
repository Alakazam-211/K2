// 3D model bytes → a three.js object + stats. Lazy (three.js is big):
// only ModelView's dynamic import reaches this file, and each format's
// loader is its own dynamic import again. No DOM beyond DOMParser (3MF),
// so the same code runs under Node in scripts/test-cad-samples.mjs.

import {
  Box3,
  BufferGeometry,
  DoubleSide,
  Group,
  LineBasicMaterial,
  Material,
  Mesh,
  MeshPhongMaterial,
  MeshStandardMaterial,
  Object3D,
  Points,
  PointsMaterial,
  Texture,
  Vector3,
} from 'three'

export type ModelStats = {
  triangles: number
  meshes: number
  /** Bounding box in the file's own axes and units (before Z-up turn). */
  size: [number, number, number]
}
export type ParsedModel = { root: Object3D; stats: ModelStats }

/** Neutral grey for meshes that bring no material of their own. */
export const MODEL_GREY = 0xb4b9c2

export function greyMaterial(vertexColors = false): MeshStandardMaterial {
  return new MeshStandardMaterial({
    color: vertexColors ? 0xffffff : MODEL_GREY,
    vertexColors,
    roughness: 0.55,
    metalness: 0.05,
    // STL/OBJ winding is often inconsistent: show both faces.
    side: DoubleSide,
  })
}

function toArrayBuffer(bytes: Uint8Array): ArrayBuffer {
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer
}

function hasColors(g: BufferGeometry): boolean {
  return Boolean(g.getAttribute('color'))
}

/** Mesh (or, for a faceless PLY, a point cloud) with our grey. */
function meshFromGeometry(g: BufferGeometry, points = false): Object3D {
  const colors = hasColors(g)
  if (points) {
    return new Points(
      g,
      new PointsMaterial({
        size: 2,
        sizeAttenuation: false,
        vertexColors: colors,
        color: colors ? 0xffffff : MODEL_GREY,
      }),
    )
  }
  if (!g.getAttribute('normal')) g.computeVertexNormals()
  return new Mesh(g, greyMaterial(colors))
}

/** A plain white Phong with no map: the loader's "no material" default. */
function isDefaultMaterial(m: Material): boolean {
  if (!(m instanceof MeshPhongMaterial)) return false
  return !m.map && !m.vertexColors && m.color.getHex() === 0xffffff
}

/** Swap loader defaults (and every OBJ material: no .mtl is fetched) for grey. */
function greyDefaults(root: Object3D, all: boolean) {
  root.traverse((o) => {
    if (!(o instanceof Mesh)) return
    const swap = (m: Material) => {
      if (!all && !isDefaultMaterial(m)) return m
      m.dispose()
      return greyMaterial(hasColors(o.geometry))
    }
    o.material = Array.isArray(o.material) ? o.material.map(swap) : swap(o.material)
  })
}

/** glTF that needs files or decoders we cannot reach from bytes alone. */
function checkGltfJson(json: {
  buffers?: { uri?: string }[]
  images?: { uri?: string }[]
  extensionsRequired?: string[]
}) {
  const external = [...(json.buffers ?? []), ...(json.images ?? [])].filter(
    (b) => typeof b.uri === 'string' && !b.uri.startsWith('data:'),
  )
  if (external.length) {
    throw new Error(
      'This .gltf points at separate files (buffers or textures). Only self-contained .gltf (data: URIs) or .glb preview here.',
    )
  }
  const needs = (json.extensionsRequired ?? []).filter((x) => /draco|meshopt|basisu|ktx/i.test(x))
  if (needs.length) throw new Error(`This glTF needs ${needs.join(', ')}, which this viewer does not decode.`)
}

function glbJson(buf: ArrayBuffer): unknown {
  const v = new DataView(buf)
  if (buf.byteLength < 20 || v.getUint32(0, true) !== 0x46546c67) return null
  const len = v.getUint32(12, true)
  if (v.getUint32(16, true) !== 0x4e4f534a) return null
  return JSON.parse(new TextDecoder().decode(new Uint8Array(buf, 20, len)))
}

export function modelStats(root: Object3D): ModelStats {
  let triangles = 0
  let meshes = 0
  root.updateMatrixWorld(true)
  root.traverse((o) => {
    if (!(o instanceof Mesh)) return
    const g = o.geometry as BufferGeometry
    const count = g.index ? g.index.count : (g.getAttribute('position')?.count ?? 0)
    triangles += Math.floor(count / 3)
    meshes++
  })
  const box = new Box3().setFromObject(root)
  const s = box.isEmpty() ? new Vector3() : box.getSize(new Vector3())
  return { triangles, meshes, size: [s.x, s.y, s.z] }
}

/** Parse by extension (".stl", ".obj", ".3mf", ".ply", ".glb", ".gltf"). */
export async function parseModel(bytes: Uint8Array, ext: string): Promise<ParsedModel> {
  const e = ext.toLowerCase()
  const buf = toArrayBuffer(bytes)
  let root: Object3D
  // CAD/print formats are Z-up; three.js is Y-up.
  let zUp = false

  if (e === '.stl') {
    const { STLLoader } = await import('three/addons/loaders/STLLoader.js')
    // STLLoader's binary sniff reads a DataView past 84 bytes: a tiny or
    // truncated file throws a RangeError. Say what it is instead.
    let g: BufferGeometry
    try {
      g = new STLLoader().parse(buf)
    } catch (err) {
      throw new Error(`This STL could not be read (${err instanceof Error ? err.message : String(err)}).`)
    }
    if (!g.getAttribute('position')?.count) throw new Error('No triangles in this STL.')
    root = meshFromGeometry(g)
    zUp = true
  } else if (e === '.obj') {
    const { OBJLoader } = await import('three/addons/loaders/OBJLoader.js')
    root = new OBJLoader().parse(new TextDecoder().decode(bytes))
    greyDefaults(root, true)
    root.traverse((o) => {
      if (o instanceof Points) o.material = new PointsMaterial({ size: 2, sizeAttenuation: false, color: MODEL_GREY })
      else if (!(o instanceof Mesh) && 'material' in o)
        (o as { material: Material }).material = new LineBasicMaterial({ color: MODEL_GREY })
    })
  } else if (e === '.3mf') {
    const { ThreeMFLoader } = await import('three/addons/loaders/3MFLoader.js')
    root = new ThreeMFLoader().parse(buf)
    greyDefaults(root, false)
    zUp = true
  } else if (e === '.ply') {
    const { PLYLoader } = await import('three/addons/loaders/PLYLoader.js')
    const g = new PLYLoader().parse(buf)
    // PLYLoader indexes faces; no index means a point cloud.
    root = meshFromGeometry(g, !g.index)
  } else if (e === '.glb' || e === '.gltf') {
    const { GLTFLoader } = await import('three/addons/loaders/GLTFLoader.js')
    let data: ArrayBuffer | string = buf
    if (e === '.gltf') {
      data = new TextDecoder().decode(bytes)
      checkGltfJson(JSON.parse(data))
    } else {
      const json = glbJson(buf)
      if (json) checkGltfJson(json as Parameters<typeof checkGltfJson>[0])
    }
    const gltf = await new GLTFLoader().parseAsync(data, '')
    root = gltf.scene
  } else {
    throw new Error(`No 3D loader for ${ext || 'this file'}.`)
  }

  const stats = modelStats(root)
  if (stats.triangles === 0 && stats.meshes === 0) {
    // Points and lines count too, including a root that is itself the
    // object (a faceless PLY's point cloud).
    let any = false
    root.traverse((o) => {
      const g = (o as { geometry?: BufferGeometry }).geometry
      if ((g?.getAttribute('position')?.count ?? 0) > 0) any = true
    })
    if (!any) throw new Error('No geometry in this file.')
  }
  if (zUp) {
    const wrap = new Group()
    root.rotation.x = -Math.PI / 2
    wrap.add(root)
    root = wrap
  }
  return { root, stats }
}

/** Free GPU-side resources: geometries, materials and their textures. */
export function disposeObject(root: Object3D) {
  root.traverse((o) => {
    const g = (o as { geometry?: BufferGeometry }).geometry
    if (g) g.dispose()
    const mat = (o as { material?: Material | Material[] }).material
    for (const m of Array.isArray(mat) ? mat : mat ? [mat] : []) {
      for (const v of Object.values(m)) if (v instanceof Texture) v.dispose()
      m.dispose()
    }
  })
}
