//! `k2-app-engine serve | version | process-run <app-id>`.
fn main() {
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
        ["process-run", _id] => {
            // The `process` exception (S13) runs through this entry point
            // from the root helper's k2app-process@.service. Not built yet:
            // fail closed, never run App code here.
            eprintln!("k2-app-engine: the process exception arrives with the open path (S13); refusing");
            78
        }
        _ => {
            eprintln!("usage: k2-app-engine serve | version");
            2
        }
    };
    std::process::exit(code);
}
