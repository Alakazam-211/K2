//! Who a reply goes to: `k2 mail draft <id>` and `k2 mail reply <id>` with
//! `--reply-all`, `--cc` and `--bcc` (0.46.15).
//!
//! A plain reply goes to the reply's locked recipient only (the original's
//! sender, or its Reply-To where the backend already uses it). `--reply-all`
//! adds the original's To to the reply's To and the original's Cc to its Cc,
//! minus every address of the replying inbox (case-insensitive) and minus
//! repeats. `--cc` / `--bcc` add on top. One pure planner serves every
//! backend (hosted, linked IMAP/Gmail, Microsoft 365), so the rule can't
//! drift between them.
//!
//! Drafts only (0.46.17; a person reviews and sends them): `--to` replaces
//! the reply's To (threading stays); a reply to a message this inbox SENT
//! goes to that message's To (and, with `--reply-all`, its Cc), like Gmail's
//! Reply on a sent item, not back to the inbox; and a draft may carry up to
//! [`crate::mail::drafts::MAX_DRAFT_BCC`] blind copies while To + Cc keep
//! the send cap. A SENT reply keeps the old rules on all three.
//!
//! A SENT reply (`k2 mail reply`) may reach only people already on the
//! original under the reply's own rules: an explicit `--cc` / `--bcc` that
//! isn't on it ([`outside_addresses`]) is a new recipient, and the route
//! lets it through only with what `k2 mail send` to it would need. A draft
//! takes any copies (a person sends it).

use std::collections::HashSet;

use crate::mail::addresses;
use crate::mail::jmap::MailAddr;

/// What the caller asked for beyond the locked recipient. `cc` / `bcc` are
/// already checked addresses (the routes run them through
/// `send::normalize_recipients`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReplyAsk {
    /// Copy everyone on the original (To + Cc), minus the inbox itself.
    pub all: bool,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    /// Drafts only: the reply's To, replacing the locked recipient (checked
    /// addresses; empty = the usual To).
    pub to: Vec<String>,
    /// This is a draft (a person sends it): `to`, the sent-item rule and
    /// the draft caps apply. `false` = a sent reply, the old rules.
    pub draft: bool,
}

impl ReplyAsk {
    /// Nothing beyond the locked recipient (a plain reply, as before).
    pub fn is_plain(&self) -> bool {
        !self.all && self.cc.is_empty() && self.bcc.is_empty() && self.to.is_empty()
    }
}

/// The original message's parties, as a reply-all needs them.
#[derive(Debug, Clone)]
pub struct SourceParties {
    /// The reply's locked recipient (the backend's own rule).
    pub primary: MailAddr,
    pub to: Vec<MailAddr>,
    pub cc: Vec<MailAddr>,
    pub subject: String,
    /// Every address on the original: From, Reply-To, To, Cc.
    pub on_original: Vec<String>,
}

/// The reply's recipients. `to[0]` is the locked recipient unless
/// `replaced_to` (a draft's `--to`, or a reply to the inbox's own sent
/// message): then `to` is the whole To and a provider that picks the
/// locked recipient itself must overwrite it.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplyParties {
    pub to: Vec<MailAddr>,
    pub cc: Vec<MailAddr>,
    pub bcc: Vec<MailAddr>,
    pub replaced_to: bool,
}

impl ReplyParties {
    pub fn to_emails(&self) -> Vec<String> {
        self.to.iter().map(|a| a.email.clone()).collect()
    }
    pub fn cc_emails(&self) -> Vec<String> {
        self.cc.iter().map(|a| a.email.clone()).collect()
    }
    pub fn bcc_emails(&self) -> Vec<String> {
        self.bcc.iter().map(|a| a.email.clone()).collect()
    }
    /// The To recipients after the locked one (what a provider that picks
    /// the locked recipient itself must add).
    pub fn extra_to_emails(&self) -> Vec<String> {
        self.to.iter().skip(1).map(|a| a.email.clone()).collect()
    }
    pub fn total(&self) -> usize {
        self.to.len() + self.cc.len() + self.bcc.len()
    }
}

