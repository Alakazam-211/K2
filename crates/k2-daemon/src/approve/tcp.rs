//! The TCP dispatcher's arm for `/cli/approve/*`: the body cap, the caller
//! (`who_from_tcp`) and the hop to a blocking thread (the store, the card,
//! and `wait`'s long-poll). The dispatcher's role gate has already run for
//! a login, and its top-level guard refuses a POST to a GET route.

use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

use super::routes;
use crate::routes::http;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn serve(
    stream: &mut TcpStream,
    buf: &mut [u8],
    is_post: bool,
    p: &str,
    path: &str,
    query: &str,
    bearer: Option<&str>,
    owner_token: &str,
) {
    let body = if is_post {
        match http::read_post_body_capped(&mut *stream, buf, routes::MAX_BODY).await {
            Ok(b) => b,
            Err(_) => {
                let r = routes::too_large();
                http::send_response(stream, r.status, r.content_type, &r.body).await;
                return;
            }
        }
    } else {
        let _ = stream.read(buf).await;
        Vec::new()
    };
    let who = crate::compute_routes::who_from_tcp(query, bearer, owner_token);
    let params = http::parse_params(path, query);
    let p = p.to_string();
    let r = tokio::task::spawn_blocking(move || routes::handle(&p, &who, is_post, &body, &params))
        .await
        .unwrap_or_else(|e| crate::cli_response::CliResponse::internal_error(format!("worker join: {e}")));
    http::send_response(stream, r.status, r.content_type, &r.body).await;
}
