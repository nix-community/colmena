//! Node filters.

use std::collections::HashSet;
use std::convert::AsRef;
use std::iter::{FromIterator, Iterator};
use std::str::FromStr;

use clap::Args;
use glob::Pattern as GlobPattern;

use super::{ColmenaError, ColmenaResult, NodeConfig, NodeName};

#[derive(Debug, Default, Args)]
pub struct NodeFilterOpts {
    /// Node selector
    ///
    /// Select a list of nodes to deploy to. The list is comma-separated and globs are supported.
    /// To match tags, prepend the filter by @. Valid examples:
    ///
    /// - host1,host2,host3
    /// - edge-*
    /// - edge-*,core-*
    /// - @a-tag,@tags-can-have-*
    #[arg(long, value_name = "NODES")]
    pub on: Option<NodeFilter>,
}

/// A node filter containing a list of rules.
#[derive(Clone, Debug)]
pub struct NodeFilter {
    rules: Vec<Rule>,
}

/// A filter rule.
///
/// The filter rules are OR'd together.
#[derive(Debug, Clone, Eq, PartialEq)]
enum Rule {
    /// Matches a node's attribute name.
    MatchName(GlobPattern),

    /// Matches a node's `deployment.tags`.
    MatchTag(GlobPattern),
}

impl FromStr for NodeFilter {
    type Err = ColmenaError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl NodeFilter {
    /// Creates a new filter using an expression passed using `--on`.
    pub fn new<S: AsRef<str>>(filter: S) -> ColmenaResult<Self> {
        let filter = filter.as_ref();
        let trimmed = filter.trim();

        if trimmed.is_empty() {
            tracing::warn!("Filter \"{}\" is blank and will match nothing", filter);

            return Ok(Self { rules: Vec::new() });
        }

        let rules = trimmed
            .split(',')
            .map(|pattern| {
                let pattern = pattern.trim();

                if pattern.is_empty() {
                    return Err(ColmenaError::EmptyFilterRule);
                }

                if let Some(tag_pattern) = pattern.strip_prefix('@') {
                    Ok(Rule::MatchTag(GlobPattern::new(tag_pattern).unwrap()))
                } else {
                    Ok(Rule::MatchName(GlobPattern::new(pattern).unwrap()))
                }
            })
            .collect::<Vec<ColmenaResult<Rule>>>();

        let rules = Result::from_iter(rules)?;

        Ok(Self { rules })
    }

    /// Returns whether the filter has any rule matching NodeConfig information.
    ///
    /// Evaluating `config.deployment` can potentially be very expensive,
    /// especially when its values (e.g., tags) depend on other parts of
    /// the configuration.
    pub fn has_node_config_rules(&self) -> bool {
        self.rules.iter().any(|rule| rule.matches_node_config())
    }

    /// Runs the filter against a set of NodeConfigs and returns the matched ones.
    pub fn filter_node_configs<'a, I>(&self, nodes: I) -> HashSet<NodeName>
    where
        I: Iterator<Item = (&'a NodeName, &'a NodeConfig)>,
    {
        if self.rules.is_empty() {
            return HashSet::new();
        }

        nodes
            .filter_map(|(name, node)| {
                for rule in self.rules.iter() {
                    match rule {
                        Rule::MatchName(pat) => {
                            if pat.matches(name.as_str()) {
                                return Some(name);
                            }
                        }
                        Rule::MatchTag(pat) => {
                            for tag in node.tags() {
                                if pat.matches(tag) {
                                    return Some(name);
                                }
                            }
                        }
                    }
                }

                None
            })
            .cloned()
            .collect()
    }

    /// Runs the filter against a set of node names and returns the matched ones.
    pub fn filter_node_names(&self, nodes: &[NodeName]) -> ColmenaResult<HashSet<NodeName>> {
        nodes.iter().filter_map(|name| -> Option<ColmenaResult<NodeName>> {
            for rule in self.rules.iter() {
                match rule {
                    Rule::MatchName(pat) => {
                        if pat.matches(name.as_str()) {
                            return Some(Ok(name.clone()));
                        }
                    }
                    _ => {
                        return Some(Err(ColmenaError::Unknown {
                            message: format!("Not enough information to run rule {:?} - We only have node names", rule),
                        }));
                    }
                }
            }
            None
        }).collect()
    }
}

impl Rule {
    /// Returns whether the rule matches against the NodeConfig (i.e., `config.deployment`).
    pub fn matches_node_config(&self) -> bool {
        match self {
            Self::MatchTag(_) => true,
            Self::MatchName(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::{HashMap, HashSet};

    macro_rules! node {
        ($n:expr) => {
            NodeName::new($n.to_string()).unwrap()
        };
    }

    #[test]
    fn test_empty_filter() {
        let filter = NodeFilter::new("").unwrap();
        assert_eq!(0, filter.rules.len());

        let filter = NodeFilter::new("\t").unwrap();
        assert_eq!(0, filter.rules.len());

        let filter = NodeFilter::new("    ").unwrap();
        assert_eq!(0, filter.rules.len());
    }

    #[test]
    fn test_empty_filter_rule() {
        assert!(NodeFilter::new(",").is_err());
        assert!(NodeFilter::new("a,,b").is_err());
        assert!(NodeFilter::new("a,b,c,").is_err());
    }

    #[test]
    fn test_filter_rule_mixed() {
        let filter = NodeFilter::new("@router,gamma-*").unwrap();
        assert_eq!(
            vec![
                Rule::MatchTag(GlobPattern::new("router").unwrap()),
                Rule::MatchName(GlobPattern::new("gamma-*").unwrap()),
            ],
            filter.rules,
        );

        let filter = NodeFilter::new("a, \t@b ,    c-*").unwrap();
        assert_eq!(
            vec![
                Rule::MatchName(GlobPattern::new("a").unwrap()),
                Rule::MatchTag(GlobPattern::new("b").unwrap()),
                Rule::MatchName(GlobPattern::new("c-*").unwrap()),
            ],
            filter.rules,
        );
    }

    #[test]
    fn test_filter_node_names() {
        let nodes = vec![node!("lax-alpha"), node!("lax-beta"), node!("sfo-gamma")];

        assert_eq!(
            &HashSet::from_iter([node!("lax-alpha")]),
            &NodeFilter::new("lax-alpha")
                .unwrap()
                .filter_node_names(&nodes)
                .unwrap(),
        );

        assert_eq!(
            &HashSet::from_iter([node!("lax-alpha"), node!("lax-beta")]),
            &NodeFilter::new("lax-*")
                .unwrap()
                .filter_node_names(&nodes)
                .unwrap(),
        );
    }

    #[test]
    fn test_filter_node_configs() {
        // TODO: Better way to mock
        let template = NodeConfig {
            tags: vec![],
            target_host: None,
            target_user: None,
            target_port: None,
            allow_local_deployment: false,
            build_on_target: false,
            no_substitute: false,
            replace_unknown_profiles: false,
            privilege_escalation_command: vec![],
            extra_ssh_options: vec![],
            keys: HashMap::new(),
        };

        let mut nodes = HashMap::new();

        nodes.insert(
            node!("alpha"),
            NodeConfig {
                tags: vec!["web".to_string(), "infra-lax".to_string()],
                ..template.clone()
            },
        );

        nodes.insert(
            node!("beta"),
            NodeConfig {
                tags: vec!["router".to_string(), "infra-sfo".to_string()],
                ..template.clone()
            },
        );

        nodes.insert(
            node!("gamma-a"),
            NodeConfig {
                tags: vec!["controller".to_string()],
                ..template.clone()
            },
        );

        nodes.insert(
            node!("gamma-b"),
            NodeConfig {
                tags: vec!["ewaste".to_string()],
                ..template
            },
        );

        assert_eq!(4, nodes.len());

        assert_eq!(
            &HashSet::from_iter([node!("alpha")]),
            &NodeFilter::new("@web")
                .unwrap()
                .filter_node_configs(nodes.iter()),
        );

        assert_eq!(
            &HashSet::from_iter([node!("alpha"), node!("beta")]),
            &NodeFilter::new("@infra-*")
                .unwrap()
                .filter_node_configs(nodes.iter()),
        );

        assert_eq!(
            &HashSet::from_iter([node!("beta"), node!("gamma-a")]),
            &NodeFilter::new("@router,@controller")
                .unwrap()
                .filter_node_configs(nodes.iter()),
        );

        assert_eq!(
            &HashSet::from_iter([node!("beta"), node!("gamma-a"), node!("gamma-b")]),
            &NodeFilter::new("@router,gamma-*")
                .unwrap()
                .filter_node_configs(nodes.iter()),
        );
    }
}
