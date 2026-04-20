//! 시맨틱 분석 — AST → HIR lowering.
//!
//! # 역할
//! [`orv_resolve`] 가 AST 의 모든 식별자 스팬에 부여한 [`NameId`] 를 이용해
//! AST 를 HIR 로 변환한다. 이번 단계에서는 타입 추론을 수행하지 않으며,
//! 모든 [`HirExpr::ty`] 슬롯은 [`Type::Unknown`] 로 남는다.
//!
//! # 사용 순서
//! ```ignore
//! let lex_result = lex(src, file_id);
//! let parse_result = parse(lex_result.tokens, file_id);
//! let resolved = orv_resolve::resolve(&parse_result.program);
//! // resolved.diagnostics 에 에러가 있으면 lowering 을 하지 않는다.
//! let hir = lower(&parse_result.program, &resolved);
//! ```
//!
//! # 계약
//! - 호출 전 `resolved.diagnostics` 가 비어 있어야 한다. 미정의 변수가
//!   남아 있으면 lowering 은 `NameId(u32::MAX)` 로 스텁하여 진행하므로
//!   동작은 정의돼 있지만 결과 HIR 은 의미가 없다. 런타임은 이를 직접
//!   조회하지 않도록 설계돼야 한다.
//! - AST 의 스팬과 HIR 의 스팬은 1:1 로 대응한다.

#![warn(missing_docs)]

use orv_hir as hir;
use orv_resolve::{NameId, ResolveResult};
use orv_syntax::ast;

/// 프로그램 전체를 HIR 로 변환한다.
#[must_use]
pub fn lower(program: &ast::Program, resolved: &ResolveResult) -> hir::HirProgram {
    let lowerer = Lowerer { resolved };
    hir::HirProgram {
        items: program.items.iter().map(|s| lowerer.stmt(s)).collect(),
        span: program.span,
    }
}

struct Lowerer<'a> {
    resolved: &'a ResolveResult,
}

impl<'a> Lowerer<'a> {
    fn name_id(&self, ident: &ast::Ident) -> NameId {
        self.resolved
            .name_of
            .get(&ident.span.into())
            .copied()
            .unwrap_or(NameId(u32::MAX))
    }

    fn ident(&self, ident: &ast::Ident) -> hir::HirIdent {
        hir::HirIdent {
            id: self.name_id(ident),
            name: ident.name.clone(),
            span: ident.span,
        }
    }

    fn ty_ref(&self, t: &ast::TypeRef) -> hir::HirTypeRef {
        hir::HirTypeRef {
            span: t.span,
            kind: match &t.kind {
                ast::TypeRefKind::Named(id) => hir::HirTypeRefKind::Named(id.name.clone()),
                ast::TypeRefKind::Nullable(inner) => {
                    hir::HirTypeRefKind::Nullable(Box::new(self.ty_ref(inner)))
                }
                ast::TypeRefKind::Array(inner) => {
                    hir::HirTypeRefKind::Array(Box::new(self.ty_ref(inner)))
                }
            },
        }
    }

    fn param(&self, p: &ast::Param) -> hir::HirParam {
        hir::HirParam {
            name: self.ident(&p.name),
            annotation: p.ty.as_ref().map(|t| self.ty_ref(t)),
            span: p.span,
        }
    }

    fn stmt(&self, s: &ast::Stmt) -> hir::HirStmt {
        match s {
            ast::Stmt::Let(l) => hir::HirStmt::Let(Box::new(hir::HirLetStmt {
                kind: match l.kind {
                    ast::LetKind::Immutable => hir::HirLetKind::Immutable,
                    ast::LetKind::Mutable => hir::HirLetKind::Mutable,
                    ast::LetKind::Signal => hir::HirLetKind::Signal,
                },
                name: self.ident(&l.name),
                annotation: l.ty.as_ref().map(|t| self.ty_ref(t)),
                init: self.expr(&l.init),
                span: l.span,
            })),
            ast::Stmt::Const(c) => hir::HirStmt::Const(Box::new(hir::HirConstStmt {
                name: self.ident(&c.name),
                annotation: c.ty.as_ref().map(|t| self.ty_ref(t)),
                init: self.expr(&c.init),
                span: c.span,
            })),
            ast::Stmt::Function(f) => hir::HirStmt::Function(Box::new(self.function(f))),
            ast::Stmt::Struct(s) => hir::HirStmt::Struct(Box::new(hir::HirStructStmt {
                name: self.ident(&s.name),
                fields: s
                    .fields
                    .iter()
                    .map(|f| hir::HirStructField {
                        name: f.name.name.clone(),
                        name_span: f.name.span,
                        annotation: self.ty_ref(&f.ty),
                        span: f.span,
                    })
                    .collect(),
                span: s.span,
            })),
            ast::Stmt::Return(r) => hir::HirStmt::Return(hir::HirReturnStmt {
                value: r.value.as_ref().map(|e| self.expr(e)),
                span: r.span,
            }),
            ast::Stmt::Expr(e) => hir::HirStmt::Expr(self.expr(e)),
        }
    }

