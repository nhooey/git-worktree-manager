//! `env`, `provision`, `clean` and `sessions`: each reads its facts through
//! the edge, takes its plan from [`crate::build_root`], and has the edge
//! carry it out.

use serde_json::{Map, Value, json};

use crate::build_root::{self, Env, Language, Layout, ProvisionAction, Shell};
use crate::clean;
use crate::names;
use crate::python::{self, path_join};
use crate::system::{self, Failure};

/// How a command ended, as its exit status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Done = 0,
    Failed = 1,
    Usage = 2,
    /// `clean` kept the worktree, and printed why.
    Refused = 3,
}

/// The command's result as JSON, or as text for a person.
fn emit(data: &Value, human: &str, as_json: bool) -> Result<(), Failure> {
    if as_json {
        system::print(&(python::json_dumps(data, Some(2)) + "\n"))
    } else {
        system::print(human)
    }
}

/// The path given, or the current directory when none or an empty one is.
fn given_or_here(path: Option<&str>) -> Result<String, Failure> {
    match path.filter(|p| !p.is_empty()) {
        Some(p) => Ok(p.to_owned()),
        None => system::current_dir(),
    }
}

const SETTINGS: &str = ".claude/settings.local.json";

// env and provision -----------------------------------------------------------

/// Provisions one worktree and returns what it did. Running it again does
/// nothing new.
fn provision(env: &Env, lay: &Layout) -> Result<Vec<ProvisionAction>, Failure> {
    system::ensure_build_dirs(env, lay)?;
    let main = if lay.is_main {
        lay.worktree.clone()
    } else {
        system::worktrees(&lay.worktree)?
            .first()
            .map(|w| w.path.clone())
            .unwrap_or_default()
    };
    let in_worktree = |name: &str| path_join(&lay.worktree, &[name]);
    let in_main = |name: &str| path_join(&main, &[name]);
    let actions = build_root::provision_actions(
        lay.is_main,
        system::exists(&in_worktree(SETTINGS)),
        system::is_file(&in_main(SETTINGS)),
        system::read_bytes(&in_worktree(".envrc")).as_deref(),
        system::read_bytes(&in_main(".envrc")).as_deref(),
        system::which(env, "direnv").is_some(),
    );
    actions.iter().try_for_each(|action| match action {
        ProvisionAction::CopySettings => {
            system::copy_new(&in_main(SETTINGS), &in_worktree(SETTINGS))
        }
        ProvisionAction::DirenvAllow => {
            system::run_checked(&["direnv", "allow", &lay.worktree], &[]).map(|_| ())
        }
    })?;
    system::write_text(&build_root::marker_path(lay), &(system::now_iso() + "\n"))?;
    Ok(actions)
}

pub struct EnvArgs {
    pub languages: Option<String>,
    pub shell: Shell,
    pub json: bool,
}

pub fn env(args: &EnvArgs, env: &Env) -> Result<Status, Failure> {
    let Some(lay) = system::layout_for(env, &system::current_dir()?) else {
        return Ok(Status::Done);
    };
    let provisioned = system::exists(&build_root::marker_path(&lay))
        && system::exists(&path_join(&lay.wt, &[names::WORKTREE_FILE]));
    if !provisioned {
        // A failure here must never cost the shell its exports.
        if let Err(error) = provision(env, &lay) {
            system::warn(&format!("provisioning {} failed: {error}", lay.worktree));
        }
    }
    let (languages, unknown) = match &args.languages {
        Some(text) => build_root::parse_languages(text),
        None => (
            build_root::detect_languages(&system::list_dir(&lay.worktree)?),
            Vec::new(),
        ),
    };
    if !unknown.is_empty() {
        let known: Vec<&str> = Language::ALL.iter().map(|l| l.as_str()).collect();
        system::warn(&format!(
            "unknown languages: {}; known: {}",
            unknown.join(", "),
            known.join(", ")
        ));
        return Ok(Status::Usage);
    }
    let plan = build_root::env_plan(
        &build_root::wanted_variables(&languages, &lay, system::platform()),
        &languages,
        env,
    );
    let exports: Map<String, Value> = plan
        .exports
        .iter()
        .map(|(name, value)| (name.clone(), Value::from(value.as_str())))
        .collect();
    emit(
        &json!({"exports": exports, "unsets": plan.unsets, "languages": languages}),
        &build_root::render_env(&plan, args.shell),
        args.json,
    )?;
    Ok(Status::Done)
}

pub struct ProvisionArgs {
    pub worktree: Option<String>,
    pub dry_run: bool,
    pub json: bool,
}

pub fn provision_command(args: &ProvisionArgs, env: &Env) -> Result<Status, Failure> {
    let path = given_or_here(args.worktree.as_deref())?;
    let Some(lay) = system::layout_for(env, &path) else {
        system::warn(&format!("{path} is not in a git worktree"));
        return Ok(Status::Failed);
    };
    if args.dry_run {
        emit(
            &json!({"worktree": lay.worktree, "would": true}),
            &format!("would provision {}\n", lay.worktree),
            args.json,
        )?;
        return Ok(Status::Done);
    }
    let actions = provision(env, &lay)?;
    let done: String = actions.iter().map(|a| format!(": {a}")).collect();
    emit(
        &json!({"worktree": lay.worktree, "actions": actions, "marker": build_root::marker_path(&lay)}),
        &format!("provisioned {}{done}\n", lay.worktree),
        args.json,
    )?;
    Ok(Status::Done)
}

