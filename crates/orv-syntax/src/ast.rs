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
    /// `function` 선언.
    Function(Box<FunctionStmt>),
    /// `struct` 선언.
    Struct(Box<StructStmt>),
    /// `return` 문.
    Return(ReturnStmt),
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
            Self::Function(s) => s.span,
            Self::Struct(s) => s.span,
            Self::Return(s) => s.span,
            Self::Expr(e) => e.span,
        }
    }
}

/// 함수 선언 (SPEC §5).
#[derive(Clone, Debug)]
pub struct FunctionStmt {
    /// 함수 이름.
    pub name: Ident,
    /// 파라미터 목록.
    pub params: Vec<Param>,
    /// 반환 타입 (선택).
    pub return_ty: Option<TypeRef>,
    /// 본문 — `{ ... }` 블록 또는 단일 표현식.
    pub body: FunctionBody,
    /// B2 MVP: `async function` 선언 여부. 타입 표면만 보존하며 interp 는
    /// sync 실행한다. 실제 Future 스케줄링은 후속 마일스톤.
    pub is_async: bool,
    /// 전체 범위.
    pub span: Span,
}

/// 함수 파라미터.
#[derive(Clone, Debug)]
pub struct Param {
    /// 파라미터 이름.
    pub name: Ident,
    /// 타입 어노테이션 (선택 — MVP에서는 필수이나 누락 허용).
    pub ty: Option<TypeRef>,
    /// 소스 위치.
    pub span: Span,
}

/// 함수 본문 변형.
#[derive(Clone, Debug)]
pub enum FunctionBody {
    /// 블록 본문 `{ ... }`.
    Block(Block),
    /// 단일 표현식 본문 (`-> expr`, 블록 아님).
    Expr(Expr),
}

/// `return expr` 혹은 `return`.
#[derive(Clone, Debug)]
pub struct ReturnStmt {
    /// 반환 값 (없으면 void).
    pub value: Option<Expr>,
    /// 소스 위치.
    pub span: Span,
}

/// `struct` 선언 (SPEC §4.6).
#[derive(Clone, Debug)]
pub struct StructStmt {
    /// 구조체 이름.
    pub name: Ident,
    /// 필드 목록 — 선언 순서 유지.
    pub fields: Vec<StructField>,
    /// 전체 범위.
    pub span: Span,
}

/// struct 필드.
#[derive(Clone, Debug)]
pub struct StructField {
    /// 필드 이름.
    pub name: Ident,
    /// 타입 어노테이션.
    pub ty: TypeRef,
    /// 소스 위치.
    pub span: Span,
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
    /// 배열 (`T[]`).
    Array(Box<TypeRef>),
}

/// 표현식.
#[derive(Clone, Debug)]
pub struct Expr {
    /// 표현식 종류.
    pub kind: ExprKind,
    /// 소스 위치.
    pub span: Span,
}

/// 표현식 변형.
#[derive(Clone, Debug)]
pub enum ExprKind {
    /// 정수 리터럴 — 원문 슬라이스 보관.
    Integer(String),
    /// 부동소수점 리터럴.
    Float(String),
    /// 문자열 리터럴 — 보간 세그먼트 목록.
    /// 보간이 없는 단순 문자열도 `[Str(literal)]` 한 세그먼트로 표현된다.
    String(Vec<StringSegment>),
    /// `true`.
    True,
    /// `false`.
    False,
    /// `void`.
    Void,
    /// 식별자 참조.
    Ident(Ident),
    /// 전위 단항 연산 (`!x`, `-x`, `~x`).
    Unary {
        /// 연산자.
        op: UnaryOp,
        /// 피연산자.
        expr: Box<Expr>,
    },
    /// 이항 연산.
    Binary {
        /// 연산자.
        op: BinaryOp,
        /// 좌변.
        lhs: Box<Expr>,
        /// 우변.
        rhs: Box<Expr>,
    },
    /// 괄호 그룹 `( expr )` — 구문 구조를 보존하기 위해 유지.
    Paren(Box<Expr>),
    /// 도메인 호출 (`@out "hi"`, `@route GET /api`).
    ///
    /// MVP에서는 단순 token 인자(표현식 한 개 이상)만 지원한다. property
    /// (`key=value`), 중첩 경로(`@db.find`), `{}` 본문 등은 이후 커밋에서
    /// 확장한다.
    Domain {
        /// 도메인 이름 (`@`를 제외한 본체).
        name: Ident,
        /// 인자 표현식 목록 — 순서대로 token으로 처리.
        args: Vec<Expr>,
    },
    /// `{ stmt*  final_expr? }` 블록. 마지막 표현식이 블록의 값.
    Block(Block),
    /// `if cond { then } else { else_branch }` — else는 선택.
    If {
        /// 조건.
        cond: Box<Expr>,
        /// then 분기 블록.
        then: Block,
        /// else 분기 블록 또는 또 다른 if(else if).
        else_branch: Option<Box<Expr>>,
    },
    /// `when scrutinee { arm* }` 패턴 매칭.
    When {
        /// 검사 대상 표현식.
        scrutinee: Box<Expr>,
        /// 분기 목록 — 순서대로 매칭된다.
        arms: Vec<WhenArm>,
    },
    /// 대입 `lhs = rhs`. 현재는 식별자 좌변만 지원.
    Assign {
        /// 좌변 식별자.
        target: Ident,
        /// 우변.
        value: Box<Expr>,
    },
    /// 함수 호출 `callee(args)`.
    Call {
        /// 호출 대상 표현식.
        callee: Box<Expr>,
        /// 인자 목록.
        args: Vec<Expr>,
    },
    /// `for binding in iter { body }` 루프.
    For {
        /// 루프 변수 이름.
        var: Ident,
        /// 반복 대상 표현식.
        iter: Box<Expr>,
        /// 본문 블록.
        body: Block,
    },
    /// `while cond { body }` 루프.
    While {
        /// 조건.
        cond: Box<Expr>,
        /// 본문 블록.
        body: Block,
    },
    /// `break` — 가장 가까운 루프 종료.
    Break,
    /// `continue` — 루프 다음 반복으로.
    Continue,
    /// 범위 표현식 `a..b` 또는 `a..=b`. 현재는 정수 범위만 사용.
    Range {
        /// 시작 값.
        start: Box<Expr>,
        /// 끝 값.
        end: Box<Expr>,
        /// inclusive 여부.
        inclusive: bool,
    },
    /// 배열 리터럴 `[a, b, c]`.
    Array(Vec<Expr>),
    /// 객체 리터럴 `{ key: value, ... }`.
    /// 타입 없는 인라인 오브젝트 또는 struct 인스턴스화 양쪽에 사용.
    Object(Vec<ObjectField>),
    /// 인덱스 접근 `target[index]`.
    Index {
        /// 대상 표현식.
        target: Box<Expr>,
        /// 인덱스 표현식.
        index: Box<Expr>,
    },
    /// 필드/속성 접근 `target.field`.
    Field {
        /// 대상 표현식.
        target: Box<Expr>,
        /// 필드 이름.
        field: Ident,
    },
    /// 람다 리터럴 `(params) -> body`.
    Lambda {
        /// 파라미터 목록.
        params: Vec<Param>,
        /// 본문 — 블록 또는 단일 표현식.
        body: Box<FunctionBody>,
    },
    /// `throw expr` — 에러 발생.
    Throw(Box<Expr>),
    /// `await expr` — async 결과 대기. B2 MVP 는 identity (sync 평가).
    Await(Box<Expr>),
    /// `try { ... } catch [binding [: type]] { ... }`.
    Try {
        /// 시도 블록.
        try_block: Block,
        /// catch 절 (선택). 없으면 단순 `try { }` 형태.
        catch: Option<CatchClause>,
    },
}

