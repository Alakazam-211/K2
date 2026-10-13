// The agent look a Garden widget may get: the same rules as the daemon's App
// surface (`k2_core::workspace::agent_look::{normalize_color,
// guest_image_view}`).
import { describe, expect, it } from 'vitest'
import { AGENT_LOOK_MAX_IMAGE_BYTES, agentLookColor, agentLookImage, agentLookTextOn } from './agent-look'

const b64 = (bytes: number[]): string => Buffer.from(bytes).toString('base64')
const padTo = (head: number[], n: number): number[] => [...head, ...new Array(Math.max(0, n - head.length)).fill(0)]

const PNG = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]
const JPEG = [0xff, 0xd8, 0xff, 0xe0]
const WEBP = [0x52, 0x49, 0x46, 0x46, 0, 0, 0, 0, 0x57, 0x45, 0x42, 0x50]
const GIF = [0x47, 0x49, 0x46, 0x38, 0x39, 0x61]

describe('agentLookColor', () => {
  it('#rgb and #rrggbb (any case, with or without #) become lowercase #rrggbb', () => {
    expect(agentLookColor('#C26')).toBe('#cc2266')
    expect(agentLookColor('3366AA')).toBe('#3366aa')
    expect(agentLookColor('  #abcdef ')).toBe('#abcdef')
  })

  it('anything else is null', () => {
    for (const bad of ['red', '#12345', '#ggg', 'rgb(1,2,3)', '', null, undefined]) {
      expect(agentLookColor(bad as string | null | undefined), String(bad)).toBeNull()
    }
  })
})

describe('agentLookImage', () => {
  it('passes PNG, JPEG, WebP and GIF data URLs whose bytes match their type', () => {
    const cases: Array<[string, number[]]> = [
      ['image/png', PNG],
      ['image/jpeg', JPEG],
      ['image/jpg', JPEG],
      ['image/webp', WEBP],
      ['image/gif', GIF],
    ]
    for (const [mime, head] of cases) {
      const url = `data:${mime};base64,${b64(padTo(head, 24))}`
      expect(agentLookImage(url), mime).toBe(url)
    }
  })

  it('refuses SVG, URLs, lying bytes, bad base64 and anything over 256 KiB', () => {
    const svg = `data:image/svg+xml;base64,${btoa('<svg xmlns="http://www.w3.org/2000/svg"></svg>')}`
    const lying = `data:image/png;base64,${b64(padTo(GIF, 24))}`
    const notB64 = 'data:image/png;base64,iVBO!!!!'
    const plain = `data:image/png,${b64(padTo(PNG, 24))}`
    const big = `data:image/png;base64,${b64(padTo(PNG, AGENT_LOOK_MAX_IMAGE_BYTES + 3))}`
    const atLimit = `data:image/png;base64,${b64(padTo(PNG, AGENT_LOOK_MAX_IMAGE_BYTES))}`
    for (const bad of [svg, lying, notB64, plain, big, 'http://127.0.0.1:38472/x.png', 'file:///x.png', '', null]) {
      expect(agentLookImage(bad), String(bad).slice(0, 40)).toBeNull()
    }
    expect(agentLookImage(atLimit)).toBe(atLimit)
  })
})

describe('agentLookTextOn', () => {
  it('white on dark colors, near-black on light ones', () => {
    expect(agentLookTextOn('#cc2266')).toBe('#ffffff')
    expect(agentLookTextOn('#111111')).toBe('#ffffff')
    expect(agentLookTextOn('#f5f5a0')).toBe('#111111')
    expect(agentLookTextOn('#ffffff')).toBe('#111111')
  })
})
