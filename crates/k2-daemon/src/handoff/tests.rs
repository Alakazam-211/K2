use super::*;

/// The SCM_RIGHTS round trip: a listening socket sent over the socketpair
/// is the SAME socket on the other side (a connection to its port is
/// accepted through the received fd), and the JSON body survives.
#[cfg(unix)]
#[test]
fn listener_fd_round_trips_over_the_socketpair() {
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;

    let (a, b) = UnixStream::pair().expect("socketpair");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let lockfile = tempfile_in_tmp();

    let body = serde_json::to_vec(&Msg::Take {
        roles: vec![Role::Main],
        front_routes: None,
    })
    .expect("ser");
    fdpass::send_msg(&a, &body, &[lockfile.as_raw_fd(), listener.as_raw_fd()]).expect("send");
    // The sender's copies go away: the receiver's keep the socket alive.
    drop(listener);
    drop(lockfile);

    let (got, mut fds) = fdpass::recv_msg(&b, 8).expect("recv");
    match serde_json::from_slice::<Msg>(&got).expect("parse") {
        Msg::Take { roles, .. } => assert_eq!(roles, vec![Role::Main]),
        other => panic!("expected TAKE, got {other:?}"),
    }
    assert_eq!(fds.len(), 2, "lock + one listener");
    let received = std::net::TcpListener::from(fds.remove(1));
    assert_eq!(received.local_addr().expect("addr").port(), port);

    let t = std::thread::spawn(move || {
        let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
        c.write_all(b"ping").expect("write");
    });
    let (mut s, _) = received.accept().expect("accept through the received fd");
    let mut buf = [0u8; 4];
    s.read_exact(&mut buf).expect("read");
    assert_eq!(&buf, b"ping");
    t.join().expect("client");
}

#[cfg(unix)]
fn tempfile_in_tmp() -> std::fs::File {
    let p = std::env::temp_dir().join(format!("k2-handoff-lock-{}", uuid::Uuid::new_v4().simple()));
    let f = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&p)
        .expect("lock file");
    let _ = std::fs::remove_file(&p);
    f
}

/// A message with more fds than the receiver allows is refused, not
/// half-accepted (and the extra fds are closed with it).
#[cfg(unix)]
#[test]
fn too_many_fds_are_refused() {
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    let (a, b) = UnixStream::pair().expect("socketpair");
    let f1 = tempfile_in_tmp();
    let f2 = tempfile_in_tmp();
    fdpass::send_msg(&a, b"{\"t\":\"ack\"}", &[f1.as_raw_fd(), f2.as_raw_fd()]).expect("send");
    let err = fdpass::recv_msg(&b, 1).expect_err("2 fds > max 1");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

/// A peer that closes mid-protocol is an error, never an empty message.
#[cfg(unix)]
#[test]
fn closed_peer_is_an_error() {
    use std::os::unix::net::UnixStream;
    let (a, b) = UnixStream::pair().expect("socketpair");
    drop(a);
    let err = fdpass::recv_msg(&b, 0).expect_err("closed");
    assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
}

/// The drain counter follows the guards exactly.
#[test]
fn splice_guards_count_live_splices() {
    let base = live_splices();
    let g1 = splice_guard();
    let g2 = splice_guard();
    assert_eq!(live_splices(), base + 2);
    drop(g1);
    assert_eq!(live_splices(), base + 1);
    drop(g2);
    assert_eq!(live_splices(), base);
}

/// Inherited listeners are handed out once per role.
#[test]
fn inherited_listeners_are_taken_once() {
    // The tunnel listeners' tests take inherited sockets under the env
    // lock; hold it so none of them grabs this one.
    let _home = crate::test_support::TempHome::new();
    let l =std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = l.local_addr().expect("addr").port();
    inherited()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(Role::TunnelIngress, l);
    assert!(has_inherited(Role::TunnelIngress));
    let got = take_inherited(Role::TunnelIngress).expect("first take");
    assert_eq!(got.local_addr().expect("addr").port(), port);
    assert!(take_inherited(Role::TunnelIngress).is_none(), "taken once");
}

/// front443: a pre-0.46.1 successor (HELLO without `roles`) never gets a
/// role it can't parse; a 0.46.1 successor gets the front socket.
#[test]
fn the_front_socket_goes_only_to_a_successor_that_names_it() {
    let held = [Role::Main, Role::TunnelIngress, Role::TunnelTls, Role::Front443];
    assert_eq!(roles_for_successor(&held, None), Role::LEGACY.to_vec());
    assert_eq!(roles_for_successor(&held, Some(&Role::ALL)), held.to_vec());
    assert_eq!(roles_for_successor(&[Role::Main], Some(&Role::ALL)), vec![Role::Main]);
    // A 0.46.0 HELLO still parses, as "legacy".
    let old = br#"{"t":"hello","pid":7,"version":"0.46.0"}"#;
    match serde_json::from_slice::<Msg>(old).expect("old hello") {
        Msg::Hello { roles, .. } => assert_eq!(roles, None),
        other => panic!("{other:?}"),
    }
    // A 0.46.1 HELLO carries its roles, and an old parser would ignore the
    // extra field (no deny_unknown_fields on Msg).
    let new = serde_json::to_string(&Msg::Hello { pid: 7, version: "0.46.1".into(), roles: Some(Role::ALL.to_vec()) }).unwrap();
    assert!(new.contains(r#""roles":["main","tunnelIngress","tunnelTls","front443"]"#), "{new}");
}
