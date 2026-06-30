{
  pkgs,
  evaluator ? "chunked",
}:

let
  tools = pkgs.callPackage ../tools.nix { };
in
tools.runTest {
  name = "colmena-apply-${evaluator}";

  colmena.test = {
    bundle = ./.;
    testScript = ''
      colmena = "${tools.colmenaExec}"
      evaluator = "${evaluator}"
    ''
    + builtins.readFile ./test-script.py;
  };
}
