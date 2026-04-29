//! 토큰 정의.
//!
//! SPEC.md §2 어휘 구조 전체를 커버한다.
//! - 원시 타입 리터럴(정수/부동/문자열/불리언/void)
//! - 예약 키워드 (SPEC §2.3)
//! - 연산자 및 구분자 (SPEC §2.5)
//! - 식별자, 도메인 호출(`@ident`), 정규식 리터럴(`r"..."flags`)

use orv_diagnostics::Span;

/// 한 개의 토큰.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Token {
    /// 토큰 종류.
    pub kind: TokenKind,
    /// 소스 위치.
    pub span: Span,
}

impl Token {
    /// 새 토큰 생성.
    #[must_use]
    pub const fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span }
    }
}

/// 토큰 종류. 페이로드는 문자열 리터럴/정규식처럼 값을 동반해야 하는 경우에만 가진다.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenKind {
    // ── 리터럴 ──
    /// 정수 리터럴 — 10진수만 지원 (MVP). 값은 소스 슬라이스로 재파싱한다.
    Integer(String),
    /// 부동소수점 리터럴. `3.14` 형태, 지수부 없음.
    Float(String),
    /// 문자열 리터럴 본문(따옴표 제외). 보간/이스케이프 처리는 파서 단계에서.
    String(String),
    /// 정규식 리터럴 — 본문 + 플래그.
    Regex {
        /// 정규식 본문(따옴표 사이).
        pattern: String,
        /// 플래그 문자열 (예: `"gi"`).
        flags: String,
    },
    /// `true`.
    True,
    /// `false`.
    False,

    // ── 식별자/키워드 ──
    /// 일반 식별자 (영문/숫자/`_`).
    Ident(String),
    /// `@ident` 형태의 도메인 호출/지시어.
    At(String),
    /// 예약 키워드.
    Keyword(Keyword),

    // ── 구분자 ──
    /// `{`
    LBrace,
    /// `}`
    RBrace,
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `[`
    LBracket,
    /// `]`
    RBracket,
    /// `,`
    Comma,
    /// `;`
    Semicolon,
    /// `:`
    Colon,
    /// `.`
    Dot,
    /// `..`
    DotDot,
    /// `..=`
    DotDotEq,
    /// `...` — spread / rest
    DotDotDot,
    /// `->` — 함수/도메인 본문 시작
    Arrow,
    /// `?`
    Question,

    // ── 산술 ──
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/`
    Slash,
    /// `%`
    Percent,
    /// `**`
    StarStar,

    // ── 비교 ──
    /// `==`
    EqEq,
    /// `!=`
    BangEq,
    /// `<`
    Lt,
    /// `>`
    Gt,
    /// `<=`
    LtEq,
    /// `>=`
    GtEq,

    // ── 논리 ──
    /// `&&`
    AmpAmp,
    /// `||`
    PipePipe,
    /// `!`
    Bang,

    // ── 비트 ──
    /// `&`
    Amp,
    /// `|`
    Pipe,
    /// `^`
    Caret,
    /// `~`
    Tilde,
    /// `<<`
    LtLt,
    /// `>>`
    GtGt,

    // ── 대입 ──
    /// `=`
    Eq,
    /// `+=`
    PlusEq,
    /// `-=`
    MinusEq,

    // ── 기타 ──
    /// `??` — 널 병합
    QuestionQuestion,
    /// `$` — where 절과 when 분기의 현재 값 참조 (SPEC §4.10, §6.3)
    Dollar,

    // ── 종료 ──
    /// 파일 끝.
    Eof,
}

/// SPEC §2.3 예약 키워드.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[allow(missing_docs)]
pub enum Keyword {
    Let,
    Mut,
    Sig,
    Const,
    Function,
    Async,
    Await,
    Return,
    If,
    Else,
    When,
    For,
    In,
    While,
    Break,
    Continue,
    Try,
    Catch,
    Throw,
    Struct,
    Enum,
    Type,
    Define,
    Pub,
    Import,
    Void,
    As,
}

impl Keyword {
    /// 키워드 문자열을 매칭한다. 존재하지 않으면 `None`.
    ///
    /// `FromStr` trait 대신 inherent method 를 쓰는 이유: 렉서에서만 호출되고
    /// 에러 타입이 필요 없어 `Option<Self>` 로 충분하다. trait 구현은
    /// 오버헤드만 늘어난다.
    #[must_use]
    pub fn match_keyword(s: &str) -> Option<Self> {
        Some(match s {
            "let" => Self::Let,
            "mut" => Self::Mut,
            "sig" => Self::Sig,
            "const" => Self::Const,
            "function" => Self::Function,
            "async" => Self::Async,
            "await" => Self::Await,
            "return" => Self::Return,
            "if" => Self::If,
            "else" => Self::Else,
            "when" => Self::When,
            "for" => Self::For,
            "in" => Self::In,
            "while" => Self::While,
            "break" => Self::Break,
            "continue" => Self::Continue,
            "try" => Self::Try,
            "catch" => Self::Catch,
            "throw" => Self::Throw,
            "struct" => Self::Struct,
            "enum" => Self::Enum,
            "type" => Self::Type,
            "define" => Self::Define,
            "pub" => Self::Pub,
            "import" => Self::Import,
            "void" => Self::Void,
            "as" => Self::As,
            _ => return None,
        })
    }
}
