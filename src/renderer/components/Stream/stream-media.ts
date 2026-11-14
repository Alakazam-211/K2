// Seams for players that read media files (MP4 segments over HTTP) rather
// than the live stream: fetch with a byte range, a clock, and WebCodecs'
// audio decoder. Types only; nothing here runs. Real code passes the
// webview's `fetch`, `performance.now` / `setTimeout` and `AudioDecoder`
// behind these shapes; tests pass `testing.ts`'s fakes (`fakeRangeFetch`,
// `fakeClock`, `fakeAudioCodecs`).
//
// Pure: no DOM, no React, no `window`.

// ── Fetch ───────────────────────────────────────────────────────────────

/** The parts of a fetch `Response` a segment reader uses. */
export interface FetchResponseLike {
  readonly ok: boolean
  /** 200 (whole file), 206 (the range asked for), 404, 416 (range past the end), ... */
  readonly status: number
  readonly headers: { get(name: string): string | null }
  arrayBuffer(): Promise<ArrayBuffer>
}

/** The parts of a fetch request init a segment reader sends. A range is
 *  asked with `headers: { Range: 'bytes=<first>-<last>' }` (inclusive, as
 *  HTTP has it). */
export interface FetchInitLike {
  headers?: Record<string, string>
  signal?: AbortSignal
}

/** `fetch`, as far as a segment reader needs. */
export type FetchLike = (url: string, init?: FetchInitLike) => Promise<FetchResponseLike>

// ── Clock ───────────────────────────────────────────────────────────────

/** Wall time and timers, in ms. A media clock (pts-driven) is built on top
 *  of this; injecting it lets tests step time by hand. */
export interface ClockLike {
  now(): number
  setTimeout(fn: () => void, ms: number): number
  clearTimeout(id: number): void
}

// ── Audio (WebCodecs `AudioDecoder`) ────────────────────────────────────

/** A WebCodecs `AudioSampleFormat` (`f32-planar`, `s16`, ...). */
export type AudioSampleFormatLike = string

/** The parts of a WebCodecs `AudioData` we use. */
export interface AudioDataLike {
  /** µs. */
  readonly timestamp: number
  /** µs. */
  readonly duration: number
  readonly sampleRate: number
  readonly numberOfFrames: number
  readonly numberOfChannels: number
  readonly format: AudioSampleFormatLike | null
  allocationSize(options: { planeIndex: number; format?: AudioSampleFormatLike }): number
  copyTo(destination: ArrayBuffer | ArrayBufferView, options: { planeIndex: number; format?: AudioSampleFormatLike }): void
  close(): void
}

export interface AudioDecoderConfigLike {
  /** e.g. `opus`. */
  codec: string
  sampleRate: number
  numberOfChannels: number
  /** Codec setup out of band (Opus: the `dOps` body as an `OpusHead`). */
  description?: ArrayBuffer | ArrayBufferView
}

export interface AudioDecoderInitLike {
  output(data: AudioDataLike): void
  error(err: unknown): void
}

export interface AudioDecoderLike {
  readonly state: string
  readonly decodeQueueSize?: number
  configure(config: AudioDecoderConfigLike): void
  decode(chunk: unknown): void
  /** Resolves once every chunk handed in has been output. */
  flush(): Promise<void>
  reset(): void
  close(): void
}

export interface EncodedAudioChunkInitLike {
  type: 'key' | 'delta'
  /** µs. */
  timestamp: number
  duration?: number
  data: Uint8Array
}

/** WebCodecs' audio side, or a fake of it (tests). */
export interface AudioCodecsLike {
  createDecoder(init: AudioDecoderInitLike): AudioDecoderLike
  createChunk(init: EncodedAudioChunkInitLike): unknown
  isConfigSupported(config: AudioDecoderConfigLike): Promise<boolean>
}
