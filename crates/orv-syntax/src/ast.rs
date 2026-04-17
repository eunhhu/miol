//! AST 노드 정의.
//!
//! SPEC §3~§4 기본 구조. 파서 1차 구현에서는 let/const/literal만 커버하고,
//! 이후 커밋에서 함수, 제어 흐름, 도메인 등을 추가한다.

use orv_diagnostics::Span;

/// 프로그램 전체 — 파일 하나의 최상위 스테이트먼트 목록.
#[derive(Clone, Debug)]
pub struct Program {
    /// 최상위 스테이트먼트.
    pub items: Vec<Stmt>,
    /// 전체 소스 범위.
    pub span: Span,
}

/// 스테이트먼트.
#[derive(Clone, Debug)]
pub enum Stmt {
    /// `let` 또는 `let mut` 또는 `let sig` 바인딩.
    Let(Box<LetStmt>),
    /// `const` 상수 선언.
    Const(Box<ConstStmt>),
    /// 표현식 스테이트먼트 (void scope 자동 출력 포함).
    Expr(Expr),
}

impl Stmt {
    /// 스테이트먼트의 소스 범위.
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Let(s) => s.span,
            Self::Const(s) => s.span,
            Self::Expr(e) => e.span,
        }
    }
}

/// `let` 바인딩. `mut`/`sig` 여부는 바인딩 종류로 구분.
#[derive(Clone, Debug)]
pub struct LetStmt {
    /// 바인딩 종류.
    pub kind: LetKind,
    /// 변수 이름.
    pub name: Ident,
    /// 타입 어노테이션 (선택).
    pub ty: Option<TypeRef>,
    /// 초기값 표현식.
    pub init: Expr,
    /// 전체 범위.
    pub span: Span,
}

/// `let` 바인딩의 변형.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LetKind {
    /// `let` — 불변.
    Immutable,
    /// `let mut` — 가변.
    Mutable,
    /// `let sig` — 반응형.
    Signal,
}

/// `const` 선언.
#[derive(Clone, Debug)]
pub struct ConstStmt {
    /// 상수 이름.
    pub name: Ident,
    /// 타입 어노테이션 (선택).
    pub ty: Option<TypeRef>,
    /// 초기값 표현식.
    pub init: Expr,
    /// 전체 범위.
    pub span: Span,
}

/// 식별자 + 스팬.
#[derive(Clone, Debug)]
pub struct Ident {
    /// 이름.
    pub name: String,
    /// 소스 위치.
    pub span: Span,
}

/// 타입 참조 — MVP에서는 식별자와 nullable(`T?`)만 지원.
#[derive(Clone, Debug)]
pub struct TypeRef {
    /// 타입 종류.
    pub kind: TypeRefKind,
    /// 소스 위치.
    pub span: Span,
}

/// 타입 참조 변형.
#[derive(Clone, Debug)]
pub enum TypeRefKind {
    /// 이름 타입 (`int`, `string`, `User` 등).
    Named(Ident),
    /// nullable (`T?`).
    Nullable(Box<TypeRef>),
}

/// 표현식.
#[derive(Clone, Debug)]
pub struct Expr {
    /// 표현식 종류.
    pub kind: ExprKind,
    /// 소스 위치.
    pub span: Span,
}

/// 표현식 변형. MVP는 리터럴과 식별자만.
#[derive(Clone, Debug)]
pub enum ExprKind {
    /// 정수 리터럴 — 원문 슬라이스 보관.
    Integer(String),
    /// 부동소수점 리터럴.
    Float(String),
    /// 문자열 리터럴 (보간/이스케이프 미해석 원문).
    String(String),
    /// `true`.
    True,
    /// `false`.
    False,
    /// `void`.
    Void,
    /// 식별자 참조.
    Ident(Ident),
}
