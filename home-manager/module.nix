# homeManagerModules.default: `programs.worktrees`. The `worktrees` program
# on the PATH, a worktree's environment loaded on entering it through direnv
# or fish, an hourly sweep of finished worktrees on launchd or systemd, and
# the disk floors agents' builds stop at.
#
# The module holds no logic of its own: every decision is the program's. It
# writes the settings the program, the plugin's hooks and the headless
# runner read, and schedules `worktrees sweep --all`.
#
# Called through `importApply` by this repository's flake.nix, so the
# default package is this flake's own `worktrees`.
{ withSystem }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  inherit (lib)
    mkEnableOption
    mkIf
    mkMerge
    mkOption
    types
    ;

  cfg = config.programs.worktrees;
  exe = lib.getExe cfg.package;
  inherit (pkgs.stdenv.hostPlatform) isDarwin isLinux;

  stateDir = "${config.xdg.stateHome}/worktrees";
  sweepLog = "${stateDir}/sweep.log";

  sweepArgs = [
    exe
    "sweep"
    "--all"
  ]
  ++ lib.optional cfg.sweep.apply "--apply";

  # A scheduled job starts with a bare environment: the user's profile for
  # ssh and the dev shell cache tool the sweep calls, then the system's. The
  # XDG folders are the ones this module writes the settings into.
  sweepEnvironment = {
    PATH = lib.concatStringsSep ":" [
      "${config.home.profileDirectory}/bin"
      "/run/current-system/sw/bin"
      "/nix/var/nix/profiles/default/bin"
      "/usr/bin"
      "/bin"
      "/usr/sbin"
      "/sbin"
    ];
    XDG_CONFIG_HOME = config.xdg.configHome;
    XDG_STATE_HOME = config.xdg.stateHome;
  };

  # Merged by the program over its shipped defaults.json, so only the
  # settings this module owns are named.
  settings = {
    roots = cfg.cloneRoots;
    cargo_cap_gb = cfg.sweep.cargoCapGB;
    sweep.apply = cfg.sweep.apply;
  };

  # Read by the program, by the plugin's disk floor hook (a POSIX shell
  # script) and by the headless runner: whole numbers, as `KEY=value` lines
  # whose keys are the variables that override them.
  floorFile = ''
    WORKTREES_FLOOR_BUILD_GB=${toString cfg.floor.buildGB}
    WORKTREES_FLOOR_CLEANUP_ONLY_GB=${toString cfg.floor.cleanupOnlyGB}
  '';

  gigabytes = types.ints.positive;
