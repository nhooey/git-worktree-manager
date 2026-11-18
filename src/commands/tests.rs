use super::*;
use crate::names::PROGRAM;
use crate::system::Fake;

fn system() -> Fake {
    Fake::new("2026-11-19T09:12:00Z".parse().expect("a time"))
}

#[test]
fn test_a_failure_says_why_on_standard_error() {
    let mut system = system();
    let failure = Failure::step_failed("git worktree remove failed");
    assert_eq!(
        failed(&mut system, CommandName::Finish, false, &failure),
        Status::Failed
    );
    assert_eq!(system.stdout, "");
    assert_eq!(
        system.stderr,
        format!("{PROGRAM}: git worktree remove failed\n")
    );
}

#[test]
fn test_a_failure_under_json_prints_the_error_shape() {
    let mut system = system();
    let failure = Failure::unreadable("repos.json: no such file");
    assert_eq!(
        failed(&mut system, CommandName::Status, true, &failure),
        Status::Failed
    );
    let printed: serde_json::Value = serde_json::from_str(&system.stdout).expect("JSON");
    assert_eq!(
        printed,
        serde_json::json!({
            "schema": 1,
            "command": "status",
            "error": {"code": "unreadable", "detail": "repos.json: no such file"},
        })
    );
    assert_eq!(
        system.stderr,
        format!("{PROGRAM}: repos.json: no such file\n")
    );
}

#[test]
fn test_outside_a_repository_nothing_is_printed_on_standard_output() {
    for json in [false, true] {
        let mut system = system();
        let failure = Failure::new(ErrorCode::NotARepository, "/tmp is not in a repository");
        assert_eq!(
            failed(&mut system, CommandName::Env, json, &failure),
            Status::Failed
        );
        assert_eq!(system.stdout, "", "json: {json}");
        assert_eq!(
            system.stderr,
            format!("{PROGRAM}: /tmp is not in a repository\n")
        );
    }
}
