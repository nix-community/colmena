{
  description = "A simple, stateless NixOS deployment tool modeled after NixOps.";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    stable.url = "github:NixOS/nixpkgs/release-26.05";

    nix-github-actions = {
      url = "github:nix-community/nix-github-actions";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flake-utils.url = "github:numtide/flake-utils";

    flake-compat = {
      url = "github:edolstra/flake-compat";
      flake = false;
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      stable,
      flake-utils,
      nix-github-actions,
      ...
    }@inputs:
    let
      supportedSystems = [
        "x86_64-linux"
        "i686-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      colmenaOptions = import ./src/nix/hive/options.nix;
      colmenaModules = import ./src/nix/hive/modules.nix;
    in
    flake-utils.lib.eachSystem supportedSystems (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
      in
      rec {
        # We still maintain the expression in a Nixpkgs-acceptable form
        defaultPackage = self.packages.${system}.colmena;
        packages = rec {
          colmena = pkgs.callPackage ./package.nix { };

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
        };

        defaultApp = self.apps.${system}.colmena;
        apps.default = self.apps.${system}.colmena;
        apps.colmena = {
          type = "app";
          program = "${defaultPackage}/bin/colmena";
        };

        devShell = pkgs.mkShell {
          RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
          NIX_PATH = "nixpkgs=${pkgs.path}";

          inputsFrom = [
            defaultPackage
            packages.manualFast
          ];
          packages = with pkgs; [
            bashInteractive
            editorconfig-checker
            nixfmt
            clippy
            rust-analyzer
            cargo-outdated
            cargo-audit
            rustfmt
            python3
            python3Packages.flake8

            nix
          ];
        };
        checks =
          let
            inputsOverlay = final: prev: {
              _inputs = inputs;
            };
          in
          if pkgs.stdenv.isLinux then
            import ./integration-tests {
              pkgs = import nixpkgs {
                inherit system;
                overlays = [
                  self.overlays.default
                  inputsOverlay
                ];
              };
              pkgsStable = import stable {
                inherit system;
                overlays = [
                  self.overlays.default
                  inputsOverlay
                ];
              };
            }
          else
            { };
      }
    )
    // {
      overlay = self.overlays.default;
      overlays.default = final: prev: {
        colmena = final.callPackage ./package.nix { };
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
    extra-substituters = [
      "https://colmena.cachix.org"
    ];
    extra-trusted-public-keys = [
      "colmena.cachix.org-1:7BzpDnjjH8ki2CT3f6GdOk7QAzPOl+1t3LvTLXqYcSg="
    ];
  };
}
