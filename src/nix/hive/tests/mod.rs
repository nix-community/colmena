//! Integration-ish tests

use super::*;

use crate::error::ColmenaError;
use crate::nix::deployment::{Deployment, EvaluationNodeLimit, EvaluatorType, Goal, Options};
use std::collections::HashSet;
use std::fs;
use std::hash::Hash;
use std::io::Write;
use std::iter::{FromIterator, Iterator};
use std::ops::Deref;
use std::path::{Path, PathBuf};

use tempfile::{Builder as TempFileBuilder, NamedTempFile, TempDir};
use tokio_test::block_on;

macro_rules! node {
    ($n:expr) => {
        NodeName::new($n.to_string()).unwrap()
    };
}

fn set_eq<T>(a: &[T], b: &[T]) -> bool
where
    T: Eq + Hash,
{
    let a: HashSet<_> = HashSet::from_iter(a);
    let b: HashSet<_> = HashSet::from_iter(b);

    a == b
}

/// An ad-hoc Hive configuration.
struct TempHive {
    hive: Hive,
    _temp_file: NamedTempFile,
}

impl TempHive {
    pub fn new(text: &str) -> Self {
        Self::with_flags(text, NixFlags::default())
    }

    pub fn with_flags(text: &str, flags: NixFlags) -> Self {
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(text.as_bytes()).unwrap();

        let hive_path = block_on(HivePath::from_path(temp_file.path(), &flags)).unwrap();
        let hive = Hive::new(hive_path, flags).unwrap();

        Self {
            hive,
            _temp_file: temp_file,
        }
    }

    /// Asserts that the configuration is valid.
    ///
    /// Note that this _does not_ attempt to evaluate `config.toplevel`.
    pub fn valid(text: &str) {
        let mut flags = NixFlags::default();
        flags.set_show_trace(true);

        let hive = Self::with_flags(text, flags);
        assert!(block_on(hive.deployment_info()).is_ok());
    }

    /// Asserts that the configuration is invalid.
    ///
    /// Note that this _does not_ attempt to evaluate `config.toplevel`.
    pub fn invalid(text: &str) {
        let hive = Self::new(text);
        assert!(block_on(hive.deployment_info()).is_err());
    }

    /// Asserts that the specified nodes can be fully evaluated.
    pub fn eval_success(text: &str, nodes: Vec<NodeName>) {
        let hive = Self::new(text);
        let profiles = block_on(hive.eval_selected(&nodes, None));
        assert!(profiles.is_ok());
    }

    /// Asserts that the specified nodes will fail to evaluate.
    pub fn eval_failure(text: &str, nodes: Vec<NodeName>) {
        let hive = Self::new(text);
        let profiles = block_on(hive.eval_selected(&nodes, None));
        assert!(profiles.is_err());
    }
}

impl Deref for TempHive {
    type Target = Hive;

    fn deref(&self) -> &Hive {
        &self.hive
    }
}

// eval.nix tests

#[test]
fn test_parse_simple() {
    let hive = TempHive::new(
        r#"
      {
        defaults = { pkgs, ... }: {
          environment.systemPackages = with pkgs; [
            vim wget curl
          ];
          boot.loader.grub.device = "/dev/sda";
          fileSystems."/" = {
            device = "/dev/sda1";
            fsType = "ext4";
          };

          deployment.tags = [ "common-tag" ];
        };

        host-a = { name, nodes, ... }: {
          networking.hostName = name;
          time.timeZone = nodes.host-b.config.time.timeZone;

          deployment.tags = [ "a-tag" ];
        };

        host-b = {
          deployment = {
            targetHost = "somehost.tld";
            targetPort = 1234;
            targetUser = "luser";
          };
          time.timeZone = "America/Los_Angeles";
        };
      }
    "#,
    );
    let nodes = block_on(hive.deployment_info()).unwrap();

    assert!(set_eq(
        &["host-a", "host-b"],
        &nodes.keys().map(NodeName::as_str).collect::<Vec<&str>>(),
    ));

    // host-a
    let host_a = &nodes[&node!("host-a")];
    assert!(set_eq(
        &["common-tag", "a-tag"],
        &host_a
            .tags
            .iter()
            .map(String::as_str)
            .collect::<Vec<&str>>(),
    ));
    assert_eq!(Some("host-a"), host_a.target_host.as_deref());
    assert_eq!(None, host_a.target_port);
    assert_eq!(Some("root"), host_a.target_user.as_deref());

    // host-b
    let host_b = &nodes[&node!("host-b")];
    assert!(set_eq(
        &["common-tag"],
        &host_b
            .tags
            .iter()
            .map(String::as_str)
            .collect::<Vec<&str>>(),
    ));
    assert_eq!(Some("somehost.tld"), host_b.target_host.as_deref());
    assert_eq!(Some(1234), host_b.target_port);
    assert_eq!(Some("luser"), host_b.target_user.as_deref());
}