    fn function(&self, f: &ast::FunctionStmt) -> hir::HirFunctionStmt {
        hir::HirFunctionStmt {
            name: self.ident(&f.name),
            params: f.params.iter().map(|p| self.param(p)).collect(),
            return_ty: f.return_ty.as_ref().map(|t| self.ty_ref(t)),
            body: self.function_body(&f.body),
            is_async: f.is_async,
            span: f.span,
        }
    }

    fn function_body(&self, b: &ast::FunctionBody) -> hir::HirFunctionBody {
        match b {
            ast::FunctionBody::Block(block) => hir::HirFunctionBody::Block(self.block(block)),
            ast::FunctionBody::Expr(e) => hir::HirFunctionBody::Expr(self.expr(e)),
        }
    }

    fn block(&self, b: &ast::Block) -> hir::HirBlock {
        hir::HirBlock {
            stmts: b.stmts.iter().map(|s| self.stmt(s)).collect(),
            span: b.span,
        }
    }

    fn expr(&self, e: &ast::Expr) -> hir::HirExpr {
        hir::HirExpr {
            kind: self.expr_kind(e),
            ty: hir::Type::Unknown,
            span: e.span,
        }
    }

    fn expr_kind(&self, e: &ast::Expr) -> hir::HirExprKind {
        match &e.kind {
            ast::ExprKind::Integer(s) => hir::HirExprKind::Integer(s.clone()),
            ast::ExprKind::Float(s) => hir::HirExprKind::Float(s.clone()),
            ast::ExprKind::String(segments) => hir::HirExprKind::String(
                segments
                    .iter()
                    .map(|seg| match seg {
                        ast::StringSegment::Str(s) => hir::HirStringSegment::Str(s.clone()),
                        ast::StringSegment::Interp(inner) => {
                            hir::HirStringSegment::Interp(self.expr(inner))
                        }
                    })
                    .collect(),
            ),
            ast::ExprKind::True => hir::HirExprKind::True,
            ast::ExprKind::False => hir::HirExprKind::False,
            ast::ExprKind::Void => hir::HirExprKind::Void,
            ast::ExprKind::Ident(id) => hir::HirExprKind::Ident(self.ident(id)),
            ast::ExprKind::Unary { op, expr } => hir::HirExprKind::Unary {
                op: unary_op(*op),
                expr: Box::new(self.expr(expr)),
            },
            ast::ExprKind::Binary { op, lhs, rhs } => hir::HirExprKind::Binary {
                op: binary_op(*op),
                lhs: Box::new(self.expr(lhs)),
                rhs: Box::new(self.expr(rhs)),
            },
            ast::ExprKind::Paren(inner) => hir::HirExprKind::Paren(Box::new(self.expr(inner))),
            ast::ExprKind::Domain { name, args } => self.lower_domain(e, name, args),
            ast::ExprKind::Block(b) => hir::HirExprKind::Block(self.block(b)),
            ast::ExprKind::If {
                cond,
                then,
                else_branch,
            } => hir::HirExprKind::If {
                cond: Box::new(self.expr(cond)),
                then: self.block(then),
                else_branch: else_branch.as_ref().map(|e| Box::new(self.expr(e))),
            },
            ast::ExprKind::When { scrutinee, arms } => hir::HirExprKind::When {
                scrutinee: Box::new(self.expr(scrutinee)),
                arms: arms
                    .iter()
                    .map(|arm| hir::HirWhenArm {
                        pattern: self.pattern(&arm.pattern),
                        body: self.expr(&arm.body),
                    })
                    .collect(),
            },
            ast::ExprKind::Assign { target, value } => hir::HirExprKind::Assign {
                target: self.ident(target),
                value: Box::new(self.expr(value)),
            },
            ast::ExprKind::Call { callee, args } => hir::HirExprKind::Call {
                callee: Box::new(self.expr(callee)),
                args: args.iter().map(|a| self.expr(a)).collect(),
            },
            ast::ExprKind::For { var, iter, body } => hir::HirExprKind::For {
                var: self.ident(var),
                iter: Box::new(self.expr(iter)),
                body: self.block(body),
            },
            ast::ExprKind::While { cond, body } => hir::HirExprKind::While {
                cond: Box::new(self.expr(cond)),
                body: self.block(body),
            },
            ast::ExprKind::Break => hir::HirExprKind::Break,
            ast::ExprKind::Continue => hir::HirExprKind::Continue,
            ast::ExprKind::Range {
                start,
                end,
                inclusive,
            } => hir::HirExprKind::Range {
                start: Box::new(self.expr(start)),
                end: Box::new(self.expr(end)),
                inclusive: *inclusive,
            },
            ast::ExprKind::Array(items) => {
                hir::HirExprKind::Array(items.iter().map(|i| self.expr(i)).collect())
            }
            ast::ExprKind::Object(fields) => hir::HirExprKind::Object(
                fields
                    .iter()
                    .map(|f| hir::HirObjectField {
                        name: f.name.name.clone(),
                        name_span: f.name.span,
                        value: self.expr(&f.value),
                        span: f.span,
                    })
                    .collect(),
            ),
            ast::ExprKind::Index { target, index } => hir::HirExprKind::Index {
                target: Box::new(self.expr(target)),
                index: Box::new(self.expr(index)),
            },
            ast::ExprKind::Field { target, field } => hir::HirExprKind::Field {
                target: Box::new(self.expr(target)),
                field: field.name.clone(),
                field_span: field.span,
            },
            ast::ExprKind::Lambda { params, body } => hir::HirExprKind::Lambda {
                params: params.iter().map(|p| self.param(p)).collect(),
                body: Box::new(self.function_body(body)),
            },
            ast::ExprKind::Throw(inner) => hir::HirExprKind::Throw(Box::new(self.expr(inner))),
            ast::ExprKind::Await(inner) => hir::HirExprKind::Await(Box::new(self.expr(inner))),
            ast::ExprKind::Try { try_block, catch } => hir::HirExprKind::Try {
                try_block: self.block(try_block),
                catch: catch.as_ref().map(|c| hir::HirCatchClause {
                    binding: c.binding.as_ref().map(|b| self.ident(b)),
                    annotation: c.ty.as_ref().map(|t| self.ty_ref(t)),
                    body: self.block(&c.body),
                    span: c.span,
                }),
            },
        }
    }

