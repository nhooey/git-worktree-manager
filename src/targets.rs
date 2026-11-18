//! Which repositories a command runs over: the one it runs in, the one
//! `--repository` names, or every entry of a `--repositories` manifest.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::model::ids::DefaultSource;
use crate::system::{Failure, System};

/// One repository to read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// The checkout, as an absolute path; it may not exist.
    pub path: PathBuf,
    /// The default branch to take when `refs/remotes/origin/HEAD` names
    /// none, each with where it came from, the first first.
    pub default_branches: Vec<(DefaultSource, String)>,
}

/// Where the repositories come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source<'a> {
    /// The repository the command runs in.
    Here,
    /// `--repository <checkout>`.
    Repository(&'a Path),
    /// `--repositories <file>`, with relative paths read against
    /// `--root <dir>`, else against the current folder.
    Manifest {
        file: &'a Path,
        root: Option<&'a Path>,
    },
}

/// One entry of the manifest, a JSON array of `{"path", "default_branch"}`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ManifestEntry {
    pub path: PathBuf,
    pub default_branch: Option<String>,
}

/// The manifest's entries; fails as a whole when it isn't the array above.
pub fn parse_manifest(bytes: &[u8]) -> Result<Vec<ManifestEntry>, String> {
    serde_json::from_slice(bytes).map_err(|error| error.to_string())
}

/// The targets of `source`, with `--default-branch` after any manifest's.
pub fn targets(
    system: &impl System,
    source: Source<'_>,
    default_branch: Option<&str>,
) -> Result<Vec<Target>, Failure> {
    let current_dir = system.current_dir()?;
    let option = default_branch.map(|branch| (DefaultSource::Option, branch.to_owned()));
    let target = |path: &Path, manifest: Option<&str>| Target {
        path: crate::system::normal_path(path, &current_dir),
        default_branches: manifest
            .map(|branch| (DefaultSource::Manifest, branch.to_owned()))
            .into_iter()
            .chain(option.clone())
            .collect(),
    };
    match source {
        Source::Here => Ok(vec![target(&current_dir, None)]),
        Source::Repository(path) => Ok(vec![target(path, None)]),
        Source::Manifest { file, root } => {
            let bytes = system
                .read_file(file)?
                .ok_or_else(|| Failure::unreadable(format!("{}: no such file", file.display())))?;
            let entries = parse_manifest(&bytes)
                .map_err(|error| Failure::unreadable(format!("{}: {error}", file.display())))?;
            let base = root.map_or_else(|| current_dir.clone(), |root| current_dir.join(root));
            Ok(entries
                .iter()
                .map(|entry| target(&base.join(&entry.path), entry.default_branch.as_deref()))
                .collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::Fake;

    fn system() -> Fake {
        Fake {
            current_dir: "/work".into(),
            ..Fake::new("2026-11-19T09:12:00Z".parse().expect("a time"))
        }
        .with_file(
            "/work/repos.json",
            r#"[{"path": "app", "default_branch": "trunk"}, {"path": "/abs/lib"}]"#,
        )
    }

    #[test]
    fn test_here_and_a_named_repository() {
        assert_eq!(
            targets(&system(), Source::Here, None),
            Ok(vec![Target {
                path: "/work".into(),
                default_branches: vec![]
            }])
        );
        assert_eq!(
            targets(
                &system(),
                Source::Repository(Path::new("../app")),
                Some("main")
            ),
            Ok(vec![Target {
                path: "/app".into(),
                default_branches: vec![(DefaultSource::Option, "main".to_owned())]
            }])
        );
    }

    #[test]
    fn test_a_manifest_read_against_its_root() {
        let manifest = Source::Manifest {
            file: Path::new("repos.json"),
            root: Some(Path::new("/src")),
        };
        assert_eq!(
            targets(&system(), manifest, Some("main")),
            Ok(vec![
                Target {
                    path: "/src/app".into(),
                    default_branches: vec![
                        (DefaultSource::Manifest, "trunk".to_owned()),
                        (DefaultSource::Option, "main".to_owned()),
                    ]
                },
                Target {
                    path: "/abs/lib".into(),
                    default_branches: vec![(DefaultSource::Option, "main".to_owned())]
                },
            ])
        );
    }

    #[test]
    fn test_a_bad_manifest_fails() {
        let system = system().with_file("/work/bad.json", r#"{"path": "app"}"#);
        let manifest = |file| Source::Manifest {
            file: Path::new(file),
            root: None,
        };
        assert!(targets(&system, manifest("bad.json"), None).is_err());
        assert!(targets(&system, manifest("missing.json"), None).is_err());
    }
}
