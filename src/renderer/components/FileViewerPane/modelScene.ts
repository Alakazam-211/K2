// three.js viewer for ModelViewer. Lazy: imported only when a 3D file
// opens (three core + OrbitControls land in this chunk; each format's
// loader is its own chunk again, see modelParse.ts). Ported from the K2
// Companion (`src/zen/files/modelScene.ts`), with mouse/trackpad controls:
// left-drag orbits, right/middle-drag (or Shift-drag) pans, wheel/pinch
// dollies toward the cursor, arrow keys pan once the canvas has focus,
// double-click resets. One canvas, rendered on demand; everything
// GPU-side is freed on dispose (including the GL context: WebKit caps
// live contexts per page).

import {
  Box3,
  Color,
  DirectionalLight,
  GridHelper,
  Group,
  HemisphereLight,
  Material,
  Object3D,
  PerspectiveCamera,
  Scene,
  Sphere,
  Vector3,
  WebGLRenderer,
} from 'three'
import { OrbitControls } from 'three/addons/controls/OrbitControls.js'
import { disposeObject } from './modelParse'

export { disposeObject, parseModel } from './modelParse'
export type { ModelStats, ParsedModel } from './modelParse'

/** Retina is plenty; more costs fill rate and memory on big displays. */
export const MAX_PIXEL_RATIO = 2

export type ModelViewer = { reset: () => void; dispose: () => void }

/** three.js r163+ is WebGL2-only. Probe on a throwaway canvas. */
export function hasWebGL2(): boolean {
  try {
    const c = document.createElement('canvas')
    const gl = c.getContext('webgl2')
    if (!gl) return false
    gl.getExtension('WEBGL_lose_context')?.loseContext()
    return true
  } catch {
    return false
  }
}

function cssColor(el: Element, name: string, fallback: string): Color {
  const c = new Color(fallback)
  try {
    const v = getComputedStyle(el).getPropertyValue(name).trim()
    if (v) c.setStyle(v)
  } catch {
    // Unparseable token: keep the fallback.
  }
  return c
}

/** A round grid size just past the model's footprint. */
function niceSize(v: number): number {
  const p = 10 ** Math.floor(Math.log10(v))
  for (const m of [1, 2, 5, 10]) if (m * p >= v) return m * p
  return 10 * p
}

export function createViewer(host: HTMLElement, model: Object3D): ModelViewer {
  const canvas = document.createElement('canvas')
  canvas.style.display = 'block'
  canvas.style.width = '100%'
  canvas.style.height = '100%'
  canvas.style.touchAction = 'none'
  canvas.style.outline = 'none'
  // Focusable so OrbitControls' arrow-key pan only fires for this pane.
  canvas.tabIndex = 0
  canvas.setAttribute('data-testid', 'model-canvas')
  host.appendChild(canvas)

  let renderer: WebGLRenderer
  try {
    renderer = new WebGLRenderer({ canvas, antialias: true, powerPreference: 'default' })
  } catch (e) {
    canvas.remove()
    throw e
  }
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, MAX_PIXEL_RATIO))

  const scene = new Scene()
  scene.background = cssColor(host, '--color-bg', '#0f1012')

  // Centre the model on the origin inside a pivot.
  const pivot = new Group()
  pivot.add(model)
  scene.add(pivot)
  pivot.updateMatrixWorld(true)
  const box = new Box3().setFromObject(pivot)
  const center = box.isEmpty() ? new Vector3() : box.getCenter(new Vector3())
  model.position.sub(center)
  pivot.updateMatrixWorld(true)
  box.setFromObject(pivot)
  const sphere = box.isEmpty() ? new Sphere(new Vector3(), 1) : box.getBoundingSphere(new Sphere())
  const r = sphere.radius > 0 ? sphere.radius : 1

  // Ground grid under the model.
  const size = niceSize(r * 4)
  const grid = new GridHelper(size, 20, 0x5a606c, 0x2c3038)
  const gm = grid.material as Material
  gm.transparent = true
  gm.opacity = 0.45
  gm.depthWrite = false
  grid.position.y = box.isEmpty() ? 0 : box.min.y
  scene.add(grid)

  const camera = new PerspectiveCamera(40, 1, r / 100, r * 100)
  const fov = (camera.fov * Math.PI) / 180
  const dist = (r / Math.sin(fov / 2)) * 1.15
  camera.position.copy(new Vector3(1, 0.75, 1.25).normalize().multiplyScalar(dist))
  scene.add(camera)

  // Sky/ground fill plus a key light that rides with the camera.
  scene.add(new HemisphereLight(0xffffff, 0x3a3d44, 1.6))
  const key = new DirectionalLight(0xffffff, 2.2)
  key.position.set(1, 2, 3)
  camera.add(key)

  const controls = new OrbitControls(camera, canvas)
  controls.enableDamping = true
  controls.dampingFactor = 0.12
  controls.screenSpacePanning = true
  controls.zoomToCursor = true
  controls.listenToKeyEvents(canvas)
  controls.minDistance = r * 0.05
  controls.maxDistance = r * 20
  controls.target.set(0, 0, 0)
  controls.update()
  controls.saveState()

  let dirty = true
  let raf = 0
  const onChange = () => {
    dirty = true
  }
  controls.addEventListener('change', onChange)
  const tick = () => {
    raf = requestAnimationFrame(tick)
    const moving = controls.update()
    if (moving || dirty) {
      renderer.render(scene, camera)
      dirty = false
    }
  }

  const resize = () => {
    const w = host.clientWidth
    const h = host.clientHeight
    if (!w || !h) return
    renderer.setSize(w, h, false)
    camera.aspect = w / h
    camera.updateProjectionMatrix()
    dirty = true
  }
  resize()
  const ro = typeof ResizeObserver !== 'undefined' ? new ResizeObserver(resize) : null
  ro?.observe(host)

  const reset = () => {
    controls.reset()
    dirty = true
  }

  // WebKit trackpad pinch must not page-zoom under the canvas; the
  // context menu would swallow right-drag pans.
  const stop = (e: Event) => e.preventDefault()
  canvas.addEventListener('gesturestart', stop)
  canvas.addEventListener('contextmenu', stop)
  canvas.addEventListener('dblclick', reset)
  const onLost = (e: Event) => e.preventDefault()
  canvas.addEventListener('webglcontextlost', onLost)

  raf = requestAnimationFrame(tick)

  let disposed = false
  return {
    reset,
    dispose() {
      if (disposed) return
      disposed = true
      cancelAnimationFrame(raf)
      ro?.disconnect()
      canvas.removeEventListener('gesturestart', stop)
      canvas.removeEventListener('contextmenu', stop)
      canvas.removeEventListener('dblclick', reset)
      canvas.removeEventListener('webglcontextlost', onLost)
      controls.removeEventListener('change', onChange)
      controls.dispose()
      disposeObject(scene)
      scene.clear()
      renderer.renderLists.dispose()
      renderer.dispose()
      // Hand the GL context back now, not at GC: WebKit caps live contexts.
      renderer.forceContextLoss()
      canvas.remove()
    },
  }
}
