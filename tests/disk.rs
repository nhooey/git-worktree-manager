//! `disk`: the floors' sources and their order, the states at each floor,
//! and the command over the fake system and as the binary.

mod sandbox;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use git_worktree_manager::cli;
use git_worktree_manager::commands::Status;
use git_worktree_manager::commands::disk::{Args, run_in};
use git_worktree_manager::disk::{
    self, DEFAULT_FLOORS, DiskState, Floors, FloorsFile, Measured, NIX, Source,
};
use git_worktree_manager::environment::Environment;
use git_worktree_manager::locate::Located;
use git_worktree_manager::model::ids::ErrorCode;
use git_worktree_manager::names::{self, PROGRAM, Variable};
use git_worktree_manager::system::Fake;
use sandbox::{Sandbox, join};

fn build() -> String {
    Variable::FloorBuildGb.name()
}

fn cleanup_only() -> String {
    Variable::FloorCleanupOnlyGb.name()
}

fn environment(pairs: &[(&str, &str)]) -> Environment {
    Environment::from_pairs(pairs.iter().copied())
}

const FILE: &str = "/home/me/.config/floor";

fn file(text: &str) -> Option<FloorsFile<'_>> {
    Some(FloorsFile {
        path: Path::new(FILE),
        text,
    })
}

fn at_line(line: usize) -> Source {
    Source::File {
        path: PathBuf::from(FILE),
        line,
    }
}

// The floors ------------------------------------------------------------------------

#[test]
fn test_with_nothing_set_the_floors_are_70_and_35() {
    let sourced = disk::floors(&environment(&[]), None).expect("floors");
    assert_eq!(
        sourced.floors,
        Floors {
            build_gb: 70.0,
            cleanup_only_gb: 35.0
        }
    );
    assert_eq!(
        (sourced.build, sourced.cleanup_only),
        (Source::Default, Source::Default)
    );
    assert_eq!(
        disk::floors(&environment(&[]), file("")).expect("floors"),
        disk::floors(&environment(&[]), None).expect("floors"),
    );
}

#[test]
fn test_the_file_sets_each_floor_from_its_last_line_unquoted() {
    let text = format!(
        "# floors, with {}=1 commented out\n\n{} = 80\nnot a setting\n{}='40'\n{}=\"75\"\n",
        build(),
        build(),
        cleanup_only(),
        build()
    );
    let sourced = disk::floors(&environment(&[]), file(&text)).expect("floors");
    assert_eq!(
        sourced.floors,
        Floors {
            build_gb: 75.0,
            cleanup_only_gb: 40.0
        }
    );
    assert_eq!(
        (sourced.build, sourced.cleanup_only),
        (at_line(6), at_line(5))
    );
    assert_eq!(sourced.warnings, Vec::<String>::new());
}

#[test]
fn test_a_file_line_whose_key_is_no_floor_is_ignored_with_a_warning() {
    let text = format!("OTHER=x\nexport {}=10\n{}=80\n", build(), build());
    let sourced = disk::floors(&environment(&[]), file(&text)).expect("floors");
    assert_eq!((sourced.floors.build_gb, sourced.build), (80.0, at_line(3)));
    assert_eq!(
        sourced.warnings,
        [
            format!("{FILE} line 1: OTHER is not a floor, and is ignored"),
            format!(
                "{FILE} line 2: export {} is not a floor, and is ignored",
                build()
            ),
        ]
    );
}

#[test]
fn test_a_variable_wins_over_the_file_trimmed_and_an_empty_one_is_unset() {
    let text = format!("{}=80\n{}=40\n", build(), cleanup_only());
    let sourced = disk::floors(
        &environment(&[(&build(), " 90 "), (&cleanup_only(), "")]),
        file(&text),
    )
    .expect("floors");
    assert_eq!(
        sourced.floors,
        Floors {
            build_gb: 90.0,
            cleanup_only_gb: 40.0
        }
    );
    assert_eq!(
        (sourced.build, sourced.cleanup_only),
        (Source::Variable(build()), at_line(2))
    );
}

#[test]
fn test_a_bad_variable_fails_naming_it() {
    for value in ["seventy", "-1", "-0", "inf", "1e400", "NaN", "70GB", " 7 0"] {
        let failure =
            disk::floors(&environment(&[(&cleanup_only(), value)]), None).expect_err(value);
        assert_eq!(failure.code, ErrorCode::Unreadable);
        assert_eq!(
            failure.detail,
            format!(
                "{} is {:?}, not a number of GB",
                cleanup_only(),
                value.trim()
            )
        );
    }
}