/// Copies the makehive flake into a temporary directory with the colmena
/// input pointed at this checkout.
fn makehive_flake_dir() -> TempDir {
    let src_dir = PathBuf::from("./src/nix/hive/tests/makehive-flake");
    let flake_dir = TempFileBuilder::new()
        .prefix("makehive-flake-")
        .tempdir()
        .unwrap();

    for entry in fs::read_dir(src_dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            fs::copy(entry.path(), flake_dir.as_ref().join(entry.file_name())).unwrap();
        }
    }

    let flake_nix = flake_dir.as_ref().join("flake.nix");
    let patched_flake = fs::read_to_string(&flake_nix)
        .unwrap()
        .replace("@repoPath@", env!("CARGO_MANIFEST_DIR"));

    fs::write(flake_nix, patched_flake).unwrap();

    flake_dir
}

/// Loads the makehive flake in the given directory as a hive.
fn makehive_flake(flake_dir: &Path) -> Hive {
    let flake = block_on(Flake::from_dir(flake_dir, &NixFlags::default())).unwrap();

    let mut flags = NixFlags::default();
    flags.set_show_trace(true);

    Hive::new(HivePath::Flake(flake), flags).unwrap()
}

#[test]
fn test_parse_makehive_flake() {
    let flake_dir = makehive_flake_dir();
    let hive = makehive_flake(flake_dir.path());

    let nodes = block_on(hive.deployment_info()).unwrap();
    assert!(set_eq(
        &["host-a", "host-b"],
        &nodes.keys().map(NodeName::as_str).collect::<Vec<&str>>(),
    ));

    // nix-eval-jobs --flake <installable> --select <fn>
    {
        let selected = hive.eval_selected_expr(&[node!("host-a")]).unwrap();
        assert!(selected.installable().unwrap().ends_with("#colmenaHive"));

        let expr = selected.expression();
        assert!(expr.starts_with("with builtins; hive:"));
        assert!(expr.contains("host-a"));
    }

    drop(flake_dir);
}

#[test]
fn test_repl_expression_flake_evaluates() {
    let flake_dir = makehive_flake_dir();
    let hive = makehive_flake(flake_dir.path());

    let mut expr_file = TempFileBuilder::new()
        .prefix("colmena-repl-")
        .suffix(".nix")
        .tempfile()
        .unwrap();
    expr_file
        .write_all(hive.get_repl_expression().as_bytes())
        .unwrap();

    // nix repl is impure, and getFlake on an unlocked ref needs that
    let mut flags = NixFlags::default();
    flags.set_impure(true);

    let node_names = || {
        block_on(
            NixCommand::nix(flags.clone())
                .args(["eval", "--file"])
                .arg(expr_file.path())
                .args(["--apply", "s: builtins.attrNames s.nodes"])
                .build()
                .capture_output(),
        )
        .unwrap()
    };

    let output = node_names();
    assert!(output.contains("host-a"));
    assert!(output.contains("host-b"));

    // evaluating the same file again must see an edit, which a locked uri would not
    let flake_nix = flake_dir.path().join("flake.nix");
    let edited = fs::read_to_string(&flake_nix).unwrap().replace(
        "host-b = {",
        "host-c = { boot.isContainer = true; };\n      host-b = {",
    );
    fs::write(&flake_nix, edited).unwrap();

    assert!(node_names().contains("host-c"));
}

/// Returns an expression for a derivation no earlier run has written.
fn fresh_derivation(name: &str) -> String {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    format!(
        r#"derivation {{ name = "colmena-test-{name}-{nonce}"; builder = "/bin/sh"; system = "x86_64-linux"; }}"#
    )
}

