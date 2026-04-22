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
    /// SPEC §4.4 `enum` 선언.
    Enum(Box<HirEnumStmt>),
    /// `return` 문.
    Return(HirReturnStmt),
    /// SPEC §8 `import` — 멀티파일 로더가 실제 병합을 수행하고, 단일파일
    /// pipeline 에서는 silent noop. span 만 보존한다.
    Import(Span),
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
            Self::Enum(s) => s.span,
            Self::Return(s) => s.span,
            Self::Import(span) => *span,
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
    /// B2 MVP: `async function` 여부. 타입 표면에만 영향, interp 는 sync.
    pub is_async: bool,
    /// C0: `define` 선언 여부. 후속 마일스톤에서 `@Name` invoke registry 가
    /// 이 플래그를 참조한다.
    pub is_define: bool,
    /// C0: `pub` 가시성 modifier. B3 import 에서 실제 의미 부여.
    pub is_pub: bool,
    /// SPEC §9.4: body 최상단에서 선언된 token slot 목록.
    /// 호출부의 positional 인자가 이 slot 에 `T[]` 로 바인딩된다.
    pub token_slots: Vec<HirTokenSlot>,
    /// 전체 범위.
    pub span: Span,
}

/// SPEC §9.4 token slot.
///
/// slot 이름은 function scope 에 param 처럼 바인딩되므로 `HirIdent` 로 유지
/// 하고, element 타입은 향후 타입 체커가 패턴 매칭 입력으로 사용한다.
#[derive(Clone, Debug)]
pub struct HirTokenSlot {
    /// slot 이름 (decl).
    pub name: HirIdent,
    /// element 타입.
    pub ty: HirTypeRef,
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

/// SPEC §4.4 enum 선언.
#[derive(Clone, Debug)]
pub struct HirEnumStmt {
    /// enum 이름 (decl).
    pub name: HirIdent,
    /// variant 목록.
    pub variants: Vec<HirEnumVariant>,
    /// 전체 범위.
    pub span: Span,
}

/// enum 한 variant.
#[derive(Clone, Debug)]
pub struct HirEnumVariant {
    /// variant 이름 (binding 아님, namespace 키).
    pub name: String,
    /// 이름 스팬.
    pub name_span: Span,
    /// 값 표현식.
    pub value: HirExpr,
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
    /// `@route METHOD /path { handler }` — HTTP 라우트 선언.
    ///
    /// 이 variant 는 실제 실행을 담지 않는다. 런타임이 `@server { ... }`
    /// 블록 안에서 평가할 때 라우트 등록 테이블에 push 한다 (C5). 그 외
    /// 맥락에서 평가되면 silent noop. 실행 본체는 handler block 이 요청
    /// 시점마다 새 스코프로 평가된다.
    Route {
        /// HTTP 메서드 (`GET`, `POST`, ...) 또는 wildcard `"*"`.
        method: String,
        /// method 토큰 스팬.
        method_span: Span,
        /// 경로 패턴 (`"/api/users/:id"` 등). `:param` 은 현재 문자열에
        /// 그대로 보존되며, 매칭/파라미터 추출은 런타임 몫이다.
        path: String,
        /// 경로 토큰들의 전체 스팬.
        path_span: Span,
        /// 요청 처리 블록. 요청마다 새 env 로 평가된다.
        handler: HirBlock,
    },
    /// `@respond <status> <payload>?` — HTTP 응답 생성 + early-return.
    ///
    /// SPEC §11.4 규칙: 호출 즉시 현재 route handler 의 실행이 끝난다
    /// (`return` 과 같은 시맨틱). 런타임은 `@route` handler 평가 중에 이
    /// variant 를 만나면 응답 슬롯을 채우고 블록 종료 신호를 낸다. payload
    /// 가 생략된 경우 lowering 이 `Void` 리터럴을 채워 넣는다 (`204` 등).
    Respond {
        /// 상태 코드 표현식 (보통 Integer 리터럴).
        status: Box<HirExpr>,
        /// 응답 본문 표현식 — 주로 object literal 이지만 어떤 값이든 가능.
        /// 생략 시 `HirExprKind::Void` 로 채워진다.
        payload: Box<HirExpr>,
    },
    /// `@server { @listen N; @route ...; ... }` — HTTP 서버 선언.
    ///
    /// 이번 커밋(C5a)에서는 구조적 레이어만 정의한다. 런타임은 이 variant
    /// 를 평가할 때 silent noop(`Value::Void`)을 반환하며, 실제 tokio+hyper
    /// 서버 기동은 후속 커밋(C5b)에서 이 arm 을 교체해 구현한다.
    ///
    /// # 구조 (advisor 피드백 반영)
    /// - `listen`: `@listen N` 자식에서 수집한 port 표현식. 없으면 `None`.
    /// - `routes`: `@route METHOD /path { ... }` 자식들 — 반드시
    ///   [`HirExprKind::Route`] variant 만 들어간다 (analyzer 에서 강제).
    /// - `body_stmts`: `@listen`/`@route` 가 아닌 기타 문장(`@out "boot"`,
    ///   미들웨어 등). SPEC §11.1 예제가 `@out` 을 server 블록 안에 쓰므로
    ///   drop 하면 유효 프로그램이 깨진다. 현재는 보존만 하고 실행 시점은
    ///   C5b 에서 결정한다 (서버 기동 직전에 평가).
    ///
    /// # 범위 밖
    /// - `@route /admin { @route ... }` 형태의 중첩 라우트 그룹(SPEC §11.7):
    ///   바깥 `@route` 가 method 없이 path-only 이므로 C1 parser 가 수용
    ///   불가. C6 이후 마일스톤에서 처리.
    Server {
        /// `@listen N` 에서 수집한 port 표현식. 여러 개 선언 시 마지막이
        /// 우세하며 analyzer 가 중복 진단을 낸다.
        listen: Option<Box<HirExpr>>,
        /// 라우트 목록. analyzer 는 `HirExprKind::Route` 만 여기에 넣는다.
        routes: Vec<HirExpr>,
        /// `@listen`/`@route` 이외의 기타 문장 — `@out`, 미들웨어 등.
        /// 원래 순서대로 보존한다.
        body_stmts: Vec<HirStmt>,
    },
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
    /// SPEC §4.6 필드 재대입 — `obj.field = value`.
    AssignField {
        /// 좌변 객체.
        object: Box<HirExpr>,
        /// 필드 이름.
        field: String,
        /// 필드 이름 스팬.
        field_span: Span,
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
    ///
    /// SPEC §6.4: `index_var` 가 `Some` 이면 `for (var, index_var) in iter` 형태
    /// 이며, 인덱스가 0부터 증가하며 해당 바인딩에 주입된다.
    For {
        /// 루프 변수 (decl).
        var: HirIdent,
        /// 인덱스 변수 (decl, optional).
        index_var: Option<HirIdent>,
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
    /// SPEC 부록: `target[start:end]` 슬라이싱 (string / array).
    ///
    /// 양쪽 경계 모두 `Option` 으로 유지해 `[:]` / `[:b]` / `[a:]` 를
    /// 표현한다. 런타임은 음수 인덱스를 파이썬식으로 해석하며, 결과 타입은
    /// target 과 동일하다 (String → String, Array(T) → Array(T)).
    Slice {
        /// 대상 표현식.
        target: Box<HirExpr>,
        /// 시작 인덱스. `None` 은 0.
        start: Option<Box<HirExpr>>,
        /// 끝 인덱스(exclusive). `None` 은 `length`.
        end: Option<Box<HirExpr>>,
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
    /// `await expr` — B2 MVP 는 identity (피연산자 평가 결과 반환).
    Await(Box<HirExpr>),
    /// SPEC §4.9 `expr as <type>` — 타입 캐스팅.
    ///
    /// 런타임은 원시 타입 간 변환을 허용한다 (numeric width, string→int 파싱,
    /// display-based string 캐스트 등). 타입 체커는 `ty` 의 해석을 HIR 타입
    /// 슬롯에 실어 준다.
    Cast {
        /// 피연산자.
        expr: Box<HirExpr>,
        /// 타겟 타입 어노테이션.
        ty: HirTypeRef,
    },
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
    /// 필드 이름. spread 의 경우 `"__spread__"`.
    pub name: String,
    /// 필드 이름 스팬.
    pub name_span: Span,
    /// 값 표현식.
    pub value: HirExpr,
    /// SPEC §2.5 spread `...expr` 여부.
    pub is_spread: bool,
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
///
/// B5 에서 `Type` 이 커지며 `HirExpr` 도 커졌고, `Wildcard` 같은 unit variant
/// 와 `Literal(HirExpr)` 사이 크기 차이가 clippy 경고 기준(200 bytes) 을 넘긴다.
/// 각 variant 를 Box 로 바꾸면 공용 호출 경로(`match arm.pattern { ... }`) 에
/// 영향이 넓게 퍼지므로, 당분간 lint 만 억제한다. HIR 사이즈 최적화는 후속.
#[allow(clippy::large_enum_variant)]
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
    /// `!EXPR` — 스크루티니가 값과 같지 않을 때 매치.
    Not(HirExpr),
    /// `in EXPR` — 스크루티니 컬렉션/문자열이 값을 포함할 때 매치.
    Contains(HirExpr),
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
/// B5 Stage 1: 기본 원시 + nullable + array + struct 이름. 더 깊은 구조
/// (generic, union, tuple, map) 는 후속 스테이지에서 확장한다.
///
/// 추론 실패/어노테이션 미사용은 [`Type::Unknown`] 으로 유지하며, 타입 체커
/// 는 Unknown 과의 비교를 "무해" 처리해 점진적 도입이 가능하도록 한다.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Type {
    /// 아직 결정되지 않은 타입 (conservative 통과).
    #[default]
    Unknown,
    /// 정수 계열 — MVP 는 i64 단일.
    Int,
    /// 부동소수점 계열 — MVP 는 f64 단일.
    Float,
    /// 문자열.
    String,
    /// 불리언.
    Bool,
    /// 값 없음.
    Void,
    /// `T?` — void 를 허용하는 nullable.
    Nullable(Box<Type>),
    /// `T[]` — 동종 배열.
    Array(Box<Type>),
    /// 사용자 정의 struct. 필드 타입 lookup 은 별도 테이블을 통해 수행.
    Struct(String),
    /// 함수 타입 — 파라미터 타입 시퀀스와 반환 타입.
    ///
    /// MVP 는 arity-exact 매칭. optional/rest/named argument 는 후속.
    Function {
        /// 파라미터 타입들.
        params: Vec<Type>,
        /// 반환 타입. annotation 누락 시 `Unknown`.
        ret: Box<Type>,
    },
}

impl Type {
    /// `Nullable(inner)` 이면 `inner` 를 꺼내고, 아니면 그대로.
    #[must_use]
    pub fn strip_nullable(&self) -> &Self {
        match self {
            Self::Nullable(inner) => inner,
            other => other,
        }
    }

    /// 두 타입이 assignment-compatible 한가 (MVP, non-symmetric).
    ///
    /// 규칙:
    /// - `Unknown` 은 양쪽 모두 통과 — 추론 미완 상태에서 errors 를 양산하지
    ///   않도록 conservative.
    /// - 완전히 같은 타입은 통과.
    /// - `target == Nullable(T)` 이면 value 가 `Void` 또는 `T` 호환이면 통과.
    /// - `Int` ↔ `Float` 은 묵시적 변환 불가 (SPEC §4.9 는 `as` 명시 요구).
    /// - `Array(A)` ↔ `Array(B)` 는 A 와 B 가 compatible 이면 통과.
    #[must_use]
    pub fn is_assignable_from(&self, value: &Self) -> bool {
        if matches!(self, Self::Unknown) || matches!(value, Self::Unknown) {
            return true;
        }
        if self == value {
            return true;
        }
        if let Self::Nullable(inner) = self {
            if matches!(value, Self::Void) {
                return true;
            }
            return inner.is_assignable_from(value);
        }
        if let (Self::Array(a), Self::Array(b)) = (self, value) {
            return a.is_assignable_from(b);
        }
        false
    }

    /// 사람이 읽을 수 있는 표기 — 진단 메시지용.
    #[must_use]
    pub fn display(&self) -> String {
        match self {
            Self::Unknown => "unknown".into(),
            Self::Int => "int".into(),
            Self::Float => "float".into(),
            Self::String => "string".into(),
            Self::Bool => "bool".into(),
            Self::Void => "void".into(),
            Self::Nullable(inner) => format!("{}?", inner.display()),
            Self::Array(inner) => format!("{}[]", inner.display()),
            Self::Struct(name) => name.clone(),
            Self::Function { params, ret } => {
                let ps = params
                    .iter()
                    .map(Self::display)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("Function<({ps}), {}>", ret.display())
            }
        }
    }
}
