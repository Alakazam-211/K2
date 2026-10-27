// k2-simhelper — K2's iOS Simulator helper (L-I lane, prd-mobile-dev-v1 §3.3).
//
// One process per booted device, spawned by the K2 daemon as a plain child
// (M-A15). It talks ONLY over its own stdin/stdout pipes: no socket, no port,
// so nothing else on the machine can reach it (stricter than M-A16's
// loopback + random token).
//
//   k2-simhelper --set <device-set-path> --udid <UDID>
//
// stdin : one JSON request per line  {"id":1,"op":"tree"}
// stdout: framed messages  [u32 BE length][u8 kind][payload]   (length counts kind + payload)
//         kind 'J' = UTF-8 JSON (replies carry the request id; events carry "event")
//         kind 'F' = frame: [u32 BE width][u32 BE height][u64 BE seq] + JPEG bytes
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

let HELPER_VERSION = 1

// MARK: - Output

enum Out {
    static let lock = NSLock()
    static func send(_ kind: UInt8, _ payload: Data) {
        var len = UInt32(payload.count + 1).bigEndian
        var d = Data(bytes: &len, count: 4)
        d.append(kind)
        d.append(payload)
        lock.lock(); defer { lock.unlock() }
        FileHandle.standardOutput.write(d)
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
            self.streaming = on
            self.fps = max(1, min(60, fps))
            self.scale = max(0.1, min(1, scale))
            self.quality = max(0.3, min(0.95, quality))
            if on { self.emit() }   // a key frame for the (re)joining viewer
        }
    }

    private var lastArrival = 0.0

    private func frameArrived() {
        guard streaming else { return }
        lastArrival = now()
        let interval = 1.0 / fps
        let dt = lastArrival - lastEmit
        if dt >= interval {
            emit()
        } else {
            scheduleTrailing()
        }
    }

    /// Coalesce: deliver the last state after a burst of motion, but never a
    /// second frame inside one interval (a direct emit may have beaten the timer).
    private func scheduleTrailing() {
        guard !trailing else { return }
        trailing = true
        let wait = max(0.001, 1.0 / fps - (now() - lastEmit))
        q.asyncAfter(deadline: .now() + wait) { [weak self] in
            guard let self else { return }
            self.trailing = false
            guard self.streaming, self.lastArrival > self.lastEmit else { return }
            if now() - self.lastEmit >= 1.0 / self.fps - 0.001 { self.emit() } else { self.scheduleTrailing() }
        }
    }

    private func emit() {
        guard let s = surface(), let img = Image.cgImage(s) else { return }
        guard let jpeg = Image.encode(img, scale: scale, type: UTType.jpeg, quality: quality, crop: nil) else { return }
        lastEmit = now()
        seq += 1
        var w = UInt32(jpeg.width).bigEndian, h = UInt32(jpeg.height).bigEndian, sq = seq.bigEndian
        var payload = Data(bytes: &w, count: 4)
        payload.append(Data(bytes: &h, count: 4))
        payload.append(Data(bytes: &sq, count: 8))
        payload.append(jpeg.data)
        Out.send(UInt8(ascii: "F"), payload)
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

    func press(_ usage: UInt32, shift: Bool) throws {
        if shift { try send(key(0xE1, 1)) }
        try send(key(usage, 1)); usleep(8_000); try send(key(usage, 2))
        if shift { try send(key(0xE1, 2)) }
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
    Out.log("usage: k2-simhelper --set <device-set-path> --udid <UDID>")
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
                "developer_dir": devDir, "xcode": xcode]
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
        try needHID().press(UInt32(try num(r, "usage")), shift: (r["shift"] as? Bool) ?? false)
        return [:]
    case "button":
        try needHID().hardwareButton(try str(r, "name"))
        return [:]
    case "frames":
        try needFrames().configure(on: (r["on"] as? Bool) ?? true, fps: (r["fps"] as? NSNumber)?.doubleValue ?? 30,
                                   scale: (r["scale"] as? NSNumber)?.doubleValue ?? (1.0 / screen.scale),
                                   quality: (r["quality"] as? NSNumber)?.doubleValue ?? 0.7)
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

Out.json(["event": "ready", "version": HELPER_VERSION, "xcode": xcode])
while let line = readLine(strippingNewline: true) {
    guard !line.isEmpty else { continue }
    guard let data = line.data(using: .utf8),
          let r = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else {
        Out.json(["ok": false, "code": "bad_json", "error": "request is not a JSON object"]); continue
    }
    let id = r["id"] ?? NSNull()
    let t0 = now()
    do {
        var reply = try handle(r)
        reply["id"] = id; reply["ok"] = true; reply["ms"] = Int(((now() - t0) * 1000).rounded())
        Out.json(reply)
    } catch let e as HelperError {
        Out.json(["id": id, "ok": false, "code": e.code, "error": e.message])
    } catch {
        Out.json(["id": id, "ok": false, "code": "error", "error": "\(error)"])
    }
}
exit(0)   // stdin closed: the daemon went away