#[test]
fn test_introspect_instantiate_flake() {
    let flake_dir = makehive_flake_dir();
    let hive = makehive_flake(flake_dir.path());

    let expr = format!("{{ ... }}: {}", fresh_derivation("flake"));
    let output = block_on(hive.introspect(expr, true)).unwrap();

    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(1, lines.len());
    assert!(lines[0].ends_with(".drv"));
    assert!(Path::new(lines[0]).exists());
    assert!(output.ends_with('\n'));
}

#[test]
fn test_drv_paths_snippet() {
    // fake derivations, nothing is instantiated
    let eval = |fixture: &str| {
        block_on(
            NixCommand::nix(NixFlags::default())
                .args(["eval", "--json", "--expr"])
                .arg(format!(
                    r#"{} (let fakeDrv = name: {{ type = "derivation"; drvPath = "/nix/store/${{name}}.drv"; outputName = "out"; }}; in {})"#,
                    DRV_PATHS_SNIPPET, fixture
                ))
                .build()
                .capture_output(),
        )
        .unwrap()
    };

    let set = r#"{
      a = fakeDrv "a";
      b = "str";
      c = { recurseForDerivations = true; d = fakeDrv "d"; };
      e = { f = fakeDrv "f"; };
      g = (fakeDrv "g") // { outputName = "bin"; };
      "x.y" = fakeDrv "xy";
    }"#;
    assert_eq!(
        r#"["/nix/store/a.drv","/nix/store/d.drv","/nix/store/g.drv!bin","/nix/store/xy.drv"]"#,
        eval(set).trim()
    );

    let list = r#"[ (fakeDrv "a") [ (fakeDrv "b") ] { c = fakeDrv "c"; } ]"#;
    assert_eq!(
        r#"["/nix/store/a.drv","/nix/store/b.drv","/nix/store/c.drv"]"#,
        eval(list).trim()
    );

    // the bindings of the snippet stay out of the scope of the value
    let scope = r#"with { drvPath = fakeDrv "w"; }; drvPath"#;
    assert_eq!(r#"["/nix/store/w.drv"]"#, eval(scope).trim());

    // lix prints the context of an error only with --show-trace
    let mut flags = NixFlags::default();
    flags.set_show_trace(true);

    let error = |fixture: &str| {
        let mut command = NixCommand::nix(flags.clone())
            .args(["eval", "--expr"])
            .arg(format!("{} ({})", DRV_PATHS_SNIPPET, fixture))
            .build();
        let output = block_on(async { command.output().await }).unwrap();

        assert!(!output.status.success());
        String::from_utf8(output.stderr).unwrap()
    };

    // a set with __functor throws like a function, where nix-instantiate
    // would call it
    let functor = "{ __functor = self: x: x; }";
    assert!(error(functor).contains("The expression must evaluate to a derivation"));

    // an error names the attribute it comes from
    let attribute = r#"{ a = throw "boom"; }"#;
    assert!(error(attribute).contains("while evaluating the attribute 'a'"));
}

#[test]
fn test_parse_node_references() {
    TempHive::valid(
        r#"
      with builtins;
      {
        host-a = { name, nodes, ... }:
          assert name == "host-a";
          assert length (attrNames nodes) == 2;
        {
          time.timeZone = "America/Los_Angeles";
        };
        host-b = { name, nodes, ... }:
          assert name == "host-b";
          assert length (attrNames nodes) == 2;
          assert nodes.host-a.config.time.timeZone == "America/Los_Angeles";
        {};
      }
    "#,
    );
}

#[test]
fn test_parse_unknown_option() {
    TempHive::invalid(
        r#"
      {
        bad = {
          deployment.noSuchOption = "not kidding";
        };
      }
    "#,
    );
}

#[test]
fn test_config_list() {
    TempHive::valid(
        r#"
      with builtins;
      {
        host-a = [
          {
            time.timeZone = "America/Los_Angeles";
          }
          {
            deployment.tags = [ "some-tag" ];
          }
        ];
        host-b = { name, nodes, ... }:
          assert length (attrNames nodes) == 2;
          assert nodes.host-a.config.time.timeZone == "America/Los_Angeles";
          assert elem "some-tag" nodes.host-a.config.deployment.tags;
        {};
      }
    "#,
    );
}

