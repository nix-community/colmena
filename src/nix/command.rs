//! Builder for Nix invocations.
//!
//! It renders one set of [`NixFlags`] per executable and omits the flags
//! that executable rejects.

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use tokio::process::Command;

use super::NixFlags;

/// A Nix executable.
#[derive(Debug, Clone)]
enum NixExe {
    /// The nix3 CLI (`nix eval`, `nix copy`, `nix flake`, `nix repl`, ...).
    Nix,
    NixInstantiate,
    NixStore,
    NixEnv,
    NixCopyClosure,
    /// `nix-eval-jobs` at the given path.
    NixEvalJobs(PathBuf),
}

/// The flags a Nix executable accepts beyond the settings flags, which
/// every executable accepts.
struct Caps {
    /// Whether `--impure` is accepted.
    ///
    /// `--impure` is an evaluator flag. `nix-store` rejects it and
    /// `nix-copy-closure` reads it as the host argument. `nix-env --set`
    /// evaluates nothing, so it is omitted there too.
    impure: bool,

    /// Experimental features to enable by default via
    /// `--extra-experimental-features`.
    features: &'static [&'static str],
}

impl NixExe {
    fn executable(&self) -> Cow<'static, OsStr> {
        match self {
            Self::Nix => OsStr::new("nix").into(),
            Self::NixInstantiate => OsStr::new("nix-instantiate").into(),
            Self::NixStore => OsStr::new("nix-store").into(),
            Self::NixEnv => OsStr::new("nix-env").into(),
            Self::NixCopyClosure => OsStr::new("nix-copy-closure").into(),
            Self::NixEvalJobs(path) => path.clone().into_os_string().into(),
        }
    }

    fn caps(&self) -> Caps {
        match self {
            Self::Nix => Caps {
                impure: true,
                features: &["nix-command", "flakes"],
            },
            // only nix-eval-jobs --flake needs flakes, its call site enables it per hive
            Self::NixInstantiate | Self::NixEvalJobs(_) => Caps {
                impure: true,
                features: &[],
            },
            Self::NixStore | Self::NixEnv | Self::NixCopyClosure => Caps {
                impure: false,
                features: &[],
            },
        }
    }
}

/// A builder for a Nix command invocation.
///
/// The builder holds a shared set of [`NixFlags`] and only emits the
/// flags the target executable accepts, so the same flags can be
/// threaded into every Nix invocation regardless of CLI dialect.
#[derive(Debug, Clone)]
#[must_use]
pub struct NixCommand {
    exe: NixExe,
    flags: NixFlags,
    extra_features: Vec<&'static str>,
    args: Vec<OsString>,
}

impl NixCommand {
    /// Creates an invocation of the nix3 CLI (`nix`).
    pub fn nix(flags: NixFlags) -> Self {
        Self::new(NixExe::Nix, flags)
    }

    /// Creates an invocation of `nix-instantiate`.
    pub fn nix_instantiate(flags: NixFlags) -> Self {
        Self::new(NixExe::NixInstantiate, flags)
    }

    /// Creates an invocation of `nix-store`.
    pub fn nix_store(flags: NixFlags) -> Self {
        Self::new(NixExe::NixStore, flags)
    }

    /// Creates an invocation of `nix-env`.
    pub fn nix_env(flags: NixFlags) -> Self {
        Self::new(NixExe::NixEnv, flags)
    }

    /// Creates an invocation of `nix-copy-closure`.
    pub fn nix_copy_closure(flags: NixFlags) -> Self {
        Self::new(NixExe::NixCopyClosure, flags)
    }

    /// Creates an invocation of `nix-eval-jobs` at the given path.
    pub fn nix_eval_jobs(executable: PathBuf, flags: NixFlags) -> Self {
        Self::new(NixExe::NixEvalJobs(executable), flags)
    }

