// k2-simhelper — K2's iOS Simulator helper (L-I lane, prd-mobile-dev-v1 §3.3).
//
// One process per booted device, spawned by the K2 daemon as a plain child
// (M-A15). It talks ONLY over its own stdin/stdout pipes: no socket, no port,
// so nothing else on the machine can reach it (stricter than M-A16's
// loopback + random token).
//
//   k2-simhelper --set <device-set-path> --udid <UDID> [--frames-fd <n>]
//
// stdin : one JSON request per line  {"id":1,"op":"tree"}
//         "nr": true (protocol 3) = no reply on success; a failure is sent as an
//         event instead: {"event":"input_error","op":…,"code":…,"error":…}
//         (the daemon sends a touch's down and moves this way)
// stdout: framed messages  [u32 BE length][u8 kind][payload]   (length counts kind + payload)
//         kind 'J' = UTF-8 JSON (replies carry the request id; events carry "event")
// fd <n>: with --frames-fd (protocol 3), the 'F' messages go there, framed the
//         same way and under their own lock, so a reply never queues behind a
//         frame; without it they share stdout
//         kind 'F' = frame: [u32 BE width][u32 BE height][u64 BE seq]
//                    [u64 BE capture ns][u64 BE emit ns] + JPEG bytes   (protocol 2)
//                    capture = the IOSurface callback the frame answers, emit = the
//                    encode finished; both ns of uptime, the clock `hello.clock_ns` reads
//         (the `frames` op). Protocol 4 (the `layers` op): kind 'F' = one unit, the
//         40-byte header of k2-simvideo's link.rs then an Annex B H.264 access unit
//         (SPS + PPS in front of every IDR) or a JPEG. Its first byte is the codec
//         (1-3); a protocol 3 frame's is a width's high byte (0), which is how the
//         daemon tells them apart on one pipe across a switch.
//         protocol 4 requests: {"op":"layers","layers":[{id,codec,max_width,fps,
//         bitrate,quality,on}]} (the whole list; one encode per layer),
//         {"op":"keyframe","layer":n,"since_ns":t}, {"op":"rate","layer":n,"fps":f,"bitrate":b}
// stderr: diagnostics only.
//
// It drives the Simulator through CoreSimulator + SimulatorKit + the private
// AccessibilityPlatformTranslation framework, loaded at runtime from the
// selected Xcode (nothing linked), and it finds the device in K2's OWN
// device set (SimServiceContext deviceSetWithPath:) — never the person's
// default Simulator set. AXe and serve-sim both hard-code the default set,
// which is why K2 carries its own helper (L-I spike, M-A14).
//
// Prior art, studied not copied: idb's FBSimulatorControl (MIT, Meta) and
// serve-sim (Apache-2.0, Evan Bacon) show which private selectors exist.
//
// The daemon compiles this file with the person's own Xcode on first use
// (`xcrun swiftc`) and caches the binary by source hash.

import Foundation
import AppKit
import IOSurface
import CoreGraphics
import ImageIO
import UniformTypeIdentifiers
import ObjectiveC
import VideoToolbox
import CoreMedia
import CoreVideo

/// 4: H.264 units from VideoToolbox (`layers`, `keyframe`, `rate`); `frames`
/// still sends protocol 3's JPEG frames.
let HELPER_VERSION = 4
let HELPER_ACCEPTS = [3, 4]

// MARK: - Output

enum Out {
    static let lock = NSLock()
    static let frameLock = NSLock()
    /// The frames pipe (--frames-fd), if the daemon gave one.
    static let frames: FileHandle? = {
        let a = CommandLine.arguments
        guard let i = a.firstIndex(of: "--frames-fd"), i + 1 < a.count, let fd = Int32(a[i + 1]), fd > 2 else { return nil }
        return FileHandle(fileDescriptor: fd, closeOnDealloc: false)
    }()
    static func framed(_ kind: UInt8, _ payload: Data) -> Data {
        var len = UInt32(payload.count + 1).bigEndian
        var d = Data(bytes: &len, count: 4)
        d.append(kind)
        d.append(payload)
        return d
    }
    static func send(_ kind: UInt8, _ payload: Data) {
        let d = framed(kind, payload)
        if kind == UInt8(ascii: "F"), let fh = frames {
            frameLock.lock(); defer { frameLock.unlock() }
            try? fh.write(contentsOf: d)   // the daemon gone: its stdin closing ends us
            return
        }
        lock.lock(); defer { lock.unlock() }
        FileHandle.standardOutput.write(d)
    }
    /// The lock 'F' messages are written under.
    static var unitLock: NSLock { frames != nil ? frameLock : lock }
    /// One protocol 4 unit: `make` runs under the frames lock (it takes the
    /// layer's next seq), so a layer's units leave in seq order, one write each.
    static func unit(_ make: () -> Data?) {
        let lk = unitLock
        lk.lock(); defer { lk.unlock() }
        guard let payload = make() else { return }
        let d = framed(UInt8(ascii: "F"), payload)
        if let fh = frames { try? fh.write(contentsOf: d) } else { FileHandle.standardOutput.write(d) }
    }
    static func json(_ obj: [String: Any]) {
        guard let d = try? JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys]) else {
            send(UInt8(ascii: "J"), Data("{\"ok\":false,\"error\":\"unserializable reply\"}".utf8)); return
        }
        send(UInt8(ascii: "J"), d)
    }
    static func log(_ s: String) { FileHandle.standardError.write(Data(("[k2-simhelper] " + s + "\n").utf8)) }
}

func now() -> Double { Double(DispatchTime.now().uptimeNanoseconds) / 1e9 }
func nowNs() -> UInt64 { DispatchTime.now().uptimeNanoseconds }

struct HelperError: Error { let code: String; let message: String }

// MARK: - Frameworks + device

func developerDir() -> String {
    if let d = ProcessInfo.processInfo.environment["DEVELOPER_DIR"], !d.isEmpty { return d }
    let p = Process(); let pipe = Pipe()
    p.executableURL = URL(fileURLWithPath: "/usr/bin/xcode-select"); p.arguments = ["-p"]
    p.standardOutput = pipe; p.standardError = FileHandle.nullDevice
    do { try p.run() } catch { return "/Applications/Xcode.app/Contents/Developer" }
    p.waitUntilExit()
    return String(data: pipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8)?
        .trimmingCharacters(in: .whitespacesAndNewlines) ?? "/Applications/Xcode.app/Contents/Developer"
}

let devDir = developerDir()

func loadFrameworks() {
    for path in ["/Library/Developer/PrivateFrameworks/CoreSimulator.framework/CoreSimulator",
                 "\(devDir)/Library/PrivateFrameworks/CoreSimulator.framework/CoreSimulator",
                 "\(devDir)/../SharedFrameworks/SimulatorKit.framework/SimulatorKit",          // Xcode 27+
                 "\(devDir)/Library/PrivateFrameworks/SimulatorKit.framework/SimulatorKit"] {    // Xcode 26 and older
        _ = dlopen(path, RTLD_NOW)
    }
}

/// "26.5 (17F42)" from the selected Xcode's Info.plist and version.plist.
func xcodeVersion() -> String {
    let contents = URL(fileURLWithPath: devDir).deletingLastPathComponent()
    let info = NSDictionary(contentsOf: contents.appendingPathComponent("Info.plist"))
    let ver = NSDictionary(contentsOf: contents.appendingPathComponent("version.plist"))
    let short = info?["CFBundleShortVersionString"] as? String ?? "unknown"
    let build = ver?["ProductBuildVersion"] as? String
    return build.map { "\(short) (\($0))" } ?? short
}

/// Everything the helper relies on, checked before any request: a missing
/// private class or symbol becomes one clear error naming the Xcode, never a
/// crash or a hang later.
func selfCheck() -> [String] {
    var missing: [String] = []
    for c in ["SimServiceContext", "_TtC12SimulatorKit24SimDeviceLegacyHIDClient", "AXPTranslator", "AXPTranslatorResponse"]
        where NSClassFromString(c) == nil { missing.append(c) }
    let h = UnsafeMutableRawPointer(bitPattern: -2)
    for s in ["IndigoHIDMessageForMouseNSEvent", "IndigoHIDMessageForKeyboardArbitrary", "IndigoHIDMessageForButton"]
        where dlsym(h, s) == nil { missing.append(s) }
    return missing
}

func findDevice(setPath: String, udid: String) throws -> NSObject {
    guard let ctxClass = NSClassFromString("SimServiceContext") as? NSObject.Type else {
        throw HelperError(code: "no_coresimulator", message: "CoreSimulator is not loadable (is Xcode installed?)")
    }
    guard let ctx = ctxClass.perform(NSSelectorFromString("sharedServiceContextForDeveloperDir:error:"), with: devDir, with: nil)?
        .takeUnretainedValue() as? NSObject else {
        throw HelperError(code: "no_service_context", message: "no CoreSimulator service context for \(devDir)")
    }
    guard let set = ctx.perform(NSSelectorFromString("deviceSetWithPath:error:"), with: setPath, with: nil)?
        .takeUnretainedValue() as? NSObject else {
        throw HelperError(code: "no_device_set", message: "cannot open device set \(setPath)")
    }
    let devices = set.value(forKey: "devices") as? [NSObject] ?? []
    guard let d = devices.first(where: { ($0.value(forKey: "UDID") as? NSUUID)?.uuidString == udid }) else {
        throw HelperError(code: "no_device", message: "device \(udid) is not in K2's device set")
    }
    return d
}

