//! The provider-neutral pieces of the linked-calendar event JSON
//! (prd-google-calendar-link-v1 §3.5): the **join** field, one response
//! vocabulary, Windows → IANA zones, and description redaction. Google and
//! Microsoft Graph shape their own fields, then call these.
//!
//! **No meeting-service calls (GC-25, Rosson 2026-11-18).** The join field
//! only *labels* a meeting link already written in the event (its
//! conference data, location or description). Everything here is string
//! work: no HTTP client, no DNS, no link is ever opened or resolved. The
//! host is matched exactly (`zoom.us` or a subdomain), so
//! `zoom.us.evil.example` is not Zoom.
//!
//! **Descriptions.** A description is read here in memory to find the
//! meeting link; only the matched `url`, its `provider` label and its
//! `meeting_id` leave (Q5, yes). No phone line, passcode or other text.

use serde_json::{json, Value};

/// Where a join link was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinFrom {
    Conference,
    Location,
    Description,
}

impl JoinFrom {
    pub fn as_str(self) -> &'static str {
        match self {
            JoinFrom::Conference => "conference",
            JoinFrom::Location => "location",
            JoinFrom::Description => "description",
        }
    }
}

/// The normalised join field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Join {
    /// `meet` | `teams` | `zoom` | `other`.
    pub provider: &'static str,
    pub meeting_id: Option<String>,
    /// The link as written (a Zoom `pwd=` stays inside it).
    pub url: String,
    /// Dial-in numbers from the provider's own conference data only
    /// (never parsed out of a description).
    pub phone: Vec<String>,
    pub from: JoinFrom,
}

impl Join {
    pub fn to_json(&self) -> Value {
        json!({
            "provider": self.provider,
            "meeting_id": self.meeting_id,
            "url": self.url,
            "phone": self.phone,
            "from": self.from.as_str(),
        })
    }
}

/// Other meeting services whose links are labelled `other` (never called).
const OTHER_MEETING_HOSTS: &[&str] = &[
    "webex.com",
    "whereby.com",
    "gotomeeting.com",
    "gotomeet.me",
    "meet.goto.com",
    "bluejeans.com",
    "chime.aws",
    "meet.jit.si",
    "skype.com",
];

/// `host` is `domain` or a subdomain of it (exact label match).
fn host_is(host: &str, domain: &str) -> bool {
    host == domain || host.strip_suffix(domain).is_some_and(|rest| rest.ends_with('.'))
}

/// The lowercased host of an `https://` URL; `None` for anything else
/// (plain http, userinfo, an empty host).
pub fn https_host(url: &str) -> Option<String> {
    let rest = url.get(..8).filter(|p| p.eq_ignore_ascii_case("https://")).map(|_| &url[8..])?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') || authority.is_empty() {
        return None;
    }
    let host = authority.split(':').next().unwrap_or("").trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return None;
    }
    Some(host)
}

/// The meeting service a host belongs to; `None` = not a meeting link.
pub fn classify_host(host: &str) -> Option<&'static str> {
    if host == "meet.google.com" {
        Some("meet")
    } else if host == "teams.microsoft.com" || host == "teams.live.com" {
        Some("teams")
    } else if host_is(host, "zoom.us") || host_is(host, "zoomgov.com") {
        Some("zoom")
    } else if OTHER_MEETING_HOSTS.iter().any(|d| host_is(host, d)) {
        Some("other")
    } else {
        None
    }
}

/// The meeting id written in the URL itself: a Meet code, or the digits
/// after Zoom's `/j/` or `/w/`. Nothing is guessed.
pub fn meeting_id_from_url(provider: &str, url: &str) -> Option<String> {
    let after_host = url.get(8..)?.split_once('/').map(|(_, p)| p).unwrap_or("");
    let path = after_host.split(['?', '#']).next().unwrap_or("");
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match provider {
        "meet" => {
            let code = segs.first()?.to_ascii_lowercase();
            let parts: Vec<&str> = code.split('-').collect();
            let shape = parts.len() == 3
                && [3, 4, 3].iter().zip(&parts).all(|(n, p)| p.len() == *n && p.chars().all(|c| c.is_ascii_lowercase()));
            shape.then_some(code)
        }
        "zoom" => segs
            .windows(2)
            .find(|w| matches!(w[0], "j" | "w") && (9..=12).contains(&w[1].len()) && w[1].chars().all(|c| c.is_ascii_digit()))
            .map(|w| w[1].to_string()),
        _ => None,
    }
}

