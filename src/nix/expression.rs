//! Nix expression serializer.

use serde::Serialize;

/// A Nix expression.
pub trait NixExpression: Send + Sync {
    /// When [`Self::installable`] returns `None`,
    /// returns the full Nix expression to be evaluated (`nix-eval-jobs --expr`),
    /// o.w. return function applied to flake eval root (`nix-eval-jobs --select`).
    fn expression(&self) -> String;

    /// Returns the full flake `colmenaHive` accessor or `None`.
    fn installable(&self) -> Option<String> {
        None
    }

    /// Returns whether this expression requires the use of flakes.
    fn requires_flakes(&self) -> bool {
        false
    }
}

/// A serialized Nix expression.
pub struct SerializedNixExpression(String);

impl NixExpression for String {
    fn expression(&self) -> String {
        self.clone()
    }
}

impl SerializedNixExpression {
    pub fn new<T>(data: T) -> Self
    where
        T: Serialize,
    {
        let json = serde_json::to_string(&data).expect("Could not serialize data");
        let quoted = nix_quote(&json);

        Self(quoted)
    }
}

impl NixExpression for SerializedNixExpression {
    fn expression(&self) -> String {
        format!("(builtins.fromJSON {})", &self.0)
    }
}

/// Turns a string into a quoted Nix string expression.
fn nix_quote(s: &str) -> String {
    let inner = s
        .replace('\\', r#"\\"#)
        .replace('"', r#"\""#)
        .replace("${", r#"\${"#);

    format!("\"{}\"", inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nix_quote() {
        let cases = [
            (r#"["a", "b"]"#, r#""[\"a\", \"b\"]""#),
            (
                r#"["\"a\"", "\"b\""]"#,
                r#""[\"\\\"a\\\"\", \"\\\"b\\\"\"]""#,
            ),
            (r#"${dontExpandMe}"#, r#""\${dontExpandMe}""#),
            (r#"\${dontExpandMe}"#, r#""\\\${dontExpandMe}""#),
        ];

        for (orig, quoted) in cases {
            assert_eq!(quoted, nix_quote(orig));
        }
    }
}
