//! 고수준 중간 표현 (HIR).
//!
//! # 역할
//! AST 를 구조적으로 미러링한 컴파일러 친화 표현. 차이점은 두 가지:
//! 1. 스코프 바인딩 사이트/참조가 [`HirIdent`] 로 교체되어 유일한
//!    [`NameId`] 를 들고 다닌다.
//! 2. 모든 [`HirExpr`] 는 [`Type`] 슬롯을 달고 있으며, 초기 단계에서는
//!    [`Type::Unknown`] 으로 채워진다.
//!
//! # 이번 커밋 범위
//! 타입 정의만. lowering (`AST → HIR`) 은 커밋 24, domain 분해는 커밋 25,
//! 타입 체크는 이후 커밋에서 추가된다. 지금은 소비자가 없으므로 단위
//! 테스트도 두지 않는다 — `cargo build` 통과가 곧 검증이다.
//!
//! # 설계 노트
//! - `Field.field`, `ObjectField.name`, `HirDomain.name` 은 스코프 바인딩이
//!   아니므로 `String + Span` 을 유지한다.
//! - `Domain` 의 인자는 `Vec<HirExpr>` 로 유지한다. `@route` 등 도메인별
//!   포지션 해석은 커밋 25 에서 전용 HIR variant 로 분해된다.
//! - `Type::Unknown` 단일 variant 로 시작한다. 타입 체커 합류 시점에
//!   Int/Float/String/Bool/Array/Object/Function 등을 열어 채운다.

#![warn(missing_docs)]

use orv_diagnostics::Span;
pub use orv_resolve::NameId;

/// 프로그램 — 파일 하나의 최상위 문 목록.
#[derive(Clone, Debug)]
pub struct HirProgram {
    /// 최상위 문.
    pub items: Vec<HirStmt>,
    /// 전체 소스 범위.
    pub span: Span,
}

/// 문.
#[derive(Clone, Debug)]
pub enum HirStmt {
    /// `let` / `let mut` / `let sig` 바인딩.
    Let(Box<HirLetStmt>),
    /// `const` 선언.
    Const(Box<HirConstStmt>),
    /// `function` 선언.
    Function(Box<HirFunctionStmt>),
    /// `struct` 선언.
    Struct(Box<HirStructStmt>),
    /// `return` 문.
    Return(HirReturnStmt),
    /// 표현식 문.
    Expr(HirExpr),
}

impl HirStmt {
    /// 문의 소스 범위.
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Let(s) => s.span,
            Self::Const(s) => s.span,
            Self::Function(s) => s.span,
            Self::Struct(s) => s.span,
            Self::Return(s) => s.span,
            Self::Expr(e) => e.span,
        }
    }
}

/// `let` 바인딩.
#[derive(Clone, Debug)]
pub struct HirLetStmt {
    /// 변이 여부.
    pub kind: HirLetKind,
    /// 바인딩 이름 (decl 사이트).
    pub name: HirIdent,
    /// 타입 어노테이션 (있다면).
    pub annotation: Option<HirTypeRef>,
    /// 초기값.
    pub init: HirExpr,
    /// 전체 범위.
    pub span: Span,
}

/// `let` 변형.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HirLetKind {
    /// `let` — 불변.
    Immutable,
    /// `let mut` — 가변.
    Mutable,
    /// `let sig` — 반응형.
    Signal,
}

/// `const` 선언.
#[derive(Clone, Debug)]
pub struct HirConstStmt {
    /// 이름 (decl).
    pub name: HirIdent,
    /// 타입 어노테이션 (있다면).
    pub annotation: Option<HirTypeRef>,
    /// 초기값.
    pub init: HirExpr,
    /// 전체 범위.
    pub span: Span,
}

/// 함수 선언.
#[derive(Clone, Debug)]
pub struct HirFunctionStmt {
    /// 함수 이름 (decl).
    pub name: HirIdent,
    /// 파라미터 목록.
    pub params: Vec<HirParam>,
    /// 반환 타입 어노테이션.
    pub return_ty: Option<HirTypeRef>,
    /// 본문.
    pub body: HirFunctionBody,
    /// 전체 범위.
    pub span: Span,
}

