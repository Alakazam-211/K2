//! Whether send-feedback is hidden from this caller while agents may not
//! report, as the shell CLI asks before `k2 help` (the daily tier) and
//! `k2 study send-feedback` (`_sf_agents_off_here` and
//! `_sf_agents_off_probe`, `cli/src/send-feedback.sh`). Hidden, the daily
//! tier leaves out its block (`cli/text/help/_daily-feedback.txt`) and the
//! study page is one line ([`OFF_LINE`]).
//!
//! The shell's rule, for a page:
//!
//! - No passport (`SCOPED_TOKEN`): shown, and nothing is asked.
//! - A passport and the cell socket a socket: one `GET
//!   /cli/product-feedback/settings` there, the passport as Bearer.
//! - Else, a port: the same GET over TCP, the passport as Bearer.
//! - Else shown, nothing asked.
//!
//! One try, connect 1 s, at most 1 s, as for the
//! permission cards probe ([`crate::cards`]), which asks the same way:
//! [`ask`] is [`cards::ask`], with its rules for when the page goes to the
//! shell instead. Hidden only for a 200 whose body the shell's Python
//! reads as a JSON object with `"agents"` exactly `false` and
//! `"senderKind"` exactly `"agent"`; anything else shows everything and
//! prints nothing. The answer
//! is kept for the rest of the process ([`hidden`]).
//!
//! With no `python3` the shell's probe can't read any answer, so it always
//! shows. The binary's own probe reads the answer itself (K2's ruling, the
//! named difference `feedback-optin-without-python3` in
//! `scripts/cli-parity.sh`), as the cards probe does.

use crate::cards::{self, Ask, Decode, Reader};
use crate::offline::{Env, Prelude};
use crate::out::json::Value;
use crate::transport::Client;
use k2_client::{cli_name, product_name};
use std::sync::OnceLock;

/// With a passport, `k2`, `k2 help`, `k2 --help` (and the other forms of
/// the daily tier) and `k2 study send-feedback` ask the daemon whether
/// agents may report. True: the binary asks the same way and prints the
/// page for the answer. False: the shell answers them; without a
/// passport they stay the binary's (nothing is asked).
pub const FEEDBACK_HIDE_NATIVE: bool = true;

/// The study page while hidden (`_sf_off_line`).
pub const OFF_LINE: &str = concat!(
    "Agents don't report ",
    product_name!(),
    " problems on this server: the owner or an admin turns it on with ",
    cli_name!(),
    " send-feedback settings --agents cli|all (or ",
    cli_name!(),
    " setup agent-reports)."
);

/// The answer, once asked: the shell keeps it for the process
/// (`_K2_SF_AGENTS_OFF`).
static HIDDEN: OnceLock<bool> = OnceLock::new();

/// What the shell's probe will do in `env`, or `None`: the page goes to
/// the shell. The same request rules as [`cards::ask`].
pub fn ask(env: &Env, p: &Prelude, would_refuse: impl FnOnce() -> bool) -> Option<Ask> {
    cards::ask(env, p, would_refuse)
}

/// Whether the page hides send-feedback: the answer this process already
/// has, else `ask`'s request sent and read.
pub fn hidden(ask: Ask) -> bool {
    match ask {
        // Nothing to ask: shown, and nothing kept.
        Ask::Off => false,
        Ask::Request { client, reader } => *HIDDEN.get_or_init(|| read(client, reader)),
    }
}

fn read(mut client: Box<Client>, reader: Reader) -> bool {
    match client.feedback_settings() {
        Ok(r) => {
            r.status == 200
                && match reader {
                    Reader::Python(decode) => agents_off(&r.body, decode),
                    Reader::ShellWithout => false,
                    Reader::Own => agents_off(&r.body, Decode::Lossy),
                }
        }
        Err(_) => false,
    }
}

