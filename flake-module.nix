# flakeModules.default: a numtide devshell gets the `worktrees` command and,
# when it loads, the shared build-cache variables `worktrees env` prints for
# the worktree it loads in.
#
# The module holds no logic of its own: the build root, each language's
# variables, leaving a variable already set alone and provisioning a new
# worktree are all the program's. It only runs `worktrees env` and evals
# what it prints, so a login shell, direnv and an agent session with no dev
# shell get the same values. Its disk floor warning is the plugin's own
# hook script, run with `--warn`.
#
# Called through `importApply` by this repository's flake.nix, so the
# default package is this flake's own `worktrees`.
{ withSystem }:
{ lib, flake-parts-lib, ... }:
let
  inherit (lib) mkOption types;
in
{
  options.perSystem = flake-parts-lib.mkPerSystemOption (
    { system, ... }:
    {
      options.worktrees = {
        languages = mkOption {
          type = types.nullOr (types.listOf types.str);
          default = null;
          example = [
            "rust"
            "python"
          ];
          description = ''
            The languages whose shared build-cache variables the dev shell
            sets, passed to `worktrees env --languages`; the program's README
            lists them and their aliases. `null` lets the program detect them
            from the files at the worktree's root.
          '';
        };

        package = mkOption {
          type = types.package;
          default = withSystem system ({ config, ... }: config.packages.worktrees);
          defaultText = lib.literalMD "nix-worktrees' own `packages.<system>.worktrees`";
          description = "The `worktrees` program the dev shell runs and offers as a command.";
        };

        diskFloorWarning = mkOption {
          type = types.bool;
          default = true;
          description = ''
            Whether the dev shell, when it loads, warns on standard error
            that the volume of its root is below a disk floor, with the
            check the plugin's disk floor hook makes before every agent's
            Bash command. It warns and never fails, since a shell that won't
            start can't be used to clean up.
          '';
        };

        hooksPlugin = mkOption {
          type = types.package;
          default = withSystem system ({ config, ... }: config.packages.hooks-plugin);
          defaultText = lib.literalMD "nix-worktrees' own `packages.<system>.hooks-plugin`";
          description = "The Claude Code plugin whose `scripts/disk-floor.sh --warn` makes the dev shell's disk floor check.";
        };

        devshell = mkOption {
          type = types.str;
          default = "default";
          description = "The name, under `devshells`, of the dev shell to set up.";
        };
      };
    }
  );

  config.perSystem =
    { config, pkgs, ... }:
    let
      cfg = config.worktrees;
      exe = lib.getExe cfg.package;
      languages = lib.optionalString (cfg.languages != null) (
        " --languages " + lib.escapeShellArg (lib.concatStringsSep "," cfg.languages)
      );
    in
    {
      devshells.${cfg.devshell} = {
        commands = [
          {
            category = "worktrees";
            name = "worktrees";
            help = "A worktree's shared build caches, provisioning, cleanup and removal";
            package = cfg.package;
          }
        ];

        # Outside git `env` prints nothing; when it fails, its reason is on
        # standard error and the dev shell loads without the variables.
        devshell.startup.worktrees-env = lib.noDepEntry ''
          if __worktrees_env=$(cd "$PRJ_ROOT" && ${exe} env${languages}); then
            eval "$__worktrees_env"
          fi
          unset __worktrees_env
        '';

        # The disk floors as a warning; the script exits 0 whatever it finds.
        devshell.startup.worktrees-disk-floor = lib.mkIf cfg.diskFloorWarning (
          lib.noDepEntry ''
            ${lib.getExe pkgs.dash} ${cfg.hooksPlugin}/scripts/disk-floor.sh --warn "$PRJ_ROOT" || :
          ''
        );
      };
    };
}
