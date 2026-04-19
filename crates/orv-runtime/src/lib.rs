//! orv-runtime — 레퍼런스 런타임.
//!
//! 현재는 tree-walking 인터프리터(최소)만 제공한다. HIR 기반 정식 실행
//! 경로는 이후 커밋에서 추가된다.

#![warn(missing_docs)]

pub mod interp;
pub mod server;

pub use interp::{
    run, run_handler_with_request, run_with_writer, HandlerOutcome, RequestCtx, ResponseCtx,
    RuntimeError, Value,
};
