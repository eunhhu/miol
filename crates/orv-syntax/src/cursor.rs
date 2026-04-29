//! 소스 커서. UTF-8 바이트 기준 peek/advance.
//!
//! lexer는 문자 단위 판단이 필요하므로 `char` 기반 API를 제공하되,
//! 내부 오프셋은 바이트 단위로 유지한다 (SPEC의 `ByteRange`와 일치).

use std::str::Chars;

/// 소스 문자열 위를 걸어다니는 커서.
#[derive(Clone, Debug)]
pub struct Cursor<'src> {
    source: &'src str,
    chars: Chars<'src>,
    /// 현재 바이트 오프셋 (다음 읽을 문자 시작 위치).
    offset: u32,
}

impl<'src> Cursor<'src> {
    pub fn new(source: &'src str) -> Self {
        Self {
            source,
            chars: source.chars(),
            offset: 0,
        }
    }

    /// 현재 바이트 오프셋 반환.
    pub const fn offset(&self) -> u32 {
        self.offset
    }

    /// 다음 문자 확인 (소비하지 않음).
    pub fn peek(&self) -> Option<char> {
        self.chars.clone().next()
    }

    /// 그 다음 문자 확인 (소비하지 않음).
    pub fn peek2(&self) -> Option<char> {
        let mut it = self.chars.clone();
        it.next();
        it.next()
    }

    /// 다음 문자 소비 후 반환.
    pub fn advance(&mut self) -> Option<char> {
        let c = self.chars.next()?;
        self.offset += u32::try_from(c.len_utf8()).expect("char UTF-8 length fits in u32");
        Some(c)
    }

    /// 주어진 문자와 같으면 소비하고 true 반환.
    pub fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// 술어가 참인 동안 소비.
    pub fn eat_while(&mut self, mut pred: impl FnMut(char) -> bool) {
        while let Some(c) = self.peek() {
            if pred(c) {
                self.advance();
            } else {
                break;
            }
        }
    }

    /// 소스의 슬라이스를 바이트 오프셋으로 추출.
    pub fn slice(&self, start: u32, end: u32) -> &'src str {
        &self.source[start as usize..end as usize]
    }

    /// EOF 도달 여부.
    pub fn is_eof(&self) -> bool {
        self.peek().is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peek_does_not_advance() {
        let c = Cursor::new("abc");
        assert_eq!(c.peek(), Some('a'));
        assert_eq!(c.peek(), Some('a'));
        assert_eq!(c.offset(), 0);
    }

    #[test]
    fn advance_returns_and_moves() {
        let mut c = Cursor::new("ab");
        assert_eq!(c.advance(), Some('a'));
        assert_eq!(c.offset(), 1);
        assert_eq!(c.advance(), Some('b'));
        assert_eq!(c.offset(), 2);
        assert_eq!(c.advance(), None);
    }

    #[test]
    fn multibyte_offset_tracks_utf8_len() {
        let mut c = Cursor::new("한글");
        c.advance();
        assert_eq!(c.offset(), 3); // '한' = 3 bytes UTF-8
        c.advance();
        assert_eq!(c.offset(), 6);
    }

    #[test]
    fn eat_while_consumes_matching() {
        let mut c = Cursor::new("12345abc");
        c.eat_while(|ch| ch.is_ascii_digit());
        assert_eq!(c.offset(), 5);
        assert_eq!(c.peek(), Some('a'));
    }

    #[test]
    fn slice_extracts_bytes() {
        let c = Cursor::new("hello world");
        assert_eq!(c.slice(0, 5), "hello");
        assert_eq!(c.slice(6, 11), "world");
    }

    #[test]
    fn peek_chain_works() {
        let c = Cursor::new("abc");
        assert_eq!(c.peek(), Some('a'));
        assert_eq!(c.peek2(), Some('b'));
    }
}