#[test]
fn test_a_bad_file_line_fails_naming_the_file_and_line() {
    let text = format!("{}=70\n{}=35GB\n", build(), cleanup_only());
    let failure = disk::floors(&environment(&[]), file(&text)).expect_err("a bad line");
    assert_eq!(failure.code, ErrorCode::Unreadable);
    assert_eq!(
        failure.detail,
        format!(
            "{FILE} line 2: {} is \"35GB\", not a number of GB",
            cleanup_only()
        )
    );
}

#[test]
fn test_a_build_floor_below_the_cleanup_only_floor_fails() {
    let text = format!("{}=40\n", cleanup_only());
    let failure = disk::floors(&environment(&[(&build(), "10")]), file(&text))
        .expect_err("floors out of order");
    assert_eq!(failure.code, ErrorCode::Unreadable);
    assert_eq!(
        failure.detail,
        format!(
            "the build floor, 10 GB from {}, is below the cleanup-only floor, 40 GB from \
             {FILE} line 1",
            build()
        )
    );
    let equal = disk::floors(&environment(&[(&build(), "40")]), file(&text)).expect("floors");
    assert_eq!(
        equal.floors,
        Floors {
            build_gb: 40.0,
            cleanup_only_gb: 40.0
        }
    );
}

// The state -------------------------------------------------------------------------

#[test]
fn test_the_state_at_each_floor_and_a_byte_either_side() {
    let states: Vec<(u64, DiskState)> = [
        70_000_000_001,
        70_000_000_000,
        69_999_999_999,
        35_000_000_001,
        35_000_000_000,
        34_999_999_999,
        0,
    ]
    .into_iter()
    .map(|bytes| (bytes, disk::state(disk::gb(bytes), &DEFAULT_FLOORS)))
    .collect();
    assert_eq!(
        states,
        [
            (70_000_000_001, DiskState::Ok),
            (70_000_000_000, DiskState::Ok),
            (69_999_999_999, DiskState::BelowBuild),
            (35_000_000_001, DiskState::BelowBuild),
            (35_000_000_000, DiskState::BelowBuild),
            (34_999_999_999, DiskState::CleanupOnly),
            (0, DiskState::CleanupOnly),
        ]
    );
}

// The volumes -----------------------------------------------------------------------

#[test]
fn test_each_volume_is_reported_once_under_its_first_path() {
    let measured =
        [("/a", 1, 80), ("/b", 2, 50), ("/c", 1, 80), ("/d", 3, 10)].map(|(path, volume, gb)| {
            Measured {
                path: PathBuf::from(path),
                volume,
                free_bytes: gb * 1_000_000_000,
            }
        });
    let report = disk::report(DEFAULT_FLOORS, &measured);
    let disks: Vec<(&Path, f64, DiskState)> = report
        .disks
        .iter()
        .map(|disk| (disk.path.as_path(), disk.free_gb, disk.state))
        .collect();
    assert_eq!(
        disks,
        [
            (Path::new("/a"), 80.0, DiskState::Ok),
            (Path::new("/b"), 50.0, DiskState::BelowBuild),
            (Path::new("/d"), 10.0, DiskState::CleanupOnly),
        ]
    );
}

#[test]
fn test_the_places_are_the_build_root_the_checkout_the_backup_and_nix() {
    let environment = environment(&[("HOME", "/home/me")]);
    let common_dir = PathBuf::from("/srv/app.git");
    let bare = Located {
        common_dir: common_dir.clone(),
        git_dir: common_dir.clone(),
        worktree: None,
    };
    let build_root = names::build_root(&environment, &common_dir).expect("a build root");
    let backup = names::backup_root(&environment).expect("a backup");
    assert_eq!(
        disk::places(&environment, &bare, false),
        [build_root.clone(), common_dir.clone(), backup.clone()]
    );
    assert_eq!(
        disk::places(&environment, &bare, true),
        [build_root, common_dir.clone(), backup, PathBuf::from(NIX)]
    );
    assert_eq!(
        disk::places(&Environment::default(), &bare, false),
        [common_dir]
    );
}

// The command over the fake system --------------------------------------------------

const HOME: &str = "/home/me";
const REPOSITORY: &str = "/home/me/repo";

fn located() -> Located {
    Located {
        common_dir: PathBuf::from(REPOSITORY).join(".git"),
        git_dir: PathBuf::from(REPOSITORY).join(".git"),
        worktree: Some(PathBuf::from(REPOSITORY)),
    }
}

