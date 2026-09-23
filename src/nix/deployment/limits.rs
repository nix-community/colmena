//! Parallelism limits.

use std::str::FromStr;

use tokio::sync::Semaphore;

/// Amount of RAM reserved for the system, in MB.
const EVAL_RESERVE_MB: u64 = 1024;

/// Estimated amount of RAM needed to evaluate one host, in MB.
const EVAL_PER_HOST_MB: u64 = 512;

/// The parallelism limit for a deployment.
#[derive(Debug)]
pub struct ParallelismLimit {
    /// Limit of concurrent evaluation processes.
    pub evaluation: Semaphore,

    /// Limit of concurrent apply processes.
    pub apply: Semaphore,
}

impl Default for ParallelismLimit {
    fn default() -> Self {
        Self {
            evaluation: Semaphore::new(1),
            apply: Semaphore::new(10),
        }
    }
}

impl ParallelismLimit {
    /// Sets the concurrent apply limit.
    pub fn set_apply_limit(&mut self, limit: usize) {
        self.apply = Semaphore::new(limit);
    }
}

/// Limit of the number of nodes in each evaluation process.
///
/// The evaluation process is very RAM-intensive, with memory
/// consumption scaling linearly with the number of nodes
/// evaluated at the same time. This can be a problem if you
/// are deploying to a large number of nodes at the same time,
/// where `nix-instantiate` may consume too much RAM and get
/// killed by the OS (`ChildKilled` error).
///
/// Evaluating each node on its own is not an efficient solution,
/// with total CPU time and memory consumption vastly exceeding the
/// case where we evaluate the same set of nodes at the same time
/// (TODO: Provide statistics).
///
/// To overcome this problem, we split the evaluation process into
/// chunks when necessary, with the maximum number of nodes in
/// each `nix-instantiate` invocation determined with:
///
/// - A simple heuristic based on remaining memory in the system
/// - A supplied number
/// - No limit at all
#[derive(Copy, Clone, Debug, Default)]
pub enum EvaluationNodeLimit {
    /// Use a naive heuristic based on available memory.
    #[default]
    Heuristic,

    /// Supply the maximum number of nodes.
    Manual(usize),

    /// Do not limit the number of nodes in each evaluation process
    None,
}

impl FromStr for EvaluationNodeLimit {
    type Err = &'static str;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "auto" {
            return Ok(EvaluationNodeLimit::Heuristic);
        }

        match s.parse::<usize>() {
            Ok(0) => Ok(EvaluationNodeLimit::None),
            Ok(n) => Ok(EvaluationNodeLimit::Manual(n)),
            Err(_) => Err("The value must be a valid number or `auto`"),
        }
    }
}

impl std::fmt::Display for EvaluationNodeLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Heuristic => write!(f, "auto"),
            Self::None => write!(f, "0"),
            Self::Manual(n) => write!(f, "{n}"),
        }
    }
}

impl EvaluationNodeLimit {
    /// Returns the maximum number of hosts in each evaluation of `n_nodes` hosts.
    ///
    /// The result should be cached.
    pub fn get_limit(&self, n_nodes: usize) -> usize {
        match self {
            EvaluationNodeLimit::Heuristic => {
                if let Ok(mem_info) = sys_info::mem_info() {
                    let mut mb = mem_info.avail / 1024;

                    if mb >= EVAL_RESERVE_MB {
                        mb -= EVAL_RESERVE_MB;
                    }

                    let nodes = mb / EVAL_PER_HOST_MB;

                    if nodes == 0 { 1 } else { nodes as usize }
                } else {
                    10
                }
            }
            EvaluationNodeLimit::Manual(limit) => *limit,
            // nix-eval-jobs rejects zero workers and chunks(0) panics
            EvaluationNodeLimit::None => n_nodes.max(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_limit() {
        assert_eq!(19, EvaluationNodeLimit::None.get_limit(19));
        assert_eq!(1, EvaluationNodeLimit::None.get_limit(0));
        assert_eq!(4, EvaluationNodeLimit::Manual(4).get_limit(19));
    }
}
