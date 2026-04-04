use orv_hir::SymbolRef;
use orv_resolve::{ResolveResult, ScopeId, ScopeKind};
use orv_syntax::ast::{
    self, Expr as AstExpr, Item as AstItem, Stmt as AstStmt, StringPart, TypeExpr,
};

pub(crate) fn lower_module(result: &ResolveResult, module: &ast::Module) -> orv_hir::Module {
    HirLowerer::new(result).lower_module(module)
}

struct HirLowerer<'a> {
    result: &'a ResolveResult,
    current_scope: ScopeId,
    next_scope_index: usize,
}

impl<'a> HirLowerer<'a> {
    fn new(result: &'a ResolveResult) -> Self {
        Self {
            result,
            current_scope: result.root_scope,
            next_scope_index: 1,
        }
    }

    fn lower_module(&mut self, module: &ast::Module) -> orv_hir::Module {
        orv_hir::Module {
            items: module
                .items
                .iter()
                .map(|item| self.lower_item(item.node()))
                .collect(),
        }
    }

    fn lower_item(&mut self, item: &AstItem) -> orv_hir::Item {
        match item {
            AstItem::Import(import) => {
                let binding_name = import
                    .alias
                    .as_ref()
                    .map(|alias| alias.node().as_str())
                    .or_else(|| import.names.first().map(|name| name.node().as_str()))
                    .or_else(|| import.path.last().map(|name| name.node().as_str()));

                orv_hir::Item {
                    symbol: binding_name.and_then(|name| self.top_level_symbol(name)),
                    kind: orv_hir::ItemKind::Import(orv_hir::ImportItem {
                        path: import.path.iter().map(spanned_string).collect(),
                        names: import.names.iter().map(spanned_string).collect(),
                        alias: import.alias.as_ref().map(spanned_string),
                    }),
                }
            }
            AstItem::Function(function) => orv_hir::Item {
                symbol: self.top_level_symbol(function.name.node()),
                kind: orv_hir::ItemKind::Function(self.lower_function(function)),
            },
            AstItem::Define(define) => orv_hir::Item {
                symbol: self.top_level_symbol(define.name.node()),
                kind: orv_hir::ItemKind::Define(self.lower_define(define)),
            },
            AstItem::Struct(item) => orv_hir::Item {
                symbol: self.top_level_symbol(item.name.node()),
                kind: orv_hir::ItemKind::Struct(orv_hir::StructItem {
                    name: item.name.node().clone(),
                    is_pub: item.is_pub,
                    fields: item
                        .fields
                        .iter()
                        .map(|field| orv_hir::StructField {
                            name: field.node().name.node().clone(),
                            ty: lower_type(&field.node().ty),
                        })
                        .collect(),
                }),
            },
            AstItem::Enum(item) => orv_hir::Item {
                symbol: self.top_level_symbol(item.name.node()),
                kind: orv_hir::ItemKind::Enum(orv_hir::EnumItem {
                    name: item.name.node().clone(),
                    is_pub: item.is_pub,
                    variants: item
                        .variants
                        .iter()
                        .map(|variant| orv_hir::EnumVariant {
                            name: variant.node().name.node().clone(),
                            fields: variant.node().fields.iter().map(lower_type).collect(),
                        })
                        .collect(),
                }),
            },
            AstItem::TypeAlias(item) => orv_hir::Item {
                symbol: self.top_level_symbol(item.name.node()),
                kind: orv_hir::ItemKind::TypeAlias(orv_hir::TypeAliasItem {
                    name: item.name.node().clone(),
                    is_pub: item.is_pub,
                    ty: lower_type(&item.ty),
                }),
            },
            AstItem::Binding(binding) => orv_hir::Item {
                symbol: self.top_level_symbol(binding.name.node()),
                kind: orv_hir::ItemKind::Binding(self.lower_binding(binding)),
            },
            AstItem::Stmt(stmt) => orv_hir::Item {
                symbol: None,
                kind: orv_hir::ItemKind::Stmt(self.lower_stmt(stmt)),
            },
            AstItem::Error => orv_hir::Item {
                symbol: None,
                kind: orv_hir::ItemKind::Error,
            },
        }
    }

    fn lower_function(&mut self, function: &ast::FunctionItem) -> orv_hir::FunctionItem {
        let scope = self.take_scope(ScopeKind::Function);
        self.with_scope(scope, |this| orv_hir::FunctionItem {
            name: function.name.node().clone(),
            is_pub: function.is_pub,
            is_async: function.is_async,
            scope: scope.raw(),
            params: function
                .params
                .iter()
                .map(|param| this.lower_param(param))
                .collect(),
            return_type: function.return_type.as_ref().map(lower_type),
            body: this.lower_expr(&function.body),
        })
    }

