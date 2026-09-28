{
  description = "A simple, stateless NixOS deployment tool modeled after NixOps.";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    stable.url = "github:nixos/nixpkgs/release-26.05";
    systems.url = "github:nix-systems/triplet";
    nix-github-actions.url = "github:nix-community/nix-github-actions";
    nix-github-actions.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs =
    inputs@{
      self,
      nixpkgs,
      stable,
      systems,
      nix-github-actions,
      ...
    }:
    let
      eachSystem = nixpkgs.lib.genAttrs (import systems);
      colmenaOptions = import ./src/nix/hive/options.nix;
      colmenaModules = import ./src/nix/hive/modules.nix;
    in
    {
      packages = eachSystem (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        rec {
          colmena = pkgs.callPackage ./default.nix { };
          default = colmena;

          # Full user manual
          manual =
            let
              suppressModuleArgsDocs =
                { lib, ... }:
                {
                  options = {
                    _module.args = lib.mkOption {
                      internal = true;
                    };
                  };
                };
              colmena = self.packages.${system}.colmena;
              deploymentOptionsMd =
                (pkgs.nixosOptionsDoc {
                  inherit
                    (pkgs.lib.evalModules {
                      modules = [
                        colmenaOptions.deploymentOptions
                        suppressModuleArgsDocs
                      ];
                      specialArgs = {
                        name = "nixos";
                        nodes = { };
                      };
                    })
                    options
                    ;
                }).optionsCommonMark;
              metaOptionsMd =
                (pkgs.nixosOptionsDoc {
                  inherit
                    (pkgs.lib.evalModules {
                      modules = [
                        colmenaOptions.metaOptions
                        suppressModuleArgsDocs
                      ];
                    })
                    options
                    ;
                }).optionsCommonMark;
            in
            pkgs.callPackage ./manual {
              inherit colmena deploymentOptionsMd metaOptionsMd;
            };

          # User manual without the CLI reference
          manualFast = manual.override { colmena = null; };

          # User manual with the version treated as stable
          manualForceStable = manual.override { unstable = false; };
        }
      );

      devShells = eachSystem (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            inputsFrom = [
              self.packages.${system}.colmena
              self.packages.${system}.manualFast
            ];

            packages = with pkgs; [
              bashInteractive
              cargo-audit
              cargo-nextest
              cargo-outdated
              clippy
              editorconfig-checker
              nixVersions.git
              nixfmt
              rust-analyzer
              rustfmt
            ];

            env = {
              NIX_PATH = "nixpkgs=${pkgs.path}";
              RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
            };
          };
        }
      );

      checks = eachSystem (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          overlays = [
            self.overlays.default
            (_: _: { inherit inputs; })
          ];
        in
        if pkgs.stdenv.hostPlatform.isLinux then
          import ./integration-tests {
            pkgs = import nixpkgs { inherit system overlays; };
            pkgsStable = import stable { inherit system overlays; };
          }
        else
          { }
      );

      formatter = eachSystem (system: nixpkgs.legacyPackages.${system}.callPackage ./formatter.nix { });

      overlays.default = final: _: {
        colmena = final.callPackage ./default.nix { };
      };

      nixosModules = {
        inherit (colmenaOptions) deploymentOptions metaOptions;
        inherit (colmenaModules) keyChownModule keyServiceModule assertionModule;
      };

      lib.makeHive =
        rawHive:
        import ./src/nix/hive/eval.nix {
          inherit rawHive colmenaOptions colmenaModules;
          hermetic = true;
        };

      githubActions = nix-github-actions.lib.mkGithubMatrix {
        checks = {
          inherit (self.checks) x86_64-linux;
        };
      };
    };

  nixConfig = {
    extra-substituters = [ "https://colmena.cachix.org" ];
    extra-trusted-public-keys = [ "colmena.cachix.org-1:7BzpDnjjH8ki2CT3f6GdOk7QAzPOl+1t3LvTLXqYcSg=" ];
  };
}
