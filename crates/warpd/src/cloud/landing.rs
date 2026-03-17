//! Landing page — public marketing site for WarpGrid.
//!
//! Serves the polished landing page and subpages from `landing/` at compile time
//! via `include_str!`. No filesystem access needed at runtime.

use axum::Router;
use axum::response::{Html, Redirect};
use axum::routing::get;

const LANDING_HTML: &str = include_str!("../../../../landing/index.html");
const BENCHMARKS_HTML: &str = include_str!("../../../../landing/benchmarks.html");
const BLOG_DOCKER_VS_WASM_HTML: &str = include_str!("../../../../landing/blog/docker-vs-wasm.html");

/// Build the landing page router.
pub fn landing_router() -> Router {
    Router::new()
        .route("/", get(landing_page))
        .route("/benchmarks", get(benchmarks_page))
        .route("/pricing", get(pricing_redirect))
        .route("/blog/docker-vs-wasm", get(blog_docker_vs_wasm))
}

async fn landing_page() -> Html<&'static str> {
    Html(LANDING_HTML)
}

async fn benchmarks_page() -> Html<&'static str> {
    Html(BENCHMARKS_HTML)
}

async fn pricing_redirect() -> Redirect {
    Redirect::permanent("/")
}

async fn blog_docker_vs_wasm() -> Html<&'static str> {
    Html(BLOG_DOCKER_VS_WASM_HTML)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landing_html_contains_warpgrid() {
        assert!(LANDING_HTML.contains("WarpGrid"));
    }

    #[test]
    fn benchmarks_page_has_proper_styling() {
        assert!(BENCHMARKS_HTML.contains("--accent: #00e5a0"));
        assert!(BENCHMARKS_HTML.contains("Outfit"));
        assert!(BENCHMARKS_HTML.contains("Cold Start"));
    }

    #[test]
    fn blog_post_has_content() {
        assert!(BLOG_DOCKER_VS_WASM_HTML.contains("Docker"));
        assert!(BLOG_DOCKER_VS_WASM_HTML.contains("WarpGrid"));
    }
}
