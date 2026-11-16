# checks.home-manager-module: Home Manager configurations with every option of
# `programs.worktrees` on, for Linux and macOS, each with and without
# `sweep.apply`.
#
# Every configuration is evaluated, its activation package instantiated, and
# its floor file, settings and sweep command line compared with what the
# options say; a mismatch fails evaluation with the list of them. On the
# system the check runs on, check.sh then builds the configuration's files
# and loads the direnv function and the fish hook in a Rust repository.
{
  pkgs,
  lib,
  nixpkgs,
  home-manager,
  module,
}:
let
  targets = {
    linux = "x86_64-linux";
    darwin = "aarch64-darwin";
  };

  options = apply: {
    enable = true;
    cloneRoots = [
      "~/git"
      "/srv/clones"
    ];
    direnv.enable = true;
    fish.enable = true;
    sweep = {
      enable = true;
      inherit apply;
      interval = 1800;
      cargoCapGB = 50;
    };
    floor = {
      buildGB = 80;
      cleanupOnlyGB = 40;
    };
  };

  configuration =
    system: apply:
    (home-manager.lib.homeManagerConfiguration {
      pkgs = nixpkgs.legacyPackages.${system};
      modules = [
        module
        {
          home = {
            username = "tester";
            homeDirectory =
              if nixpkgs.legacyPackages.${system}.stdenv.hostPlatform.isDarwin then
                "/Users/tester"
              else
                "/home/tester";
            stateVersion = "26.05";
          };
          programs.worktrees = options apply;
          # Home Manager's own manual page is not under test.
          manual.manpages.enable = false;
        }
      ];
    }).config;

  plain = builtins.unsafeDiscardStringContext;

  # What one configuration got wrong, as messages; none when it is right.
  problems =
    name: system: apply:
    let
      config = configuration system apply;
      exe = plain (lib.getExe config.programs.worktrees.package);
      isDarwin = nixpkgs.legacyPackages.${system}.stdenv.hostPlatform.isDarwin;
      home = config.home.homeDirectory;
      log = "${home}/.local/state/worktrees/sweep.log";
      wantCommand = "${exe} sweep --all" + lib.optionalString apply " --apply";
      agent = config.launchd.agents.worktrees-sweep.config;
      # Home Manager keeps a unit's values as lists, one per line of the unit.
      unitValues = lib.mapAttrs (_: value: map plain (lib.toList value));
      service = unitValues config.systemd.user.services.worktrees-sweep.Service;
      timer = unitValues config.systemd.user.timers.worktrees-sweep.Timer;
      command =
        if isDarwin then
          lib.concatStringsSep " " (map plain agent.ProgramArguments)
        else
          lib.concatStringsSep " " service.ExecStart;
      expect =
        what: got: want:
        lib.optional (got != want)
          "${name}, sweep.apply = ${lib.boolToString apply}: ${what} is ${builtins.toJSON got}, not ${builtins.toJSON want}";
    in
    # Forces the whole configuration, its assertions included.
    builtins.seq (plain config.home.activationPackage.drvPath) (
      expect "the floor file" config.xdg.configFile."worktrees/floor".text ''
        WORKTREES_FLOOR_BUILD_GB=80
        WORKTREES_FLOOR_CLEANUP_ONLY_GB=40
      ''
      ++ expect "the settings" (builtins.fromJSON config.xdg.configFile."worktrees/config.json".text) {
        roots = [
          "~/git"
          "/srv/clones"
        ];
        cargo_cap_gb = 50;
        sweep.apply = apply;
      }
      ++ expect "the sweep's command line" command wantCommand
      ++ expect "the direnv library" (config.xdg.configFile ? "direnv/lib/worktrees.sh") true
      ++ expect "the fish hook" (config.xdg.configFile ? "fish/conf.d/worktrees.fish") true
      ++
        expect "the package on the PATH" (lib.elem config.programs.worktrees.package config.home.packages)
          true
      ++ (
        if isDarwin then
          expect "the launchd interval" agent.StartInterval 1800
          ++ expect "the launchd log" [ agent.StandardOutPath agent.StandardErrorPath ] [ log log ]
          ++ expect "a systemd unit on macOS" (config.systemd.user.services ? worktrees-sweep) false
        else
          expect "the timer" (timer.OnStartupSec ++ timer.OnUnitActiveSec) [
            "1800s"
            "1800s"
          ]
          ++ expect "the systemd log" (service.StandardOutput ++ service.StandardError) [
            "append:${log}"
            "append:${log}"
          ]
          ++ expect "a launchd agent on Linux" (config.launchd.agents ? worktrees-sweep) false
      )
    );

  failures = lib.concatLists (
    lib.mapAttrsToList (name: system: problems name system false ++ problems name system true) targets
  );

  # The configuration for the system the check runs on, whose files are built.
  local = configuration pkgs.stdenv.hostPlatform.system true;
in
if failures != [ ] then
  throw "checks.home-manager-module:\n  ${lib.concatStringsSep "\n  " failures}"
else
  pkgs.runCommand "worktrees-home-manager-module-check"
    {
      nativeBuildInputs = [
        pkgs.bash
        pkgs.fish
        pkgs.git
      ];
    }
    ''
      bash ${./check.sh} ${local.home-files} ${./check.fish} "$TMPDIR/scratch"
      touch $out
    ''