// MARK: - Screen geometry

struct Screen {
    let widthPt: Double, heightPt: Double, scale: Double
}

func screenOf(_ device: NSObject) -> Screen {
    var scale = 3.0, w = 0.0, h = 0.0
    if let type = device.value(forKey: "deviceType") as? NSObject {
        if let s = type.value(forKey: "mainScreenScale") as? NSNumber, s.doubleValue > 0 { scale = s.doubleValue }
        if let v = type.value(forKey: "mainScreenSize") as? NSValue { let sz = v.sizeValue; w = Double(sz.width); h = Double(sz.height) }
    }
    return Screen(widthPt: w / scale, heightPt: h / scale, scale: scale)
}

// MARK: - Frames (IOSurface callbacks, event-driven)

@objc protocol FramebufferDescriptor {
    @objc(registerScreenCallbacksWithUUID:callbackQueue:frameCallback:surfacesChangedCallback:propertiesChangedCallback:)
    func registerScreenCallbacks(uuid: UUID, callbackQueue: DispatchQueue,
                                 frameCallback: @convention(block) @escaping () -> Void,
                                 surfacesChangedCallback: @convention(block) @escaping () -> Void,
                                 propertiesChangedCallback: @convention(block) @escaping () -> Void)
}

final class Frames {
    let q = DispatchQueue(label: "k2.frames", qos: .userInteractive)
    private var descs: [NSObject] = []
    private let io: NSObject
    // Streaming state (only touched on q).
    private var streaming = false
    private var fps = 30.0
    private var scale = 1.0 / 3.0
    private var quality = 0.7
    private var lastEmit = 0.0
    /// When the next frame is due. It moves on by one interval from when the
    /// last frame was due, not from when that frame finished encoding, so
    /// encode time and timer lateness don't stretch the frame interval.
    private var nextDue = 0.0
    private var trailing = false
    private var seq: UInt64 = 0

    init(device: NSObject) throws {
        guard let io = device.perform(NSSelectorFromString("io"))?.takeUnretainedValue() as? NSObject else {
            throw HelperError(code: "no_io", message: "device has no IO client")
        }
        self.io = io
        try wire()
    }

    private func wire() throws {
        io.perform(NSSelectorFromString("updateIOPorts"))
        var found: [NSObject] = []
        for port in io.value(forKey: "deviceIOPorts") as? [NSObject] ?? [] {
            guard let pid = port.perform(NSSelectorFromString("portIdentifier"))?.takeUnretainedValue(),
                  "\(pid)" == "com.apple.framebuffer.display",
                  let d = port.perform(NSSelectorFromString("descriptor"))?.takeUnretainedValue() as? NSObject,
                  d.responds(to: NSSelectorFromString("framebufferSurface")) else { continue }
            found.append(d)
        }
        if found.isEmpty { throw HelperError(code: "no_framebuffer", message: "no framebuffer display port") }
        descs = found
        for d in found {
            let onFrame: @convention(block) () -> Void = { [weak self] in self?.frameArrived() }
            let onSurfaces: @convention(block) () -> Void = { [weak self] in self?.frameArrived() }
            let none: @convention(block) () -> Void = {}
            (d as AnyObject).registerScreenCallbacks(uuid: UUID(), callbackQueue: q, frameCallback: onFrame,
                                                     surfacesChangedCallback: onSurfaces, propertiesChangedCallback: none)
        }
    }

    /// The live main-screen surface: the largest one (secondary planes are smaller).
    func surface() -> IOSurface? {
        var best: IOSurface?; var area = 0
        for d in descs {
            guard let o = d.perform(NSSelectorFromString("framebufferSurface"))?.takeUnretainedValue() else { continue }
            let s = unsafeBitCast(o, to: IOSurface.self)
            let a = IOSurfaceGetWidth(s) * IOSurfaceGetHeight(s)
            if a > area { area = a; best = s }
        }
        return best
    }

    func configure(on: Bool, fps: Double, scale: Double, quality: Double) {
        q.sync {
            if on { for id in Array(self.layers.keys) { self.teardown(id) } }   // protocol 3 takes the pipe back
            self.streaming = on
            self.fps = max(1, min(60, fps))
            self.scale = max(0.1, min(1, scale))
            self.quality = max(0.3, min(0.95, quality))
            self.nextDue = 0   // the schedule restarts from this key frame
            if on { self.emit() }   // a key frame for the (re)joining viewer
        }
    }

    private var lastArrival = 0.0
    private var lastArrivalNs: UInt64 = 0

    private func frameArrived() {
        guard streaming || !layers.isEmpty else { return }
        lastArrivalNs = nowNs()
        lastArrival = Double(lastArrivalNs) / 1e9
        for l in layers.values { tick(l) }
        guard streaming else { return }
        if lastArrival >= nextDue {
            emit()
        } else {
            scheduleTrailing()
        }
    }

    /// Coalesce: deliver the last state after a burst of motion, but never a
    /// frame before it is due (a direct emit may have beaten the timer).
    private func scheduleTrailing() {
        guard !trailing else { return }
        trailing = true
        let wait = max(0.001, nextDue - now())
        q.asyncAfter(deadline: .now() + wait) { [weak self] in
            guard let self else { return }
            self.trailing = false
            guard self.streaming, self.lastArrival > self.lastEmit else { return }
            if now() >= self.nextDue { self.emit() } else { self.scheduleTrailing() }
        }
    }

    /// Moves the schedule on for a frame starting now. More than an interval
    /// behind (a still screen, a stall), it restarts from now instead of
    /// bursting to catch up. A frame ahead of schedule leaves it alone.
    private func advanceSchedule() {
        let t = now()
        guard t >= nextDue else { return }
        let interval = 1.0 / fps
        nextDue += interval
        if nextDue <= t { nextDue = t + interval }
    }

    private func emit() {
        advanceSchedule()
        // The screen this frame shows appeared at the last callback (a
        // (re)joining viewer's first frame may be older than that).
        let captured = lastArrivalNs != 0 ? lastArrivalNs : nowNs()
        guard let s = surface(), let img = Image.cgImage(s) else { return }
        guard let jpeg = Image.encode(img, scale: scale, type: UTType.jpeg, quality: quality, crop: nil) else { return }
        let emitted = nowNs()
        lastEmit = Double(emitted) / 1e9
        seq += 1
        var w = UInt32(jpeg.width).bigEndian, h = UInt32(jpeg.height).bigEndian, sq = seq.bigEndian
        var cap = captured.bigEndian, emt = emitted.bigEndian
        var payload = Data(bytes: &w, count: 4)
        payload.append(Data(bytes: &h, count: 4))
        payload.append(Data(bytes: &sq, count: 8))
        payload.append(Data(bytes: &cap, count: 8))
        payload.append(Data(bytes: &emt, count: 8))
        payload.append(jpeg.data)
        Out.send(UInt8(ascii: "F"), payload)
    }

    // MARK: Protocol 4: layers (all on q)

    private var layers: [UInt8: Layer] = [:]

    /// The whole layer list (`layers`): a layer missing from it or with
    /// `on: false` is torn down; a new one starts with a key unit of the
    /// screen as it is; a changed one is retuned in place (a new size makes
    /// the next unit a source start).
    func setLayers(_ specs: [LayerSpec]) {
        q.sync {
            self.streaming = false   // protocol 4 takes the pipe
            let want = Dictionary(uniqueKeysWithValues: specs.filter { $0.on }.map { ($0.id, $0) })
            for (id, l) in self.layers where want[id]?.codec != l.codec { self.teardown(id) }
            for spec in specs where spec.on {
                if let l = self.layers[spec.id] {
                    l.fps = spec.fps
                    if l.maxWidth != spec.maxWidth { l.maxWidth = spec.maxWidth; l.wantKey = true; l.keySince = 0 }
                    l.lq.async { l.retune(spec) }
                    self.tick(l)
                } else {
                    let l = Layer(spec)
                    self.layers[spec.id] = l
                    Out.log("layer \(spec.id): \(spec.codec) max_width \(spec.maxWidth) at \(Int(spec.fps)) fps, \(spec.bitrate) b/s")
                    self.tick(l)
                }
            }
        }
    }

    /// A keyframe on the first frame read at or after `since` (the request
    /// can't name a frame VideoToolbox already holds: the key is set on the
    /// very frame it is encoded with). A still screen gets one too.
    func keyframe(layer id: UInt8, since: UInt64) -> Bool {
        q.sync {
            guard let l = self.layers[id] else { return false }
            l.wantKey = true
            l.keySince = since
            self.tick(l)
            return true
        }
    }

