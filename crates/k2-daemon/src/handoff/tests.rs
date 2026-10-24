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
        apps_follow: false,
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
    let new = serde_json::to_string(&Msg::Hello { pid: 7, version: "0.46.1".into(), roles: Some(Role::ALL.to_vec()), app_listeners: false }).unwrap();
    assert!(new.contains(r#""roles":["main","tunnelIngress","tunnelTls","front443"]"#), "{new}");
    assert!(!new.contains("app_listeners"), "a successor without App listeners says nothing new: {new}");
}

/// R19 (update continuity, 0.46.4), the wire: a 0.46.3 HELLO and TAKE parse
/// with no App listeners (nobody waits for any); a 0.46.4 pair carries the
/// flags.
#[test]
fn app_listener_flags_are_absent_for_old_peers() {
    match serde_json::from_slice::<Msg>(br#"{"t":"hello","pid":7,"version":"0.46.3","roles":["main"]}"#).unwrap() {
        Msg::Hello { app_listeners, .. } => assert!(!app_listeners),
        other => panic!("{other:?}"),
    }
    match serde_json::from_slice::<Msg>(br#"{"t":"take","roles":["main"]}"#).unwrap() {
        Msg::Take { apps_follow, .. } => assert!(!apps_follow, "an old predecessor sends no App listeners"),
        other => panic!("{other:?}"),
    }
    let h = serde_json::to_string(&Msg::Hello { pid: 7, version: "0.46.4".into(), roles: None, app_listeners: true }).unwrap();
    assert!(h.contains(r#""app_listeners":true"#), "{h}");
    let t = serde_json::to_string(&Msg::Take { roles: vec![], front_routes: None, apps_follow: true }).unwrap();
    assert!(t.contains(r#""apps_follow":true"#), "{t}");
}

/// R19 (T10): App listeners cross the socketpair in messages of at most 8
/// fds (here 19 = 3 messages), each the SAME socket on the other side; a
/// successor that receives none (a legacy predecessor) has nothing to take.
#[cfg(unix)]
#[test]
fn app_listeners_cross_in_messages_of_eight() {
    use std::os::unix::net::UnixStream;
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let (a, b) = UnixStream::pair().expect("socketpair");
    let mut held = Vec::new();
    let mut ports = Vec::new();
    for i in 0..19 {
        let std_l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        ports.push(std_l.local_addr().unwrap().port());
        std_l.set_nonblocking(true).unwrap();
        let l = {
            let _g = rt.enter();
            Arc::new(tokio::net::TcpListener::from_std(std_l).unwrap())
        };
        let kind = if i % 2 == 0 { AppKind::Front } else { AppKind::Egress };
        held.push((AppSock { app: format!("r19-app-{i}"), kind }, l));
    }
    let t = std::thread::spawn(move || receive_app_listeners(&b));
    assert_eq!(send_app_listeners(&a, &held).unwrap(), 19);
    drop(held);
    assert_eq!(t.join().unwrap().unwrap(), 19);
    for (i, port) in ports.iter().enumerate() {
        let kind = if i % 2 == 0 { AppKind::Front } else { AppKind::Egress };
        let l = take_inherited_app(&format!("r19-app-{i}"), kind).expect("inherited");
        assert_eq!(l.local_addr().unwrap().port(), *port, "the same socket, still bound after the sender closed its copy");
        assert!(take_inherited_app(&format!("r19-app-{i}"), kind).is_none(), "taken once");
    }
    assert!(take_inherited_app("r19-never-sent", AppKind::Front).is_none());
}
