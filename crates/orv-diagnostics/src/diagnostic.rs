//! 진단 메시지 구조.
//!
//! 렉서/파서/리졸버/애널라이저 모든 단계가 동일한 `Diagnostic`을 쌓는다.
//! 출력 포매팅은 `codespan-reporting`에 위임한다.

use crate::span::Span;

/// 진단의 심각도.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Severity {
    /// 치명적 에러 — 컴파일 중단.
    Error,
    /// 경고 — 빌드는 성공하나 주의.
    Warning,
    /// 정보 — 참고용 힌트.
    Note,
    /// 도움말 — 제안하는 수정 방향.
    Help,
}

/// 소스 위치에 붙는 라벨.
#[derive(Clone, Debug)]
pub struct Label {
    /// 라벨이 가리키는 소스 위치.
    pub span: Span,
    /// 라벨 본문.
    pub message: String,
}

impl Label {
    /// 새 라벨 생성.
    #[must_use]
    pub fn new(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
        }
    }
}

/// 하나의 진단 메시지.
///
/// 주 라벨(primary)과 보조 라벨들(secondary)을 분리해 주된 위치를 강조한다.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    /// 심각도.
    pub severity: Severity,
    /// 헤더 메시지.
    pub message: String,
    /// 에러 코드 (예: `E0001`).
    pub code: Option<String>,
    /// 메인 위치 라벨.
    pub primary: Option<Label>,
    /// 보조 라벨들.
    pub secondary: Vec<Label>,
    /// 추가 힌트.
    pub notes: Vec<String>,
}

impl Diagnostic {
    /// 심각도와 메시지로 생성. 라벨/노트는 builder로 붙인다.
    #[must_use]
    pub fn new(severity: Severity, message: impl Into<String>) -> Self {
        Self {
            severity,
            message: message.into(),
            code: None,
            primary: None,
            secondary: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// 에러 심각도.
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self::new(Severity::Error, message)
    }

    /// 경고 심각도.
    #[must_use]
    pub fn warning(message: impl Into<String>) -> Self {
        Self::new(Severity::Warning, message)
    }

    /// 에러 코드 설정.
    #[must_use]
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    /// 주 라벨 설정.
    #[must_use]
    pub fn with_primary(mut self, span: Span, message: impl Into<String>) -> Self {
        self.primary = Some(Label::new(span, message));
        self
    }

    /// 보조 라벨 추가.
    #[must_use]
    pub fn with_secondary(mut self, span: Span, message: impl Into<String>) -> Self {
        self.secondary.push(Label::new(span, message));
        self
    }

    /// 힌트 추가.
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::{ByteRange, FileId};

    #[test]
    fn diagnostic_builder() {
        let span = Span::new(FileId(0), ByteRange::new(10, 15));
        let d = Diagnostic::error("unexpected token")
            .with_code("E0001")
            .with_primary(span, "here")
            .with_note("try inserting a semicolon");

        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.code.as_deref(), Some("E0001"));
        assert_eq!(d.primary.as_ref().unwrap().message, "here");
        assert_eq!(d.notes, vec!["try inserting a semicolon".to_string()]);
    }

    #[test]
    fn warning_has_warning_severity() {
        let d = Diagnostic::warning("deprecated");
        assert_eq!(d.severity, Severity::Warning);
    }
}
