{
  lib,
  writeShellScriptBin,
  cargo,
  deno,
  nixfmt-tree,
  taplo,
}:

writeShellScriptBin "formatter" ''
  set -eoux pipefail
  shopt -s globstar

  root="$PWD"
  while [[ ! -f "$root/.git/index" ]]; do
    if [[ "$root" == "/" ]]; then
      exit 1
    fi
    root="$(dirname "$root")"
  done
  pushd "$root" > /dev/null

  # disable this for now
  # ${lib.getExe deno} fmt **/*.md **/*.{yml,yaml} **/*.js

  ${lib.getExe nixfmt-tree} .

  ${lib.getExe taplo} format **/*.toml

  ${lib.getExe cargo} clippy --all-features --fix --allow-dirty -- -D warnings
  ${lib.getExe cargo} fmt --all

  popd
''
