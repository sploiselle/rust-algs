//! The bench targets as cargo builds them now, and what each is built from.
//!
//! A result stands for as long as its target is built from the source it was
//! measured from. So this asks the build what that source is, rather than
//! keeping a list of it: rustc writes down every file it reads to compile a
//! crate, and cargo knows which crates a target links. A target's
//! [`sources`](Target::sources) is one hash of
//!
//! - every file rustc read to compile the target and the workspace's crates
//!   it links, by contents and not by path, so moving one changes nothing;
//! - the other crates it links, by name, version, source and features;
//! - the compiler, the workspace's profiles, and whatever else can change how
//!   the same source is compiled: the flags and profile overrides in the
//!   environment, and cargo's configuration files.
//!
//! A crate a target doesn't link isn't in its hash, and can't change its
//! machine code either. That is why the report, the plan and the tables are
//! a package of their own: nothing measured links them.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::paths;
use crate::record;

/// A bench target, as cargo builds it now.
#[derive(Clone, Debug)]
pub struct Target {
    pub name: String,
    /// Its package's directory, where it runs, as `cargo bench` would run it.
    pub directory: PathBuf,
    pub executable: PathBuf,
    /// The executable's hash: see [`record::hash_file`].
    pub binary: String,
    /// What it's built from, as one hash: see the module's docs.
    pub sources: String,
    /// The files in that hash, from the workspace's root.
    pub files: Vec<PathBuf>,
}

/// Every bench target in `packages`, as it builds now. Cargo builds any
/// that's out of date, without running it, and says where each executable
/// is. Each package is built on its own, as the runner runs its targets:
/// built together, cargo could give a shared dependency other features, and
/// so other machine code.
///
/// # Errors
///
/// If cargo can't build them, or what it wrote about a build can't be read.
pub fn built(packages: &[&str]) -> Result<Vec<Target>, String> {
    let workspace = Workspace::read()?;
    let with = built_with()?;
    let mut targets = Vec::new();
    for package in packages {
        targets.extend(build(package, &workspace, &with)?);
    }
    Ok(targets)
}

/// What cargo built for one crate: where it is, and whose it is.
struct Artifact {
    package: String,
    name: String,
    is_bench: bool,
    is_lib: bool,
    /// A file it built to: the executable, or a library.
    file: PathBuf,
}

fn build(package: &str, workspace: &Workspace, with: &[String]) -> Result<Vec<Target>, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let out = Command::new(&cargo)
        .args(["bench", "-p", package, "--no-run", "--message-format=json"])
        .current_dir(paths::workspace())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|err| format!("running cargo to build {package}'s benches: {err}"))?;
    if !out.status.success() {
        return Err(format!("cargo couldn't build {package}'s benches"));
    }
    let artifacts: Vec<Artifact> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|message| message["reason"] == "compiler-artifact")
        .filter_map(|message| artifact(&message))
        .collect();
    // Each workspace crate's dep-info, by package.
    let mut libraries: HashMap<&str, Vec<PathBuf>> = HashMap::new();
    for built in artifacts.iter().filter(|built| built.is_lib) {
        if workspace.members.contains(&built.package) {
            libraries
                .entry(&built.package)
                .or_default()
                .push(dep_info_of(&built.file));
        }
    }
    let mut targets = Vec::new();
    for bench in artifacts.iter().filter(|built| built.is_bench) {
        let linked = workspace.linked(&bench.package);
        let mut read = vec![dep_info_of(&bench.file)];
        for id in &linked {
            read.extend(libraries.get(id.as_str()).into_iter().flatten().cloned());
        }
        let mut files = BTreeSet::new();
        let mut lines: Vec<String> = with.to_vec();
        for path in &read {
            let info = DepInfo::read(path)?;
            files.extend(info.files);
            lines.extend(info.env.into_iter().map(|env| format!("env! {env}")));
        }
        for id in &linked {
            lines.push(workspace.described(id));
        }
        let mut contents = Vec::new();
        for file in &files {
            let bytes = fs::read(paths::workspace().join(file))
                .map_err(|err| format!("reading {}: {err}", file.display()))?;
            contents.push(record::hash_bytes(&bytes));
        }
        // By contents alone, in no order a path decides.
        contents.sort_unstable();
        lines.sort_unstable();
        lines.dedup();
        lines.extend(contents);
        let binary = record::hash_file(&bench.file)
            .map_err(|err| format!("hashing {}: {err}", bench.file.display()))?;
        targets.push(Target {
            name: bench.name.clone(),
            directory: workspace.directory(&bench.package)?,
            executable: bench.file.clone(),
            binary,
            sources: record::hash_bytes(lines.join("\n").as_bytes()),
            files: files.into_iter().collect(),
        });
    }
    Ok(targets)
}

