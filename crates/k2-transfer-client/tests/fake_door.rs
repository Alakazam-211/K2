//! The client against a small in-process stand-in for K2's transfer door:
//! uploads that resume and retry, downloads that resume with If-Range and
//! restart when the file changed, and refusals that keep K2's code.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use k2_transfer_client::{Client, Endpoint, Retry, UploadOptions};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const TOKEN: &str = "k2up_test.token";

#[derive(Default)]
struct Door {
    size: u64,
    data: Mutex<Vec<u8>>,
    received: Mutex<Vec<(u64, u64)>>,
    /// Answer this many PUTs with a 503 first.
    fail_puts: AtomicUsize,
    puts: AtomicUsize,
    put_bytes: AtomicUsize,
    /// The file a download serves, and its ETag.
    file: Mutex<Vec<u8>>,
    etag: Mutex<String>,
    /// Cut the next full download after this many body bytes.
    cut_after: Mutex<Option<usize>>,
    ranges_seen: Mutex<Vec<String>>,
    /// Answer every request with this refusal.
    refuse: Mutex<Option<(u16, &'static str)>>,
}

fn sha(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

fn insert(r: &mut Vec<(u64, u64)>, a: u64, b: u64) {
    r.push((a, b));
    r.sort();
    let mut out: Vec<(u64, u64)> = Vec::new();
    for (s, e) in r.drain(..) {
        match out.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    *r = out;
}

impl Door {
    fn status(&self) -> serde_json::Value {
        let rec = self.received.lock().unwrap().clone();
        let mut missing = Vec::new();
        let mut at = 0;
        for &(a, b) in &rec {
            if a > at {
                missing.push([at, a - 1]);
            }
            at = b;
        }
        if at < self.size {
            missing.push([at, self.size - 1]);
        }
        let held: u64 = rec.iter().map(|(a, b)| b - a).sum();
        serde_json::json!({
            "uploadId": "u1",
            "url": "/_k2/transfers/uploads/u1",
            "size": self.size,
            "received": rec.iter().map(|(a, b)| [*a, b - 1]).collect::<Vec<_>>(),
            "missing": missing,
            "receivedBytes": held,
            "complete": held == self.size,
            "done": false,
            "expiresAt": 1u64,
            "partMax": 16 * 1024 * 1024,
        })
    }
}

async fn reply(s: &mut TcpStream, status: &str, headers: &[(&str, String)], body: &[u8]) {
    let mut out = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    let _ = s.write_all(out.as_bytes()).await;
    let _ = s.write_all(body).await;
    let _ = s.shutdown().await;
}

async fn handle(door: Arc<Door>, mut s: TcpStream) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let head_end = loop {
        let n = s.read(&mut tmp).await.unwrap_or(0);
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.lines();
    let first: Vec<String> = lines.next().unwrap_or("").split_whitespace().map(str::to_string).collect();
    let headers: HashMap<String, String> = lines
        .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())))
        .collect();
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut body = buf[head_end..].to_vec();
    while body.len() < len {
        let n = s.read(&mut tmp).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    let (method, path) = (first[0].as_str(), first[1].as_str());
    assert_eq!(headers.get("authorization").map(String::as_str), Some(&*format!("Bearer {TOKEN}")), "the token rides every request");
    let refuse = *door.refuse.lock().unwrap();
    if let Some((status, code)) = refuse {
        let b = serde_json::json!({ "error": { "code": code, "hint": "nope" } }).to_string();
        return reply(&mut s, &format!("{status} Refused"), &[], b.as_bytes()).await;
    }
    let json = |v: serde_json::Value| v.to_string().into_bytes();
    match (method, path) {
        ("GET", "/_k2/transfers/uploads/u1") => reply(&mut s, "200 OK", &[], &json(door.status())).await,
        ("PUT", "/_k2/transfers/uploads/u1") => {
            door.puts.fetch_add(1, Ordering::SeqCst);
            if door.fail_puts.load(Ordering::SeqCst) > 0 {
                door.fail_puts.fetch_sub(1, Ordering::SeqCst);
                return reply(&mut s, "503 Busy", &[("Retry-After", "0".into())], b"{\"error\":{\"code\":\"busy\",\"hint\":\"later\"}}").await;
            }
            let cr = headers.get("content-range").expect("Content-Range");
            let (span, total) = cr.trim_start_matches("bytes ").split_once('/').unwrap();
            let (a, b) = span.split_once('-').unwrap();
            let (a, b): (u64, u64) = (a.parse().unwrap(), b.parse().unwrap());
            assert_eq!(total.parse::<u64>().unwrap(), door.size);
            assert_eq!(body.len() as u64, b - a + 1);
            assert_eq!(headers.get("k2-content-sha256"), Some(&sha(&body)), "each range carries its own sha256");
            door.put_bytes.fetch_add(body.len(), Ordering::SeqCst);
            door.data.lock().unwrap()[a as usize..=b as usize].copy_from_slice(&body);
            insert(&mut door.received.lock().unwrap(), a, b + 1);
            reply(&mut s, "200 OK", &[], &json(door.status())).await
        }
        ("POST", "/_k2/transfers/uploads/u1/complete") => {
            let want: serde_json::Value = serde_json::from_slice(&body).unwrap();
            let data = door.data.lock().unwrap().clone();
            assert_eq!(want["sha256"], sha(&data), "complete carries the file's sha256");
            let v = serde_json::json!({ "uploadId": "u1", "done": true, "relPath": "in/big.bin", "size": data.len(), "sha256": sha(&data) });
            reply(&mut s, "200 OK", &[], &json(v)).await
        }
        ("GET", "/_k2/transfers/downloads/d1") => {
            let file = door.file.lock().unwrap().clone();
            let etag = door.etag.lock().unwrap().clone();
            let range = headers.get("range").cloned();
            if let Some(r) = &range {
                door.ranges_seen.lock().unwrap().push(r.clone());
            }
            let if_range_ok = headers.get("if-range").map_or(true, |v| *v == etag);
            match range.filter(|_| if_range_ok) {
                Some(r) => {
                    let from: usize = r.trim_start_matches("bytes=").trim_end_matches('-').parse().unwrap();
                    let part = &file[from..];
                    let cr = format!("bytes {from}-{}/{}", file.len() - 1, file.len());
                    reply(&mut s, "206 Partial Content", &[("ETag", etag), ("Content-Range", cr)], part).await
                }
                None => {
                    let cut = door.cut_after.lock().unwrap().take();
                    match cut {
                        Some(n) => {
                            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: {etag}\r\nConnection: close\r\n\r\n", file.len());
                            let _ = s.write_all(head.as_bytes()).await;
                            let _ = s.write_all(&file[..n]).await;
                            let _ = s.flush().await;
                            // Drop the connection mid-body.
                        }
                        None => reply(&mut s, "200 OK", &[("ETag", etag)], &file).await,
                    }
                }
            }
        }
        _ => reply(&mut s, "404 Not Found", &[], b"{}").await,
    }
}

async fn start(door: Arc<Door>) -> String {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else { return };
            tokio::spawn(handle(door.clone(), s));
        }
    });
    format!("http://{addr}")
}