/// The comparable form of an address: normalized (lowercase, punycode), or
/// `None` when it isn't a replyable address at all (a group label, junk).
fn key(email: &str) -> Option<String> {
    addresses::normalize_address(email).ok()
}

/// Plan the reply's recipients. `own` = every address of the replying inbox
/// (any case). The locked recipient stays first and as given; addresses the
/// original carried that aren't replyable are left out; explicit `cc` /
/// `bcc` are kept even when they name the inbox itself (the caller asked
/// for them). Refuses (usage words) past [`crate::mail::send::MAX_RECIPIENTS`]
/// (a draft: [`crate::mail::drafts::check_draft_caps`]).
pub fn plan(
    primary: &MailAddr,
    orig_to: &[MailAddr],
    orig_cc: &[MailAddr],
    own: &[String],
    ask: &ReplyAsk,
) -> Result<ReplyParties, String> {
    let own: HashSet<String> = own.iter().filter_map(|a| key(a)).collect();
    if ask.draft && ask.all && !ask.to.is_empty() {
        return Err("--to sets who the reply goes to and --reply-all copies everyone on the original: \
                    pick one (add others with --cc)"
            .to_string());
    }
    let primary_key = key(&primary.email).unwrap_or_else(|| primary.email.trim().to_ascii_lowercase());
    let mut seen: HashSet<String> = HashSet::new();
    let mut to = Vec::new();
    let mut cc = Vec::new();
    let mut bcc = Vec::new();
    let mut replaced_to = false;
    // A draft's `--to`: the whole To, as asked.
    if ask.draft && !ask.to.is_empty() {
        for e in &ask.to {
            let k = key(e).unwrap_or_else(|| e.trim().to_ascii_lowercase());
            if seen.insert(k.clone()) {
                to.push(MailAddr { name: None, email: k });
            }
        }
        replaced_to = true;
    } else if ask.draft && own.contains(&primary_key) {
        // A draft answering the inbox's OWN sent message: to the people it
        // went to, never back to the inbox (Gmail's Reply on a sent item).
        for a in orig_to {
            let Some(k) = key(&a.email) else { continue };
            if own.contains(&k) || !seen.insert(k.clone()) {
                continue;
            }
            to.push(MailAddr { name: a.name.clone(), email: k });
        }
        replaced_to = !to.is_empty();
    }
    if !replaced_to {
        seen.insert(primary_key);
        to = vec![primary.clone()];
    }
    let take_original = |list: &[MailAddr], out: &mut Vec<MailAddr>, seen: &mut HashSet<String>| {
        for a in list {
            let Some(k) = key(&a.email) else { continue };
            if own.contains(&k) || !seen.insert(k.clone()) {
                continue;
            }
            out.push(MailAddr { name: a.name.clone(), email: k });
        }
    };
    if ask.all {
        if !replaced_to {
            take_original(orig_to, &mut to, &mut seen);
        }
        take_original(orig_cc, &mut cc, &mut seen);
    }
    for (list, out) in [(&ask.cc, &mut cc), (&ask.bcc, &mut bcc)] {
        for e in list {
            let k = key(e).unwrap_or_else(|| e.trim().to_ascii_lowercase());
            if seen.insert(k.clone()) {
                out.push(MailAddr { name: None, email: k });
            }
        }
    }
    let parties = ReplyParties { to, cc, bcc, replaced_to };
    if ask.draft {
        crate::mail::drafts::check_draft_caps(parties.to.len() + parties.cc.len(), parties.bcc.len())?;
        return Ok(parties);
    }
    let max = crate::mail::send::MAX_RECIPIENTS;
    if parties.total() > max {
        return Err(format!(
            "too many recipients ({}) — max {max} per message, counting to + cc + bcc. Reply to \
             fewer people (drop --reply-all or some --cc/--bcc)",
            parties.total()
        ));
    }
    Ok(parties)
}

