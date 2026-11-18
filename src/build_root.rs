//! The pure core of `env`, `provision` and `clean`.
//!
//! The build root, the language variables, provisioning, git's porcelain
//! output, `clean`'s plan and the formatting helpers. Nothing here reads
//! files, the clock or the environment, or runs a command: the edge passes
//! those in as plain values. [`crate::rules`] holds the sweep's decisions.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::names;
use crate::python::{self, basename, dirname, path_join};

/// The process environment, by variable name.
pub type Env = BTreeMap<String, String>;

/// A variable's value, unless it is unset or empty, as Python's `or` reads it.
fn var<'a>(env: &'a Env, name: &str) -> Option<&'a str> {
    env.get(name).map(String::as_str).filter(|v| !v.is_empty())
}

fn home(env: &Env) -> &str {
    env.get("HOME").map_or("", String::as_str)
}

// The build root ----------------------------------------------------------------

/// The first 8 hex digits of the SHA-256 of `text`.
pub fn sha8(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The repository's name from its common git directory.
///
/// `/x/nix-worktrees/.git` gives `nix-worktrees`; a bare `/x/foo.git`
/// gives `foo`.
pub fn repo_name(common_dir: &str) -> String {
    let trimmed = common_dir.trim_end_matches('/');
    let base = basename(trimmed);
    if base == ".git" {
        basename(dirname(trimmed)).to_owned()
    } else {
        base.strip_suffix(".git").unwrap_or(base).to_owned()
    }
}

/// `<repo>-<hash8>`, the per-repository folder name under the build and
/// state roots.
pub fn repo_key(common_dir: &str) -> String {
    format!("{}-{}", repo_name(common_dir), sha8(common_dir))
}

pub fn cache_home(env: &Env) -> String {
    var(env, "XDG_CACHE_HOME").map_or_else(|| path_join(home(env), &[".cache"]), str::to_owned)
}

pub fn state_home(env: &Env) -> String {
    var(env, "XDG_STATE_HOME")
        .map_or_else(|| path_join(home(env), &[".local", "state"]), str::to_owned)
}

pub fn config_home(env: &Env) -> String {
    var(env, "XDG_CONFIG_HOME").map_or_else(|| path_join(home(env), &[".config"]), str::to_owned)
}

/// `${XDG_CACHE_HOME:-$HOME/.cache}/worktree-builds`.
pub fn builds_top(env: &Env) -> String {
    path_join(&cache_home(env), &[names::BUILDS_DIR])
}

/// `<builds top>/<repo>-<hash8>`, from the real path of the common dir.
pub fn repo_build_root(env: &Env, common_dir: &str) -> String {
    path_join(&builds_top(env), &[&repo_key(common_dir)])
}

/// `~/backups/git-archive`: one bare repository of archives per project,
/// `<owner>/<repo>.git`.
pub fn archive_top(env: &Env) -> String {
    path_join(home(env), &["backups", "git-archive"])
}

/// `${XDG_STATE_HOME:-~/.local/state}/worktrees/<repo>-<hash8>`: the log
/// and the sweep's files.
pub fn repo_state_dir(env: &Env, common_dir: &str) -> String {
    path_join(&state_home(env), &[names::STATE_DIR, &repo_key(common_dir)])
}

/// `main` for the main checkout, else the folder's name and its path's hash.
pub fn wt_id(worktree: &str, is_main: bool) -> String {
    if is_main {
        "main".to_owned()
    } else {
        format!(
            "{}-{}",
            basename(worktree.trim_end_matches('/')),
            sha8(worktree)
        )
    }
}

/// Where one worktree's build output goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub root: String,
    pub wt: String,
    pub worktree: String,
    pub common_dir: String,
    pub git_dir: String,
    pub is_main: bool,
}

/// The layout of a worktree, from real paths of its git directories.
pub fn layout(env: &Env, common_dir: &str, git_dir: &str, worktree: &str) -> Layout {
    let is_main = git_dir.trim_end_matches('/') == common_dir.trim_end_matches('/');
    let root = repo_build_root(env, common_dir);
    Layout {
        wt: path_join(&root, &["wt", &wt_id(worktree, is_main)]),
        root,
        worktree: worktree.to_owned(),
        common_dir: common_dir.to_owned(),
        git_dir: git_dir.to_owned(),
        is_main,
    }
}

/// The provisioning marker, in the worktree's own git directory.
pub fn marker_path(lay: &Layout) -> String {
    path_join(&lay.git_dir, &[names::PROVISIONED_MARKER])
}