    /// New rate targets, no keyframe.
    func rate(layer id: UInt8, fps: Double, bitrate: Int) -> Bool {
        q.sync {
            guard let l = self.layers[id] else { return false }
            l.fps = fps
            l.lq.async { l.setRates(fps: fps, bitrate: bitrate) }
            return true
        }
    }

    private func teardown(_ id: UInt8) {
        guard let l = layers.removeValue(forKey: id) else { return }
        Out.unitLock.lock(); l.live = false; Out.unitLock.unlock()   // no unit of it leaves after this
        l.lq.async { l.close() }
        Out.log("layer \(id): off")
    }

    /// A layer's frame is due on its own schedule (the same rule as `emit`'s).
    private func tick(_ l: Layer) {
        guard layers[l.id] === l, !l.busy else { return }   // busy: the encode's end ticks again
        let dirty = lastArrival > l.lastCapture
        let keyReady = l.wantKey && nowNs() >= l.keySince
        guard dirty || keyReady else {
            if l.wantKey { trail(l, until: Double(l.keySince) / 1e9) }   // a keyframe asked for later on a still screen
            return
        }
        if now() >= l.nextDue { capture(l) } else { trail(l, until: l.nextDue) }
    }

    private func trail(_ l: Layer, until: Double) {
        guard !l.trailing else { return }
        l.trailing = true
        q.asyncAfter(deadline: .now() + max(0.001, until - now())) { [weak self] in
            l.trailing = false
            self?.tick(l)
        }
    }

    private func capture(_ l: Layer) {
        guard let s = surface() else { return }
        let t = now()
        if t >= l.nextDue {
            l.nextDue += 1.0 / l.fps
            if l.nextDue <= t { l.nextDue = t + 1.0 / l.fps }
        }
        l.lastCapture = t
        let captured = lastArrivalNs != 0 ? lastArrivalNs : nowNs()
        let key = l.wantKey && nowNs() >= l.keySince
        if key { l.wantKey = false }
        l.busy = true
        l.lq.async { [weak self] in
            let ok = l.encode(s, captured: captured, key: key) {
                // A forced key or source start the encoder dropped: ask again.
                self?.q.async { l.wantKey = true; l.keySince = 0; self?.tick(l) }
            }
            self?.q.async {
                l.busy = false
                if !ok && key { l.wantKey = true; l.keySince = 0 }
                self?.tick(l)
            }
        }
    }
}

/// One layer the daemon asked for (protocol 4's `LayerSpec`).
struct LayerSpec {
    let id: UInt8, codec: String, maxWidth: Int, fps: Double, bitrate: Int, quality: Double, on: Bool

    static func parse(_ r: [String: Any], codecs: [String]) throws -> LayerSpec {
        guard let id = (r["id"] as? NSNumber)?.intValue, (0...255).contains(id) else { throw HelperError(code: "bad_request", message: "a layer needs an id 0-255") }
        guard let codec = r["codec"] as? String else { throw HelperError(code: "bad_request", message: "layer \(id) needs a codec") }
        let on = (r["on"] as? Bool) ?? true
        if on && !codecs.contains(codec) {
            throw HelperError(code: "bad_request", message: "layer \(id): codec \(codec) isn't available here (codecs: \(codecs.joined(separator: ", ")))")
        }
        guard let fps = (r["fps"] as? NSNumber)?.doubleValue else { throw HelperError(code: "bad_request", message: "layer \(id) needs fps") }
        let bitrate = (r["bitrate"] as? NSNumber)?.intValue ?? 0
        let quality = (r["quality"] as? NSNumber).map { max(0.05, min(1, $0.doubleValue / 100)) } ?? 0.7
        return LayerSpec(id: UInt8(id), codec: codec, maxWidth: max(0, (r["max_width"] as? NSNumber)?.intValue ?? 0),
                         fps: max(1, min(60, fps)), bitrate: bitrate > 0 ? bitrate : 6_000_000, quality: quality, on: on)
    }
}

/// One layer: its schedule (on `Frames.q`), its encoder (on its own queue,
/// so the IOSurface callback queue never waits on VideoToolbox) and its
/// output (under the frames lock).
final class Layer {
    let id: UInt8, codec: String
    let lq: DispatchQueue
    // Schedule: only on Frames.q.
    var fps: Double
    var maxWidth: Int
    var nextDue = 0.0, lastCapture = -1.0
    var trailing = false, busy = false
    var wantKey = true, keySince: UInt64 = 0
    // Output: only under Out.unitLock.
    var live = true
    static var seqs: [UInt8: UInt64] = [:]   // per layer id, never back (a restarted layer goes on)
    // Encoder: only on lq.
    private var spec: LayerSpec
    private var session: VTCompressionSession?
    private var transfer: VTPixelTransferSession?
    private var size = (0, 0)
    private var fresh = true   // the next unit is a source start
    private var lastPts: Int64 = -1

    init(_ spec: LayerSpec) {
        id = spec.id; codec = spec.codec; fps = spec.fps; maxWidth = spec.maxWidth; self.spec = spec
        lq = DispatchQueue(label: "k2.layer.\(spec.id)", qos: .userInteractive)
    }

    /// The coded size for a native surface: no wider than max_width or the
    /// surface, even, aspect kept.
    static func fit(_ nw: Int, _ nh: Int, maxWidth: Int) -> (Int, Int) {
        let w = max(2, (maxWidth > 0 ? min(maxWidth, nw) : nw) & ~1)
        let h = max(2, Int((Double(nh) * Double(w) / Double(max(1, nw))).rounded()) & ~1)
        return (w, h)
    }

    func retune(_ s: LayerSpec) {
        let rates = s.fps != spec.fps || s.bitrate != spec.bitrate
        spec = s
        if rates { setRates(fps: s.fps, bitrate: s.bitrate) }
    }

    func setRates(fps: Double, bitrate: Int) {
        spec = LayerSpec(id: spec.id, codec: spec.codec, maxWidth: spec.maxWidth, fps: fps, bitrate: bitrate > 0 ? bitrate : spec.bitrate, quality: spec.quality, on: spec.on)
        if let s = session { H264.rates(s, fps: spec.fps, bitrate: spec.bitrate) }
    }

    func close() {
        if let s = session { VTCompressionSessionInvalidate(s) }
        if let t = transfer { VTPixelTransferSessionInvalidate(t) }
        session = nil; transfer = nil
    }

    /// Encodes the surface as it is now. false: nothing was sent or queued.
    /// `rearm` runs when VideoToolbox drops a frame that had to be a key.
    func encode(_ s: IOSurface, captured: UInt64, key: Bool, rearm: @escaping () -> Void) -> Bool {
        codec == "jpeg" ? jpeg(s, captured: captured) : h264(s, captured: captured, key: key, rearm: rearm)
    }

    private func jpeg(_ s: IOSurface, captured: UInt64) -> Bool {
        let nw = IOSurfaceGetWidth(s)
        let (w, _) = Layer.fit(nw, IOSurfaceGetHeight(s), maxWidth: spec.maxWidth)
        guard let img = Image.cgImage(s),
              let j = Image.encode(img, scale: Double(w) / Double(max(1, nw)), type: UTType.jpeg, quality: spec.quality, crop: nil) else { return false }
        let start = fresh || (j.width, j.height) != size
        fresh = false; size = (j.width, j.height)
        send(codec: 3, key: true, start: start, width: j.width, height: j.height, captured: captured, emitted: nowNs(), data: j.data)
        return true
    }

