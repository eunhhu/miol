use crate::hir::*;

#[must_use]
pub fn dump_hir(module: &Module) -> String {
    let mut out = String::new();
    dump_module(module, &mut out, 0);
    out
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn dump_module(module: &Module, out: &mut String, depth: usize) {
    indent(out, depth);
    out.push_str("Module\n");
    for item in &module.items {
        dump_item(item, out, depth + 1);
    }
}

fn dump_item(item: &Item, out: &mut String, depth: usize) {
    indent(out, depth);
    if let Some(symbol) = item.symbol {
        out.push_str(&format!("symbol#{symbol} "));
    }

    match &item.kind {
        ItemKind::Import(import) => {
            out.push_str("Import ");
            out.push_str(&import.path.join("."));
            if !import.names.is_empty() {
                out.push_str(" {");
                out.push_str(&import.names.join(", "));
                out.push('}');
            }
            if let Some(alias) = &import.alias {
                out.push_str(" as ");
                out.push_str(alias);
            }
            out.push('\n');
        }
        ItemKind::Function(function) => {
            out.push_str(&format!(
                "Function {} scope#{}\n",
                function.name, function.scope
            ));
            for param in &function.params {
                dump_param(param, out, depth + 1);
            }
            if let Some(ret) = &function.return_type {
                indent(out, depth + 1);
                out.push_str("return ");
                dump_type_inline(ret, out);
                out.push('\n');
            }
            dump_expr(&function.body, out, depth + 1);
        }
        ItemKind::Define(define) => {
            out.push_str(&format!("Define {} scope#{}", define.name, define.scope));
            if let Some(domain) = &define.return_domain {
                out.push_str(" -> @");
                out.push_str(domain);
            }
            out.push('\n');
            for param in &define.params {
                dump_param(param, out, depth + 1);
            }
            dump_expr(&define.body, out, depth + 1);
        }
        ItemKind::Struct(item) => {
            out.push_str(&format!("Struct {}\n", item.name));
            for field in &item.fields {
                indent(out, depth + 1);
                out.push_str(&field.name);
                out.push_str(": ");
                dump_type_inline(&field.ty, out);
                out.push('\n');
            }
        }
        ItemKind::Enum(item) => {
            out.push_str(&format!("Enum {}\n", item.name));
            for variant in &item.variants {
                indent(out, depth + 1);
                out.push_str(&variant.name);
                if !variant.fields.is_empty() {
                    out.push('(');
                    for (index, field) in variant.fields.iter().enumerate() {
                        if index > 0 {
                            out.push_str(", ");
                        }
                        dump_type_inline(field, out);
                    }
                    out.push(')');
                }
                out.push('\n');
            }
        }
        ItemKind::TypeAlias(item) => {
            out.push_str(&format!("TypeAlias {} = ", item.name));
            dump_type_inline(&item.ty, out);
            out.push('\n');
        }
        ItemKind::Binding(binding) => {
            out.push_str("Binding\n");
            dump_binding(binding, out, depth + 1);
        }
        ItemKind::Stmt(stmt) => {
            out.push_str("Stmt\n");
            dump_stmt(stmt, out, depth + 1);
        }
        ItemKind::Error => {
            out.push_str("Error\n");
        }
    }
}

fn dump_param(param: &Param, out: &mut String, depth: usize) {
    indent(out, depth);
    out.push_str("Param ");
    if let Some(symbol) = param.symbol {
        out.push_str(&format!("symbol#{symbol} "));
    }
    out.push_str(&param.name);
    if let Some(ty) = &param.ty {
        out.push_str(": ");
        dump_type_inline(ty, out);
    }
    if let Some(default) = &param.default {
        out.push_str(" = ");
        dump_expr_inline(default, out);
    }
    out.push('\n');
}

fn dump_binding(binding: &Binding, out: &mut String, depth: usize) {
    indent(out, depth);
    out.push_str("let ");
    if let Some(symbol) = binding.symbol {
        out.push_str(&format!("symbol#{symbol} "));
    }
    out.push_str(&binding.name);
    if let Some(ty) = &binding.ty {
        out.push_str(": ");
        dump_type_inline(ty, out);
    }
    if let Some(value) = &binding.value {
        out.push_str(" = ");
        dump_expr_inline(value, out);
    }
    out.push('\n');
}

fn dump_stmt(stmt: &Stmt, out: &mut String, depth: usize) {
    match stmt {
        Stmt::Binding(binding) => dump_binding(binding, out, depth),
        Stmt::Return(expr) => {
            indent(out, depth);
            out.push_str("return");
            if let Some(expr) = expr {
                out.push(' ');
                dump_expr_inline(expr, out);
            }
            out.push('\n');
        }
        Stmt::If(if_stmt) => {
            indent(out, depth);
            out.push_str("if ");
            dump_expr_inline(&if_stmt.condition, out);
            out.push('\n');
            indent(out, depth + 1);
            out.push_str(&format!("then scope#{}\n", if_stmt.then_scope));
            dump_expr(&if_stmt.then_body, out, depth + 2);
            if let Some(scope) = if_stmt.else_scope {
                indent(out, depth + 1);
                out.push_str(&format!("else scope#{scope}\n"));
            }
            if let Some(body) = &if_stmt.else_body {
                dump_expr(body, out, depth + 2);
            }
        }
        Stmt::For(for_stmt) => {
            indent(out, depth);
            out.push_str(&format!("for scope#{} ", for_stmt.scope));
            if let Some(symbol) = for_stmt.binding_symbol {
                out.push_str(&format!("symbol#{symbol} "));
            }
            out.push_str(&for_stmt.binding);
            out.push_str(" of ");
            dump_expr_inline(&for_stmt.iterable, out);
            out.push('\n');
            dump_expr(&for_stmt.body, out, depth + 1);
        }
        Stmt::While(while_stmt) => {
            indent(out, depth);
            out.push_str(&format!("while scope#{} ", while_stmt.scope));
            dump_expr_inline(&while_stmt.condition, out);
            out.push('\n');
            dump_expr(&while_stmt.body, out, depth + 1);
        }
        Stmt::Expr(expr) => dump_expr(expr, out, depth),
        Stmt::Error => {
            indent(out, depth);
            out.push_str("ErrorStmt\n");
        }
    }
}

fn dump_expr(expr: &Expr, out: &mut String, depth: usize) {
    indent(out, depth);
    dump_expr_inline(expr, out);
    out.push('\n');

    match expr {
        Expr::Block { stmts, .. } => {
            for stmt in stmts {
                dump_stmt(stmt, out, depth + 1);
            }
        }
        Expr::Node(node) => {
            if let Some(body) = &node.body {
                dump_expr(body, out, depth + 1);
            }
        }
        Expr::When { arms, .. } => {
            for arm in arms {
                indent(out, depth + 1);
                out.push_str(&format!("arm scope#{} ", arm.scope));
                dump_pattern_inline(&arm.pattern, out);
                out.push('\n');
                dump_expr(&arm.body, out, depth + 2);
            }
        }
        _ => {}
    }
}

fn dump_expr_inline(expr: &Expr, out: &mut String) {
    match expr {
        Expr::IntLiteral(value) => out.push_str(&value.to_string()),
        Expr::FloatLiteral(value) => out.push_str(&value.to_string()),
        Expr::StringLiteral(value) => out.push_str(&format!("{value:?}")),
        Expr::StringInterp(parts) => {
            out.push_str("interp(");
            for (index, part) in parts.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                match part {
                    StringPart::Lit(value) => out.push_str(&format!("{value:?}")),
                    StringPart::Expr(expr) => dump_expr_inline(expr, out),
                }
            }
            out.push(')');
        }
        Expr::BoolLiteral(value) => out.push_str(&value.to_string()),
        Expr::Void => out.push_str("void"),
        Expr::Ident(name) => {
            out.push_str(&name.name);
            if let Some(symbol) = name.symbol {
                out.push_str(&format!("@symbol#{symbol}"));
            } else {
                out.push_str("@unresolved");
            }
        }
        Expr::Binary { left, op, right } => {
            dump_expr_inline(left, out);
            out.push(' ');
            out.push_str(match op {
                BinaryOp::Add => "+",
                BinaryOp::Sub => "-",
                BinaryOp::Mul => "*",
                BinaryOp::Div => "/",
                BinaryOp::Eq => "==",
                BinaryOp::NotEq => "!=",
                BinaryOp::Lt => "<",
                BinaryOp::LtEq => "<=",
                BinaryOp::Gt => ">",
                BinaryOp::GtEq => ">=",
                BinaryOp::And => "&&",
                BinaryOp::Or => "||",
                BinaryOp::Pipe => "|>",
            });
            out.push(' ');
            dump_expr_inline(right, out);
        }
        Expr::Unary { op, operand } => {
            out.push_str(match op {
                UnaryOp::Neg => "-",
                UnaryOp::Not => "!",
            });
            dump_expr_inline(operand, out);
        }
        Expr::Assign { target, op, value } => {
            dump_expr_inline(target, out);
            out.push(' ');
            out.push_str(match op {
                AssignOp::Assign => "=",
                AssignOp::AddAssign => "+=",
                AssignOp::SubAssign => "-=",
            });
            out.push(' ');
            dump_expr_inline(value, out);
        }
        Expr::Call { callee, args } => {
            dump_expr_inline(callee, out);
            out.push('(');
            for (index, arg) in args.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                if let Some(name) = &arg.name {
                    out.push_str(name);
                    out.push('=');
                }
                dump_expr_inline(&arg.value, out);
            }
            out.push(')');
        }
        Expr::Field { object, field } => {
            dump_expr_inline(object, out);
            out.push('.');
            out.push_str(field);
        }
        Expr::Index { object, index } => {
            dump_expr_inline(object, out);
            out.push('[');
            dump_expr_inline(index, out);
            out.push(']');
        }
        Expr::Block { scope, .. } => out.push_str(&format!("block scope#{scope}")),
        Expr::When { subject, arms } => {
            out.push_str("when ");
            dump_expr_inline(subject, out);
            out.push_str(" { ");
            for (index, arm) in arms.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                dump_pattern_inline(&arm.pattern, out);
                out.push_str(" -> ");
                dump_expr_inline(&arm.body, out);
            }
            out.push_str(" }");
        }
        Expr::Object(fields) => {
            out.push('{');
            for (index, field) in fields.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str(&field.key);
                out.push_str(": ");
                dump_expr_inline(&field.value, out);
            }
            out.push('}');
        }
        Expr::Map(fields) => {
            out.push_str("#{");
            for (index, field) in fields.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str(&field.key);
                out.push_str(": ");
                dump_expr_inline(&field.value, out);
            }
            out.push('}');
        }
        Expr::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                dump_expr_inline(item, out);
            }
            out.push(']');
        }
        Expr::Node(node) => {
            out.push('@');
            out.push_str(&node.name);
            for positional in &node.positional {
                out.push(' ');
                dump_expr_inline(positional, out);
            }
            for property in &node.properties {
                out.push(' ');
                out.push('%');
                out.push_str(&property.name);
                out.push('=');
                dump_expr_inline(&property.value, out);
            }
        }
        Expr::Paren(inner) => {
            out.push('(');
            dump_expr_inline(inner, out);
            out.push(')');
        }
        Expr::Await(inner) => {
            out.push_str("await ");
            dump_expr_inline(inner, out);
        }
        Expr::Error => out.push_str("<error>"),
    }
}

