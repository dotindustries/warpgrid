//! Embedded HTML templates for the wastebin UI.
//!
//! Uses inline CSS with a dark theme. No external dependencies.

/// HTML-escape a string to prevent XSS.
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Base HTML wrapper with dark theme CSS.
fn page(title: &str, body: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{title} - wastebin</title>
<style>
* {{ margin: 0; padding: 0; box-sizing: border-box; }}
body {{ font-family: 'SF Mono', 'Cascadia Code', 'Fira Code', monospace; background: #0d1117; color: #c9d1d9; min-height: 100vh; }}
a {{ color: #58a6ff; text-decoration: none; }}
a:hover {{ text-decoration: underline; }}
.container {{ max-width: 800px; margin: 0 auto; padding: 24px 16px; }}
header {{ border-bottom: 1px solid #21262d; padding-bottom: 16px; margin-bottom: 24px; display: flex; justify-content: space-between; align-items: center; }}
header h1 {{ font-size: 20px; color: #f0f6fc; }}
.badge {{ font-size: 11px; background: #1f6feb22; color: #58a6ff; padding: 2px 8px; border-radius: 12px; border: 1px solid #1f6feb44; }}
form {{ margin-bottom: 24px; }}
textarea {{ width: 100%; min-height: 200px; background: #161b22; border: 1px solid #30363d; border-radius: 6px; padding: 12px; color: #c9d1d9; font-family: inherit; font-size: 14px; resize: vertical; }}
textarea:focus {{ outline: none; border-color: #58a6ff; }}
.row {{ display: flex; gap: 12px; margin-top: 12px; flex-wrap: wrap; }}
input[type="text"], select {{ background: #161b22; border: 1px solid #30363d; border-radius: 6px; padding: 8px 12px; color: #c9d1d9; font-family: inherit; font-size: 13px; }}
input[type="text"]:focus, select:focus {{ outline: none; border-color: #58a6ff; }}
button {{ background: #238636; color: #fff; border: none; padding: 8px 16px; border-radius: 6px; cursor: pointer; font-family: inherit; font-size: 13px; font-weight: 600; }}
button:hover {{ background: #2ea043; }}
.btn-danger {{ background: #da3633; }}
.btn-danger:hover {{ background: #f85149; }}
.paste-list {{ list-style: none; }}
.paste-list li {{ padding: 10px 0; border-bottom: 1px solid #21262d; display: flex; justify-content: space-between; align-items: center; }}
.paste-list li:last-child {{ border-bottom: none; }}
.paste-meta {{ font-size: 12px; color: #8b949e; }}
pre {{ background: #161b22; border: 1px solid #30363d; border-radius: 6px; padding: 16px; overflow-x: auto; font-size: 14px; line-height: 1.5; }}
.actions {{ display: flex; gap: 8px; margin-top: 16px; }}
.empty {{ text-align: center; color: #8b949e; padding: 48px 0; }}
footer {{ margin-top: 48px; padding-top: 16px; border-top: 1px solid #21262d; text-align: center; font-size: 11px; color: #484f58; }}
</style>
</head>
<body>
<div class="container">
{body}
<footer>wastebin on WarpGrid &middot; wasm32-wasip2</footer>
</div>
</body>
</html>"#,
        title = html_escape(title),
        body = body,
    )
}

/// Index page with paste form and recent pastes list.
pub fn index_page(pastes: &[(String, Option<String>, Option<String>, u64)]) -> String {
    let mut list = String::new();
    if pastes.is_empty() {
        list.push_str(r#"<p class="empty">No pastes yet. Create one above.</p>"#);
    } else {
        list.push_str("<ul class=\"paste-list\">");
        for (id, title, lang, created_at) in pastes {
            let display_title = title
                .as_deref()
                .unwrap_or("Untitled");
            let lang_label = lang
                .as_deref()
                .unwrap_or("text");
            list.push_str(&format!(
                r#"<li><a href="/{id}">{title}</a> <span class="paste-meta">{lang} &middot; {ts}</span></li>"#,
                id = html_escape(id),
                title = html_escape(display_title),
                lang = html_escape(lang_label),
                ts = created_at,
            ));
        }
        list.push_str("</ul>");
    }

    let body = format!(
        r#"<header><h1>wastebin</h1><span class="badge">WarpGrid</span></header>
<form method="POST" action="/">
<textarea name="content" placeholder="Paste your code here..." required></textarea>
<div class="row">
<input type="text" name="title" placeholder="Title (optional)">
<select name="language">
<option value="">Auto-detect</option>
<option value="rust">Rust</option>
<option value="javascript">JavaScript</option>
<option value="python">Python</option>
<option value="go">Go</option>
<option value="sql">SQL</option>
<option value="bash">Bash</option>
<option value="text">Plain text</option>
</select>
<label style="display:flex;align-items:center;gap:4px;font-size:13px;color:#8b949e;">
<input type="checkbox" name="burn_after" value="true"> Burn after reading
</label>
<button type="submit">Create paste</button>
</div>
</form>
<h2 style="font-size:16px;margin-bottom:12px;">Recent pastes</h2>
{list}"#,
    );

    page("Home", &body)
}

/// Single paste view page.
pub fn paste_page(
    id: &str,
    title: Option<&str>,
    content: &str,
    language: Option<&str>,
    created_at: u64,
) -> String {
    let display_title = title.unwrap_or("Untitled");
    let lang_class = language.unwrap_or("text");

    let body = format!(
        r#"<header><h1><a href="/">wastebin</a></h1><span class="badge">WarpGrid</span></header>
<div style="margin-bottom:16px;">
<h2 style="font-size:18px;">{title}</h2>
<span class="paste-meta">{lang} &middot; {ts} &middot; <a href="/raw/{id}">raw</a></span>
</div>
<pre>{content}</pre>
<div class="actions">
<form method="POST" action="/{id}" style="display:inline;">
<input type="hidden" name="_method" value="DELETE">
<button type="submit" class="btn-danger">Delete</button>
</form>
</div>"#,
        id = html_escape(id),
        title = html_escape(display_title),
        lang = html_escape(lang_class),
        ts = created_at,
        content = html_escape(content),
    );

    page(display_title, &body)
}

/// Error page.
pub fn error_page(status: u16, message: &str) -> String {
    let body = format!(
        r#"<header><h1><a href="/">wastebin</a></h1><span class="badge">WarpGrid</span></header>
<div class="empty">
<h2 style="font-size:48px;margin-bottom:8px;">{status}</h2>
<p>{message}</p>
</div>"#,
        status = status,
        message = html_escape(message),
    );
    page(&format!("Error {status}"), &body)
}
