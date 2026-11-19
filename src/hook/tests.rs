use serde_json::{Value, json};

use super::*;
use crate::model::Oid;
use crate::model::event::{Event, EventKind};

const ZERO: &str = "0000000000000000000000000000000000000000";
const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const DEFAULT: &str = "refs/remotes/origin/main";

fn time() -> Timestamp {
    "2026-11-19T09:12:00Z".parse().expect("a time")
}

fn hooked(input: &str) -> Hooked {
    lines(input, Some(DEFAULT), time(), 42)
}

fn parsed(line: &str) -> Value {
    assert!(line.ends_with('\n'), "{line:?}");
    assert_eq!(line.matches('\n').count(), 1, "{line:?}");
    serde_json::from_str(line).expect("JSON")
}

fn event(reference: &str, old: &str, new: &str) -> Value {
    json!({
        "time": "2026-11-19T09:12:00Z",
        "pid": 42,
        "event": "ref",
        "ref": reference,
        "old": old,
        "new": new,
    })
}

#[test]
fn test_a_creation_an_update_and_a_deletion() {
    let got = hooked(&format!(
        "{ZERO} {A} refs/heads/new\n{A} {B} refs/heads/moved\n{B} {ZERO} refs/heads/gone\n"
    ));
    assert_eq!(
        got.lines
            .iter()
            .map(|line| parsed(line))
            .collect::<Vec<_>>(),
        [
            event("refs/heads/new", ZERO, A),
            event("refs/heads/moved", A, B),
            event("refs/heads/gone", B, ZERO),
        ]
    );
    assert!(!got.sweep);
    assert_eq!(got.malformed, 0);
}

#[test]
fn test_json_strings_escape_as_serde_does() {
    let samples: Vec<String> = (0..=0x7f_u8)
        .map(char::from)
        .chain(['é', '\u{2028}', '\u{fffd}', '😀'])
        .map(String::from)
        .chain(["refs/heads/a\"b\\c\n\u{1}\u{1f}é".to_owned(), String::new()])
        .collect();
    for sample in samples {
        assert_eq!(
            json_string(&sample),
            serde_json::to_string(&sample).expect("JSON"),
            "{sample:?}"
        );
    }
}

#[test]
fn test_a_line_is_what_serde_writes_for_the_event() {
    let time: Timestamp = "2026-11-19T09:12:00.123456789Z".parse().expect("a time");
    let reference = "refs/heads/a\"b\\c\u{1}é";
    let input = format!("{ZERO} {A} {reference}\n{A} {B} refs/heads/x\n{B} {ZERO} refs/heads/y\n");
    let got = lines(&input, None, time, 7);
    let expected = [
        (reference, ZERO, A),
        ("refs/heads/x", A, B),
        ("refs/heads/y", B, ZERO),
    ]
    .map(|(reference, old, new)| Event {
        time,
        pid: 7,
        kind: EventKind::Ref {
            reference: reference.to_owned(),
            old: Oid::from_hex(old).expect("an object name"),
            new: Oid::from_hex(new).expect("an object name"),
        },
    });
    assert_eq!(got.lines.len(), expected.len());
    for (line, event) in got.lines.iter().zip(&expected) {
        assert_eq!(
            serde_json::from_str::<Event>(line).expect("an event"),
            *event
        );
        assert_eq!(
            line.trim_end(),
            serde_json::to_string(event).expect("JSON"),
            "{line}"
        );
    }
}

#[test]
fn test_a_line_is_the_logs_event() {
    let got = hooked(&format!("{ZERO} {A} refs/heads/fix\n"));
    let read: Event = serde_json::from_str(&got.lines[0]).expect("an event");
    assert_eq!(
        read.kind,
        EventKind::Ref {
            reference: "refs/heads/fix".to_owned(),
            old: Oid::from_hex(ZERO).expect("null"),
            new: Oid::from_hex(A).expect("an object name"),
        }
    );
}

#[test]
fn test_the_default_ref_among_many_starts_the_sweep() {
    let input: String = (0..1000)
        .map(|i| format!("{A} {B} refs/remotes/origin/b{i:04}\n"))
        .chain([format!("{A} {B} {DEFAULT}\n")])
        .collect();
    let got = hooked(&input);
    assert_eq!(got.lines.len(), 1001);
    assert!(got.sweep);
}

#[test]
fn test_no_default_ref_starts_no_sweep() {
    let input = format!("{A} {B} {DEFAULT}\n");
    assert!(!lines(&input, None, time(), 42).sweep);
    assert!(!lines(&input, Some("refs/remotes/origin/trunk"), time(), 42).sweep);
    assert!(!hooked(&format!("{A} {B} {DEFAULT}-not\n")).sweep);
}

#[test]
fn test_malformed_lines_are_counted_and_not_logged() {
    let got = hooked(&format!(
        "garbage\n{A} {B}\n{A} nothex refs/heads/x\n{A} {B} \n{A}{B} refs/heads/y\n\n{ZERO} {A} refs/heads/ok"
    ));
    assert_eq!(
        got.lines
            .iter()
            .map(|line| parsed(line))
            .collect::<Vec<_>>(),
        [event("refs/heads/ok", ZERO, A)]
    );
    assert_eq!(got.malformed, 5);
}

#[test]
fn test_a_symbolic_ref_is_neither_logged_nor_malformed() {
    let got = hooked(&format!(
        "{ZERO} ref:{DEFAULT} refs/remotes/origin/HEAD\nref:refs/heads/a ref:refs/heads/b HEAD\n"
    ));
    assert_eq!(got.lines, Vec::<String>::new());
    assert_eq!(got.malformed, 0);
    assert!(!got.sweep);
}