    fn lower_define(&mut self, define: &ast::DefineItem) -> orv_hir::DefineItem {
        let scope = self.take_scope(ScopeKind::Define);
        self.with_scope(scope, |this| orv_hir::DefineItem {
            name: define.name.node().clone(),
            is_pub: define.is_pub,
            scope: scope.raw(),
            params: define
                .params
                .iter()
                .map(|param| this.lower_param(param))
                .collect(),
            return_domain: define
                .return_domain
                .as_ref()
                .map(|name| name.node().to_string()),
            body: this.lower_expr(&define.body),
        })
    }

    fn lower_param(&mut self, param: &orv_span::Spanned<ast::Param>) -> orv_hir::Param {
        orv_hir::Param {
            symbol: self.lookup_local_symbol(self.current_scope, param.node().name.node()),
            name: param.node().name.node().clone(),
            ty: param.node().ty.as_ref().map(lower_type),
            default: param
                .node()
                .default
                .as_ref()
                .map(|expr| self.lower_expr(expr)),
        }
    }

    fn lower_binding(&mut self, binding: &ast::BindingStmt) -> orv_hir::Binding {
        orv_hir::Binding {
            symbol: self.lookup_local_symbol(self.current_scope, binding.name.node()),
            name: binding.name.node().clone(),
            is_pub: binding.is_pub,
            is_const: binding.is_const,
            is_mut: binding.is_mut,
            is_sig: binding.is_sig,
            ty: binding.ty.as_ref().map(lower_type),
            value: binding.value.as_ref().map(|expr| self.lower_expr(expr)),
        }
    }

    fn lower_stmt(&mut self, stmt: &AstStmt) -> orv_hir::Stmt {
        match stmt {
            AstStmt::Binding(binding) => orv_hir::Stmt::Binding(self.lower_binding(binding)),
            AstStmt::Return(expr) => {
                orv_hir::Stmt::Return(expr.as_ref().map(|expr| self.lower_expr(expr)))
            }
            AstStmt::If(if_stmt) => {
                let condition = self.lower_expr(&if_stmt.condition);
                let then_scope = self.take_scope(ScopeKind::IfBranch);
                let then_body =
                    self.with_scope(then_scope, |this| this.lower_expr(&if_stmt.then_body));
                let (else_scope, else_body) = if let Some(body) = &if_stmt.else_body {
                    let scope = self.take_scope(ScopeKind::IfBranch);
                    let lowered = self.with_scope(scope, |this| this.lower_expr(body));
                    (Some(scope.raw()), Some(lowered))
                } else {
                    (None, None)
                };

                orv_hir::Stmt::If(orv_hir::IfStmt {
                    condition,
                    then_scope: then_scope.raw(),
                    then_body,
                    else_scope,
                    else_body,
                })
            }
            AstStmt::For(for_stmt) => {
                let iterable = self.lower_expr(&for_stmt.iterable);
                let scope = self.take_scope(ScopeKind::ForLoop);
                let binding_symbol = self.lookup_local_symbol(scope, for_stmt.binding.node());
                let binding = for_stmt.binding.node().clone();
                let body = self.with_scope(scope, |this| this.lower_expr(&for_stmt.body));

                orv_hir::Stmt::For(orv_hir::ForStmt {
                    scope: scope.raw(),
                    binding,
                    binding_symbol,
                    iterable,
                    body,
                })
            }
            AstStmt::While(while_stmt) => {
                let condition = self.lower_expr(&while_stmt.condition);
                let scope = self.take_scope(ScopeKind::WhileLoop);
                let body = self.with_scope(scope, |this| this.lower_expr(&while_stmt.body));

                orv_hir::Stmt::While(orv_hir::WhileStmt {
                    scope: scope.raw(),
                    condition,
                    body,
                })
            }
            AstStmt::Expr(expr) => orv_hir::Stmt::Expr(self.lower_expr(expr)),
            AstStmt::Error => orv_hir::Stmt::Error,
        }
    }

