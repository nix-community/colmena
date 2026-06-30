{
  lib,
  stdenv,
  runCommand,
  colmena,
  ansi2html,
}:

with builtins;

let
  subcommands = [
    null
    "apply"
  ]
  ++ lib.optional stdenv.isLinux "apply-local"
  ++ [
    "build"
    "upload-keys"
    "eval"
    "exec"
    "nix-info"
    "repl"
  ];
  renderHelp =
    subcommand:
    let
      fullCommand = if subcommand == null then "colmena" else "colmena ${subcommand}";
    in
    ''
      (
          echo '## `${fullCommand}`'
          echo -n '<pre><div class="hljs">'
          TERM=xterm-256color CLICOLOR_FORCE=1 ${fullCommand} --help | ansi2html -p
          echo '</div></pre>'
      )>>$out
    '';
in
runCommand "colmena-colorized-help"
  {
    nativeBuildInputs = [
      colmena
      ansi2html
    ];
  }
  (
    ''
      ansi2html -H > $out
    ''
    + concatStringsSep "\n" (map renderHelp subcommands)
  )
