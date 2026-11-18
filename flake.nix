{
  description = "Remove git worktrees and branches once their work lands, archive what would be lost, and keep worktrees' build output outside them and shared";

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

    crane.url = "github:ipetkov/crane";

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
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = import systems;

      imports = [
        inputs.agent-skill-flake.flakeModules.devshellSkills
        inputs.treefmt-nix.flakeModule
      ];

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
          ...
        }:
        let
          # What the program runs besides Python: git, ps (adv_cmds on macOS,
          # procps elsewhere; a Nix builder can't run macOS's setuid /bin/ps),
          # du from coreutils, and awk and date for the hooks.
          runtimeInputs = [
            pkgs.git
            pkgs.ps
            pkgs.coreutils
            pkgs.gawk
          ];

          # The one version, `__version__` in worktrees/__init__.py.
          version = lib.pipe ./worktrees/__init__.py [
            builtins.readFile
            (lib.splitString "\n")
            (lib.findFirst (lib.hasPrefix "__version__ = ") "")
            (lib.removePrefix "__version__ = \"")
            (lib.removeSuffix "\"")
          ];

          source = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./bin
              ./worktrees
            ];
          };

          # The Rust crate. crane reads its name and version from Cargo.toml,
          # and builds its dependencies once for every check below. The
          # source is the cargo files and the shipped settings, which the
          # binary embeds.
          craneLib = inputs.crane.mkLib pkgs;
          crate = craneLib.crateNameFromCargoToml { cargoToml = ./Cargo.toml; };
          crateArgs = {
            src = lib.cleanSourceWith {
              src = ./.;
              filter =
                path: type:
                craneLib.filterCargoSources path type
                || lib.hasSuffix "/worktrees/defaults.json" path
                || (type == "directory" && lib.hasSuffix "/worktrees" path);
            };
            strictDeps = true;
          };
          cargoArtifacts = craneLib.buildDepsOnly crateArgs;
        in
        {
          packages = {
            # `bin/worktrees`, the sh entry point, with the package's paths
            # written into its `packaged_*` lines. The hooks' functions run in
            # sh before Python is reached, and the system's tools come first on
            # the PATH, so the user's git, ps and awk win where they exist.
            worktrees = pkgs.stdenvNoCC.mkDerivation {
              pname = "worktrees";
              inherit version;
              src = source;
              dontConfigure = true;
              dontBuild = true;
              installPhase = ''
                runHook preInstall
                mkdir -p $out/bin $out/lib/worktrees
                cp -r worktrees $out/lib/worktrees/
                rm -rf $out/lib/worktrees/worktrees/tests
                substitute bin/worktrees $out/bin/worktrees \
                  --replace-fail 'packaged_root=' 'packaged_root=${placeholder "out"}/lib/worktrees' \
                  --replace-fail 'packaged_python=' 'packaged_python=${lib.getExe pkgs.python3}' \
                  --replace-fail 'packaged_path=' 'packaged_path=${lib.makeBinPath runtimeInputs}'
                chmod +x $out/bin/worktrees
                runHook postInstall
              '';
              meta = {
                description = "Remove git worktrees and branches once their work lands, and lose nothing";
                license = lib.licenses.asl20;
                mainProgram = "worktrees";
              };
            };
            default = config.packages.worktrees;

            # The Rust port, under the name in Cargo.toml. Its tests run in
            # the `crate-tests` check.
            ${crate.pname} = craneLib.buildPackage (
              crateArgs
              // {
                inherit cargoArtifacts;
                doCheck = false;
                meta = {
                  description = "Remove git worktrees and branches once their work lands, and lose nothing";
                  license = lib.licenses.asl20;
                  mainProgram = crate.pname;
                };
              }
            );
          };

          checks = {
            package = config.packages.worktrees;

            # The package runs: `--help`, and a dry run in a fresh repository.
            package-runs = pkgs.runCommand "worktrees-package-runs" {
              nativeBuildInputs = [ pkgs.git ];
              worktrees = lib.getExe config.packages.worktrees;
            } (builtins.readFile ./checks/package-runs.sh);

            # The unit tests of the core and the rules, the command tests over
            # temporary git repositories, the hooks' tests and the 1,000-ref
            # fetch timing test.
            tests = pkgs.runCommand "worktrees-tests" {
              nativeBuildInputs = [ pkgs.python3 ] ++ runtimeInputs;
              sourceTree = ./.;
            } (builtins.readFile ./checks/tests.sh);

            # The Rust package runs: `--version`, `--help`, and `env` and a
            # dry-run `clean` in a fresh repository.
            crate-runs = pkgs.runCommand "${crate.pname}-runs" {
              nativeBuildInputs = [ pkgs.git ];
              program = lib.getExe config.packages.${crate.pname};
            } (builtins.readFile ./checks/crate-runs.sh);

            # The command tests run the binary over temporary git repositories.
            crate-tests = craneLib.cargoTest (
              crateArgs
              // {
                inherit cargoArtifacts;
                nativeCheckInputs = [ pkgs.git ];
              }
            );

            crate-clippy = craneLib.cargoClippy (
              crateArgs
              // {
                inherit cargoArtifacts;
                cargoClippyExtraArgs = "--all-targets -- --deny warnings";
              }
            );
          };

          treefmt = {
            projectRootFile = "flake.nix";
            programs = {
              nixfmt.enable = true;
              ruff-format.enable = true;
              ruff-check.enable = true;
              rustfmt.enable = true;
              shellcheck.enable = true;
            };
            settings.formatter.shellcheck.includes = [
              "bin/worktrees"
              "checks/*.sh"
            ];
          };

          devshells.default = {
            packages = [
              pkgs.python3
              pkgs.cargo
              pkgs.rustc
              pkgs.clippy
              pkgs.rustfmt
            ]
            ++ runtimeInputs;

            commands = [
              {
                category = "worktrees";
                name = "worktrees";
                help = "The worktrees program, from this checkout";
                command = ''exec "$PRJ_ROOT/bin/worktrees" "$@"'';
              }
              {
                category = "dev";
                name = "tests";
                help = "Run the Python tests (python3 -m unittest); cargo test runs the crate's";
                command = ''cd "$PRJ_ROOT" && exec python3 -m unittest discover --start-directory worktrees/tests --top-level-directory . "$@"'';
              }
            ];
          };
        };
    };
}
