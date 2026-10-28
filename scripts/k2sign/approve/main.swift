// K2 Release Approve — one Touch ID per release (prd-signing-user-v1 rev 2 §4.4, S2).
//
// Runs as the release user (z3thon), started by scripts/lib/sign-approve.sh with
// `open -W -n -a "K2 Release Approve" --args --challenge <f> --out <f>`.
// It is UNTRUSTED by design: the trust is in the Secure Enclave key (Touch ID
// or the Mac login password, asked by the system sheet on every use: the access
// control is [.privateKeyUsage, .biometryAny, .or, .devicePasscode], Rosson
// 2026-10-28; never cached, reuse window 0) and in the signer's own openssl check against
// /usr/local/etc/k2sign/approver.pem. Without a live finger on this Mac's
// sensor it can't make the signature, whoever runs it.
//
//   --enrol [--replace]          make the SE key; write ~/.k2-release-approve/{key.blob,approver.pem}
//                                (--replace first renames an existing key.blob/approver.pem to *.old-<time>)
//   --challenge <f> --out <f> [--timeout <s>]
//                                show the challenge in the approval sheet, sign its exact bytes,
//                                write the DER signature to <out> (or the reason to <out>.err);
//                                --timeout cancels the sheet after <s> seconds with no answer
//   --key-dir <dir>              (with the above) use <dir> instead of ~/.k2-release-approve
//   --capabilities               print what this build supports (release.sh refuses a build
//                                without "timeout"; no sheet, no key use)
//   --soft-selftest <dir>        S0 (c): a SOFTWARE P-256 key (no Secure Enclave, no Touch ID)
//                                signs <dir>/data; writes <dir>/pub.pem and <dir>/sig.der so
//                                LibreSSL can be checked against CryptoKit's DER format
//
// The key is CryptoKit's dataRepresentation blob in a 0600 file (no keychain,
// so no keychain entitlement: a restricted entitlement on a Developer ID build
// gets the binary killed by AMFI). The blob is useless without the finger.

import CryptoKit
import Foundation
import LocalAuthentication

let home = FileManager.default.homeDirectoryForCurrentUser
// --key-dir: the S0 spike enrols a throwaway key elsewhere (never the real one).
let keyDir: URL = {
    let a = CommandLine.arguments
    if let i = a.firstIndex(of: "--key-dir"), i + 1 < a.count { return URL(fileURLWithPath: a[i + 1]) }
    return home.appendingPathComponent(".k2-release-approve")
}()
let blobURL = keyDir.appendingPathComponent("key.blob")
let pemURL = keyDir.appendingPathComponent("approver.pem")

func fail(_ msg: String, errFile: URL? = nil) -> Never {
    FileHandle.standardError.write(("K2 Release Approve: " + msg + "\n").data(using: .utf8)!)
    if let e = errFile { try? msg.write(to: e, atomically: true, encoding: .utf8) }
    exit(1)
}

func arg(_ name: String) -> String? {
    let a = CommandLine.arguments
    guard let i = a.firstIndex(of: name), i + 1 < a.count else { return nil }
    return a[i + 1]
}

func writePrivate(_ data: Data, to url: URL) throws {
    let fm = FileManager.default
    try? fm.removeItem(at: url)
    guard fm.createFile(atPath: url.path, contents: data, attributes: [.posixPermissions: 0o600]) else {
        throw NSError(domain: "k2", code: 1, userInfo: [NSLocalizedDescriptionKey: "can't write \(url.path)"])
    }
}

func accessControl() -> SecAccessControl {
    var err: Unmanaged<CFError>?
    guard let ac = SecAccessControlCreateWithFlags(
        nil, kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
        // Touch ID (any enrolled finger, so adding a fingerprint doesn't kill
        // the key) OR the Mac login password, typed into the system sheet.
        // Spelled out rather than .userPresence, so nothing else (an Apple
        // Watch, a future factor) can satisfy it.
        [.privateKeyUsage, .biometryAny, .or, .devicePasscode], &err) else {
        fail("can't make the access control: \(String(describing: err?.takeRetainedValue()))")
    }
    return ac
}

// A fresh context for every run: nothing is pre-evaluated or reused, so the
// key operation itself puts up the system sheet (Touch ID, or "Use Password…").
// The sheet appears on the Mac's screen even when this process was started
// over SSH (that is how release.sh's `open -W` works from SSH too): the
// boundary is the person answering that sheet, never how the helper was
// launched. A same-user process can run its own copy of this helper anyway,
// so a launch-path check would prove nothing.
func approvalContext(reason: String) -> LAContext {
    let ctx = LAContext()
    ctx.localizedReason = reason
    ctx.touchIDAuthenticationAllowableReuseDuration = 0
    ctx.interactionNotAllowed = false
    var err: NSError?
    guard ctx.canEvaluatePolicy(.deviceOwnerAuthentication, error: &err) else {
        fail("this Mac can't show the approval sheet (\(err?.localizedDescription ?? "unknown")); approve at the Mac's screen")
    }
    return ctx
}

