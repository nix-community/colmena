# Ad Hoc Evaluation

Sometimes you may want to extract values from your Hive configuration for consumption in another program (e.g., [OctoDNS](https://github.com/octodns/octodns)).
To do that, create a `.nix` file with a lambda:

```nix
{ nodes, pkgs, lib, ... }:
# Feels like a NixOS module - But you can return any JSON-serializable value
lib.attrsets.mapAttrs (k: v: v.config.deployment.targetHost) nodes
```

Then you can obtain a JSON output with:

```console
$ colmena eval target-hosts.nix
{"alpha":"fd12:3456::1","beta":"fd12:3456::2"}
```

You can also specify an expression directly on the command line:

```console
$ colmena eval -E '{ nodes, pkgs, lib, ... }: ...'
```

## Evaluating Selected Hosts

To evaluate the system profile derivations for selected hosts, pass the same `--on` selector used
by `apply` and `build`:

```console
$ colmena eval --on @web
{
  "alpha": "/nix/store/00000000000000000000000000000000-nixos-system-alpha.drv",
  "beta": "/nix/store/11111111111111111111111111111111-nixos-system-beta.drv"
}
```

This is useful for quickly checking whether selected hosts still evaluate without building them.
Use `--eval-node-limit` to control how many hosts are evaluated in each Nix invocation.

## Instantiation

You may directly instantiate an expression that evaluates to a derivation:

```console
$ colmena eval --instantiate -E '{ nodes, ... }: nodes.alpha.config.boot.kernelPackages.kernel'
/nix/store/7ggmhnwvywrqcd1z2sdpan8afz55sw7z-linux-5.14.14.drv
```

## Interactive REPL

To explore the configurations interactively, start a REPL session with `colmena repl`:

```console
$ colmena repl
[INFO ] Using flake: git+file:///home/user/cluster
Welcome to Nix 2.10.3. Type :? for help.

Loading installable ''...
Added 3 variables.
nix-repl> nodes.alpha.config.deployment.targetHost
"fd12:3456::1"
```