    fn new(exe: NixExe, flags: NixFlags) -> Self {
        Self {
            exe,
            flags,
            extra_features: Vec::new(),
            args: Vec::new(),
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I>(mut self, args: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Enables additional experimental features.
    pub fn extra_features(mut self, features: &[&'static str]) -> Self {
        self.extra_features.extend_from_slice(features);
        self
    }

    /// Builds a [`Command`] ready to be spawned locally.
    pub fn build(self) -> Command {
        let mut command = Command::new(self.exe.executable());
        command.args(&self.args);
        command.args(self.render_flags());
        command
    }

    /// Returns the full argv, for wrapping in another command such as ssh or sudo.
    ///
    /// All arguments must be valid UTF-8. The argv is unquoted:
    /// transports that pass through a shell (like ssh) must escape
    /// each element.
    pub fn into_argv(self) -> Vec<String> {
        let flags = self.render_flags();

        let executable = self
            .exe
            .executable()
            .into_owned()
            .into_string()
            .expect("Executable path must be valid UTF-8");

        let mut argv = vec![executable];
        argv.extend(
            self.args
                .into_iter()
                .map(|arg| arg.into_string().expect("Arguments must be valid UTF-8")),
        );
        argv.extend(flags);
        argv
    }

    /// Renders the [`NixFlags`] the executable accepts, followed by
    /// the experimental features.
    fn render_flags(&self) -> Vec<String> {
        let caps = self.exe.caps();
        let mut out = Vec::new();

        let flags = &self.flags;

        if flags.show_trace {
            out.push("--show-trace".to_string());
        }

        if flags.pure_eval {
            out.push("--pure-eval".to_string());
        }

        if caps.impure && flags.impure {
            out.push("--impure".to_string());
        }

        for (name, value) in flags.options.iter() {
            out.push("--option".to_string());
            out.push(name.to_string());
            out.push(value.to_string());
        }

        // --option experimental-features replaces the setting
        // --extra-experimental-features appends to it
        // nix applies flags left to right, so the features go last
        let mut features: Vec<&str> = caps.features.to_vec();
        features.extend(&self.extra_features);
        if !features.is_empty() {
            out.push("--extra-experimental-features".to_string());
            out.push(features.join(" "));
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_flags() -> NixFlags {
        let mut flags = NixFlags::default();
        flags.set_show_trace(true);
        flags.set_impure(true);
        flags.add_option("cores".to_string(), "4".to_string());
        flags.add_option(
            "substituters".to_string(),
            "https://a https://b".to_string(),
        );
        flags
    }

    #[test]
    fn test_nix_receives_all_flags() {
        let argv = NixCommand::nix(test_flags()).arg("eval").into_argv();

        assert_eq!(
            argv,
            vec![
                "nix",
                "eval",
                "--show-trace",
                "--impure",
                "--option",
                "cores",
                "4",
                "--option",
                "substituters",
                "https://a https://b",
                "--extra-experimental-features",
                "nix-command flakes",
            ]
        );
    }

    #[test]
    fn test_features_survive_user_experimental_features() {
        let mut flags = NixFlags::default();
        flags.add_option(
            "experimental-features".to_string(),
            "ca-derivations".to_string(),
        );

        let argv = NixCommand::nix(flags).into_argv();

        assert_eq!(
            argv,
            vec![
                "nix",
                "--option",
                "experimental-features",
                "ca-derivations",
                "--extra-experimental-features",
                "nix-command flakes",
            ]
        );
    }

    #[test]
    fn test_legacy_binaries_drop_impure() {
        let commands = [
            NixCommand::nix_store(test_flags()),
            NixCommand::nix_env(test_flags()),
            NixCommand::nix_copy_closure(test_flags()),
        ];

        for command in commands {
            let argv = command.into_argv();

            assert!(!argv.contains(&"--impure".to_string()));
            assert!(argv.contains(&"--show-trace".to_string()));
            assert!(argv.windows(3).any(|w| w == ["--option", "cores", "4"]));
        }
    }

    #[test]
    fn test_builders_option() {
        let mut flags = NixFlags::default();
        flags.add_option("builders".to_string(), "@/path/to/machines".to_string());

        let argv = NixCommand::nix_store(flags).into_argv();

        assert_eq!(
            argv,
            vec!["nix-store", "--option", "builders", "@/path/to/machines"]
        );
    }

    #[test]
    fn test_pure_eval() {
        let mut flags = NixFlags::default();
        flags.set_pure_eval(true);

        let argv = NixCommand::nix_store(flags).into_argv();
        assert_eq!(argv, vec!["nix-store", "--pure-eval"]);
    }
}
