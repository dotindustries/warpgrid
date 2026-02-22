use std::path::Path;

pub fn pack(path: &str) -> anyhow::Result<()> {
    let project_path = Path::new(path);
    match warp_pack::pack(project_path) {
        Ok(result) => {
            println!("✓ Compiled to Wasm ({:.1} MB)", result.size_bytes as f64 / 1_048_576.0);
            println!("  Output: {}", result.output_path);
            println!("  SHA256: {}", result.sha256);
            Ok(())
        }
        Err(e) => {
            eprintln!("Pack failed: {e}");
            Err(e)
        }
    }
}
