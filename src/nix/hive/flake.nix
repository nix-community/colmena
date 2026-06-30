# Only used for --legacy-flake-eval
{
  description = "Internal Colmena expressions (deprecated)";

  inputs = {
    hive.url = "%hive%";
  };

  outputs = { self, hive }: {
    processFlake =
      let
        compatibleSchema = "v0.5";

        # Evaluates a raw hive.
        #
        # This uses the `colmena` output.
        evalHive =
          rawFlake:
          import ./eval.nix {
            inherit rawFlake;
            hermetic = true;
            colmenaOptions = import ./options.nix;
            colmenaModules = import ./modules.nix;
          };

        # Uses an already-evaluated hive.
        #
        # This uses the `colmenaHive` output.
        checkPreparedHive =
          hiveOutput:
          if !(hiveOutput ? __schema) then
            throw ''
              The colmenaHive output does not contain a valid evaluated hive.

              Hint: Use `colmena.lib.makeHive`.
            ''
          else if hiveOutput.__schema != compatibleSchema then
            throw ''
              The colmenaHive output (schema ${hiveOutput.__schema}) isn't compatible with this version of Colmena.

              Hint: Use the same version of Colmena as in the Flake input.
            ''
          else
            hiveOutput;
      in
      if hive.outputs ? colmenaHive then checkPreparedHive hive.outputs.colmenaHive else evalHive hive;
  };
}