/// 함수 파라미터.
#[derive(Clone, Debug)]
pub struct HirParam {
    /// 파라미터 이름 (decl).
    pub name: HirIdent,
    /// 타입 어노테이션.
    pub annotation: Option<HirTypeRef>,
    /// 소스 위치.
    pub span: Span,
}

/// 함수/람다 본문.
#[derive(Clone, Debug)]
pub enum HirFunctionBody {
    /// 블록 본문.
    Block(HirBlock),
    /// 단일 표현식 본문.
    Expr(HirExpr),
}

/// `return expr`.
#[derive(Clone, Debug)]
pub struct HirReturnStmt {
    /// 반환 값 (없으면 void).
    pub value: Option<HirExpr>,
    /// 소스 위치.
    pub span: Span,
}

/// `struct` 선언.
#[derive(Clone, Debug)]
pub struct HirStructStmt {
    /// 구조체 이름 (decl).
    pub name: HirIdent,
    /// 필드 목록.
    pub fields: Vec<HirStructField>,
    /// 전체 범위.
    pub span: Span,
}

/// 구조체 필드 — 이름은 바인딩이 아니므로 `String + Span`.
#[derive(Clone, Debug)]
pub struct HirStructField {
    /// 필드 이름.
    pub name: String,
    /// 이름 스팬.
    pub name_span: Span,
    /// 타입 어노테이션.
    pub annotation: HirTypeRef,
    /// 전체 필드 위치.
    pub span: Span,
}

/// 스코프 바인딩/참조에 붙는 식별자.
///
/// - `id` 는 [`orv_resolve`] 가 부여한 유일한 바인딩 ID.
/// - `name` 은 진단 포맷용. 런타임 조회에는 사용하지 않는다.
/// - 참조 사이트에서도 `id` 가 같은 선언을 가리킨다.
#[derive(Clone, Debug)]
pub struct HirIdent {
    /// 이 식별자가 가리키는 바인딩.
    pub id: NameId,
    /// 원본 철자 — 진단/디버그 용도.
    pub name: String,
    /// 소스 위치.
    pub span: Span,
}

/// 타입 참조 — 소스에 기록된 어노테이션.
///
/// 타입 체커 전 단계에서는 `Named`/`Array`/`Nullable` 구조만 보존한다. 실제
/// 타입 해소는 타입 체커 몫이며, 그 결과는 [`HirExpr::ty`] 슬롯으로 흐른다.
#[derive(Clone, Debug)]
pub struct HirTypeRef {
    /// 타입 참조 종류.
    pub kind: HirTypeRefKind,
    /// 소스 위치.
    pub span: Span,
}

/// 타입 참조 종류.
#[derive(Clone, Debug)]
pub enum HirTypeRefKind {
    /// `int`, `string`, `User` 등 이름 타입. `NameId` 로 해결되지 않은
    /// 문자열 형태를 유지한다 (타입 해석기 몫).
    Named(String),
    /// `T?`.
    Nullable(Box<HirTypeRef>),
    /// `T[]`.
    Array(Box<HirTypeRef>),
}

/// 표현식.
#[derive(Clone, Debug)]
pub struct HirExpr {
    /// 표현식 종류.
    pub kind: HirExprKind,
    /// 추론/체크된 타입. 타입 체커 전에는 [`Type::Unknown`].
    pub ty: Type,
    /// 소스 위치.
    pub span: Span,
}

