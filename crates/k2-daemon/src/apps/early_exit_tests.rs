use super::*;

#[test]
fn the_state_names_the_exit_and_the_logs_command() {
    let r = state_reason(
        "shop",
        "d-0123456789ab",
        "the instance exited before it answered GET /health (last probe: connection refused)",
        exit_text(true, Some(1), None).as_deref(),
    );
    assert_eq!(
        r,
        "the instance exited before it answered GET /health (last probe: connection refused); it exited with code 1. \
         Its output: k2 app logs shop --deployment d-0123456789ab"
    );
}

#[test]
fn no_exit_still_points_at_the_logs() {
    let r = state_reason("shop", "d-0123456789ab", "GET /health never answered 2xx within 20s (last probe: HTTP 500).", None);
    assert_eq!(r, "GET /health never answered 2xx within 20s (last probe: HTTP 500). Its output: k2 app logs shop --deployment d-0123456789ab");
}

#[test]
fn exit_words_cover_codes_signals_and_neither() {
    assert_eq!(exit_text(false, Some(1), None), None);
    assert_eq!(exit_text(true, Some(3), None).as_deref(), Some("exited with code 3"));
    assert_eq!(exit_text(true, None, Some(9)).as_deref(), Some("was killed by SIGKILL"));
    assert_eq!(exit_text(true, None, None).as_deref(), Some("exited"));
}