fn dump_pattern_inline(pattern: &Pattern, out: &mut String) {
    match pattern {
        Pattern::Wildcard => out.push('_'),
        Pattern::Binding(name) => out.push_str(name),
        Pattern::IntLiteral(value) => out.push_str(&value.to_string()),
        Pattern::FloatLiteral(value) => out.push_str(&value.to_string()),
        Pattern::StringLiteral(value) => out.push_str(&format!("{value:?}")),
        Pattern::BoolLiteral(value) => out.push_str(&value.to_string()),
        Pattern::Void => out.push_str("void"),
        Pattern::Variant { path, fields } => {
            out.push_str(&path.join("."));
            if !fields.is_empty() {
                out.push('(');
                for (index, field) in fields.iter().enumerate() {
                    if index > 0 {
                        out.push_str(", ");
                    }
                    dump_pattern_inline(field, out);
                }
                out.push(')');
            }
        }
        Pattern::Error => out.push_str("<error-pattern>"),
    }
}

fn dump_type_inline(ty: &Type, out: &mut String) {
    match ty {
        Type::Named(name) => out.push_str(name),
        Type::Nullable(inner) => {
            dump_type_inline(inner, out);
            out.push('?');
        }
        Type::Generic { name, args } => {
            out.push_str(name);
            out.push('<');
            for (index, arg) in args.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                dump_type_inline(arg, out);
            }
            out.push('>');
        }
        Type::Function { params, ret } => {
            out.push('(');
            for (index, param) in params.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                dump_type_inline(param, out);
            }
            out.push_str(") -> ");
            dump_type_inline(ret, out);
        }
        Type::Node(name) => {
            out.push('@');
            out.push_str(name);
        }
        Type::Error => out.push_str("<type-error>"),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use crate::hir::*;

    use super::dump_hir;

    #[test]
    fn dump_hir_includes_symbols_and_scopes() {
        let module = Module {
            items: vec![Item {
                symbol: Some(0),
                kind: ItemKind::Function(FunctionItem {
                    name: "greet".into(),
                    is_pub: false,
                    is_async: false,
                    scope: 1,
                    params: vec![Param {
                        symbol: Some(1),
                        name: "name".into(),
                        ty: Some(Type::Named("string".into())),
                        default: None,
                    }],
                    return_type: Some(Type::Named("string".into())),
                    body: Expr::Ident(ResolvedName {
                        name: "name".into(),
                        symbol: Some(1),
                    }),
                }),
            }],
        };

        let output = dump_hir(&module);
        assert!(output.contains("symbol#0 Function greet scope#1"));
        assert!(output.contains("Param symbol#1 name: string"));
        assert!(output.contains("name@symbol#1"));
    }

    #[test]
    fn dump_hir_block_expression() {
        let output = dump_hir(&Module {
            items: vec![Item {
                symbol: None,
                kind: ItemKind::Stmt(Stmt::Expr(Expr::Block {
                    scope: 2,
                    stmts: vec![Stmt::Return(Some(Expr::IntLiteral(1)))],
                })),
            }],
        });

        assert_eq!(
            output,
            "Module\n  Stmt\n    block scope#2\n      return 1\n"
        );
    }
}