/// Characters that end a URL written in free text.
fn url_end(c: char) -> bool {
    c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '`' | '|' | '\\' | '^' | '{' | '}' | '[' | ']')
}

/// The first meeting link in free text (a location, a description). Only
/// `https` links on a meeting service's host count.
pub fn find_meeting_url(text: &str) -> Option<(&'static str, String)> {
    let lower = text.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find("https://") {
        let start = from + i;
        let end = text[start..].find(url_end).map(|e| start + e).unwrap_or(text.len());
        let url = text[start..end].trim_end_matches(['.', ',', ';', ':', ')', '!', '?']);
        if let Some(p) = https_host(url).as_deref().and_then(classify_host) {
            return Some((p, url.to_string()));
        }
        from = start + 8;
    }
    None
}

/// A Teams "Meeting ID: 123 456 789 012" written next to the link in an
/// event body: the digits only (nothing else from the body leaves).
pub fn teams_meeting_id_in(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let at = lower.find("meeting id")?;
    let tail = &text[at + "meeting id".len()..];
    let tail = tail.trim_start_matches([':', ' ', '\u{a0}', '\t']);
    let digits: String = tail
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ' ' || *c == '\u{a0}')
        .filter(|c| c.is_ascii_digit())
        .collect();
    (9..=20).contains(&digits.len()).then_some(digits)
}

/// A join from a link in free text (`from` = location or description).
pub fn join_from_text(text: &str, from: JoinFrom) -> Option<Join> {
    let (provider, url) = find_meeting_url(text)?;
    let meeting_id = match provider {
        "teams" => teams_meeting_id_in(text),
        p => meeting_id_from_url(p, &url),
    };
    Some(Join { provider, meeting_id, url, phone: Vec::new(), from })
}

/// A join from a provider's own conference link. An unknown host still
/// counts here (the provider attached it): `other`. Only `https`.
pub fn join_from_conference(url: &str, meeting_id: Option<String>, phone: Vec<String>) -> Option<Join> {
    let url = url.trim();
    let host = https_host(url)?;
    let provider = classify_host(&host).unwrap_or("other");
    let meeting_id = meeting_id.filter(|m| !m.trim().is_empty()).or_else(|| meeting_id_from_url(provider, url));
    Some(Join { provider, meeting_id, url: url.to_string(), phone, from: JoinFrom::Conference })
}

/// Google's response words are the one vocabulary (rev 3): a Graph
/// response maps onto them. `organizer` reads as `accepted`.
pub fn norm_response(raw: &str) -> Option<&'static str> {
    match raw.trim() {
        "needsAction" | "notResponded" | "none" => Some("needsAction"),
        "accepted" | "organizer" => Some("accepted"),
        "tentative" | "tentativelyAccepted" => Some("tentative"),
        "declined" => Some("declined"),
        _ => None,
    }
}

/// Attendees beyond this aren't listed (Google stops sending responses
/// past 200 guests): `attendeesPartial` is set instead.
pub const MAX_ATTENDEES: usize = 200;

/// The fields every linked event gets on top of the provider's own shape.
pub struct Neutral {
    pub calendar_id: Option<String>,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub join: Option<Join>,
    pub organizer: Option<String>,
    pub organizer_name: Option<String>,
    pub organizer_is_self: bool,
    pub self_response: Option<&'static str>,
    pub attendees: Vec<Value>,
    pub attendees_partial: bool,
    pub series: Option<Value>,
    pub web_link: Option<String>,
    pub event_type: Option<String>,
}

/// Write the neutral fields into a provider's summary shape. `full` adds
/// the attendee list (`show`, `event`, `events --full`).
pub fn apply(out: &mut Value, n: Neutral, full: bool) {
    let my_role = if n.organizer_is_self { "organizer" } else { "attendee" };
    out["calendarId"] = json!(n.calendar_id);
    out["created"] = json!(n.created);
    out["updated"] = json!(n.updated);
    out["join"] = n.join.as_ref().map(Join::to_json).unwrap_or(Value::Null);
    out["organizer"] = json!(n.organizer);
    out["organizerName"] = json!(n.organizer_name);
    // snake_case on purpose, like `join.meeting_id` and the changes feed
    // (GC-24): a heartbeat keys on these in both places.
    out["organizer_is_self"] = json!(n.organizer_is_self);
    out["self_response"] = json!(n.self_response);
    out["myRole"] = json!(my_role);
    out["attendeeCount"] = json!(n.attendees.len());
    out["attendeesPartial"] = json!(n.attendees_partial || n.attendees.len() > MAX_ATTENDEES);
    out["series"] = n.series.unwrap_or(Value::Null);
    out["webLink"] = json!(n.web_link);
    if let Some(t) = n.event_type {
        out["eventType"] = json!(t);
    }
    out["untrusted"] = json!(["title", "location", "description", "attendees"]);
    if full {
        let mut list = n.attendees;
        list.truncate(MAX_ATTENDEES);
        out["attendees"] = Value::Array(list);
    }
}

