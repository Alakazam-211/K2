import { describe, expect, it } from 'vitest'

import { decodeGridFrame } from './gridWire'
import {
  ackVersionAfterApply,
  decodeK1QueuedBatch,
  queueK1Binary,
} from './k1FramePipeline'

// Same Rust-encoded fixtures as gridWire.test.ts. Queue must not
// require a full decode; apply/ack happens after the batch.

const SNAPSHOT_HEX =
  '6b0101070070616e652dcf800c000400efbeadde0000000003000000020005000105040000000200070068c3a96c6c6f200088ff00ffffffff010a00f09f908de4b8ade69687ffffffffff000000c606000000010001007e00000000ffffff0038010004007461696cffffffffffffffff00020000000000010007006f6c6420726f77ffffffffffffffff00'

const DELTA_HEX =
  '6b0102070070616e652dcf800c000400f0beadde0000000000000000010004000002000000010001000500ce94726f7700ff0000ffffffff00030000000100000001000c007363726f6c6c656420e29ca8ffffffffffffffff00'

function hexToArrayBuffer(hex: string): ArrayBuffer {
  const bytes = new Uint8Array(hex.length / 2)
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16)
  }
  return bytes.buffer
}

describe('k1 decode/enqueue/ack-after-apply', () => {
  it('enqueues the ArrayBuffer after a header peek, without decoding the grid', () => {
    const buf = hexToArrayBuffer(SNAPSHOT_HEX)
    const queued = queueK1Binary(buf)
    expect(queued.kind).toBe('snapshot')
    expect(queued.buf).toBe(buf)
    // Header-only buffer: peek/queue succeeds; full decode is deferred
    // and must fail — proving enqueue did not walk the body.
    const headerOnly = buf.slice(0, 3)
    const headerQueued = queueK1Binary(headerOnly)
    expect(headerQueued.kind).toBe('snapshot')
    expect(() => decodeK1QueuedBatch([headerQueued])).toThrow(/truncated/)
  })

  it('decodes on apply and acks the highest version only after the last chunk', () => {
    const snapBuf = hexToArrayBuffer(SNAPSHOT_HEX)
    const deltaBuf = hexToArrayBuffer(DELTA_HEX)
    const queued = [queueK1Binary(snapBuf), queueK1Binary(deltaBuf)]
    // Nothing applied at enqueue — ack floor stays 0.
    expect(ackVersionAfterApply([])).toBe(0)

    const applied = decodeK1QueuedBatch(queued)
    expect(applied).toHaveLength(2)
    expect(applied[0].kind).toBe('snapshot')
    expect(applied[1].kind).toBe('delta')
    expect(applied[0].payload).toStrictEqual(decodeGridFrame(snapBuf).payload)
    expect(applied[1].payload).toStrictEqual(decodeGridFrame(deltaBuf).payload)

    const ack = ackVersionAfterApply(applied)
    expect(ack).toBe(applied[1].payload.version)
    expect(ack).toBeGreaterThan(applied[0].payload.version)
  })

  it('acks the snapshot version when the coalescer superseded earlier deltas', () => {
    // Coalescer drops frames before a snapshot; apply sees only the snap.
    const snap = queueK1Binary(hexToArrayBuffer(SNAPSHOT_HEX))
    const applied = decodeK1QueuedBatch([snap])
    expect(applied).toHaveLength(1)
    expect(applied[0].kind).toBe('snapshot')
    expect(ackVersionAfterApply(applied)).toBe(applied[0].payload.version)
  })

  it('does not ack a failed decode (no floor bump on a skipped chunk)', () => {
    const good = queueK1Binary(hexToArrayBuffer(DELTA_HEX))
    const applied = decodeK1QueuedBatch([good])
    expect(ackVersionAfterApply(applied)).toBe(applied[0].payload.version)
    expect(() =>
      decodeK1QueuedBatch([queueK1Binary(new Uint8Array([0x6b, 0x01, 0x01]).buffer)]),
    ).toThrow(/truncated/)
  })
})
