//! A Sew that is up, for tests that mint chat passes (Sew 0.3.1 readiness,
//! `k2_daemon::sew::health::mint_gate`).
//!
//! A pass or owner-handoff mint probes `GET /api/v1/health` on Sew's
//! loopback client listener first and refuses with 503 `sew_unavailable`
//! unless Sew says it is ready. Tests that seed a running `sew_server` row
//! (what `k2 hostchat enable` leaves) also need something answering that
//! probe, as a real install would. [`serve`] starts a loopback listener
//! that answers every request `200 {"ready":true}` and records it as Sew's
//! client URL (the same link state S29 writes after enable).
//!
//! Call it after the test's temp `$HOME` is set: the URL lives under it.

use std::io::{Read, Write};
use std::net::TcpListener;

/// Start the listener on a free loopback port and point K2 at it. The
/// thread lives until the test process ends.
pub fn serve() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the Sew health stand-in");
    let port = listener.local_addr().expect("stand-in address").port();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut s) = conn else { continue };
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let body = br#"{"ready":true}"#;
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(body);
        }
    });
    k2_daemon::chat_pass::link::set_client_url(&format!("http://127.0.0.1:{port}")).expect("record Sew's client url");
}