// clean -----------------------------------------------------------------------

fn render_clean(listing: &clean::Listing, applied: bool) -> String {
    let verb = if applied { "deleted" } else { "would delete" };
    let lines: String = listing
        .build_dir
        .iter()
        .chain(&listing.remove)
        .map(|path| format!("{verb} {path}\n"))
        .collect();
    if lines.is_empty() {
        format!("nothing to clean in {}\n", listing.worktree)
    } else {
        lines
    }
}

pub struct CleanArgs {
    pub worktree: String,
    pub dry_run: bool,
    pub json: bool,
}

pub fn clean(args: &CleanArgs, env: &Env) -> Result<Status, Failure> {
    let cfg = system::load_config(env);
    let Some(found) = system::locate(&args.worktree) else {
        system::warn(&format!("{} is not in a git worktree", args.worktree));
        return Ok(Status::Failed);
    };
    let listed = system::worktrees(&found.toplevel)?;
    let main = listed.first().map_or("", |w| w.path.as_str());
    let Some(wt) = listed.iter().find(|w| w.path == found.toplevel) else {
        system::warn(&format!("{} is not a worktree of {main}", args.worktree));
        return Ok(Status::Failed);
    };
    let status = system::git(&wt.path, &["status", "--porcelain"])?;
    let changes: Vec<&str> = python::splitlines(&status)
        .into_iter()
        .filter(|line| !line.is_empty())
        .collect();
    let refusal = build_root::clean_refusals(wt.is_main, wt.bare, &changes);
    if !refusal.is_empty() {
        emit(
            &json!({"worktree": wt.path, "cleaned": false, "reasons": refusal}),
            &format!("kept {}: {}\n", wt.path, refusal.join("; ")),
            args.json,
        )?;
        return Ok(Status::Refused);
    }
    let listing = clean::listing(env, &cfg, main, &found.common_dir, &wt.path, wt.is_main)?;
    if !args.dry_run {
        clean::apply(&listing)?;
        let entries = listing.remove.len() + usize::from(listing.build_dir.is_some());
        system::log_at(
            &build_root::repo_state_dir(env, &found.common_dir),
            &found.common_dir,
            "clean",
            "clean",
            Map::from_iter([
                ("worktree".to_owned(), Value::from(wt.path.as_str())),
                ("entries".to_owned(), Value::from(entries)),
            ]),
        )?;
    }
    let data = match serde_json::to_value(&listing) {
        Ok(Value::Object(fields)) => Value::Object(
            fields
                .into_iter()
                .chain([
                    ("applied".to_owned(), Value::from(!args.dry_run)),
                    ("would".to_owned(), Value::from(args.dry_run)),
                ])
                .collect(),
        ),
        _ => Value::Null,
    };
    emit(&data, &render_clean(&listing, !args.dry_run), args.json)?;
    Ok(Status::Done)
}

// sessions --------------------------------------------------------------------

pub struct SessionsArgs {
    pub checkout: Option<String>,
    pub json: bool,
}

pub fn sessions(args: &SessionsArgs, env: &Env) -> Result<Status, Failure> {
    let path = given_or_here(args.checkout.as_deref())?;
    let checkout = match system::locate(&path) {
        Some(found) => system::worktrees(&found.toplevel)?
            .first()
            .map(|w| w.path.clone())
            .unwrap_or_default(),
        None => system::real(&path),
    };
    let prefix = build_root::claude_project_folder(&checkout);
    let projects = system::claude_projects(env);
    let mut folders: Vec<String> = if system::is_dir(&projects) {
        system::list_dir(&projects)?
            .into_iter()
            .filter(|name| name.starts_with(&prefix))
            .collect()
    } else {
        Vec::new()
    };
    folders.sort();
    let mut found: Vec<(String, String, system::FileStat)> = folders
        .iter()
        .map(|folder| {
            system::files_in(&path_join(&projects, &[folder])).map(|files| {
                files
                    .into_iter()
                    .filter_map(|(name, stat)| {
                        name.strip_suffix(".jsonl")
                            .map(|session| (session.to_owned(), folder.clone(), stat))
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    // Newest first; a stable sort keeps ties in the order they were found.
    found.sort_by(|a, b| b.2.modified.total_cmp(&a.2.modified));
    let sessions: Vec<Value> = found
        .iter()
        .map(|(session, project, stat)| {
            json!({"session": session, "project": project, "modified": stat.modified, "size": stat.size})
        })
        .collect();
    let rows: Vec<Vec<String>> = found
        .iter()
        .map(|(session, project, stat)| {
            vec![
                system::local_time(stat.modified, "%Y-%m-%d %H:%M"),
                session.clone(),
                project.clone(),
                build_root::human_bytes(Some(stat.size)),
            ]
        })
        .collect();
    let human = if rows.is_empty() {
        format!("no sessions for {checkout}\n")
    } else {
        build_root::table(&rows)
    };
    emit(
        &json!({"checkout": checkout, "sessions": sessions}),
        &human,
        args.json,
    )?;
    Ok(Status::Done)
}
