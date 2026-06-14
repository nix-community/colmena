# Adapted from the NixOps test in Nixpkgs.
#
# By default, we have four nodes: deployer, alpha, beta, gamma.
# deployer is where colmena will run.
#
# TODO: Modularize most of this

{
  insideVm ? false,
  deployers ? [ "deployer" ], # Nodes configured as deployers (with Colmena and pre-built system closure)
  targets ? [
    "alpha"
    "beta"
    "gamma"
  ], # Nodes configured as targets (minimal config)
  extraDeployerConfig ? { }, # Extra config on the deployer
  prebuiltTarget ? "alpha", # Target node to prebuild system closure for, or null

  pkgs ? if insideVm then import <nixpkgs> { } else throw "Must specify pkgs",
  colmena ? if !insideVm then pkgs.colmena else throw "Cannot eval inside VM",
}:

with builtins;

assert elem "deployer" deployers;

let
  inherit (pkgs) lib;

  colmenaExec = "${colmena}/bin/colmena";

  ## Utilities
  sshKeys = import (pkgs.path + "/nixos/tests/ssh-keys.nix") pkgs;
  nixosLib = import (pkgs.path + "/nixos/lib") { };

  inputClosureOf =
    pkg:
    pkgs.runCommand "full-closure"
      {
        refs = pkgs.writeClosure [ pkg.drvPath ];
      }
      ''
        touch $out

        while read ref; do
          case $ref in
            *.drv)
              cat $ref >>$out
              ;;
          esac
        done <$refs
      '';

  ## The modular NixOS test framework with Colmena additions
  colmenaTestModule =
    { lib, config, ... }:
    let
      cfg = config.colmena.test;

      targetList = "[${concatStringsSep ", " targets}]";
      bundle = pkgs.stdenv.mkDerivation {
        name = "${config.name}-bundle";
        dontUnpack = true;
        dontInstall = true;
        buildPhase = ''
          cp -r ${cfg.bundle} $out
          chmod u+w $out
          cp ${./tools.nix} $out/tools.nix
        '';
      };
    in
    {
      options = {
        colmena.test = {
          bundle = lib.mkOption {
            description = ''
              Path to a directory to copy into the deployer as /tmp/bundle.
            '';
            type = lib.types.path;
          };

          testScript = lib.mkOption {
            description = ''
              The test script.

              The Colmena test framework will prepend initialization
              statements to the actual test script.
            '';
            type = lib.types.str;
          };
        };
      };
      config = {
        testScript =
          ''
            start_all()
          ''
          + lib.optionalString (prebuiltTarget != null) ''
            deployer.succeed("nix-store -qR ${prebuiltSystem}")
          ''
          + ''
            deployer.succeed("nix-store -qR ${pkgs.path}")
            deployer.succeed("ln -sf ${pkgs.path} /nixpkgs")
            deployer.succeed("mkdir -p /root/.ssh && touch /root/.ssh/id_rsa && chmod 600 /root/.ssh/id_rsa && cat ${sshKeys.snakeOilPrivateKey} > /root/.ssh/id_rsa")

            ${lib.optionalString (length targets != 0) ''
              for node in ${targetList}:
                  node.wait_for_unit("sshd.service")
                  deployer.wait_until_succeeds(f"ssh -o StrictHostKeyChecking=accept-new {node.name} true", timeout=30)
            ''}

            deployer.succeed("cp --no-preserve=mode -r ${bundle} /tmp/bundle && chmod u+w /tmp/bundle")

            orig_store_paths = set(deployer.succeed("ls /nix/store").strip().split("\n"))
            def get_new_store_paths():
                cur_store_paths = set(deployer.succeed("ls /nix/store").strip().split("\n"))
                new_store_paths = cur_store_paths.difference(orig_store_paths)
                deployer.log(f"{len(new_store_paths)} store paths were created")

                l = list(map(lambda n: f"/nix/store/{n}", new_store_paths))
                return l

            ${cfg.testScript}
          '';
      };
    };
  evalTest =
    module:
    nixosLib.evalTest {
      imports = [
        module
        colmenaTestModule
        { hostPkgs = pkgs; }
      ];
    };

  ## Common setup

  commonConfig =
    { pkgs, ... }:
    {
      # Temporary workaround for <https://github.com/zhaofengli/colmena/issues/281>
      # References:
      # - LKML discussion: <https://lore.kernel.org/all/w5ap2zcsatkx4dmakrkjmaexwh3mnmgc5vhavb2miaj6grrzat@7kzr5vlsrmh5/>
      # - Lix discussion: <https://matrix.to/#/!lymvtcwDJ7ZA9Npq:lix.systems/$wLqRlm7-iNmrkN2Tcn--Tmi92id4wgvKC5APwiEYYgw?via=lix.systems&via=matrix.org>
      # - Proposed fix: <https://lkml.org/lkml/2024/10/21/1621>
      boot.kernelPackages = pkgs.linuxPackages_6_6;

      nix.settings.substituters = lib.mkForce [ ];

      # Re-enable switch-to-configuration
      system.switch.enable = true;
    };

  # Setup for deployer nodes
  #
  # We include the input closure of a prebuilt system profile
  # so it can build system profiles for the targets without
  # network access.
  deployerConfig =
    {
      pkgs,
      lib,
      config,
      ...
    }:
    {
      imports = [
        commonConfig
        extraDeployerConfig
      ];

      nix.registry = lib.mkIf (pkgs ? _inputs) {
        nixpkgs.flake = pkgs._inputs.nixpkgs;
      };

      nix.nixPath = [
        "nixpkgs=${pkgs.path}"
      ];

      virtualisation = {
        memorySize = 6144;
        writableStore = true;
        additionalPaths =
          [
            "${pkgs.path}"
          ]
          ++ lib.optionals (prebuiltTarget != null) [
            prebuiltSystem
            (inputClosureOf prebuiltSystem)
          ];
      };

      services.openssh.enable = true;
      users.users.root.openssh.authorizedKeys.keys = [
        sshKeys.snakeOilPublicKey
      ];

      environment.systemPackages = with pkgs; [
        git # for git flake tests
        inotify-tools # for key services build

        # HACK: copy stderr to both stdout and stderr
        # (the test framework only captures stdout, and only stderr appears on screen during the build)
        (writeShellScriptBin "run-copy-stderr" ''
          exec "$@" 2> >(tee /dev/stderr)
        '')
      ];
    };

  # Setup for target nodes
  #
  # Kept as minimal as possible.
  targetConfig =
    { lib, ... }:
    {
      imports = [
        commonConfig
      ];

      documentation.nixos.enable = lib.mkOverride 60 true;

      services.openssh.enable = true;
      users.users.root.openssh.authorizedKeys.keys = [
        sshKeys.snakeOilPublicKey
      ];
      virtualisation.writableStore = true;
    };

  nodes =
    let
      deployerNodes = map (name: lib.nameValuePair name deployerConfig) deployers;
      targetNodes = map (name: lib.nameValuePair name targetConfig) targets;
    in
    listToAttrs (deployerNodes ++ targetNodes);

  # A "shallow" re-evaluation of the test for use from Colmena
  standaloneTest = evalTest (
    { ... }:
    {
      inherit nodes;
    }
  );

  prebuiltSystem = standaloneTest.config.nodes.${prebuiltTarget}.system.build.toplevel;

  getStandaloneConfigFor =
    node:
    { lib, config, ... }:
    {
      imports = [
        (pkgs.path + "/nixos/lib/testing/nixos-test-base.nix")
        (pkgs.path + "/nixos/modules/virtualisation/qemu-vm.nix")
        (if elem node deployers then deployerConfig else targetConfig)
        standaloneTest.config.nodes.${node}.system.build.networkConfig
      ];

      documentation.nixos.enable = lib.mkOverride 55 false;
      boot.loader.grub.enable = false;
      system.nixos.revision = lib.mkForce "constant-nixos-revision";

      nix.nixPath = lib.mkForce [ "nixpkgs=/nixpkgs" ];

      deployment.tags = lib.optional (config.networking.hostName != "deployer") "target";
    };
in
{
  inherit
    pkgs
    nodes
    colmena
    colmenaExec
    getStandaloneConfigFor
    inputClosureOf
    ;

  runTest =
    module:
    (evalTest (
      { config, ... }:
      {
        imports = [
          module
          { inherit nodes; }
        ];
        result = config.test;
      }
    )).config.result;
}