#[test]
fn test_parse_key_text() {
    TempHive::valid(
        r#"
      {
        test = {
          deployment.keys.topSecret = {
            text = "be sure to drink your ovaltine";
          };
        };
      }
    "#,
    );
}

#[test]
fn test_parse_key_command_good() {
    TempHive::valid(
        r#"
      {
        test = {
          deployment.keys.elohim = {
            keyCommand = [ "eternalize" ];
          };
        };
      }
    "#,
    );
}

#[test]
fn test_parse_key_command_bad() {
    TempHive::invalid(
        r#"
      {
        test = {
          deployment.keys.elohim = {
            keyCommand = "transcend";
          };
        };
      }
    "#,
    );
}

#[test]
fn test_parse_key_file() {
    TempHive::valid(
        r#"
      {
        test = {
          deployment.keys.l337hax0rwow = {
            keyFile = "/etc/passwd";
          };
        };
      }
    "#,
    );
}

#[test]
fn test_key_assertion_message() {
    let hive = TempHive::new(
        r#"
      {
        test = {
          boot.isContainer = true;
          nixpkgs.system = "x86_64-linux";
          deployment.keys.both = {
            text = "secret";
            keyFile = "/etc/passwd";
          };
        };
      }
    "#,
    );

    let expr = r#"
      { nodes, ... }:
        map (a: a.message) (builtins.filter (a: !a.assertion) nodes.test.config.assertions)
    "#
    .to_string();

    let messages = block_on(hive.introspect(expr, false)).unwrap();

    assert!(messages.contains("`test.deployment.keys.both.text`"));
}

#[test]
fn test_eval_non_existent_pkg() {
    // Sanity check
    TempHive::eval_failure(
        r#"
      {
        test = { pkgs, ... }: {
          boot.isContainer = true;
          nixpkgs.system = "x86_64-linux";
          environment.systemPackages = with pkgs; [ thisPackageDoesNotExist ];
        };
      }
    "#,
        vec![node!("test")],
    );
}

// Nixpkgs config tests

#[test]
fn test_nixpkgs_system() {
    TempHive::valid(
        r#"
      {
        meta = {
          nixpkgs = import <nixpkgs> {
            system = "armv5tel-linux";
          };
        };
        test = { pkgs, ... }: {
          boot.isContainer = assert pkgs.system == "armv5tel-linux"; true;
        };
      }
    "#,
    );

    TempHive::valid(
        r#"
      {
        meta = {
          nixpkgs = import <nixpkgs> {
            system = "x86_64-linux";
          };
        };
        test = { pkgs, ... }: {
          nixpkgs.system = "armv5tel-linux";
          boot.isContainer = assert pkgs.system == "armv5tel-linux"; true;
        };
      }
    "#,
    );
}

#[test]
fn test_nixpkgs_path_like() {
    TempHive::valid(
        r#"
      {
        meta = {
          nixpkgs = {
            outPath = <nixpkgs>;
          };
        };
        test = { pkgs, ... }: {
          boot.isContainer = true;
        };
      }
    "#,
    );
}

#[test]
fn test_nixpkgs_overlay_meta_nixpkgs() {
    // Only set overlays in meta.nixpkgs
    TempHive::eval_success(
        r#"
      {
        meta = {
          nixpkgs = import <nixpkgs> {
            system = "x86_64-linux";
            overlays = [
              (self: super: { my-coreutils = super.coreutils; })
            ];
          };
        };
        test = { pkgs, ... }: {
          boot.isContainer = true;
          environment.systemPackages = with pkgs; [ my-coreutils ];
        };
      }
    "#,
        vec![node!("test")],
    );
}

#[test]
fn test_nixpkgs_overlay_node_config() {
    // Only set overlays in node config
    TempHive::eval_success(
        r#"
      {
        test = { pkgs, ... }: {
          boot.isContainer = true;
          nixpkgs.system = "x86_64-linux";
          nixpkgs.overlays = [
            (self: super: { my-coreutils = super.coreutils; })
          ];
          environment.systemPackages = with pkgs; [ my-coreutils ];
        };
      }
    "#,
        vec![node!("test")],
    );
}

