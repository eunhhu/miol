use std::collections::{BTreeMap, HashMap, HashSet};

use orv_diagnostics::{Diagnostic, DiagnosticBag, Label};
use orv_span::Spanned;
use orv_syntax::ast::{
    AssignOp, BindingStmt, EnumItem, Expr, FunctionItem, Item, Module, NodeExpr, Pattern, Stmt,
    StructItem, TypeExpr,
};

pub fn type_check(module: &Module) -> DiagnosticBag {
    let registry = Registry::new(module);
    let mut checker = TypeChecker::new(registry);
    checker.check_module(module);
    checker.diagnostics
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Ty {
    Unknown,
    Error,
    Void,
    Bool,
    String,
    Int(String),
    Float(String),
    Named(String),
    Struct(String),
    Enum(String),
    Nullable(Box<Ty>),
    Vec(Box<Ty>),
    HashMap(Box<Ty>, Box<Ty>),
    Function { params: Vec<Ty>, ret: Box<Ty> },
    Node(String),
    Object(BTreeMap<String, Ty>),
}

impl Ty {
    fn display(&self) -> String {
        match self {
            Self::Unknown => "<unknown>".to_owned(),
            Self::Error => "<error>".to_owned(),
            Self::Void => "void".to_owned(),
            Self::Bool => "bool".to_owned(),
            Self::String => "string".to_owned(),
            Self::Int(name)
            | Self::Float(name)
            | Self::Named(name)
            | Self::Struct(name)
            | Self::Enum(name) => name.clone(),
            Self::Nullable(inner) => format!("{}?", inner.display()),
            Self::Vec(inner) => format!("Vec<{}>", inner.display()),
            Self::HashMap(key, value) => {
                format!("HashMap<{}, {}>", key.display(), value.display())
            }
            Self::Function { params, ret } => format!(
                "({}) -> {}",
                params
                    .iter()
                    .map(Self::display)
                    .collect::<Vec<_>>()
                    .join(", "),
                ret.display()
            ),
            Self::Node(name) => format!("@{name}"),
            Self::Object(fields) => format!(
                "{{ {} }}",
                fields
                    .iter()
                    .map(|(key, value)| format!("{key}: {}", value.display()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    fn is_numeric(&self) -> bool {
        matches!(self, Self::Int(_) | Self::Float(_))
    }
}

#[derive(Debug, Clone)]
struct ParamSig {
    name: String,
    ty: Ty,
    has_default: bool,
}

#[derive(Debug, Clone)]
struct FunctionSig {
    params: Vec<ParamSig>,
    ret: Ty,
}

#[derive(Debug, Clone)]
struct StructSpec {
    fields: BTreeMap<String, Ty>,
}

#[derive(Debug, Clone)]
struct EnumVariantSpec {
    fields: Vec<Ty>,
}

#[derive(Debug, Clone)]
struct EnumSpec {
    variants: HashMap<String, EnumVariantSpec>,
}

struct Registry<'a> {
    structs: HashMap<String, &'a StructItem>,
    enums: HashMap<String, &'a EnumItem>,
    aliases: HashMap<String, &'a Spanned<TypeExpr>>,
    functions: HashMap<String, FunctionSig>,
}

impl<'a> Registry<'a> {
    fn new(module: &'a Module) -> Self {
        let mut structs = HashMap::new();
        let mut enums = HashMap::new();
        let mut aliases = HashMap::new();
        for item in &module.items {
            match item.node() {
                Item::Struct(struct_item) => {
                    structs.insert(struct_item.name.node().clone(), struct_item);
                }
                Item::Enum(enum_item) => {
                    enums.insert(enum_item.name.node().clone(), enum_item);
                }
                Item::TypeAlias(alias) => {
                    aliases.insert(alias.name.node().clone(), &alias.ty);
                }
                _ => {}
            }
        }

        let mut registry = Self {
            structs,
            enums,
            aliases,
            functions: HashMap::new(),
        };
        registry.collect_functions(module);
        registry
    }

    fn collect_functions(&mut self, module: &Module) {
        for item in &module.items {
            if let Item::Function(function) = item.node() {
                let params = function
                    .params
                    .iter()
                    .map(|param| ParamSig {
                        name: param.node().name.node().clone(),
                        ty: param
                            .node()
                            .ty
                            .as_ref()
                            .map_or(Ty::Unknown, |ty| self.resolve_type_expr(ty.node())),
                        has_default: param.node().default.is_some(),
                    })
                    .collect();
                let ret = function
                    .return_type
                    .as_ref()
                    .map_or(Ty::Unknown, |ty| self.resolve_type_expr(ty.node()));
                self.functions
                    .insert(function.name.node().clone(), FunctionSig { params, ret });
            }
        }
    }

    fn resolve_type_expr(&self, expr: &TypeExpr) -> Ty {
        self.resolve_type_expr_inner(expr, &mut HashSet::new())
    }

    fn resolve_type_expr_inner(&self, expr: &TypeExpr, visiting: &mut HashSet<String>) -> Ty {
        match expr {
            TypeExpr::Named(name) => self.resolve_named_type(name, visiting),
            TypeExpr::Nullable(inner) => Ty::Nullable(Box::new(
                self.resolve_type_expr_inner(inner.node(), visiting),
            )),
            TypeExpr::Generic { name, args } if name.node() == "Vec" && args.len() == 1 => Ty::Vec(
                Box::new(self.resolve_type_expr_inner(args[0].node(), visiting)),
            ),
            TypeExpr::Generic { name, args } if name.node() == "HashMap" && args.len() == 2 => {
                Ty::HashMap(
                    Box::new(self.resolve_type_expr_inner(args[0].node(), visiting)),
                    Box::new(self.resolve_type_expr_inner(args[1].node(), visiting)),
                )
            }
            TypeExpr::Generic { name, args } => Ty::Named(format!(
                "{}<{}>",
                name.node(),
                args.iter()
                    .map(|arg| self.resolve_type_expr_inner(arg.node(), visiting).display())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            TypeExpr::Function { params, ret } => Ty::Function {
                params: params
                    .iter()
                    .map(|param| self.resolve_type_expr_inner(param.node(), visiting))
                    .collect(),
                ret: Box::new(self.resolve_type_expr_inner(ret.node(), visiting)),
            },
            TypeExpr::Node(name) => Ty::Node(name.node().to_string()),
            TypeExpr::Error => Ty::Error,
        }
    }

    fn resolve_named_type(&self, name: &str, visiting: &mut HashSet<String>) -> Ty {
        if name == "string" {
            return Ty::String;
        }
        if name == "bool" {
            return Ty::Bool;
        }
        if name == "void" {
            return Ty::Void;
        }
        if is_int_type(name) {
            return Ty::Int(name.to_owned());
        }
        if is_float_type(name) {
            return Ty::Float(name.to_owned());
        }
        if self.structs.contains_key(name) {
            return Ty::Struct(name.to_owned());
        }
        if self.enums.contains_key(name) {
            return Ty::Enum(name.to_owned());
        }
        if let Some(alias) = self.aliases.get(name) {
            if !visiting.insert(name.to_owned()) {
                return Ty::Error;
            }
            let ty = self.resolve_type_expr_inner(alias.node(), visiting);
            visiting.remove(name);
            return ty;
        }
        Ty::Named(name.to_owned())
    }

    fn struct_spec(&self, name: &str) -> Option<StructSpec> {
        let struct_item = self.structs.get(name)?;
        let fields = struct_item
            .fields
            .iter()
            .map(|field| {
                (
                    field.node().name.node().clone(),
                    self.resolve_type_expr(field.node().ty.node()),
                )
            })
            .collect();
        Some(StructSpec { fields })
    }

    fn function_sig(&self, name: &str) -> Option<&FunctionSig> {
        self.functions.get(name)
    }

    fn enum_spec(&self, name: &str) -> Option<EnumSpec> {
        let enum_item = self.enums.get(name)?;
        let variants = enum_item
            .variants
            .iter()
            .map(|variant| {
                (
                    variant.node().name.node().clone(),
                    EnumVariantSpec {
                        fields: variant
                            .node()
                            .fields
                            .iter()
                            .map(|field| self.resolve_type_expr(field.node()))
                            .collect(),
                    },
                )
            })
            .collect();
        Some(EnumSpec { variants })
    }
}

struct TypeChecker<'a> {
    registry: Registry<'a>,
    diagnostics: DiagnosticBag,
    scopes: Vec<HashMap<String, Ty>>,
}

#[derive(Debug, Clone)]
struct Narrowing {
    name: String,
    then_ty: Ty,
    else_ty: Ty,
}

enum PatternCoverage {
    Wildcard,
    Variant(String),
    Other,
}

impl<'a> TypeChecker<'a> {
    fn new(registry: Registry<'a>) -> Self {
        Self {
            registry,
            diagnostics: DiagnosticBag::new(),
            scopes: vec![HashMap::new()],
        }
    }

    fn check_module(&mut self, module: &Module) {
        self.predeclare_top_level_bindings(module);
        for item in &module.items {
            self.check_item(item.node());
        }
    }

    fn predeclare_top_level_bindings(&mut self, module: &Module) {
        for item in &module.items {
            match item.node() {
                Item::Binding(binding) => {
                    let ty = binding
                        .ty
                        .as_ref()
                        .map_or(Ty::Unknown, |ty| self.registry.resolve_type_expr(ty.node()));
                    self.bind(binding.name.node().clone(), ty);
                }
                Item::Function(function) => {
                    if let Some(sig) = self.registry.function_sig(function.name.node()) {
                        self.bind(
                            function.name.node().clone(),
                            Ty::Function {
                                params: sig.params.iter().map(|param| param.ty.clone()).collect(),
                                ret: Box::new(sig.ret.clone()),
                            },
                        );
                    }
                }
                _ => {}
            }
        }
    }

    fn check_item(&mut self, item: &Item) {
        match item {
            Item::Function(function) => self.check_function(function),
            Item::Define(define) => self.check_define(define),
            Item::Binding(binding) => self.check_binding(binding),
            Item::Stmt(stmt) => {
                self.check_stmt(stmt, None);
            }
            Item::Import(_)
            | Item::Struct(_)
            | Item::Enum(_)
            | Item::TypeAlias(_)
            | Item::Error => {}
        }
    }

    fn check_function(&mut self, function: &FunctionItem) {
        let declared_return = function
            .return_type
            .as_ref()
            .map(|ty| self.registry.resolve_type_expr(ty.node()));
        self.push_scope();
        for param in &function.params {
            let param_ty = param
                .node()
                .ty
                .as_ref()
                .map_or(Ty::Unknown, |ty| self.registry.resolve_type_expr(ty.node()));
            self.bind(param.node().name.node().clone(), param_ty.clone());
            if let Some(default) = &param.node().default {
                let default_ty = self.infer_expr(default, Some(&param_ty));
                self.expect_assignable(default.span(), &param_ty, &default_ty);
            }
        }

        let body_ty = self.infer_expr(&function.body, declared_return.as_ref());
        if let Some(expected) = declared_return {
            self.expect_assignable(function.body.span(), &expected, &body_ty);
        }
        self.pop_scope();
    }

    fn check_define(&mut self, define: &orv_syntax::ast::DefineItem) {
        self.push_scope();
        for param in &define.params {
            let param_ty = param
                .node()
                .ty
                .as_ref()
                .map_or(Ty::Unknown, |ty| self.registry.resolve_type_expr(ty.node()));
            self.bind(param.node().name.node().clone(), param_ty.clone());
            if let Some(default) = &param.node().default {
                let default_ty = self.infer_expr(default, Some(&param_ty));
                self.expect_assignable(default.span(), &param_ty, &default_ty);
            }
        }
        let _ = self.infer_expr(&define.body, None);
        self.pop_scope();
    }

    fn check_binding(&mut self, binding: &BindingStmt) {
        let declared = binding
            .ty
            .as_ref()
            .map(|ty| self.registry.resolve_type_expr(ty.node()));
        let inferred = binding
            .value
            .as_ref()
            .map(|value| self.infer_expr(value, declared.as_ref()))
            .unwrap_or(Ty::Unknown);
        if let Some(expected) = &declared {
            if let Some(value) = &binding.value {
                self.expect_assignable(value.span(), expected, &inferred);
            }
        }
        self.bind(binding.name.node().clone(), declared.unwrap_or(inferred));
    }

    fn check_stmt(&mut self, stmt: &Stmt, expected: Option<&Ty>) -> Ty {
        match stmt {
            Stmt::Binding(binding) => {
                self.check_binding(binding);
                Ty::Void
            }
            Stmt::Return(expr) => expr
                .as_ref()
                .map_or(Ty::Void, |expr| self.infer_expr(expr, expected)),
            Stmt::If(if_stmt) => {
                let condition_ty = self.infer_expr(&if_stmt.condition, Some(&Ty::Bool));
                self.expect_assignable(if_stmt.condition.span(), &Ty::Bool, &condition_ty);
                let narrowings = self.condition_narrowings(&if_stmt.condition);

                self.push_scope();
                self.apply_branch_narrowings(&narrowings, true);
                let then_ty = self.infer_expr(&if_stmt.then_body, expected);
                self.pop_scope();

                let else_ty = if let Some(else_body) = &if_stmt.else_body {
                    self.push_scope();
                    self.apply_branch_narrowings(&narrowings, false);
                    let ty = self.infer_expr(else_body, expected);
                    self.pop_scope();
                    ty
                } else {
                    Ty::Void
                };

                if same_type(&then_ty, &else_ty) {
                    then_ty
                } else {
                    Ty::Unknown
                }
            }
            Stmt::For(for_stmt) => {
                let iterable_ty = self.infer_expr(&for_stmt.iterable, None);
                let element_ty = match iterable_ty {
                    Ty::Vec(inner) => *inner,
                    _ => Ty::Unknown,
                };
                self.push_scope();
                self.bind(for_stmt.binding.node().clone(), element_ty);
                let _ = self.infer_expr(&for_stmt.body, None);
                self.pop_scope();
                Ty::Void
            }
            Stmt::While(while_stmt) => {
                let condition_ty = self.infer_expr(&while_stmt.condition, Some(&Ty::Bool));
                self.expect_assignable(while_stmt.condition.span(), &Ty::Bool, &condition_ty);
                self.push_scope();
                let _ = self.infer_expr(&while_stmt.body, None);
                self.pop_scope();
                Ty::Void
            }
            Stmt::Expr(expr) => self.infer_expr(expr, expected),
            Stmt::Error => Ty::Error,
        }
    }

    fn infer_expr(&mut self, expr: &Spanned<Expr>, expected: Option<&Ty>) -> Ty {
        match expr.node() {
            Expr::IntLiteral(_) => contextual_int(expected),
            Expr::FloatLiteral(_) => contextual_float(expected),
            Expr::StringLiteral(_) => Ty::String,
            Expr::StringInterp(parts) => {
                for part in parts {
                    if let orv_syntax::ast::StringPart::Expr(expr) = part {
                        let _ = self.infer_expr(expr, None);
                    }
                }
                Ty::String
            }
            Expr::BoolLiteral(_) => Ty::Bool,
            Expr::Void => Ty::Void,
            Expr::Ident(name) => self.lookup(name).unwrap_or_else(|| {
                self.registry
                    .function_sig(name)
                    .map(|sig| Ty::Function {
                        params: sig.params.iter().map(|param| param.ty.clone()).collect(),
                        ret: Box::new(sig.ret.clone()),
                    })
                    .unwrap_or(Ty::Unknown)
            }),
            Expr::Binary { left, op, right } => {
                let left_ty = self.infer_expr(left, expected);
                let right_ty = self.infer_expr(right, Some(&left_ty));
                match op.node() {
                    orv_syntax::ast::BinOp::Add
                    | orv_syntax::ast::BinOp::Sub
                    | orv_syntax::ast::BinOp::Mul
                    | orv_syntax::ast::BinOp::Div => {
                        if left_ty.is_numeric() && right_ty.is_numeric() {
                            if same_type(&left_ty, &right_ty) || matches!(right_ty, Ty::Unknown) {
                                left_ty
                            } else {
                                self.type_mismatch(expr.span(), &left_ty, &right_ty);
                                Ty::Error
                            }
                        } else {
                            self.emit_type_error(
                                expr.span(),
                                "arithmetic operators require numeric operands",
                            );
                            Ty::Error
                        }
                    }
                    orv_syntax::ast::BinOp::Eq | orv_syntax::ast::BinOp::NotEq => {
                        if !is_equality_comparable(&left_ty, &right_ty) {
                            self.type_mismatch(expr.span(), &left_ty, &right_ty);
                        }
                        Ty::Bool
                    }
                    orv_syntax::ast::BinOp::Lt
                    | orv_syntax::ast::BinOp::LtEq
                    | orv_syntax::ast::BinOp::Gt
                    | orv_syntax::ast::BinOp::GtEq => {
                        if !same_type(&left_ty, &right_ty) && !matches!(right_ty, Ty::Unknown) {
                            self.type_mismatch(expr.span(), &left_ty, &right_ty);
                        }
                        Ty::Bool
                    }
                    orv_syntax::ast::BinOp::And | orv_syntax::ast::BinOp::Or => {
                        self.expect_assignable(left.span(), &Ty::Bool, &left_ty);
                        self.expect_assignable(right.span(), &Ty::Bool, &right_ty);
                        Ty::Bool
                    }
                    orv_syntax::ast::BinOp::Pipe => right_ty,
                }
            }
            Expr::Unary { op, operand } => {
                let operand_ty = self.infer_expr(operand, expected);
                match op.node() {
                    orv_syntax::ast::UnaryOp::Neg => {
                        if operand_ty.is_numeric() {
                            operand_ty
                        } else {
                            self.emit_type_error(
                                expr.span(),
                                "negation requires a numeric operand",
                            );
                            Ty::Error
                        }
                    }
                    orv_syntax::ast::UnaryOp::Not => {
                        self.expect_assignable(operand.span(), &Ty::Bool, &operand_ty);
                        Ty::Bool
                    }
                }
            }
            Expr::Assign { target, op, value } => {
                let target_ty = self.infer_expr(target, None);
                let value_ty = self.infer_expr(value, Some(&target_ty));
                match op.node() {
                    AssignOp::Assign => self.expect_assignable(value.span(), &target_ty, &value_ty),
                    AssignOp::AddAssign | AssignOp::SubAssign => {
                        if !target_ty.is_numeric() {
                            self.emit_type_error(
                                target.span(),
                                "compound assignment requires a numeric target",
                            );
                        }
                        self.expect_assignable(value.span(), &target_ty, &value_ty);
                    }
                }
                target_ty
            }
            Expr::Call { callee, args } => self.infer_call(expr, callee, args),
            Expr::Field { object, field } => {
                let object_ty = self.infer_expr(object, None);
                self.field_type(expr.span(), &object_ty, field.node())
            }
            Expr::Index { object, index } => {
                let object_ty = self.infer_expr(object, None);
                let index_ty = self.infer_expr(index, None);
                match object_ty {
                    Ty::Vec(inner) => {
                        if !matches!(index_ty, Ty::Int(_)) {
                            self.emit_type_error(index.span(), "vector indices must be integers");
                        }
                        *inner
                    }
                    Ty::HashMap(key, value) => {
                        self.expect_assignable(index.span(), &key, &index_ty);
                        *value
                    }
                    _ => Ty::Unknown,
                }
            }
            Expr::Block(stmts) => {
                self.push_scope();
                let mut tail = Ty::Void;
                for stmt in stmts {
                    tail = self.check_stmt(stmt.node(), expected);
                }
                self.pop_scope();
                tail
            }
            Expr::When { subject, arms } => self.infer_when(expr.span(), subject, arms, expected),
            Expr::Object(fields) => self.infer_object(expr.span(), fields, expected),
            Expr::Map(fields) => self.infer_map(expr.span(), fields, expected),
            Expr::Array(items) => self.infer_array(expr.span(), items, expected),
            Expr::Node(node) => self.infer_node(node, expr.span(), expected),
            Expr::Paren(inner) => self.infer_expr(inner, expected),
            Expr::Await(inner) => self.infer_expr(inner, expected),
            Expr::Error => Ty::Error,
        }
    }

    fn infer_call(
        &mut self,
        expr: &Spanned<Expr>,
        callee: &Spanned<Expr>,
        args: &[Spanned<orv_syntax::ast::CallArg>],
    ) -> Ty {
        if let Expr::Ident(name) = callee.node()
            && let Some(sig) = self.registry.function_sig(name).cloned()
        {
            self.check_declared_call_arguments(expr, name, &sig, args);
            return sig.ret.clone();
        }

        if let Expr::Field { object, field } = callee.node() {
            let object_ty = self.infer_expr(object, None);
            if field.node() == "len" && args.is_empty() {
                if matches!(
                    object_ty,
                    Ty::Vec(_) | Ty::String | Ty::Object(_) | Ty::HashMap(_, _)
                ) {
                    return Ty::Int("i32".to_owned());
                }
            }
        }

        let callee_ty = self.infer_expr(callee, None);
        match callee_ty {
            Ty::Function { params, ret } => {
                for (arg, expected_ty) in args.iter().zip(&params) {
                    let actual = self.infer_expr(&arg.node().value, Some(expected_ty));
                    self.expect_assignable(arg.node().value.span(), expected_ty, &actual);
                }
                *ret
            }
            Ty::Unknown => Ty::Unknown,
            _ => {
                self.emit_type_error(expr.span(), "attempted to call a non-callable value");
                Ty::Error
            }
        }
    }

    fn infer_object(
        &mut self,
        span: orv_span::Span,
        fields: &[Spanned<orv_syntax::ast::ObjectField>],
        expected: Option<&Ty>,
    ) -> Ty {
        if let Some(Ty::Struct(name)) = expected {
            return self.check_struct_object(span, fields, name);
        }

        let mut object_fields = BTreeMap::new();
        for field in fields {
            let value_ty = self.infer_expr(&field.node().value, None);
            object_fields.insert(field.node().key.node().clone(), value_ty);
        }
        Ty::Object(object_fields)
    }

    fn infer_map(
        &mut self,
        span: orv_span::Span,
        fields: &[Spanned<orv_syntax::ast::ObjectField>],
        expected: Option<&Ty>,
    ) -> Ty {
        let expected_value = match expected {
            Some(Ty::HashMap(key, value)) => {
                if !matches!(key.as_ref(), Ty::String) {
                    self.emit_type_error(
                        span,
                        format!(
                            "map literals currently require `string` keys, found `{}`",
                            key.display()
                        ),
                    );
                }
                Some(value.as_ref())
            }
            _ => None,
        };

        if fields.is_empty() {
            if let Some(value) = expected_value {
                return Ty::HashMap(Box::new(Ty::String), Box::new(value.clone()));
            }
            self.emit_type_error(span, "cannot infer the value type of an empty map literal");
            return Ty::HashMap(Box::new(Ty::String), Box::new(Ty::Unknown));
        }

        let first_ty = self.infer_expr(&fields[0].node().value, expected_value);
        for field in &fields[1..] {
            let actual_ty = self.infer_expr(&field.node().value, Some(&first_ty));
            if !same_type(&first_ty, &actual_ty) && !matches!(actual_ty, Ty::Unknown) {
                self.type_mismatch(field.node().value.span(), &first_ty, &actual_ty);
            }
        }

        Ty::HashMap(Box::new(Ty::String), Box::new(first_ty))
    }

    fn check_struct_object(
        &mut self,
        span: orv_span::Span,
        fields: &[Spanned<orv_syntax::ast::ObjectField>],
        struct_name: &str,
    ) -> Ty {
        let Some(spec) = self.registry.struct_spec(struct_name) else {
            return Ty::Struct(struct_name.to_owned());
        };

        let mut seen = HashSet::new();
        for field in fields {
            let key = field.node().key.node().clone();
            seen.insert(key.clone());
            if let Some(expected_ty) = spec.fields.get(&key) {
                let actual_ty = self.infer_expr(&field.node().value, Some(expected_ty));
                self.expect_assignable(field.node().value.span(), expected_ty, &actual_ty);
            } else {
                self.emit_type_error(
                    field.span(),
                    format!("extra field `{key}` in `{struct_name}` object literal"),
                );
            }
        }

        for field_name in spec.fields.keys() {
            if !seen.contains(field_name) {
                self.emit_type_error(
                    span,
                    format!("missing field `{field_name}` for `{struct_name}`"),
                );
            }
        }

        Ty::Struct(struct_name.to_owned())
    }

    fn infer_array(
        &mut self,
        span: orv_span::Span,
        items: &[Spanned<Expr>],
        expected: Option<&Ty>,
    ) -> Ty {
        let expected_elem = match expected {
            Some(Ty::Vec(inner)) => Some(inner.as_ref()),
            _ => None,
        };

        if items.is_empty() {
            if let Some(inner) = expected_elem {
                return Ty::Vec(Box::new(inner.clone()));
            }
            self.emit_type_error(
                span,
                "cannot infer the element type of an empty array literal",
            );
            return Ty::Vec(Box::new(Ty::Unknown));
        }

        let first_ty = self.infer_expr(&items[0], expected_elem);
        for item in &items[1..] {
            let item_ty = self.infer_expr(item, Some(&first_ty));
            if !same_type(&first_ty, &item_ty) && !matches!(item_ty, Ty::Unknown) {
                self.type_mismatch(item.span(), &first_ty, &item_ty);
            }
        }
        Ty::Vec(Box::new(first_ty))
    }

    fn infer_node(&mut self, node: &NodeExpr, span: orv_span::Span, expected: Option<&Ty>) -> Ty {
        let name = node.name.node().to_string();
        if name == "env" {
            if let Some(value) = node.positional.first() {
                let _ = self.infer_expr(value, None);
            }
            return expected.cloned().unwrap_or(Ty::String);
        }

        for positional in &node.positional {
            let _ = self.infer_expr(positional, None);
        }
        for property in &node.properties {
            let _ = self.infer_expr(&property.node().value, None);
        }
        if let Some(body) = &node.body {
            let _ = self.infer_expr(body, None);
        }

        if name == "response" {
            return Ty::Node("response".to_owned());
        }
        if name == "serve" {
            return Ty::Node("serve".to_owned());
        }
        if name == "html" || matches!(expected, Some(Ty::Node(_))) {
            return Ty::Node(name);
        }

        if name.is_empty() {
            self.emit_type_error(span, "invalid node expression");
            return Ty::Error;
        }

        Ty::Node(name)
    }

    fn infer_when(
        &mut self,
        span: orv_span::Span,
        subject: &Spanned<Expr>,
        arms: &[Spanned<orv_syntax::ast::WhenArm>],
        expected: Option<&Ty>,
    ) -> Ty {
        let subject_ty = self.infer_expr(subject, None);
        let mut result_ty: Option<Ty> = None;
        let mut covered_variants = HashSet::new();
        let mut has_wildcard = false;

        for arm in arms {
            self.push_scope();
            let coverage = self.check_pattern(&arm.node().pattern, &subject_ty);
            let arm_ty = self.infer_expr(&arm.node().body, expected);
            self.pop_scope();

            if let Some(existing) = &result_ty {
                if !same_type(existing, &arm_ty) {
                    self.type_mismatch(arm.node().body.span(), existing, &arm_ty);
                    result_ty = Some(Ty::Unknown);
                }
            } else {
                result_ty = Some(arm_ty);
            }

            match coverage {
                PatternCoverage::Wildcard => has_wildcard = true,
                PatternCoverage::Variant(name) => {
                    covered_variants.insert(name);
                }
                PatternCoverage::Other => {}
            }
        }

        if let Ty::Enum(enum_name) = &subject_ty
            && !has_wildcard
            && let Some(spec) = self.registry.enum_spec(enum_name)
        {
            for variant in spec.variants.keys() {
                if !covered_variants.contains(variant) {
                    self.emit_type_error(
                        span,
                        format!(
                            "non-exhaustive `when` for `{enum_name}`: missing variant `{variant}`"
                        ),
                    );
                    break;
                }
            }
        }

        result_ty.unwrap_or(Ty::Unknown)
    }

    fn field_type(&mut self, span: orv_span::Span, object_ty: &Ty, field: &str) -> Ty {
        match object_ty {
            Ty::Struct(name) => self
                .registry
                .struct_spec(name)
                .and_then(|spec| spec.fields.get(field).cloned())
                .unwrap_or_else(|| {
                    self.emit_type_error(span, format!("unknown field `{field}` on `{name}`"));
                    Ty::Unknown
                }),
            Ty::Object(fields) => fields.get(field).cloned().unwrap_or(Ty::Unknown),
            Ty::Vec(_) | Ty::String if field == "len" => Ty::Function {
                params: Vec::new(),
                ret: Box::new(Ty::Int("i32".to_owned())),
            },
            Ty::HashMap(key, _) if field == "keys" => Ty::Function {
                params: Vec::new(),
                ret: Box::new(Ty::Vec(Box::new(key.as_ref().clone()))),
            },
            Ty::HashMap(_, value) if field == "values" => Ty::Function {
                params: Vec::new(),
                ret: Box::new(Ty::Vec(Box::new(value.as_ref().clone()))),
            },
            Ty::HashMap(_, _) if field == "len" => Ty::Function {
                params: Vec::new(),
                ret: Box::new(Ty::Int("i32".to_owned())),
            },
            _ => Ty::Unknown,
        }
    }

    fn check_pattern(&mut self, pattern: &Spanned<Pattern>, subject_ty: &Ty) -> PatternCoverage {
        match pattern.node() {
            Pattern::Wildcard => PatternCoverage::Wildcard,
            Pattern::Binding(name) => {
                self.bind(name.clone(), subject_ty.clone());
                PatternCoverage::Other
            }
            Pattern::IntLiteral(_) => {
                self.expect_pattern_type(
                    pattern.span(),
                    subject_ty,
                    &contextual_int(Some(subject_ty)),
                );
                PatternCoverage::Other
            }
            Pattern::FloatLiteral(_) => {
                self.expect_pattern_type(
                    pattern.span(),
                    subject_ty,
                    &contextual_float(Some(subject_ty)),
                );
                PatternCoverage::Other
            }
            Pattern::StringLiteral(_) => {
                self.expect_pattern_type(pattern.span(), subject_ty, &Ty::String);
                PatternCoverage::Other
            }
            Pattern::BoolLiteral(_) => {
                self.expect_pattern_type(pattern.span(), subject_ty, &Ty::Bool);
                PatternCoverage::Other
            }
            Pattern::Void => {
                self.expect_pattern_type(pattern.span(), subject_ty, &Ty::Void);
                PatternCoverage::Other
            }
            Pattern::Variant { path, fields } => {
                self.check_variant_pattern(pattern.span(), path, fields, subject_ty)
            }
            Pattern::Error => PatternCoverage::Other,
        }
    }

    fn check_variant_pattern(
        &mut self,
        span: orv_span::Span,
        path: &[Spanned<String>],
        fields: &[Spanned<Pattern>],
        subject_ty: &Ty,
    ) -> PatternCoverage {
        let Some((enum_name, variant_name)) = self.resolve_pattern_variant(path, subject_ty) else {
            self.emit_type_error(span, "variant patterns require an enum-typed subject");
            return PatternCoverage::Other;
        };

        let Some(spec) = self.registry.enum_spec(&enum_name) else {
            self.emit_type_error(span, format!("unknown enum `{enum_name}` in pattern"));
            return PatternCoverage::Other;
        };
        let Some(variant) = spec.variants.get(&variant_name) else {
            self.emit_type_error(
                span,
                format!("enum `{enum_name}` has no variant `{variant_name}`"),
            );
            return PatternCoverage::Other;
        };

        if variant.fields.len() != fields.len() {
            self.emit_type_error(
                span,
                format!(
                    "variant `{enum_name}.{variant_name}` expects {} field(s), found {}",
                    variant.fields.len(),
                    fields.len()
                ),
            );
        }

        for (field_pattern, field_ty) in fields.iter().zip(&variant.fields) {
            self.check_pattern(field_pattern, field_ty);
        }

        PatternCoverage::Variant(variant_name)
    }

    fn resolve_pattern_variant(
        &self,
        path: &[Spanned<String>],
        subject_ty: &Ty,
    ) -> Option<(String, String)> {
        let subject_enum = match subject_ty {
            Ty::Enum(name) => Some(name.clone()),
            Ty::Nullable(inner) => match inner.as_ref() {
                Ty::Enum(name) => Some(name.clone()),
                _ => None,
            },
            _ => None,
        };

        match path {
            [variant] => subject_enum.map(|enum_name| (enum_name, variant.node().clone())),
            [enum_name, variant_name] => {
                if let Some(subject_enum) = subject_enum
                    && subject_enum != *enum_name.node()
                {
                    return None;
                }
                Some((enum_name.node().clone(), variant_name.node().clone()))
            }
            _ => None,
        }
    }

    fn expect_pattern_type(&mut self, span: orv_span::Span, subject_ty: &Ty, pattern_ty: &Ty) {
        if matches!(subject_ty, Ty::Unknown | Ty::Error) {
            return;
        }
        if !is_equality_comparable(subject_ty, pattern_ty) {
            self.type_mismatch(span, subject_ty, pattern_ty);
        }
    }

    fn bind(&mut self, name: String, ty: Ty) {
        self.scopes
            .last_mut()
            .expect("type checker scope stack must be non-empty")
            .insert(name, ty);
    }

    fn lookup(&self, name: &str) -> Option<Ty> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn expect_assignable(&mut self, span: orv_span::Span, expected: &Ty, actual: &Ty) {
        if !is_assignable(expected, actual) {
            self.type_mismatch(span, expected, actual);
        }
    }

    fn type_mismatch(&mut self, span: orv_span::Span, expected: &Ty, actual: &Ty) {
        self.diagnostics.push(
            Diagnostic::error(format!(
                "type mismatch: expected `{}`, found `{}`",
                expected.display(),
                actual.display()
            ))
            .with_label(Label::primary(span, "incompatible type here")),
        );
    }

    fn emit_type_error(&mut self, span: orv_span::Span, message: impl Into<String>) {
        self.diagnostics
            .push(Diagnostic::error(message.into()).with_label(Label::primary(span, "here")));
    }

    fn check_declared_call_arguments(
        &mut self,
        expr: &Spanned<Expr>,
        function_name: &str,
        sig: &FunctionSig,
        args: &[Spanned<orv_syntax::ast::CallArg>],
    ) {
        let mut assigned = vec![false; sig.params.len()];
        let mut next_positional = 0usize;
        let mut saw_named = false;

        for arg in args {
            if let Some(name) = &arg.node().name {
                saw_named = true;
                let Some(index) = sig
                    .params
                    .iter()
                    .position(|param| param.name == *name.node())
                else {
                    self.emit_type_error(
                        arg.span(),
                        format!(
                            "function `{function_name}` has no parameter named `{}`",
                            name.node()
                        ),
                    );
                    let _ = self.infer_expr(&arg.node().value, None);
                    continue;
                };

                if assigned[index] {
                    self.emit_type_error(
                        arg.span(),
                        format!(
                            "parameter `{}` is passed more than once to `{function_name}`",
                            name.node()
                        ),
                    );
                }
                assigned[index] = true;
                let expected_ty = &sig.params[index].ty;
                let actual = self.infer_expr(&arg.node().value, Some(expected_ty));
                self.expect_assignable(arg.node().value.span(), expected_ty, &actual);
                continue;
            }

            if saw_named {
                self.emit_type_error(
                    arg.span(),
                    format!(
                        "positional arguments must come before named arguments in `{function_name}`"
                    ),
                );
            }

            while next_positional < assigned.len() && assigned[next_positional] {
                next_positional += 1;
            }
            if next_positional >= sig.params.len() {
                self.emit_type_error(
                    arg.span(),
                    format!(
                        "function `{function_name}` expects at most {} argument(s), got {}",
                        sig.params.len(),
                        args.len()
                    ),
                );
                let _ = self.infer_expr(&arg.node().value, None);
                continue;
            }

            assigned[next_positional] = true;
            let expected_ty = &sig.params[next_positional].ty;
            let actual = self.infer_expr(&arg.node().value, Some(expected_ty));
            self.expect_assignable(arg.node().value.span(), expected_ty, &actual);
            next_positional += 1;
        }

        for (index, param) in sig.params.iter().enumerate() {
            if !assigned[index] && !param.has_default {
                self.emit_type_error(
                    expr.span(),
                    format!(
                        "function `{function_name}` is missing required argument `{}`",
                        param.name
                    ),
                );
            }
        }
    }

    fn condition_narrowings(&self, expr: &Spanned<Expr>) -> Vec<Narrowing> {
        match expr.node() {
            Expr::Binary { left, op, right }
                if matches!(
                    op.node(),
                    orv_syntax::ast::BinOp::Eq | orv_syntax::ast::BinOp::NotEq
                ) =>
            {
                self.narrow_from_void_comparison(left, op.node(), right)
                    .into_iter()
                    .collect()
            }
            Expr::Paren(inner) => self.condition_narrowings(inner),
            _ => Vec::new(),
        }
    }

    fn narrow_from_void_comparison(
        &self,
        left: &Spanned<Expr>,
        op: &orv_syntax::ast::BinOp,
        right: &Spanned<Expr>,
    ) -> Option<Narrowing> {
        let (ident, ty) = match (left.node(), right.node()) {
            (Expr::Ident(name), Expr::Void) => (name.as_str(), self.lookup(name)?),
            (Expr::Void, Expr::Ident(name)) => (name.as_str(), self.lookup(name)?),
            _ => return None,
        };

        let Ty::Nullable(inner) = ty else {
            return None;
        };

        let (then_ty, else_ty) = match op {
            orv_syntax::ast::BinOp::NotEq => (inner.as_ref().clone(), Ty::Void),
            orv_syntax::ast::BinOp::Eq => (Ty::Void, inner.as_ref().clone()),
            _ => return None,
        };

        Some(Narrowing {
            name: ident.to_owned(),
            then_ty,
            else_ty,
        })
    }

    fn apply_branch_narrowings(&mut self, narrowings: &[Narrowing], then_branch: bool) {
        for narrowing in narrowings {
            self.bind(
                narrowing.name.clone(),
                if then_branch {
                    narrowing.then_ty.clone()
                } else {
                    narrowing.else_ty.clone()
                },
            );
        }
    }
}

fn contextual_int(expected: Option<&Ty>) -> Ty {
    match expected {
        Some(Ty::Int(name)) => Ty::Int(name.clone()),
        _ => Ty::Int("i64".to_owned()),
    }
}

fn contextual_float(expected: Option<&Ty>) -> Ty {
    match expected {
        Some(Ty::Float(name)) => Ty::Float(name.clone()),
        _ => Ty::Float("f64".to_owned()),
    }
}

fn is_assignable(expected: &Ty, actual: &Ty) -> bool {
    match (expected, actual) {
        (_, Ty::Unknown | Ty::Error) | (Ty::Unknown | Ty::Error, _) => true,
        (Ty::Void, Ty::Void) | (Ty::Bool, Ty::Bool) | (Ty::String, Ty::String) => true,
        (Ty::Int(a), Ty::Int(b)) | (Ty::Float(a), Ty::Float(b)) => a == b,
        (Ty::Named(a), Ty::Named(b))
        | (Ty::Struct(a), Ty::Struct(b))
        | (Ty::Enum(a), Ty::Enum(b))
        | (Ty::Node(a), Ty::Node(b)) => a == b,
        (Ty::Nullable(inner), Ty::Void) => !matches!(**inner, Ty::Void),
        (Ty::Nullable(inner), other) => is_assignable(inner, other),
        (Ty::Vec(a), Ty::Vec(b)) => is_assignable(a, b),
        (Ty::HashMap(ka, va), Ty::HashMap(kb, vb)) => {
            is_assignable(ka, kb) && is_assignable(va, vb)
        }
        (
            Ty::Function {
                params: expected_params,
                ret: expected_ret,
            },
            Ty::Function {
                params: actual_params,
                ret: actual_ret,
            },
        ) => {
            expected_params.len() == actual_params.len()
                && expected_params
                    .iter()
                    .zip(actual_params)
                    .all(|(expected, actual)| is_assignable(expected, actual))
                && is_assignable(expected_ret, actual_ret)
        }
        (Ty::Object(expected_fields), Ty::Object(actual_fields)) => {
            expected_fields.len() == actual_fields.len()
                && expected_fields.iter().all(|(key, expected)| {
                    actual_fields
                        .get(key)
                        .is_some_and(|actual| is_assignable(expected, actual))
                })
        }
        _ => false,
    }
}

fn same_type(left: &Ty, right: &Ty) -> bool {
    left == right || matches!(left, Ty::Unknown) || matches!(right, Ty::Unknown)
}

fn is_equality_comparable(left: &Ty, right: &Ty) -> bool {
    same_type(left, right)
        || matches!(
            (left, right),
            (Ty::Nullable(_), Ty::Void) | (Ty::Void, Ty::Nullable(_))
        )
}

fn is_int_type(name: &str) -> bool {
    matches!(
        name,
        "u8" | "u16" | "u32" | "u64" | "usize" | "i8" | "i16" | "i32" | "i64" | "isize"
    )
}

fn is_float_type(name: &str) -> bool {
    matches!(name, "f32" | "f64")
}