fn client() -> Client {
    Client::new().unwrap().retry(Retry { attempts: 4, base: Duration::from_millis(1), max: Duration::from_millis(5) })
}

fn tmp(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let p = std::env::temp_dir().join(format!("k2tc-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[tokio::test]
async fn an_upload_sends_only_whats_missing_retries_and_completes() {
    let data: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
    let door = Arc::new(Door { size: data.len() as u64, data: Mutex::new(vec![0; data.len()]), ..Default::default() });
    // The door already holds bytes 10000..20000 (an earlier, interrupted run).
    door.data.lock().unwrap()[10_000..20_000].copy_from_slice(&data[10_000..20_000]);
    insert(&mut door.received.lock().unwrap(), 10_000, 20_000);
    door.fail_puts.store(1, Ordering::SeqCst);
    let base = start(door.clone()).await;
    let dir = tmp("up");
    let f = dir.join("big.bin");
    std::fs::write(&f, &data).unwrap();
    let ep = Endpoint::new(&base, "/_k2/transfers/uploads/u1", TOKEN);
    let mut seen = Vec::new();
    let done = client()
        .upload_file(&ep, &f, UploadOptions { part_size: 8192, range_checksums: true }, |p| seen.push(p.done))
        .await
        .expect("upload");
    assert_eq!(done.rel_path, "in/big.bin");
    assert_eq!(*door.data.lock().unwrap(), data);
    assert_eq!(door.put_bytes.load(Ordering::SeqCst), 40_000, "the 10 000 bytes K2 held were not sent again");
    // 40 000 bytes in 8 KiB parts across two gaps is 6 PUTs, plus the 503.
    assert_eq!(door.puts.load(Ordering::SeqCst), 7);
    assert_eq!(seen.first(), Some(&10_000));
    assert_eq!(seen.last(), Some(&50_000));
}

#[tokio::test]
async fn a_download_resumes_with_if_range_after_a_cut() {
    let file: Vec<u8> = (0..70_000u32).map(|i| (i % 97) as u8).collect();
    let door = Arc::new(Door::default());
    *door.file.lock().unwrap() = file.clone();
    *door.etag.lock().unwrap() = "\"v1\"".into();
    *door.cut_after.lock().unwrap() = Some(30_000);
    let base = start(door.clone()).await;
    let dir = tmp("down");
    let dest = dir.join("report.pdf");
    let ep = Endpoint::new(&base, "/_k2/transfers/downloads/d1", TOKEN);
    let done = client().download_to(&ep, &dest, |_| {}).await.expect("download");
    assert_eq!(std::fs::read(&dest).unwrap(), file);
    assert_eq!(done.size, 70_000);
    assert_eq!(done.fetched, 40_000, "the second try fetched only the rest");
    assert_eq!(*door.ranges_seen.lock().unwrap(), vec!["bytes=30000-".to_string()]);
    assert!(!dir.join("report.pdf.part").exists() && !dir.join("report.pdf.part.etag").exists());
}

#[tokio::test]
async fn a_changed_file_restarts_the_download() {
    let file: Vec<u8> = b"the new version of the file".to_vec();
    let door = Arc::new(Door::default());
    *door.file.lock().unwrap() = file.clone();
    *door.etag.lock().unwrap() = "\"v2\"".into();
    let base = start(door.clone()).await;
    let dir = tmp("changed");
    let dest = dir.join("notes.txt");
    std::fs::write(dir.join("notes.txt.part"), b"the old ver").unwrap();
    std::fs::write(dir.join("notes.txt.part.etag"), "\"v1\"").unwrap();
    let ep = Endpoint::new(&base, "/_k2/transfers/downloads/d1", TOKEN);
    let done = client().download_to(&ep, &dest, |_| {}).await.expect("download");
    assert_eq!(std::fs::read(&dest).unwrap(), file, "no mix of versions");
    assert_eq!(done.etag.as_deref(), Some("\"v2\""));
}

#[tokio::test]
async fn a_refusal_keeps_k2s_code_and_isnt_retried() {
    let door = Arc::new(Door { size: 10, data: Mutex::new(vec![0; 10]), ..Default::default() });
    *door.refuse.lock().unwrap() = Some((401, "transfer_token_expired"));
    let base = start(door.clone()).await;
    let ep = Endpoint::new(&base, "/_k2/transfers/uploads/u1", TOKEN);
    let e = client().put_range(&ep, 0, vec![1; 10], 10, true).await.expect_err("refused");
    assert_eq!(e.code(), Some("transfer_token_expired"));
    assert!(!e.retryable());
    assert_eq!(door.puts.load(Ordering::SeqCst), 0, "refused before the handler counted it");
}
