//! The build root: export lines that put build output under it, caches
//! that can be shared under `shared/` and per-worktree output under
//! `wt/<id>/`.
//!
//! A variable the environment already sets is a person's and stays as it
//! is, unless [`exported_variable`] names it: `env` set it, so it follows
//! the worktree `env` runs in now, or is unset once no language asked
//! wants it.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use gix::bstr::ByteSlice;
use gix::glob::wildmatch;
use serde::Serialize;

use crate::environment::Environment;
use crate::locate::{Reach, locate};
use crate::model::ids::ErrorCode;
use crate::names;
use crate::system::{Failure, FileKind, System};

/// The folder of caches every worktree shares.
pub const SHARED: &str = "shared";

/// The folder holding each worktree's own folder.
pub const WORKTREES: &str = "wt";

/// The id of the main checkout's folder.
pub const MAIN_ID: &str = "main";

/// In the build root: the repository's common git directory, a line.
pub const REPO_FILE: &str = "REPO";

/// The back-pointer `provision` writes in a worktree's own folder:
/// `<build root>/wt/<id>/WORKTREE`, holding the bytes of the worktree's
/// real path and one newline, nothing else. The sweep deletes a `wt/<id>/`
/// whose back-pointer names a folder git no longer lists, so its name,
/// place and content are a contract. The sweep compares the named folder
/// with git's list by file identity or real path, and reads a file without
/// its final newline as no back-pointer.
pub const WORKTREE_FILE: &str = "WORKTREE";

/// In the folder of every build root, on macOS: Spotlight indexes nothing
/// under a folder holding it.
pub const NEVER_INDEX_FILE: &str = ".metadata_never_index";

/// In the folder of every build root, on macOS: written once `tmutil
/// addexclusion` has excluded that folder from Time Machine.
pub const TIME_MACHINE_FILE: &str = ".excluded-from-time-machine";

/// The operating system, where a variable or a step depends on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Other,
}

impl Platform {
    /// The system the program was built for.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Other
        }
    }
}

/// Where one worktree's build output goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// The repository's build root, from `names::build_root`.
    pub root: PathBuf,
    /// The worktree whose output it is, as a real path.
    pub worktree: PathBuf,
    pub main: bool,
}

impl Layout {
    /// [`MAIN_ID`] for the main checkout, else the folder's name and the
    /// first 8 hex digits of its path's hash, so two worktrees with one
    /// name differ.
    pub fn id(&self) -> String {
        if self.main {
            MAIN_ID.to_owned()
        } else {
            let name = self
                .worktree
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            format!("{name}-{}", names::hash8(&self.worktree))
        }
    }

    /// `<root>/wt/<id>`: the worktree's own output.
    pub fn dir(&self) -> PathBuf {
        self.root.join(WORKTREES).join(self.id())
    }

    /// `<root>/shared`.
    pub fn shared(&self) -> PathBuf {
        self.root.join(SHARED)
    }
}

string_enum! {
    /// A language `env` knows. Those that set no variable are accepted,
    /// and add nothing.
    pub enum Language {
        C => "c",
        Clojure => "clojure",
        Dotnet => "dotnet",
        Go => "go",
        Gradle => "gradle",
        Javascript => "javascript",
        Maven => "maven",
        Nix => "nix",
        Python => "python",
        Rust => "rust",
        Scala => "scala",
        Swift => "swift",
        Terraform => "terraform",
        Xcode => "xcode",
    }
}

/// Other names people give a language.
pub const ALIASES: [(&str, Language); 11] = [
    ("babashka", Language::Clojure),
    ("c++", Language::C),
    ("cpp", Language::C),
    ("csharp", Language::Dotnet),
    ("java", Language::Gradle),
    ("js", Language::Javascript),
    ("kotlin", Language::Gradle),
    ("opentofu", Language::Terraform),
    ("tofu", Language::Terraform),
    ("ts", Language::Javascript),
    ("typescript", Language::Javascript),
];