    fn lower_expr(&mut self, expr: &orv_span::Spanned<AstExpr>) -> orv_hir::Expr {
        match expr.node() {
            AstExpr::IntLiteral(value) => orv_hir::Expr::IntLiteral(*value),
            AstExpr::FloatLiteral(value) => orv_hir::Expr::FloatLiteral(*value),
            AstExpr::StringLiteral(value) => orv_hir::Expr::StringLiteral(value.clone()),
            AstExpr::StringInterp(parts) => orv_hir::Expr::StringInterp(
                parts
                    .iter()
                    .map(|part| match part {
                        StringPart::Lit(value) => orv_hir::StringPart::Lit(value.clone()),
                        StringPart::Expr(expr) => orv_hir::StringPart::Expr(self.lower_expr(expr)),
                    })
                    .collect(),
            ),
            AstExpr::BoolLiteral(value) => orv_hir::Expr::BoolLiteral(*value),
            AstExpr::Void => orv_hir::Expr::Void,
            AstExpr::Ident(name) => orv_hir::Expr::Ident(orv_hir::ResolvedName {
                name: name.clone(),
                symbol: self.lookup_symbol(self.current_scope, name),
            }),
            AstExpr::Binary { left, op, right } => orv_hir::Expr::Binary {
                left: Box::new(self.lower_expr(left)),
                op: lower_binary_op(*op.node()),
                right: Box::new(self.lower_expr(right)),
            },
            AstExpr::Unary { op, operand } => orv_hir::Expr::Unary {
                op: lower_unary_op(*op.node()),
                operand: Box::new(self.lower_expr(operand)),
            },
            AstExpr::Assign { target, op, value } => orv_hir::Expr::Assign {
                target: Box::new(self.lower_expr(target)),
                op: lower_assign_op(*op.node()),
                value: Box::new(self.lower_expr(value)),
            },
            AstExpr::Call { callee, args } => orv_hir::Expr::Call {
                callee: Box::new(self.lower_expr(callee)),
                args: args
                    .iter()
                    .map(|arg| orv_hir::CallArg {
                        name: arg.node().name.as_ref().map(spanned_string),
                        value: self.lower_expr(&arg.node().value),
                    })
                    .collect(),
            },
            AstExpr::Field { object, field } => orv_hir::Expr::Field {
                object: Box::new(self.lower_expr(object)),
                field: field.node().clone(),
            },
            AstExpr::Index { object, index } => orv_hir::Expr::Index {
                object: Box::new(self.lower_expr(object)),
                index: Box::new(self.lower_expr(index)),
            },
            AstExpr::Block(stmts) => {
                let scope = self.take_scope(ScopeKind::Block);
                let stmts = self.with_scope(scope, |this| {
                    stmts
                        .iter()
                        .map(|stmt| this.lower_stmt(stmt.node()))
                        .collect()
                });
                orv_hir::Expr::Block {
                    scope: scope.raw(),
                    stmts,
                }
            }
            AstExpr::When { subject, arms } => orv_hir::Expr::When {
                subject: Box::new(self.lower_expr(subject)),
                arms: arms
                    .iter()
                    .map(|arm| {
                        let scope = self.take_scope(ScopeKind::WhenArm);
                        let pattern = lower_pattern(&arm.node().pattern);
                        let body = self.with_scope(scope, |this| this.lower_expr(&arm.node().body));
                        orv_hir::WhenArm {
                            scope: scope.raw(),
                            pattern,
                            body,
                        }
                    })
                    .collect(),
            },
            AstExpr::Object(fields) => orv_hir::Expr::Object(
                fields
                    .iter()
                    .map(|field| orv_hir::ObjectField {
                        key: field.node().key.node().clone(),
                        value: self.lower_expr(&field.node().value),
                    })
                    .collect(),
            ),
            AstExpr::Array(items) => {
                orv_hir::Expr::Array(items.iter().map(|item| self.lower_expr(item)).collect())
            }
            AstExpr::Map(fields) => orv_hir::Expr::Map(
                fields
                    .iter()
                    .map(|field| orv_hir::ObjectField {
                        key: field.node().key.node().clone(),
                        value: self.lower_expr(&field.node().value),
                    })
                    .collect(),
            ),
            AstExpr::Node(node) => orv_hir::Expr::Node(orv_hir::NodeExpr {
                name: node.name.node().to_string(),
                positional: node
                    .positional
                    .iter()
                    .map(|expr| self.lower_expr(expr))
                    .collect(),
                properties: node
                    .properties
                    .iter()
                    .map(|property| orv_hir::Property {
                        name: property.node().name.node().clone(),
                        value: self.lower_expr(&property.node().value),
                    })
                    .collect(),
                body: node
                    .body
                    .as_ref()
                    .map(|body| Box::new(self.lower_expr(body))),
            }),
            AstExpr::Paren(inner) => orv_hir::Expr::Paren(Box::new(self.lower_expr(inner))),
            AstExpr::Await(inner) => orv_hir::Expr::Await(Box::new(self.lower_expr(inner))),
            AstExpr::Error => orv_hir::Expr::Error,
        }
    }

    fn top_level_symbol(&self, name: &str) -> Option<SymbolRef> {
        self.lookup_local_symbol(self.result.root_scope, name)
    }

    fn lookup_symbol(&self, scope: ScopeId, name: &str) -> Option<SymbolRef> {
        self.result
            .scopes
            .lookup(scope, name)
            .map(|symbol| symbol.raw())
    }

