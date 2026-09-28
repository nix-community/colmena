//! Integration-ish tests

use super::*;

use std::collections::HashSet;
use std::fs;
use std::hash::Hash;
use std::io::Write;
use std::iter::{FromIterator, Iterator};
use std::ops::Deref;
use std::path::PathBuf;

use tempfile::{Builder as TempFileBuilder, NamedTempFile};
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
        let hive = block_on(Hive::new(hive_path, flags)).unwrap();

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

#[test]
fn test_parse_makehive_flake() {
    // make a copy of the flake so we can edit the colmena input
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

    // run the test
    let flake = block_on(Flake::from_dir(flake_dir.as_ref(), &NixFlags::default())).unwrap();

    let mut flags = NixFlags::default();
    flags.set_show_trace(true);

    let hive_path = HivePath::Flake(flake);
    let mut hive = block_on(Hive::new(hive_path, flags)).unwrap();

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

    // nix-eval-jobs --expr <expr>
    {
        hive.set_evaluation_method(EvaluationMethod::NixInstantiate);
        assert!(
            hive.eval_selected_expr(&[node!("host-a")])
                .unwrap()
                .installable()
                .is_none()
        );
    }

    drop(flake_dir);
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