/// The files at a checkout's root that name each language, as globs.
const MARKERS: [(Language, &[&str]); 14] = [
    (Language::Rust, &["Cargo.toml"]),
    (Language::Swift, &["Package.swift"]),
    (Language::Xcode, &["*.xcodeproj", "*.xcworkspace"]),
    (Language::Javascript, &["package.json"]),
    (
        Language::Gradle,
        &[
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle*",
            "gradlew",
        ],
    ),
    (Language::Maven, &["pom.xml"]),
    (
        Language::Python,
        &[
            "pyproject.toml",
            "setup.py",
            "setup.cfg",
            "requirements*.txt",
            "uv.lock",
        ],
    ),
    (Language::Go, &["go.mod"]),
    (Language::Terraform, &["*.tf"]),
    (
        Language::Clojure,
        &["deps.edn", "project.clj", "bb.edn", "shadow-cljs.edn"],
    ),
    (Language::Dotnet, &["*.sln", "*.csproj", "*.fsproj"]),
    (Language::Scala, &["build.sbt", "build.mill", "build.sc"]),
    (
        Language::C,
        &["CMakeLists.txt", "meson.build", "configure.ac"],
    ),
    (Language::Nix, &["flake.nix", "default.nix", "shell.nix"]),
];

/// The languages whose marker files are among `names`, the names in a
/// checkout's root folder, in the order of [`MARKERS`].
pub fn detect<S: AsRef<OsStr>>(names: &[S]) -> Vec<Language> {
    let found = |glob: &str| {
        gix::glob::parse(glob).is_some_and(|pattern| {
            names.iter().any(|name| {
                pattern.matches(
                    name.as_ref().as_encoded_bytes().as_bstr(),
                    wildmatch::Mode::NO_MATCH_SLASH_LITERAL,
                )
            })
        })
    };
    MARKERS
        .iter()
        .filter(|(_, globs)| globs.iter().any(|glob| found(glob)))
        .map(|(language, _)| *language)
        .collect()
}

/// The languages a comma- or space-separated list names, each once, by
/// name or alias in any case; and the words that name none.
pub fn parse_languages(text: &str) -> (Vec<Language>, Vec<String>) {
    let words: Vec<String> = text
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect();
    let named = |word: &str| {
        ALIASES
            .iter()
            .find(|(alias, _)| *alias == word)
            .map(|(_, language)| *language)
            .or_else(|| Language::parse(word))
    };
    let languages = words.iter().filter_map(|word| named(word)).fold(
        Vec::new(),
        |seen: Vec<Language>, language| {
            if seen.contains(&language) {
                seen
            } else {
                seen.into_iter().chain([language]).collect()
            }
        },
    );
    let unknown = words
        .iter()
        .filter(|word| named(word).is_none())
        .cloned()
        .collect();
    (languages, unknown)
}

/// One line `env` prints: a variable set, or unset when `value` is None.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Export {
    pub name: String,
    pub value: Option<String>,
}

/// `WORKTREE_MANAGER_BUILD_ROOT`: the repository's build root.
pub fn root_variable() -> String {
    names::variable("BUILD_ROOT")
}

/// `WORKTREE_MANAGER_BUILD_DIR`: the worktree's own folder in it.
pub fn dir_variable() -> String {
    names::variable("BUILD_DIR")
}

/// `WORKTREE_MANAGER_EXPORTED`: the languages' variables `env` set, with
/// `:` between them.
pub fn exported_variable() -> String {
    names::variable("EXPORTED")
}

