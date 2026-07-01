{ rawHive ? null               # Colmena Hive attrset
, rawFlake ? null              # Nix Flake attrset with `outputs.colmena`
, hermetic ? rawFlake != null  # Whether we are allowed to use <nixpkgs>
, colmenaOptions ? import ./options.nix
, colmenaModules ? import ./modules.nix
}:
with builtins;
let

  defaultHive = {
    # Will be set in defaultHiveMeta
    meta = {};

    # Like in NixOps, there is a special host named `defaults`
    # containing configurations that will be applied to all
    # hosts.
    defaults = {};

    # Darwin-specific defaults (only applied to darwin nodes)
    darwinDefaults = {};
  };

  # Check if a node is defined in darwinConfigurations (for auto-detection)
  isDarwinFromFlake = name:
    rawFlake != null &&
    rawFlake.outputs ? darwinConfigurations &&
    rawFlake.outputs.darwinConfigurations ? ${name};

  uncheckedHive = let
    flakeToHive = rawFlake:
      if rawFlake.outputs ? colmena then rawFlake.outputs.colmena else throw "Flake must define outputs.colmena.";

    rawToHive = rawHive:
      if typeOf rawHive == "lambda" || rawHive ? __functor then rawHive {}
      else if typeOf rawHive == "set" then rawHive
      else throw "The config must evaluate to an attribute set.";
  in
    if rawHive != null then rawToHive rawHive
    else if rawFlake != null then flakeToHive rawFlake
    else throw "Either a plain Hive attribute set or a Nix Flake attribute set must be specified.";

  uncheckedUserMeta =
    if uncheckedHive ? meta && uncheckedHive ? network then
      throw "Only one of `network` and `meta` may be specified. `meta` should be used as `network` is for NixOps compatibility."
    else if uncheckedHive ? meta then uncheckedHive.meta
    else if uncheckedHive ? network then uncheckedHive.network
    else {};

  # The final hive will always have the meta key instead of network.
  hive = let
    userMeta = (lib.modules.evalModules {
      modules = [ colmenaOptions.metaOptions uncheckedUserMeta ];
    }).config;

    mergedHive =
      assert lib.assertMsg (!(uncheckedHive ? __schema)) ''
        You cannot pass in an already-evaluated Hive into the evaluator.

        Hint: Use the `colmenaHive` output instead of `colmena`.
      '';
      removeAttrs (defaultHive // uncheckedHive) [ "meta" "network" ];

    meta = {
      meta =
        if !hermetic && userMeta.nixpkgs == null
        then userMeta // { nixpkgs = <nixpkgs>; }
        else userMeta;
    };
  in mergedHive // meta;

  configsFor = node: let
    nodeConfig = hive.${node};
  in
    assert lib.assertMsg (!elem node reservedNames) "\"${node}\" is a reserved name and cannot be used as the name of a node";
    if typeOf nodeConfig == "list" then nodeConfig
    else [ nodeConfig ];

  mkNixpkgs = configName: pkgConf: let
    uninitializedError = typ: ''
      Passing ${typ} as ${configName} is no longer accepted with Flakes.
      Please initialize Nixpkgs like the following:

      {
        # ...
        outputs = { nixpkgs, ... }: {
          colmena = {
            ${configName} = import nixpkgs {
              system = "x86_64-linux"; # Set your desired system here
              overlays = [];
            };
          };
        };
      }
    '';
  in
    if typeOf pkgConf == "path" || (typeOf pkgConf == "set" && pkgConf ? outPath) then
      if hermetic then throw (uninitializedError "a path to Nixpkgs")
      # The referenced file might return an initialized Nixpkgs attribute set directly
      else mkNixpkgs configName (import pkgConf)
    else if typeOf pkgConf == "lambda" then
      if hermetic then throw (uninitializedError "a Nixpkgs lambda")
      else pkgConf { overlays = []; }
    else if typeOf pkgConf == "set" then
      if pkgConf ? outputs then throw (uninitializedError "an uninitialized Nixpkgs input")
      else pkgConf
    else throw ''
      ${configName} must be one of:

      - A path to Nixpkgs (e.g., <nixpkgs>)
      - A Nixpkgs lambda (e.g., import <nixpkgs>)
      - A Nixpkgs attribute set
    '';

  nixpkgs = let
    # Can't rely on the module system yet
    nixpkgsConf =
      if uncheckedUserMeta ? nixpkgs then uncheckedUserMeta.nixpkgs
      else if hermetic then throw "meta.nixpkgs must be specified in hermetic mode."
      else <nixpkgs>;
  in mkNixpkgs "meta.nixpkgs" nixpkgsConf;

  lib = nixpkgs.lib;
  reservedNames = [ "defaults" "darwinDefaults" "network" "meta" ];

  # The `specialArgs` passed to every node evaluation.
  mkSpecialArgs = name: {
    inherit name;
    nodes = uncheckedNodes;
  } // hive.meta.specialArgs // (hive.meta.nodeSpecialArgs.${name} or {});

  # The Nixpkgs to use for a node: its per-node override, else the hive default.
  npkgsFor = name:
    if hasAttr name hive.meta.nodeNixpkgs
    then mkNixpkgs "meta.nodeNixpkgs.${name}" hive.meta.nodeNixpkgs.${name}
    else nixpkgs;

  # Determine the system type for a node.
  #
  # Priority:
  #   1. An explicit `deployment.systemType` set anywhere in the node's config.
  #   2. Auto-detection from the flake's `darwinConfigurations`.
  #   3. Default: "nixos".
  #
  # `deployment.systemType` must be read *robustly*: a node config is usually a
  # module *function* (`{ ... }: { ... }`), whose attributes cannot be observed
  # by plain attribute introspection. We therefore evaluate only the
  # platform-independent deployment options through the module system.
  # `_module.check = false` lets us ignore all the NixOS/darwin options the
  # config also sets, and lazy evaluation means those unrelated options are
  # never forced. The probe is wrapped in `tryEval` so a node with unusual
  # module-argument requirements falls back to attribute introspection rather
  # than aborting the whole evaluation.
  getNodeSystemType = name: configs: let
    probe = tryEval (lib.evalModules {
      modules = [
        colmenaOptions.deploymentOptions
        { _module.check = false; _module.args.pkgs = nixpkgs; }
      ] ++ configs;
      specialArgs = mkSpecialArgs name;
    }).config.deployment.systemType;

    # Fallback: attribute introspection, which only sees plain-attrset configs.
    introspected = lib.findFirst
      (c: c ? deployment && c.deployment ? systemType)
      null
      configs;

    explicitType =
      if probe.success then probe.value
      else if introspected != null then introspected.deployment.systemType
      else "nixos";
  in
    # `probe.value` carries the option default ("nixos") when unset, so we let
    # flake auto-detection win over a non-explicit "nixos".
    if explicitType != "nixos" then explicitType
    else if isDarwinFromFlake name then "darwin"
    else "nixos";

  # Memoize per-node system type so the detection probe runs at most once per
  # node (it is consulted both when evaluating the node and its toplevel).
  nodeSystemTypes = listToAttrs (map
    (name: { inherit name; value = getNodeSystemType name (configsFor name); })
    nodeNames);

  # Evaluate a NixOS node
  evalNixOSNode = name: configs: let
    npkgs = npkgsFor name;
    evalConfig = import (npkgs.path + "/nixos/lib/eval-config.nix");

    # Here we need to merge the configurations in meta.nixpkgs
    # and in machine config.
    nixpkgsModule = { config, lib, ... }: let
      hasTypedConfig = lib.versionAtLeast lib.version "22.11pre";
    in {
      nixpkgs.overlays = lib.mkBefore npkgs.overlays;
      nixpkgs.config = if hasTypedConfig then lib.mkBefore npkgs.config else lib.mkOptionDefault npkgs.config;

      warnings = let
        # Before 22.11, most config keys were untyped thus the merging
        # was broken. Let's warn the user if not all config attributes
        # set in meta.nixpkgs are overridden.
        metaKeys = attrNames npkgs.config;
        nodeKeys = [ "doCheckByDefault" "warnings" "allowAliases" ] ++ (attrNames config.nixpkgs.config);
        remainingKeys = filter (k: ! elem k nodeKeys) metaKeys;
      in
        lib.optional (!hasTypedConfig && length remainingKeys != 0)
        "The following Nixpkgs configuration keys set in meta.nixpkgs will be ignored: ${toString remainingKeys}";
    };
  in evalConfig {
    inherit (npkgs.stdenv.hostPlatform) system;

    modules = [
      nixpkgsModule
      colmenaModules.assertionModule
      colmenaModules.keyChownModule
      colmenaModules.keyServiceModule
      colmenaOptions.deploymentOptions
      hive.defaults
    ] ++ configs;
    specialArgs = mkSpecialArgs name;
  };

  # Evaluate a darwin node
  evalDarwinNode = name: configs: let
    npkgs = npkgsFor name;

    # Get the nix-darwin input from meta. It must be provided for darwin nodes.
    darwinInput = hive.meta.nix-darwin or null;

    # Use darwin's eval-config. nix-darwin exposes `darwinSystem` under `lib` or
    # as a top-level flake output. The null check must live here (not in a
    # separate `let` binding) so it is actually forced when the node is
    # evaluated — an unreferenced `assert` binding would never run.
    evalConfig =
      if darwinInput == null then
        throw "meta.nix-darwin must be set when deploying darwin nodes. Add your nix-darwin flake input to meta.nix-darwin."
      else if darwinInput ? lib.darwinSystem then darwinInput.lib.darwinSystem
      else if darwinInput ? darwinSystem then darwinInput.darwinSystem
      else throw "Could not find darwinSystem in meta.nix-darwin. Expected a nix-darwin flake input.";

    # Darwin-specific nixpkgs module
    nixpkgsModule = { lib, ... }: let
      hasTypedConfig = lib.versionAtLeast lib.version "22.11pre";
    in {
      nixpkgs.overlays = lib.mkBefore npkgs.overlays;
      nixpkgs.config = if hasTypedConfig then lib.mkBefore npkgs.config else lib.mkOptionDefault npkgs.config;
    };

  in evalConfig {
    inherit (npkgs.stdenv.hostPlatform) system;

    modules = [
      nixpkgsModule
      colmenaModules.assertionModule
      colmenaOptions.deploymentOptions
      hive.defaults
      hive.darwinDefaults
    ] ++ configs;
    specialArgs = mkSpecialArgs name;
  };

  # Evaluate a node based on its system type
  evalNode = name: configs: let
    systemType = nodeSystemTypes.${name};
  in
    if systemType == "darwin" then evalDarwinNode name configs
    else evalNixOSNode name configs;

  nodeNames = filter (name: ! elem name reservedNames) (attrNames hive);

  # Used as the `nodes` argument in modules. We skip recursive type checking
  # for performance.
  uncheckedNodes = listToAttrs (map (name: let
    configs = [
      {
        _module.check = false;
      }
    ] ++ configsFor name;
  in {
    inherit name;
    value = evalNode name configs;
  }) nodeNames);

  # Add required config Key here since we don't want to eval nixpkgs
  metaConfigKeys = [
    "name" "description"
    "machinesFile"
    "allowApplyAll"
  ];

  # Get the toplevel/system output based on node system type
  # NixOS uses system.build.toplevel, darwin uses system directly
  getNodeToplevel = name: node: let
    systemType = nodeSystemTypes.${name};
  in
    if systemType == "darwin" then node.system
    else node.config.system.build.toplevel;

in rec {
  # Exported attributes
  __schema = "v0.5";

  nodes = listToAttrs (map (name: { inherit name; value = evalNode name (configsFor name); }) nodeNames);
  toplevel =         lib.mapAttrs getNodeToplevel nodes;
  deploymentConfig = lib.mapAttrs (_: v: v.config.deployment)            nodes;
  deploymentConfigSelected = names: lib.filterAttrs (name: _: elem name names) deploymentConfig;
  evalSelected =             names: lib.filterAttrs (name: _: elem name names) toplevel;
  evalSelectedDrvPaths =     names: lib.mapAttrs    (_: v: v.drvPath)          (evalSelected names);
  metaConfig = lib.filterAttrs (n: v: elem n metaConfigKeys) hive.meta;
  introspect = f: f { inherit lib; pkgs = nixpkgs; nodes = uncheckedNodes; };
}