/// Per-grant description omission (§3.2): the text never leaves; the
/// join field, already read out of it, stays.
pub fn redact_description(out: &mut Value) {
    out["description"] = Value::Null;
    out["redacted"] = json!(["description"]);
}

/// Windows zone ids (Exchange's `originalStartTimeZone`) → IANA, from
/// CLDR `windowsZones` (territory 001; Unicode License v3, data only).
/// `tzone://Microsoft/Custom` and unknown names → `None`.
const WINDOWS_ZONES: &[(&str, &str)] = &[
    ("Dateline Standard Time", "Etc/GMT+12"),
    ("UTC-11", "Etc/GMT+11"),
    ("Aleutian Standard Time", "America/Adak"),
    ("Hawaiian Standard Time", "Pacific/Honolulu"),
    ("Marquesas Standard Time", "Pacific/Marquesas"),
    ("Alaskan Standard Time", "America/Anchorage"),
    ("UTC-09", "Etc/GMT+9"),
    ("Pacific Standard Time (Mexico)", "America/Tijuana"),
    ("UTC-08", "Etc/GMT+8"),
    ("Pacific Standard Time", "America/Los_Angeles"),
    ("US Mountain Standard Time", "America/Phoenix"),
    ("Mountain Standard Time (Mexico)", "America/Mazatlan"),
    ("Mountain Standard Time", "America/Denver"),
    ("Yukon Standard Time", "America/Whitehorse"),
    ("Central America Standard Time", "America/Guatemala"),
    ("Central Standard Time", "America/Chicago"),
    ("Easter Island Standard Time", "Pacific/Easter"),
    ("Central Standard Time (Mexico)", "America/Mexico_City"),
    ("Canada Central Standard Time", "America/Regina"),
    ("SA Pacific Standard Time", "America/Bogota"),
    ("Eastern Standard Time (Mexico)", "America/Cancun"),
    ("Eastern Standard Time", "America/New_York"),
    ("Haiti Standard Time", "America/Port-au-Prince"),
    ("Cuba Standard Time", "America/Havana"),
    ("US Eastern Standard Time", "America/Indiana/Indianapolis"),
    ("Turks And Caicos Standard Time", "America/Grand_Turk"),
    ("Paraguay Standard Time", "America/Asuncion"),
    ("Atlantic Standard Time", "America/Halifax"),
    ("Venezuela Standard Time", "America/Caracas"),
    ("Central Brazilian Standard Time", "America/Cuiaba"),
    ("SA Western Standard Time", "America/La_Paz"),
    ("Pacific SA Standard Time", "America/Santiago"),
    ("Newfoundland Standard Time", "America/St_Johns"),
    ("Tocantins Standard Time", "America/Araguaina"),
    ("E. South America Standard Time", "America/Sao_Paulo"),
    ("SA Eastern Standard Time", "America/Cayenne"),
    ("Argentina Standard Time", "America/Argentina/Buenos_Aires"),
    ("Greenland Standard Time", "America/Nuuk"),
    ("Montevideo Standard Time", "America/Montevideo"),
    ("Magallanes Standard Time", "America/Punta_Arenas"),
    ("Saint Pierre Standard Time", "America/Miquelon"),
    ("Bahia Standard Time", "America/Bahia"),
    ("UTC-02", "Etc/GMT+2"),
    ("Mid-Atlantic Standard Time", "Etc/GMT+2"),
    ("Azores Standard Time", "Atlantic/Azores"),
    ("Cape Verde Standard Time", "Atlantic/Cape_Verde"),
    ("UTC", "Etc/UTC"),
    ("GMT Standard Time", "Europe/London"),
    ("Greenwich Standard Time", "Atlantic/Reykjavik"),
    ("Sao Tome Standard Time", "Africa/Sao_Tome"),
    ("Morocco Standard Time", "Africa/Casablanca"),
    ("W. Europe Standard Time", "Europe/Berlin"),
    ("Central Europe Standard Time", "Europe/Budapest"),
    ("Romance Standard Time", "Europe/Paris"),
    ("Central European Standard Time", "Europe/Warsaw"),
    ("W. Central Africa Standard Time", "Africa/Lagos"),
    ("Jordan Standard Time", "Asia/Amman"),
    ("GTB Standard Time", "Europe/Bucharest"),
    ("Middle East Standard Time", "Asia/Beirut"),
    ("Egypt Standard Time", "Africa/Cairo"),
    ("E. Europe Standard Time", "Europe/Chisinau"),
    ("Syria Standard Time", "Asia/Damascus"),
    ("West Bank Standard Time", "Asia/Hebron"),
    ("South Africa Standard Time", "Africa/Johannesburg"),
    ("FLE Standard Time", "Europe/Kiev"),
    ("Israel Standard Time", "Asia/Jerusalem"),
    ("South Sudan Standard Time", "Africa/Juba"),
    ("Kaliningrad Standard Time", "Europe/Kaliningrad"),
    ("Sudan Standard Time", "Africa/Khartoum"),
    ("Libya Standard Time", "Africa/Tripoli"),
    ("Namibia Standard Time", "Africa/Windhoek"),
    ("Arabic Standard Time", "Asia/Baghdad"),
    ("Turkey Standard Time", "Europe/Istanbul"),
    ("Arab Standard Time", "Asia/Riyadh"),
    ("Belarus Standard Time", "Europe/Minsk"),
    ("Russian Standard Time", "Europe/Moscow"),
    ("E. Africa Standard Time", "Africa/Nairobi"),
    ("Volgograd Standard Time", "Europe/Volgograd"),
    ("Iran Standard Time", "Asia/Tehran"),
    ("Arabian Standard Time", "Asia/Dubai"),
    ("Astrakhan Standard Time", "Europe/Astrakhan"),
    ("Azerbaijan Standard Time", "Asia/Baku"),
    ("Russia Time Zone 3", "Europe/Samara"),
    ("Mauritius Standard Time", "Indian/Mauritius"),
    ("Saratov Standard Time", "Europe/Saratov"),
    ("Georgian Standard Time", "Asia/Tbilisi"),
    ("Caucasus Standard Time", "Asia/Yerevan"),
    ("Afghanistan Standard Time", "Asia/Kabul"),
    ("West Asia Standard Time", "Asia/Tashkent"),
    ("Ekaterinburg Standard Time", "Asia/Yekaterinburg"),
    ("Pakistan Standard Time", "Asia/Karachi"),
    ("Qyzylorda Standard Time", "Asia/Qyzylorda"),
    ("India Standard Time", "Asia/Kolkata"),
    ("Sri Lanka Standard Time", "Asia/Colombo"),
    ("Nepal Standard Time", "Asia/Kathmandu"),
    ("Central Asia Standard Time", "Asia/Almaty"),
    ("Bangladesh Standard Time", "Asia/Dhaka"),
    ("Omsk Standard Time", "Asia/Omsk"),
    ("Myanmar Standard Time", "Asia/Yangon"),
    ("SE Asia Standard Time", "Asia/Bangkok"),
    ("Altai Standard Time", "Asia/Barnaul"),
    ("W. Mongolia Standard Time", "Asia/Hovd"),
    ("North Asia Standard Time", "Asia/Krasnoyarsk"),
    ("N. Central Asia Standard Time", "Asia/Novosibirsk"),
    ("Tomsk Standard Time", "Asia/Tomsk"),
    ("China Standard Time", "Asia/Shanghai"),
    ("North Asia East Standard Time", "Asia/Irkutsk"),
    ("Singapore Standard Time", "Asia/Singapore"),
    ("W. Australia Standard Time", "Australia/Perth"),
    ("Taipei Standard Time", "Asia/Taipei"),
    ("Ulaanbaatar Standard Time", "Asia/Ulaanbaatar"),
    ("Aus Central W. Standard Time", "Australia/Eucla"),
    ("Transbaikal Standard Time", "Asia/Chita"),
    ("Tokyo Standard Time", "Asia/Tokyo"),
    ("North Korea Standard Time", "Asia/Pyongyang"),
    ("Korea Standard Time", "Asia/Seoul"),
    ("Yakutsk Standard Time", "Asia/Yakutsk"),
    ("Cen. Australia Standard Time", "Australia/Adelaide"),
    ("AUS Central Standard Time", "Australia/Darwin"),
    ("E. Australia Standard Time", "Australia/Brisbane"),
    ("AUS Eastern Standard Time", "Australia/Sydney"),
    ("West Pacific Standard Time", "Pacific/Port_Moresby"),
    ("Tasmania Standard Time", "Australia/Hobart"),
    ("Vladivostok Standard Time", "Asia/Vladivostok"),
    ("Lord Howe Standard Time", "Australia/Lord_Howe"),
    ("Bougainville Standard Time", "Pacific/Bougainville"),
    ("Russia Time Zone 10", "Asia/Srednekolymsk"),
    ("Magadan Standard Time", "Asia/Magadan"),
    ("Norfolk Standard Time", "Pacific/Norfolk"),
    ("Sakhalin Standard Time", "Asia/Sakhalin"),
    ("Central Pacific Standard Time", "Pacific/Guadalcanal"),
    ("Russia Time Zone 11", "Asia/Kamchatka"),
    ("New Zealand Standard Time", "Pacific/Auckland"),
    ("UTC+12", "Etc/GMT-12"),
    ("Fiji Standard Time", "Pacific/Fiji"),
    ("Kamchatka Standard Time", "Asia/Kamchatka"),
    ("Chatham Islands Standard Time", "Pacific/Chatham"),
    ("UTC+13", "Etc/GMT-13"),
    ("Tonga Standard Time", "Pacific/Tongatapu"),
    ("Samoa Standard Time", "Pacific/Apia"),
    ("Line Islands Standard Time", "Pacific/Kiritimati"),
];