// Languages and variables ---------------------------------------------------------

string_enum! {
    /// A language whose build output `env` moves out of the worktree.
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
fn alias(word: &str) -> Option<Language> {
    Some(match word {
        "babashka" => Language::Clojure,
        "c++" | "cpp" => Language::C,
        "csharp" => Language::Dotnet,
        "java" | "kotlin" => Language::Gradle,
        "js" | "ts" | "typescript" => Language::Javascript,
        "opentofu" | "tofu" => Language::Terraform,
        _ => return None,
    })
}

/// Files at a checkout's root that name its languages, as fnmatch patterns.
const DETECT: &[(Language, &[&str])] = &[
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

pub const GRADLE_CACHING: &str = "-Dorg.gradle.caching=true";

/// The languages whose marker files are among `names`, in table order.
pub fn detect_languages<S: AsRef<str>>(names: &[S]) -> Vec<Language> {
    DETECT
        .iter()
        .filter(|(_, patterns)| {
            names.iter().any(|name| {
                patterns
                    .iter()
                    .any(|p| python::fnmatchcase(name.as_ref(), p))
            })
        })
        .map(|(language, _)| *language)
        .collect()
}

/// `(languages, unknown)` from a comma- or space-separated list.
pub fn parse_languages(text: &str) -> (Vec<Language>, Vec<String>) {
    let words: Vec<String> = text
        .split(|c: char| c == ',' || python::is_space(c))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    let named: Vec<Option<Language>> = words
        .iter()
        .map(|w| alias(w).or_else(|| Language::parse(w)))
        .collect();
    let languages = named
        .iter()
        .flatten()
        .fold(Vec::new(), |mut seen, language| {
            if !seen.contains(language) {
                seen.push(*language);
            }
            seen
        });
    let unknown = words
        .iter()
        .zip(&named)
        .filter(|(_, n)| n.is_none())
        .map(|(w, _)| w.clone())
        .collect();
    (languages, unknown)
}

/// The operating system, where a variable depends on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Darwin,
    Other,
}

fn pair(name: &str, value: String) -> (String, String) {
    (name.to_owned(), value)
}

/// The variables one language sets, per the README's table.
pub fn language_variables(
    language: Language,
    lay: &Layout,
    platform: Platform,
) -> Vec<(String, String)> {
    let (root, wt) = (&lay.root, &lay.wt);
    match language {
        Language::Rust => vec![
            pair(
                "CARGO_BUILD_BUILD_DIR",
                format!("{root}/shared/cargo-build"),
            ),
            pair("CARGO_TARGET_DIR", format!("{wt}/target")),
        ],
        Language::Swift => vec![pair(
            "CLANG_MODULE_CACHE_PATH",
            format!("{root}/shared/clang-modules"),
        )],
        Language::Python => [
            pair("PYTHONPYCACHEPREFIX", format!("{root}/shared/pycache")),
            pair("UV_PROJECT_ENVIRONMENT", format!("{wt}/venv")),
            pair("RUFF_CACHE_DIR", format!("{wt}/ruff")),
            pair("MYPY_CACHE_DIR", format!("{wt}/mypy")),
        ]
        .into_iter()
        .chain((platform == Platform::Darwin).then(|| pair("UV_LINK_MODE", "clone".to_owned())))
        .collect(),
        Language::Terraform => vec![pair("TF_DATA_DIR", format!("{wt}/tf"))],
        Language::C => vec![
            pair("CCACHE_DIR", format!("{root}/shared/ccache")),
            pair("CCACHE_BASEDIR", lay.worktree.clone()),
        ],
        _ => Vec::new(),
    }
}

/// Pairs as a Python dict keeps them: each name where it first appears,
/// with the last value given for it.
fn as_dict(pairs: impl IntoIterator<Item = (String, String)>) -> Vec<(String, String)> {
    pairs
        .into_iter()
        .fold(Vec::new(), |dict: Vec<(String, String)>, (name, value)| {
            if dict.iter().any(|(seen, _)| *seen == name) {
                dict.into_iter()
                    .map(|(n, v)| {
                        if n == name {
                            (n, value.clone())
                        } else {
                            (n, v)
                        }
                    })
                    .collect()
            } else {
                dict.into_iter().chain([(name, value)]).collect()
            }
        })
}

/// `WORKTREE_BUILDS_ROOT`, `WORKTREE_BUILDS_WT` and each language's.
pub fn wanted_variables(
    languages: &[Language],
    lay: &Layout,
    platform: Platform,
) -> Vec<(String, String)> {
    as_dict(
        [
            pair(names::BUILDS_ROOT_VAR, lay.root.clone()),
            pair(names::BUILDS_WT_VAR, lay.wt.clone()),
        ]
        .into_iter()
        .chain(
            languages
                .iter()
                .flat_map(|language| language_variables(*language, lay, platform)),
        ),
    )
}

/// What `env` prints: the variables to export, then those to unset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvPlan {
    pub exports: Vec<(String, String)>,
    pub unsets: Vec<String>,
}