    private func h264(_ s: IOSurface, captured: UInt64, key: Bool, rearm: @escaping () -> Void) -> Bool {
        let want = Layer.fit(IOSurfaceGetWidth(s), IOSurfaceGetHeight(s), maxWidth: spec.maxWidth)
        if session == nil || want != size {
            if let old = session {
                // The old size's last units go first, then the new size starts.
                VTCompressionSessionCompleteFrames(old, untilPresentationTimeStamp: .invalid)
                VTCompressionSessionInvalidate(old)
                session = nil
            }
            guard let o = H264.open(width: want.0, height: want.1, fps: spec.fps, bitrate: spec.bitrate) else { return false }
            session = o; size = want; fresh = true; lastPts = -1
        }
        guard let session else { return false }
        if transfer == nil {
            var t: VTPixelTransferSession?
            guard VTPixelTransferSessionCreate(allocator: nil, pixelTransferSessionOut: &t) == noErr, let t else {
                Out.log("layer \(id): no pixel transfer session"); return false
            }
            VTSessionSetProperty(t, key: kVTPixelTransferPropertyKey_DestinationYCbCrMatrix, value: kCVImageBufferYCbCrMatrix_ITU_R_709_2)
            VTSessionSetProperty(t, key: kVTPixelTransferPropertyKey_DestinationColorPrimaries, value: kCVImageBufferColorPrimaries_ITU_R_709_2)
            VTSessionSetProperty(t, key: kVTPixelTransferPropertyKey_DestinationTransferFunction, value: kCVImageBufferTransferFunction_ITU_R_709_2)
            transfer = t
        }
        // The framebuffer itself, no copy; then GPU scale + BGRA → NV12.
        var srcOut: Unmanaged<CVPixelBuffer>?
        guard CVPixelBufferCreateWithIOSurface(nil, s, nil, &srcOut) == kCVReturnSuccess, let src = srcOut?.takeRetainedValue() else {
            Out.log("layer \(id): CVPixelBufferCreateWithIOSurface failed"); return false
        }
        guard let pool = VTCompressionSessionGetPixelBufferPool(session) else { Out.log("layer \(id): no pixel buffer pool"); return false }
        var dstOut: CVPixelBuffer?
        guard CVPixelBufferPoolCreatePixelBuffer(nil, pool, &dstOut) == kCVReturnSuccess, let dst = dstOut else { return false }
        let tst = VTPixelTransferSessionTransferImage(transfer!, from: src, to: dst)
        guard tst == noErr else { Out.log("layer \(id): pixel transfer failed (OSStatus \(tst))"); return false }
        CVBufferSetAttachment(dst, kCVImageBufferYCbCrMatrixKey, kCVImageBufferYCbCrMatrix_ITU_R_709_2, .shouldPropagate)
        CVBufferSetAttachment(dst, kCVImageBufferColorPrimariesKey, kCVImageBufferColorPrimaries_ITU_R_709_2, .shouldPropagate)
        CVBufferSetAttachment(dst, kCVImageBufferTransferFunctionKey, kCVImageBufferTransferFunction_ITU_R_709_2, .shouldPropagate)
        let start = fresh
        fresh = false
        let force = key || start
        let pts = max(Int64(captured / 1000), lastPts + 1)
        lastPts = pts
        let props: CFDictionary? = force ? [kVTEncodeFrameOptionKey_ForceKeyFrame: kCFBooleanTrue!] as CFDictionary : nil
        let st = VTCompressionSessionEncodeFrame(session, imageBuffer: dst, presentationTimeStamp: CMTime(value: pts, timescale: 1_000_000),
                                                 duration: .invalid, frameProperties: props, infoFlagsOut: nil) { [weak self] status, flags, sb in
            // VideoToolbox's thread: never touches the encoder state.
            let emitted = nowNs()
            guard let self else { return }
            guard status == noErr, !flags.contains(.frameDropped), let sb, let au = H264.annexB(sb), !(start && !au.key) else {
                if status != noErr { Out.log("layer \(self.id): encode failed (OSStatus \(status))") }
                if force {
                    if start { self.lq.async { self.fresh = true } }
                    rearm()
                }
                return
            }
            self.send(codec: 1, key: au.key, start: start, width: au.width, height: au.height, captured: captured, emitted: emitted, data: au.data)
        }
        if st != noErr {
            Out.log("layer \(id): VTCompressionSessionEncodeFrame failed (OSStatus \(st))")
            if start { fresh = true }
            return false
        }
        return true
    }

    private func send(codec: UInt8, key: Bool, start: Bool, width: Int, height: Int, captured: UInt64, emitted: UInt64, data: Data) {
        Out.unit {
            guard self.live else { return nil }
            let seq = (Layer.seqs[self.id] ?? 0) + 1
            Layer.seqs[self.id] = seq
            var d = unitHeader(codec: codec, key: key || start, start: start, layer: self.id, width: width, height: height,
                               seq: seq, captured: captured, emitted: emitted)
            d.append(data)
            return d
        }
    }
}

/// link.rs's 40-byte unit header, big-endian (its golden test pins the bytes).
func unitHeader(codec: UInt8, key: Bool, start: Bool, layer: UInt8, width: Int, height: Int, seq: UInt64, captured: UInt64, emitted: UInt64) -> Data {
    var d = Data(capacity: 40)
    func be<T: FixedWidthInteger>(_ v: T) { withUnsafeBytes(of: v.bigEndian) { d.append(contentsOf: $0) } }
    d.append(codec)
    d.append((key ? 1 : 0) | (start ? 2 : 0))
    d.append(layer)
    d.append(0)
    be(UInt32(0))   // source: 0 for a simulator
    be(UInt32(width)); be(UInt32(height))
    be(seq); be(captured); be(emitted)
    return d
}

// MARK: - H.264 (VideoToolbox; the settings are kessel-video's)

enum H264 {
    private static let lock = NSLock()
    private static var checked = "unknown"
    /// The encoder the last session opened on: "hardware", "software" or
    /// "unknown" (nothing said which; never a guess).
    static var encoder: String { lock.lock(); defer { lock.unlock() }; return checked }
    static private(set) var available = false

    /// At start: can an H.264 session open here, and on what? (`codecs`, `encoder`.)
    static func probe(width: Int, height: Int) {
        if let s = open(width: width, height: height, fps: 60, bitrate: 6_000_000) {
            available = true
            VTCompressionSessionInvalidate(s)
        }
    }

    /// Hardware or software, from what VideoToolbox says: the session's own
    /// "using hardware" answer, else its EncoderID's entry in the encoder list.
    static func check(_ s: VTCompressionSession) -> (kind: String, id: String) {
        let id = copy(s, kVTCompressionPropertyKey_EncoderID) as? String
        if let b = copy(s, kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder) as? Bool {
            return (b ? "hardware" : "software", id ?? "?")
        }
        guard let id else { return ("unknown", "?") }
        var list: CFArray?
        guard VTCopyVideoEncoderList(nil, &list) == noErr, let entries = list as? [[String: Any]] else { return ("unknown", id) }
        for e in entries where e[kVTVideoEncoderList_EncoderID as String] as? String == id {
            return ((e[kVTVideoEncoderList_IsHardwareAccelerated as String] as? Bool) == true ? "hardware" : "software", id)
        }
        return ("unknown", id)
    }

    /// A session property (a copy we own), or nil when the session won't say.
    static func copy(_ s: VTCompressionSession, _ k: CFString) -> CFTypeRef? {
        var v: CFTypeRef?
        let st = withUnsafeMutablePointer(to: &v) { VTSessionCopyProperty(s, key: k, allocator: nil, valueOut: UnsafeMutableRawPointer($0)) }
        return st == noErr ? v : nil
    }

    @discardableResult
    static func set(_ s: VTCompressionSession, _ k: CFString, _ v: CFTypeRef) -> Bool { VTSessionSetProperty(s, key: k, value: v) == noErr }

    /// Opens a real-time session: hardware required first (with low-latency
    /// rate control, then without), then hardware allowed. A required-hardware
    /// session that can't be confirmed as hardware is dropped.
    static func open(width: Int, height: Int, fps: Double, bitrate: Int) -> VTCompressionSession? {
        let attrs: [CFString: Any] = [
            kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
            kCVPixelBufferWidthKey: width, kCVPixelBufferHeightKey: height,
            kCVPixelBufferIOSurfacePropertiesKey: [CFString: Any](),
        ]
        for (require, lowLatency) in [(true, true), (true, false), (false, true), (false, false)] {
            let ask = "\(require ? "require" : "enable") hardware\(lowLatency ? " + low latency" : "")"
            let hardware = require ? kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder
                                   : kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder
            var spec: [CFString: Any] = [hardware: true]
            if lowLatency { spec[kVTVideoEncoderSpecification_EnableLowLatencyRateControl] = true }
            var out: VTCompressionSession?
            let st = VTCompressionSessionCreate(allocator: nil, width: Int32(width), height: Int32(height), codecType: kCMVideoCodecType_H264,
                                                encoderSpecification: spec as CFDictionary, imageBufferAttributes: attrs as CFDictionary,
                                                compressedDataAllocator: nil, outputCallback: nil, refcon: nil, compressionSessionOut: &out)
            guard st == noErr, let s = out else { Out.log("h264 \(width)x\(height): \(ask): no session (OSStatus \(st))"); continue }
            let (kind, id) = check(s)
            if require && kind != "hardware" {
                Out.log("h264 \(width)x\(height): \(ask): encoder \(id) is \(kind), not confirmed hardware: dropped")
                VTCompressionSessionInvalidate(s); continue
            }
            set(s, kVTCompressionPropertyKey_RealTime, kCFBooleanTrue)
            set(s, kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse)
            var profile = "ConstrainedHigh"
            if !set(s, kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_ConstrainedHigh_AutoLevel) {
                profile = "High"
                if !set(s, kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_High_AutoLevel) {
                    Out.log("h264 \(width)x\(height): \(ask): no Constrained High or High profile: dropped")
                    VTCompressionSessionInvalidate(s); continue
                }
            }
            // Keyframes only on demand; this is a 10-minute safety net.
            set(s, kVTCompressionPropertyKey_MaxKeyFrameInterval, NSNumber(value: 36_000))
            set(s, kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, NSNumber(value: 600.0))
            set(s, kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality, kCFBooleanTrue)
            rates(s, fps: fps, bitrate: bitrate)
            VTCompressionSessionPrepareToEncodeFrames(s)
            lock.lock(); checked = kind; lock.unlock()
            Out.log("h264 \(width)x\(height): \(ask): \(kind) (encoder \(id), profile \(profile))")
            return s
        }
        return nil
    }

