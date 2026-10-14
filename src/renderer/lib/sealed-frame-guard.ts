// The sealed-frame guard as text (src/shared/sealed-frame-guard.js): no
// WebRTC, link resource hints or `<a ping>` in K2's subframes, which never
// need them. The desktop app also registers the same file as an all-frames
// init script (src-tauri/src/k2_app_window.rs); the copies K2 puts inside a
// frame's own head (frameSrcDoc `scripted`, the widget prelude) are best
// effort for the web client, which has no app-level scripts.

import guardText from '../../shared/sealed-frame-guard.js?raw'

/** The guard script text. Safe inside `<script>…</script>` as-is. */
export const SEALED_FRAME_GUARD: string = guardText

if (/<\/script/i.test(SEALED_FRAME_GUARD)) {
  throw new Error('sealed-frame-guard: the script text must not contain </script')
}
