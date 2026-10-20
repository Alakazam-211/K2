//! Id-cursor pagination streams (C5, S25 §8.5). Streams spawn nothing.

use std::collections::VecDeque;
use std::future::Future;

use futures_util::stream::{self, Stream};
use sew_proto::api::channels::{Channel, ChannelMember};
use sew_proto::api::messages::Message;
use sew_proto::api::spaces::{Space, SpaceMember};
use sew_proto::http::{Page, PageQuery};
use sew_proto::id::Id;
use sew_proto::mgmt::MgmtPrincipal;

use crate::error::ClientError;

/// Which way a stream walks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// `before = start` (none = newest), then `before = the smallest id seen`; newest first.
    Older,
    /// `after = start`, then `after = the largest id seen`; oldest first.
    Newer,
}

/// An item keyed by an id (the cursor).
pub trait HasId {
    /// The cursor id.
    fn cursor_id(&self) -> Id;
}

impl HasId for Space {
    fn cursor_id(&self) -> Id {
        self.id
    }
}

impl HasId for Channel {
    fn cursor_id(&self) -> Id {
        self.id
    }
}

impl HasId for Message {
    fn cursor_id(&self) -> Id {
        self.id
    }
}

impl HasId for SpaceMember {
    fn cursor_id(&self) -> Id {
        self.member_id
    }
}

impl HasId for ChannelMember {
    fn cursor_id(&self) -> Id {
        self.member_id
    }
}

impl HasId for MgmtPrincipal {
    fn cursor_id(&self) -> Id {
        self.member_id
    }
}

struct State<T, F> {
    fetch: F,
    dir: Direction,
    limit: u32,
    cursor: Option<Id>,
    buf: VecDeque<T>,
    done: bool,
}

/// Walks an id-keyed list page by page. `start` is the exclusive cursor (`None`: from the
/// newest for `Older`, from the oldest for `Newer`). Stops when `has_more = false` or a page is
/// empty (a server bug, logged at `warn`). A page whose ids go backward against the direction
/// ends the stream with `Decode` (fail loudly).
pub fn paginate_ids<T, F, Fut>(
    start: Option<Id>,
    dir: Direction,
    limit: u32,
    fetch: F,
) -> impl Stream<Item = Result<T, ClientError>>
where
    T: HasId,
    F: FnMut(PageQuery) -> Fut,
    Fut: Future<Output = Result<Page<T>, ClientError>>,
{
    let cursor = match (dir, start) {
        (_, Some(id)) => Some(id),
        // The smallest valid id: everything newer than "nothing".
        (Direction::Newer, None) => Some(Id::new_const(1)),
        (Direction::Older, None) => None,
    };
    let st = State {
        fetch,
        dir,
        limit,
        cursor,
        buf: VecDeque::new(),
        done: false,
    };
    stream::unfold(st, |mut st| async move {
        loop {
            if let Some(item) = st.buf.pop_front() {
                return Some((Ok(item), st));
            }
            if st.done {
                return None;
            }
            let q = match st.dir {
                Direction::Older => PageQuery {
                    before: st.cursor,
                    limit: Some(st.limit),
                    ..PageQuery::default()
                },
                Direction::Newer => PageQuery {
                    after: st.cursor,
                    limit: Some(st.limit),
                    ..PageQuery::default()
                },
            };
            let page = match (st.fetch)(q).await {
                Ok(p) => p,
                Err(e) => {
                    st.done = true;
                    return Some((Err(e), st));
                }
            };
            if page.items.is_empty() {
                if page.has_more {
                    tracing::warn!("sew-client: empty page with has_more = true; stopping");
                }
                st.done = true;
                continue;
            }
            let mut prev = st.cursor;
            for item in &page.items {
                let id = item.cursor_id();
                let ok = match (st.dir, prev) {
                    (_, None) => true,
                    (Direction::Older, Some(p)) => id < p,
                    (Direction::Newer, Some(p)) => id > p,
                };
                if !ok {
                    st.done = true;
                    let e = ClientError::Decode {
                        status: 200,
                        body_snippet: String::new(),
                        error: format!(
                            "page ids go backward: {id} after {} walking {:?}",
                            prev.map(|p| p.to_string()).unwrap_or_default(),
                            st.dir
                        ),
                    };
                    return Some((Err(e), st));
                }
                prev = Some(id);
            }
            st.cursor = prev;
            st.done = !page.has_more;
            st.buf.extend(page.items);
        }
    })
}