/// The flag that turns Gradle's build cache on.
pub const GRADLE_CACHING: &str = "-Dorg.gradle.caching=true";

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The variables `language` sets in `layout` on `platform`.
pub fn variables(
    language: Language,
    layout: &Layout,
    platform: Platform,
) -> Vec<(&'static str, String)> {
    let (shared, own) = (layout.shared(), layout.dir());
    match language {
        Language::C => vec![
            ("CCACHE_DIR", text(&shared.join("ccache"))),
            ("CCACHE_BASEDIR", text(&layout.worktree)),
        ],
        Language::Python => [
            ("PYTHONPYCACHEPREFIX", text(&shared.join("pycache"))),
            ("UV_PROJECT_ENVIRONMENT", text(&own.join("venv"))),
            ("RUFF_CACHE_DIR", text(&own.join("ruff"))),
            ("MYPY_CACHE_DIR", text(&own.join("mypy"))),
        ]
        .into_iter()
        .chain((platform == Platform::MacOs).then(|| ("UV_LINK_MODE", "clone".to_owned())))
        .collect(),
        Language::Rust => vec![
            ("CARGO_BUILD_BUILD_DIR", text(&shared.join("cargo-build"))),
            ("CARGO_TARGET_DIR", text(&own.join("target"))),
        ],
        Language::Swift => vec![(
            "CLANG_MODULE_CACHE_PATH",
            text(&shared.join("clang-modules")),
        )],
        Language::Terraform => vec![("TF_DATA_DIR", text(&own.join("tf")))],
        Language::Clojure
        | Language::Dotnet
        | Language::Go
        | Language::Gradle
        | Language::Javascript
        | Language::Maven
        | Language::Nix
        | Language::Scala
        | Language::Xcode => vec![],
    }
}

/// Whether any language could set `name`, so `env` may unset it.
fn settable(name: &str) -> bool {
    let layout = Layout {
        root: PathBuf::new(),
        worktree: PathBuf::new(),
        main: true,
    };
    Language::ALL.iter().any(|language| {
        variables(*language, &layout, Platform::MacOs)
            .iter()
            .any(|(known, _)| *known == name)
    })
}

/// The lines `env` prints for `languages` in `layout`: first the unsets of
/// what it set before that no language wants now, then the build root and
/// the worktree's folder, then each language's variables that aren't a
/// person's, then `GRADLE_OPTS` with [`GRADLE_CACHING`] added once for
/// Gradle, then [`exported_variable`] naming what it set.
pub fn exports(
    layout: &Layout,
    languages: &[Language],
    environment: &Environment,
    platform: Platform,
) -> Vec<Export> {
    let exported_before: Vec<&str> = environment
        .var(&exported_variable())
        .unwrap_or_default()
        .split(':')
        .filter(|name| settable(name))
        .collect();
    let wanted = languages
        .iter()
        .flat_map(|language| variables(*language, layout, platform))
        .fold(Vec::<(&str, String)>::new(), |seen, (name, value)| {
            if seen.iter().any(|(known, _)| *known == name) {
                seen
            } else {
                seen.into_iter().chain([(name, value)]).collect()
            }
        });
    let ours: Vec<(&str, String)> = wanted
        .into_iter()
        .filter(|(name, _)| environment.var(name).is_none() || exported_before.contains(name))
        .collect();
    let set = |name: String, value: String| Export {
        name,
        value: Some(value),
    };
    let unsets: Vec<Export> = exported_before
        .iter()
        .filter(|before| !ours.iter().any(|(name, _)| name == *before))
        .filter(|before| environment.var(before).is_some())
        .map(|name| Export {
            name: (*name).to_owned(),
            value: None,
        })
        .collect();
    let gradle_opts = environment.var("GRADLE_OPTS").unwrap_or_default();
    let gradle = (languages.contains(&Language::Gradle)
        && !gradle_opts
            .split_whitespace()
            .any(|word| word == GRADLE_CACHING))
    .then(|| {
        set(
            "GRADLE_OPTS".to_owned(),
            format!("{gradle_opts} {GRADLE_CACHING}").trim().to_owned(),
        )
    });
    let listed = ours
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(":");
    unsets
        .into_iter()
        .chain([
            set(root_variable(), text(&layout.root)),
            set(dir_variable(), text(&layout.dir())),
        ])
        .chain(
            ours.into_iter()
                .map(|(name, value)| set(name.to_owned(), value)),
        )
        .chain(gradle)
        .chain([set(exported_variable(), listed)])
        .collect()
}

