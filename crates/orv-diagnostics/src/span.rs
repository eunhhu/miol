//! 소스 위치 타입.
//!
//! `Span`은 `FileId` + `ByteRange`의 쌍이다. 모든 AST 노드, 토큰, 진단
//! 메시지가 `Span`을 갖는다. 바이트 오프셋은 UTF-8 기준 0-base 반개구간
//! `[start, end)`이다.

use std::fmt;

/// 소스 파일 식별자.
///
/// 컴파일러가 관리하는 소스 맵에서 파일을 가리키는 인덱스. `u32`로 제한해
/// 메모리 사용을 절반으로 줄인다 (수백만 파일은 비현실적).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FileId(pub u32);

impl FileId {
    /// 소스가 없는 합성 노드용 dummy id.
    pub const DUMMY: Self = Self(u32::MAX);

    /// 내부 정수 값 반환.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0
    }
}

impl fmt::Display for FileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == Self::DUMMY {
            write!(f, "<dummy>")
        } else {
            write!(f, "file#{}", self.0)
        }
    }
}

/// 바이트 오프셋 반개구간.
///
/// 시작 포함, 끝 제외. `start <= end`가 불변식이다.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ByteRange {
    /// 시작 바이트 오프셋 (포함).
    pub start: u32,
    /// 끝 바이트 오프셋 (제외).
    pub end: u32,
}

impl ByteRange {
    /// 두 오프셋으로 새 범위 생성.
    ///
    /// # Panics
    /// `start > end`인 경우 debug 빌드에서 panic.
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Self {
        debug_assert!(start <= end);
        Self { start, end }
    }

    /// 단일 지점(빈 범위) 생성.
    #[must_use]
    pub const fn point(at: u32) -> Self {
        Self { start: at, end: at }
    }

    /// 범위 길이(바이트).
    #[must_use]
    pub const fn len(self) -> u32 {
        self.end - self.start
    }

    /// 빈 범위 여부.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// 두 범위를 포함하는 최소 범위 계산.
    #[must_use]
    pub fn join(self, other: Self) -> Self {
        Self {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

impl fmt::Display for ByteRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

/// 파일 + 바이트 범위 쌍.
///
/// AST 노드와 토큰이 실제로 갖는 위치 타입.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Span {
    /// 소속 파일.
    pub file: FileId,
    /// 파일 내 바이트 범위.
    pub range: ByteRange,
}

impl Span {
    /// 소스 없는 합성 스팬.
    pub const DUMMY: Self = Self {
        file: FileId::DUMMY,
        range: ByteRange { start: 0, end: 0 },
    };

    /// 새 스팬 생성.
    #[must_use]
    pub const fn new(file: FileId, range: ByteRange) -> Self {
        Self { file, range }
    }

    /// 같은 파일 내 두 스팬을 병합. 파일이 다르면 self 반환.
    #[must_use]
    pub fn join(self, other: Self) -> Self {
        if self.file == other.file {
            Self::new(self.file, self.range.join(other.range))
        } else {
            self
        }
    }

    /// 스팬 길이.
    #[must_use]
    pub const fn len(self) -> u32 {
        self.range.len()
    }

    /// 빈 스팬(점) 여부.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.range.is_empty()
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.file, self.range)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_range_basic() {
        let r = ByteRange::new(3, 7);
        assert_eq!(r.len(), 4);
        assert!(!r.is_empty());
    }

    #[test]
    fn byte_range_point() {
        let p = ByteRange::point(5);
        assert!(p.is_empty());
        assert_eq!(p.len(), 0);
    }

    #[test]
    fn byte_range_join() {
        let a = ByteRange::new(0, 3);
        let b = ByteRange::new(5, 10);
        assert_eq!(a.join(b), ByteRange::new(0, 10));
        assert_eq!(b.join(a), ByteRange::new(0, 10));
    }

    #[test]
    fn span_join_same_file() {
        let f = FileId(1);
        let a = Span::new(f, ByteRange::new(0, 3));
        let b = Span::new(f, ByteRange::new(10, 12));
        assert_eq!(a.join(b).range, ByteRange::new(0, 12));
    }

    #[test]
    fn span_join_different_file_returns_self() {
        let a = Span::new(FileId(1), ByteRange::new(0, 3));
        let b = Span::new(FileId(2), ByteRange::new(10, 12));
        assert_eq!(a.join(b), a);
    }

    #[test]
    fn dummy_span_detected() {
        assert_eq!(Span::DUMMY.file, FileId::DUMMY);
        assert!(Span::DUMMY.is_empty());
    }
}
