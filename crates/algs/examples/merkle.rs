//! Prints what changed under a directory since the last run, keeping the
//! previous tree in an index file between runs.

use std::env;
use std::path::Path;

use algs::merkle::MerkleTree;
use anyhow::{Context, bail};

fn main() -> anyhow::Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let [dir, index] = args.as_slice() else {
        bail!("usage: merkle <dir> <index-file>");
    };
    let index = Path::new(index);

    let current = MerkleTree::build(dir)?;
    let exists = index
        .try_exists()
        .with_context(|| format!("checking for {}", index.display()))?;
    if exists {
        let previous = MerkleTree::load(index)?;
        for change in previous.diff(&current) {
            println!("{change}");
        }
    } else {
        eprintln!("no index at {}; creating one", index.display());
    }
    current.save(index)?;
    Ok(())
}
