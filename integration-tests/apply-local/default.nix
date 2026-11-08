{ pkgs }:

let
  tools = pkgs.callPackage ../tools.nix {
    targets = [ ];
    prebuiltTarget = "deployer";
    extraDeployerConfig = {
      users.users.colmena = {
        isNormalUser = true;
        extraGroups = [ "wheel" ];
      };
      security.sudo.wheelNeedsPassword = false;
    };
  };
in
tools.runTest {
  name = "colmena-apply-local";

  colmena.test = {
    bundle = ./.;

    testScript = ''
      deployer.succeed("cd /tmp/bundle && sudo -u colmena ${tools.colmenaExec} apply-local --sudo")
      deployer.succeed("grep SUCCESS /etc/deployment")
      deployer.succeed("grep SECRET /run/keys/key-text")

      # A failing key command must keep the existing key. The first apply
      # removed the colmena user, so we run as root.
      deployer.succeed("sed -i 's/text = \"SECRET\"/keyCommand = [ \"false\" ]/' /tmp/bundle/hive.nix")
      deployer.fail("cd /tmp/bundle && ${tools.colmenaExec} apply-local")
      deployer.succeed("grep SECRET /run/keys/key-text")
    '';
  };
}
