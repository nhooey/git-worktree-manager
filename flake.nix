{
  description = "Keep git worktrees' build output outside them and shared, provision them, remove finished ones and keep builds off a full disk";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    flake-parts = {
      url = "github:hercules-ci/flake-parts";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };

    systems.url = "github:nix-systems/default";

    # `agent-skill-flake` is the builder library, not a skill — it provides the
    # `flakeModules.devshellSkills` flake-parts module that wires the dev-shell
    # skill set in below. That module bundles numtide/devshell, so this flake
    # needs no `devshell` input of its own. The skill sources themselves are NOT
    # inputs here: they live only in the `skills-devshell/` sub-flake's lock,
    # which this dev shell invokes at RUNTIME (never as a root input), keeping
    # this flake a leaf with zero skill inputs.
    agent-skill-flake = {
      url = "github:nhooey/agent-skill-flake";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    # Only for checks.home-manager-module, which evaluates the Home Manager
    # module in test configurations; the module itself needs no input.
    home-manager = {
      url = "github:nix-community/home-manager";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    inputs@{
      flake-parts,
      systems,
      ...
    }:
    flake-parts.lib.mkFlake { inherit inputs; } (
      { withSystem, flake-parts-lib, ... }:
      {
        systems = import systems;

        imports = [
          flake-parts.flakeModules.flakeModules
          inputs.agent-skill-flake.flakeModules.devshellSkills
          inputs.treefmt-nix.flakeModule
        ];

        # For a repository's dev shell: `worktrees` as a command, and the
        # shared build-cache variables of `worktrees env` when it loads.
        flake.flakeModules.default = flake-parts-lib.importApply ./flake-module.nix {
          inherit withSystem;
        };

        # For a Home Manager configuration: `programs.worktrees`, the program
        # on the PATH, the direnv and fish loaders, the scheduled sweep and
        # the disk floors.
        flake.homeManagerModules.default = flake-parts-lib.importApply ./home-manager/module.nix {
          inherit withSystem;
        };

        # numtide/devshell installs its launcher as $DEVSHELL_DIR/bin/<name>,
        # so the name must not be a command's: inside this devshell it would
        # shadow the real command.
        agent-skill-flake.devshellSkills = {
          name = "nix-worktrees-dev";
        };

        perSystem =
          {
            pkgs,
            lib,
            config,
            system,
            ...
          }:
          let
            # What the program runs besides Python: git, lsof for the processes
            # inside a worktree, ps (adv_cmds on macOS, procps elsewhere; a Nix
            # builder can't run macOS's setuid /bin/ps), and df and du.
            runtimeInputs = [
              pkgs.git
              pkgs.lsof
              pkgs.ps
              pkgs.coreutils
            ];

            # The disk floor hook's df: see packages.hooks-plugin.
            floorDf = if pkgs.stdenv.hostPlatform.isDarwin then "/bin/df" else lib.getExe' pkgs.coreutils "df";
          in
          {
            packages = {
              worktrees = pkgs.python3Packages.buildPythonApplication {
                pname = "worktrees";
                # The one version, `__version__` in worktrees/__init__.py.
                version = lib.pipe ./worktrees/__init__.py [
                  builtins.readFile
                  (lib.splitString "\n")
                  (lib.findFirst (lib.hasPrefix "__version__ = ") "")
                  (lib.removePrefix "__version__ = \"")
                  (lib.removeSuffix "\"")
                ];
                pyproject = true;
                src = lib.fileset.toSource {
                  root = ./.;
                  fileset = lib.fileset.unions [
                    ./pyproject.toml
                    ./README.md
                    ./worktrees
                  ];
                };
                build-system = [ pkgs.python3Packages.setuptools ];
                # git first, for `rev-parse --path-format`; the rest after the
                # user's own, so the system's ps and lsof win on macOS.
                makeWrapperArgs = [
                  "--prefix"
                  "PATH"
                  ":"
                  (lib.makeBinPath [ pkgs.git ])
                  "--suffix"
                  "PATH"
                  ":"
                  (lib.makeBinPath runtimeInputs)
                ];
                # The tests run in `checks.tests`, over temporary repositories.
                doCheck = false;
                meta = {
                  description = "A git worktree's shared build caches, provisioning, cleanup and removal";
                  license = lib.licenses.asl20;
                  mainProgram = "worktrees";
                };
              };
              default = config.packages.worktrees;

              # The Claude Code plugin `nix-worktrees-hooks`: plugin/ with
              # its scripts' jq and git as store paths, the disk floor hook
              # run by dash and its df the system's on macOS, which start
              # in half the time of /bin/sh and coreutils' df there and keep
              # it under 10 ms before every Bash command. The same derivation
              # as agent-plugins-nixpkgs' `lib.mkClaudePlugin`, name and
              # version from the manifest, `passthru.claudePluginName` and
              # `passthru.version` for its `mkMarketplace`; built here since
              # agent-plugins-nixpkgs takes this flake as an input, and an
              # input back would be a cycle.
              hooks-plugin =
                let
                  manifest = lib.importJSON ./plugin/.claude-plugin/plugin.json;
                in
                pkgs.runCommand "claude-plugin-${manifest.name}-${manifest.version}"
                  {
                    passthru = {
                      claudePluginName = manifest.name;
                      inherit (manifest) version;
                    };
                  }
                  ''
                    cp -r ${./plugin} $out
                    chmod -R u+w $out
                    substituteInPlace $out/scripts/hook-lib.sh \
                      --replace-fail 'WT_JQ=jq' 'WT_JQ=${lib.getExe pkgs.jq}' \
                      --replace-fail 'WT_GIT=git' 'WT_GIT=${lib.getExe pkgs.git}'
                    substituteInPlace $out/scripts/disk-floor.sh \
                      --replace-fail 'WT_JQ=jq' 'WT_JQ=${lib.getExe pkgs.jq}' \
                      --replace-fail 'WORKTREES_FLOOR_DF:-df}' 'WORKTREES_FLOOR_DF:-${floorDf}}'
                    substituteInPlace $out/hooks/hooks.json \
                      --replace-fail '"command": "sh ' '"command": "${lib.getExe pkgs.dash} '
                  '';
            };

            checks = {
              package = config.packages.worktrees;

              # The built plugin's hook scripts: shellcheck, then
              # tests/plugin/hooks.bats with a stub `worktrees` over temporary
              # repositories, which also checks hooks.json and the manifest,
              # and tests/plugin/disk-floor.bats with a stub df, run by dash
              # as hooks.json runs it. Its run-time test, 10 ms a call when
              # run alone, is held to 50 ms here, since the other checks
              # build beside it and every process start slows with them.
              hooks-plugin =
                pkgs.runCommand "worktrees-hooks-plugin-check"
                  {
                    nativeBuildInputs = [
                      pkgs.bash
                      pkgs.bats
                      pkgs.coreutils
                      pkgs.gawk
                      pkgs.git
                      pkgs.jq
                      pkgs.shellcheck
                    ];
                  }
                  ''
                    shellcheck -x -P SCRIPTDIR ${config.packages.hooks-plugin}/scripts/*.sh
                    export TMPDIR=$PWD/tmp
                    mkdir -p "$TMPDIR"
                    PLUGIN_ROOT=${config.packages.hooks-plugin} bats ${./tests/plugin}/hooks.bats
                    PLUGIN_ROOT=${config.packages.hooks-plugin} DISK_FLOOR_SH=${lib.getExe pkgs.dash} \
                      DISK_FLOOR_MAX_US=50000 bats ${./tests/plugin}/disk-floor.bats
                    touch $out
                  '';

              # The unit tests of the core and the command tests, which build
              # temporary git repositories and run `python3 -m worktrees`.
              tests =
                pkgs.runCommand "worktrees-tests"
                  {
                    nativeBuildInputs = [ pkgs.python3 ] ++ runtimeInputs;
                  }
                  ''
                    cp -r ${./.} src
                    chmod -R u+w src
                    cd src
                    export HOME=$TMPDIR/home
                    mkdir -p "$HOME"
                    python3 -m unittest discover --start-directory worktrees/tests --top-level-directory . --verbose
                    touch $out
                  '';

              # The test flake, which imports flakeModules.default for Rust and
              # Python, evaluated with this flake as its nix-worktrees input;
              # tests/dev-shell/check.sh loads its dev shell in a repository
              # and a second worktree of it.
              dev-shell =
                let
                  testInputs = {
                    self = testFlake // {
                      outPath = ./tests/dev-shell;
                      inputs = testInputs;
                    };
                    nix-worktrees = inputs.self;
                    inherit (inputs) nixpkgs;
                  };
                  testFlake = (import ./tests/dev-shell/flake.nix).outputs testInputs;
                  devshellDir = testFlake.devShells.${system}.default.passthru.config.devshell.package;
                in
                pkgs.runCommand "worktrees-dev-shell-check"
                  {
                    nativeBuildInputs = [
                      pkgs.bash
                      pkgs.git
                      pkgs.gnugrep
                    ];
                  }
                  ''
                    mkdir scratch
                    bash ${./tests/dev-shell/check.sh} ${devshellDir}/entrypoint "$PWD/scratch"
                    touch $out
                  '';

              # Home Manager configurations with every option on, evaluated
              # for Linux and macOS with and without sweep.apply; this
              # system's files are built and its loaders run.
              home-manager-module = import ./tests/home-manager {
                inherit pkgs lib;
                inherit (inputs) nixpkgs home-manager;
                module = inputs.self.homeManagerModules.default;
              };
            };

            treefmt = {
              projectRootFile = "flake.nix";
              programs = {
                nixfmt.enable = true;
                ruff-format.enable = true;
                ruff-check.enable = true;
              };
            };

            devshells.default = {
              packages = [ pkgs.python3 ] ++ runtimeInputs;

              commands = [
                {
                  category = "worktrees";
                  name = "worktrees";
                  help = "The worktrees program, from this checkout";
                  command = ''PYTHONPATH="$PRJ_ROOT''${PYTHONPATH:+:$PYTHONPATH}" exec python3 -m worktrees "$@"'';
                }
                {
                  category = "dev";
                  name = "tests";
                  help = "Run the core and command tests (python3 -m unittest)";
                  command = ''cd "$PRJ_ROOT" && exec python3 -m unittest discover --start-directory worktrees/tests --top-level-directory . "$@"'';
                }
              ];
            };
          };
      }
    );
}
