//! Merkle trees over a directory hierarchy, for cheaply finding which paths
//! changed between two snapshots. Trees can be saved to a file and loaded back
//! later, e.g. to find what changed while the process wasn't running.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, FileType};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

// Parking lot:
// - On rebuild, reuse a saved file's hash if its size and mtime haven't
//   changed, rather than rereading every file.

/// Starts every saved tree, so loading some other file fails clearly.
const MAGIC: &[u8] = b"ELKREM\0";

/// Bump this whenever the file layout or hashing scheme changes, so that stale
/// files fail to load instead of reporting every path as modified.
const FORMAT_VERSION: u32 = 1;

/// The kind of node being hashed, mixed into its hash so that, e.g., an empty
/// file and an empty directory don't collide. The values are part of every
/// saved hash, so changing them means bumping `FORMAT_VERSION`.
#[repr(u8)]
enum Tag {
    File = 0,
    Symlink = 1,
    Dir = 2,
}

impl Tag {
    /// Starts hashing a node of this kind.
    fn hasher(self) -> blake3::Hasher {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&[self as u8]);
        hasher
    }
}

/// An error building, saving, or loading a [`MerkleTree`].
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("reading {}", .path.display())]
    Read { path: PathBuf, source: io::Error },
    #[error("writing {}", .path.display())]
    Write { path: PathBuf, source: io::Error },
    #[error("unsupported file type at {}", .0.display())]
    UnsupportedFileType(PathBuf),
    #[error("encoding tree for {}", .path.display())]
    Encode {
        path: PathBuf,
        source: postcard::Error,
    },
    #[error("decoding {}", .path.display())]
    Decode {
        path: PathBuf,
        source: postcard::Error,
    },
    #[error("{} is not a saved MerkleTree", .0.display())]
    NotATree(PathBuf),
    #[error(
        "{} has format version {found}, but only {} is supported",
        .path.display(),
        FORMAT_VERSION
    )]
    UnsupportedVersion { path: PathBuf, found: u32 },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    fn read(path: &Path) -> impl FnOnce(io::Error) -> Self {
        move |source| Error::Read {
            path: path.to_owned(),
            source,
        }
    }

    fn write(path: &Path) -> impl FnOnce(io::Error) -> Self {
        move |source| Error::Write {
            path: path.to_owned(),
            source,
        }
    }
}

/// A Merkle tree mirroring a directory hierarchy. Files are hashed by their
/// contents and directories by their children's names and hashes, so two
/// subtrees with equal hashes have equal contents, regardless of where they
/// live on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MerkleTree {
    root: PathBuf,
    node: Node,
}

impl MerkleTree {
    /// Builds a tree from everything under `root`. Symlinks are hashed by their
    /// target path rather than followed.
    ///
    /// # Errors
    ///
    /// Fails if anything under `root` can't be read, or is something other
    /// than a file, directory, or symlink.
    pub fn build(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let file_type = fs::symlink_metadata(&root)
            .map_err(Error::read(&root))?
            .file_type();
        let node = Node::build(&root, file_type)?;
        Ok(Self { root, node })
    }

    /// Loads a tree written by [`MerkleTree::save`].
    ///
    /// # Errors
    ///
    /// Fails if `path` can't be read, or doesn't hold a tree saved in the
    /// current format.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let decode_err = |source| Error::Decode {
            path: path.to_owned(),
            source,
        };

        let bytes = fs::read(path).map_err(Error::read(path))?;
        let bytes = bytes
            .strip_prefix(MAGIC)
            .ok_or_else(|| Error::NotATree(path.to_owned()))?;
        let (version, tree) = postcard::take_from_bytes::<u32>(bytes).map_err(decode_err)?;
        if version != FORMAT_VERSION {
            return Err(Error::UnsupportedVersion {
                path: path.to_owned(),
                found: version,
            });
        }
        postcard::from_bytes(tree).map_err(decode_err)
    }

    /// Writes the tree to `path`, replacing any existing file atomically so a
    /// crash can't leave a partially written one behind.
    ///
    /// # Errors
    ///
    /// Fails if `path` can't be written, or if the tree's root isn't valid
    /// UTF-8 and so can't be encoded.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let bytes =
            postcard::to_extend(&(FORMAT_VERSION, self), MAGIC.to_vec()).map_err(|source| {
                Error::Encode {
                    path: path.to_owned(),
                    source,
                }
            })?;
        write_atomically(path, &bytes).map_err(Error::write(path))
    }

    /// The path this tree was built from.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The hash of the whole tree. Two trees with equal hashes have equal
    /// contents.
    #[must_use]
    pub fn hash(&self) -> [u8; 32] {
        self.node.hash()
    }

    /// Returns what changed going from `self` to `newer`, sorted by path. Paths
    /// are relative to each tree's root. An added or removed directory is
    /// reported once, not once per entry inside it.
    #[must_use]
    pub fn diff(&self, newer: &MerkleTree) -> Vec<Change> {
        let mut changes = Vec::new();
        self.node
            .diff_into(&newer.node, Path::new(""), &mut changes);
        changes
    }
}

