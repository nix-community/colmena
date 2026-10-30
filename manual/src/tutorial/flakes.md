# Usage with Flakes

## Installation

<!-- STABLE_BEGIN -->
`colmena` is included in Nixpkgs beginning with 21.11.

For this tutorial, use the following command to enter an ephemeral environment with the `colmena` command:

```bash
nix shell nixpkgs#colmena
```

If you are interested in trying out the bleeding-edge version of Colmena, Read [the unstable version](https://colmena.cli.rs/unstable) of the Manual for instructions.
<!-- STABLE_END -->

<!-- UNSTABLE_BEGIN -->
<!-- To install the latest stable version, read [the corresponding Manual](https://colmena.cli.rs/stable) for instructions. -->

To quickly try Colmena out, use the following command to enter an ephemeral environment with the latest development version of `colmena`:

```bash
nix shell github:nix-community/colmena
```

To install Colmena to the user profile, use the following command:

```bash
nix profile add github:nix-community/colmena
```

You can also add `github:nix-community/colmena` as an input in your Flake and add the `colmena` package to your `devShell`.

### Unstable Binary Cache

A public binary cache is available at [https://colmena.cachix.org](https://colmena.cachix.org), courtesy of Cachix.
This binary cache contains unstable versions of Colmena built by [GitHub Actions](https://github.com/nix-community/colmena/actions).
<!-- UNSTABLE_END -->

## Basic Configuration

Colmena reads the `colmenaHive` output in your Flake, generated with `colmena.lib.makeHive`.

Here is a short example:

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    colmena.url = "github:nix-community/colmena";
  };
  outputs = { nixpkgs, colmena, ... }: {
    colmenaHive = colmena.lib.makeHive {
      meta = {
        nixpkgs = import nixpkgs {
          system = "x86_64-linux";
          overlays = [];
        };
      };

      host-a = { name, nodes, pkgs, ... }: {
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
```

The full set of `deployment` options can be found [here](../reference/deployment.md).
You can also check out the example in [the main tutorial](index.md) for some inspiration.

Now you are ready to use Colmena! To build the configuration:

```bash
colmena build
```

To build and deploy to all nodes:

```bash
colmena apply
```

## Migrating to Direct Flake Evaluation

> error: flake 'git+file:///path/to/flake' does not provide attribute 'packages.x86_64-linux.colmenaHive', 'legacyPackages.x86_64-linux.colmenaHive' or 'colmenaHive'

Colmena now uses `nix eval` to evaluate flakes.
Your flake needs to depend on Colmena itself as an input and expose a new output called `colmenaHive`:

```diff
 {
   inputs = {
+    # ADDED: Colmena input
+    colmena.url = "github:nix-community/colmena";

     # ... Rest of configuration ...
   };
   outputs = { self, colmena, ... }: {
+    # ADDED: New colmenaHive output
+    colmenaHive = colmena.lib.makeHive self.outputs.colmena;

     # Your existing colmena output
     colmena = {
       # ... Rest of configuration ...
     };
   };
 }
```

## Next Steps

- Head to the [Features](../features/index.md) section to see what else Colmena can do.
- Read more about options available in Colmena in the [Reference](../reference/index.md) section.