/// The explicit `--cc` / `--bcc` addresses that aren't on the original
/// (`on_original`: its From, Reply-To, To and Cc), compared normalized, in
/// the order asked, once each. For a sent reply these are new recipients.
pub fn outside_addresses(on_original: &[String], ask: &ReplyAsk) -> Vec<String> {
    let on: HashSet<String> = on_original.iter().filter_map(|a| key(a)).collect();
    let mut out: Vec<String> = Vec::new();
    for e in ask.cc.iter().chain(ask.bcc.iter()) {
        let k = key(e).unwrap_or_else(|| e.trim().to_ascii_lowercase());
        if !on.contains(&k) && !out.contains(&k) {
            out.push(k);
        }
    }
    out
}

/// The addresses of these lists, as [`SourceParties::on_original`] holds them.
pub fn addresses_of(lists: &[&[MailAddr]]) -> Vec<String> {
    lists.iter().flat_map(|l| l.iter().map(|a| a.email.clone())).collect()
}

/// Every address of a linked inbox: its address, and its login name when
/// that is a different address.
pub fn linked_own_addresses(email_address: &str, username: &str) -> Vec<String> {
    let mut out = vec![email_address.to_string()];
    if username.contains('@') && !username.eq_ignore_ascii_case(email_address) {
        out.push(username.to_string());
    }
    out
}