/// Replaces `path` with `contents` by writing them to a temp file and renaming
/// it over `path`.
fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    // The temp file must be on the same filesystem as `path` for the rename to
    // be atomic, so put it in the same directory.
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let mut file = NamedTempFile::new_in(dir)?;
    file.write_all(contents)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}

/// A difference between two [`MerkleTree`]s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Present only in the newer tree.
    Added(PathBuf),
    /// Present only in the older tree.
    Removed(PathBuf),
    /// Present in both trees, but with different contents or file types.
    Modified(PathBuf),
}

impl Change {
    /// The path that changed, relative to the trees' roots.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Change::Added(path) | Change::Removed(path) | Change::Modified(path) => path,
        }
    }
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let marker = match self {
            Change::Added(_) => '+',
            Change::Removed(_) => '-',
            Change::Modified(_) => '~',
        };
        write!(f, "{marker} {}", self.path().display())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum Node {
    /// A regular file or symlink.
    Leaf { hash: [u8; 32] },
    Dir {
        hash: [u8; 32],
        children: BTreeMap<OsString, Node>,
    },
}

impl Node {
    fn build(path: &Path, file_type: FileType) -> Result<Self> {
        if file_type.is_dir() {
            Self::build_dir(path)
        } else if file_type.is_file() {
            let file = File::open(path).map_err(Error::read(path))?;
            let hash = Tag::File
                .hasher()
                .update_reader(file)
                .map_err(Error::read(path))?
                .finalize();
            Ok(Node::Leaf { hash: hash.into() })
        } else if file_type.is_symlink() {
            let target = fs::read_link(path).map_err(Error::read(path))?;
            let hash = Tag::Symlink
                .hasher()
                .update(target.as_os_str().as_encoded_bytes())
                .finalize();
            Ok(Node::Leaf { hash: hash.into() })
        } else {
            Err(Error::UnsupportedFileType(path.to_owned()))
        }
    }

    fn build_dir(path: &Path) -> Result<Self> {
        let mut children = BTreeMap::new();
        for entry in fs::read_dir(path).map_err(Error::read(path))? {
            let entry = entry.map_err(Error::read(path))?;
            let file_type = entry.file_type().map_err(Error::read(path))?;
            children.insert(entry.file_name(), Node::build(&entry.path(), file_type)?);
        }

        // `children` is sorted by name, so the hash doesn't depend on the order
        // `read_dir` returned entries in.
        let mut hasher = Tag::Dir.hasher();
        for (name, child) in &children {
            // Length-prefix names so adjacent entries can't run together.
            let name = name.as_encoded_bytes();
            hasher.update(&(name.len() as u64).to_le_bytes());
            hasher.update(name);
            hasher.update(&child.hash());
        }

        Ok(Node::Dir {
            hash: hasher.finalize().into(),
            children,
        })
    }

    fn hash(&self) -> [u8; 32] {
        match self {
            Node::Leaf { hash } | Node::Dir { hash, .. } => *hash,
        }
    }

