//! Which of an agent's rules a new message meets (prd §4.6, §5.2, vs-live
//! W26–W29). Pure, so the cases are tested without a mail server.

use crate::mail::jmap::MailAddr;
use crate::mail::messages::{auth_header_from, parse_auth_results, AuthAuthority};
use crate::mail::watch::store::{Vip, VipKind, Wake};

/// What the agent hears about a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Class {
    /// Interrupt: Thread card and a line in the session.
    Vip { auth: String },
    /// The next digest. `flag` says why it isn't a VIP when it claimed to be.
    Digest { flag: Option<String> },
    /// Not for this agent (a VIP-only inbox, an automated mail, its own).
    Ignored { reason: &'static str },
}

/// What the rules see about one message.
#[derive(Debug, Clone)]
pub struct Facts<'a> {
    pub from: &'a [MailAddr],
    pub auth_lines: &'a [String],
    pub authority: &'a AuthAuthority,
    pub auto_submitted: bool,
    /// Sent from one of this agent's own addresses.
    pub own_mail: bool,
    /// The sender is a mailbox on this box (mail between them gets no stamp).
    pub sender_on_this_box: bool,
    /// That sender's mailbox has an agent-created credential (W26): a local
    /// VIP then trusts every agent holding it, so it doesn't count.
    pub sender_has_agent_credential: bool,
}

fn domain_of(addr: &str) -> Option<String> {
    addr.rsplit_once('@').map(|(_, d)| d.trim().to_ascii_lowercase())
}

/// Exact domain, or `*.example.com` for its subdomains (W28: `@acme.com`
/// does not match `mail.acme.com`).
fn domain_matches(pattern: &str, domain: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(base) => domain.ends_with(&format!(".{base}")),
        None => pattern == domain,
    }
}

/// An entry that names this sender. A domain entry is checked again against
/// the DMARC-authenticated domain in [`vip_authentic`].
fn names_sender(vip: &Vip, sender: &str) -> bool {
    let sender = sender.trim().to_ascii_lowercase();
    match vip.kind {
        VipKind::Address => sender == vip.value,
        VipKind::Domain => domain_of(&sender).is_some_and(|d| domain_matches(&vip.value, &d)),
        // Person entries are not offered yet (no verified address to resolve
        // them to); an entry that existed would match nobody.
        VipKind::Person => false,
    }
}

/// DMARC said pass for this sender's domain, in the topmost header from the
/// server we trust. `Some(words)` for the stamp; `None` = not authenticated.
fn vip_authentic(f: &Facts<'_>, sender: &str) -> Option<String> {
    let dmarc_pass = {
        let parsed = parse_auth_results(f.auth_lines, f.authority);
        parsed["dmarc"].as_str() == Some("pass")
    };
    if dmarc_pass {
        // W28: the header-From domain, not whatever the AR line claims. An
        // `header.from` naming another domain vouches for nobody.
        let domain = domain_of(sender)?;
        let aligned = match f.auth_lines.first().and_then(|l| auth_header_from(l)) {
            Some(hf) => hf.eq_ignore_ascii_case(&domain),
            None => true,
        };
        return aligned.then(|| "DMARC pass".to_string());
    }
    if f.sender_on_this_box && !f.sender_has_agent_credential {
        // Mail between mailboxes on this server has no stamp of its own
        // (W26). A forged From from outside carries one, and it failed above.
        let no_stamp = parse_auth_results(f.auth_lines, f.authority)["dmarc"].is_null();
        if no_stamp {
            return Some("this server".to_string());
        }
    }
    None
}

