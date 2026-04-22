//! orv-syntax — 렉서와 파서.
//!
//! SPEC.md §2 어휘 구조부터 §9까지 구문을 처리한다. 파서는 현재 `let`/
//! `const`/리터럴/식별자만 다루며, 함수·제어 흐름·도메인은 이후 커밋에서
//! 추가된다.

#![warn(missing_docs)]

pub mod ast;
mod cursor;
mod lexer;
mod parser;
mod token;

pub use lexer::{lex, LexResult};
pub use parser::{parse, parse_with_newlines, ParseResult};
pub use token::{Keyword, Token, TokenKind};