/// The repository and the backup on `/home`, the build root's cache on
/// `/data`, and the floors file setting the build floor to 70.
///
/// Neither the build root nor the backup exists yet. Each lies under a
/// mount whose folder doesn't exist either, and only the climb to the
/// nearest folder that does reads the volume it is on.
fn fake(pairs: &[(&str, &str)]) -> Fake {
    let environment = environment(
        &[("HOME", HOME), ("XDG_CACHE_HOME", "/data/cache")]
            .into_iter()
            .chain(pairs.iter().copied())
            .collect::<Vec<_>>(),
    );
    let floor_file = names::floor_file(&environment).expect("a floors file");
    Fake::new("2026-11-19T09:12:00Z".parse().expect("a time"))
        .with_environment(environment)
        .with_file(join(REPOSITORY, ".git/HEAD"), "ref: refs/heads/main\n")
        .with_file("/data/other", "")
        .with_file(floor_file, format!("{}=70\n", build()))
        .with_volume("/", 500_000_000_000)
        .with_volume("/home", 69_999_999_999)
        .with_volume("/data", 30_000_000_000)
        .with_volume("/data/cache", 1)
        .with_volume(join(HOME, "backups"), 2)
}

fn args(format: &str) -> Args {
    match cli::parse([PROGRAM, "disk", "--format", format].map(OsString::from)) {
        Ok(cli::Command::Disk(args)) => args,
        other => panic!("{other:?}"),
    }
}

fn build_root(system: &Fake) -> PathBuf {
    names::build_root(&system.environment, &located().common_dir).expect("a build root")
}

#[test]
fn test_disk_prints_the_floors_and_each_volume_as_text() {
    let mut system = fake(&[]);
    assert_eq!(
        run_in(&args("text"), &located(), &mut system),
        Ok(Status::Done)
    );
    let floor_file = names::floor_file(&system.environment).expect("a floors file");
    assert_eq!(
        system.stdout,
        format!(
            "build floor: 70 GB ({} line 1)\n\
             cleanup-only floor: 35 GB (the default)\n\
             {}: 30.00 GB free, cleanup-only\n\
             {REPOSITORY}: 69.99 GB free, below-build\n",
            floor_file.display(),
            build_root(&system).display(),
        )
    );
    assert_eq!(
        (system.stderr.as_str(), system.acts.as_slice()),
        ("", &[][..])
    );
}

#[test]
fn test_disk_prints_the_nodes_json_unrounded_with_nix() {
    let mut system = fake(&[(&cleanup_only(), "20")])
        .with_file(join(NIX, "store/x"), "")
        .with_volume(NIX, 200_000_000_000);
    assert_eq!(
        run_in(&args("json"), &located(), &mut system),
        Ok(Status::Done)
    );
    let printed: serde_json::Value = serde_json::from_str(&system.stdout).expect("JSON");
    assert_eq!(
        printed,
        serde_json::json!({
            "schema": 1,
            "command": "disk",
            "floors": {"build_gb": 70.0, "cleanup_only_gb": 20.0},
            "disks": [
                {"path": build_root(&system), "free_gb": 30.0, "state": "below-build"},
                {"path": REPOSITORY, "free_gb": disk::gb(69_999_999_999), "state": "below-build"},
                {"path": NIX, "free_gb": 200.0, "state": "ok"},
            ],
        })
    );
    assert!(system.acts.is_empty());
}

#[test]
fn test_the_floors_files_warnings_go_to_standard_error() {
    let floor_file = names::floor_file(&fake(&[]).environment).expect("a floors file");
    let text = format!("{}=70\nexport {}=80\n", build(), build());
    let mut system = fake(&[]).with_file(&floor_file, text);
    assert_eq!(
        run_in(&args("json"), &located(), &mut system),
        Ok(Status::Done)
    );
    assert_eq!(
        system.stderr,
        format!(
            "{PROGRAM}: {} line 2: export {} is not a floor, and is ignored\n",
            floor_file.display(),
            build()
        )
    );
}

#[test]
fn test_outside_a_repository_disk_prints_nothing_and_fails() {
    // A real folder, which the fake holds too, outside any repository.
    let s = Sandbox::new();
    for format in ["text", "json"] {
        let mut system = Fake {
            current_dir: PathBuf::from(&s.tmp),
            ..Fake::new("2026-11-19T09:12:00Z".parse().expect("a time"))
        }
        .with_file(join(&s.tmp, "notes"), "");
        let argv = [PROGRAM, "disk", "--format", format].map(OsString::from);
        assert_eq!(cli::run(argv, &mut system), Status::Failed);
        assert_eq!(system.stdout, "", "{format}");
        assert_eq!(
            system.stderr,
            format!("{PROGRAM}: {} is not in a repository\n", s.tmp)
        );
    }
}

// The binary -------------------------------------------------------------------------

/// The build root of the repository whose common git directory is
/// `common_dir`, in the sandbox.
fn sandbox_build_root(s: &Sandbox, common_dir: &str) -> String {
    names::build_root(&s.environment(), Path::new(common_dir))
        .expect("a build root")
        .to_string_lossy()
        .into_owned()
}