/// Classify a message for one agent. Rule order is the PRD's.
pub fn classify(f: &Facts<'_>, vips: &[Vip], wake: Wake) -> Class {
    if f.own_mail {
        return Class::Ignored { reason: "own_mail" };
    }
    // RFC 3834: a mail marked as sent by a machine is never a wake (and
    // this is what stops two auto-replying mailboxes looping).
    if f.auto_submitted {
        return Class::Ignored { reason: "auto_submitted" };
    }
    // A message that doesn't name exactly one sender can't be vouched for.
    if let [sender] = f.from {
        if vips.iter().any(|v| names_sender(v, &sender.email)) {
            return match vip_authentic(f, &sender.email) {
                Some(auth) => Class::Vip { auth },
                None => match wake {
                    // Claimed to be a VIP and couldn't prove it. On a
                    // VIP-only inbox that means nothing: it's just not a VIP.
                    Wake::Vip => Class::Ignored { reason: "claimed_vip_failed_auth" },
                    Wake::All => Class::Digest { flag: Some("claimed VIP, failed authentication".to_string()) },
                },
            };
        }
    }
    match wake {
        Wake::Vip => Class::Ignored { reason: "not_vip" },
        Wake::All => Class::Digest { flag: None },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(e: &str) -> MailAddr {
        MailAddr { name: None, email: e.to_string() }
    }
    fn host() -> AuthAuthority {
        AuthAuthority::ThisBox(Some("mx.lztek.io".to_string()))
    }
    fn lines(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }
    fn vip(v: &str) -> Vip {
        match v.strip_prefix('@') {
            Some(d) => Vip { kind: VipKind::Domain, value: d.to_string() },
            None => Vip { kind: VipKind::Address, value: v.to_string() },
        }
    }

    fn run(from: &[MailAddr], al: &[String], auth: &AuthAuthority, vips: &[Vip], wake: Wake) -> Class {
        classify(
            &Facts {
                from,
                auth_lines: al,
                authority: auth,
                auto_submitted: false,
                own_mail: false,
                sender_on_this_box: false,
                sender_has_agent_credential: false,
            },
            vips,
            wake,
        )
    }

    const PASS: &str = "mx.lztek.io; dmarc=pass header.from=acme.com";

    #[test]
    fn a_vip_who_passes_dmarc_is_vip() {
        let c = run(&[addr("Jane@ACME.com")], &lines(&[PASS]), &host(), &[vip("jane@acme.com")], Wake::All);
        assert_eq!(c, Class::Vip { auth: "DMARC pass".to_string() });
    }

    #[test]
    fn a_vip_who_fails_dmarc_is_a_flagged_digest_item_not_an_interrupt() {
        let c = run(&[addr("jane@acme.com")], &lines(&["mx.lztek.io; dmarc=fail header.from=acme.com"]), &host(), &[vip("jane@acme.com")], Wake::All);
        assert_eq!(c, Class::Digest { flag: Some("claimed VIP, failed authentication".to_string()) });
    }

    #[test]
    fn a_planted_lower_authentication_results_header_is_ignored() {
        // The top header (the server we trust) has no dmarc=; an attacker's
        // lower one says pass. Nothing vouches for this sender.
        let c = run(
            &[addr("ceo@acme.com")],
            &lines(&["mx.lztek.io; spf=pass", "evil.example; dmarc=pass header.from=acme.com"]),
            &host(),
            &[vip("ceo@acme.com")],
            Wake::All,
        );
        assert_eq!(c, Class::Digest { flag: Some("claimed VIP, failed authentication".to_string()) });
    }

    #[test]
    fn a_header_from_naming_another_domain_vouches_for_nobody() {
        let c = run(&[addr("jane@acme.com")], &lines(&["mx.lztek.io; dmarc=pass header.from=evil.example"]), &host(), &[vip("jane@acme.com")], Wake::All);
        assert!(matches!(c, Class::Digest { flag: Some(_) }), "{c:?}");
    }

    #[test]
    fn a_domain_entry_matches_exactly_not_subdomains_unless_asked() {
        let al = lines(&["mx.lztek.io; dmarc=pass header.from=mail.acme.com"]);
        assert!(matches!(run(&[addr("x@mail.acme.com")], &al, &host(), &[vip("@acme.com")], Wake::All), Class::Digest { flag: None }));
        assert!(matches!(run(&[addr("x@mail.acme.com")], &al, &host(), &[vip("@*.acme.com")], Wake::All), Class::Vip { .. }));
    }

    #[test]
    fn a_vip_only_inbox_ignores_everyone_else_and_all_inbox_digests_them() {
        let al = lines(&[]);
        assert_eq!(run(&[addr("bob@x.com")], &al, &host(), &[vip("jane@acme.com")], Wake::Vip), Class::Ignored { reason: "not_vip" });
        assert_eq!(run(&[addr("bob@x.com")], &al, &host(), &[vip("jane@acme.com")], Wake::All), Class::Digest { flag: None });
    }

    #[test]
    fn a_vip_only_inbox_does_not_digest_a_failed_vip_claim() {
        let c = run(&[addr("jane@acme.com")], &lines(&[]), &host(), &[vip("jane@acme.com")], Wake::Vip);
        assert_eq!(c, Class::Ignored { reason: "claimed_vip_failed_auth" });
    }

    #[test]
    fn own_mail_and_auto_submitted_are_never_a_wake() {
        let base = Facts {
            from: &[],
            auth_lines: &[],
            authority: &AuthAuthority::Provider,
            auto_submitted: false,
            own_mail: false,
            sender_on_this_box: false,
            sender_has_agent_credential: false,
        };
        assert_eq!(classify(&Facts { own_mail: true, ..base.clone() }, &[], Wake::All), Class::Ignored { reason: "own_mail" });
        assert_eq!(classify(&Facts { auto_submitted: true, ..base }, &[], Wake::All), Class::Ignored { reason: "auto_submitted" });
    }

    #[test]
    fn local_mail_counts_only_with_no_agent_credential_on_the_sender() {
        let from = [addr("rosson@lztek.io")];
        let no_lines: Vec<String> = vec![];
        let facts = Facts {
            from: &from,
            auth_lines: &no_lines,
            authority: &AuthAuthority::ThisBox(Some("mx.lztek.io".into())),
            auto_submitted: false,
            own_mail: false,
            sender_on_this_box: true,
            sender_has_agent_credential: false,
        };
        let v = [vip("rosson@lztek.io")];
        assert_eq!(classify(&facts, &v, Wake::All), Class::Vip { auth: "this server".to_string() });
        let agent_held = Facts { sender_has_agent_credential: true, ..facts };
        assert!(matches!(classify(&agent_held, &v, Wake::All), Class::Digest { flag: Some(_) }));
    }

    #[test]
    fn several_from_addresses_are_never_vip() {
        let c = run(
            &[addr("jane@acme.com"), addr("x@evil.example")],
            &lines(&[PASS]),
            &host(),
            &[vip("jane@acme.com")],
            Wake::All,
        );
        assert_eq!(c, Class::Digest { flag: None });
    }
}