/// One of cargo's `compiler-artifact` messages, if it's for a bench target's
/// executable or a library.
fn artifact(message: &Value) -> Option<Artifact> {
    let kinds: Vec<&str> = message["target"]["kind"]
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let is_bench = kinds.contains(&"bench");
    let is_lib = kinds.iter().any(|kind| kind.ends_with("lib"));
    let file = if is_bench {
        message["executable"].as_str()?
    } else if is_lib {
        message["filenames"].as_array()?.first()?.as_str()?
    } else {
        return None;
    };
    Some(Artifact {
        package: message["package_id"].as_str()?.to_owned(),
        name: message["target"]["name"].as_str()?.to_owned(),
        is_bench,
        is_lib,
        file: PathBuf::from(file),
    })
}

/// Where rustc wrote down what it read to build `artifact`: beside it, under
/// the crate's name and hash. A library's file has a `lib` before them and
/// an extension after.
fn dep_info_of(artifact: &Path) -> PathBuf {
    let stem = artifact
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let is_library = artifact.extension().is_some();
    let stem = match stem.strip_prefix("lib") {
        Some(rest) if is_library => rest.to_owned(),
        _ if is_library => stem,
        // An executable's name has no extension to take for a stem.
        _ => artifact
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    artifact.with_file_name(format!("{stem}.d"))
}

/// What rustc read to compile one crate: its source files, and the
/// environment variables its `env!`s read.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DepInfo {
    pub files: Vec<PathBuf>,
    /// Each as `NAME=value`, or `NAME` if it wasn't set.
    pub env: Vec<String>,
}

impl DepInfo {
    fn read(path: &Path) -> Result<DepInfo, String> {
        let text =
            fs::read_to_string(path).map_err(|err| format!("reading {}: {err}", path.display()))?;
        Ok(DepInfo::parse(&text))
    }

    /// Reads a dep-info file: a Makefile rule naming the crate's sources
    /// after its first colon, with a space in a path escaped, and a comment
    /// for each environment variable. Cargo's own variables are left out:
    /// they say where the package is, which isn't what it measures.
    pub fn parse(text: &str) -> DepInfo {
        let mut info = DepInfo::default();
        let rule = text
            .lines()
            .find(|line| !line.starts_with('#') && line.contains(": "));
        if let Some((_, sources)) = rule.and_then(|line| line.split_once(": ")) {
            let mut path = String::new();
            let mut chars = sources.chars().peekable();
            while let Some(c) = chars.next() {
                match c {
                    '\\' if chars.peek() == Some(&' ') => {
                        path.push(' ');
                        chars.next();
                    }
                    ' ' => {
                        if !path.is_empty() {
                            info.files.push(PathBuf::from(std::mem::take(&mut path)));
                        }
                    }
                    c => path.push(c),
                }
            }
            if !path.is_empty() {
                info.files.push(PathBuf::from(path));
            }
        }
        for line in text.lines() {
            if let Some(env) = line.strip_prefix("# env-dep:")
                && !env.starts_with("CARGO_")
                && !env.starts_with("CLIPPY_")
            {
                info.env.push(env.to_owned());
            }
        }
        info
    }
}

/// What `cargo metadata` says of the workspace: its packages, and what each
/// links.
struct Workspace {
    /// The ids of the workspace's own packages.
    members: HashSet<String>,
    packages: HashMap<String, Value>,
    /// Each package's dependencies, and whether each is only for its tests
    /// and benches.
    dependencies: HashMap<String, Vec<(String, bool)>>,
    features: HashMap<String, Vec<String>>,
}