/// Every address of a hosted mailbox: its address plus its aliases on the
/// mail server. The alias read is best effort: when the mail server can't
/// say, the mailbox's own address is still left out (logged).
pub fn hosted_own_addresses(
    client: &crate::mail::jmap::StalwartClient,
    address: &str,
    account_id: &str,
) -> Vec<String> {
    let mut out = vec![address.to_string()];
    let domain = address.split_once('@').map(|(_, d)| d).unwrap_or("");
    match client.account_get_user(account_id) {
        Ok(user) => {
            for a in &user.aliases {
                out.push(format!("{}@{domain}", a.name));
            }
        }
        Err(e) => k2_core::log_debug!(
            "[mail/reply-all] {address}'s aliases unread ({e}); only its own address is left out"
        ),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(email: &str) -> MailAddr {
        MailAddr { name: None, email: email.to_string() }
    }
    fn named(name: &str, email: &str) -> MailAddr {
        MailAddr { name: Some(name.to_string()), email: email.to_string() }
    }
    fn emails(l: &[MailAddr]) -> Vec<&str> {
        l.iter().map(|x| x.email.as_str()).collect()
    }

    #[test]
    fn plain_reply_is_the_locked_recipient_only() {
        let p = plan(
            &a("sender@example.com"),
            &[a("bot@example.org"), a("ada@example.com")],
            &[a("bo@example.com")],
            &["bot@example.org".to_string()],
            &ReplyAsk::default(),
        )
        .expect("plan");
        assert_eq!(emails(&p.to), ["sender@example.com"]);
        assert!(p.cc.is_empty() && p.bcc.is_empty(), "{p:?}");
    }

    #[test]
    fn reply_all_carries_to_and_three_ccs_and_drops_our_own_addresses() {
        let own = vec!["Bot@Example.org".to_string(), "help@example.org".to_string()];
        let p = plan(
            &named("Sender", "sender@example.com"),
            &[a("BOT@example.org"), named("Ada", "Ada@Example.com")],
            &[a("bo@example.com"), a("help@EXAMPLE.org"), a("cy@example.com"), a("di@example.com")],
            &own,
            &ReplyAsk { all: true, ..Default::default() },
        )
        .expect("plan");
        assert_eq!(emails(&p.to), ["sender@example.com", "ada@example.com"]);
        assert_eq!(p.to[1].name.as_deref(), Some("Ada"), "names ride along");
        assert_eq!(emails(&p.cc), ["bo@example.com", "cy@example.com", "di@example.com"]);
        assert!(p.bcc.is_empty());
    }

    #[test]
    fn repeats_collapse_across_to_cc_and_explicit_copies() {
        let p = plan(
            &a("sender@example.com"),
            &[a("ada@example.com"), a("SENDER@example.com"), a("ada@example.com")],
            &[a("Ada@example.com"), a("bo@example.com"), a("bo@example.com")],
            &[],
            &ReplyAsk {
                all: true,
                cc: vec!["bo@example.com".into(), "cy@example.com".into(), "cy@example.com".into()],
                bcc: vec!["cy@example.com".into(), "di@example.com".into(), "sender@example.com".into()],
                ..Default::default()
            },
        )
        .expect("plan");
        assert_eq!(emails(&p.to), ["sender@example.com", "ada@example.com"]);
        assert_eq!(emails(&p.cc), ["bo@example.com", "cy@example.com"]);
        assert_eq!(emails(&p.bcc), ["di@example.com"]);
    }

    #[test]
    fn explicit_cc_and_bcc_add_on_a_plain_reply() {
        let p = plan(
            &a("sender@example.com"),
            &[a("ada@example.com")],
            &[a("bo@example.com")],
            &[],
            &ReplyAsk { all: false, cc: vec!["a@example.com".into(), "b@example.com".into()], bcc: vec!["c@example.com".into()], ..Default::default() },
        )
        .expect("plan");
        assert_eq!(emails(&p.to), ["sender@example.com"], "no reply-all: the original's parties stay off");
        assert_eq!(emails(&p.cc), ["a@example.com", "b@example.com"]);
        assert_eq!(emails(&p.bcc), ["c@example.com"]);
        assert_eq!(p.extra_to_emails(), Vec::<String>::new());
    }

    #[test]
    fn unreplyable_original_addresses_are_left_out() {
        let p = plan(
            &a("sender@example.com"),
            &[a("undisclosed-recipients"), a("ada@example.com")],
            &[a("not an address")],
            &[],
            &ReplyAsk { all: true, ..Default::default() },
        )
        .expect("plan");
        assert_eq!(emails(&p.to), ["sender@example.com", "ada@example.com"]);
        assert!(p.cc.is_empty(), "{p:?}");
    }

    #[test]
    fn past_the_recipient_cap_is_refused() {
        let many: Vec<MailAddr> = (0..crate::mail::send::MAX_RECIPIENTS).map(|i| a(&format!("p{i}@example.com"))).collect();
        let err = plan(&a("sender@example.com"), &many, &[], &[], &ReplyAsk { all: true, ..Default::default() })
            .expect_err("one over the cap");
        assert!(err.contains("too many recipients"), "{err}");
    }

    #[test]
    fn outside_addresses_are_the_copies_not_on_the_original() {
        let on = addresses_of(&[
            &[named("Sender", "sender@example.com")],
            &[a("replies@example.com")],
            &[a("Ada@Example.com")],
            &[a("bo@example.com")],
        ]);
        let ask = ReplyAsk {
            all: true,
            cc: vec!["ada@example.com".into(), "eve@example.com".into(), "REPLIES@example.com".into()],
            bcc: vec!["bo@example.com".into(), "mal@example.net".into(), "eve@example.com".into()],
            ..Default::default()
        };
        assert_eq!(outside_addresses(&on, &ask), ["eve@example.com", "mal@example.net"]);
        assert!(outside_addresses(&on, &ReplyAsk { all: true, ..Default::default() }).is_empty(), "reply-all alone adds nobody new");
    }

    fn draft() -> ReplyAsk {
        ReplyAsk { draft: true, ..Default::default() }
    }

    #[test]
    fn a_draft_to_replaces_the_locked_recipient() {
        let p = plan(
            &a("sender@example.com"),
            &[a("ada@example.com")],
            &[],
            &["bot@example.org".to_string()],
            &ReplyAsk { to: vec!["Boss@Example.com".into(), "boss@example.com".into()], cc: vec!["cy@example.com".into()], ..draft() },
        )
        .expect("plan");
        assert_eq!(emails(&p.to), ["boss@example.com"], "--to is the whole To, once");
        assert!(p.replaced_to);
        assert_eq!(emails(&p.cc), ["cy@example.com"]);
        let err = plan(&a("s@example.com"), &[], &[], &[], &ReplyAsk { all: true, to: vec!["x@example.com".into()], ..draft() })
            .expect_err("--to with --reply-all");
        assert!(err.contains("pick one"), "{err}");
        // A sent reply ignores `to` (the route never sets it there).
        let sent = plan(&a("s@example.com"), &[], &[], &[], &ReplyAsk { to: vec!["x@example.com".into()], ..Default::default() })
            .expect("plan");
        assert_eq!(emails(&sent.to), ["s@example.com"]);
        assert!(!sent.replaced_to);
    }

    #[test]
    fn a_draft_answering_our_own_sent_message_goes_to_its_recipients() {
        let own = vec!["bot@example.org".to_string()];
        let p = plan(
            &a("Bot@Example.org"),
            &[named("Ada", "ada@example.com"), a("bot@example.org"), a("bo@example.com")],
            &[a("cy@example.com")],
            &own,
            &draft(),
        )
        .expect("plan");
        assert_eq!(emails(&p.to), ["ada@example.com", "bo@example.com"], "never back to ourselves");
        assert!(p.replaced_to);
        assert!(p.cc.is_empty(), "plain reply: no Cc");
        let all = plan(&a("bot@example.org"), &[a("ada@example.com")], &[a("cy@example.com"), a("bot@example.org")], &own, &ReplyAsk { all: true, ..draft() })
            .expect("plan");
        assert_eq!(emails(&all.to), ["ada@example.com"]);
        assert_eq!(emails(&all.cc), ["cy@example.com"]);
        // Sent only to ourselves: the reply stays addressed to us.
        let me = plan(&a("bot@example.org"), &[a("bot@example.org")], &[], &own, &draft()).expect("plan");
        assert_eq!(emails(&me.to), ["bot@example.org"]);
        assert!(!me.replaced_to);
        // A SENT reply keeps the old rule (the route decides sends).
        let sent = plan(&a("bot@example.org"), &[a("ada@example.com")], &[], &own, &ReplyAsk::default()).expect("plan");
        assert_eq!(emails(&sent.to), ["bot@example.org"]);
    }

    #[test]
    fn a_draft_takes_500_blind_copies_but_keeps_the_to_cc_cap() {
        let bcc: Vec<String> = (0..crate::mail::drafts::MAX_DRAFT_BCC).map(|i| format!("m{i}@example.com")).collect();
        let p = plan(&a("sender@example.com"), &[], &[], &[], &ReplyAsk { bcc: bcc.clone(), ..draft() }).expect("500 bcc fit a draft");
        assert_eq!(p.bcc.len(), 500);
        let mut over = bcc.clone();
        over.push("one-more@example.com".into());
        let err = plan(&a("sender@example.com"), &[], &[], &[], &ReplyAsk { bcc: over, ..draft() }).expect_err("501");
        assert!(err.contains("500"), "{err}");
        let cc: Vec<String> = (0..crate::mail::send::MAX_RECIPIENTS).map(|i| format!("c{i}@example.com")).collect();
        let err = plan(&a("sender@example.com"), &[], &[], &[], &ReplyAsk { cc, ..draft() }).expect_err("11 in To + Cc");
        assert!(err.contains("To + Cc"), "{err}");
        // A SENT reply: still 10 counting Bcc.
        let err = plan(&a("sender@example.com"), &[], &[], &[], &ReplyAsk { bcc: bcc[..10].to_vec(), ..Default::default() })
            .expect_err("a send keeps the cap");
        assert!(err.contains("too many recipients (11)"), "{err}");
    }

    #[test]
    fn linked_own_addresses_include_a_distinct_login() {
        assert_eq!(linked_own_addresses("a@example.com", "a@example.com"), vec!["a@example.com"]);
        assert_eq!(linked_own_addresses("a@example.com", "A@EXAMPLE.COM"), vec!["a@example.com"]);
        assert_eq!(
            linked_own_addresses("a@example.com", "login@example.net"),
            vec!["a@example.com", "login@example.net"]
        );
        assert_eq!(linked_own_addresses("a@example.com", "plainuser"), vec!["a@example.com"]);
    }
}