    /// Target bits a second, a 1 s ceiling at 1.5x, the expected rate.
    static func rates(_ s: VTCompressionSession, fps: Double, bitrate: Int) {
        set(s, kVTCompressionPropertyKey_AverageBitRate, NSNumber(value: bitrate))
        set(s, kVTCompressionPropertyKey_DataRateLimits, [NSNumber(value: Int(Double(bitrate) / 8 * 1.5)), NSNumber(value: 1.0)] as CFArray)
        set(s, kVTCompressionPropertyKey_ExpectedFrameRate, NSNumber(value: fps))
    }

    /// AVCC sample → one Annex B access unit, SPS + PPS (from the format
    /// description) in front of a keyframe. Key = no NotSync attachment.
    static func annexB(_ sb: CMSampleBuffer) -> (data: Data, key: Bool, width: Int, height: Int)? {
        var key = true
        if let atts = CMSampleBufferGetSampleAttachmentsArray(sb, createIfNecessary: false) as? [[CFString: Any]],
           let a = atts.first, (a[kCMSampleAttachmentKey_NotSync] as? Bool) == true { key = false }
        guard let desc = CMSampleBufferGetFormatDescription(sb) else { return nil }
        let dims = CMVideoFormatDescriptionGetDimensions(desc)
        var count = 0
        var nalLen: Int32 = 0
        guard CMVideoFormatDescriptionGetH264ParameterSetAtIndex(desc, parameterSetIndex: 0, parameterSetPointerOut: nil, parameterSetSizeOut: nil,
                                                                 parameterSetCountOut: &count, nalUnitHeaderLengthOut: &nalLen) == noErr, nalLen > 0 else { return nil }
        let startCode: [UInt8] = [0, 0, 0, 1]
        var out = Data()
        if key {
            for i in 0..<count {
                var p: UnsafePointer<UInt8>?
                var n = 0
                guard CMVideoFormatDescriptionGetH264ParameterSetAtIndex(desc, parameterSetIndex: i, parameterSetPointerOut: &p, parameterSetSizeOut: &n,
                                                                         parameterSetCountOut: nil, nalUnitHeaderLengthOut: nil) == noErr, let p else { return nil }
                out.append(contentsOf: startCode)
                out.append(p, count: n)
            }
        }
        guard let bb = CMSampleBufferGetDataBuffer(sb) else { return nil }
        let len = CMBlockBufferGetDataLength(bb)
        var avcc = [UInt8](repeating: 0, count: len)
        guard CMBlockBufferCopyDataBytes(bb, atOffset: 0, dataLength: len, destination: &avcc) == noErr else { return nil }
        let L = Int(nalLen)
        var i = 0
        while i + L <= len {
            var n = 0
            for j in 0..<L { n = n << 8 | Int(avcc[i + j]) }
            i += L
            guard n > 0, i + n <= len else { return nil }
            let type = avcc[i] & 0x1f
            // Parameter sets are written once, above, from the format.
            if !(key && (type == 7 || type == 8)) {
                out.append(contentsOf: startCode)
                out.append(contentsOf: avcc[i..<(i + n)])
            }
            i += n
        }
        return (out, key, Int(dims.width), Int(dims.height))
    }
}

enum Image {
    static func cgImage(_ s: IOSurface) -> CGImage? {
        IOSurfaceLock(s, .readOnly, nil); defer { IOSurfaceUnlock(s, .readOnly, nil) }
        let w = IOSurfaceGetWidth(s), h = IOSurfaceGetHeight(s), bpr = IOSurfaceGetBytesPerRow(s)
        guard let ctx = CGContext(data: IOSurfaceGetBaseAddress(s), width: w, height: h, bitsPerComponent: 8, bytesPerRow: bpr,
                                  space: CGColorSpaceCreateDeviceRGB(),
                                  bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue) else { return nil }
        return ctx.makeImage()   // copies out of the surface before we unlock
    }

    /// Scale (and optionally crop, in source pixels) then encode.
    static func encode(_ img: CGImage, scale: Double, type: UTType, quality: Double, crop: CGRect?) -> (data: Data, width: Int, height: Int)? {
        var src = img
        if let crop, let c = img.cropping(to: crop.integral) { src = c }
        let ow = max(1, Int((Double(src.width) * scale).rounded())), oh = max(1, Int((Double(src.height) * scale).rounded()))
        guard let small = CGContext(data: nil, width: ow, height: oh, bitsPerComponent: 8, bytesPerRow: 0,
                                    space: CGColorSpaceCreateDeviceRGB(),
                                    bitmapInfo: CGImageAlphaInfo.noneSkipFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue) else { return nil }
        small.interpolationQuality = .medium
        small.draw(src, in: CGRect(x: 0, y: 0, width: ow, height: oh))
        guard let out = small.makeImage() else { return nil }
        let buf = NSMutableData()
        guard let dst = CGImageDestinationCreateWithData(buf, type.identifier as CFString, 1, nil) else { return nil }
        CGImageDestinationAddImage(dst, out, [kCGImageDestinationLossyCompressionQuality: quality] as CFDictionary)
        guard CGImageDestinationFinalize(dst) else { return nil }
        return (buf as Data, ow, oh)
    }
}

// MARK: - HID (SimDeviceLegacyHIDClient + Indigo messages)

final class HID {
    typealias MouseFn = @convention(c) (UnsafePointer<CGPoint>, UnsafePointer<CGPoint>?, UInt32, Int32, CGFloat, CGFloat, UInt32) -> UnsafeMutableRawPointer?
    typealias KeyFn = @convention(c) (UInt32, UInt32) -> UnsafeMutableRawPointer?
    typealias ButtonFn = @convention(c) (Int32, Int32, Int32) -> UnsafeMutableRawPointer?
    private let mouse: MouseFn, key: KeyFn, button: ButtonFn
    private let client: NSObject
    private let sendSel = NSSelectorFromString("sendWithMessage:freeWhenDone:completionQueue:completion:")
    let screen: Screen

    init(device: NSObject, screen: Screen) throws {
        self.screen = screen
        let h = UnsafeMutableRawPointer(bitPattern: -2)
        guard let m = dlsym(h, "IndigoHIDMessageForMouseNSEvent"),
              let k = dlsym(h, "IndigoHIDMessageForKeyboardArbitrary"),
              let b = dlsym(h, "IndigoHIDMessageForButton") else {
            throw HelperError(code: "no_indigo", message: "SimulatorKit Indigo HID symbols are missing in this Xcode")
        }
        mouse = unsafeBitCast(m, to: MouseFn.self); key = unsafeBitCast(k, to: KeyFn.self); button = unsafeBitCast(b, to: ButtonFn.self)
        guard let cls = NSClassFromString("_TtC12SimulatorKit24SimDeviceLegacyHIDClient") as? NSObject.Type else {
            throw HelperError(code: "no_hid_client", message: "SimDeviceLegacyHIDClient is missing in this Xcode")
        }
        typealias InitFn = @convention(c) (AnyObject, Selector, AnyObject, AutoreleasingUnsafeMutablePointer<NSError?>) -> AnyObject?
        let sel = NSSelectorFromString("initWithDevice:error:")
        guard let imp = class_getMethodImplementation(cls, sel) else {
            throw HelperError(code: "no_hid_client", message: "SimDeviceLegacyHIDClient has no initWithDevice:error:")
        }
        var err: NSError?
        let raw = cls.perform(NSSelectorFromString("alloc")).takeUnretainedValue()
        guard let c = unsafeBitCast(imp, to: InitFn.self)(raw, sel, device, &err) as? NSObject else {
            throw HelperError(code: "hid_init_failed", message: "HID client: \(err?.localizedDescription ?? "unknown")")
        }
        client = c
    }

    private func send(_ msg: UnsafeMutableRawPointer?) throws {
        guard let msg else { throw HelperError(code: "hid_message_nil", message: "Indigo returned no message") }
        typealias SendFn = @convention(c) (AnyObject, Selector, UnsafeMutableRawPointer, ObjCBool, AnyObject?, AnyObject?) -> Void
        guard let cls = object_getClass(client), let imp = class_getMethodImplementation(cls, sendSel) else {
            free(msg); throw HelperError(code: "hid_send_missing", message: "HID client cannot send")
        }
        unsafeBitCast(imp, to: SendFn.self)(client, sendSel, msg, ObjCBool(true), nil, nil)
    }

    /// x, y in points; Indigo wants 0..1 of the screen.
    private func touch(_ down: Bool, _ x: Double, _ y: Double) throws {
        guard screen.widthPt > 0, screen.heightPt > 0 else { throw HelperError(code: "no_screen", message: "unknown screen size") }
        var p = CGPoint(x: min(max(x / screen.widthPt, 0), 1), y: min(max(y / screen.heightPt, 0), 1))
        try send(mouse(&p, nil, 0x32, down ? 1 : 2, 1.0, 1.0, 0))
    }

