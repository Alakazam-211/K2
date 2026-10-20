// 0.46.3: the frame shape the renderer makes (`frameSrcDoc` +
// `zenFramePrelude`) and the one the daemon makes for `k2 zen widget preview
// --shot` (crates/k2-core/src/zen/widget_shot.rs) must be the same. Both
// sides build the same fixed frame and compare it with
// src/shared/zen-frame-shape.golden.html, where the two big generated texts
// (the sealed-frame guard and k2-frame.js) stand as {{GUARD}} and
// {{K2FRAME}}. The last line pins ZEN_FRAME_FEATURES (ZenCustomFrame.tsx).

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { frameSrcDoc } from '../frame-csp'
import { SEALED_FRAME_GUARD } from '../sealed-frame-guard'
import { k2FrameScript, zenFramePrelude, zenScriptText } from './zen-custom-prelude'

const root = resolve(__dirname, '../../../..')
const golden = readFileSync(resolve(root, 'src/shared/zen-frame-shape.golden.html'), 'utf8')

/** The fixed frame both sides build. */
export function zenFrameShapeFixture(): string {
  const nonce = 'AAAAAAAAAAAAAAAAAAAAAA'
  const prelude = zenFramePrelude({ nonce, theme: { vars: { '--zen-canvas': '#222222', '--zen-text': '#111111' }, scheme: 'dark' }, libs: ['LIB_ONE', 'x</script>y'] })
  const html = '<!DOCTYPE html>\n<html><body><p>shape</p><script nonce="AAAAAAAAAAAAAAAAAAAAAA">k2.ready()</script></body></html>'
  return frameSrcDoc(html, 'widget', { nonce, prelude })
    .split(zenScriptText(SEALED_FRAME_GUARD))
    .join('{{GUARD}}')
    .split(zenScriptText(k2FrameScript()))
    .join('{{K2FRAME}}')
}

function featuresFromSource(): string[] {
  const src = readFileSync(resolve(root, 'src/renderer/components/Zen/widgets/ZenCustomFrame.tsx'), 'utf8')
  const m = /export const ZEN_FRAME_FEATURES: readonly string\[\] = \[([\s\S]*?)\]/.exec(src)
  if (!m) throw new Error('ZEN_FRAME_FEATURES not found in ZenCustomFrame.tsx')
  return [...m[1].matchAll(/'([^']+)'/g)].map((x) => x[1])
}

describe('the widget frame shape is pinned for the daemon’s --shot', () => {
  it('frameSrcDoc + zenFramePrelude make the golden frame', () => {
    const [frame, features] = golden.replace(/\n$/, '').split('\n<!-- features: ')
    expect(zenFrameShapeFixture()).toBe(frame)
    expect(features.replace(/ -->$/, '').split(',')).toEqual(featuresFromSource())
  })
})
