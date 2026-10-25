use std::fmt;

use super::span::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Help,
    Note,
    Warning,
    Error,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Help => "help",
            Severity::Note => "note",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A message attached to a source range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub span: Span,
    pub message: String,
    /// The primary label is the "main" location (`^^^`); others are context (`---`).
    pub primary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Stable identifier such as `E0102`, if the producer has one.
    pub code: Option<&'static str>,
    pub message: String,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
    pub helps: Vec<String>,
}

impl Diagnostic {
    pub fn new(severity: Severity, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            severity,
            code: None,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
            helps: Vec::new(),
        }
    }

    pub fn error(message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(Severity::Error, message)
    }

    pub fn warning(message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(Severity::Warning, message)
    }

    pub fn with_code(mut self, code: &'static str) -> Diagnostic {
        self.code = Some(code);
        self
    }

    /// Adds a primary label.
    pub fn with_label(mut self, span: Span, message: impl Into<String>) -> Diagnostic {
        self.labels.push(Label { span, message: message.into(), primary: true });
        self
    }

    /// Adds a secondary (context) label.
    pub fn with_secondary(mut self, span: Span, message: impl Into<String>) -> Diagnostic {
        self.labels.push(Label { span, message: message.into(), primary: false });
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Diagnostic {
        self.notes.push(note.into());
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Diagnostic {
        self.helps.push(help.into());
        self
    }

    /// Span of the first primary label.
    pub fn primary_span(&self) -> Option<Span> {
        self.labels.iter().find(|l| l.primary).map(|l| l.span)
    }
}

/// Collects diagnostics instead of aborting on the first one, so a single
/// run can report every problem (and tooling can keep a partial result).
#[derive(Debug, Clone, Default)]
pub struct DiagBag {
    items: Vec<Diagnostic>,
    errors: usize,
    warnings: usize,
}

impl DiagBag {
    pub fn new() -> DiagBag {
        DiagBag::default()
    }

    pub fn report(&mut self, d: Diagnostic) {
        match d.severity {
            Severity::Error => self.errors += 1,
            Severity::Warning => self.warnings += 1,
            Severity::Note | Severity::Help => {}
        }
        self.items.push(d);
    }

    /// Shorthand for an error with a single primary label.
    pub fn error(&mut self, span: Span, message: impl Into<String>) {
        let message = message.into();
        self.report(Diagnostic::error(message.clone()).with_label(span, message));
    }

    /// Shorthand for a warning with a single primary label.
    pub fn warning(&mut self, span: Span, message: impl Into<String>) {
        let message = message.into();
        self.report(Diagnostic::warning(message.clone()).with_label(span, message));
    }

    pub fn has_errors(&self) -> bool {
        self.errors > 0
    }

    pub fn error_count(&self) -> usize {
        self.errors
    }

    pub fn warning_count(&self) -> usize {
        self.warnings
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Diagnostic> {
        self.items.iter()
    }

    pub fn as_slice(&self) -> &[Diagnostic] {
        &self.items
    }

    /// Moves every diagnostic of `other` into this bag.
    pub fn extend(&mut self, other: DiagBag) {
        self.errors += other.errors;
        self.warnings += other.warnings;
        self.items.extend(other.items);
    }

    pub fn clear(&mut self) {
        *self = DiagBag::default();
    }

    pub fn into_vec(self) -> Vec<Diagnostic> {
        self.items
    }
}

impl<'a> IntoIterator for &'a DiagBag {
    type Item = &'a Diagnostic;
    type IntoIter = std::slice::Iter<'a, Diagnostic>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::span::FileId;

    fn sp(lo: u32, hi: u32) -> Span {
        Span::new(FileId(0), lo, hi)
    }

    #[test]
    fn severity_orders_by_gravity() {
        assert!(Severity::Error > Severity::Warning);
        assert!(Severity::Warning > Severity::Note);
        assert!(Severity::Note > Severity::Help);
        assert_eq!(Severity::Warning.to_string(), "warning");
    }

    #[test]
    fn builder_collects_parts() {
        let d = Diagnostic::error("bad")
            .with_code("E0001")
            .with_secondary(sp(0, 1), "context")
            .with_label(sp(4, 6), "here")
            .with_note("n")
            .with_help("h");
        assert_eq!(d.code, Some("E0001"));
        assert_eq!(d.labels.len(), 2);
        assert_eq!(d.primary_span(), Some(sp(4, 6)));
        assert_eq!((d.notes.len(), d.helps.len()), (1, 1));
        assert_eq!(Diagnostic::warning("w").primary_span(), None);
    }

    #[test]
    fn bag_counts_by_severity() {
        let mut bag = DiagBag::new();
        assert!(bag.is_empty() && !bag.has_errors());
        bag.error(sp(0, 1), "e1");
        bag.warning(sp(1, 2), "w1");
        bag.report(Diagnostic::new(Severity::Note, "n"));
        bag.error(sp(2, 3), "e2");
        assert!(bag.has_errors());
        assert_eq!((bag.error_count(), bag.warning_count(), bag.len()), (2, 1, 4));
        assert_eq!(bag.iter().count(), 4);
        let msgs: Vec<_> = (&bag).into_iter().map(|d| d.message.as_str()).collect();
        assert_eq!(msgs, ["e1", "w1", "n", "e2"]);
    }

    #[test]
    fn warnings_alone_are_not_errors() {
        let mut bag = DiagBag::new();
        bag.warning(sp(0, 1), "w");
        assert!(!bag.has_errors());
    }

    #[test]
    fn extend_merges_counts_and_order() {
        let mut a = DiagBag::new();
        a.error(sp(0, 1), "a");
        let mut b = DiagBag::new();
        b.warning(sp(0, 1), "b");
        b.error(sp(0, 1), "c");
        a.extend(b);
        assert_eq!((a.error_count(), a.warning_count()), (2, 1));
        let msgs: Vec<_> = a.as_slice().iter().map(|d| d.message.as_str()).collect();
        assert_eq!(msgs, ["a", "b", "c"]);
        a.clear();
        assert!(a.is_empty() && !a.has_errors());
    }

    #[test]
    fn shorthand_attaches_label() {
        let mut bag = DiagBag::new();
        bag.error(sp(3, 5), "oops");
        let d = &bag.as_slice()[0];
        assert_eq!(d.labels[0].message, "oops");
        assert!(d.labels[0].primary);
        assert_eq!(bag.into_vec().len(), 1);
    }
}
