mod render;

pub use render::{render_diagnostics, render_diagnostics_to_string};

use orv_span::Span;

/// The severity level of a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Help,
    Warning,
    Error,
}

/// A label attached to a source span within a diagnostic.
#[derive(Debug, Clone)]
pub struct Label {
    pub span: Span,
    pub message: String,
    pub is_primary: bool,
}

impl Label {
    /// Creates a primary label at the given span.
    pub fn primary(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
            is_primary: true,
        }
    }

    /// Creates a secondary label at the given span.
    pub fn secondary(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
            is_primary: false,
        }
    }
}

/// A structured compiler diagnostic.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
}

impl Diagnostic {
    /// Creates an error diagnostic with the given message.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Creates a warning diagnostic with the given message.
    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Adds a label to this diagnostic.
    #[must_use]
    pub fn with_label(mut self, label: Label) -> Self {
        self.labels.push(label);
        self
    }

    /// Adds a note to this diagnostic.
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Returns `true` if this diagnostic is an error.
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// A collection of diagnostics emitted during compilation.
#[derive(Debug, Default)]
pub struct DiagnosticBag {
    diagnostics: Vec<Diagnostic>,
}

impl DiagnosticBag {
    /// Creates an empty diagnostic bag.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a diagnostic to the bag.
    pub fn push(&mut self, diag: Diagnostic) {
        self.diagnostics.push(diag);
    }

    /// Returns `true` if any diagnostic is an error.
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(Diagnostic::is_error)
    }

    /// Iterates over all diagnostics.
    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter()
    }

    /// Returns the number of diagnostics.
    pub const fn len(&self) -> usize {
        self.diagnostics.len()
    }

    /// Returns `true` if there are no diagnostics.
    pub const fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }

    /// Consumes the bag and returns the underlying vector.
    pub fn into_vec(self) -> Vec<Diagnostic> {
        self.diagnostics
    }
}

#[cfg(test)]
mod tests {
    use orv_span::{FileId, Span};
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn diagnostic_builder() {
        let file = FileId::new(0);
        let span = Span::new(file, 0, 5);
        let diag = Diagnostic::error("something went wrong")
            .with_label(Label::primary(span, "here"))
            .with_note("try doing X instead");

        assert_eq!(diag.severity, Severity::Error);
        assert_eq!(diag.message, "something went wrong");
        assert_eq!(diag.labels.len(), 1);
        assert!(diag.labels[0].is_primary);
        assert_eq!(diag.labels[0].message, "here");
        assert_eq!(diag.notes, vec!["try doing X instead"]);
        assert!(diag.is_error());
    }

    #[test]
    fn diagnostic_bag_tracks_errors() {
        let mut bag = DiagnosticBag::new();
        assert!(bag.is_empty());

        bag.push(Diagnostic::warning("unused variable"));
        assert!(!bag.has_errors());
        assert_eq!(bag.len(), 1);

        bag.push(Diagnostic::error("type mismatch"));
        assert!(bag.has_errors());
        assert_eq!(bag.len(), 2);
    }

    #[test]
    fn diagnostic_with_multiple_labels() {
        let file = FileId::new(0);
        let primary_span = Span::new(file, 10, 20);
        let secondary_span = Span::new(file, 30, 40);

        let diag = Diagnostic::error("mismatched types")
            .with_label(Label::primary(primary_span, "expected `i32`"))
            .with_label(Label::secondary(secondary_span, "defined here"));

        assert_eq!(diag.labels.len(), 2);
        assert!(diag.labels[0].is_primary);
        assert!(!diag.labels[1].is_primary);
        assert_eq!(diag.labels[0].message, "expected `i32`");
        assert_eq!(diag.labels[1].message, "defined here");
    }

    #[test]
    fn render_snapshot_single_error() {
        use orv_span::SourceMap;

        let mut source_map = SourceMap::default();
        let file = source_map.add("test.orv", "let x = @badnode\nlet y = 2");

        let diag = Diagnostic::error("unknown node `@badnode`")
            .with_label(Label::primary(Span::new(file, 8, 16), "not a valid node"))
            .with_note("valid nodes include @div, @text, @button");

        let output = render_diagnostics_to_string(&source_map, &[diag]);

        assert!(output.contains("error"), "should contain 'error'");
        assert!(
            output.contains("unknown node `@badnode`"),
            "should contain the error message"
        );
        assert!(
            output.contains("not a valid node"),
            "should contain the label text"
        );
        assert!(output.contains("test.orv"), "should contain the filename");
        assert!(
            output.contains("valid nodes include @div, @text, @button"),
            "should contain the note"
        );
    }

    #[test]
    fn render_snapshot_cross_file() {
        use orv_span::SourceMap;

        let mut source_map = SourceMap::default();
        let main_file = source_map.add("main.orv", "import components.Missing");
        let comp_file = source_map.add("components.orv", "pub define Button() -> @button \"ok\"");

        let diag = Diagnostic::error("unresolved import `Missing`")
            .with_label(Label::primary(
                Span::new(main_file, 18, 25),
                "not found in module",
            ))
            .with_label(Label::secondary(
                Span::new(comp_file, 0, 35),
                "module defined here",
            ))
            .with_note("available exports: Button");

        let output = render_diagnostics_to_string(&source_map, &[diag]);

        assert!(
            output.contains("main.orv"),
            "should contain main.orv filename"
        );
        assert!(
            output.contains("components.orv"),
            "should contain components.orv filename"
        );
        assert!(
            output.contains("unresolved import `Missing`"),
            "should contain the error message"
        );
    }
}
