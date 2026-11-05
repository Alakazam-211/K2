// TEST ONLY: a stand-in for a noise filter in an AudioWorklet, with its
// WebAssembly compiled inside the worklet (as kits that ship a .wasm do).
class K2NoiseCheck extends AudioWorkletProcessor {
  constructor(options) {
    super()
    this.frames = 0
    this.report = {}
    try {
      const instance = new WebAssembly.Instance(new WebAssembly.Module(options.processorOptions.bytes))
      this.report.wasmInWorklet = instance.exports.add(20, 22)
    } catch (e) {
      this.report.wasmInWorklet = `${e.name}: ${e.message}`
    }
    this.port.onmessage = (e) => {
      try {
        this.report.moduleInWorklet = new WebAssembly.Instance(e.data.module).exports.add(1, 2)
      } catch (err) {
        this.report.moduleInWorklet = `${err.name}: ${err.message}`
      }
      this.port.postMessage(this.report)
    }
  }

  process(inputs, outputs) {
    this.frames++
    const input = inputs[0] && inputs[0][0]
    const output = outputs[0] && outputs[0][0]
    if (input && output) for (let i = 0; i < output.length; i++) output[i] = input[i] * 0.5
    if (this.frames % 16 === 0) this.port.postMessage({ ...this.report, frames: this.frames })
    return true
  }
}

registerProcessor('k2-noise-check', K2NoiseCheck)