/// The disks are `first`, the one volume the sandbox is on, then `/nix`
/// only when it exists. Where `/nix` exists it may still be missing, when
/// it shares the sandbox's volume, which is the machine's to decide.
fn assert_disks(printed: &serde_json::Value, first: &str) {
    let paths: Vec<&str> = printed["disks"]
        .as_array()
        .expect("disks")
        .iter()
        .filter_map(|disk| disk["path"].as_str())
        .collect();
    let allowed: Vec<Vec<&str>> = if Path::new(NIX).exists() {
        vec![vec![first, NIX], vec![first]]
    } else {
        vec![vec![first]]
    };
    assert!(allowed.contains(&paths), "{paths:?}");
}

#[test]
fn test_the_binary_prints_the_build_floor_and_creates_nothing() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    let floor_file = names::floor_file(&s.environment()).expect("a floors file");
    s.write(
        &floor_file.to_string_lossy(),
        &format!("{}=1\n{}=0.5\n", build(), cleanup_only()),
    );
    let ran = s.run(
        &["disk", "--format", "json"],
        Some(&repo),
        &[(&cleanup_only(), "0.25")],
    );
    assert_eq!(ran.code, 0, "{}", ran.stderr);
    let printed: serde_json::Value = serde_json::from_str(&ran.stdout).expect("JSON");
    assert_eq!(printed["floors"]["build_gb"], 1.0);
    assert_eq!(printed["floors"]["cleanup_only_gb"], 0.25);
    assert_disks(&printed, &sandbox_build_root(&s, &join(&repo, ".git")));
    let common_dir = Path::new(&repo).join(".git");
    let untouched = [
        PathBuf::from(s.var("XDG_CACHE_HOME")),
        PathBuf::from(s.var("XDG_STATE_HOME")),
        names::state_dir(&s.environment(), &common_dir).expect("a state folder"),
        names::backup_root(&s.environment()).expect("a backup"),
    ];
    for path in untouched {
        assert!(!path.exists(), "{}", path.display());
    }
}

#[test]
fn test_the_binary_fails_on_a_bad_floor_with_the_error_json() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    let ran = s.run(
        &["disk", "--format", "json"],
        Some(&repo),
        &[(&build(), "lots")],
    );
    let detail = format!("{} is \"lots\", not a number of GB", build());
    assert_eq!(ran.code, 1);
    let printed: serde_json::Value = serde_json::from_str(&ran.stdout).expect("JSON");
    assert_eq!(
        printed,
        serde_json::json!({
            "schema": 1,
            "command": "disk",
            "error": {"code": "unreadable", "detail": detail},
        })
    );
    assert_eq!(ran.stderr, format!("{PROGRAM}: {detail}\n"));
}

#[test]
fn test_the_binary_outside_a_repository_prints_nothing_and_exits_1() {
    let s = Sandbox::new();
    let ran = s.run(&["disk", "--format", "json"], None, &[]);
    assert_eq!((ran.code, ran.stdout.as_str()), (1, ""));
    assert_eq!(
        ran.stderr,
        format!("{PROGRAM}: {} is not in a repository\n", s.tmp)
    );
}

#[test]
fn test_the_binary_reads_the_repository_it_is_given() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    let ran = s.run(
        &["disk", "--repository", &repo, "--format", "json"],
        None,
        &[],
    );
    assert_eq!(ran.code, 0, "{}", ran.stderr);
    let printed: serde_json::Value = serde_json::from_str(&ran.stdout).expect("JSON");
    assert_disks(&printed, &sandbox_build_root(&s, &join(&repo, ".git")));
}

#[test]
fn test_the_binary_given_a_folder_outside_a_repository_prints_nothing_and_exits_1() {
    let s = Sandbox::new();
    let home = s.var("HOME");
    let ran = s.run(
        &["disk", "--repository", &home, "--format", "json"],
        None,
        &[],
    );
    assert_eq!((ran.code, ran.stdout.as_str()), (1, ""));
    assert_eq!(
        ran.stderr,
        format!("{PROGRAM}: {home} is not in a repository\n")
    );
}

#[test]
fn test_the_binary_reads_a_bare_repository() {
    let s = Sandbox::new();
    let bare = s.bare(&join(&s.tmp, "app.git"));
    let ran = s.run(
        &["disk", "--repository", &bare, "--format", "json"],
        None,
        &[],
    );
    assert_eq!(ran.code, 0, "{}", ran.stderr);
    let printed: serde_json::Value = serde_json::from_str(&ran.stdout).expect("JSON");
    assert_disks(&printed, &sandbox_build_root(&s, &bare));
}