/// `catch` 절.
#[derive(Clone, Debug)]
pub struct CatchClause {
    /// 에러 바인딩 이름 (선택).
    pub binding: Option<Ident>,
    /// 타입 어노테이션 (선택 — MVP에서는 기록만 함).
    pub ty: Option<TypeRef>,
    /// 핸들러 블록.
    pub body: Block,
    /// 전체 범위.
    pub span: Span,
}

/// 중괄호 블록 — 문장 목록 + 블록 값을 결정하는 여부.
#[derive(Clone, Debug)]
pub struct Block {
    /// 블록 안의 문장들.
    pub stmts: Vec<Stmt>,
    /// 블록 범위.
    pub span: Span,
}

/// 객체 리터럴의 필드 항목.
#[derive(Clone, Debug)]
pub struct ObjectField {
    /// 필드 이름.
    pub name: Ident,
    /// 값 표현식.
    pub value: Expr,
    /// 소스 위치.
    pub span: Span,
}

/// `when` 분기.
#[derive(Clone, Debug)]
pub struct WhenArm {
    /// 패턴.
    pub pattern: Pattern,
    /// 본문 표현식.
    pub body: Expr,
}

/// `when`의 패턴. MVP는 리터럴, `_` 와일드카드, `$` 현재값 참조, 범위만.
///
/// `Wildcard` 는 zero-size variant 라 Expr 를 담는 다른 variant 와 크기 차이가
/// 크지만, 패턴 분기는 한 번에 보존되며 빈도가 균등해 Box 래핑의 실익이 없다.
/// clippy 의 `large_enum_variant` 는 이 영역에서 false positive 로 취급.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Pattern {
    /// `_` 기본 분기.
    Wildcard,
    /// 리터럴 패턴 (정수/부동/문자열/불리언/void).
    Literal(Expr),
    /// 범위 `a..b` 또는 `a..=b`.
    Range {
        /// 시작 값.
        start: Expr,
        /// 끝 값.
        end: Expr,
        /// inclusive 여부.
        inclusive: bool,
    },
    /// `$ ...` 가드 — 현재 값 참조 표현식.
    Guard(Expr),
    /// `!EXPR` — 스크루티니가 값과 같지 않을 때 매치 (§6.3).
    Not(Expr),
    /// `in EXPR` — 스크루티니 컬렉션/문자열이 값을 포함할 때 매치.
    Contains(Expr),
}

/// 전위 단항 연산자.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOp {
    /// `!` 논리 부정.
    Not,
    /// `-` 부호 반전.
    Neg,
    /// `~` 비트 반전.
    BitNot,
}

/// 문자열 리터럴의 구성 세그먼트.
#[derive(Clone, Debug)]
pub enum StringSegment {
    /// 리터럴 문자열 조각 (이스케이프 해제된 최종 값).
    Str(String),
    /// `{expr}` 보간 부분 — 내부 표현식 그대로.
    Interp(Expr),
}

/// 이항 연산자.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `%`
    Rem,
    /// `**`
    Pow,
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `>`
    Gt,
    /// `<=`
    Le,
    /// `>=`
    Ge,
    /// `&&`
    And,
    /// `||`
    Or,
    /// `&`
    BitAnd,
    /// `|`
    BitOr,
    /// `^`
    BitXor,
    /// `<<`
    Shl,
    /// `>>`
    Shr,
    /// `??` 널 병합.
    Coalesce,
}