    /// Appends the changes from `self` to `newer` to `changes`, where `path` is
    /// where both nodes sit relative to their roots.
    fn diff_into(&self, newer: &Node, path: &Path, changes: &mut Vec<Change>) {
        if self.hash() == newer.hash() {
            return;
        }

        let (Node::Dir { children: old, .. }, Node::Dir { children: new, .. }) = (self, newer)
        else {
            changes.push(Change::Modified(path.to_owned()));
            return;
        };

        let names: BTreeSet<&OsString> = old.keys().chain(new.keys()).collect();
        for name in names {
            let path = path.join(name);
            match (old.get(name), new.get(name)) {
                (Some(old), Some(new)) => old.diff_into(new, &path, changes),
                (Some(_), None) => changes.push(Change::Removed(path)),
                (None, Some(_)) => changes.push(Change::Added(path)),
                (None, None) => unreachable!("{name:?} came from one of the maps"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::{Change, Error, FORMAT_VERSION, MAGIC, MerkleTree};

    /// Creates a temp directory containing `files`, given as (relative path,
    /// contents) pairs. Paths ending in `/` become empty directories.
    fn dir_with(files: &[(&str, &str)]) -> TempDir {
        let dir = TempDir::new().unwrap();
        for (rel, contents) in files {
            let path = dir.path().join(rel);
            if rel.ends_with('/') {
                fs::create_dir_all(&path).unwrap();
            } else {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, contents).unwrap();
            }
        }
        dir
    }

    fn build(dir: &TempDir) -> MerkleTree {
        MerkleTree::build(dir.path()).unwrap()
    }

    #[test]
    fn identical_contents_in_different_locations_match() {
        let files = [("a.txt", "a"), ("sub/b.txt", "b"), ("empty/", "")];
        let (left, right) = (build(&dir_with(&files)), build(&dir_with(&files)));

        assert_ne!(left.root(), right.root());
        assert_eq!(left.hash(), right.hash());
        assert_eq!(left.diff(&right), vec![]);
    }

    #[test]
    fn reports_added_removed_and_modified_paths() {
        let old = build(&dir_with(&[
            ("same.txt", "same"),
            ("edited.txt", "before"),
            ("deleted.txt", "gone"),
            ("old_dir/x.txt", "x"),
            ("nested/deep/edited.txt", "before"),
            ("nested/deep/same.txt", "same"),
        ]));
        let new = build(&dir_with(&[
            ("same.txt", "same"),
            ("edited.txt", "after"),
            ("created.txt", "new"),
            ("new_dir/y.txt", "y"),
            ("new_dir/z.txt", "z"),
            ("nested/deep/edited.txt", "after"),
            ("nested/deep/same.txt", "same"),
        ]));

        assert_eq!(
            old.diff(&new),
            vec![
                Change::Added(PathBuf::from("created.txt")),
                Change::Removed(PathBuf::from("deleted.txt")),
                Change::Modified(PathBuf::from("edited.txt")),
                Change::Modified(PathBuf::from("nested/deep/edited.txt")),
                Change::Added(PathBuf::from("new_dir")),
                Change::Removed(PathBuf::from("old_dir")),
            ]
        );
    }

    #[test]
    fn diff_is_reversible() {
        let old = build(&dir_with(&[("a.txt", "a")]));
        let new = build(&dir_with(&[("b.txt", "b")]));

        assert_eq!(
            new.diff(&old),
            vec![
                Change::Added(PathBuf::from("a.txt")),
                Change::Removed(PathBuf::from("b.txt")),
            ]
        );
    }

    #[test]
    fn file_type_changes_are_modifications() {
        let old = build(&dir_with(&[("thing", "")]));
        let new = build(&dir_with(&[("thing/", "")]));

        assert_ne!(old.hash(), new.hash());
        assert_eq!(
            old.diff(&new),
            vec![Change::Modified(PathBuf::from("thing"))]
        );
    }

    #[test]
    fn detects_changes_to_the_same_directory_over_time() {
        let dir = dir_with(&[("a.txt", "a")]);
        let before = build(&dir);
        fs::write(dir.path().join("a.txt"), "changed").unwrap();
        let after = build(&dir);

        assert_eq!(
            before.diff(&after),
            vec![Change::Modified(PathBuf::from("a.txt"))]
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_hashed_by_target_without_following() {
        let dir = dir_with(&[("a.txt", "a"), ("b.txt", "b")]);
        std::os::unix::fs::symlink("a.txt", dir.path().join("link")).unwrap();
        let before = build(&dir);

        // Editing the target doesn't change the link...
        fs::write(dir.path().join("a.txt"), "changed").unwrap();
        let edited = build(&dir);
        assert_eq!(
            before.diff(&edited),
            vec![Change::Modified(PathBuf::from("a.txt"))]
        );

        // ...but retargeting it does.
        fs::remove_file(dir.path().join("link")).unwrap();
        std::os::unix::fs::symlink("b.txt", dir.path().join("link")).unwrap();
        let retargeted = build(&dir);
        assert_eq!(
            edited.diff(&retargeted),
            vec![Change::Modified(PathBuf::from("link"))]
        );
    }

    #[test]
    fn missing_root_is_an_error() {
        let dir = TempDir::new().unwrap();
        let err = MerkleTree::build(dir.path().join("nope")).unwrap_err();
        assert!(
            matches!(&err, Error::Read { path, .. } if path.ends_with("nope")),
            "{err:?}"
        );
    }

    #[test]
    fn root_can_be_a_file() {
        let dir = dir_with(&[("a.txt", "a"), ("b.txt", "b")]);
        let a = MerkleTree::build(dir.path().join("a.txt")).unwrap();
        let b = MerkleTree::build(dir.path().join("b.txt")).unwrap();

        assert_eq!(a.diff(&b), vec![Change::Modified(PathBuf::new())]);
        assert_eq!(a.diff(&a), vec![]);
    }

    #[test]
    fn save_and_load_round_trip() {
        let tree = build(&dir_with(&[
            ("a.txt", "a"),
            ("sub/b.txt", "b"),
            ("empty/", ""),
        ]));
        let index = TempDir::new().unwrap();
        let path = index.path().join("index");

        tree.save(&path).unwrap();
        assert_eq!(MerkleTree::load(&path).unwrap(), tree);
    }

    #[test]
    fn detects_changes_since_a_saved_tree() {
        let dir = dir_with(&[("a.txt", "a"), ("b.txt", "b")]);
        let index = TempDir::new().unwrap();
        let path = index.path().join("index");
        build(&dir).save(&path).unwrap();

        fs::write(dir.path().join("a.txt"), "changed").unwrap();
        fs::remove_file(dir.path().join("b.txt")).unwrap();

        let saved = MerkleTree::load(&path).unwrap();
        let current = MerkleTree::build(saved.root()).unwrap();
        assert_eq!(
            saved.diff(&current),
            vec![
                Change::Modified(PathBuf::from("a.txt")),
                Change::Removed(PathBuf::from("b.txt")),
            ]
        );

        // Saving again replaces the old tree.
        current.save(&path).unwrap();
        assert_eq!(MerkleTree::load(&path).unwrap(), current);
    }

    #[test]
    fn rejects_other_format_versions() {
        let tree = build(&dir_with(&[("a.txt", "a")]));
        let index = TempDir::new().unwrap();
        let path = index.path().join("index");
        let bytes = postcard::to_extend(&(FORMAT_VERSION + 1, &tree), MAGIC.to_vec()).unwrap();
        fs::write(&path, bytes).unwrap();

        let err = MerkleTree::load(&path).unwrap_err();
        assert!(
            matches!(err, Error::UnsupportedVersion { found, .. } if found == FORMAT_VERSION + 1),
            "{err:?}"
        );
    }

    #[test]
    fn rejects_corrupt_files() {
        let index = TempDir::new().unwrap();
        let path = index.path().join("index");
        fs::write(&path, "junk").unwrap();
        let err = MerkleTree::load(&path).unwrap_err();
        assert!(matches!(err, Error::NotATree(_)), "{err:?}");

        let mut bytes = postcard::to_extend(&FORMAT_VERSION, MAGIC.to_vec()).unwrap();
        bytes.extend([0xff, 0xff]);
        fs::write(&path, bytes).unwrap();
        let err = MerkleTree::load(&path).unwrap_err();
        assert!(matches!(err, Error::Decode { .. }), "{err:?}");
    }

    /// Saved trees are only comparable with new ones if hashing never changes.
    /// If this fails, bump `FORMAT_VERSION` and update the expected hash.
    #[test]
    fn hashes_are_stable() {
        let tree = build(&dir_with(&[
            ("a.txt", "a"),
            ("sub/b.txt", "b"),
            ("empty/", ""),
        ]));
        assert_eq!(
            blake3::Hash::from(tree.hash()).to_hex().as_str(),
            "912fb47fec5598ce88c7e0010e9e618e6a01c38ceecf4674e0bfa8d8af300d5f"
        );
    }
}
