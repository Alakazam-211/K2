//! The door's parsing, without Postgres: routes, bodies (fixed, chunked,
//! multipart), ranges and heads. The real-Postgres tests drive whole
//! requests (`storage_real_pg.rs`).

use super::*;

fn r(m: &str, p: &str) -> Route {
    route(m, p).unwrap_or_else(|e| panic!("{m} {p}: {e}"))
}

#[test]
fn routes_match_supabase_js_calls() {
    assert_eq!(r("POST", "/object/photos/org1/sub1/p1.jpg"), Route::Object("photos".into(), "org1/sub1/p1.jpg".into()));
    assert_eq!(r("GET", "/object/authenticated/photos/a/b.png"), Route::Object("photos".into(), "a/b.png".into()));
    assert_eq!(r("DELETE", "/object/photos"), Route::Remove("photos".into()));
    assert_eq!(r("POST", "/object/list/photos"), Route::List("photos".into()));
    assert_eq!(r("POST", "/object/move"), Route::Move);
    assert_eq!(r("POST", "/object/copy"), Route::Copy);
    assert_eq!(r("POST", "/object/sign/photos/a/b.png"), Route::SignOne("photos".into(), "a/b.png".into()));
    assert_eq!(r("POST", "/object/sign/photos"), Route::SignMany("photos".into()));
    assert_eq!(r("GET", "/object/sign/photos/a/b.png"), Route::SignedGet("photos".into(), "a/b.png".into()));
    assert_eq!(r("POST", "/object/upload/sign/photos/a.png"), Route::UploadSign("photos".into(), "a.png".into()));
    assert_eq!(r("PUT", "/object/upload/sign/photos/a.png"), Route::UploadSigned("photos".into(), "a.png".into()));
    assert_eq!(r("GET", "/object/public/avatars/u/1.png"), Route::Public("avatars".into(), "u/1.png".into()));
    assert_eq!(r("GET", "/object/info/authenticated/photos/a.png"), Route::Info("photos".into(), "a.png".into()));
    assert_eq!(r("GET", "/object/info/photos/a.png"), Route::Info("photos".into(), "a.png".into()));
    assert_eq!(r("GET", "/bucket"), Route::BucketList);
    assert_eq!(r("POST", "/bucket"), Route::BucketCreate);
    assert_eq!(r("GET", "/bucket/photos"), Route::Bucket("photos".into()));
    assert_eq!(r("POST", "/bucket/photos/empty"), Route::BucketEmpty("photos".into()));
    assert_eq!(r("GET", "/object/photos/a%20b%2Fc.png"), Route::Object("photos".into(), "a b/c.png".into()));
}

#[test]
fn what_v1_doesnt_carry_is_named_never_guessed() {
    assert_eq!(r("GET", "/render/image/authenticated/photos/a.png"), Route::Unsupported("image transformation"));
    assert_eq!(r("POST", "/upload/resumable"), Route::Unsupported("resumable (TUS) upload"));
    assert_eq!(r("GET", "/s3/photos"), Route::Unsupported("the S3 protocol"));
    let e = StorageError::unsupported("image transformation");
    assert_eq!(e.http_status(), 400);
    assert_eq!(e.error, "k2_unsupported");
    assert!(route("GET", "/nope").is_err());
    assert!(route("GET", "/object/photos").is_err(), "a bucket with no path is a remove (DELETE) only");
    assert!(route("GET", "/object/photos/%zz").is_err(), "bad escape");
}

#[test]
fn heads_and_targets() {
    assert!(is_door("/storage/v1/object/x/y?download"));
    assert!(!is_door("/storage/v10/object"));
    assert!(!is_door("/api/storage/v1/"));
    let raw = "POST /storage/v1/object/b/k HTTP/1.1\r\nHost: a\r\nContent-Length: 4\r\n\r\nBODY";
    let (h, n) = door_head(raw.as_bytes()).expect("door head");
    assert_eq!(&raw.as_bytes()[n..], b"BODY");
    let req = Req::parse(&h).expect("parse");
    assert_eq!((req.method.as_str(), req.path.as_str(), req.h("content-length")), ("POST", "/object/b/k", Some("4")));
    let q = Req::parse("GET /storage/v1/object/sign/b/k?token=a.b.c&download=x%20y.png HTTP/1.1\r\n\r\n").expect("q");
    assert_eq!(q.query.get("token").map(String::as_str), Some("a.b.c"));
    assert_eq!(q.query.get("download").map(String::as_str), Some("x y.png"));
}

#[test]
fn an_answer_head_never_carries_a_line_break_from_a_value() {
    let h = head_text(200, &[("X-Evil".into(), "a\r\nSet-Cookie: x=y".into())], 2);
    assert!(!h.contains("Set-Cookie"), "{h}");
    assert!(h.ends_with("Content-Length: 2\r\nConnection: close\r\n\r\n"));
}

async fn read_body(raw: &[u8], head: &str) -> std::io::Result<Vec<u8>> {
    let req = Req::parse(head).expect("head");
    let mut src = BufReader::new(raw);
    let mut b = Body::new(&mut src, &req);
    let mut out = Vec::new();
    b.read_to_end(&mut out).await?;
    Ok(out)
}

