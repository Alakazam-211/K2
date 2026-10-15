//! `k2-app-engine serve | version | process-run <app-id>`.
fn main() {
    // Called through a link in the engine's `oci/` dir: the OCI runtime
    // shim (Podman runs it as `crun` / `youki`).
    let argv0 = std::env::args().next().unwrap_or_default();
    if let Some(rt) = k2_app_engine::oci::runtime_for_argv0(&argv0) {
        let rest: Vec<String> = std::env::args().skip(1).collect();
        std::process::exit(k2_app_engine::oci::main(rt, &rest));
    }
    let a: Vec<String> = std::env::args().skip(1).collect();
    let code = match a.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["serve"] => {
            // SAFETY: umask has no preconditions.
            unsafe { libc::umask(0o077) };
            match k2_app_engine::server::production().and_then(|e| {
                e.init()?;
                k2_app_engine::server::serve(e, std::path::Path::new(k2_root_proto::engine::SOCKET))
            }) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("k2-app-engine: {e}");
                    1
                }
            }
        }
        ["version"] => {
            let (marker, test) = k2_app_engine::build_strings();
            println!(
                "{}",
                serde_json::json!({"name": "k2-app-engine", "version": env!("K2_PRODUCT_VERSION"), "protocol": k2_root_proto::engine::ENGINE_PROTOCOL, "build": marker, "testKeys": test})
            );
            0
        }
        // The `process` exception: inside k2app-process@<id>.service, as
        // k2appx (process.rs). Returns only on failure.
        ["process-run", id] => k2_app_engine::process::run(id),
        _ => {
            eprintln!("usage: k2-app-engine serve | version | process-run <app-id>");
            2
        }
    };
    std::process::exit(code);
}