/// 표현식 종류.
#[derive(Clone, Debug)]
pub enum HirExprKind {
    /// 정수 리터럴 — 원문 슬라이스.
    Integer(String),
    /// 부동소수점 리터럴 — 원문 슬라이스.
    Float(String),
    /// 문자열 리터럴 — 보간 세그먼트 목록.
    String(Vec<HirStringSegment>),
    /// `true`.
    True,
    /// `false`.
    False,
    /// `void`.
    Void,
    /// 식별자 참조 (NameId 로 해결됨).
    Ident(HirIdent),
    /// 전위 단항 연산.
    Unary {
        /// 연산자.
        op: UnaryOp,
        /// 피연산자.
        expr: Box<HirExpr>,
    },
    /// 이항 연산.
    Binary {
        /// 연산자.
        op: BinaryOp,
        /// 좌변.
        lhs: Box<HirExpr>,
        /// 우변.
        rhs: Box<HirExpr>,
    },
    /// 괄호 그룹.
    Paren(Box<HirExpr>),
    /// `@out arg` — 한 줄 출력. `Domain` 에서 분리된 첫 전용 variant.
    ///
    /// 인자가 없는 `@out` 은 빈 줄 출력이며 lowering 이 `Void` 리터럴을
    /// 채워 넣는다. 다중 인자는 기존 동작과 동일하게 첫 인자만 취한다.
    Out(Box<HirExpr>),
    /// `@html { ... }` — HTML 문서 트리 루트.
    ///
    /// 본문은 평범한 HIR 블록이다. 런타임은 이 블록을 "HTML 렌더 모드" 로
    /// 평가하며, 그 동안 도메인 호출(`@p`, `@head` 등)과 평가 결과 문자열은
    /// 태그/텍스트로 버퍼에 누적된다. `for`/`if`/`let`/함수 호출 같은 기존
    /// 문법은 그대로 동작 — HTML 전용 문법을 새로 학습할 필요 없음.
    /// 결과는 `<html>...</html>` 래퍼를 씌운 `Value::Str`.
    Html(HirBlock),
    /// 아직 전용 variant 로 분해되지 않은 도메인 호출.
    ///
    /// 도메인이 정식 variant 를 받으면 lowering 이 이쪽에 떨어뜨리지 않고
    /// 전용 노드로 보낸다. 미지원 도메인은 런타임에서 에러로 보고된다.
    Domain {
        /// 도메인 이름 (`@` 제외).
        name: String,
        /// 이름 스팬.
        name_span: Span,
        /// 인자 표현식 목록.
        args: Vec<HirExpr>,
    },
    /// 블록 표현식.
    Block(HirBlock),
    /// `if cond { then } else { else_branch }`.
    If {
        /// 조건.
        cond: Box<HirExpr>,
        /// then 블록.
        then: HirBlock,
        /// else 분기 (블록 또는 else-if 표현식).
        else_branch: Option<Box<HirExpr>>,
    },
    /// `when`.
    When {
        /// 검사 대상.
        scrutinee: Box<HirExpr>,
        /// 분기 목록.
        arms: Vec<HirWhenArm>,
    },
    /// 대입 — 현재는 식별자 좌변만.
    Assign {
        /// 좌변 (기존 바인딩의 NameId 를 가리킨다).
        target: HirIdent,
        /// 우변.
        value: Box<HirExpr>,
    },
    /// 함수 호출.
    Call {
        /// 호출 대상.
        callee: Box<HirExpr>,
        /// 인자 목록.
        args: Vec<HirExpr>,
    },
    /// `for var in iter { body }`.
    For {
        /// 루프 변수 (decl).
        var: HirIdent,
        /// 반복 대상.
        iter: Box<HirExpr>,
        /// 본문 블록.
        body: HirBlock,
    },
    /// `while cond { body }`.
    While {
        /// 조건.
        cond: Box<HirExpr>,
        /// 본문 블록.
        body: HirBlock,
    },
    /// `break`.
    Break,
    /// `continue`.
    Continue,
    /// 범위 `a..b` / `a..=b`.
    Range {
        /// 시작.
        start: Box<HirExpr>,
        /// 끝.
        end: Box<HirExpr>,
        /// inclusive 여부.
        inclusive: bool,
    },
    /// 배열 리터럴.
    Array(Vec<HirExpr>),
    /// 객체 리터럴.
    Object(Vec<HirObjectField>),
    /// 인덱스 접근.
    Index {
        /// 대상.
        target: Box<HirExpr>,
        /// 인덱스.
        index: Box<HirExpr>,
    },
    /// 필드 접근 — 필드 이름은 문자열 그대로.
    Field {
        /// 대상.
        target: Box<HirExpr>,
        /// 필드 이름.
        field: String,
        /// 필드 이름 스팬.
        field_span: Span,
    },
    /// 람다 리터럴 `(params) -> body`.
    Lambda {
        /// 파라미터 목록.
        params: Vec<HirParam>,
        /// 본문.
        body: Box<HirFunctionBody>,
    },
    /// `throw expr`.
    Throw(Box<HirExpr>),
    /// `try { ... } catch [binding] { ... }`.
    Try {
        /// 시도 블록.
        try_block: HirBlock,
        /// catch 절 (있다면).
        catch: Option<HirCatchClause>,
    },
}