    fn lookup_local_symbol(&self, scope: ScopeId, name: &str) -> Option<SymbolRef> {
        self.result
            .scopes
            .lookup_local(scope, name)
            .map(|symbol| symbol.raw())
    }

    fn take_scope(&mut self, expected_kind: ScopeKind) -> ScopeId {
        let scope = self
            .result
            .scopes
            .scope_id_at(self.next_scope_index)
            .expect("missing scope during HIR lowering");
        let actual_kind = self.result.scopes.get(scope).kind();
        assert_eq!(
            actual_kind, expected_kind,
            "scope order drift during HIR lowering: expected {expected_kind:?}, found {actual_kind:?}"
        );
        self.next_scope_index += 1;
        scope
    }

    fn with_scope<T>(&mut self, scope: ScopeId, f: impl FnOnce(&mut Self) -> T) -> T {
        let previous = self.current_scope;
        self.current_scope = scope;
        let value = f(self);
        self.current_scope = previous;
        value
    }
}

fn lower_type(ty: &orv_span::Spanned<TypeExpr>) -> orv_hir::Type {
    match ty.node() {
        TypeExpr::Named(name) => orv_hir::Type::Named(name.clone()),
        TypeExpr::Nullable(inner) => orv_hir::Type::Nullable(Box::new(lower_type(inner))),
        TypeExpr::Generic { name, args } => orv_hir::Type::Generic {
            name: name.node().clone(),
            args: args.iter().map(lower_type).collect(),
        },
        TypeExpr::Function { params, ret } => orv_hir::Type::Function {
            params: params.iter().map(lower_type).collect(),
            ret: Box::new(lower_type(ret)),
        },
        TypeExpr::Node(name) => orv_hir::Type::Node(name.node().to_string()),
        TypeExpr::Error => orv_hir::Type::Error,
    }
}

fn lower_binary_op(op: ast::BinOp) -> orv_hir::BinaryOp {
    match op {
        ast::BinOp::Add => orv_hir::BinaryOp::Add,
        ast::BinOp::Sub => orv_hir::BinaryOp::Sub,
        ast::BinOp::Mul => orv_hir::BinaryOp::Mul,
        ast::BinOp::Div => orv_hir::BinaryOp::Div,
        ast::BinOp::Eq => orv_hir::BinaryOp::Eq,
        ast::BinOp::NotEq => orv_hir::BinaryOp::NotEq,
        ast::BinOp::Lt => orv_hir::BinaryOp::Lt,
        ast::BinOp::LtEq => orv_hir::BinaryOp::LtEq,
        ast::BinOp::Gt => orv_hir::BinaryOp::Gt,
        ast::BinOp::GtEq => orv_hir::BinaryOp::GtEq,
        ast::BinOp::And => orv_hir::BinaryOp::And,
        ast::BinOp::Or => orv_hir::BinaryOp::Or,
        ast::BinOp::Pipe => orv_hir::BinaryOp::Pipe,
    }
}

fn lower_unary_op(op: ast::UnaryOp) -> orv_hir::UnaryOp {
    match op {
        ast::UnaryOp::Neg => orv_hir::UnaryOp::Neg,
        ast::UnaryOp::Not => orv_hir::UnaryOp::Not,
    }
}

fn lower_assign_op(op: ast::AssignOp) -> orv_hir::AssignOp {
    match op {
        ast::AssignOp::Assign => orv_hir::AssignOp::Assign,
        ast::AssignOp::AddAssign => orv_hir::AssignOp::AddAssign,
        ast::AssignOp::SubAssign => orv_hir::AssignOp::SubAssign,
    }
}

fn lower_pattern(pattern: &orv_span::Spanned<ast::Pattern>) -> orv_hir::Pattern {
    match pattern.node() {
        ast::Pattern::Wildcard => orv_hir::Pattern::Wildcard,
        ast::Pattern::Binding(name) => orv_hir::Pattern::Binding(name.clone()),
        ast::Pattern::IntLiteral(value) => orv_hir::Pattern::IntLiteral(*value),
        ast::Pattern::FloatLiteral(value) => orv_hir::Pattern::FloatLiteral(*value),
        ast::Pattern::StringLiteral(value) => orv_hir::Pattern::StringLiteral(value.clone()),
        ast::Pattern::BoolLiteral(value) => orv_hir::Pattern::BoolLiteral(*value),
        ast::Pattern::Void => orv_hir::Pattern::Void,
        ast::Pattern::Variant { path, fields } => orv_hir::Pattern::Variant {
            path: path.iter().map(spanned_string).collect(),
            fields: fields.iter().map(lower_pattern).collect(),
        },
        ast::Pattern::Error => orv_hir::Pattern::Error,
    }
}

fn spanned_string(value: &orv_span::Spanned<String>) -> String {
    value.node().clone()
}