    /// One raw phase from a person's pointer: down (also "move": down at the new point) or up.
    func touchPhase(down: Bool, _ x: Double, _ y: Double) throws { try touch(down, x, y) }

    func tap(_ x: Double, _ y: Double, holdMs: Double) throws {
        try touch(true, x, y); usleep(useconds_t(holdMs * 1000)); try touch(false, x, y)
    }

    func swipe(_ x1: Double, _ y1: Double, _ x2: Double, _ y2: Double, ms: Double) throws {
        let steps = max(2, Int(ms / 10))
        try touch(true, x1, y1)
        for i in 1...steps {
            let t = Double(i) / Double(steps)
            usleep(useconds_t(ms * 1000 / Double(steps)))
            try touch(true, x1 + (x2 - x1) * t, y1 + (y2 - y1) * t)   // "move" = down at the new point
        }
        try touch(false, x2, y2)
    }

    /// One key with modifiers held: Left Shift 0xE1, Left Option 0xE2,
    /// Left Command 0xE3 (⌘V pastes, ⌘Z undoes, ⌥← jumps a word).
    func press(_ usage: UInt32, shift: Bool, alt: Bool = false, cmd: Bool = false) throws {
        let mods: [UInt32] = (cmd ? [0xE3] : []) + (alt ? [0xE2] : []) + (shift ? [0xE1] : [])
        for m in mods { try send(key(m, 1)) }
        try send(key(usage, 1)); usleep(8_000); try send(key(usage, 2))
        for m in mods.reversed() { try send(key(m, 2)) }
        usleep(8_000)
    }

    func hardwareButton(_ name: String) throws {
        let source: Int32
        switch name {
        case "home": source = 0x0
        case "lock": source = 0x1
        case "side": source = 0xbb8
        case "siri": source = 0x400002
        default: throw HelperError(code: "bad_button", message: "unknown button \(name) (home, lock, side, siri)")
        }
        try send(button(source, 1, 0x33)); usleep(60_000); try send(button(source, 2, 0x33))
    }

    /// US keyboard layout: character -> (HID usage, shift).
    static func usage(_ c: Character) -> (UInt32, Bool)? {
        guard let a = c.asciiValue else { return nil }
        switch a {
        case 97...122: return (UInt32(a - 97) + 0x04, false)
        case 65...90: return (UInt32(a - 65) + 0x04, true)
        case 49...57: return (UInt32(a - 49) + 0x1E, false)
        case 48: return (0x27, false)
        default: break
        }
        let table: [Character: (UInt32, Bool)] = [
            " ": (0x2C, false), "\n": (0x28, false), "\t": (0x2B, false),
            "-": (0x2D, false), "_": (0x2D, true), "=": (0x2E, false), "+": (0x2E, true),
            "[": (0x2F, false), "{": (0x2F, true), "]": (0x30, false), "}": (0x30, true),
            "\\": (0x31, false), "|": (0x31, true), ";": (0x33, false), ":": (0x33, true),
            "'": (0x34, false), "\"": (0x34, true), "`": (0x35, false), "~": (0x35, true),
            ",": (0x36, false), "<": (0x36, true), ".": (0x37, false), ">": (0x37, true),
            "/": (0x38, false), "?": (0x38, true),
            "!": (0x1E, true), "@": (0x1F, true), "#": (0x20, true), "$": (0x21, true), "%": (0x22, true),
            "^": (0x23, true), "&": (0x24, true), "*": (0x25, true), "(": (0x26, true), ")": (0x27, true),
        ]
        return table[c]
    }

    func type(_ text: String) throws {
        // Check the whole string first so a half-typed value never happens.
        var plan: [(UInt32, Bool)] = []
        for c in text {
            guard let u = HID.usage(c) else {
                throw HelperError(code: "unmappable", message: "no US-keyboard key for \(String(c).unicodeScalars.map { String(format: "U+%04X", $0.value) }.joined())")
            }
            plan.append(u)
        }
        for (u, sh) in plan { try press(u, shift: sh) }
    }
}

// MARK: - Accessibility (AXPTranslator bridged onto SimDevice accessibility XPC)

final class AX: NSObject {
    private let device: NSObject
    private let q = DispatchQueue(label: "k2.ax")
    private var translator: NSObject!
    private static let maxElements = 600
    private static let maxDepth = 60

    init(device: NSObject) throws {
        self.device = device
        super.init()
        guard dlopen("/System/Library/PrivateFrameworks/AccessibilityPlatformTranslation.framework/AccessibilityPlatformTranslation", RTLD_NOW) != nil,
              let cls = NSClassFromString("AXPTranslator") as? NSObject.Type,
              let t = cls.perform(NSSelectorFromString("sharedInstance"))?.takeUnretainedValue() as? NSObject else {
            throw HelperError(code: "no_ax", message: "AccessibilityPlatformTranslation is not loadable")
        }
        guard device.responds(to: NSSelectorFromString("sendAccessibilityRequestAsync:completionQueue:completionHandler:")) else {
            throw HelperError(code: "no_ax", message: "this CoreSimulator has no accessibility requests")
        }
        t.setValue(self, forKey: "bridgeTokenDelegate")
        t.setValue(true, forKey: "supportsDelegateTokens")
        t.setValue(true, forKey: "accessibilityEnabled")
        translator = t
    }

    @objc(accessibilityTranslationDelegateBridgeCallbackWithToken:)
    func bridgeCallback(token: String) -> AnyObject {
        let block: @convention(block) (AnyObject?) -> AnyObject? = { [weak self] req in
            guard let self, let req else { return AX.emptyResponse() }
            let sel = NSSelectorFromString("sendAccessibilityRequestAsync:completionQueue:completionHandler:")
            guard let imp = self.device.method(for: sel) else { return AX.emptyResponse() }
            typealias SendFn = @convention(c) (AnyObject, Selector, AnyObject, DispatchQueue, AnyObject) -> Void
            final class Box { var v: AnyObject? }
            let box = Box(); let sem = DispatchSemaphore(value: 0)
            let done: @convention(block) (AnyObject?) -> Void = { r in box.v = r; sem.signal() }
            unsafeBitCast(imp, to: SendFn.self)(self.device, sel, req, self.q, done as AnyObject)
            if sem.wait(timeout: .now() + 5) == .timedOut { return AX.emptyResponse() }
            return box.v ?? AX.emptyResponse()
        }
        return block as AnyObject
    }

    @objc(accessibilityTranslationConvertPlatformFrameToSystem:withToken:)
    func convertFrame(rect: NSRect, withToken token: String) -> NSRect { rect }

    @objc(accessibilityTranslationRootParentWithToken:)
    func rootParent(withToken token: String) -> AnyObject? { nil }

    private static func emptyResponse() -> AnyObject? {
        (NSClassFromString("AXPTranslatorResponse") as? NSObject.Type)?
            .perform(NSSelectorFromString("emptyResponse"))?.takeUnretainedValue() as AnyObject?
    }

    private func macElement(_ translation: NSObject, _ token: NSString) -> NSAccessibilityElement? {
        translation.setValue(token, forKey: "bridgeDelegateToken")
        let ms = NSSelectorFromString("macPlatformElementFromTranslation:")
        typealias MacFn = @convention(c) (AnyObject, Selector, AnyObject) -> AnyObject?
        guard let imp = translator.method(for: ms) else { return nil }
        let e = unsafeBitCast(imp, to: MacFn.self)(translator, ms, translation) as? NSObject
        (e?.value(forKey: "translation") as? NSObject)?.setValue(token, forKey: "bridgeDelegateToken")
        return e as? NSAccessibilityElement
    }

    private func root(_ token: NSString) throws -> NSAccessibilityElement {
        let fs = NSSelectorFromString("frontmostApplicationWithDisplayId:bridgeDelegateToken:")
        typealias FrontFn = @convention(c) (AnyObject, Selector, UInt32, NSString) -> AnyObject?
        guard let imp = translator.method(for: fs),
              let tr = unsafeBitCast(imp, to: FrontFn.self)(translator, fs, 0, token) as? NSObject,
              let r = macElement(tr, token) else {
            throw HelperError(code: "no_frontmost_app", message: "no frontmost app yet (still booting?)")
        }
        return r
    }

    private func element(at x: Double, _ y: Double, _ token: NSString) -> NSAccessibilityElement? {
        let ps = NSSelectorFromString("objectAtPoint:displayId:bridgeDelegateToken:")
        typealias PointFn = @convention(c) (AnyObject, Selector, CGPoint, UInt32, NSString) -> AnyObject?
        guard let imp = translator.method(for: ps),
              let tr = unsafeBitCast(imp, to: PointFn.self)(translator, ps, CGPoint(x: x, y: y), 0, token) as? NSObject else { return nil }
        return macElement(tr, token)
    }

    private static func frameKey(_ f: NSRect) -> String {
        "\(Int(f.minX.rounded())),\(Int(f.minY.rounded())),\(Int(f.width.rounded())),\(Int(f.height.rounded()))"
    }