    /// 도메인 호출을 variant 별로 분해한다.
    ///
    /// 이번 단계에서는 `@out` 만 전용 [`hir::HirExprKind::Out`] 로 내려간다.
    /// 나머지 도메인은 fallback 인 [`hir::HirExprKind::Domain`] 에 그대로
    /// 남으며, 각 도메인이 정식 구현되는 후속 커밋에서 하나씩 전용 variant
    /// 로 옮겨진다.
    fn lower_domain(
        &self,
        origin: &ast::Expr,
        name: &ast::Ident,
        args: &[ast::Expr],
    ) -> hir::HirExprKind {
        if name.name == "out" {
            // 인자가 없으면 빈 줄 출력 동작을 유지하기 위해 `void` 리터럴을
            // 채워 넣는다. 다중 인자는 기존 인터프리터 동작(첫 인자만)과
            // 일치시키기 위해 첫 인자만 취한다.
            let arg = match args.first() {
                Some(first) => self.expr(first),
                None => hir::HirExpr {
                    kind: hir::HirExprKind::Void,
                    ty: hir::Type::Unknown,
                    span: origin.span,
                },
            };
            return hir::HirExprKind::Out(Box::new(arg));
        }
        if name.name == "html" {
            return hir::HirExprKind::Html(self.lower_html_body(origin, args));
        }
        if name.name == "route" {
            if let Some(kind) = self.lower_route(args) {
                return kind;
            }
        }
        if name.name == "respond" {
            return self.lower_respond(origin, args);
        }
        if name.name == "server" {
            if let Some(kind) = self.lower_server(args) {
                return kind;
            }
        }
        hir::HirExprKind::Domain {
            name: name.name.clone(),
            name_span: name.span,
            args: args.iter().map(|a| self.expr(a)).collect(),
        }
    }

