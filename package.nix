{
  lib,
  stdenv,
  rustPlatform,
  nix-gitignore,
  installShellFiles,
  nix-eval-jobs,
}:

let
  fs = lib.fileset;
  srcIgnored = fs.unions [
    ./.github
    ./CNAME
    ./renovate.json

    ./manual
    ./integration-tests

    ./nix
    ./default.nix
    ./flake-compat.nix
    ./package.nix
    ./shell.nix
  ];
  srcFiles = fs.difference ./. srcIgnored;
in
rustPlatform.buildRustPackage rec {
  pname = "colmena";
  version = "0.5.0-pre";

  src = fs.toSource {
    root = ./.;
    fileset = srcFiles;
  };

  cargoLock = {
    lockFile = ./Cargo.lock;
  };

  nativeBuildInputs = [ installShellFiles ];

  buildInputs = [ nix-eval-jobs ];

  NIX_EVAL_JOBS = "${nix-eval-jobs}/bin/nix-eval-jobs";

  preBuild = ''
    if [[ -z "$NIX_EVAL_JOBS" ]]; then
      unset NIX_EVAL_JOBS
    fi
  '';

  postInstall = lib.optionalString (stdenv.hostPlatform == stdenv.buildPlatform) ''
    installShellCompletion --cmd colmena \
      --bash <($out/bin/colmena gen-completions bash) \
      --zsh <($out/bin/colmena gen-completions zsh) \
      --fish <($out/bin/colmena gen-completions fish)
  '';

  # Recursive Nix is not stable yet
  doCheck = false;

  passthru = {
    # We guarantee CLI and Nix API stability for the same minor version
    apiVersion = builtins.concatStringsSep "." (lib.take 2 (lib.splitString "." version));
  };

  meta = with lib; {
    description = "A simple, stateless NixOS deployment tool";
    homepage = "https://colmena.cli.rs/${passthru.apiVersion}";
    license = licenses.mit;
    maintainers = with maintainers; [ zhaofengli ];
    platforms = platforms.linux ++ platforms.darwin;
    mainProgram = "colmena";
  };
}
