//! What `clean` deletes in one worktree, read from git, and the deleting.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::build_root::{self, Env};
use crate::names;
use crate::python::path_join;
use crate::system::{self, Failure};

/// One worktree's build folder, the ignored entries that go whole, and the
/// single files that go from folders holding something kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Listing {
    pub worktree: String,
    pub build_root: String,
    pub build_dir: Option<String>,
    pub remove: Vec<String>,
    pub mixed_dirs: Vec<String>,
    pub kept: Vec<String>,
}

/// The defaults' keep patterns, then the repository's own files of them.
fn keep_patterns(cfg: &Map<String, Value>, worktree: &str, main: &str) -> Vec<String> {
    let shipped = cfg
        .get("keep")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned);
    let own = names::KEEP_FILES.iter().flat_map(|name| {
        let text = system::read_text(&path_join(worktree, &[name]))
            .or_else(|| system::read_text(&path_join(main, &[name])));
        build_root::parse_pattern_file(&text.unwrap_or_default())
    });
    shipped.chain(own).collect()
}

/// Ignored files, file by file; a nested repository shows as `path/`.
fn ignored_files(worktree: &str, paths: &[&str]) -> Result<Vec<String>, Failure> {
    let args: Vec<&str> = [
        "ls-files",
        "-z",
        "--others",
        "--ignored",
        "--exclude-standard",
        "--",
    ]
    .into_iter()
    .chain(paths.iter().copied())
    .collect();
    system::git(worktree, &args).map(|out| build_root::split_nul(&out))
}

/// Ignored files a keep pattern matches, at any depth.
fn kept_files(
    worktree: &str,
    patterns: &[String],
    ignored: &[String],
) -> Result<BTreeSet<String>, Failure> {
    let excludes: Vec<String> = patterns.iter().map(|p| format!("--exclude={p}")).collect();
    let args: Vec<&str> = ["ls-files", "-z", "--others", "--ignored"]
        .into_iter()
        .chain(excludes.iter().map(String::as_str))
        .collect();
    let matching: BTreeSet<String> = build_root::split_nul(&system::git(worktree, &args)?)
        .into_iter()
        .collect();
    Ok(ignored
        .iter()
        .filter(|p| !p.ends_with('/') && matching.contains(*p))
        .cloned()
        .collect())
}

/// What `clean` deletes in `worktree`, whose main checkout is `main`.
pub fn listing(
    env: &Env,
    cfg: &Map<String, Value>,
    main: &str,
    common_dir: &str,
    worktree: &str,
    is_main: bool,
) -> Result<Listing, Failure> {
    let root = build_root::repo_build_root(env, common_dir);
    let build = path_join(&root, &["wt", &build_root::wt_id(worktree, is_main)]);
    let patterns = keep_patterns(cfg, worktree, main);
    let ignored = ignored_files(worktree, &[])?;
    let kept = kept_files(worktree, &patterns, &ignored)?;
    let entries = build_root::parse_would_remove(&system::git(worktree, &["clean", "-ndX"])?);
    let nested: Vec<&String> = ignored.iter().filter(|p| p.ends_with('/')).collect();
    let (whole, mixed) = build_root::clean_plan(&entries, &kept, &nested);
    let files = mixed
        .iter()
        .map(|folder| {
            ignored_files(worktree, &[folder])
                .map(|inside| build_root::files_to_remove(&inside, &kept))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Listing {
        worktree: worktree.to_owned(),
        build_root: root,
        build_dir: system::is_dir(&build).then_some(build),
        remove: whole
            .into_iter()
            .chain(files.into_iter().flatten())
            .collect(),
        mixed_dirs: mixed,
        kept: kept.into_iter().collect(),
    })
}

/// Deletes what the listing names, then the folders it emptied.
pub fn apply(listing: &Listing) -> Result<(), Failure> {
    if let Some(build_dir) = &listing.build_dir {
        system::remove_tree(build_dir, &listing.build_root)?;
    }
    listing.remove.iter().try_for_each(|rel| {
        system::remove_tree(
            &path_join(&listing.worktree, &[rel.trim_end_matches('/')]),
            &listing.worktree,
        )
        .map(|_| ())
    })?;
    listing
        .mixed_dirs
        .iter()
        .for_each(|rel| system::prune_empty_dirs(&path_join(&listing.worktree, &[rel])));
    Ok(())
}