    /// `@server { ... }` 블록의 자식 문장을 3 갈래로 분류해
    /// [`hir::HirExprKind::Server`] 로 내린다.
    ///
    /// parser 가 `args == [Block]` 로 넘기는 것이 전제 (`parse_server_call`).
    /// 그 외 형태면 `None` 을 돌려 fallback `Domain` 경로로 떨어뜨리고 런타임
    /// 이 "unsupported" 에러로 보고한다.
    ///
    /// 분류 규칙 (advisor 피드백):
    /// - `@listen <expr>` → `listen` 슬롯. 두 번 이상 등장하면 마지막이
    ///   우세하며 진단을 낸다 (SPEC §11.1 은 단일 listen 을 가정).
    /// - `@route METHOD /path { ... }` → `routes` 벡터. 반드시
    ///   [`hir::HirExprKind::Route`] variant 로만 저장한다.
    /// - 그 외 statement (`@out "boot"`, 미들웨어 등) → `body_stmts` 벡터에
    ///   순서를 보존해 담는다. C5b 가 서버 기동 직전에 평가할 예정.
    ///
    /// # 범위 밖
    /// SPEC §11.7 의 중첩 라우트 그룹(`@route /admin { @route ... }`)은 path-
    /// only `@route` 를 parser 가 아직 수용하지 않는다. 이번 커밋 범위 밖.
    fn lower_server(&self, args: &[ast::Expr]) -> Option<hir::HirExprKind> {
        let [body_expr] = args else {
            return None;
        };
        let ast::ExprKind::Block(block) = &body_expr.kind else {
            return None;
        };

        let mut listen: Option<Box<hir::HirExpr>> = None;
        let mut routes: Vec<hir::HirExpr> = Vec::new();
        let mut body_stmts: Vec<hir::HirStmt> = Vec::new();

        for stmt in &block.stmts {
            // `@listen`/`@route` 만 특수 처리. 그 외 stmt 는 body_stmts 에.
            if let ast::Stmt::Expr(expr) = stmt {
                if let ast::ExprKind::Domain { name, args: d_args } = &expr.kind {
                    if name.name == "listen" {
                        // `@listen N` — 첫 인자를 port 표현식으로 사용한다.
                        // 인자가 없거나 여러 개여도 첫 인자만 취하고 나머지
                        // 는 무시 (MVP).
                        let port_expr = match d_args.first() {
                            Some(e) => self.expr(e),
                            None => hir::HirExpr {
                                kind: hir::HirExprKind::Void,
                                ty: hir::Type::Unknown,
                                span: expr.span,
                            },
                        };
                        // 중복 @listen 은 마지막이 우세. 진단은 C5b 의 서버
                        // 기동 시 에러로 엮어 올리는 쪽이 자연스러우므로
                        // 여기서는 조용히 덮어쓴다.
                        listen = Some(Box::new(port_expr));
                        continue;
                    }
                    if name.name == "route" {
                        // A2a: leaf `@route METHOD /path { ... }` 는 단일
                        // Route, group `@route /prefix { @route ... }` 는
                        // 내부를 재귀로 펼쳐 여러 Route 가 된다. 평평한
                        // vector 로 받는다.
                        let flattened = self.flatten_route_args(d_args, "", expr.span);
                        if !flattened.is_empty() {
                            routes.extend(flattened);
                            continue;
                        }
                        // @route 형태가 이상하면 body_stmts 로 흘려보내고
                        // runtime 이 unsupported 로 처리하게 둔다.
                    }
                }
            }
            // 그 외: 원형 그대로 보존.
            body_stmts.push(self.stmt(stmt));
        }

        Some(hir::HirExprKind::Server {
            listen,
            routes,
            body_stmts,
        })
    }

    /// A2a: `@route` 하나를 재귀적으로 펼쳐 flat `HirExpr` 목록으로 만든다.
    ///
    /// - leaf (`method != ""`): 단일 Route variant 반환, path 에 `prefix` 를 앞에 붙임.
    /// - group (`method == ""`): body block 내부의 `@route` 들을 재귀 lower,
    ///   자신의 path 를 prefix 에 이어 붙여 전달. group body 의 비-route
    ///   stmt 는 현재 silent 로 drop — A2-min 은 미들웨어 등을 지원 범위
    ///   밖으로 둔다. C_middleware 마일스톤에서 진단 경로 합류.
    ///
    /// 형식이 이상한 입력(인자 수/타입 불일치)은 빈 벡터. 호출자가 이를
    /// "변환 실패" 로 간주해 body_stmts 로 밀어 넣는다.
    fn flatten_route_args(
        &self,
        args: &[ast::Expr],
        prefix: &str,
        span: orv_diagnostics::Span,
    ) -> Vec<hir::HirExpr> {
        let [method_expr, path_expr, body_expr] = args else {
            return Vec::new();
        };
        let method = match &method_expr.kind {
            ast::ExprKind::String(segs) => match segs.as_slice() {
                [ast::StringSegment::Str(s)] => s.clone(),
                _ => return Vec::new(),
            },
            _ => return Vec::new(),
        };
        let path_raw = match &path_expr.kind {
            ast::ExprKind::String(segs) => match segs.as_slice() {
                [ast::StringSegment::Str(s)] => s.clone(),
                _ => return Vec::new(),
            },
            _ => return Vec::new(),
        };
        let ast::ExprKind::Block(block) = &body_expr.kind else {
            return Vec::new();
        };
        let joined = join_route_paths(prefix, &path_raw);

        if method.is_empty() {
            // Group. body block 의 각 stmt 중 `@route` 만 재귀 unfold.
            let mut out = Vec::new();
            for stmt in &block.stmts {
                if let ast::Stmt::Expr(inner) = stmt {
                    if let ast::ExprKind::Domain {
                        name: inner_name,
                        args: inner_args,
                    } = &inner.kind
                    {
                        if inner_name.name == "route" {
                            out.extend(self.flatten_route_args(
                                inner_args,
                                &joined,
                                inner.span,
                            ));
                        }
                    }
                }
                // 비-route stmt: A2-min 은 silent drop. 미들웨어/let 등의
                // 그룹-레벨 선언은 C_middleware / 후속에서 재논의.
            }
            return out;
        }

        // Leaf.
        vec![hir::HirExpr {
            kind: hir::HirExprKind::Route {
                method,
                method_span: method_expr.span,
                path: joined,
                path_span: path_expr.span,
                handler: self.block(block),
            },
            ty: hir::Type::Unknown,
            span,
        }]
    }

