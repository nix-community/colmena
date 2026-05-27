{
  lib,
  stdenv,
  nix-gitignore,
  mdbook,
  mdbook-linkcheck2,
  python3,
  callPackage,
  writeScript,
  deploymentOptionsMd ? null,
  metaOptionsMd ? null,
  colmena ? null,

  # Full version
  version ? if colmena != null then colmena.version else "unstable",

  # Whether this build is unstable
  unstable ? version == "unstable" || lib.hasInfix "-" version,
}:

let
  apiVersion = builtins.concatStringsSep "." (lib.take 2 (lib.splitString "." version));

  colorizedHelp =
    let
      help = callPackage ./colorized-help.nix {
        inherit colmena;
      };
    in
    if colmena != null then help else null;

  redirectTemplate = lib.escapeShellArg ''
    <!doctype html>
    <html>
    <head>
      <meta charset="utf-8">
      <title>Redirecting</title>
      <meta http-equiv="refresh" content="0; URL=https://colmena.cli.rs@path@">
    </head>
    <body>
      Redirecting to <a href="https://colmena.cli.rs@path@">https://colmena.cli.rs@path@</a>
    </body>
    </html>
  '';

in
stdenv.mkDerivation {
  inherit
    version
    deploymentOptionsMd
    metaOptionsMd
    colorizedHelp
    ;

  pname = "colmena-manual" + (if unstable then "-unstable" else "");

  src = nix-gitignore.gitignoreSource [ ] ./.;

  nativeBuildInputs = [
    mdbook
    mdbook-linkcheck2
    python3
  ];

  outputs = [
    "out"
    "redirectFarm"
  ];

  COLMENA_VERSION = version;
  COLMENA_UNSTABLE = unstable;

  patchPhase = ''
    if [ -z "${toString unstable}" ]; then
        sed "s|@apiVersion@|${apiVersion}|g" book.stable.toml > book.toml
    fi
  '';

  buildPhase = ''
    if [[ -n "$colorizedHelp" ]]; then
        cat "$colorizedHelp" >> src/reference/cli.md
    else
        echo "Error: No colmena executable passed to the builder" >> src/reference/cli.md
    fi

    if [[ -n "$deploymentOptionsMd" ]]; then
        cat "$deploymentOptionsMd" >> src/reference/deployment.md
    else
        echo "No deployment options text passed the the builder" >> src/reference/deployment.md
    fi

    if [[ -n "$metaOptionsMd" ]]; then
        cat "$metaOptionsMd" >> src/reference/meta.md
    else
        echo "No meta options text passed the the builder" >> src/reference/meta.md
    fi

    mdbook build -d ./build
    cp -r ./build/html $out

    # Build the redirect farm
    # GitHub Pages doesn't play well with directory symlinks. Default
    # pages (index.html) don't work when a symlink is traversed.

    mkdir -p $redirectFarm

    subdir="/unstable"
    if [ -z "${toString unstable}" ]; then
      subdir="/${apiVersion}"
    fi

    pushd $redirectFarm
    (cd $out; find . -name "*.html") | while read -r page; do
        strippedPage=''${page#.}
        target="$subdir$strippedPage"
        mkdir -p $(dirname $page)
        echo ${redirectTemplate} | sed "s|@path@|$target|g" > $page
    done
    popd
  '';

  installPhase = "true";
}
