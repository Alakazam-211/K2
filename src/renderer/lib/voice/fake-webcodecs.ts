// A stand-in for the webview's WebCodecs, for vitest (node and jsdom have none).
// It decodes nothing: each decoded chunk becomes a `FakeVideoFrame` of the size
// the test says, and every call is recorded so a test can check the decoder was
// configured, reset and fed as the plugin's frames say.

import type { FrameSurface, WebCodecsEnv } from './video-decoder'

export class FakeEncodedVideoChunk {
  readonly type: 'key' | 'delta'
  readonly timestamp: number
  readonly data: Uint8Array
  constructor(init: { type: 'key' | 'delta'; timestamp: number; data: BufferSource }) {
    this.type = init.type
    this.timestamp = init.timestamp
    const d = init.data
    this.data = d instanceof Uint8Array ? new Uint8Array(d) : ArrayBuffer.isView(d) ? new Uint8Array(d.buffer.slice(d.byteOffset, d.byteOffset + d.byteLength)) : new Uint8Array(d)
  }
}

export class FakeVideoFrame {
  static open = 0
  readonly displayWidth: number
  readonly displayHeight: number
  readonly format: string | null
  readonly timestamp: number
  closed = false
  constructor(_data: unknown, init: { format?: string; codedWidth?: number; codedHeight?: number; timestamp: number }) {
    this.displayWidth = init.codedWidth ?? 0
    this.displayHeight = init.codedHeight ?? 0
    this.format = init.format ?? null
    this.timestamp = init.timestamp
    FakeVideoFrame.open += 1
  }
  close(): void {
    if (this.closed) return
    this.closed = true
    FakeVideoFrame.open -= 1
  }
}

type DecoderState = 'unconfigured' | 'configured' | 'closed'

export class FakeVideoDecoder {
  static instances: FakeVideoDecoder[] = []
  /** What `isConfigSupported` says yes to (`vp8`, `avc1…`). */
  static supported = new Set(['vp8', 'avc1'])
  /** Each decoded chunk comes out as a frame of this size at once. */
  static output = { width: 640, height: 360, auto: true }

  state: DecoderState = 'unconfigured'
  decodeQueueSize = 0
  readonly configs: VideoDecoderConfig[] = []
  readonly chunks: FakeEncodedVideoChunk[] = []
  resets = 0
  private readonly init: VideoDecoderInit

  constructor(init: VideoDecoderInit) {
    this.init = init
    FakeVideoDecoder.instances.push(this)
  }

  static async isConfigSupported(config: VideoDecoderConfig): Promise<VideoDecoderSupport> {
    const family = config.codec.split('.')[0]
    return { supported: FakeVideoDecoder.supported.has(family), config }
  }

  static reset(): void {
    FakeVideoDecoder.instances = []
    FakeVideoDecoder.supported = new Set(['vp8', 'avc1'])
    FakeVideoDecoder.output = { width: 640, height: 360, auto: true }
    FakeVideoFrame.open = 0
  }

  configure(config: VideoDecoderConfig): void {
    if (this.state === 'closed') throw new Error('InvalidStateError: closed')
    this.configs.push(config)
    this.state = 'configured'
  }

  decode(chunk: FakeEncodedVideoChunk): void {
    if (this.state !== 'configured') throw new Error(`InvalidStateError: decode while ${this.state}`)
    this.chunks.push(chunk)
    if (FakeVideoDecoder.output.auto) this.emit(chunk.timestamp)
  }

  /** One decoded picture out. */
  emit(timestamp = 0): void {
    const { width, height } = FakeVideoDecoder.output
    this.init.output(new FakeVideoFrame(null, { codedWidth: width, codedHeight: height, timestamp }) as unknown as VideoFrame)
  }

  reset(): void {
    if (this.state === 'closed') throw new Error('InvalidStateError: closed')
    this.resets += 1
    this.state = 'unconfigured'
  }

  close(): void {
    this.state = 'closed'
  }

  /** What a real decoder does on a bad stream: closes itself, then calls `error`. */
  fail(message = 'EncodingError: bad bitstream'): void {
    this.state = 'closed'
    this.init.error(new Error(message) as DOMException)
  }
}

export function fakeWebCodecs(): WebCodecsEnv {
  return {
    VideoDecoder: FakeVideoDecoder as unknown as typeof VideoDecoder,
    EncodedVideoChunk: FakeEncodedVideoChunk as unknown as typeof EncodedVideoChunk,
    VideoFrame: FakeVideoFrame as unknown as typeof VideoFrame,
  }
}

/** A surface that remembers what it drew. */
export class RecordingSurface implements FrameSurface {
  readonly drawn: Array<{ kind: 'frame' | 'rgba'; width: number; height: number }> = []
  draw(frame: VideoFrame): void {
    this.drawn.push({ kind: 'frame', width: frame.displayWidth, height: frame.displayHeight })
  }
  drawRgba(rgba: Uint8ClampedArray<ArrayBuffer>, width: number, height: number): void {
    if (rgba.length !== width * height * 4) throw new Error(`rgba: ${rgba.length} bytes for ${width}x${height}`)
    this.drawn.push({ kind: 'rgba', width, height })
  }
}
