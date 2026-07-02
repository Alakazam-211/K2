import { describe, expect, it } from 'vitest'

import { decodeGridFrame } from './gridWire'

// Cross-language fixture: the hex and JSON constants below are the
// OUTPUT of the Rust encoder for the canonical nontrivial fixture
// (unicode incl. emoji + CJK, explicit colors, every style flag,
// wrapped runs, empty rows, mode bits). Regenerate with:
//
//   cargo test -p k2-core --lib \
//     grid_wire::tests::fixture_hex_and_json_dump -- --nocapture
//
// The JSON constant is serde's own serialization of the same value,
// so this test pins the parity contract end to end: Rust encode →
// TS decode → deep-equal what JSON.parse of the JSON frame yields.

const SNAPSHOT_HEX =
  '6b0101070070616e652dcf800c000400efbeadde0000000003000000020005000105040000000200070068c3a96c6c6f200088ff00ffffffff010a00f09f908de4b8ade69687ffffffffff000000c606000000010001007e00000000ffffff0038010004007461696cffffffffffffffff00020000000000010007006f6c6420726f77ffffffffffffffff00'

const SNAPSHOT_JSON =
  '{"paneId":"pane-π","cols":12,"rows":4,"grid":[[{"text":"héllo ","fg":16746496,"bg":null,"bold":true,"italic":false,"underline":false,"inverse":false,"dim":false,"strikeout":false},{"text":"🐍中文","fg":null,"bg":255,"bold":false,"italic":true,"underline":true,"inverse":false,"dim":false,"strikeout":false,"wrapped":true,"cols":6}],[],[{"text":"~","fg":0,"bg":16777215,"bold":false,"italic":false,"underline":false,"inverse":true,"dim":true,"strikeout":true}],[{"text":"tail","fg":null,"bg":null,"bold":false,"italic":false,"underline":false,"inverse":false,"dim":false,"strikeout":false}]],"scrollback":[[],[{"text":"old row","fg":null,"bg":null,"bold":false,"italic":false,"underline":false,"inverse":false,"dim":false,"strikeout":false}]],"cursor":{"row":2,"col":5,"visible":true},"version":3735928559,"displayOffset":3,"mouseReport":true,"sgrMouse":false,"altScreen":true}'

const DELTA_HEX =
  '6b0102070070616e652dcf800c000400f0beadde0000000000000000010004000002000000010001000500ce94726f7700ff0000ffffffff00030000000100000001000c007363726f6c6c656420e29ca8ffffffffffffffff00'

const DELTA_JSON =
  '{"paneId":"pane-π","cols":12,"rows":4,"damagedRows":[{"row":1,"runs":[{"text":"Δrow","fg":65280,"bg":null,"bold":false,"italic":false,"underline":false,"inverse":false,"dim":false,"strikeout":false}]},{"row":3,"runs":[]}],"scrollbackAppended":[[{"text":"scrolled ✨","fg":null,"bg":null,"bold":false,"italic":false,"underline":false,"inverse":false,"dim":false,"strikeout":false}]],"cursor":{"row":1,"col":4,"visible":false},"version":3735928560,"displayOffset":0}'

function hexToArrayBuffer(hex: string): ArrayBuffer {
  const bytes = new Uint8Array(hex.length / 2)
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16)
  }
  return bytes.buffer
}

describe('gridWire k1 decoder', () => {
  it('decodes the Rust-encoded snapshot fixture to the exact serde JSON object', () => {
    const frame = decodeGridFrame(hexToArrayBuffer(SNAPSHOT_HEX))
    expect(frame.kind).toBe('snapshot')
    // toStrictEqual: key ABSENCE matters — a non-wrapped run must not
    // carry a `wrapped` key, matching serde's skip_serializing_if.
    expect(frame.payload).toStrictEqual(JSON.parse(SNAPSHOT_JSON))
  })

  it('decodes the Rust-encoded delta fixture to the exact serde JSON object', () => {
    const frame = decodeGridFrame(hexToArrayBuffer(DELTA_HEX))
    expect(frame.kind).toBe('delta')
    expect(frame.payload).toStrictEqual(JSON.parse(DELTA_JSON))
  })

  it('omits the wrapped key on non-wrapped runs and sets it on wrapped ones', () => {
    const frame = decodeGridFrame(hexToArrayBuffer(SNAPSHOT_HEX))
    if (frame.kind !== 'snapshot') throw new Error('expected snapshot')
    const [first, second] = frame.payload.grid[0]
    expect(Object.prototype.hasOwnProperty.call(first, 'wrapped')).toBe(false)
    expect(second.wrapped).toBe(true)
  })

  it('omits the cols key on single-width runs and decodes the trailing u16 on wide ones', () => {
    const frame = decodeGridFrame(hexToArrayBuffer(SNAPSHOT_HEX))
    if (frame.kind !== 'snapshot') throw new Error('expected snapshot')
    const [first, second] = frame.payload.grid[0]
    expect(Object.prototype.hasOwnProperty.call(first, 'cols')).toBe(false)
    // "\u{1F40D}\u4E2D\u6587" = 3 chars spanning 6 terminal columns.
    expect(second.cols).toBe(6)
  })

  it('rejects bad magic, unsupported version, unknown kind and truncation', () => {
    expect(() =>
      decodeGridFrame(new Uint8Array([0x00, 0x01, 0x01]).buffer),
    ).toThrow(/bad magic/)
    expect(() =>
      decodeGridFrame(new Uint8Array([0x6b, 0x02, 0x01]).buffer),
    ).toThrow(/unsupported wire version/)
    expect(() =>
      decodeGridFrame(new Uint8Array([0x6b, 0x01, 0x09]).buffer),
    ).toThrow(/unknown frame kind/)
    const full = new Uint8Array(hexToArrayBuffer(SNAPSHOT_HEX))
    expect(() =>
      decodeGridFrame(full.slice(0, full.length - 1).buffer),
    ).toThrow(/truncated/)
    expect(() => decodeGridFrame(new ArrayBuffer(0))).toThrow(/truncated/)
  })
})