#[test]
fn test_nixpkgs_overlay_both() {
    // Set overlays both in meta.nixpkgs and in node config
    TempHive::eval_success(
        r#"
      {
        meta = {
          nixpkgs = import <nixpkgs> {
            system = "x86_64-linux";
            overlays = [
              (self: super: { meta-coreutils = super.coreutils; })
            ];
          };
        };
        test = { pkgs, ... }: {
          boot.isContainer = true;
          nixpkgs.overlays = [
            (self: super: { node-busybox = super.busybox; })
          ];
          environment.systemPackages = with pkgs; [ meta-coreutils node-busybox ];
        };
      }
    "#,
        vec![node!("test")],
    );
}

#[test]
fn test_nixpkgs_config_meta_nixpkgs() {
    // Set config in meta.nixpkgs
    TempHive::eval_success(
        r#"
      {
        meta = {
          nixpkgs = import <nixpkgs> {
            system = "x86_64-linux";
            config = {
              allowUnfree = true;
            };
          };
        };
        test = { pkgs, ... }: {
          nixpkgs.config = {
            allowAliases = false;
          };
          boot.isContainer = assert pkgs.config.allowUnfree; true;
        };
      }
    "#,
        vec![node!("test")],
    );
}

#[test]
fn test_nixpkgs_config_node_config() {
    // Set config in node config
    TempHive::eval_success(
        r#"
      {
        test = { pkgs, ... }: {
          nixpkgs.system = "x86_64-linux";
          nixpkgs.config = {
            allowUnfree = true;
          };
          boot.isContainer = assert pkgs.config.allowUnfree; true;
        };
      }
    "#,
        vec![node!("test")],
    );
}

#[test]
fn test_nixpkgs_config_override() {
    // Set same config both in meta.nixpkgs and in node config
    let template = r#"
      {
        meta = {
          nixpkgs = import <nixpkgs> {
            system = "x86_64-linux";
            config = {
              allowUnfree = META_VAL;
            };
          };
        };
        test = { pkgs, ... }: {
          nixpkgs.config = {
            allowUnfree = NODE_VAL;
          };
          boot.isContainer = assert pkgs.config.allowUnfree == EXPECTED_VAL; true;
        };
      }
    "#;

    TempHive::eval_success(
        &template
            .replace("META_VAL", "true")
            .replace("NODE_VAL", "false")
            .replace("EXPECTED_VAL", "false"),
        vec![node!("test")],
    );

    TempHive::eval_success(
        &template
            .replace("META_VAL", "false")
            .replace("NODE_VAL", "true")
            .replace("EXPECTED_VAL", "true"),
        vec![node!("test")],
    );
}

#[test]
fn test_meta_special_args() {
    TempHive::valid(
        r#"
      {
        meta.specialArgs = {
          undine = "assimilated";
        };

        borg = { undine, ... }:
          assert undine == "assimilated";
        {
          boot.isContainer = true;
        };
      }
    "#,
    );
}

#[test]
fn test_meta_node_special_args() {
    TempHive::valid(
        r#"
      {
        meta.specialArgs = {
          someArg = "global";
        };

        meta.nodeSpecialArgs.node-a = {
          someArg = "node-specific";
        };

        node-a = { someArg, ... }:
          assert someArg == "node-specific";
        {
          boot.isContainer = true;
        };

        node-b = { someArg, ... }:
          assert someArg == "global";
        {
          boot.isContainer = true;
        };
      }
    "#,
    );
}

#[test]
fn test_hive_autocall() {
    TempHive::valid(
        r#"
      {
        argument ? "with default value"
      }: {
        borg = { ... }: {
          boot.isContainer = true;
        };
      }
    "#,
    );

    TempHive::valid(
        r#"
      {
        some = "value";
        __functor = self: { argument ? "with default value" }: {
          borg = { ... }: {
            boot.isContainer = assert self.some == "value"; true;
          };
        };
      }
    "#,
    );

    TempHive::invalid(
        r#"
      {
        thisWontWork
      }: {
        borg = { ... }: {
          boot.isContainer = true;
        };
      }
    "#,
    );
}

