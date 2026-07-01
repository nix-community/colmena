{
  pkgs ? import ./nixpkgs.nix,
  pkgsStable ? import ./nixpkgs-stable.nix,
}:

{
  apply = import ./apply { inherit pkgs; };
  apply-streaming = import ./apply {
    inherit pkgs;
    evaluator = "streaming";
  };
  apply-local = import ./apply-local { inherit pkgs; };
  build-on-target = import ./build-on-target { inherit pkgs; };
  exec = import ./exec { inherit pkgs; };

  flakes = import ./flakes {
    inherit pkgs;
  };
  flakes-impure = import ./flakes {
    inherit pkgs;
    pure = false;
  };
  flakes-streaming = import ./flakes { inherit pkgs; evaluator = "streaming"; };

  parallel = import ./parallel { inherit pkgs; };

  allow-apply-all = import ./allow-apply-all { inherit pkgs; };

  apply-stable = import ./apply { pkgs = pkgsStable; };
}
