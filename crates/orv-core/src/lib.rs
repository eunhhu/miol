pub mod source;

pub use orv_diagnostics as diagnostics;
pub use orv_macros::orv;
pub use orv_span as span;

pub const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        assert_eq!(version(), "0.1.0");
    }

    #[test]
    fn test_orv_macro() {
        orv! {
            hello world
        };
    }

    #[test]
    fn test_span_reexport() {
        let id = span::FileId::new(0);
        let s = span::Span::new(id, 0, 5);
        assert_eq!(s.len(), 5);
    }

    #[test]
    fn test_diagnostics_reexport() {
        let diag = diagnostics::Diagnostic::error("test");
        assert!(diag.is_error());
    }
}