#[test]
fn test_a_deletion_with_no_old_value_is_logged() {
    let got = hooked(&format!(
        "{ZERO} {ZERO} refs/heads/gone\n{ZERO} {ZERO} {DEFAULT}\n"
    ));
    assert_eq!(
        got.lines
            .iter()
            .map(|line| parsed(line))
            .collect::<Vec<_>>(),
        [
            event("refs/heads/gone", ZERO, ZERO),
            event(DEFAULT, ZERO, ZERO)
        ]
    );
    assert!(got.sweep);
}

#[test]
fn test_a_symbolic_side_keeps_the_other_sides_object_name() {
    let got = hooked(&format!(
        "{A} ref:refs/heads/main HEAD\nref:refs/heads/main {B} HEAD\n"
    ));
    assert_eq!(
        got.lines
            .iter()
            .map(|line| parsed(line))
            .collect::<Vec<_>>(),
        [event("HEAD", A, ZERO), event("HEAD", ZERO, B)]
    );
    assert_eq!(got.malformed, 0);
}

#[test]
fn test_quotes_and_backslashes_in_a_ref_stay_json() {
    let got = hooked(&format!("{ZERO} {A} refs/heads/a\"b\\c\n"));
    assert_eq!(parsed(&got.lines[0])["ref"], "refs/heads/a\"b\\c");
}

#[test]
fn test_an_over_long_ref_is_cut_and_says_so() {
    for unit in ["x", "\"", "\u{1}", "é", "😀"] {
        let reference = format!("refs/heads/{}", unit.repeat(5000));
        let got = hooked(&format!("{A} {B} {reference}\n"));
        let line = &got.lines[0];
        assert!(line.len() < LINE_LIMIT, "{unit:?}: {} bytes", line.len());
        assert!(
            line.len() > LINE_LIMIT - 8,
            "{unit:?}: {} bytes",
            line.len()
        );
        let value = parsed(line);
        let cut = value["ref"].as_str().expect("a ref");
        assert!(cut.ends_with(CUT_MARK), "{unit:?}");
        assert!(
            reference.starts_with(cut.strip_suffix(CUT_MARK).expect("marked")),
            "{unit:?}"
        );
        assert_eq!(value["ref_bytes"], reference.len(), "{unit:?}");
        assert!(serde_json::from_str::<Event>(line).is_ok(), "{unit:?}");
        assert_eq!(value["new"], B);
    }
}

#[test]
fn test_a_ref_that_fits_has_no_ref_bytes() {
    let frame = hooked(&format!("{A} {B} r\n")).lines[0].len() - 1;
    let longest = format!("r{}", "x".repeat(LINE_LIMIT - 1 - frame - 1));
    let got = hooked(&format!("{A} {B} {longest}\n"));
    assert_eq!(got.lines[0].len(), LINE_LIMIT - 1);
    assert_eq!(parsed(&got.lines[0])["ref"], longest.as_str());
    assert_eq!(parsed(&got.lines[0]).get("ref_bytes"), None);
    let over = format!("{longest}x");
    let value = parsed(&hooked(&format!("{A} {B} {over}\n")).lines[0]);
    assert_eq!(value["ref_bytes"], over.len());
}

#[test]
fn test_the_default_ref_is_matched_before_any_cut() {
    let long = format!("refs/remotes/origin/{}", "x".repeat(5000));
    let got = lines(&format!("{A} {B} {long}\n"), Some(&long), time(), 42);
    assert!(got.sweep);
}

#[test]
fn test_writes_are_whole_lines_under_the_limit() {
    let input: String = (0..1000)
        .map(|i| format!("{A} {B} refs/remotes/origin/{}\n", "x".repeat(i % 300)))
        .chain([format!("{A} {B} refs/heads/{}\n", "y".repeat(5000))])
        .collect();
    let lines = hooked(&input).lines;
    let got = writes(&lines);
    assert_eq!(got.concat(), lines.concat());
    assert!(got.iter().all(|write| write.len() < LINE_LIMIT));
    let ends = |texts: &[String]| -> Vec<usize> {
        texts
            .iter()
            .scan(0, |end, text| {
                *end += text.len();
                Some(*end)
            })
            .collect()
    };
    let line_ends = ends(&lines);
    assert!(ends(&got).iter().all(|end| line_ends.contains(end)));
    assert!(got.len() < lines.len() / 10, "{} writes", got.len());
    assert_eq!(writes(&[]), Vec::<String>::new());
}

#[test]
fn test_the_shim_is_three_lines_that_exec_the_program() {
    let text = shim(
        std::path::Path::new("/nix/store/x/bin/it's"),
        std::path::Path::new("/state/events.jsonl"),
        DEFAULT,
    );
    let log = crate::names::Variable::Log.name();
    let default = crate::names::Variable::DefaultRef.name();
    let program = r"'/nix/store/x/bin/it'\''s'";
    assert_eq!(
        text,
        format!(
            "#!/bin/sh\n\
             [ \"$1\" = committed ] && [ -x {program} ] || exit 0\n\
             {log}='/state/events.jsonl' {default}='{DEFAULT}' exec {program} hook {HOOK} \"$@\"\n"
        )
    );
}

#[test]
fn test_the_shims_words_are_the_command() {
    let words = ["hook", HOOK, COMMITTED].map(std::ffi::OsString::from);
    let command = crate::cli::parse(
        std::iter::once(std::ffi::OsString::from(crate::names::PROGRAM)).chain(words),
    )
    .expect("parses");
    assert_eq!(command.name(), crate::model::ids::CommandName::Hook);
}

#[test]
fn test_empty_input_does_nothing() {
    assert_eq!(
        hooked(""),
        Hooked {
            lines: vec![],
            sweep: false,
            malformed: 0
        }
    );
}
