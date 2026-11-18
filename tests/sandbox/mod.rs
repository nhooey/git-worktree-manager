//! A sandbox for the command tests: a temporary HOME, and repositories with
//! a bare remote.
//!
//! Every removal in the tests happens here, in a temporary directory with
//! HOME and the XDG folders pointed inside it, never in a real checkout.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use git_worktree_manager::build_root;
use serde_json::Value;

/// The program under test, found by the package's name.
pub const BIN: &str = env!(concat!("CARGO_BIN_EXE_", env!("CARGO_PKG_NAME")));

pub const GITIGNORE: &str = "\
.env*
.envrc*
*.local.*
.claude/
.idea/
*.iml
.superpowers/
HANDOFF.md
*.tfplan
state/
*.tfstate*
.xtdb/
.nrepl-port
skills/
.gstack/
reports/
secret.db
build/
target/
*.o
nested/
";

/// What a run of the program returned.
pub struct Ran {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn real(path: &str) -> String {
    fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_owned())
}

pub fn join(base: &str, rel: &str) -> String {
    Path::new(base).join(rel).to_string_lossy().into_owned()
}

pub fn exists(path: &str) -> bool {
    Path::new(path).exists()
}

pub fn read(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Sets a file's access and modification times to `seconds` since the epoch.
pub fn utime(path: &str, seconds: f64) {
    let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs_f64(seconds);
    fs::File::options()
        .write(true)
        .open(path)
        .and_then(|file| file.set_times(fs::FileTimes::new().set_accessed(at).set_modified(at)))
        .unwrap_or_else(|e| panic!("{path}: {e}"));
}

pub struct Sandbox {
    pub tmp: String,
    pub env: Vec<(String, String)>,
    // Deletes the sandbox when the test ends.
    _dir: tempfile::TempDir,
}

impl Sandbox {
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("worktrees-test-")
            .tempdir()
            .expect("a temporary directory");
        let tmp = real(&dir.path().to_string_lossy());
        let home = join(&tmp, "home");
        fs::create_dir_all(&home).expect("HOME");
        let gitconfig = join(&tmp, "gitconfig");
        fs::write(
            &gitconfig,
            "[user]\n\tname = Test\n\temail = test@example.invalid\n\
             [commit]\n\tgpgsign = false\n[tag]\n\tgpgsign = false\n\
             [init]\n\tdefaultBranch = main\n\
             [advice]\n\tdetachedHead = false\n\
             [protocol \"file\"]\n\tallow = always\n",
        )
        .expect("gitconfig");
        let env = [
            (
                "PATH",
                std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_owned()),
            ),
            ("HOME", home),
            ("XDG_CACHE_HOME", join(&tmp, "cache")),
            ("XDG_STATE_HOME", join(&tmp, "state")),
            ("XDG_CONFIG_HOME", join(&tmp, "config")),
            ("CLAUDE_CONFIG_DIR", join(&tmp, "claude")),
            ("GIT_CONFIG_GLOBAL", gitconfig),
            ("GIT_CONFIG_NOSYSTEM", "1".to_owned()),
            ("TMPDIR", tmp.clone()),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect();
        Sandbox {
            tmp,
            env,
            _dir: dir,
        }
    }

    pub fn var(&self, name: &str) -> String {
        self.env
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }

    fn command(&self, program: &str, args: &[&str], cwd: Option<&str>) -> Command {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(cwd.unwrap_or(&self.tmp))
            .env_clear()
            .envs(self.env.iter().map(|(n, v)| (n.as_str(), v.as_str())))
            .stdin(Stdio::null());
        command
    }

    // Running things -----------------------------------------------------------

    pub fn sh(&self, args: &[&str], cwd: Option<&str>) -> String {
        let (program, rest) = args.split_first().expect("a command");
        let output = self
            .command(program, rest, cwd)
            .output()
            .unwrap_or_else(|e| panic!("{args:?}: {e}"));
        assert!(
            output.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    pub fn git(&self, repo: &str, args: &[&str]) -> String {
        let argv: Vec<&str> = ["git", "-C", repo]
            .into_iter()
            .chain(args.iter().copied())
            .collect();
        self.sh(&argv, None)
    }

    pub fn wt(&self, args: &[&str], cwd: Option<&str>, env: &[(&str, &str)]) -> Ran {
        let output = self
            .command(BIN, args, cwd)
            .envs(env.iter().copied())
            .output()
            .expect("the program runs");
        Ran {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    pub fn wt_json(&self, args: &[&str], cwd: Option<&str>, code: i32) -> Value {
        let with_json: Vec<&str> = args.iter().copied().chain(["--json"]).collect();
        let result = self.wt(&with_json, cwd, &[]);
        assert_eq!(result.code, code, "{}{}", result.stderr, result.stdout);
        serde_json::from_str(&result.stdout).expect("JSON")
    }

    // Repositories ---------------------------------------------------------------

    pub fn write(&self, path: &str, text: &str) {
        if let Some(parent) = Path::new(path).parent() {
            fs::create_dir_all(parent).expect("folders");
        }
        fs::write(path, text).expect("a file");
    }

    pub fn commit(&self, repo: &str, name: &str, text: &str) -> String {
        self.write(&join(repo, name), text);
        self.git(repo, &["add", "--", name]);
        self.git(repo, &["commit", "--quiet", "-m", &format!("add {name}")]);
        self.git(repo, &["rev-parse", "HEAD"]).trim().to_owned()
    }

    /// A repository with no remote.
    pub fn repo(&self, files: &[(&str, &str)]) -> String {
        let path = join(&self.tmp, "repo");
        self.sh(&["git", "init", "--quiet", &path], None);
        self.commit(&path, ".gitignore", GITIGNORE);
        files.iter().for_each(|(rel, text)| {
            self.commit(&path, rel, text);
        });
        path
    }

    /// A clone of a bare remote, `repo.remote.git`, with `origin/HEAD` at
    /// `main`.
    pub fn cloned(&self) -> String {
        let remote = join(&self.tmp, "repo.remote.git");
        self.sh(&["git", "init", "--quiet", "--bare", &remote], None);
        self.git(&remote, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        let path = self.repo(&[]);
        self.git(&path, &["remote", "add", "origin", &remote]);
        self.git(
            &path,
            &["push", "--quiet", "--set-upstream", "origin", "main"],
        );
        self.git(&path, &["remote", "set-head", "origin", "main"]);
        path
    }

    pub fn add_worktree(&self, repo: &str, branch: &str) -> String {
        let target = join(&format!("{repo}.worktrees"), &branch.replace('/', "-"));
        self.git(
            repo,
            &["worktree", "add", "--quiet", "-b", branch, &target, "HEAD"],
        );
        real(&target)
    }

    pub fn state_dir(&self, repo: &str) -> String {
        let common = real(
            self.git(
                repo,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .trim(),
        );
        build_root::repo_state_dir(&self.env.iter().cloned().collect(), &common)
    }

    pub fn events(&self, repo: &str) -> Vec<Value> {
        let path = join(&self.state_dir(repo), "events.jsonl");
        if !exists(&path) {
            return Vec::new();
        }
        read(&path)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("a JSON line"))
            .collect()
    }

    pub fn listed(&self, repo: &str) -> Vec<String> {
        self.git(repo, &["worktree", "list", "--porcelain"])
            .lines()
            .filter_map(|line| line.strip_prefix("worktree "))
            .map(|path| {
                if exists(path) {
                    real(path)
                } else {
                    path.to_owned()
                }
            })
            .collect()
    }

    pub fn entry(&self, report: &Value, kind: &str, key: &str) -> Value {
        let field = if kind == "worktrees" {
            "path"
        } else {
            "branch"
        };
        report["repositories"][0][kind]
            .as_array()
            .into_iter()
            .flatten()
            .find(|e| e[field] == key)
            .cloned()
            .unwrap_or_else(|| panic!("no {kind} entry {key}"))
    }
}
