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
        name = "git-worktree-manager-dev";
      };

      perSystem =
        {
          pkgs,
          lib,
          config,
          ...
        }:
        let
          # What the binary runs: git, and ps (adv_cmds on macOS, procps
          # elsewhere; a Nix builder can't run macOS's setuid /bin/ps). The
          # package isn't wrapped: the hook's shim starts it once per
          # committed transaction, and a wrapper would add one more process
          # start to each. It finds both on the PATH.
          runtimeInputs = [
            pkgs.git
            pkgs.ps
          ];

          # The crate. crane reads its name and version from Cargo.toml, and
          # builds its dependencies once for every check below. The source
          # is the cargo files, the JSON fixtures under src/ and every
          # fixture under tests/fixtures/.
          craneLib = inputs.crane.mkLib pkgs;
          crate = craneLib.crateNameFromCargoToml { cargoToml = ./Cargo.toml; };
          crateArgs = {
            src = lib.cleanSourceWith {
              src = ./.;
              filter =
                path: type:
                craneLib.filterCargoSources path type
                || (lib.hasInfix "/src/" path && lib.hasSuffix ".json" path)
                || lib.hasInfix "/tests/fixtures/" path;
            };
            strictDeps = true;
          };
          cargoArtifacts = craneLib.buildDepsOnly crateArgs;
        in
        {
          packages = {
            # The binary, under the name in Cargo.toml. Its tests run in the
            # `crate-tests` check.
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
            default = config.packages.${crate.pname};
          };

          checks = {
            package = config.packages.default;

            # The package runs: `git-worktree-manager --version` prints the
            # version in Cargo.toml.
            package-runs = pkgs.testers.testVersion {
              package = config.packages.default;
              inherit (crate) version;
            };

            # The tests that run git and ps do so in temporary repositories.
            crate-tests = craneLib.cargoTest (
              crateArgs
              // {
                inherit cargoArtifacts;
                nativeCheckInputs = [ pkgs.fish ] ++ runtimeInputs;
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
              rustfmt.enable = true;
            };
          };

          # fish runs the `env --shell fish` test, which cargo test skips
          # without it.
          devshells.default = {
            packages = [
              pkgs.cargo
              pkgs.clippy
              pkgs.fish
              pkgs.rustc
              pkgs.rustfmt
            ]
            ++ runtimeInputs;
          };
        };
    };
}