    /// `@route METHOD /path { body }` 를 전용 variant 로 분해한다.
    ///
    /// 파서가 넘긴 3-인자 Domain 은 `[Ident(method), String(path), Block(body)]`
    /// 모양이다. 이 형태가 아니면 `None` 을 돌려 fallback Domain 경로에
    /// 떨어뜨린다 (진단은 상위 계층 몫).
    #[allow(dead_code)]
    fn lower_route(&self, args: &[ast::Expr]) -> Option<hir::HirExprKind> {
        let [method_expr, path_expr, body_expr] = args else {
            return None;
        };
        let method = match &method_expr.kind {
            ast::ExprKind::String(segs) => match segs.as_slice() {
                [ast::StringSegment::Str(s)] => s.clone(),
                _ => return None,
            },
            _ => return None,
        };
        let ast::ExprKind::String(segments) = &path_expr.kind else {
            return None;
        };
        let path = match segments.as_slice() {
            [ast::StringSegment::Str(s)] => s.clone(),
            _ => return None, // path 에 보간이 있으면 이번 커밋 범위 밖.
        };
        let ast::ExprKind::Block(block) = &body_expr.kind else {
            return None;
        };
        Some(hir::HirExprKind::Route {
            method,
            method_span: method_expr.span,
            path,
            path_span: path_expr.span,
            handler: self.block(block),
        })
    }

    /// `@respond <status> <payload>?` 을 전용 variant 로 내린다.
    ///
    /// parser 가 `args` 를 `[status]` 또는 `[status, payload]` 로 넘긴다.
    /// payload 가 빠진 경우(`@respond 204` 등) 여기서 `Void` 를 채워 넣어
    /// 런타임이 항상 같은 모양을 보도록 한다.
    fn lower_respond(&self, origin: &ast::Expr, args: &[ast::Expr]) -> hir::HirExprKind {
        let status = match args.first() {
            Some(e) => self.expr(e),
            None => hir::HirExpr {
                kind: hir::HirExprKind::Void,
                ty: hir::Type::Unknown,
                span: origin.span,
            },
        };
        let payload = match args.get(1) {
            Some(e) => self.expr(e),
            None => hir::HirExpr {
                kind: hir::HirExprKind::Void,
                ty: hir::Type::Unknown,
                span: origin.span,
            },
        };
        hir::HirExprKind::Respond {
            status: Box::new(status),
            payload: Box::new(payload),
        }
    }

    /// `@html` body 를 평범한 HIR 블록으로 내린다.
    ///
    /// `args == [Block]` 이면 그대로 lowering. 관용 규칙: body 가 없거나
    /// block 이 아니면 단일 표현식을 stmt 하나로 감싼 합성 블록을 만든다.
    /// 런타임은 이 블록을 HTML 렌더 모드로 평가한다.
    fn lower_html_body(&self, origin: &ast::Expr, args: &[ast::Expr]) -> hir::HirBlock {
        let Some(first) = args.first() else {
            return hir::HirBlock {
                stmts: Vec::new(),
                span: origin.span,
            };
        };
        if let ast::ExprKind::Block(block) = &first.kind {
            return self.block(block);
        }
        hir::HirBlock {
            stmts: vec![hir::HirStmt::Expr(self.expr(first))],
            span: first.span,
        }
    }

    fn pattern(&self, p: &ast::Pattern) -> hir::HirPattern {
        match p {
            ast::Pattern::Wildcard => hir::HirPattern::Wildcard,
            ast::Pattern::Literal(e) => hir::HirPattern::Literal(self.expr(e)),
            ast::Pattern::Range {
                start,
                end,
                inclusive,
            } => hir::HirPattern::Range {
                start: self.expr(start),
                end: self.expr(end),
                inclusive: *inclusive,
            },
            ast::Pattern::Guard(e) => hir::HirPattern::Guard(self.expr(e)),
            ast::Pattern::Not(e) => hir::HirPattern::Not(self.expr(e)),
            ast::Pattern::Contains(e) => hir::HirPattern::Contains(self.expr(e)),
        }
    }
}

