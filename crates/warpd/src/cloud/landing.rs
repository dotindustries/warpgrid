//! Landing page — public marketing site for WarpGrid.
//!
//! Serves static HTML pages at `/`, `/benchmarks`, and `/pricing`.
//! No authentication required. Designed to be the public-facing site
//! served by WarpGrid itself (dogfooding).

use axum::Router;
use axum::response::Html;
use axum::routing::get;

// ── Router ─────────────────────────────────────────────────────

/// Build the landing page router with marketing pages.
pub fn landing_router() -> Router {
    Router::new()
        .route("/", get(landing_page))
        .route("/benchmarks", get(benchmarks_page))
        .route("/pricing", get(pricing_page))
}

// ── Shared CSS ─────────────────────────────────────────────────

const LANDING_CSS: &str = r#"
    * { margin: 0; padding: 0; box-sizing: border-box; }
    body {
        background: #0a0a0a;
        color: #e0e0e0;
        font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', sans-serif;
        line-height: 1.6;
        -webkit-font-smoothing: antialiased;
    }
    a { color: #00ff88; text-decoration: none; }
    a:hover { text-decoration: underline; }
    code { font-family: 'SF Mono', 'Fira Code', 'Cascadia Code', monospace; }

    /* ── Nav ── */
    .nav {
        position: fixed; top: 0; left: 0; right: 0; z-index: 100;
        display: flex; align-items: center; justify-content: space-between;
        padding: 16px 40px;
        background: rgba(10, 10, 10, 0.85);
        backdrop-filter: blur(12px);
        border-bottom: 1px solid #1a1a1a;
    }
    .nav-logo {
        font-size: 20px; font-weight: 700; color: #00ff88;
        letter-spacing: -0.5px;
    }
    .nav-links { display: flex; align-items: center; gap: 28px; }
    .nav-links a { color: #999; font-size: 14px; font-weight: 500; }
    .nav-links a:hover { color: #e0e0e0; text-decoration: none; }
    .nav-cta {
        background: #00ff88; color: #0a0a0a;
        padding: 8px 20px; border-radius: 6px;
        font-size: 14px; font-weight: 600;
        transition: background 0.15s;
    }
    .nav-cta:hover { background: #00cc6e; text-decoration: none; }

    /* ── Layout ── */
    .container { max-width: 1100px; margin: 0 auto; padding: 0 24px; }
    .section { padding: 100px 0; }
    .section-alt { background: #0f0f0f; }

    /* ── Hero ── */
    .hero {
        padding: 160px 0 100px;
        text-align: center;
    }
    .hero h1 {
        font-size: 56px; font-weight: 800; letter-spacing: -2px;
        line-height: 1.1; margin-bottom: 20px;
    }
    .hero h1 span { color: #00ff88; }
    .hero .subtitle {
        font-size: 22px; color: #888; max-width: 600px;
        margin: 0 auto 40px; font-weight: 400;
    }
    .hero-buttons { display: flex; gap: 16px; justify-content: center; flex-wrap: wrap; }
    .btn-primary {
        display: inline-block;
        background: #00ff88; color: #0a0a0a;
        padding: 14px 32px; border-radius: 8px;
        font-size: 16px; font-weight: 700;
        transition: background 0.15s;
    }
    .btn-primary:hover { background: #00cc6e; text-decoration: none; }
    .btn-secondary {
        display: inline-block;
        background: transparent; color: #e0e0e0;
        padding: 14px 32px; border-radius: 8px;
        font-size: 16px; font-weight: 600;
        border: 1px solid #333;
        transition: border-color 0.15s;
    }
    .btn-secondary:hover { border-color: #00ff88; text-decoration: none; }

    /* ── How it works ── */
    .steps { display: grid; grid-template-columns: repeat(3, 1fr); gap: 32px; }
    .step {
        background: #111; border: 1px solid #1a1a1a; border-radius: 12px;
        padding: 32px;
    }
    .step-number {
        display: inline-block;
        width: 32px; height: 32px; line-height: 32px;
        text-align: center; border-radius: 50%;
        background: #00ff8820; color: #00ff88;
        font-size: 14px; font-weight: 700;
        margin-bottom: 16px;
    }
    .step h3 { font-size: 18px; margin-bottom: 8px; }
    .step code {
        display: block; background: #0a0a0a; border: 1px solid #1a1a1a;
        border-radius: 6px; padding: 12px 16px; margin-top: 12px;
        color: #00ff88; font-size: 14px;
    }
    .step p { color: #888; font-size: 14px; }

    /* ── Comparison table ── */
    .compare-table { width: 100%; border-collapse: collapse; margin-top: 32px; }
    .compare-table th {
        text-align: left; padding: 14px 20px;
        color: #666; font-size: 12px; text-transform: uppercase;
        letter-spacing: 1px; border-bottom: 1px solid #1a1a1a;
    }
    .compare-table td {
        padding: 16px 20px; border-bottom: 1px solid #111;
        font-size: 15px;
    }
    .compare-table .metric { color: #888; font-weight: 500; }
    .compare-table .wg-val { color: #00ff88; font-weight: 700; font-family: 'SF Mono', monospace; }
    .compare-table .docker-val { color: #666; font-family: 'SF Mono', monospace; }

    /* ── Badges ── */
    .badges { display: flex; gap: 12px; flex-wrap: wrap; justify-content: center; margin-top: 20px; }
    .badge {
        display: inline-block; padding: 8px 20px;
        background: #111; border: 1px solid #1a1a1a;
        border-radius: 24px; font-size: 14px; font-weight: 600;
        color: #ccc;
    }

    /* ── Section headings ── */
    .section-heading {
        text-align: center; margin-bottom: 48px;
    }
    .section-heading h2 {
        font-size: 36px; font-weight: 700; letter-spacing: -1px;
        margin-bottom: 12px;
    }
    .section-heading p { color: #888; font-size: 16px; max-width: 500px; margin: 0 auto; }

    /* ── Footer ── */
    .footer {
        border-top: 1px solid #1a1a1a;
        padding: 40px 0; text-align: center; color: #555; font-size: 13px;
    }
    .footer a { color: #777; }
    .footer a:hover { color: #00ff88; }
    .footer-tagline { margin-top: 8px; color: #333; font-size: 12px; }

    /* ── Benchmarks page ── */
    .bench-section { margin-bottom: 64px; }
    .bench-section h3 {
        font-size: 22px; font-weight: 600; margin-bottom: 24px;
        padding-bottom: 12px; border-bottom: 1px solid #1a1a1a;
    }
    .bar-chart { display: flex; flex-direction: column; gap: 16px; }
    .bar-row {
        display: flex; align-items: center; gap: 16px;
    }
    .bar-label {
        width: 100px; font-size: 14px; font-weight: 600;
        text-align: right; color: #ccc; flex-shrink: 0;
    }
    .bar-track {
        flex: 1; height: 32px; background: #111;
        border-radius: 6px; overflow: hidden; position: relative;
    }
    .bar-fill {
        height: 100%; background: #00ff88;
        border-radius: 6px; display: flex;
        align-items: center; padding-left: 12px;
        font-size: 13px; font-weight: 700; color: #0a0a0a;
        transition: width 0.6s ease;
    }
    .bar-value {
        font-size: 14px; font-weight: 600; color: #00ff88;
        width: 80px; flex-shrink: 0;
    }

    /* ── Pricing page ── */
    .pricing-grid {
        display: grid; grid-template-columns: repeat(3, 1fr);
        gap: 24px; margin-top: 48px;
    }
    .pricing-card {
        background: #111; border: 1px solid #1a1a1a;
        border-radius: 12px; padding: 36px;
        display: flex; flex-direction: column;
    }
    .pricing-card.featured { border-color: #00ff88; }
    .pricing-card h3 {
        font-size: 20px; font-weight: 700; margin-bottom: 8px;
    }
    .pricing-card .price {
        font-size: 42px; font-weight: 800; margin: 16px 0 8px;
    }
    .pricing-card .price span {
        font-size: 16px; font-weight: 400; color: #666;
    }
    .pricing-card .desc {
        color: #888; font-size: 14px; margin-bottom: 24px;
    }
    .pricing-card ul {
        list-style: none; margin-bottom: 32px; flex: 1;
    }
    .pricing-card ul li {
        padding: 8px 0; font-size: 14px; color: #ccc;
        border-bottom: 1px solid #1a1a1a;
    }
    .pricing-card ul li::before {
        content: "\2713 "; color: #00ff88; font-weight: 700; margin-right: 8px;
    }
    .pricing-cta {
        display: block; text-align: center;
        padding: 12px; border-radius: 8px;
        font-size: 15px; font-weight: 600;
    }
    .pricing-cta-primary {
        background: #00ff88; color: #0a0a0a;
    }
    .pricing-cta-primary:hover { background: #00cc6e; text-decoration: none; }
    .pricing-cta-secondary {
        background: transparent; color: #e0e0e0;
        border: 1px solid #333;
    }
    .pricing-cta-secondary:hover { border-color: #00ff88; text-decoration: none; }
    .beta-banner {
        background: #00ff8815; border: 1px solid #00ff8830;
        border-radius: 8px; padding: 16px 24px;
        text-align: center; margin-top: 48px;
        font-size: 15px; color: #00ff88; font-weight: 600;
    }

    /* ── Responsive ── */
    @media (max-width: 768px) {
        .hero h1 { font-size: 36px; }
        .hero .subtitle { font-size: 18px; }
        .steps { grid-template-columns: 1fr; }
        .pricing-grid { grid-template-columns: 1fr; }
        .nav { padding: 12px 20px; }
        .nav-links { gap: 16px; }
        .nav-links a.nav-hide-mobile { display: none; }
        .container { padding: 0 16px; }
        .section { padding: 60px 0; }
        .hero { padding: 120px 0 60px; }
    }
"#;

// ── Shared fragments ───────────────────────────────────────────

fn landing_nav() -> &'static str {
    r#"<nav class="nav">
    <a href="/" class="nav-logo">WarpGrid</a>
    <div class="nav-links">
        <a href="https://docs.warpgrid.io" class="nav-hide-mobile">Docs</a>
        <a href="/benchmarks">Benchmarks</a>
        <a href="/pricing">Pricing</a>
        <a href="https://github.com/dot-inc/warpgrid" class="nav-hide-mobile">GitHub</a>
        <a href="/console/login" class="nav-cta">Get Started</a>
    </div>
</nav>"#
}

fn landing_footer() -> &'static str {
    r#"<footer class="footer">
    <div class="container">
        <div>Served by WarpGrid &middot;
            <a href="https://github.com/dot-inc/warpgrid">GitHub</a>
        </div>
        <div class="footer-tagline">Wasm-native. No containers. No Kubernetes.</div>
    </div>
</footer>"#
}

fn page_wrapper(title: &str, body: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{title} — WarpGrid</title>
  <style>{css}</style>
</head>
<body>
  {nav}
  {body}
  {footer}
</body>
</html>"#,
        title = title,
        css = LANDING_CSS,
        nav = landing_nav(),
        body = body,
        footer = landing_footer(),
    )
}

// ── Page handlers ──────────────────────────────────────────────

async fn landing_page() -> Html<String> {
    let body = r#"
<section class="hero">
  <div class="container">
    <h1>WarpGrid — Deploy <span>Wasm</span>,<br>not containers.</h1>
    <p class="subtitle">The edge runtime that's 10x denser than Docker. Cold starts under 1ms. Deploy globally in seconds.</p>
    <div class="hero-buttons">
      <a href="/console/login" class="btn-primary">Get Started Free</a>
      <a href="/benchmarks" class="btn-secondary">View Benchmarks</a>
    </div>
  </div>
</section>

<section class="section section-alt">
  <div class="container">
    <div class="section-heading">
      <h2>Ship in three commands</h2>
      <p>From zero to globally deployed in under 60 seconds.</p>
    </div>
    <div class="steps">
      <div class="step">
        <div class="step-number">1</div>
        <h3>Initialize</h3>
        <p>Scaffold a new Wasm component with your language of choice.</p>
        <code>$ warp init --lang rust my-app</code>
      </div>
      <div class="step">
        <div class="step-number">2</div>
        <h3>Deploy</h3>
        <p>Build and push your component to the global edge network.</p>
        <code>$ warp deploy</code>
      </div>
      <div class="step">
        <div class="step-number">3</div>
        <h3>Live</h3>
        <p>Your app is running on every continent. Zero config networking.</p>
        <code>&#x2713; live at my-app.warpgrid.io</code>
      </div>
    </div>
  </div>
</section>

<section class="section">
  <div class="container">
    <div class="section-heading">
      <h2>WarpGrid vs Docker</h2>
      <p>WebAssembly components start faster, weigh less, and pack denser.</p>
    </div>
    <table class="compare-table">
      <thead>
        <tr>
          <th>Metric</th>
          <th>WarpGrid (Wasm)</th>
          <th>Docker</th>
        </tr>
      </thead>
      <tbody>
        <tr>
          <td class="metric">Cold start</td>
          <td class="wg-val">&lt; 1 ms</td>
          <td class="docker-val">200 &ndash; 500 ms</td>
        </tr>
        <tr>
          <td class="metric">App size</td>
          <td class="wg-val">42 KB</td>
          <td class="docker-val">50+ MB</td>
        </tr>
        <tr>
          <td class="metric">Memory per instance</td>
          <td class="wg-val">2 MB</td>
          <td class="docker-val">50+ MB</td>
        </tr>
        <tr>
          <td class="metric">Instances per GB</td>
          <td class="wg-val">500</td>
          <td class="docker-val">~20</td>
        </tr>
      </tbody>
    </table>
  </div>
</section>

<section class="section section-alt">
  <div class="container">
    <div class="section-heading">
      <h2>Your language. Our runtime.</h2>
      <p>Compile to Wasm from the languages you already know.</p>
    </div>
    <div class="badges">
      <span class="badge">Rust</span>
      <span class="badge">Go</span>
      <span class="badge">TypeScript</span>
      <span class="badge">Bun</span>
    </div>
  </div>
</section>
"#;

    Html(page_wrapper("Deploy Wasm, not containers", body))
}

async fn benchmarks_page() -> Html<String> {
    let body = r#"
<section class="hero" style="padding-bottom:60px">
  <div class="container">
    <h1 style="font-size:42px">Performance <span>Benchmarks</span></h1>
    <p class="subtitle">Real numbers from production workloads on WarpGrid edge nodes.</p>
  </div>
</section>

<section class="section" style="padding-top:0">
  <div class="container">

    <div class="bench-section">
      <h3>Cold Start by Language</h3>
      <div class="bar-chart">
        <div class="bar-row">
          <div class="bar-label">Rust</div>
          <div class="bar-track"><div class="bar-fill" style="width:21%">0.3 ms</div></div>
          <div class="bar-value">0.3 ms</div>
        </div>
        <div class="bar-row">
          <div class="bar-label">Go</div>
          <div class="bar-track"><div class="bar-fill" style="width:57%">0.8 ms</div></div>
          <div class="bar-value">0.8 ms</div>
        </div>
        <div class="bar-row">
          <div class="bar-label">TypeScript</div>
          <div class="bar-track"><div class="bar-fill" style="width:79%">1.1 ms</div></div>
          <div class="bar-value">1.1 ms</div>
        </div>
        <div class="bar-row">
          <div class="bar-label">Bun</div>
          <div class="bar-track"><div class="bar-fill" style="width:100%">1.4 ms</div></div>
          <div class="bar-value">1.4 ms</div>
        </div>
      </div>
    </div>

    <div class="bench-section">
      <h3>P95 Latency by Region</h3>
      <div class="bar-chart">
        <div class="bar-row">
          <div class="bar-label">iad</div>
          <div class="bar-track"><div class="bar-fill" style="width:69%">0.9 ms</div></div>
          <div class="bar-value">0.9 ms</div>
        </div>
        <div class="bar-row">
          <div class="bar-label">ams</div>
          <div class="bar-track"><div class="bar-fill" style="width:85%">1.1 ms</div></div>
          <div class="bar-value">1.1 ms</div>
        </div>
        <div class="bar-row">
          <div class="bar-label">sin</div>
          <div class="bar-track"><div class="bar-fill" style="width:100%">1.3 ms</div></div>
          <div class="bar-value">1.3 ms</div>
        </div>
      </div>
    </div>

    <div class="bench-section">
      <h3>Throughput per Instance (req/s)</h3>
      <div class="bar-chart">
        <div class="bar-row">
          <div class="bar-label">Rust</div>
          <div class="bar-track"><div class="bar-fill" style="width:100%">45,000</div></div>
          <div class="bar-value">45k req/s</div>
        </div>
        <div class="bar-row">
          <div class="bar-label">Go</div>
          <div class="bar-track"><div class="bar-fill" style="width:71%">32,000</div></div>
          <div class="bar-value">32k req/s</div>
        </div>
        <div class="bar-row">
          <div class="bar-label">Bun</div>
          <div class="bar-track"><div class="bar-fill" style="width:62%">28,000</div></div>
          <div class="bar-value">28k req/s</div>
        </div>
      </div>
    </div>

    <div class="bench-section">
      <h3>Memory Density (instances per GB)</h3>
      <div class="bar-chart">
        <div class="bar-row">
          <div class="bar-label">WarpGrid</div>
          <div class="bar-track"><div class="bar-fill" style="width:100%">500</div></div>
          <div class="bar-value">500 / GB</div>
        </div>
        <div class="bar-row">
          <div class="bar-label">Docker</div>
          <div class="bar-track"><div class="bar-fill" style="width:4%;min-width:40px">20</div></div>
          <div class="bar-value">20 / GB</div>
        </div>
      </div>
    </div>

    <div style="text-align:center;margin-top:48px;color:#666;font-size:14px">
      <p>Benchmarks run on Fly.io <code>performance-2x</code> machines (4 vCPU, 8 GB RAM).</p>
      <p style="margin-top:8px">
        Source: <a href="https://github.com/dot-inc/warpgrid/tree/main/bench-harness">bench-harness</a>
      </p>
    </div>

  </div>
</section>
"#;

    Html(page_wrapper("Benchmarks", body))
}

async fn pricing_page() -> Html<String> {
    let body = r#"
<section class="hero" style="padding-bottom:60px">
  <div class="container">
    <h1 style="font-size:42px"><span>Simple</span> pricing</h1>
    <p class="subtitle">Start free. Scale when you're ready.</p>
  </div>
</section>

<section class="section" style="padding-top:0">
  <div class="container">
    <div class="pricing-grid">

      <div class="pricing-card">
        <h3>Free</h3>
        <div class="desc">For experiments and side projects.</div>
        <div class="price">$0<span>/mo</span></div>
        <ul>
          <li>3 deployments</li>
          <li>5 instances</li>
          <li>100 req/s</li>
          <li>Community support</li>
        </ul>
        <a href="/console/login" class="pricing-cta pricing-cta-primary">Get Started Free</a>
      </div>

      <div class="pricing-card featured">
        <h3>Pro</h3>
        <div class="desc">For production workloads and teams.</div>
        <div class="price">$29<span>/mo</span></div>
        <ul>
          <li>10 deployments</li>
          <li>20 instances</li>
          <li>1,000 req/s</li>
          <li>Priority support</li>
          <li>Custom domains</li>
        </ul>
        <a href="/console/login" class="pricing-cta pricing-cta-primary">Start Pro Trial</a>
      </div>

      <div class="pricing-card">
        <h3>Enterprise</h3>
        <div class="desc">For organizations with custom needs.</div>
        <div class="price">Custom</div>
        <ul>
          <li>Unlimited deployments</li>
          <li>Unlimited instances</li>
          <li>Custom rate limits</li>
          <li>Dedicated support</li>
          <li>SLA guarantee</li>
          <li>On-prem option</li>
        </ul>
        <a href="mailto:sales@warpgrid.io" class="pricing-cta pricing-cta-secondary">Contact Us</a>
      </div>

    </div>

    <div class="beta-banner">
      Free during beta &mdash; no credit card required.
    </div>
  </div>
</section>
"#;

    Html(page_wrapper("Pricing", body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn landing_page_renders() {
        let html = landing_page().await;
        let body = html.0;
        assert!(body.contains("Deploy Wasm, not containers"));
        assert!(body.contains("10x denser"));
        assert!(body.contains("/console/login"));
        assert!(body.contains("/benchmarks"));
        assert!(body.contains("warp init"));
        assert!(body.contains("warp deploy"));
        assert!(body.contains("Rust"));
        assert!(body.contains("Served by WarpGrid"));
    }

    #[tokio::test]
    async fn benchmarks_page_renders() {
        let html = benchmarks_page().await;
        let body = html.0;
        assert!(body.contains("Cold Start by Language"));
        assert!(body.contains("0.3 ms"));
        assert!(body.contains("P95 Latency"));
        assert!(body.contains("iad"));
        assert!(body.contains("ams"));
        assert!(body.contains("sin"));
        assert!(body.contains("45,000"));
        assert!(body.contains("500"));
        assert!(body.contains("bench-harness"));
    }

    #[tokio::test]
    async fn pricing_page_renders() {
        let html = pricing_page().await;
        let body = html.0;
        assert!(body.contains("$0"));
        assert!(body.contains("$29"));
        assert!(body.contains("Enterprise"));
        assert!(body.contains("Contact Us"));
        assert!(body.contains("Free during beta"));
        assert!(body.contains("no credit card required"));
    }
}
