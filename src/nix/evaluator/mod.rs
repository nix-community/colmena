//! Nix evaluator.
//!
//! A `DrvSetEvaluator` evaluates an attribute set of derivations. Such an
//! implementation may be able to parallelize the evaluation
//! (e.g., with [nix-eval-jobs](https://github.com/nix-community/nix-eval-jobs))
//! and emit results as soon as individual attributes finish evaluating.
//!
//! TODO: Port the chunked evaluator to DrvSetEvaluator

pub mod nix_eval_jobs;
pub use nix_eval_jobs::NixEvalJobs;

use std::convert::TryFrom;
use std::pin::Pin;
use std::result::Result as StdResult;

use futures::Stream;

use super::{BuildResult, NixExpression, NixFlags, StoreDerivation, StorePath};
use crate::error::{ColmenaError, ColmenaResult};
use crate::job::JobHandle;

/// The result of an evaluation.
///
/// The `Ok` variant always correspond to one attribute.
/// The `Err` variant may apply to a single attribute or to the entire
/// evaluation.
pub type EvalResult = StdResult<AttributeOutput, EvalError>;

/// An evaluation error.
#[derive(Debug)]
pub enum EvalError {
    /// An attribute-level error.
    Attribute(AttributeError),

    /// A global error.
    Global(ColmenaError),
}

/// The evaluation output of an attribute.
#[derive(Debug)]
pub struct AttributeOutput {
    attribute: String,
    drv_path: StorePath,
}

/// An attribute-level error.
#[derive(Debug)]
pub struct AttributeError {
    attribute: String,
    error: String,
}

/// A derivation set evaluator.
///
/// Such an evaluator can evaluate an attribute set of derivations.
pub trait DrvSetEvaluator {
    /// Evaluates an attribute set of derivation, returning results as they come in.
    async fn evaluate(
        &self,
        expression: &dyn NixExpression,
        flags: NixFlags,
    ) -> ColmenaResult<Pin<Box<dyn Stream<Item = EvalResult>>>>;

    /// Sets the maximum number of attributes to evaluate at the same time.
    #[allow(unused_variables)]
    fn set_eval_limit(&mut self, limit: usize) {}

    /// Provides a JobHandle to use during operations.
    #[allow(unused_variables)]
    fn set_job(&mut self, job: JobHandle) {}
}

impl AttributeOutput {
    /// Returns the attribute name.
    pub fn attribute(&self) -> &str {
        &self.attribute
    }

    /// Returns the derivation for this attribute.
    pub fn into_derivation<T>(self) -> ColmenaResult<StoreDerivation<T>>
    where
        T: TryFrom<BuildResult<T>>,
    {
        self.drv_path.into_derivation()
    }
}

impl AttributeError {
    /// Returns the attribute name.
    pub fn attribute(&self) -> &str {
        &self.attribute
    }

    /// Returns the error.
    pub fn error(&self) -> &str {
        &self.error
    }
}