/// A2a: 라우트 path prefix + suffix 합성.
///
/// 규칙:
/// - prefix 가 비어 있으면 suffix 를 그대로 반환 (normalize 포함).
/// - prefix 와 suffix 모두 있으면 `/` 로 join 하되 경계의 중복 `/` 는 축소.
/// - join 결과의 trailing `/` 는 제거 (단 루트 `/` 는 그대로) — runtime 의
///   `normalize_path` 와 동일한 방침.
fn join_route_paths(prefix: &str, suffix: &str) -> String {
    let combined = if prefix.is_empty() {
        suffix.to_string()
    } else {
        let p = prefix.trim_end_matches('/');
        let s = suffix.trim_start_matches('/');
        if s.is_empty() {
            p.to_string()
        } else {
            format!("{p}/{s}")
        }
    };
    if combined == "/" {
        return combined;
    }
    let trimmed = combined.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

fn unary_op(op: ast::UnaryOp) -> hir::UnaryOp {
    match op {
        ast::UnaryOp::Not => hir::UnaryOp::Not,
        ast::UnaryOp::Neg => hir::UnaryOp::Neg,
        ast::UnaryOp::BitNot => hir::UnaryOp::BitNot,
    }
}

fn binary_op(op: ast::BinaryOp) -> hir::BinaryOp {
    use ast::BinaryOp as A;
    use hir::BinaryOp as H;
    match op {
        A::Add => H::Add,
        A::Sub => H::Sub,
        A::Mul => H::Mul,
        A::Div => H::Div,
        A::Rem => H::Rem,
        A::Pow => H::Pow,
        A::Eq => H::Eq,
        A::Ne => H::Ne,
        A::Lt => H::Lt,
        A::Gt => H::Gt,
        A::Le => H::Le,
        A::Ge => H::Ge,
        A::And => H::And,
        A::Or => H::Or,
        A::BitAnd => H::BitAnd,
        A::BitOr => H::BitOr,
        A::BitXor => H::BitXor,
        A::Shl => H::Shl,
        A::Shr => H::Shr,
        A::Coalesce => H::Coalesce,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orv_diagnostics::FileId;
    use orv_syntax::{lex, parse};

    fn lower_src(src: &str) -> hir::HirProgram {
        let lx = lex(src, FileId(0));
        assert!(
            lx.diagnostics.is_empty(),
            "lex errors: {:?}",
            lx.diagnostics
        );
        let pr = parse(lx.tokens, FileId(0));
        assert!(
            pr.diagnostics.is_empty(),
            "parse errors: {:?}",
            pr.diagnostics
        );
        let resolved = orv_resolve::resolve(&pr.program);
        assert!(
            resolved.diagnostics.is_empty(),
            "resolve errors: {:?}",
            resolved.diagnostics
        );
        lower(&pr.program, &resolved)
    }

    #[test]
    fn lower_simple_let() {
        let prog = lower_src("let x: int = 1\n@out x");
        assert_eq!(prog.items.len(), 2);
        assert!(matches!(&prog.items[0], hir::HirStmt::Let(_)));
    }

    #[test]
    fn ident_carries_name_id() {
        let prog = lower_src("let x: int = 1\n@out x");
        let hir::HirStmt::Expr(expr) = &prog.items[1] else {
            panic!("expected expr");
        };
        let hir::HirExprKind::Out(arg) = &expr.kind else {
            panic!("expected Out");
        };
        let hir::HirExprKind::Ident(ident) = &arg.kind else {
            panic!("expected ident");
        };
        // x 의 decl (NameId(0)) 와 참조가 같은 NameId 를 가리켜야 한다.
        assert_eq!(ident.id, NameId(0));
        assert_eq!(ident.name, "x");
    }

    #[test]
    fn function_lowered_with_params() {
        let prog = lower_src("function add(a: int, b: int): int -> a + b");
        let hir::HirStmt::Function(f) = &prog.items[0] else {
            panic!("expected function");
        };
        assert_eq!(f.name.name, "add");
        assert_eq!(f.params.len(), 2);
        assert!(matches!(f.body, hir::HirFunctionBody::Expr(_)));
    }

    #[test]
    fn out_domain_lowered_to_out_variant() {
        let prog = lower_src(r#"@out "hi""#);
        let hir::HirStmt::Expr(expr) = &prog.items[0] else {
            panic!("expected expr");
        };
        assert!(
            matches!(&expr.kind, hir::HirExprKind::Out(_)),
            "expected Out variant, got {:?}",
            expr.kind
        );
    }

    #[test]
    fn empty_out_lowered_to_out_with_void() {
        let prog = lower_src("@out\n");
        let hir::HirStmt::Expr(expr) = &prog.items[0] else {
            panic!("expected expr");
        };
        let hir::HirExprKind::Out(inner) = &expr.kind else {
            panic!("expected Out");
        };
        assert!(matches!(inner.kind, hir::HirExprKind::Void));
    }

    #[test]
    fn html_domain_lowers_to_block() {
        let prog = lower_src(r#"@html { @p "hi" }"#);
        let hir::HirStmt::Expr(expr) = &prog.items[0] else {
            panic!("expected expr");
        };
        let hir::HirExprKind::Html(block) = &expr.kind else {
            panic!("expected Html, got {:?}", expr.kind);
        };
        // body 는 평범한 HIR 블록 — 내부는 기존 Domain/For/If 등 모든 문법.
        assert_eq!(block.stmts.len(), 1);
        assert!(matches!(&block.stmts[0], hir::HirStmt::Expr(_)));
    }

    #[test]
    fn html_domain_supports_nested_elements() {
        let prog = lower_src(r#"@html { @head { @title "t" } @body { @p "hi" } }"#);
        let hir::HirStmt::Expr(expr) = &prog.items[0] else {
            panic!("expected expr");
        };
        let hir::HirExprKind::Html(block) = &expr.kind else {
            panic!("expected Html");
        };
        assert_eq!(block.stmts.len(), 2);
    }

    #[test]
    fn html_domain_allows_for_loop() {
        let prog = lower_src(r#"@html { for i in 0..3 { @li "{i}" } }"#);
        let hir::HirStmt::Expr(expr) = &prog.items[0] else {
            panic!("expected expr");
        };
        let hir::HirExprKind::Html(block) = &expr.kind else {
            panic!("expected Html");
        };
        // body 의 for 는 HirExprKind::For 로 그대로 lowering — HTML 전용
        // variant 없이 기존 제어 흐름을 그대로 사용한다.
        let hir::HirStmt::Expr(stmt_expr) = &block.stmts[0] else {
            panic!("expected expr stmt");
        };
        assert!(matches!(stmt_expr.kind, hir::HirExprKind::For { .. }));
    }

    #[test]
    fn route_lowered_to_route_variant() {
        let prog = lower_src(r#"@route GET /api/users { @out "hi" }"#);
        let hir::HirStmt::Expr(expr) = &prog.items[0] else {
            panic!("expected expr");
        };
        let hir::HirExprKind::Route {
            method,
            path,
            handler,
            ..
        } = &expr.kind
        else {
            panic!("expected Route, got {:?}", expr.kind);
        };
        assert_eq!(method, "GET");
        assert_eq!(path, "/api/users");
        assert!(!handler.stmts.is_empty());
    }

    #[test]
    fn route_with_param_preserves_path_string() {
        let prog = lower_src(r#"@route POST /users/:id { @out "x" }"#);
        let hir::HirStmt::Expr(expr) = &prog.items[0] else {
            panic!("expected expr");
        };
        let hir::HirExprKind::Route { method, path, .. } = &expr.kind else {
            panic!("expected Route");
        };
        assert_eq!(method, "POST");
        assert_eq!(path, "/users/:id");
    }

    #[test]
    fn respond_lowered_to_respond_variant() {
        let prog = lower_src(r#"@respond 201 { id: 42 }"#);
        let hir::HirStmt::Expr(expr) = &prog.items[0] else {
            panic!("expected expr");
        };
        let hir::HirExprKind::Respond { status, payload } = &expr.kind else {
            panic!("expected Respond, got {:?}", expr.kind);
        };
        // status 는 Integer 리터럴 그대로 낮아진다.
        assert!(matches!(status.kind, hir::HirExprKind::Integer(ref n) if n == "201"));
        // payload 는 Object.
        assert!(matches!(payload.kind, hir::HirExprKind::Object(_)));
    }

    #[test]
    fn respond_without_payload_fills_void() {
        let prog = lower_src(r#"@respond 204"#);
        let hir::HirStmt::Expr(expr) = &prog.items[0] else {
            panic!("expected expr");
        };
        let hir::HirExprKind::Respond { status, payload } = &expr.kind else {
            panic!("expected Respond");
        };
        assert!(matches!(status.kind, hir::HirExprKind::Integer(ref n) if n == "204"));
        assert!(matches!(payload.kind, hir::HirExprKind::Void));
    }

    #[test]
    fn unknown_domain_stays_as_domain_variant() {
        // `@foo` 는 아직 전용 variant 가 없으므로 fallback Domain 으로 남는다.
        let prog = lower_src("@foo 1");
        let hir::HirStmt::Expr(expr) = &prog.items[0] else {
            panic!("expected expr");
        };
        match &expr.kind {
            hir::HirExprKind::Domain { name, .. } => assert_eq!(name, "foo"),
            other => panic!("expected Domain fallback, got {other:?}"),
        }
    }

    // --- @server lowering ---

    fn expect_server(
        prog: &hir::HirProgram,
    ) -> (
        &Option<Box<hir::HirExpr>>,
        &Vec<hir::HirExpr>,
        &Vec<hir::HirStmt>,
    ) {
        let hir::HirStmt::Expr(expr) = &prog.items[0] else {
            panic!("expected expr");
        };
        let hir::HirExprKind::Server {
            listen,
            routes,
            body_stmts,
        } = &expr.kind
        else {
            panic!("expected Server variant, got {:?}", expr.kind);
        };
        (listen, routes, body_stmts)
    }

    #[test]
    fn server_empty_block_lowers_to_empty_server() {
        let prog = lower_src("@server {}");
        let (listen, routes, body_stmts) = expect_server(&prog);
        assert!(listen.is_none());
        assert!(routes.is_empty());
        assert!(body_stmts.is_empty());
    }

    #[test]
    fn server_collects_listen_and_routes() {
        let prog = lower_src(
            r#"@server {
                @listen 8080
                @route GET /api { @respond 200 {} }
                @route POST /users { @respond 201 {} }
            }"#,
        );
        let (listen, routes, body_stmts) = expect_server(&prog);

        // listen 은 Integer 리터럴 표현식으로 저장된다.
        let listen = listen.as_ref().expect("listen slot should be populated");
        assert!(matches!(listen.kind, hir::HirExprKind::Integer(ref n) if n == "8080"));

        // routes 는 Route variant 2 개.
        assert_eq!(routes.len(), 2);
        for r in routes {
            assert!(matches!(r.kind, hir::HirExprKind::Route { .. }));
        }

        // 그 외 stmt 없음.
        assert!(body_stmts.is_empty());
    }

    #[test]
    fn server_preserves_misc_stmts_in_body_stmts() {
        // SPEC §11.1 예제: `@out "서버 시작..."` 같은 기타 도메인이 server
        // 블록 안에 올 수 있다. lower_server 는 이를 body_stmts 에 순서대로
        // 보존해야 한다 (drop/reject 금지).
        let prog = lower_src(
            r#"@server {
                @out "boot"
                @listen 3000
                @route GET /health { @respond 200 {} }
                @out "ready"
            }"#,
        );
        let (listen, routes, body_stmts) = expect_server(&prog);

        let listen = listen.as_ref().expect("listen should be present");
        assert!(matches!(listen.kind, hir::HirExprKind::Integer(ref n) if n == "3000"));
        assert_eq!(routes.len(), 1);
        // @out 두 개가 body_stmts 에 순서대로 보존.
        assert_eq!(body_stmts.len(), 2);
        for stmt in body_stmts {
            let hir::HirStmt::Expr(expr) = stmt else {
                panic!("expected expr stmt in body_stmts");
            };
            assert!(matches!(expr.kind, hir::HirExprKind::Out(_)));
        }
    }

    #[test]
    fn server_with_duplicate_listen_keeps_last() {
        // @listen 이 중복되면 마지막이 우세. 분석기는 진단 없이 덮어쓴다
        // (C5b 서버 기동 시점에 엄밀 진단을 낼지 재검토).
        let prog = lower_src(
            r#"@server {
                @listen 8080
                @listen 9090
            }"#,
        );
        let (listen, _, _) = expect_server(&prog);
        let listen = listen.as_ref().expect("listen should be present");
        assert!(matches!(listen.kind, hir::HirExprKind::Integer(ref n) if n == "9090"));
    }

    #[test]
    fn expr_slot_has_unknown_type() {
        let prog = lower_src("let x: int = 1 + 2");
        let hir::HirStmt::Let(l) = &prog.items[0] else {
            panic!("expected let");
        };
        assert_eq!(l.init.ty, hir::Type::Unknown);
    }

    // --- A2a: nested route groups ---

    /// 그룹 Route 의 method/path 를 문자열로 돌려준다.
    fn route_method_path(expr: &hir::HirExpr) -> (String, String) {
        let hir::HirExprKind::Route { method, path, .. } = &expr.kind else {
            panic!("expected Route variant, got {:?}", expr.kind);
        };
        (method.clone(), path.clone())
    }

    #[test]
    fn nested_routes_flatten_with_path_prefix() {
        // SPEC §11.7: `@route /prefix { @route METHOD /suffix { ... } }` 는
        // `/prefix/suffix` 로 평평화된다. analyzer 수준에서 unfold 하므로
        // runtime/HIR 에는 flat Route 만 들어간다.
        let prog = lower_src(
            r#"@server {
                @listen 8080
                @route /admin {
                    @route GET /users { @respond 200 {} }
                    @route DELETE /users/:id { @respond 204 {} }
                }
            }"#,
        );
        let (_, routes, _) = expect_server(&prog);
        assert_eq!(routes.len(), 2);
        let (m1, p1) = route_method_path(&routes[0]);
        let (m2, p2) = route_method_path(&routes[1]);
        assert_eq!((m1.as_str(), p1.as_str()), ("GET", "/admin/users"));
        assert_eq!(
            (m2.as_str(), p2.as_str()),
            ("DELETE", "/admin/users/:id")
        );
    }

    #[test]
    fn nested_routes_allow_empty_suffix_for_prefix_itself() {
        // `@route /admin { @route GET / { ... } }` 는 그룹 prefix 자체
        // (`/admin`) 를 매칭한다. trailing `/` 는 정규화된다.
        let prog = lower_src(
            r#"@server {
                @listen 8080
                @route /admin {
                    @route GET / { @respond 200 {} }
                }
            }"#,
        );
        let (_, routes, _) = expect_server(&prog);
        assert_eq!(routes.len(), 1);
        let (m, p) = route_method_path(&routes[0]);
        assert_eq!((m.as_str(), p.as_str()), ("GET", "/admin"));
    }

    #[test]
    fn nested_groups_support_unlimited_depth() {
        // 3단 중첩도 재귀 unfold 되어야 한다.
        let prog = lower_src(
            r#"@server {
                @listen 8080
                @route /api {
                    @route /v1 {
                        @route GET /ping { @respond 200 {} }
                    }
                }
            }"#,
        );
        let (_, routes, _) = expect_server(&prog);
        assert_eq!(routes.len(), 1);
        let (m, p) = route_method_path(&routes[0]);
        assert_eq!((m.as_str(), p.as_str()), ("GET", "/api/v1/ping"));
    }
}