/// What `env` prints, given the variables wanted and the environment.
///
/// A variable already set is left alone unless `env` set it itself, as
/// recorded in `WORKTREE_BUILDS_SET`, for another worktree, which
/// `WORKTREE_BUILDS_WT` names. Names it set before that are no longer
/// wanted are unset. `GRADLE_OPTS` gains the build cache flag.
pub fn env_plan(wanted: &[(String, String)], languages: &[Language], env: &Env) -> EnvPlan {
    let always = [names::BUILDS_ROOT_VAR, names::BUILDS_WT_VAR];
    let ours_before: BTreeSet<&str> = env
        .get(names::BUILDS_SET_VAR)
        .map_or("", String::as_str)
        .split(':')
        .filter(|n| !n.is_empty())
        .collect();
    let wanted_map: BTreeMap<&str, &str> = wanted
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    let wanted_wt = wanted_map.get(names::BUILDS_WT_VAR).copied();
    let moved = env
        .get(names::BUILDS_WT_VAR)
        .map(String::as_str)
        .or(wanted_wt)
        != wanted_wt;
    let take: Vec<(String, String)> = wanted
        .iter()
        .filter(|(n, _)| {
            always.contains(&n.as_str())
                || !env.contains_key(n)
                || (moved && ours_before.contains(n.as_str()))
        })
        .cloned()
        .collect();
    let owned = take
        .iter()
        .map(|(n, _)| n.as_str())
        .chain(
            ours_before
                .iter()
                .copied()
                .filter(|n| wanted_map.contains_key(n)),
        )
        .collect::<BTreeSet<&str>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(":");
    let gradle_opts = env.get("GRADLE_OPTS").map_or("", String::as_str);
    let gradle = (languages.contains(&Language::Gradle)
        && !python::split_whitespace(gradle_opts).any(|word| word == GRADLE_CACHING))
    .then(|| {
        pair(
            "GRADLE_OPTS",
            python::strip(&format!("{gradle_opts} {GRADLE_CACHING}")).to_owned(),
        )
    });
    EnvPlan {
        exports: take
            .into_iter()
            .chain(gradle)
            .chain([pair(names::BUILDS_SET_VAR, owned)])
            .collect(),
        unsets: ours_before
            .into_iter()
            .filter(|n| !wanted_map.contains_key(n) && env.contains_key(*n))
            .map(str::to_owned)
            .collect(),
    }
}

pub fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

pub fn fish_quote(value: &str) -> String {
    format!("'{}'", value.replace('\\', r"\\").replace('\'', r"\'"))
}

/// The shell `env` prints for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    Sh,
    Fish,
}

/// The plan as `sh` or `fish` lines.
pub fn render_env(plan: &EnvPlan, shell: Shell) -> String {
    let unset = |n: &str| match shell {
        Shell::Fish => format!("set -e {n}"),
        Shell::Sh => format!("unset {n}"),
    };
    let export = |n: &str, v: &str| match shell {
        Shell::Fish => format!("set -gx {n} {}", fish_quote(v)),
        Shell::Sh => format!("export {n}={}", sh_quote(v)),
    };
    plan.unsets
        .iter()
        .map(|n| unset(n))
        .chain(plan.exports.iter().map(|(n, v)| export(n, v)))
        .map(|line| line + "\n")
        .collect()
}

// Provisioning ----------------------------------------------------------------------

string_enum! {
    pub enum ProvisionAction {
        CopySettings => "copy-settings",
        DirenvAllow => "direnv-allow",
    }
}

