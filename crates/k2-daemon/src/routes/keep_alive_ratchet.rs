//! Keep-alive ratchet. The request head is only PEEKED before routing, so
//! every arm that answers and keeps the socket open must consume it
//! (`stream.read`, `read_post_body`, or `require_post`, which reads the
//! head of a refused GET). An arm that forgets is answered again on the
//! next loop pass, forever, and every later request on that socket reads
//! the wrong body. That was the 0.43.0 `/cli/power/status` bug.
//!
//! This walks the route match (`match path.as_str()`): for each arm, whatever
//! its pattern (`"/path"`, `p if …`, `p @ (…)`, `crate::…::PATH`, the last
//! line of a multi-line or-pattern), it drops the `if` blocks that answer
//! and `return DispatchOutcome::Done` (they close the socket, so a leftover
//! head is harmless), and fails with the arm's line when what is left sends
//! a response without consuming the request. Two reads don't count for a
//! GET: a `require_post` behind another condition (`if p != … &&
//! !require_post(…)` reads only on the POST rows) and an `if is_post { … }`
//! with no `else`. The 0.46.12 `/cli/git-tools` arm had both, and its
//! `p @ (…)` head was not scanned at all.

const SOURCE: &str = include_str!("dispatcher.rs");
const CONSUMES: [&str; 6] = [
    "stream.read(",
    "read_post_body(",
    // 0.45.5: the upload-chunk arm reads its body with a size cap.
    "read_post_body_capped(",
    "read_exact(",
    "require_post(",
    // 0.44.4: drains a GET or reads a POST body (Content-Length framed).
    "read_request_params(",
];

fn block_end(lines: &[&str], start: usize) -> usize {
    let mut depth: i64 = 0;
    for (j, line) in lines.iter().enumerate().skip(start) {
        depth += line.matches('{').count() as i64 - line.matches('}').count() as i64;
        if depth <= 0 && j > start {
            return j;
        }
    }
    lines.len() - 1
}

/// The route match's first line; its arms sit one indent deeper.
const ROUTE_MATCH: &str = "    match path.as_str() {";

/// An arm of the route match: any line at the arm indent that opens
/// the arm's block, whatever the pattern.
fn is_arm_head(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("        ") else {
        return false;
    };
    !rest.starts_with(' ') && rest.trim_end().ends_with("=> {")
}

/// A `require_post` behind another condition: it reads (and refuses)
/// only the POST rows' GETs, so it consumes nothing for the arm's GET
/// rows. Single-line (`if cond && !…require_post(…) {`) or a
/// continuation line (`&& !…require_post(…)`).
fn is_guarded_post_gate(s: &str) -> bool {
    let Some((before, _)) = s.split_once("require_post(") else {
        return false;
    };
    (s.starts_with("if ") && (before.contains("&&") || before.contains("||")))
        || s.starts_with("&& ")
        || s.starts_with("|| ")
}

fn arms_that_never_consume(source: &str) -> Vec<String> {
    let lines: Vec<&str> = source.lines().collect();
    let start = lines
        .iter()
        .position(|l| *l == ROUTE_MATCH)
        .unwrap_or_else(|| panic!("no `{}` line: the ratchet scans nothing", ROUTE_MATCH.trim()));
    let end = block_end(&lines, start);
    let mut bad = Vec::new();
    let mut i = start + 1;
    while i < end {
        if !is_arm_head(lines[i]) {
            i += 1;
            continue;
        }
        let arm_end = block_end(&lines, i);
        let mut kept = String::new();
        let mut j = i + 1;
        while j <= arm_end {
            let s = lines[j].trim();
            let opens_branch = (s.starts_with("if ")
                || s.starts_with("} else")
                || s.starts_with("else"))
                && s.ends_with('{');
            if opens_branch {
                let sub_end = block_end(&lines, j);
                let sub = lines[j..=sub_end].join("\n");
                if sub.contains("return DispatchOutcome::Done") && sub.contains("send_response") {
                    j = sub_end + 1;
                    continue;
                }
                // A POST-only read leaves a GET's head unread.
                if s == "if is_post {" && !sub.contains("} else") {
                    j = sub_end + 1;
                    continue;
                }
            }
            if is_guarded_post_gate(s) {
                j += 1;
                continue;
            }
            kept.push_str(lines[j]);
            kept.push('\n');
            j += 1;
        }
        if kept.contains("send_response") && !CONSUMES.iter().any(|k| kept.contains(k)) {
            bad.push(format!("dispatcher.rs:{}: {}", i + 1, lines[i].trim()));
        }
        i = arm_end + 1;
    }
    bad
}

