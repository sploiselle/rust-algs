//! Write every figure to disk: `cargo run -p diagrams [OUT_DIR]`. The default
//! output directory is the workspace's `figures/`. Files no figure produces
//! any more are left alone; `cargo test` reports them as stray.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let out = match args.as_slice() {
        [] => diagrams::figures_dir(),
        [dir] if !dir.starts_with('-') => PathBuf::from(dir),
        _ => {
            eprintln!(
                "usage: diagrams [OUT_DIR]\n\nWrites every figure as .svg, .html and .mmd, plus index.json, into OUT_DIR (default: the workspace's figures/)."
            );
            return ExitCode::FAILURE;
        }
    };
    match write_all(&out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn write_all(out: &PathBuf) -> std::io::Result<()> {
    fs::create_dir_all(out)?;
    let out = out.canonicalize()?;
    for (name, text) in diagrams::outputs() {
        let path = out.join(name);
        fs::write(&path, text)?;
        println!("{}", path.display());
    }
    Ok(())
}