    private func node(_ el: NSAccessibilityElement, _ token: NSString, depth: Int, budget: inout Int,
                      seen: inout Set<String>, leaves: inout [NSRect]) -> [String: Any]? {
        guard budget > 0, depth <= AX.maxDepth else { return nil }
        budget -= 1
        (el.value(forKey: "translation") as? NSObject)?.setValue(token, forKey: "bridgeDelegateToken")
        let f = el.accessibilityFrame()
        seen.insert(AX.frameKey(f))
        var d: [String: Any] = [
            "role": el.accessibilityRole()?.rawValue ?? "",
            "frame": [f.minX, f.minY, f.width, f.height].map { Double($0) },
            "enabled": el.isAccessibilityEnabled(),
        ]
        if el.isAccessibilityFocused() { d["focused"] = true }
        if el.isAccessibilitySelected() { d["selected"] = true }
        if let s = el.accessibilitySubrole()?.rawValue, !s.isEmpty { d["subrole"] = s }
        if let s = el.accessibilityLabel(), !s.isEmpty { d["label"] = s }
        let ident: String? = el.accessibilityIdentifier()
        if let ident, !ident.isEmpty { d["identifier"] = ident }
        if let v = el.accessibilityValue() {
            let s = (v as? String) ?? (v as? NSNumber)?.stringValue ?? "\(v)"
            if !s.isEmpty { d["value"] = s }
        }
        if let s = el.accessibilityPlaceholderValue(), !s.isEmpty { d["placeholder"] = s }
        var kids: [[String: Any]] = []
        for c in el.accessibilityChildren() ?? [] {
            guard let ce = c as? NSAccessibilityElement else { continue }
            if let n = node(ce, token, depth: depth + 1, budget: &budget, seen: &seen, leaves: &leaves) { kids.append(n) }
        }
        if kids.isEmpty { leaves.append(f) } else { d["children"] = kids }
        return d
    }

    /// The frontmost app's tree in points.
    /// grid "off": the recursive walk only. "auto" (default): also hit-test
    /// inside big childless elements (≥ 250 pt), which is how scroll and
    /// collection views hide their children. "full": hit-test the whole screen.
    func tree(grid: String) throws -> [String: Any] {
        let token = UUID().uuidString as NSString
        let r = try root(token)
        var budget = AX.maxElements
        var seen = Set<String>()
        var leaves: [NSRect] = []
        guard var top = node(r, token, depth: 0, budget: &budget, seen: &seen, leaves: &leaves) else {
            throw HelperError(code: "no_frontmost_app", message: "empty accessibility tree")
        }
        let bounds = r.accessibilityFrame()
        var areas: [NSRect] = []
        switch grid {
        case "full": areas = [bounds]
        case "auto":
            areas = leaves.filter { max($0.width, $0.height) >= 250 && !(abs($0.width - bounds.width) < 1 && abs($0.height - bounds.height) < 1) }
        default: areas = []
        }
        // Big containers are not leaves: let points inside them be sampled.
        leaves.removeAll { l in areas.contains { $0 == l } }
        var extra: [[String: Any]] = []
        var points = 600
        let step: CGFloat = 32
        for area in areas where area.width > 1 && area.height > 1 {
            var y = area.minY + step / 2
            while y < area.maxY, points > 0, budget > 0 {
                var x = area.minX + step / 2
                while x < area.maxX, points > 0, budget > 0 {
                    let p = CGPoint(x: x, y: y); x += step
                    if leaves.contains(where: { $0.contains(p) }) { continue }
                    points -= 1
                    guard let el = element(at: Double(p.x), Double(p.y), token) else { continue }
                    let f = el.accessibilityFrame()
                    if seen.contains(AX.frameKey(f)) { continue }
                    if abs(f.width - bounds.width) < 1, abs(f.height - bounds.height) < 1 { seen.insert(AX.frameKey(f)); continue }
                    if let n = node(el, token, depth: 1, budget: &budget, seen: &seen, leaves: &leaves) { extra.append(n) }
                }
                y += step
            }
        }
        if !extra.isEmpty {
            var kids = top["children"] as? [[String: Any]] ?? []
            kids.append(contentsOf: extra)
            top["children"] = kids
        }
        if let pid = (r.value(forKey: "translation") as? NSObject).flatMap({ t -> Int? in
            for k in ["pid", "processIdentifier"] where t.responds(to: NSSelectorFromString(k)) {
                if let n = t.value(forKey: k) as? NSNumber { return n.intValue }
            }
            return nil
        }) { top["pid"] = pid }
        top["truncated"] = budget <= 0
        return top
    }

    /// The element whose frame matches `frame` (from the last tree), found by
    /// walking; a hit-test at its centre is the fallback.
    private func locate(_ frame: [Double], _ token: NSString) throws -> NSAccessibilityElement {
        guard frame.count == 4 else { throw HelperError(code: "bad_request", message: "frame needs 4 numbers") }
        let want = AX.frameKey(NSRect(x: frame[0], y: frame[1], width: frame[2], height: frame[3]))
        let r = try root(token)
        var budget = AX.maxElements
        func find(_ el: NSAccessibilityElement, _ depth: Int) -> NSAccessibilityElement? {
            guard budget > 0, depth <= AX.maxDepth else { return nil }
            budget -= 1
            (el.value(forKey: "translation") as? NSObject)?.setValue(token, forKey: "bridgeDelegateToken")
            if depth > 0, AX.frameKey(el.accessibilityFrame()) == want { return el }
            for c in el.accessibilityChildren() ?? [] {
                if let ce = c as? NSAccessibilityElement, let hit = find(ce, depth + 1) { return hit }
            }
            return nil
        }
        if let el = find(r, 0) { return el }
        if let el = element(at: frame[0] + frame[2] / 2, frame[1] + frame[3] / 2, token) { return el }
        throw HelperError(code: "no_element", message: "no element at \(want) any more (run tree again)")
    }

    func focus(hint: (Double, Double)?) throws -> [String: Any] {
        let token = UUID().uuidString as NSString
        let r = try root(token)
        var el: NSAccessibilityElement?
        let fsel = NSSelectorFromString("accessibilityFocusedUIElement")
        if r.responds(to: fsel), let f = r.perform(fsel)?.takeUnretainedValue() as? NSAccessibilityElement {
            (f.value(forKey: "translation") as? NSObject)?.setValue(token, forKey: "bridgeDelegateToken")
            if f.accessibilityFrame() != r.accessibilityFrame() { el = f }
        }
        if el == nil, let (x, y) = hint { el = element(at: x, y, token) }
        guard let el else { throw HelperError(code: "no_element", message: "nothing has focus; tap the field first") }
        let f = el.accessibilityFrame()
        var d: [String: Any] = ["role": el.accessibilityRole()?.rawValue ?? "", "frame": [f.minX, f.minY, f.width, f.height].map { Double($0) }]
        if let s = el.accessibilitySubrole()?.rawValue, !s.isEmpty { d["subrole"] = s }
        return d
    }

    func valueOf(_ frame: [Double]) throws -> [String: Any] {
        let el = try locate(frame, UUID().uuidString as NSString)
        var d: [String: Any] = ["role": el.accessibilityRole()?.rawValue ?? ""]
        if let s = el.accessibilitySubrole()?.rawValue, !s.isEmpty { d["subrole"] = s }
        if let v = el.accessibilityValue() { d["value"] = (v as? String) ?? (v as? NSNumber)?.stringValue ?? "\(v)" }
        // An empty iOS text field reports its placeholder as its value.
        if let p = el.accessibilityPlaceholderValue(), !p.isEmpty { d["placeholder"] = p }
        return d
    }

    /// Sets an element's value through accessibility: no keystrokes at all.
    func setValue(_ frame: [Double], _ text: String) throws {
        try locate(frame, UUID().uuidString as NSString).setAccessibilityValue(text)
    }
}

// MARK: - Main loop

func arg(_ name: String) -> String? {
    let a = CommandLine.arguments
    guard let i = a.firstIndex(of: name), i + 1 < a.count else { return nil }
    return a[i + 1]
}

signal(SIGPIPE, SIG_IGN)
guard let setPath = arg("--set"), let udid = arg("--udid") else {
    Out.log("usage: k2-simhelper --set <device-set-path> --udid <UDID> [--frames-fd <n>]")
    exit(64)
}

loadFrameworks()
_ = dlopen("/System/Library/PrivateFrameworks/AccessibilityPlatformTranslation.framework/AccessibilityPlatformTranslation", RTLD_NOW)
let xcode = xcodeVersion()
let missing = selfCheck()
if !missing.isEmpty {
    Out.json(["event": "fatal", "code": "unsupported_xcode", "xcode": xcode,
              "error": "Xcode \(xcode) lacks what K2's Simulator helper needs: \(missing.joined(separator: ", "))"])
    exit(3)
}
let device: NSObject
do { device = try findDevice(setPath: setPath, udid: udid) } catch let e as HelperError {
    Out.json(["event": "fatal", "code": e.code, "error": e.message, "xcode": xcode]); exit(2)
} catch { exit(2) }
for sel in ["io", "sendAccessibilityRequestAsync:completionQueue:completionHandler:"] where !device.responds(to: NSSelectorFromString(sel)) {
    Out.json(["event": "fatal", "code": "unsupported_xcode", "xcode": xcode,
              "error": "Xcode \(xcode): SimDevice has no \(sel)"])
    exit(3)
}