in
{
  options.programs.worktrees = {
    enable = mkEnableOption "the `worktrees` program on the `PATH`, with its settings and disk floors";

    package = mkOption {
      type = types.package;
      default = withSystem pkgs.stdenv.hostPlatform.system ({ config, ... }: config.packages.worktrees);
      defaultText = lib.literalMD "nix-worktrees' own `packages.<system>.worktrees`";
      description = "The `worktrees` program the module installs and its hooks and sweep run.";
    };

    cloneRoots = mkOption {
      type = types.listOf types.str;
      example = [ "~/git/github/nhooey" ];
      description = ''
        The folders `worktrees status --all` and `worktrees sweep --all`
        look for clones under, written to `~/.config/worktrees/config.json`
        as `roots`. A leading `~` is the home folder. There is no default:
        where the clones are is a fact about the machine.
      '';
    };

    direnv.enable = mkEnableOption ''
      the direnv library function `use_worktrees`, in
      `~/.config/direnv/lib/worktrees.sh`, which evals what `worktrees env`
      prints; an `.envrc` calls it with `use worktrees`
    '';

    fish.enable = mkEnableOption ''
      a fish function on `PWD`, in `~/.config/fish/conf.d/worktrees.fish`,
      that sources `worktrees env --shell fish` in interactive shells when
      the directory's git root changes
    '';

    sweep = {
      enable = mkEnableOption ''
        `worktrees sweep --all` every `sweep.interval`, as a launchd agent on
        macOS and a systemd user timer on Linux, its report appended to
        `~/.local/state/worktrees/sweep.log`
      '';

      apply = mkOption {
        type = types.bool;
        default = false;
        description = ''
          Whether the scheduled sweep passes `--apply`, and so removes what
          it finds finished rather than only listing it. Also written to the
          settings as `sweep.apply`, where the plugin's SessionStart sweep
          reads it.
        '';
      };

      interval = mkOption {
        type = types.ints.positive;
        default = 3600;
        example = 1800;
        description = "Seconds between scheduled sweeps; an hour by default.";
      };

      cargoCapGB = mkOption {
        type = gigabytes;
        default = 60;
        description = ''
          The size in GB past which the sweep deletes a repository's shared
          Cargo build directory, written to the settings as `cargo_cap_gb`.
        '';
      };
    };

    floor = {
      buildGB = mkOption {
        type = gigabytes;
        default = 70;
        description = ''
          The free space in GB below which agents' builds are refused,
          written to `~/.config/worktrees/floor` as
          `WORKTREES_FLOOR_BUILD_GB`. 70 stays about 20 GB above Nix's
          50 GiB `min-free`, below which every build collects garbage.
        '';
      };

      cleanupOnlyGB = mkOption {
        type = gigabytes;
        default = 35;
        description = ''
          The free space in GB below which agents may only read and clean
          up, written to `~/.config/worktrees/floor` as
          `WORKTREES_FLOOR_CLEANUP_ONLY_GB`.
        '';
      };
    };
  };

  config = mkIf cfg.enable (mkMerge [
    {
      assertions = [
        {
          assertion = cfg.floor.cleanupOnlyGB <= cfg.floor.buildGB;
          message = "programs.worktrees.floor.cleanupOnlyGB (${toString cfg.floor.cleanupOnlyGB}) must not be above floor.buildGB (${toString cfg.floor.buildGB}).";
        }
      ];

      home.packages = [ cfg.package ];

      xdg.configFile = {
        "worktrees/config.json".text = builtins.toJSON settings + "\n";
        "worktrees/floor".text = floorFile;
      };
    }

    (mkIf cfg.direnv.enable {
      xdg.configFile."direnv/lib/worktrees.sh".source = pkgs.replaceVars ./direnv-lib.sh.in {
        worktrees = exe;
      };
    })

    (mkIf cfg.fish.enable {
      xdg.configFile."fish/conf.d/worktrees.fish".source = pkgs.replaceVars ./worktrees.fish.in {
        worktrees = exe;
      };
    })

    (mkIf cfg.sweep.enable {
      # Neither launchd nor systemd creates the log's folder.
      home.activation.worktreesStateDir = lib.hm.dag.entryAfter [ "writeBoundary" ] ''
        run mkdir -p $VERBOSE_ARG ${lib.escapeShellArg stateDir}
      '';
    })

    (mkIf (cfg.sweep.enable && isDarwin) {
      launchd.agents.worktrees-sweep = {
        enable = true;
        config = {
          Label = "io.github.nhooey.worktrees.sweep";
          ProgramArguments = sweepArgs;
          StartInterval = cfg.sweep.interval;
          StandardOutPath = sweepLog;
          StandardErrorPath = sweepLog;
          EnvironmentVariables = sweepEnvironment;
          ProcessType = "Background";
          LowPriorityIO = true;
        };
      };
    })

    (mkIf (cfg.sweep.enable && isLinux) {
      systemd.user.services.worktrees-sweep = {
        Unit.Description = "Sweep finished git worktrees (worktrees sweep --all)";
        Service = {
          Type = "oneshot";
          ExecStart = lib.concatStringsSep " " sweepArgs;
          Environment = lib.mapAttrsToList (name: value: "${name}=${value}") sweepEnvironment;
          StandardOutput = "append:${sweepLog}";
          StandardError = "append:${sweepLog}";
          Nice = 10;
          IOSchedulingClass = "idle";
        };
      };

      systemd.user.timers.worktrees-sweep = {
        Unit.Description = "Sweep finished git worktrees every ${toString cfg.sweep.interval} seconds";
        Timer = {
          OnStartupSec = "${toString cfg.sweep.interval}s";
          OnUnitActiveSec = "${toString cfg.sweep.interval}s";
        };
        Install.WantedBy = [ "timers.target" ];
      };
    })
  ]);
}
