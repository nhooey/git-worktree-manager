{
  description = "A dev shell that imports nix-worktrees' flakeModules.default for Rust and Python, as a repository would";

  # Everything else comes from nix-worktrees' own inputs, so this flake
  # locks nothing of its own. The parent flake's `checks.dev-shell`
  # evaluates this file directly, with its own `self` and `nixpkgs` as these
  # inputs.
  inputs = {
    nix-worktrees.url = "path:../..";
    nixpkgs.follows = "nix-worktrees/nixpkgs";
  };

  outputs =
    inputs@{ nix-worktrees, ... }:
    nix-worktrees.inputs.flake-parts.lib.mkFlake { inherit inputs; } {
      systems = import nix-worktrees.inputs.systems;

      imports = [
        # A repository's dev shell already has numtide devshell; nix-worktrees
        # reaches it through agent-skill-flake, so this test does too.
        nix-worktrees.inputs.agent-skill-flake.inputs.devshell.flakeModule
        nix-worktrees.flakeModules.default
      ];

      perSystem = {
        worktrees.languages = [
          "rust"
          "python"
        ];
        devshells.default = { };
      };
    };
}