let screen = screenOf(device)
var frames: Frames?
var hid: HID?
var ax: AX?

func needFrames() throws -> Frames { if let f = frames { return f }; let f = try Frames(device: device); frames = f; return f }
func needHID() throws -> HID { if let h = hid { return h }; let h = try HID(device: device, screen: screen); hid = h; return h }
func needAX() throws -> AX { if let a = ax { return a }; let a = try AX(device: device); ax = a; return a }

func num(_ r: [String: Any], _ k: String) throws -> Double {
    guard let n = r[k] as? NSNumber else { throw HelperError(code: "bad_request", message: "missing number \(k)") }
    return n.doubleValue
}
func frameArg(_ r: [String: Any]) throws -> [Double] {
    guard let f = r["frame"] as? [NSNumber], f.count == 4 else { throw HelperError(code: "bad_request", message: "missing frame [x,y,w,h]") }
    return f.map { $0.doubleValue }
}
func layerId(_ r: [String: Any]) throws -> UInt8 {
    guard let n = (r["layer"] as? NSNumber)?.intValue, (0...255).contains(n) else { throw HelperError(code: "bad_request", message: "missing layer 0-255") }
    return UInt8(n)
}
/// What `layers` can encode, best first.
func codecs() -> [String] { H264.available ? ["h264", "jpeg"] : ["jpeg"] }
func str(_ r: [String: Any], _ k: String) throws -> String {
    guard let s = r[k] as? String else { throw HelperError(code: "bad_request", message: "missing string \(k)") }
    return s
}

// The helper ends when the device stops: the daemon re-spawns it on the next boot.
DispatchQueue.global(qos: .utility).async {
    while true {
        sleep(2)
        let state = device.value(forKey: "stateString") as? String ?? "?"
        if state != "Booted" {
            Out.json(["event": "device_gone", "state": state])
            exit(0)
        }
    }
}

func handle(_ r: [String: Any]) throws -> [String: Any] {
    let op = try str(r, "op")
    switch op {
    case "hello":
        return ["version": HELPER_VERSION, "name": device.value(forKey: "name") as? String ?? "",
                "state": device.value(forKey: "stateString") as? String ?? "",
                "runtime": (device.value(forKey: "runtime") as? NSObject)?.value(forKey: "name") as? String ?? "",
                "screen": ["width": screen.widthPt, "height": screen.heightPt, "scale": screen.scale],
                "developer_dir": devDir, "xcode": xcode,
                "accepts": HELPER_ACCEPTS, "codecs": codecs(), "encoder": H264.encoder,
                // The frame times' clock (ns of uptime): the daemon maps it onto its own.
                "clock_ns": nowNs()]
    case "focus":
        // What typed text goes into: the focused element, else the element at the hint.
        let hint: (Double, Double)? = (r["x"] as? NSNumber).flatMap { x in (r["y"] as? NSNumber).map { (x.doubleValue, $0.doubleValue) } }
        return try needAX().focus(hint: hint)
    case "touch":
        try needHID().touchPhase(down: (r["down"] as? Bool) ?? true, try num(r, "x"), try num(r, "y"))
        return [:]
    case "tree":
        return ["tree": try needAX().tree(grid: (r["grid"] as? String) ?? "auto")]
    case "value":
        return try needAX().valueOf(try frameArg(r))
    case "set_value":
        try needAX().setValue(try frameArg(r), try str(r, "text"))
        return [:]
    case "tap":
        try needHID().tap(try num(r, "x"), try num(r, "y"), holdMs: (r["hold_ms"] as? NSNumber)?.doubleValue ?? 50)
        return [:]
    case "swipe":
        try needHID().swipe(try num(r, "x1"), try num(r, "y1"), try num(r, "x2"), try num(r, "y2"),
                            ms: (r["ms"] as? NSNumber)?.doubleValue ?? 300)
        return [:]
    case "type":
        try needHID().type(try str(r, "text"))
        return [:]
    case "key":
        try needHID().press(UInt32(try num(r, "usage")), shift: (r["shift"] as? Bool) ?? false,
                            alt: (r["alt"] as? Bool) ?? false, cmd: (r["cmd"] as? Bool) ?? false)
        return [:]
    case "button":
        try needHID().hardwareButton(try str(r, "name"))
        return [:]
    case "frames":
        try needFrames().configure(on: (r["on"] as? Bool) ?? true, fps: (r["fps"] as? NSNumber)?.doubleValue ?? 30,
                                   scale: (r["scale"] as? NSNumber)?.doubleValue ?? (1.0 / screen.scale),
                                   quality: (r["quality"] as? NSNumber)?.doubleValue ?? 0.7)
        return [:]
    case "layers":
        guard let list = r["layers"] as? [[String: Any]] else { throw HelperError(code: "bad_request", message: "layers needs a list") }
        let specs = try list.map { try LayerSpec.parse($0, codecs: codecs()) }
        guard Set(specs.map { $0.id }).count == specs.count else { throw HelperError(code: "bad_request", message: "two layers with one id") }
        try needFrames().setLayers(specs)
        return [:]
    case "keyframe":
        let layer = try layerId(r)
        let since = (r["since_ns"] as? NSNumber)?.uint64Value ?? 0
        guard try needFrames().keyframe(layer: layer, since: since) else { throw HelperError(code: "bad_request", message: "no layer \(layer) is on") }
        return [:]
    case "rate":
        let layer = try layerId(r)
        guard try needFrames().rate(layer: layer, fps: max(1, min(60, try num(r, "fps"))), bitrate: Int(try num(r, "bitrate"))) else {
            throw HelperError(code: "bad_request", message: "no layer \(layer) is on")
        }
        return [:]
    case "image":
        // A still to a file: PNG, optionally cropped to a rect in points.
        let path = try str(r, "path")
        guard let s = try needFrames().surface(), let img = Image.cgImage(s) else {
            throw HelperError(code: "no_frame", message: "no framebuffer yet")
        }
        var crop: CGRect?
        if let c = r["crop"] as? [NSNumber], c.count == 4 {
            let k = screen.scale
            crop = CGRect(x: c[0].doubleValue * k, y: c[1].doubleValue * k, width: c[2].doubleValue * k, height: c[3].doubleValue * k)
        }
        let scale = (r["scale"] as? NSNumber)?.doubleValue ?? (1.0 / screen.scale)
        guard let out = Image.encode(img, scale: scale, type: UTType.png, quality: 1, crop: crop) else {
            throw HelperError(code: "encode_failed", message: "could not encode the image")
        }
        try out.data.write(to: URL(fileURLWithPath: path), options: .atomic)
        return ["path": path, "width": out.width, "height": out.height]
    case "quit":
        Out.json(["id": r["id"] ?? 0, "ok": true]); exit(0)
    default:
        throw HelperError(code: "bad_op", message: "unknown op \(op)")
    }
}

// Can H.264 open here, and on what? A session at the screen's own size, closed again.
let probeSize = Layer.fit(Int((screen.widthPt * screen.scale).rounded()), Int((screen.heightPt * screen.scale).rounded()), maxWidth: 0)
H264.probe(width: probeSize.0 > 2 ? probeSize.0 : 1280, height: probeSize.1 > 2 ? probeSize.1 : 720)
Out.log("codecs \(codecs().joined(separator: ",")), encoder \(H264.encoder)")
Out.json(["event": "ready", "version": HELPER_VERSION, "accepts": HELPER_ACCEPTS, "codecs": codecs(), "encoder": H264.encoder,
          "clock_ns": nowNs(), "xcode": xcode])
while let line = readLine(strippingNewline: true) {
    guard !line.isEmpty else { continue }
    guard let data = line.data(using: .utf8),
          let r = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else {
        Out.json(["ok": false, "code": "bad_json", "error": "request is not a JSON object"]); continue
    }
    let id = r["id"] ?? NSNull()
    let t0 = now()
    // Fire-and-forget (protocol 3): no reply unless it failed.
    let noReply = (r["nr"] as? Bool) ?? false
    let op = r["op"] as? String ?? ""
    do {
        var reply = try handle(r)
        if noReply { continue }
        reply["id"] = id; reply["ok"] = true; reply["ms"] = Int(((now() - t0) * 1000).rounded())
        Out.json(reply)
    } catch let e as HelperError {
        if noReply { Out.json(["event": "input_error", "op": op, "code": e.code, "error": e.message]) }
        else { Out.json(["id": id, "ok": false, "code": e.code, "error": e.message]) }
    } catch {
        if noReply { Out.json(["event": "input_error", "op": op, "code": "error", "error": "\(error)"]) }
        else { Out.json(["id": id, "ok": false, "code": "error", "error": "\(error)"]) }
    }
}
exit(0)   // stdin closed: the daemon went away
