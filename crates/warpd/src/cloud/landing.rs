//! Landing page — public marketing site for WarpGrid.
//!
//! Serves the polished landing page from `landing/index.html` at `/`.
//! The HTML is embedded at compile time via `include_str!`, so no
//! filesystem access is needed at runtime.
//!
//! Additional routes (`/benchmarks`, `/pricing`) serve inline pages
//! for content not yet in the main landing page.

use axum::response::Html;
use axum::routing::get;
use axum::Router;

/// The main landing page HTML — embedded from `landing/index.html` at compile time.
const LANDING_HTML: &str = include_str!("../../../../landing/index.html");

/// Build the landing page router with marketing pages.
pub fn landing_router() -> Router {
    Router::new()
        .route("/", get(landing_page))
        .route("/benchmarks", get(benchmarks_page))
        .route("/pricing", get(pricing_page))
}

async fn landing_page() -> Html<&'static str> {
    Html(LANDING_HTML)
}

async fn benchmarks_page() -> Html<String> {
    Html(benchmarks_html())
}

async fn pricing_page() -> Html<String> {
    Html(pricing_html())
}

fn benchmarks_html() -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Benchmarks — WarpGrid</title>
  <style>
    * {{ margin: 0; padding: 0; box-sizing: border-box; }}
    body {{ background: #040608; color: #e2e8f0; font-family: 'Outfit', system-ui, sans-serif; line-height: 1.6; padding: 2rem; }}
    .container {{ max-width: 800px; margin: 0 auto; }}
    h1 {{ color: #00e5a0; margin-bottom: 2rem; }}
    h2 {{ color: #f1f5f9; margin: 2rem 0 1rem; }}
    .bar-chart {{ margin: 1rem 0; }}
    .bar {{ display: flex; align-items: center; margin: 0.5rem 0; }}
    .bar-label {{ width: 120px; font-family: 'IBM Plex Mono', monospace; font-size: 0.9rem; color: #94a3b8; }}
    .bar-fill {{ height: 28px; background: linear-gradient(90deg, #00e5a0, #00b880); border-radius: 4px; margin-right: 0.5rem; transition: width 0.5s; }}
    .bar-value {{ font-family: 'IBM Plex Mono', monospace; font-size: 0.85rem; color: #00e5a0; }}
    a {{ color: #00e5a0; }}
    .back {{ display: inline-block; margin-bottom: 2rem; text-decoration: none; }}
  </style>
</head>
<body>
  <div class="container">
    <a href="/" class="back">← Back to WarpGrid</a>
    <h1>Performance Benchmarks</h1>

    <h2>Cold Start Latency (ms)</h2>
    <div class="bar-chart">
      <div class="bar"><span class="bar-label">Rust</span><div class="bar-fill" style="width: 15%"></div><span class="bar-value">0.3ms</span></div>
      <div class="bar"><span class="bar-label">Go</span><div class="bar-fill" style="width: 40%"></div><span class="bar-value">0.8ms</span></div>
      <div class="bar"><span class="bar-label">TypeScript</span><div class="bar-fill" style="width: 55%"></div><span class="bar-value">1.1ms</span></div>
      <div class="bar"><span class="bar-label">Bun</span><div class="bar-fill" style="width: 70%"></div><span class="bar-value">1.4ms</span></div>
      <div class="bar"><span class="bar-label">Docker</span><div class="bar-fill" style="width: 100%; background: linear-gradient(90deg, #ff5c6c, #cc3344);"></div><span class="bar-value">200-500ms</span></div>
    </div>

    <h2>Throughput (req/sec per instance)</h2>
    <div class="bar-chart">
      <div class="bar"><span class="bar-label">Rust</span><div class="bar-fill" style="width: 100%"></div><span class="bar-value">45,000</span></div>
      <div class="bar"><span class="bar-label">Go</span><div class="bar-fill" style="width: 71%"></div><span class="bar-value">32,000</span></div>
      <div class="bar"><span class="bar-label">Bun</span><div class="bar-fill" style="width: 62%"></div><span class="bar-value">28,000</span></div>
    </div>

    <h2>Memory Density (instances per 1GB)</h2>
    <div class="bar-chart">
      <div class="bar"><span class="bar-label">WarpGrid</span><div class="bar-fill" style="width: 100%"></div><span class="bar-value">500</span></div>
      <div class="bar"><span class="bar-label">Docker</span><div class="bar-fill" style="width: 4%; background: linear-gradient(90deg, #ff5c6c, #cc3344);"></div><span class="bar-value">20</span></div>
    </div>

    <p style="margin-top: 2rem; color: #64748b; font-size: 0.9rem;">
      Source: <a href="https://github.com/dotindustries/warpgrid/tree/main/test-infra/bench-harness">test-infra/bench-harness</a>
    </p>
  </div>
</body>
</html>"#
    )
}

fn pricing_html() -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Pricing — WarpGrid</title>
  <style>
    * {{ margin: 0; padding: 0; box-sizing: border-box; }}
    body {{ background: #040608; color: #e2e8f0; font-family: 'Outfit', system-ui, sans-serif; line-height: 1.6; padding: 2rem; }}
    .container {{ max-width: 900px; margin: 0 auto; }}
    h1 {{ color: #00e5a0; margin-bottom: 0.5rem; }}
    .subtitle {{ color: #94a3b8; margin-bottom: 2rem; }}
    .plans {{ display: grid; grid-template-columns: repeat(3, 1fr); gap: 1.5rem; margin: 2rem 0; }}
    .plan {{ background: #0a0f14; border: 1px solid #1e2d3d; border-radius: 12px; padding: 2rem; }}
    .plan.featured {{ border-color: #00e5a0; }}
    .plan h2 {{ color: #f1f5f9; margin-bottom: 0.5rem; }}
    .price {{ font-size: 2rem; color: #00e5a0; font-weight: 700; margin: 1rem 0; }}
    .price span {{ font-size: 1rem; color: #64748b; }}
    ul {{ list-style: none; margin: 1rem 0; }}
    li {{ padding: 0.4rem 0; color: #cbd5e1; }}
    li::before {{ content: "✓ "; color: #00e5a0; }}
    .btn {{ display: inline-block; padding: 0.75rem 1.5rem; background: #00e5a0; color: #040608; font-weight: 600; border-radius: 8px; text-decoration: none; margin-top: 1rem; }}
    .btn.outline {{ background: transparent; border: 1px solid #00e5a0; color: #00e5a0; }}
    .banner {{ background: linear-gradient(90deg, rgba(0,229,160,0.1), rgba(0,229,160,0.05)); border: 1px solid rgba(0,229,160,0.2); border-radius: 8px; padding: 1rem 1.5rem; text-align: center; color: #00e5a0; margin-bottom: 2rem; }}
    a {{ color: #00e5a0; }}
    .back {{ display: inline-block; margin-bottom: 2rem; text-decoration: none; }}
    @media (max-width: 768px) {{ .plans {{ grid-template-columns: 1fr; }} }}
  </style>
</head>
<body>
  <div class="container">
    <a href="/" class="back">← Back to WarpGrid</a>
    <div class="banner">Free during beta — no credit card required</div>
    <h1>Simple, transparent pricing</h1>
    <p class="subtitle">Pay only for what you use. Scale to zero when idle.</p>
    <div class="plans">
      <div class="plan">
        <h2>Free</h2>
        <div class="price">$0<span>/mo</span></div>
        <ul>
          <li>3 deployments</li>
          <li>5 instances each</li>
          <li>100 req/s</li>
          <li>10 MB max Wasm</li>
          <li>Community support</li>
        </ul>
        <a href="/console/login" class="btn">Get Started</a>
      </div>
      <div class="plan featured">
        <h2>Pro</h2>
        <div class="price">$29<span>/mo</span></div>
        <ul>
          <li>10 deployments</li>
          <li>20 instances each</li>
          <li>1,000 req/s</li>
          <li>50 MB max Wasm</li>
          <li>Custom domains</li>
          <li>Priority support</li>
        </ul>
        <a href="/console/login" class="btn">Start Free Trial</a>
      </div>
      <div class="plan">
        <h2>Enterprise</h2>
        <div class="price">Custom</div>
        <ul>
          <li>Unlimited deployments</li>
          <li>Unlimited instances</li>
          <li>Dedicated regions</li>
          <li>SLA guarantee</li>
          <li>SSO / SAML</li>
          <li>24/7 support</li>
        </ul>
        <a href="mailto:hello@dot.industries" class="btn outline">Contact Us</a>
      </div>
    </div>
  </div>
</body>
</html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landing_html_contains_warpgrid() {
        assert!(LANDING_HTML.contains("WarpGrid"));
    }

    #[test]
    fn benchmarks_page_renders() {
        let html = benchmarks_html();
        assert!(html.contains("Benchmarks"));
        assert!(html.contains("Cold Start"));
    }

    #[test]
    fn pricing_page_renders() {
        let html = pricing_html();
        assert!(html.contains("Pricing"));
        assert!(html.contains("$29"));
    }
}