#[tokio::test]
async fn fixed_and_chunked_bodies() {
    let fixed = read_body(b"hello world, and the next request", "PUT /storage/v1/x HTTP/1.1\r\nContent-Length: 11\r\n\r\n").await.expect("fixed");
    assert_eq!(fixed, b"hello world");
    let chunked =
        read_body(b"5\r\nhello\r\n6;ext=1\r\n world\r\n0\r\n\r\n", "PUT /storage/v1/x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n").await.expect("chunked");
    assert_eq!(chunked, b"hello world");
    let short = read_body(b"abc", "PUT /storage/v1/x HTTP/1.1\r\nContent-Length: 10\r\n\r\n").await;
    assert!(short.is_err(), "a body shorter than its Content-Length is an error, not a short file");
    let cut = read_body(b"5\r\nhel", "PUT /storage/v1/x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n").await;
    assert!(cut.is_err(), "a cut chunked body is an error");
}

#[tokio::test]
async fn a_chunk_line_split_across_reads_is_kept() {
    // A reader that hands out one byte per read.
    struct Trickle(Vec<u8>, usize);
    impl AsyncRead for Trickle {
        fn poll_read(self: Pin<&mut Self>, _cx: &mut Context<'_>, out: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
            let me = self.get_mut();
            if me.1 < me.0.len() {
                out.put_slice(&me.0[me.1..me.1 + 1]);
                me.1 += 1;
            }
            Poll::Ready(Ok(()))
        }
    }
    let req = Req::parse("PUT /storage/v1/x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n").expect("head");
    let mut src = BufReader::with_capacity(1, Trickle(b"a\r\n0123456789\r\n0\r\n\r\n".to_vec(), 0));
    let mut b = Body::new(&mut src, &req);
    let mut out = Vec::new();
    b.read_to_end(&mut out).await.expect("trickled");
    assert_eq!(out, b"0123456789");
}

fn multipart(boundary: &str, file: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"cacheControl\"\r\n\r\n3600\r\n").as_bytes());
    v.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n{{\"k\":\"v\"}}\r\n").as_bytes());
    v.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"\"; filename=\"blob\"\r\nContent-Type: image/jpeg\r\n\r\n").as_bytes(),
    );
    v.extend_from_slice(file);
    v.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    v
}

#[tokio::test]
async fn multipart_like_supabase_js_and_dart() {
    let b = "----WebKitFormBoundary7MA4YWxkTrZu0gW";
    // A file whose bytes hold CRLFs and a near-boundary.
    let mut file = b"\xff\xd8 jpeg \r\n--not-the-boundary \r\n-".to_vec();
    file.extend(std::iter::repeat_n(0xabu8, 200_000));
    let body = multipart(b, &file);
    let mut src: &[u8] = &body;
    let mp = multipart_head(&mut src, b).await.expect("head");
    assert_eq!(mp.fields.get("cacheControl").map(String::as_str), Some("3600"));
    assert_eq!(mp.fields.get("metadata").map(String::as_str), Some("{\"k\":\"v\"}"));
    assert_eq!(mp.file_type.as_deref(), Some("image/jpeg"));
    let mut f = UntilDelim::new(&mut src, mp.delim, mp.carry);
    let mut got = Vec::new();
    f.read_to_end(&mut got).await.expect("file");
    assert_eq!(got.len(), file.len());
    assert_eq!(got, file);

    let mut none: &[u8] = b"--x\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\n1\r\n--x--\r\n";
    let e = multipart_head(&mut none, "x").await.err().expect("no file part");
    assert_eq!(e.code, "NoContentProvided");
    let mut cut: &[u8] = b"--x\r\nContent-Disposition: form-data; name=\"\"; filename=\"f\"\r\n\r\nabc";
    let mp = multipart_head(&mut cut, "x").await.expect("head");
    let mut f = UntilDelim::new(&mut cut, mp.delim, mp.carry);
    assert!(f.read_to_end(&mut Vec::new()).await.is_err(), "no closing boundary is an error, not a short file");
    assert_eq!(boundary("multipart/form-data; boundary=\"abc\"").as_deref(), Some("abc"));
    assert_eq!(boundary("multipart/form-data"), None);
}

#[test]
fn ranges() {
    assert!(matches!(parse_range("bytes=0-9", 100), RangeAsk::Part(0, 9)));
    assert!(matches!(parse_range("bytes=90-", 100), RangeAsk::Part(90, 99)));
    assert!(matches!(parse_range("bytes=-10", 100), RangeAsk::Part(90, 99)));
    assert!(matches!(parse_range("bytes=50-500", 100), RangeAsk::Part(50, 99)));
    assert!(matches!(parse_range("bytes=100-", 100), RangeAsk::Unsatisfiable));
    assert!(matches!(parse_range("bytes=0-1,5-6", 100), RangeAsk::Full));
    assert!(matches!(parse_range("items=0-1", 100), RangeAsk::Full));
    assert_eq!(http_date("2026-11-22T10:00:00.000Z").as_deref(), Some("Sun, 22 Nov 2026 10:00:00 GMT"));
}
