//! Cypher string-literal escaping.
//!
//! All Cypher in mycel-graph is built via `format!()` with inline string
//! literals; this helper is the single canonical escape for any String
//! interpolated into a single-quoted Cypher literal. Backticks are
//! pre-defanged at `mycel_core::QualifiedName::new`. Backslashes and
//! single quotes are escaped per Cypher's literal grammar; control
//! characters (newline, carriage return, tab) are converted to their
//! escape-sequence form so a multiline `set-description --description`
//! payload can't terminate the literal early or change query parsing.

pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
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

    #[test]
    fn escapes_newline() {
        assert_eq!(escape("a\nb"), "a\\nb");
    }

    #[test]
    fn escapes_carriage_return_and_tab() {
        assert_eq!(escape("a\r\tb"), "a\\r\\tb");
    }

    #[test]
    fn escapes_combined_control_and_quote() {
        assert_eq!(escape("line1\n'quoted'"), "line1\\n\\'quoted\\'");
    }
}
