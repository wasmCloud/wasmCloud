//! Tag listing with `n`/`last` pagination.

use crate::http::respond_owned;
use crate::storage::list_keys;
use crate::util::query_param;
use crate::{Container, Response};

pub(crate) async fn handle_tags_list(
    container: &Container,
    name: &str,
    query: &str,
) -> Result<Response, String> {
    let prefix = format!("{name}/tags/");
    let mut tags: Vec<String> = list_keys(container)
        .await?
        .into_iter()
        .filter_map(|key| key.strip_prefix(&prefix).map(str::to_string))
        .collect();
    tags.sort();

    // Pagination: `last` resumes after a given tag, `n` limits the page size.
    if let Some(last) = query_param(query, "last") {
        tags.retain(|tag| tag.as_str() > last.as_str());
    }
    let limit = query_param(query, "n").and_then(|n| n.parse::<usize>().ok());
    let mut next_link = None;
    if let Some(limit) = limit
        && tags.len() > limit
    {
        tags.truncate(limit);
        if let Some(last) = tags.last() {
            next_link = Some(format!(
                "</v2/{name}/tags/list?n={limit}&last={last}>; rel=\"next\""
            ));
        }
    }

    let body = serde_json::json!({ "name": name, "tags": tags }).to_string();
    let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
    if let Some(link) = next_link {
        headers.push(("link".to_string(), link));
    }
    Ok(respond_owned(200, headers, body.into_bytes()))
}
