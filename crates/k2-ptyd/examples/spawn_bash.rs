//! Launch ptyd, connect as its owner, run bash in a session, then attach
//! from offset 0 and read everything the session printed.
//!
//! ```text
//! cargo build -p k2-ptyd --bins
//! cargo run -p k2-ptyd --example spawn_bash            # finds target/<profile>/k2-ptyd
//! cargo run -p k2-ptyd --example spawn_bash -- /path/to/k2-ptyd
//! ```
//!
//! It uses a fresh state home under the temp dir and shuts that ptyd down
//! at the end.

#[cfg(unix)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Read;
    use std::path::PathBuf;

    use k2_ptyd::client::Client;
    use k2_ptyd::launch;
    use k2_ptyd::paths::Paths;
    use k2_ptyd::proto::{Reply, Request};

    // The server binary: the first argument, else `k2-ptyd` in the target
    // dir this example was built into (`target/<profile>/examples/..`).
    let exe = match std::env::args_os().nth(1) {
        Some(p) => PathBuf::from(p),
        None => std::env::current_exe()?.parent().and_then(|d| d.parent()).ok_or("no target dir")?.join("k2-ptyd"),
    };

    // 1. Launch. `spawn_detached` double-forks the server out of this
    //    process and returns once it listens. The state home holds the
    //    socket, key, lock and log.
    let home = std::env::temp_dir().join(format!("ptyd-example-{}", std::process::id()));
    std::fs::create_dir_all(&home)?;
    launch::spawn_detached(&exe, &home, &[])?;

    // 2. Connect as the owner (the one control client).
    let paths = Paths::from_home(&home);
    let mut owner = Client::connect_owner(&paths, "example-0.1.0", true)?;

    // 3. Spawn bash on a fresh 80x24 PTY. The child gets exactly `env`.
    let reply = owner.request(Request::Spawn {
        sid: "demo".into(),
        argv: vec!["/bin/bash".into(), "--noprofile".into(), "--norc".into(), "-c".into(), "echo hello from ptyd".into()],
        env: vec![("PATH".into(), "/usr/bin:/bin".into()), ("TERM".into(), "xterm-256color".into())],
        cwd: None,
        cols: 80,
        rows: 24,
        ring_bytes: None,
        meta: serde_json::json!({}),
    })?;
    let Reply::Spawned { pid, .. } = reply else {
        return Err(format!("spawn failed: {reply:?}").into());
    };
    println!("bash is pid {pid}");

    // 4. Wait for it to exit. ptyd kept its output in the session's ring.
    loop {
        if let Reply::Exited { sid, code, .. } = owner.next_event()? {
            println!("{sid} exited with {code:?}");
            break;
        }
    }

    // 5. Attach from offset 0: the stream replays the ring from its start.
    //    For an exited session it ends after the backlog.
    let mut stream = owner.attach("demo", 0)?;
    let mut out = Vec::new();
    stream.read_to_end(&mut out)?;
    print!("{}", String::from_utf8_lossy(&out));

    // 6. Stop this ptyd, then remove its state home (best effort: the
    //    server may still be closing its log as it exits).
    let reply = owner.request(Request::Shutdown { end_sessions: true })?;
    if reply != Reply::Ok {
        return Err(format!("shutdown failed: {reply:?}").into());
    }
    let _ = std::fs::remove_dir_all(&home);
    Ok(())
}

#[cfg(not(unix))]
fn main() {
    eprintln!("ptyd runs on macOS and Linux only");
}
