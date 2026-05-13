//! orv-syntax — 렉서와 파서.
//!
//! SPEC.md §2 어휘 구조부터 선언, 표현식, 문장, 함수, 제어 흐름, 도메인 호출까지
//! 현재 AST가 담는 ORV source 문법을 처리한다.

#![warn(missing_docs)]

pub mod ast;
mod cursor;
mod lexer;
mod parser;
mod token;

pub use lexer::{lex, LexResult};
pub use parser::{parse, parse_with_newlines, ParseResult};
pub use token::{Keyword, Token, TokenKind};
