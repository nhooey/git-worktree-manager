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
            };

            checks = {
              package = config.packages.worktrees;

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
