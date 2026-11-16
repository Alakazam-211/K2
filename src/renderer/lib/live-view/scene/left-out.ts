// What the scene's picture can't show, in plain words (k2-mpc-blend
// docs/WIRE.md at 3a0c5cee): the cameras the mesh read leaves out
// (`cameras_left_out`, each with a fixed reason word), and why a view
// couldn't be drawn (the mesh read's refusal codes and the preview route's
// failure codes). No code ever shows; the words say what it means.

import { PRODUCT_NAME } from '@/lib/brand'

/** A camera the mesh read left out: its name, its address when it has a live key, and why. */
export interface CameraLeftOut {
  name: string
  key: string | null
  reason: string
}

/** Why, after the camera's name ("Camera.003: it isn't a perspective camera"). */
export const LEFT_OUT_WHY: Record<string, string> = {
  camera_not_perspective: 'it isn’t a perspective camera',
  camera_sensor_fit_unknown: 'its sensor fit isn’t known',
  camera_no_data: 'it has no camera data',
  camera_no_lens: 'its lens is missing',
  camera_no_clip: 'its clip distance is missing',
  camera_no_aspect: 'the scene’s render size gives it no shape',
  camera_degenerate: 'it doesn’t point anywhere',
  camera_bad_fov: 'its field of view can’t be drawn',
  linked_object: 'it’s linked from another file',
  transform_unplaced: 'it isn’t placed',
  transform_not_finite: 'its placement can’t be read',
  delta_transform: `it has a delta transform ${PRODUCT_NAME} can’t place`,
  parent_not_object: 'its parent isn’t an object',
  parent_unlisted: 'its parent isn’t in the scene',
}

/** The quiet line under the cameras (every one left out, the unnamed `more` too) and each named camera's why,
 *  then "…and N more"; null when none was left out. */
export function camerasLeftOutWords(list: readonly CameraLeftOut[], more = 0): { line: string; details: string[] } | null {
  const n = list.length + more
  if (!n) return null
  const named = list.map((c) => `${c.name}: ${LEFT_OUT_WHY[c.reason] ?? `${PRODUCT_NAME} can’t place it`}`)
  return {
    line: n === 1 ? '1 camera can’t be shown here' : `${n} cameras can’t be shown here`,
    details: named.length && more ? [...named, `…and ${more} more`] : named,
  }
}

const DIDNT_FINISH = 'The picture didn’t finish.'

/** Why a view couldn't be drawn: the mesh read's refusals and the route's failures. */
const FAILURE: Record<string, string> = {
  too_large: 'This scene is too large to draw here.',
  not_blend: `${PRODUCT_NAME} couldn’t read this file as a 3D scene.`,
  unknown_form: `${PRODUCT_NAME} couldn’t read this file as a 3D scene.`,
  unreadable: `${PRODUCT_NAME} couldn’t read this file as a 3D scene.`,
  live_version: `This file was saved by a version ${PRODUCT_NAME} can’t read yet.`,
  live_ambiguous: `Part of this file points somewhere ${PRODUCT_NAME} can’t follow.`,
  pairing_mismatch: 'This view doesn’t match the file’s saved version.',
  invalid: `${PRODUCT_NAME} couldn’t read this part of the file.`,
  timeout: 'The picture took too long to draw.',
  renderer_error: DIDNT_FINISH,
  render_error: DIDNT_FINISH,
  unsupported_part: `${PRODUCT_NAME} can’t draw this part of the file yet.`,
  not_on_this_host: 'This computer can’t draw this picture.',
}

export const failureWords = (code: string | null | undefined): string => (code ? (FAILURE[code] ?? DIDNT_FINISH) : DIDNT_FINISH)