func enrol() {
    guard SecureEnclave.isAvailable else { fail("this Mac has no Secure Enclave") }
    let fm = FileManager.default
    if fm.fileExists(atPath: blobURL.path) {
        guard CommandLine.arguments.contains("--replace") else {
            fail("already enrolled (\(blobURL.path)); to replace it run --enrol --replace, then sudo k2sign-admin set-approver with the new approver.pem")
        }
        let stamp = Int(Date().timeIntervalSince1970)
        for u in [blobURL, pemURL] where fm.fileExists(atPath: u.path) {
            let old = u.deletingLastPathComponent().appendingPathComponent(u.lastPathComponent + ".old-\(stamp)")
            do { try fm.moveItem(at: u, to: old) } catch { fail("can't move \(u.path) aside: \(error.localizedDescription)") }
            print("moved the old \(u.lastPathComponent) to \(old.path)")
        }
    }
    do {
        try fm.createDirectory(at: keyDir, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let ctx = approvalContext(reason: "enrol the K2 release approver key (Touch ID or your login password)")
        let key = try SecureEnclave.P256.Signing.PrivateKey(accessControl: accessControl(), authenticationContext: ctx)
        // Prove the key works with a finger before anyone trusts its public half.
        let probe = "k2-release-approve enrol \(Date().timeIntervalSince1970)".data(using: .utf8)!
        let sig = try key.signature(for: probe)
        guard key.publicKey.isValidSignature(sig, for: probe) else { fail("the new key's test signature does not verify") }
        try writePrivate(key.dataRepresentation, to: blobURL)
        try key.publicKey.pemRepresentation.data(using: .utf8)!.write(to: pemURL)
        print("Enrolled. Public key: \(pemURL.path)")
        print("Next: sudo /usr/local/libexec/k2sign/k2sign-admin set-approver \(pemURL.path)")
    } catch {
        fail("enrol failed: \(error.localizedDescription)")
    }
}

final class Flag {
    private let lock = NSLock()
    private var value = false
    func set() { lock.lock(); value = true; lock.unlock() }
    var isSet: Bool { lock.lock(); defer { lock.unlock() }; return value }
}

func approve(challengePath: String, outPath: String) {
    let out = URL(fileURLWithPath: outPath)
    let errFile = URL(fileURLWithPath: outPath + ".err")
    guard let data = FileManager.default.contents(atPath: challengePath), data.count < 16384 else {
        fail("can't read the challenge \(challengePath)", errFile: errFile)
    }
    guard let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          obj["kind"] as? String == "k2-release-approval",
          let version = obj["version"] as? String,
          let base = obj["base_commit"] as? String,
          let assets = obj["assets"] as? [String],
          let expires = obj["expires"] as? String else {
        fail("the challenge is not a K2 release approval", errFile: errFile)
    }
    let windows = (obj["windows"] as? Bool) ?? false
    let re = (obj["reapproval"] as? Bool) ?? false
    let until = expires.replacingOccurrences(of: "T", with: " ").replacingOccurrences(of: "Z", with: " UTC")
    var reason = "\(re ? "RE-APPROVE: " : "")approve K2 release \(version) (base \(base.prefix(7))): sign \(assets.count) update files until \(until)"
    if windows { reason += ", including the Windows setup (no independent check)" }
    guard let blob = FileManager.default.contents(atPath: blobURL.path) else {
        fail("not enrolled (no \(blobURL.path)); open K2 Release Approve with --enrol first", errFile: errFile)
    }
    let timeout = Double(arg("--timeout") ?? "0") ?? 0
    let ctx = approvalContext(reason: reason)
    let timedOut = Flag()
    if timeout > 0 {
        // No answer in time: cancel the sheet; the signature call then fails.
        DispatchQueue.global().asyncAfter(deadline: .now() + timeout) {
            timedOut.set()
            ctx.invalidate()
        }
    }
    let t0 = Date()
    do {
        let key = try SecureEnclave.P256.Signing.PrivateKey(dataRepresentation: blob, authenticationContext: ctx)
        // Signs SHA-256(challenge bytes): what `openssl dgst -sha256 -verify` checks.
        let sig = try key.signature(for: data)
        ctx.invalidate()
        if timedOut.isSet { fail("no approval (timed out after \(Int(timeout)) s)", errFile: errFile) }
        try writePrivate(sig.derRepresentation, to: out)
        print("Approved after \(Int(Date().timeIntervalSince(t0) * 1000)) ms: \(reason)")
    } catch {
        ctx.invalidate()
        if timedOut.isSet { fail("no approval (timed out after \(Int(timeout)) s)", errFile: errFile) }
        fail("no approval: \(error.localizedDescription)", errFile: errFile)
    }
}

func softSelftest(dir: String) {
    let d = URL(fileURLWithPath: dir)
    guard let data = FileManager.default.contents(atPath: d.appendingPathComponent("data").path) else {
        fail("no \(dir)/data")
    }
    let key = P256.Signing.PrivateKey()
    do {
        let sig = try key.signature(for: data)
        try sig.derRepresentation.write(to: d.appendingPathComponent("sig.der"))
        try key.publicKey.pemRepresentation.data(using: .utf8)!.write(to: d.appendingPathComponent("pub.pem"))
        print("wrote \(dir)/pub.pem and \(dir)/sig.der")
    } catch {
        fail("soft self-test failed: \(error.localizedDescription)")
    }
}

// Bump when a release depends on a new behaviour (Step 0.46 checks "timeout").
let capabilities = "k2-release-approve 2 timeout replace key-dir"

let args = CommandLine.arguments
if args.contains("--capabilities") {
    print(capabilities)
    exit(0)
} else if args.contains("--enrol") {
    enrol()
} else if let dir = arg("--soft-selftest") {
    softSelftest(dir: dir)
} else if let c = arg("--challenge"), let o = arg("--out") {
    approve(challengePath: c, outPath: o)
} else {
    fail("usage: --enrol [--replace] | --challenge <file> --out <file> | --soft-selftest <dir>  [--key-dir <dir>]")
}