#[test]
fn every_keep_alive_arm_consumes_its_request() {
    let bad = arms_that_never_consume(SOURCE);
    assert!(
        bad.is_empty(),
        "these arms answer without consuming the peeked request, so the \
         keep-alive loop answers it again forever:\n{}",
        bad.join("\n")
    );
}

/// The scan must catch the shape that shipped in 0.43.0.
#[test]
fn ratchet_catches_the_0_43_0_power_status_arm() {
    let old = [
        ROUTE_MATCH,
        "        \"/cli/power/status\" => {",
        "            if !super::http::token_ok(&query, state.token.as_str()) {",
        "                let _ = stream.read(&mut buf).await;",
        "                super::http::send_response(&mut *stream, \"403\", \"x\", \"{}\").await;",
        "                return DispatchOutcome::Done;",
        "            }",
        "            let r = handle_status();",
        "            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;",
        "        }",
        "    }",
    ]
    .join("\n");
    assert_eq!(
        arms_that_never_consume(&old),
        vec!["dispatcher.rs:2: \"/cli/power/status\" => {".to_string()]
    );
}

/// And the shape that shipped in 0.46.12: a `p @ (…)` head, a
/// `require_post` only for the POST rows, a body read only for POST.
/// Its GET answered again and again on one socket, so the request after
/// it got a git-tools body.
#[test]
fn ratchet_catches_the_0_46_12_git_tools_arm() {
    let old = [
        ROUTE_MATCH,
        "        p @ (\"/cli/git-tools\" | \"/cli/git-tools/install\") => {",
        "            if p != \"/cli/git-tools\" && !super::http::require_post(&mut *stream, &mut buf, is_post).await {",
        "                return DispatchOutcome::Done;",
        "            }",
        "            if !token_ok(&query) {",
        "                let _ = stream.read(&mut buf).await;",
        "                super::http::send_response(&mut *stream, \"403\", \"x\", \"{}\").await;",
        "                return DispatchOutcome::Done;",
        "            }",
        "            if is_post {",
        "                let _ = super::http::read_post_body(&mut *stream, &mut buf).await;",
        "            }",
        "            let r = handle_status();",
        "            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;",
        "        }",
        "        p if crate::db_home_routes::is_route(p) => {",
        "            if crate::db_home_routes::is_post_route(p)",
        "                && !super::http::require_post(&mut *stream, &mut buf, is_post).await",
        "            {",
        "                return DispatchOutcome::Done;",
        "            }",
        "            let r = handle_status();",
        "            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;",
        "        }",
        "        crate::fixed::ROUTE => {",
        "            let body = if is_post {",
        "                super::http::read_post_body(&mut *stream, &mut buf).await",
        "            } else {",
        "                let _ = stream.read(&mut buf).await;",
        "                Vec::new()",
        "            };",
        "            super::http::send_response(&mut *stream, \"200 OK\", \"x\", \"{}\").await;",
        "        }",
        "    }",
    ]
    .join("\n");
    assert_eq!(
        arms_that_never_consume(&old),
        vec![
            "dispatcher.rs:2: p @ (\"/cli/git-tools\" | \"/cli/git-tools/install\") => {".to_string(),
            "dispatcher.rs:17: p if crate::db_home_routes::is_route(p) => {".to_string(),
        ],
        "the GET-and-POST arm that reads both ways (line 26) must pass"
    );
}
