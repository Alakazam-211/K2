//! S20 calls: message and file search (prd-sew-20 §14). Both are `POST` reads: the query text
//! stays out of URLs and access logs (contracts rev 27 G5). Pages follow `has_more` and the
//! opaque `cursor`; a cursor needs the same request fields as page 1.

use sew_proto::api::search::{SearchFilesResponse, SearchMessagesResponse, SearchRequest};
use sew_proto::routes as r;

use crate::{Client, ClientError};

impl Client {
    /// `POST /api/v1/search/messages`: one page of message hits (bodies and highlights from
    /// the server's current rows). Errors: 400 with `details.reason` (§14.4), 429 (`search`
    /// bucket), 503 (`search_rebuilding`, `search_busy`, `search_timeout`).
    pub async fn search_messages(
        &self,
        req: &SearchRequest,
    ) -> Result<SearchMessagesResponse, ClientError> {
        self.post(r::SEARCH_MESSAGES, req).await
    }

    /// `POST /api/v1/search/files`: one page of attachment hits by file name.
    pub async fn search_files(
        &self,
        req: &SearchRequest,
    ) -> Result<SearchFilesResponse, ClientError> {
        self.post(r::SEARCH_FILES, req).await
    }
}
