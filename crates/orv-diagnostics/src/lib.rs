//! orv-diagnostics — 소스 위치 타입과 구조화된 진단 메시지
//!
//! SPEC.md §0.3 참조. `Span`/`ByteRange`/`FileId`가 여기 있는 이유는 모든
//! 진단이 위치를 필수로 갖기 때문이다. orv-span 크레이트를 별도로 두지
//! 않고 통합한다.

#![warn(missing_docs)]

mod diagnostic;
mod span;

pub use diagnostic::{Diagnostic, Label, Severity};
pub use span::{ByteRange, FileId, Span};
