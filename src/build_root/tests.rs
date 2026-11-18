use super::*;

const OTHER: Platform = Platform::Other;

fn layout(main: bool) -> Layout {
    Layout {
        root: PathBuf::from("/cache/app-12345678"),
        worktree: PathBuf::from(if main {
            "/src/app"
        } else {
            "/src/app.worktrees/fix"
        }),
        main,
    }
}

fn environment(pairs: &[(&str, &str)]) -> Environment {
    Environment::from_pairs(pairs.iter().copied())
}

fn set(name: &str, value: &str) -> Export {
    Export {
        name: name.to_owned(),
        value: Some(value.to_owned()),
    }
}

fn unset(name: &str) -> Export {
    Export {
        name: name.to_owned(),
        value: None,
    }
}

fn listed(lines: &[Export]) -> String {
    lines
        .iter()
        .map(|export| export.name.as_str())
        .collect::<Vec<_>>()
        .join(":")
}

/// The lines every call gives, around the languages' own.
fn framed(own: &str, languages: &[Export], listed: &str) -> Vec<Export> {
    [
        set(&root_variable(), "/cache/app-12345678"),
        set(&dir_variable(), &format!("/cache/app-12345678/wt/{own}")),
    ]
    .into_iter()
    .chain(languages.iter().cloned())
    .chain([set(&exported_variable(), listed)])
    .collect()
}

fn fix_id() -> String {
    format!("fix-{}", names::hash8(Path::new("/src/app.worktrees/fix")))
}

#[test]
fn test_the_main_checkout_and_a_linked_worktree_have_their_own_folders() {
    assert_eq!(
        layout(true).dir(),
        PathBuf::from("/cache/app-12345678/wt/main")
    );
    assert_eq!(
        layout(false).dir(),
        PathBuf::from(format!("/cache/app-12345678/wt/{}", fix_id()))
    );
    assert_eq!(
        exports(&layout(true), &[], &Environment::default(), OTHER),
        framed("main", &[], "")
    );
}

#[test]
fn test_each_language_sets_its_row_of_variables() {
    let own = format!("/cache/app-12345678/wt/{}", fix_id());
    let shared = "/cache/app-12345678/shared";
    let python = [
        set("PYTHONPYCACHEPREFIX", &format!("{shared}/pycache")),
        set("UV_PROJECT_ENVIRONMENT", &format!("{own}/venv")),
        set("RUFF_CACHE_DIR", &format!("{own}/ruff")),
        set("MYPY_CACHE_DIR", &format!("{own}/mypy")),
    ];
    let rows: Vec<(Language, Platform, Vec<Export>)> = vec![
        (
            Language::C,
            OTHER,
            vec![
                set("CCACHE_DIR", &format!("{shared}/ccache")),
                set("CCACHE_BASEDIR", "/src/app.worktrees/fix"),
            ],
        ),
        (Language::Python, OTHER, python.to_vec()),
        (
            Language::Python,
            Platform::MacOs,
            python
                .iter()
                .cloned()
                .chain([set("UV_LINK_MODE", "clone")])
                .collect(),
        ),
        (
            Language::Rust,
            OTHER,
            vec![
                set("CARGO_BUILD_BUILD_DIR", &format!("{shared}/cargo-build")),
                set("CARGO_TARGET_DIR", &format!("{own}/target")),
            ],
        ),
        (
            Language::Swift,
            OTHER,
            vec![set(
                "CLANG_MODULE_CACHE_PATH",
                &format!("{shared}/clang-modules"),
            )],
        ),
        (
            Language::Terraform,
            OTHER,
            vec![set("TF_DATA_DIR", &format!("{own}/tf"))],
        ),
    ]
    .into_iter()
    .chain(
        [
            Language::Clojure,
            Language::Dotnet,
            Language::Go,
            Language::Javascript,
            Language::Maven,
            Language::Nix,
            Language::Scala,
            Language::Xcode,
        ]
        .into_iter()
        .flat_map(|language| {
            [
                (language, OTHER, vec![]),
                (language, Platform::MacOs, vec![]),
            ]
        }),
    )
    .collect();
    for (language, platform, lines) in rows {
        assert_eq!(
            exports(
                &layout(false),
                &[language],
                &Environment::default(),
                platform
            ),
            framed(&fix_id(), &lines, &listed(&lines)),
            "{language} on {platform:?}"
        );
    }
}

