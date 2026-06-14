{
  pkgs,
  evaluator ? "chunked",
  extraApplyFlags ? "",
  pure ? true,
}:

let
  inherit (pkgs) lib;

  tools = pkgs.callPackage ../tools.nix {
    targets = [ "alpha" ];
  };

  applyFlags = "--evaluator ${evaluator} ${extraApplyFlags}" + lib.optionalString (!pure) "--impure";

  # From integration-tests/nixpkgs.nix
  colmenaFlakeInputs = pkgs._inputs;
in
tools.runTest {
  name = "colmena-flakes-${evaluator}" + lib.optionalString (!pure) "-impure";

  nodes.deployer = {
    virtualisation.additionalPaths = lib.mapAttrsToList (k: v: v.outPath) colmenaFlakeInputs;
  };

  colmena.test = {
    bundle = ./.;

    testScript =
      ''
        import re

        deployer.succeed("sed -i 's @nixpkgs@ path:${pkgs._inputs.nixpkgs.outPath}?narHash=${pkgs._inputs.nixpkgs.narHash} g' /tmp/bundle/flake.nix")
        deployer.succeed("sed -i 's @colmena@ path:${tools.colmena.src} g' /tmp/bundle/flake.nix")

        with subtest("Lock flake dependencies"):
            deployer.succeed("cd /tmp/bundle && nix --extra-experimental-features \"nix-command flakes\" flake lock")

        with subtest("Deploy with a plain flake without git"):
            deployer.succeed("cd /tmp/bundle && ${tools.colmenaExec} apply --on @target ${applyFlags}")
            alpha.succeed("grep FIRST /etc/deployment")

        with subtest("Deploy with a git flake"):
            deployer.succeed("sed -i s/FIRST/SECOND/g /tmp/bundle/probe.nix")

            # don't put probe.nix in source control - should fail
            deployer.succeed("cd /tmp/bundle && git init && git add flake.nix flake.lock hive.nix tools.nix")
            logs = deployer.fail("cd /tmp/bundle && run-copy-stderr ${tools.colmenaExec} apply --on @target ${applyFlags}")
            assert re.search(r"probe.nix.*(No such file or directory|does not exist|not tracked by Git)", logs), "Expected error message not found in log"

            # now it should succeed
            deployer.succeed("cd /tmp/bundle && git add probe.nix")
            deployer.succeed("cd /tmp/bundle && ${tools.colmenaExec} apply --on @target ${applyFlags}")
            alpha.succeed("grep SECOND /etc/deployment")

      ''
      + lib.optionalString pure ''
        with subtest("Check that impure expressions are forbidden"):
            deployer.succeed("sed -i 's|SECOND|''${builtins.readFile /etc/hostname}|g' /tmp/bundle/probe.nix")
            logs = deployer.fail("cd /tmp/bundle && run-copy-stderr ${tools.colmenaExec} apply --on @target ${applyFlags}")
            assert re.search(r"access to absolute path.*forbidden in pure (eval|evaluation) mode", logs), "Expected error message not found in log"

        with subtest("Check that impure expressions can be allowed with --impure"):
            deployer.succeed("cd /tmp/bundle && ${tools.colmenaExec} apply --on @target ${applyFlags} --impure")
            alpha.succeed("grep deployer /etc/deployment")
      '';
  };
}
