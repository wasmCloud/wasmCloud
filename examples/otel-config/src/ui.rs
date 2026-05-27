//! HTML rendering for the demo landing page. Kept out of `lib.rs` so the
//! handler reads as the story.

pub(crate) struct PageData<'a> {
    pub(crate) count: u64,
    pub(crate) response_len: usize,
    pub(crate) outbound_url: &'a str,
    pub(crate) trace_id: &'a str,
    pub(crate) span_id: &'a str,
    pub(crate) otlp_endpoint: &'a str,
}

const PAGE_TEMPLATE: &str = include_str!("../templates/page.html");
const ERROR_TEMPLATE: &str = include_str!("../templates/error.html");

pub(crate) fn render_page(data: &PageData<'_>) -> String {
    PAGE_TEMPLATE
        .replace("{count}", &data.count.to_string())
        .replace("{response_len}", &data.response_len.to_string())
        .replace("{outbound_url}", &html_escape(data.outbound_url))
        .replace("{trace_id}", &html_escape(data.trace_id))
        .replace("{span_id}", &html_escape(data.span_id))
        .replace("{otlp_endpoint}", &html_escape(data.otlp_endpoint))
}

pub(crate) fn render_error(message: &str) -> String {
    ERROR_TEMPLATE.replace("{message}", &html_escape(message))
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}