/// `copy-settings` and `direnv-allow`, where each applies.
pub fn provision_actions(
    is_main: bool,
    worktree_has_settings: bool,
    main_has_settings: bool,
    worktree_envrc: Option<&[u8]>,
    main_envrc: Option<&[u8]>,
    direnv_available: bool,
) -> Vec<ProvisionAction> {
    [
        (
            ProvisionAction::CopySettings,
            !is_main && main_has_settings && !worktree_has_settings,
        ),
        (
            ProvisionAction::DirenvAllow,
            !is_main
                && direnv_available
                && worktree_envrc.is_some()
                && worktree_envrc == main_envrc,
        ),
    ]
    .into_iter()
    .filter(|(_, applies)| *applies)
    .map(|(action, _)| action)
    .collect()
}

// Git's output -------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    pub path: String,
    pub head: String,
    pub branch: Option<String>,
    pub detached: bool,
    pub bare: bool,
    pub locked: Option<String>,
    pub prunable: bool,
    pub is_main: bool,
}

fn worktree_from_block(block: &[&str], is_main: bool) -> Worktree {
    // `worktree <path>`, `HEAD <sha>`, `branch <ref>`, `locked <reason>`…
    let fields: BTreeMap<&str, &str> = block
        .iter()
        .map(|line| line.split_once(' ').unwrap_or((line, "")))
        .collect();
    let field = |key: &str| fields.get(key).map(|v| (*v).to_owned());
    Worktree {
        path: field("worktree").unwrap_or_default(),
        head: field("HEAD").unwrap_or_default(),
        branch: fields
            .get("branch")
            .map(|r| r.strip_prefix("refs/heads/").unwrap_or(r).to_owned()),
        detached: fields.contains_key("detached"),
        bare: fields.contains_key("bare"),
        locked: field("locked"),
        prunable: fields.contains_key("prunable"),
        is_main,
    }
}

/// `git worktree list --porcelain`; the first entry is the main checkout.
pub fn parse_worktree_list(text: &str) -> Vec<Worktree> {
    python::splitlines(text)
        .split(|line| line.is_empty())
        .filter(|block| !block.is_empty())
        .enumerate()
        .map(|(i, block)| worktree_from_block(block, i == 0))
        .collect()
}

/// The paths of `git clean -n`'s `Would remove <path>` lines.
pub fn parse_would_remove(text: &str) -> Vec<String> {
    python::splitlines(text)
        .into_iter()
        .filter_map(|line| line.strip_prefix("Would remove "))
        .map(str::to_owned)
        .collect()
}

