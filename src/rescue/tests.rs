use super::*;

fn oid(digit: char) -> Oid {
    Oid::from_hex(&digit.to_string().repeat(40)).expect("an object name")
}

fn null() -> Oid {
    oid('0')
}

fn change(reference: &str, old: Oid, new: Oid) -> Event {
    Event {
        time: "2026-11-19T09:12:00Z".parse().expect("a time"),
        pid: 1,
        kind: EventKind::Ref {
            reference: reference.to_owned(),
            old,
            new,
        },
    }
}

fn candidate(branch: &str, tip: Oid) -> Candidate {
    Candidate {
        branch: branch.to_owned(),
        tip,
    }
}

#[test]
fn test_a_deleted_branch_is_a_candidate_at_the_tip_it_had() {
    let events = [
        change("refs/heads/fix", null(), oid('a')),
        change("refs/heads/fix", oid('a'), oid('b')),
        change("refs/heads/fix", oid('b'), null()),
    ];
    assert_eq!(candidates(&events), [candidate("fix", oid('b'))]);
}

#[test]
fn test_a_deletion_with_no_old_value_takes_the_last_tip_logged() {
    let events = [
        change("refs/heads/fix", null(), oid('a')),
        change("refs/heads/fix", null(), null()),
    ];
    assert_eq!(candidates(&events), [candidate("fix", oid('a'))]);
}

#[test]
fn test_a_branch_made_again_is_no_candidate() {
    let events = [
        change("refs/heads/fix", oid('a'), null()),
        change("refs/heads/fix", null(), oid('c')),
    ];
    assert_eq!(candidates(&events), []);
}

#[test]
fn test_a_deletion_with_no_tip_ever_logged_is_no_candidate() {
    assert_eq!(candidates(&[change("refs/heads/fix", null(), null())]), []);
}

#[test]
fn test_only_branches_are_candidates() {
    let events = [
        change("refs/remotes/origin/fix", oid('a'), null()),
        change("refs/tags/v1", oid('a'), null()),
        change("refs/heads/", oid('a'), null()),
    ];
    assert_eq!(candidates(&events), []);
}

#[test]
fn test_a_cut_name_or_one_with_a_replacement_character_is_never_a_candidate() {
    let events = [
        change(&format!("refs/heads/long{CUT_MARK}"), oid('a'), null()),
        change("refs/heads/caf\u{FFFD}", oid('b'), null()),
        change("refs/heads/kept", oid('c'), null()),
    ];
    assert_eq!(candidates(&events), [candidate("kept", oid('c'))]);
}

#[test]
fn test_the_log_is_read_a_line_at_a_time_past_any_line_that_isnt_an_event() {
    let line = |event: &Event| serde_json::to_string(event).expect("serialisable");
    let deleted = change("refs/heads/fix", oid('a'), null());
    let log = format!(
        "{}\nnot JSON\n{{\"time\":\"2026-11-19T09:12:00Z\"\n{}\n",
        line(&change("refs/heads/fix", null(), oid('a'))),
        line(&deleted)
    );
    assert_eq!(events(log.as_bytes()).last(), Some(&deleted));
    assert_eq!(events(log.as_bytes()).len(), 2);
}