/// A Graph event zone → an IANA name: an IANA name stays; a Windows id is
/// mapped; `tzone://Microsoft/Utc` is `Etc/UTC`; anything else (including
/// `tzone://Microsoft/Custom`) is `None`, never a Windows name (GC-8).
pub fn iana_zone(raw: &str) -> Option<String> {
    let z = raw.trim();
    if z.is_empty() || z.eq_ignore_ascii_case("tzone://Microsoft/Custom") {
        return None;
    }
    if z.eq_ignore_ascii_case("tzone://Microsoft/Utc") {
        return Some("Etc/UTC".to_string());
    }
    if z.contains('/') && z.parse::<chrono_tz::Tz>().is_ok() {
        return Some(z.to_string());
    }
    WINDOWS_ZONES.iter().find(|(w, _)| w.eq_ignore_ascii_case(z)).map(|(_, i)| i.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_match_exactly_never_by_lookalike() {
        assert_eq!(classify_host("meet.google.com"), Some("meet"));
        assert_eq!(classify_host("teams.microsoft.com"), Some("teams"));
        assert_eq!(classify_host("teams.live.com"), Some("teams"));
        assert_eq!(classify_host("zoom.us"), Some("zoom"));
        assert_eq!(classify_host("us02web.zoom.us"), Some("zoom"));
        assert_eq!(classify_host("acme.zoomgov.com"), Some("zoom"));
        assert_eq!(classify_host("acme.webex.com"), Some("other"));
        assert_eq!(classify_host("zoom.us.evil.example"), None);
        assert_eq!(classify_host("evilzoom.us"), None);
        assert_eq!(classify_host("meet.google.com.evil.example"), None);
        assert_eq!(classify_host("example.org"), None);
        assert_eq!(https_host("http://zoom.us/j/123456789"), None, "plain http is never a join link");
        assert_eq!(https_host("https://user@zoom.us/j/1"), None, "userinfo is refused");
        assert_eq!(find_meeting_url("https://evil.example/zoom.us/j/81234567890"), None);
    }

    #[test]
    fn meeting_ids_come_from_the_url_text_only() {
        let z = join_from_text("Office · https://us02web.zoom.us/j/81234567890?pwd=abcDEF.1", JoinFrom::Location).unwrap();
        assert_eq!(z.provider, "zoom");
        assert_eq!(z.meeting_id.as_deref(), Some("81234567890"));
        assert_eq!(z.url, "https://us02web.zoom.us/j/81234567890?pwd=abcDEF.1", "pwd stays inside url");
        assert_eq!(z.from, JoinFrom::Location);
        let m = join_from_text("join at https://meet.google.com/abc-defg-hij.", JoinFrom::Description).unwrap();
        assert_eq!((m.provider, m.meeting_id.as_deref()), ("meet", Some("abc-defg-hij")));
        assert_eq!(m.url, "https://meet.google.com/abc-defg-hij");
        let w = join_from_text("https://acme.zoom.us/w/912345678", JoinFrom::Description).unwrap();
        assert_eq!(w.meeting_id.as_deref(), Some("912345678"));
        let odd = join_from_text("https://meet.google.com/lookup/xyz", JoinFrom::Description).unwrap();
        assert_eq!(odd.meeting_id, None, "unparsed is null, never a guess");
        let teams = "Microsoft Teams meeting\nJoin: https://teams.microsoft.com/l/meetup-join/19%3ameeting_x/0?context=%7b%7d\nMeeting ID: 123 456 789 012\nPasscode: Zx9";
        let t = join_from_text(teams, JoinFrom::Description).unwrap();
        assert_eq!(t.provider, "teams");
        assert_eq!(t.meeting_id.as_deref(), Some("123456789012"));
        assert!(!t.url.contains("Passcode"));
        let conf = join_from_conference("https://video.example.net/r/abc", None, vec!["tel:+1-555-0100".into()]).unwrap();
        assert_eq!(conf.provider, "other", "a provider-attached unknown host is other");
        assert_eq!(conf.from, JoinFrom::Conference);
        assert!(join_from_text("call me at https://example.org/x", JoinFrom::Location).is_none());
    }

    /// The omitted-description promise (Q5, yes): a description holding a
    /// link, a passcode and a private note yields the three join values
    /// and nothing else.
    #[test]
    fn only_the_three_join_values_leave_a_description() {
        let desc = "Private: patient prefers mornings.\nZoom: https://us02web.zoom.us/j/81234567890?pwd=SECRETPWD\n\
                    Passcode: 445566\nDial: +1 555 0100";
        let j = join_from_text(desc, JoinFrom::Description).unwrap();
        let v = j.to_json();
        let s = v.to_string();
        for leak in ["Private", "patient", "Passcode", "445566", "555 0100", "Dial"] {
            assert!(!s.contains(leak), "{leak} leaked: {s}");
        }
        assert_eq!(v["phone"], json!([]));
        let mut out = json!({ "description": desc, "join": v.clone() });
        redact_description(&mut out);
        assert_eq!(out["description"], Value::Null);
        assert_eq!(out["redacted"], json!(["description"]));
        assert_eq!(out["join"]["meeting_id"], "81234567890");
    }

    /// GC-25: parsing never builds an HTTP client, resolves a name or opens
    /// a socket. This module is string work only; the check reads its own
    /// source (needles split so this test doesn't match itself).
    #[test]
    fn join_parsing_never_touches_the_network() {
        let src = include_str!("calendar_event.rs");
        for needle in [
            ["req", "west"].concat(),
            ["Tcp", "Stream"].concat(),
            ["ToSocket", "Addrs"].concat(),
            ["Udp", "Socket"].concat(),
            ["Graph", "Http"].concat(),
            ["Http", "Client"].concat(),
        ] {
            assert!(!src.contains(&needle), "calendar_event.rs must not use {needle}");
        }
    }

    #[test]
    fn responses_use_googles_words() {
        for (raw, want) in [
            ("needsAction", "needsAction"),
            ("notResponded", "needsAction"),
            ("none", "needsAction"),
            ("accepted", "accepted"),
            ("organizer", "accepted"),
            ("tentativelyAccepted", "tentative"),
            ("tentative", "tentative"),
            ("declined", "declined"),
        ] {
            assert_eq!(norm_response(raw), Some(want), "{raw}");
        }
        assert_eq!(norm_response("maybe"), None);
    }

    #[test]
    fn windows_zones_map_to_iana_and_every_target_is_known() {
        assert_eq!(iana_zone("Pacific Standard Time").as_deref(), Some("America/Los_Angeles"));
        assert_eq!(iana_zone("Mountain Standard Time").as_deref(), Some("America/Denver"));
        assert_eq!(iana_zone("W. Europe Standard Time").as_deref(), Some("Europe/Berlin"));
        assert_eq!(iana_zone("America/Denver").as_deref(), Some("America/Denver"));
        assert_eq!(iana_zone("tzone://Microsoft/Custom"), None);
        assert_eq!(iana_zone("tzone://Microsoft/Utc").as_deref(), Some("Etc/UTC"));
        assert_eq!(iana_zone("Mars Standard Time"), None);
        for (w, i) in WINDOWS_ZONES {
            assert!(i.parse::<chrono_tz::Tz>().is_ok(), "{w} → {i} isn't a zone chrono-tz knows");
        }
    }
}
