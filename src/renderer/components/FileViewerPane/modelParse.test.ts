// @vitest-environment jsdom
// The real three.js loaders (no WebGL needed to parse): 3MF uses
// DOMParser, hence jsdom. Rendering is covered with a mocked scene in
// CadViewers.test.tsx.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { Mesh, Points } from 'three'
import { disposeObject, parseModel } from './modelParse'

// Copy into this realm's Uint8Array: jsdom's TextEncoder hands back a buffer
// that fails three's `instanceof ArrayBuffer` checks (PLYLoader).
const enc = (s: string): Uint8Array => Uint8Array.from(new TextEncoder().encode(s))

/** One right triangle in the XY plane, 10 × 20. */
const STL_ASCII = `solid t
facet normal 0 0 1
 outer loop
  vertex 0 0 0
  vertex 10 0 0
  vertex 0 20 0
 endloop
endfacet
endsolid t
`

/** A unit square as two faces. */
const OBJ = `o sq
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
f 1 2 3
f 1 3 4
`

const PLY_POINTS = `ply
format ascii 1.0
element vertex 3
property float x
property float y
property float z
end_header
0 0 0
1 0 0
0 2 3
`

describe('parseModel with the real three.js loaders', () => {
  it('ASCII STL: triangles, size in the file axes, Z-up turned to Y-up', async () => {
    const { root, stats } = await parseModel(enc(STL_ASCII), '.stl')
    expect(stats.triangles).toBe(1)
    expect(stats.size.map((v) => Math.round(v))).toEqual([10, 20, 0])
    // The wrapper holds the turned model.
    expect(root.children).toHaveLength(1)
    expect(root.children[0].rotation.x).toBeCloseTo(-Math.PI / 2)
    disposeObject(root)
  })

  it('OBJ: grey material on every mesh (no .mtl fetch), Y-up kept', async () => {
    const { root, stats } = await parseModel(enc(OBJ), '.obj')
    expect(stats.triangles).toBe(2)
    expect(stats.meshes).toBe(1)
    let mesh: Mesh | null = null
    root.traverse((o) => {
      if (o instanceof Mesh) mesh = o
    })
    expect(mesh).not.toBeNull()
    expect(root.rotation.x).toBe(0)
    disposeObject(root)
  })

  it('PLY with no faces is a point cloud', async () => {
    const { root } = await parseModel(enc(PLY_POINTS), '.ply')
    expect(root).toBeInstanceOf(Points)
    disposeObject(root)
  })

  it('3MF: the Companion bracket sample (zip + XML)', async () => {
    const bytes = new Uint8Array(
      readFileSync(resolve(__dirname, '__fixtures__/k2-bracket.3mf')),
    )
    const { root, stats } = await parseModel(bytes, '.3mf')
    expect(stats.triangles).toBe(288)
    expect(stats.size.map((v) => Math.round(v))).toEqual([120, 60, 6])
    disposeObject(root)
  })

  it('a .gltf that points at separate files is refused with a clear message', async () => {
    const gltf = JSON.stringify({ asset: { version: '2.0' }, buffers: [{ uri: 'mesh.bin', byteLength: 4 }] })
    await expect(parseModel(enc(gltf), '.gltf')).rejects.toThrow(/separate files/)
  })

  it('a .gltf that needs Draco says so', async () => {
    const gltf = JSON.stringify({ asset: { version: '2.0' }, extensionsRequired: ['KHR_draco_mesh_compression'] })
    await expect(parseModel(enc(gltf), '.gltf')).rejects.toThrow(/KHR_draco_mesh_compression/)
  })

  it('an STL with no triangles is an error, not an empty view', async () => {
    const empty = `solid ${'empty'.repeat(20)}\nendsolid ${'empty'.repeat(20)}\n`
    await expect(parseModel(enc(empty), '.stl')).rejects.toThrow(/No triangles in this STL/)
  })

  it('a truncated STL names the format instead of a bare RangeError', async () => {
    await expect(parseModel(enc('solid e\nendsolid e\n'), '.stl')).rejects.toThrow(/This STL could not be read/)
  })

  it('an unknown extension is refused', async () => {
    await expect(parseModel(enc('x'), '.step')).rejects.toThrow(/No 3D loader for \.step/)
  })
})
