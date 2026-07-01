//! Deployment options.

use clap::ValueEnum;

use crate::nix::CopyOptions;

/// Options for a deployment.
#[derive(Clone, Debug)]
pub struct Options {
    /// Whether to use binary caches when copying closures to remote hosts.
    pub(super) substituters_push: Option<bool>,

    /// Whether to use gzip when copying closures to remote hosts.
    pub(super) gzip: bool,

    /// Whether to upload keys when deploying.
    pub(super) upload_keys: bool,

    /// Whether to reboot the hosts after activation.
    pub(super) reboot: bool,

    /// Whether to create GC roots for node profiles.
    ///
    /// If true, .gc_roots will be created under the hive's context
    /// directory if it exists.
    pub(super) create_gc_roots: bool,

    /// Whether to override per-node setting to build on the nodes themselves.
    pub(super) force_build_on_target: Option<bool>,

    /// Ignore the node-level `deployment.replaceUnknownProfiles` option.
    pub(super) force_replace_unknown_profiles: bool,

    /// Which evaluator to use (experimental).
    pub(super) evaluator: EvaluatorType,
}

/// Which evaluator to use.
#[derive(Clone, Debug, PartialEq, Eq, Default, ValueEnum)]
pub enum EvaluatorType {
    #[default]
    Chunked,
    Streaming,
}

impl std::fmt::Display for EvaluatorType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Chunked => "chunked",
            Self::Streaming => "streaming",
        })
    }
}

impl Options {
    pub fn set_substituters_push(&mut self, value: bool) {
        self.substituters_push = Some(value);
    }

    pub fn set_gzip(&mut self, value: bool) {
        self.gzip = value;
    }

    pub fn set_upload_keys(&mut self, enable: bool) {
        self.upload_keys = enable;
    }

    pub fn set_reboot(&mut self, enable: bool) {
        self.reboot = enable;
    }

    pub fn set_create_gc_roots(&mut self, enable: bool) {
        self.create_gc_roots = enable;
    }

    pub fn set_force_build_on_target(&mut self, enable: bool) {
        self.force_build_on_target = Some(enable);
    }

    pub fn set_force_replace_unknown_profiles(&mut self, enable: bool) {
        self.force_replace_unknown_profiles = enable;
    }

    pub fn set_evaluator(&mut self, evaluator: EvaluatorType) {
        self.evaluator = evaluator;
    }

    pub fn to_copy_options(&self) -> CopyOptions {
        let options = CopyOptions::default();
        let options = match self.substituters_push {
            Some(val) => options.use_substitutes(val),
            _ => options,
        };
        options.gzip(self.gzip)
    }
}

impl Default for Options {
    fn default() -> Self {
        Self {
            substituters_push: None,
            gzip: true,
            upload_keys: true,
            reboot: false,
            create_gc_roots: false,
            force_build_on_target: None,
            force_replace_unknown_profiles: false,
            evaluator: EvaluatorType::Chunked,
        }
    }
}