#[test]
fn test_gradle_opts_gains_the_build_cache_flag_once() {
    let gradle = |pairs: &[(&str, &str)]| {
        exports(
            &layout(true),
            &[Language::Gradle],
            &environment(pairs),
            OTHER,
        )
    };
    let with = |opts: &str| framed("main", &[set("GRADLE_OPTS", opts)], "");
    assert_eq!(gradle(&[]), with(GRADLE_CACHING));
    assert_eq!(
        gradle(&[("GRADLE_OPTS", "-Xmx2g")]),
        with(&format!("-Xmx2g {GRADLE_CACHING}"))
    );
    assert_eq!(
        gradle(&[("GRADLE_OPTS", &format!("-Xmx2g {GRADLE_CACHING}"))]),
        framed("main", &[], "")
    );
}

#[test]
fn test_a_language_named_twice_is_exported_once() {
    let once = exports(
        &layout(true),
        &[Language::Rust],
        &Environment::default(),
        OTHER,
    );
    let twice = exports(
        &layout(true),
        &[Language::Rust, Language::Rust],
        &Environment::default(),
        OTHER,
    );
    assert_eq!(once, twice);
}

#[test]
fn test_a_persons_variable_is_left_alone() {
    let lines = exports(
        &layout(true),
        &[Language::Rust],
        &environment(&[("CARGO_TARGET_DIR", "/mine")]),
        OTHER,
    );
    assert_eq!(
        lines,
        framed(
            "main",
            &[set(
                "CARGO_BUILD_BUILD_DIR",
                "/cache/app-12345678/shared/cargo-build"
            )],
            "CARGO_BUILD_BUILD_DIR"
        )
    );
}

#[test]
fn test_what_env_set_before_follows_the_worktree_or_is_unset() {
    let exported = exported_variable();
    let before = environment(&[
        ("CARGO_TARGET_DIR", "/cache/app-12345678/wt/other/target"),
        ("TF_DATA_DIR", "/cache/app-12345678/wt/other/tf"),
        (
            exported.as_str(),
            "CARGO_TARGET_DIR:TF_DATA_DIR:MYPY_CACHE_DIR",
        ),
    ]);
    let lines = exports(&layout(true), &[Language::Rust], &before, OTHER);
    let expected: Vec<Export> = std::iter::once(unset("TF_DATA_DIR"))
        .chain(framed(
            "main",
            &[
                set(
                    "CARGO_BUILD_BUILD_DIR",
                    "/cache/app-12345678/shared/cargo-build",
                ),
                set("CARGO_TARGET_DIR", "/cache/app-12345678/wt/main/target"),
            ],
            "CARGO_BUILD_BUILD_DIR:CARGO_TARGET_DIR",
        ))
        .collect();
    assert_eq!(lines, expected, "MYPY_CACHE_DIR is unset already");
}

#[test]
fn test_only_a_name_a_language_sets_is_ever_unset() {
    let exported = exported_variable();
    let before = environment(&[
        ("PATH", "/bin"),
        ("HOME", "/home"),
        (exported.as_str(), "x;touch /tmp/p:PATH:HOME:$(id)"),
    ]);
    let lines = exports(&layout(true), &[], &before, OTHER);
    assert_eq!(lines, framed("main", &[], ""));
}

#[test]
fn test_each_shell_reads_its_lines_back() {
    let lines = [set("A", "it's \\ here"), unset("B")];
    assert_eq!(
        render(&lines, Shell::Sh),
        "export A='it'\\''s \\ here'\nunset B\n"
    );
    assert_eq!(
        render(&lines, Shell::Fish),
        "set --global --export A 'it\\'s \\\\ here'\nset --erase --global B\n"
    );
}

#[test]
fn test_languages_are_detected_from_the_root_folders_names() {
    assert_eq!(
        detect(&[
            "Cargo.toml",
            "main.tf",
            "requirements-dev.txt",
            "README.md",
            "App.xcodeproj",
            "settings.gradle.kts",
            "flake.nix",
            "go.mod",
        ]),
        [
            Language::Rust,
            Language::Xcode,
            Language::Gradle,
            Language::Python,
            Language::Go,
            Language::Terraform,
            Language::Nix,
        ]
    );
    assert_eq!(detect(&["src", "notes.txt"]), []);
    assert_eq!(detect::<&str>(&[]), []);
}

#[test]
fn test_languages_are_read_by_name_or_alias_in_any_case() {
    assert_eq!(
        parse_languages("Rust, ts  tofu,,C++ rust\tbabashka"),
        (
            vec![
                Language::Rust,
                Language::Javascript,
                Language::Terraform,
                Language::C,
                Language::Clojure,
            ],
            vec![]
        )
    );
    assert_eq!(
        parse_languages("rust,cobol go fortran"),
        (
            vec![Language::Rust, Language::Go],
            vec!["cobol".to_owned(), "fortran".to_owned()]
        )
    );
    assert_eq!(parse_languages(""), (vec![], vec![]));
}
