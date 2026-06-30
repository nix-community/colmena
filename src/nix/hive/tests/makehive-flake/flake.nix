{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    colmena.url = "git+file://@repoPath@";
  };
  outputs = { nixpkgs, colmena, ... }: {
    colmenaHive = colmena.lib.makeHive {
      meta = {
        nixpkgs = import nixpkgs {
          system = "x86_64-linux";
        };
      };

      host-a =
        {
          name,
          nodes,
          pkgs,
          ...
        }:
        {
          boot.isContainer = true;
          time.timeZone = nodes.host-b.config.time.timeZone;
        };
      host-b = {
        deployment = {
          targetHost = "somehost.tld";
          targetPort = 1234;
          targetUser = "luser";
        };
        boot.isContainer = true;
        time.timeZone = "America/Los_Angeles";
      };
    };
  };
}
