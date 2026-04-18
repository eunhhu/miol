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
        hir::HirExprKind::Domain {
            name: name.name.clone(),
            name_span: name.span,
            args: args.iter().map(|a| self.expr(a)).collect(),
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
        }
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
        assert!(lx.diagnostics.is_empty(), "lex errors: {:?}", lx.diagnostics);
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

    #[test]
    fn expr_slot_has_unknown_type() {
        let prog = lower_src("let x: int = 1 + 2");
        let hir::HirStmt::Let(l) = &prog.items[0] else {
            panic!("expected let");
        };
        assert_eq!(l.init.ty, hir::Type::Unknown);
    }
}