/// `catch` 절.
#[derive(Clone, Debug)]
pub struct HirCatchClause {
    /// 에러 바인딩 이름 (있다면 decl).
    pub binding: Option<HirIdent>,
    /// 타입 어노테이션 (있다면).
    pub annotation: Option<HirTypeRef>,
    /// 핸들러 블록.
    pub body: HirBlock,
    /// 전체 범위.
    pub span: Span,
}

/// 블록.
#[derive(Clone, Debug)]
pub struct HirBlock {
    /// 블록 안의 문장들.
    pub stmts: Vec<HirStmt>,
    /// 블록 범위.
    pub span: Span,
}

/// 객체 리터럴 필드 — 키는 바인딩이 아니므로 문자열.
#[derive(Clone, Debug)]
pub struct HirObjectField {
    /// 필드 이름.
    pub name: String,
    /// 필드 이름 스팬.
    pub name_span: Span,
    /// 값 표현식.
    pub value: HirExpr,
    /// 전체 범위.
    pub span: Span,
}

/// `when` 분기.
#[derive(Clone, Debug)]
pub struct HirWhenArm {
    /// 패턴.
    pub pattern: HirPattern,
    /// 본문.
    pub body: HirExpr,
}

/// `when` 패턴.
#[derive(Clone, Debug)]
pub enum HirPattern {
    /// `_` 와일드카드.
    Wildcard,
    /// 리터럴 값.
    Literal(HirExpr),
    /// 범위 패턴 `a..b` / `a..=b`.
    Range {
        /// 시작.
        start: HirExpr,
        /// 끝.
        end: HirExpr,
        /// inclusive 여부.
        inclusive: bool,
    },
    /// `$ ...` 가드 — 현재 값(`$`)을 참조하는 표현식.
    Guard(HirExpr),
}

/// 문자열 세그먼트.
#[derive(Clone, Debug)]
pub enum HirStringSegment {
    /// 이스케이프 해제된 리터럴 조각.
    Str(String),
    /// `{expr}` 보간.
    Interp(HirExpr),
}

/// 전위 단항 연산자.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOp {
    /// `!`.
    Not,
    /// `-`.
    Neg,
    /// `~`.
    BitNot,
}

/// 이항 연산자.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOp {
    /// `+`.
    Add,
    /// `-`.
    Sub,
    /// `*`.
    Mul,
    /// `/`.
    Div,
    /// `%`.
    Rem,
    /// `**`.
    Pow,
    /// `==`.
    Eq,
    /// `!=`.
    Ne,
    /// `<`.
    Lt,
    /// `>`.
    Gt,
    /// `<=`.
    Le,
    /// `>=`.
    Ge,
    /// `&&`.
    And,
    /// `||`.
    Or,
    /// `&`.
    BitAnd,
    /// `|`.
    BitOr,
    /// `^`.
    BitXor,
    /// `<<`.
    Shl,
    /// `>>`.
    Shr,
    /// `??`.
    Coalesce,
}

/// HIR 타입 슬롯.
///
/// 이번 커밋에서는 [`Type::Unknown`] 단일 variant 만 존재한다. 타입 체커가
/// 합류하는 시점에 Int/Float/Str/Bool/Array/Object/Function 등을 넓혀 채운다.
/// 지금 구조를 미리 펼치면 lowering 이 채워야 할 허수 정보가 생겨 결국
/// 죽은 코드가 되므로 의도적으로 최소화한다.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Type {
    /// 아직 결정되지 않은 타입.
    #[default]
    Unknown,
}
