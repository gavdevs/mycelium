//! Cypher string-literal escaping.
//!
//! All Cypher in mycel-graph is built via `format!()` with inline string
//! literals; this helper is the single canonical escape for any String
//! interpolated into a single-quoted Cypher literal. Backticks are
//! pre-defanged at `mycel_core::QualifiedName::new`, so this only handles
//! backslashes and single quotes.

pub(crate) fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_single_quote() {
        assert_eq!(escape("a'b"), "a\\'b");
    }

    #[test]
    fn escapes_backslash_first() {
        // Backslash MUST be escaped first; otherwise the backslash inserted
        // by the quote-escape pass would be re-doubled by a later run.
        assert_eq!(escape("a\\b"), "a\\\\b");
    }

    #[test]
    fn handles_combined() {
        assert_eq!(escape("a\\'b"), "a\\\\\\'b");
    }

    #[test]
    fn passthrough_safe_string() {
        assert_eq!(escape("crate::foo::Bar"), "crate::foo::Bar");
    }
}