/// The syntax of the export lines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Shell {
    #[default]
    Sh,
    Fish,
}

/// `value` in single quotes, as `sh` reads it.
pub fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// `value` in single quotes, as `fish` reads it.
pub fn fish_quote(value: &str) -> String {
    format!("'{}'", value.replace('\\', r"\\").replace('\'', r"\'"))
}

/// `exports` as lines of `shell`, each ending in a newline.
pub fn render(exports: &[Export], shell: Shell) -> String {
    exports
        .iter()
        .map(|export| match (shell, &export.value) {
            (Shell::Sh, Some(value)) => format!("export {}={}\n", export.name, sh_quote(value)),
            (Shell::Sh, None) => format!("unset {}\n", export.name),
            (Shell::Fish, Some(value)) => format!(
                "set --global --export {} {}\n",
                export.name,
                fish_quote(value)
            ),
            (Shell::Fish, None) => format!("set --erase --global {}\n", export.name),
        })
        .collect()
}

// Reading ---------------------------------------------------------------------

/// A worktree, as `env`, `provision` and `clean` find it. Everything but
/// the path given is read from the one repository gitoxide opens at the
/// worktree, the one `clean` deletes in, whatever `GIT_DIR` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub layout: Layout,
    /// The repository's common git directory, as a real path.
    pub common_dir: PathBuf,
    /// The main checkout, as a real path; None when the repository is bare.
    pub main_checkout: Option<PathBuf>,
    /// The lock's reason, empty when it gives none or can't be read, so a
    /// lock that can't be read still holds; None when unlocked.
    pub lock: Option<String>,
}

/// The worktree holding `path`. Outside a repository, and in a bare one,
/// it fails as [`ErrorCode::NotARepository`], so nothing is printed on
/// standard output.
pub fn place(system: &impl System, path: &Path) -> Result<Place, Failure> {
    let not_in = |what: &str| {
        Failure::new(
            ErrorCode::NotARepository,
            format!("{} is not in {what}", path.display()),
        )
    };
    let worktree = locate(system, path, Reach::Upward)?
        .ok_or_else(|| not_in("a repository"))?
        .worktree
        .ok_or_else(|| not_in("a worktree"))?;
    let failed = |error: &dyn std::fmt::Display| {
        Failure::unreadable(format!("{}: {error}", worktree.display()))
    };
    let repository = crate::locate::open(&worktree).map_err(|e| failed(&e))?;
    let own = repository.worktree();
    let main = own.as_ref().is_none_or(|own| own.is_main());
    let lock = own.filter(|own| own.is_locked()).map(|own| {
        own.lock_reason()
            .map(|reason| reason.to_str_lossy().trim().to_owned())
            .unwrap_or_default()
    });
    let main_checkout = repository
        .main_repo()
        .map_err(|e| failed(&e))?
        .workdir()
        .map(|dir| system.real_path(dir))
        .transpose()?;
    let common_dir = system.real_path(repository.common_dir())?;
    let root = names::build_root(system.environment(), &common_dir).ok_or_else(|| {
        Failure::unreadable("neither XDG_CACHE_HOME nor HOME is set, so there is no build root")
    })?;
    Ok(Place {
        layout: Layout {
            root,
            worktree,
            main,
        },
        common_dir,
        main_checkout,
        lock,
    })
}

/// Whether a file or link named `program` is in a folder of `PATH`. An
/// entry that is no folder, or can't be searched, holds nothing.
pub fn on_path(system: &impl System, program: &str) -> bool {
    system
        .environment()
        .var("PATH")
        .unwrap_or_default()
        .split(':')
        .filter(|folder| !folder.is_empty())
        .any(|folder| {
            matches!(
                system
                    .metadata(&Path::new(folder).join(program))
                    .ok()
                    .flatten()
                    .map(|meta| meta.kind),
                Some(FileKind::File | FileKind::Symlink)
            )
        })
}

#[cfg(test)]
mod tests;