impl Workspace {
    fn read() -> Result<Workspace, String> {
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let out = Command::new(&cargo)
            .args(["metadata", "--format-version", "1"])
            .current_dir(paths::workspace())
            .stderr(Stdio::inherit())
            .output()
            .map_err(|err| format!("running cargo metadata: {err}"))?;
        if !out.status.success() {
            return Err("cargo metadata failed".to_owned());
        }
        let metadata: Value = serde_json::from_slice(&out.stdout)
            .map_err(|err| format!("reading cargo metadata: {err}"))?;
        let ids = |value: &Value| -> Vec<String> {
            value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        };
        let mut workspace = Workspace {
            members: ids(&metadata["workspace_members"]).into_iter().collect(),
            packages: HashMap::new(),
            dependencies: HashMap::new(),
            features: HashMap::new(),
        };
        for package in metadata["packages"].as_array().into_iter().flatten() {
            if let Some(id) = package["id"].as_str() {
                workspace.packages.insert(id.to_owned(), package.clone());
            }
        }
        for node in metadata["resolve"]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let Some(id) = node["id"].as_str() else {
                continue;
            };
            let dependencies = node["deps"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|dep| {
                    let kinds = dep["dep_kinds"].as_array()?;
                    let dev_only = kinds.iter().all(|kind| kind["kind"] == "dev");
                    Some((dep["pkg"].as_str()?.to_owned(), dev_only))
                })
                .collect();
            workspace.dependencies.insert(id.to_owned(), dependencies);
            workspace
                .features
                .insert(id.to_owned(), ids(&node["features"]));
        }
        Ok(workspace)
    }

    /// Every package a bench target of `package` links, itself among them. A
    /// bench target links its own package's dev-dependencies, and no other
    /// package's.
    fn linked(&self, package: &str) -> BTreeSet<String> {
        let mut linked = BTreeSet::from([package.to_owned()]);
        let mut next = vec![package.to_owned()];
        while let Some(id) = next.pop() {
            for (dependency, dev_only) in self.dependencies.get(&id).into_iter().flatten() {
                if (!dev_only || id == package) && linked.insert(dependency.clone()) {
                    next.push(dependency.clone());
                }
            }
        }
        linked
    }

    /// A linked package as the hash takes it. One of the workspace's own is
    /// its features alone, since its source is hashed file by file, and its
    /// id says where it is. Any other is its id, which is its name, version
    /// and source, and its features.
    fn described(&self, id: &str) -> String {
        let features = self
            .features
            .get(id)
            .map(|f| f.join(","))
            .unwrap_or_default();
        if self.members.contains(id) {
            let name = self.packages.get(id).and_then(|p| p["name"].as_str());
            format!("member {} [{features}]", name.unwrap_or("unknown"))
        } else {
            format!("links {id} [{features}]")
        }
    }

    /// The directory `package`'s manifest is in.
    fn directory(&self, package: &str) -> Result<PathBuf, String> {
        self.packages
            .get(package)
            .and_then(|p| p["manifest_path"].as_str())
            .and_then(|manifest| Path::new(manifest).parent())
            .map(Path::to_owned)
            .ok_or_else(|| format!("cargo metadata doesn't say where {package} is"))
    }
}

/// The environment variables that change how cargo and rustc compile the
/// same source.
const BUILD_ENV: [&str; 7] = [
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "MACOSX_DEPLOYMENT_TARGET",
    "CARGO_INCREMENTAL",
];

/// And the prefixes of the rest: a profile or build setting, overridden.
const BUILD_ENV_PREFIXES: [&str; 3] = ["CARGO_PROFILE_", "CARGO_BUILD_", "CARGO_TARGET_"];