#[test]
fn test_hive_introspect_instantiate() {
    let hive = TempHive::new(
        r#"
      {
        test = { ... }: {
          boot.isContainer = true;
        };
      }
    "#,
    );

    // the same derivation twice prints one path
    let expr = format!(
        "{{ ... }}: let drv = {}; in [ drv drv ]",
        fresh_derivation("hive")
    );
    let output = block_on(hive.introspect(expr, true)).unwrap();

    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(1, lines.len());
    assert!(lines[0].ends_with(".drv"));
    assert!(Path::new(lines[0]).exists());
}

#[test]
fn test_hive_introspect() {
    let hive = TempHive::new(
        r#"
      {
        test = { ... }: {
          boot.isContainer = true;
        };
      }
    "#,
    );

    let expr = r#"
      { pkgs, lib, nodes }:
        assert pkgs ? hello;
        assert lib ? versionAtLeast;
        nodes.test.config.boot.isContainer
    "#
    .to_string();

    let eval = block_on(hive.introspect(expr, false)).unwrap();

    assert_eq!("true", eval);
}

#[test]
fn test_eval_node_limit_none_without_targets() {
    for evaluator in [EvaluatorType::Chunked, EvaluatorType::Streaming] {
        let TempHive { hive, _temp_file } = TempHive::new("{ }");

        let mut deployment = Deployment::new(hive, HashMap::new(), Goal::Build, None);
        let mut options = Options::default();
        options.set_evaluator(evaluator);
        deployment.set_options(options);
        deployment.set_evaluation_node_limit(EvaluationNodeLimit::None);

        block_on(deployment.execute()).unwrap();
    }
}

#[test]
fn test_hive_get_meta() {
    let hive = TempHive::new(
        r#"
      {
        meta.allowApplyAll = false;
        meta.specialArgs = {
          this_is_new = false;
        };
      }
  "#,
    );

    let eval = block_on(hive.get_meta_config()).unwrap();

    eprintln!("{:?}", eval);

    assert!(!eval.allow_apply_all);
}

#[test]
fn test_build_on_target_without_target_host() {
    let TempHive { hive, _temp_file } = TempHive::new(
        r#"
      {
        test = {
          boot.isContainer = true;
          nixpkgs.system = "x86_64-linux";
          deployment = {
            targetHost = null;
            buildOnTarget = true;
          };
        };
      }
    "#,
    );

    let targets = block_on(hive.select_nodes(None, None, false)).unwrap();
    let deployment = Deployment::new(hive, targets, Goal::Build, None);

    assert!(matches!(
        block_on(deployment.execute()),
        Err(ColmenaError::NoTargetHost)
    ));
}

#[test]
fn test_remote_flags_exclude_machines_file() {
    let mut flags = NixFlags::default();
    flags.add_option("cores".to_string(), "4".to_string());

    let hive = TempHive::with_flags(
        r#"
      {
        meta.machinesFile = "/etc/nix/machines";
      }
    "#,
        flags,
    );

    let with_builders = block_on(hive.nix_flags_with_builders()).unwrap();
    let argv = NixCommand::nix_store(with_builders).into_argv();
    assert!(
        argv.windows(3)
            .any(|w| w == ["--option", "builders", "@/etc/nix/machines"])
    );
    assert!(argv.windows(3).any(|w| w == ["--option", "cores", "4"]));

    let remote = NixCommand::nix_store(hive.base_flags()).into_argv();
    assert_eq!(remote, vec!["nix-store", "--option", "cores", "4"]);
}

#[test]
fn test_user_builders_override_machines_file() {
    let mut flags = NixFlags::default();
    flags.add_option("builders".to_string(), "@/custom/machines".to_string());

    let hive = TempHive::with_flags(
        r#"
      {
        meta.machinesFile = "/etc/nix/machines";
      }
    "#,
        flags,
    );

    let with_builders = block_on(hive.nix_flags_with_builders()).unwrap();
    let argv = NixCommand::nix_store(with_builders).into_argv();
    assert!(
        argv.windows(3)
            .any(|w| w == ["--option", "builders", "@/custom/machines"])
    );
    assert!(!argv.contains(&"@/etc/nix/machines".to_string()));
}