/// The shell's Python on the body: hidden only for a `dict` whose
/// `"agents"` is `False` and whose `"senderKind"` is the string
/// `"agent"`.
pub fn agents_off(body: &[u8], decode: Decode) -> bool {
    cards::object(body, decode).is_some_and(|o| {
        cards::get(&o, "agents") == Some(&Value::Bool(false))
            && matches!(cards::get(&o, "senderKind"), Some(Value::Str(s)) if s.as_str() == Some("agent"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn src(file: &str) -> String {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../cli/src")
            .join(file);
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
    }

    #[test]
    fn hidden_only_for_an_agent_while_agents_are_off() {
        let off = |b: &str| agents_off(b.as_bytes(), Decode::Strict);
        assert!(off(r#"{"agents":false,"senderKind":"agent"}"#));
        assert!(off(
            " {\"enabled\": true, \"agents\": false, \"canSend\": false, \"why\": \"needs_connect\", \"senderKind\": \"agent\", \"x\": [NaN]}\n"
        ));
        assert!(off(
            r#"{"agents":true,"senderKind":"agent","agents":false}"#
        ));
        assert!(off(r#"{"agents":false,"senderKind":"\u0061gent"}"#));
        for shown in [
            "",
            "{}",
            r#"{"agents":true,"senderKind":"agent"}"#,
            r#"{"agents":false,"senderKind":"person"}"#,
            r#"{"agents":false}"#,
            r#"{"senderKind":"agent"}"#,
            r#"{"agents":0,"senderKind":"agent"}"#,
            r#"{"agents":null,"senderKind":"agent"}"#,
            r#"{"agents":"false","senderKind":"agent"}"#,
            r#"{"agents":false,"senderKind":"Agent"}"#,
            r#"{"agents":false,"senderKind":["agent"]}"#,
            r#"{"agents":false,"senderKind":"agent","agents":true}"#,
            r#"[{"agents":false,"senderKind":"agent"}]"#,
            r#"{"agents":false,"senderKind":"agent"}x"#,
            "\u{feff}{\"agents\":false,\"senderKind\":\"agent\"}",
            "\n000{\"agents\":false,\"senderKind\":\"agent\"}",
        ] {
            assert!(!off(shown), "{shown:?}");
        }
        assert!(agents_off(
            b"{\"agents\":\0false,\"senderKind\":\"agent\"}",
            Decode::Strict
        ));
        let bad = b"{\"agents\":false,\"senderKind\":\"agent\",\"x\":\"\xff\"}";
        assert!(!agents_off(bad, Decode::Strict));
        assert!(agents_off(bad, Decode::Lossy));
    }

    /// The shell's probe reads exactly what [`agents_off`] reads, and asks
    /// as the cards probe asks but for its route and its time limit.
    #[test]
    fn the_probe_is_the_shell_s() {
        let sf = src("send-feedback.sh");
        for line in [
            "        if [ -n \"${SCOPED_TOKEN:-}\" ] && _sf_agents_off_probe; then\n",
            "    if [ -n \"$sock\" ] && [ -S \"$sock\" ] && ! _cli_skip_uds; then\n",
            "        _k2_cfg_add header \"Authorization: Bearer ${SCOPED_TOKEN}\"\n",
            "        out=\"$(_K2_CURL_TRIES=1 _k2_curl \"$_K2_CFG\" -s -G --unix-socket \"$sock\" \"http://localhost/cli/product-feedback/settings\" \\\n            --connect-timeout 1 --max-time 1 -w $'\\n%{http_code}' 2>/dev/null)\" || out=\"\"\n",
            "    elif [ -n \"${PORT:-}\" ]; then\n        _k2_cfg_auth \"$SCOPED_TOKEN\"\n",
            "        local base\n        _k2_daemon_base base; out=\"$(_K2_CURL_TRIES=1 _k2_curl \"$_K2_CFG\" -s -G \"${base}/cli/product-feedback/settings\" \\\n            --connect-timeout 1 --max-time 1 -w $'\\n%{http_code}' 2>/dev/null)\" || out=\"\"\n",
            "    [[ \"$out\" == *$'\\n200' ]] || return 1\n    out=\"${out%$'\\n200'}\"\n    printf '%s' \"$out\" | python3 -c '\n",
            "sys.exit(0 if isinstance(v, dict) and v.get(\"agents\") is False and v.get(\"senderKind\") == \"agent\" else 1)\n",
        ] {
            assert!(sf.contains(line), "send-feedback.sh moved: {line}");
        }
        assert_eq!(
            crate::transport::client::FEEDBACK_SETTINGS_ENDPOINT,
            "/cli/product-feedback/settings"
        );
        assert_eq!(
            (
                crate::transport::client::FEEDBACK_CONNECT_TIMEOUT.as_secs(),
                crate::transport::client::FEEDBACK_MAX_TIME.as_secs()
            ),
            (1, 1)
        );
    }

    /// [`OFF_LINE`] is `_sf_off_line`'s echo, `$_SF_PRODUCT` read as the
    /// shell sets it.
    #[test]
    fn the_off_line_is_the_shell_s() {
        let sf = src("send-feedback.sh");
        let product = sf
            .lines()
            .find_map(|l| l.strip_prefix("_SF_PRODUCT=\""))
            .and_then(|l| l.strip_suffix('"'))
            .expect("_SF_PRODUCT=\"...\"");
        assert_eq!(product, product_name!());
        let body = sf
            .split_once("\n_sf_off_line() {\n")
            .expect("_sf_off_line")
            .1;
        let body = &body[..body.find("\n}\n").expect("its end")];
        let echo = body
            .trim()
            .strip_prefix("echo \"")
            .and_then(|l| l.strip_suffix('"'))
            .expect("one echo of a double-quoted line");
        assert!(
            !echo.contains(['\\', '`']) && echo.matches('$').count() == 1,
            "a line this test can't read as bash does: {echo}"
        );
        assert_eq!(OFF_LINE, echo.replace("$_SF_PRODUCT", product));
        // The study page's hide: before the page, the line, exit 0.
        assert!(src("board.sh").contains(
            "_study_page_send_feedback() {\n    # 0.46.17: one line for an agent while agents may not report, so one\n    # that knows the command doesn't loop (_sf_agents_off_here).\n    if _sf_agents_off_here; then\n        _sf_off_line\n        return 0\n    fi\n    cat <<'EOF'\n%include cli/text/study/send-feedback.txt\nEOF\n}\n"
        ));
    }
}
