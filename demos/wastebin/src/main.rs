//! Standalone entry point for wastebin demo.
//!
//! Reads configuration from environment variables and processes
//! a single request (useful for testing outside of WarpGrid).

fn main() {
    let conninfo = std::env::var("WASTEBIN_DATABASE_URL")
        .unwrap_or_else(|_| "host=localhost dbname=wastebin".to_string());
    let instance_id = std::env::var("WASTEBIN_INSTANCE_ID")
        .unwrap_or_else(|_| "standalone".to_string());

    // In standalone mode, just print a health check
    let response = wastebin_demo::handle_request("GET", "/health", &[], &conninfo, &instance_id);

    println!("Status: {}", response.status);
    for (key, value) in &response.headers {
        println!("{key}: {value}");
    }
    println!();
    println!("{}", String::from_utf8_lossy(&response.body));
}