/// What every target here is built with, a line each: the compiler, the
/// workspace's profiles, the environment's build settings, and cargo's
/// configuration files.
fn built_with() -> Result<Vec<String>, String> {
    let root = paths::workspace();
    let manifest = fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|err| format!("reading the workspace's manifest: {err}"))?;
    let rustc = record::output("rustc", &["-vV"]).unwrap_or_else(|| "unknown".to_owned());
    let mut lines = vec![
        format!("rustc {}", record::hash_bytes(rustc.as_bytes())),
        format!(
            "profiles {}",
            record::hash_bytes(profiles(&manifest).as_bytes())
        ),
    ];
    lines.extend(build_env(std::env::vars()));
    // Cargo reads a configuration file from every directory above the
    // workspace, and from its home.
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")));
    let above = root.ancestors().map(|directory| directory.join(".cargo"));
    for directory in above.chain(home) {
        for name in ["config.toml", "config"] {
            if let Ok(config) = fs::read(directory.join(name)) {
                lines.push(format!("config {}", record::hash_bytes(&config)));
            }
        }
    }
    Ok(lines)
}

/// The build settings among `vars`, a line each.
pub fn build_env(vars: impl Iterator<Item = (String, String)>) -> Vec<String> {
    vars.filter(|(name, _)| {
        // Where the build goes isn't how it's built.
        !name.ends_with("TARGET_DIR")
            && (BUILD_ENV.contains(&name.as_str())
                || BUILD_ENV_PREFIXES
                    .iter()
                    .any(|prefix| name.starts_with(prefix)))
    })
    .map(|(name, value)| format!("env {name}={value}"))
    .collect()
}

/// The lines of a manifest's `[profile]` tables, without the comments around
/// them.
pub fn profiles(manifest: &str) -> String {
    let mut inside = false;
    let mut lines = String::new();
    for line in manifest.lines() {
        if line.starts_with('[') {
            inside = line.starts_with("[profile");
        }
        if inside && !line.trim_start().starts_with('#') {
            lines.push_str(line);
            lines.push('\n');
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_sources_and_the_environment_rustc_wrote_down() {
        let info = DepInfo::parse(
            "/t/deps/one-abc.d: crates/a/src/lib.rs crates/a/src/with\\ space.rs crates/a/../b/x.rs\n\
             \n\
             /t/deps/one-abc: crates/a/src/lib.rs\n\
             \n\
             crates/a/src/lib.rs:\n\
             # env-dep:CARGO_MANIFEST_DIR=/somewhere/crates/a\n\
             # env-dep:BENCH_SEED=7\n\
             # env-dep:UNSET\n",
        );
        assert_eq!(
            info.files,
            [
                "crates/a/src/lib.rs",
                "crates/a/src/with space.rs",
                "crates/a/../b/x.rs"
            ]
            .map(PathBuf::from)
        );
        assert_eq!(info.env, ["BENCH_SEED=7", "UNSET"]);
    }

    #[test]
    fn finds_the_dep_info_beside_an_executable_or_a_library() {
        assert_eq!(
            dep_info_of(Path::new("/t/deps/one_to_one-0cb4")),
            Path::new("/t/deps/one_to_one-0cb4.d")
        );
        assert_eq!(
            dep_info_of(Path::new("/t/deps/libalgs-318e.rlib")),
            Path::new("/t/deps/algs-318e.d")
        );
    }

    #[test]
    fn a_build_setting_in_the_environment_is_part_of_what_a_target_is_built_with() {
        let vars = [
            ("RUSTFLAGS", "-C target-cpu=native"),
            ("CARGO_PROFILE_BENCH_LTO", "fat"),
            ("CARGO_MANIFEST_DIR", "/somewhere"),
            ("CARGO_TARGET_DIR", "/elsewhere"),
            ("HOME", "/home/someone"),
        ]
        .map(|(name, value)| (name.to_owned(), value.to_owned()));
        assert_eq!(
            build_env(vars.into_iter()),
            [
                "env RUSTFLAGS=-C target-cpu=native",
                "env CARGO_PROFILE_BENCH_LTO=fat"
            ]
        );
    }

    #[test]
    fn a_comment_beside_a_profile_isnt_part_of_it() {
        let manifest =
            "[workspace]\nmembers = []\n\n# why\n[profile.bench]\n# and here\ncodegen-units = 1\n";
        assert_eq!(profiles(manifest), "[profile.bench]\ncodegen-units = 1\n");
    }
}
