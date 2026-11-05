// TEST ONLY: an encoded-frame transform like an E2EE worker's (XOR on both
// ends), counting frames per side.
let frames = 0
self.onrtctransform = (event) => {
  const { readable, writable, options } = event.transformer
  readable
    .pipeThrough(
      new TransformStream({
        transform(frame, controller) {
          const data = new Uint8Array(frame.data)
          for (let i = 0; i < data.length; i++) data[i] ^= 0x5a
          frame.data = data.buffer
          frames++
          if (frames % 10 === 0) self.postMessage({ side: options.side, frames })
          controller.enqueue(frame)
        },
      }),
    )
    .pipeTo(writable)
}