pub fn split_nul(text: &str) -> Vec<String> {
    text.split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Patterns of a gitignore-syntax file: no blanks, no comments.
pub fn parse_pattern_file(text: &str) -> Vec<String> {
    python::splitlines(text)
        .into_iter()
        .filter(|line| !python::strip(line).is_empty() && !python::lstrip(line).starts_with('#'))
        .map(|line| python::rstrip(line).to_owned())
        .collect()
}

pub fn is_inside(path: &str, folder: &str) -> bool {
    let base = folder.trim_end_matches('/');
    path == base
        || path
            .strip_prefix(base)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Claude Code's `~/.claude/projects/` folder name for a working directory.
pub fn claude_project_folder(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

// `clean` ------------------------------------------------------------------------------

/// `git status --porcelain` lines other than untracked and ignored ones.
pub fn tracked_changes<S: AsRef<str>>(changes: &[S]) -> Vec<String> {
    changes
        .iter()
        .map(AsRef::as_ref)
        .filter(|c| !c.starts_with("??") && !c.starts_with("!!"))
        .map(str::to_owned)
        .collect()
}

/// Why `clean` refuses a worktree; empty when it may clean.
pub fn clean_refusals<S: AsRef<str>>(is_main: bool, bare: bool, changes: &[S]) -> Vec<String> {
    let tracked = tracked_changes(changes).len();
    [
        (is_main, "the main checkout".to_owned()),
        (bare, "a bare repository".to_owned()),
        (tracked > 0, format!("{tracked} tracked files with changes")),
    ]
    .into_iter()
    .filter(|(applies, _)| *applies)
    .map(|(_, reason)| reason)
    .collect()
}

pub fn under(path: &str, entry: &str) -> bool {
    path == entry.trim_end_matches('/') || (entry.ends_with('/') && path.starts_with(entry))
}

/// `(whole, mixed)` from `git clean -ndX`'s entries.
///
/// A whole entry holds nothing kept and no nested repository, and goes at
/// once. A mixed one is a folder holding a kept file or a nested
/// repository; only its own ignored, unkept files go. A kept file entry
/// stays.
pub fn clean_plan<S: AsRef<str>, N: AsRef<str>>(
    entries: &[S],
    kept: &BTreeSet<String>,
    nested: &[N],
) -> (Vec<String>, Vec<String>) {
    let guarded: Vec<&str> = kept
        .iter()
        .map(String::as_str)
        .chain(nested.iter().map(AsRef::as_ref))
        .collect();
    let listed: Vec<&str> = entries.iter().map(AsRef::as_ref).collect();
    (
        listed
            .iter()
            .filter(|e| !guarded.iter().any(|g| under(g, e)))
            .map(|e| (*e).to_owned())
            .collect(),
        listed
            .iter()
            .filter(|e| e.ends_with('/') && guarded.iter().any(|g| under(g, e) && g != *e))
            .map(|e| (*e).to_owned())
            .collect(),
    )
}

/// Ignored files, not nested repositories, that no keep pattern holds.
pub fn files_to_remove<S: AsRef<str>>(ignored: &[S], kept: &BTreeSet<String>) -> Vec<String> {
    ignored
        .iter()
        .map(AsRef::as_ref)
        .filter(|p| !p.ends_with('/') && !kept.contains(*p))
        .map(str::to_owned)
        .collect()
}

// Formatting ----------------------------------------------------------------------------

pub fn human_bytes(n: Option<u64>) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let Some(n) = n else {
        return "?".to_owned();
    };
    let exp = (0..UNITS.len() - 1)
        .take_while(|i| u128::from(n) >= 1024u128.pow(*i as u32 + 1))
        .count();
    if exp == 0 {
        format!("{n}B")
    } else {
        // `{:.0}` rounds a tie to even, as Python's `{:.0f}` does.
        format!("{:.0}{}", n as f64 / 1024f64.powi(exp as i32), UNITS[exp])
    }
}

pub fn human_age(seconds: Option<f64>) -> String {
    let whole = |value: f64| value as i64;
    match seconds {
        None => "?".to_owned(),
        Some(s) if s < 60.0 => format!("{}s", whole(s)),
        Some(s) if s < 3600.0 => format!("{}m", whole(python::floor_div(s, 60.0))),
        Some(s) if s < 86400.0 => format!("{}h", whole(python::floor_div(s, 3600.0))),
        Some(s) => format!("{}d", whole(python::floor_div(s, 86400.0))),
    }
}

/// Rows as left-aligned columns separated by two spaces.
pub fn table<S: AsRef<str>>(rows: &[Vec<S>]) -> String {
    let width = |cell: &S| cell.as_ref().chars().count();
    let widths: Vec<usize> = rows.first().map_or_else(Vec::new, |first| {
        (0..first.len())
            .map(|i| {
                rows.iter()
                    .filter_map(|r| r.get(i))
                    .map(width)
                    .max()
                    .unwrap_or(0)
            })
            .collect()
    });
    rows.iter()
        .map(|row| {
            let line = row
                .iter()
                .zip(&widths)
                .map(|(cell, w)| format!("{:<w$}", cell.as_ref()))
                .collect::<Vec<_>>()
                .join("  ");
            python::rstrip(&line).to_owned() + "\n"
        })
        .collect()
}

/// User settings over the defaults: objects merge, everything else replaces.
pub fn merge_config(
    defaults: &serde_json::Map<String, Value>,
    user: &serde_json::Map<String, Value>,
) -> serde_json::Map<String, Value> {
    user.iter()
        .fold(defaults.clone(), |mut merged, (key, value)| {
            let combined = match (value, defaults.get(key)) {
                (Value::Object(mine), Some(Value::Object(shipped))) => {
                    Value::Object(merge_config(shipped, mine))
                }
                _ => value.clone(),
            };
            merged.insert(key.clone(), combined);
            merged
        })
}

/// The keys of the user's settings that the defaults lack, dotted.
pub fn unknown_keys(
    defaults: &serde_json::Map<String, Value>,
    user: &serde_json::Map<String, Value>,
) -> Vec<String> {
    unknown_keys_under(defaults, user, "")
}

fn unknown_keys_under(
    defaults: &serde_json::Map<String, Value>,
    user: &serde_json::Map<String, Value>,
    prefix: &str,
) -> Vec<String> {
    user.iter()
        .flat_map(|(key, value)| match (value, defaults.get(key)) {
            (Value::Object(mine), Some(Value::Object(shipped))) => {
                unknown_keys_under(shipped, mine, &format!("{prefix}{key}."))
            }
            (_, Some(_)) => Vec::new(),
            (_, None) => vec![format!("{prefix}{key}")],
        })
        .collect()
}

#[cfg(test)]
mod tests;
