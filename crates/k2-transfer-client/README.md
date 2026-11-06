# k2-transfer-client

Resumable uploads and downloads against a K2 transfer door (`/_k2/transfers/` on an App's own host), for a Tauri app's Rust side or any tokio program: ranges in any order, resume from what the door holds, retry with backoff, sha256 checks. The protocol and the iOS and Android background recipes: `k2 study app-transfers`.

The crate's own code is pure Rust; TLS is rustls through reqwest with its default provider (ring), which is the standing crypto exception to the no-C rule.

License: MIT OR Apache-2.0.
