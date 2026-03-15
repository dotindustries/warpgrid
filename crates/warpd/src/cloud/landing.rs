//! Landing page — public marketing site for WarpGrid.
//!
//! Serves the polished landing page and subpages from `landing/` at compile time
//! via `include_str!`. No filesystem access needed at runtime.

use axum::Router;
use axum::response::Html;
use axum::routing::get;

const LANDING_HTML: &str = include_str!("../../../../landing/index.html");
const BENCHMARKS_HTML: &str = include_str!("../../../../landing/benchmarks.html");
const PRICING_HTML: &str = include_str!("../../../../landing/pricing.html");

/// Build the landing page router.
pub fn landing_router() -> Router {
    Router::new()
        .route("/", get(landing_page))
        .route("/benchmarks", get(benchmarks_page))
        .route("/pricing", get(pricing_page))
}

async fn landing_page() -> Html<&'static str> {
    Html(LANDING_HTML)
}

async fn benchmarks_page() -> Html<&'static str> {
    Html(BENCHMARKS_HTML)
}

async fn pricing_page() -> Html<&'static str> {
    Html(PRICING_HTML)
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
    fn pricing_page_has_proper_styling() {
        assert!(PRICING_HTML.contains("--accent: #00e5a0"));
        assert!(PRICING_HTML.contains("Outfit"));
        assert!(PRICING_HTML.contains("$29"));
    }
}
