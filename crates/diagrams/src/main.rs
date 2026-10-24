//! Write every figure to disk: `cargo run -p diagrams [OUT_DIR]`. The default
//! output directory is the workspace's `figures/`.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() -> std::io::Result<()> {
    let out = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(diagrams::figures_dir);
    fs::create_dir_all(&out)?;
    let out = out.canonicalize()?;
    for (name, text) in diagrams::outputs() {
        let path = out.join(name);
        fs::write(&path, text)?;
        println!("{}", path.display());
    }
    Ok(())
}
