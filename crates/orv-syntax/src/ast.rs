//! AST node definitions for the orv language.
//!
//! Every node is wrapped in `Spanned<T>` at the point of use so that source
//! locations are available without cluttering the node definitions themselves.

use orv_span::Spanned;

/// A unique numeric id for an AST node, assigned during parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(u32);

impl NodeId {
    /// Creates a new `NodeId` from a raw value.
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    /// Returns the underlying raw value.
    pub const fn raw(self) -> u32 {
        self.0
    }
}

// ── Top-level ───────────────────────────────────────────────────────────────

/// A parsed source file.
#[derive(Debug, Clone)]
pub struct Module {
    pub items: Vec<Spanned<Item>>,
}

// ── Items ───────────────────────────────────────────────────────────────────

/// A top-level declaration.
#[derive(Debug, Clone)]
pub enum Item {
    /// `import path.to.{symbols}`
    Import(ImportItem),
    /// `[pub] function name(params): RetTy -> body`
    Function(FunctionItem),
    /// `[pub] define Name(params) -> @node { body }`
    Define(DefineItem),
    /// `[pub] struct Name { fields }`
    Struct(StructItem),
    /// `[pub] enum Name { variants }`
    Enum(EnumItem),
    /// `[pub] type Name = Type`
    TypeAlias(TypeAliasItem),
    /// `[pub] let/const binding`
    Binding(BindingStmt),
    /// A bare statement at top level (expression-statement, node, etc.)
    Stmt(Stmt),
    /// Placeholder for a declaration that failed to parse.
    Error,
}

/// `import path.to.{symbols}` or `import path.to.Name`
#[derive(Debug, Clone)]
pub struct ImportItem {
    /// Dot-separated path segments, e.g. `["components", "Button"]`
    pub path: Vec<Spanned<String>>,
    /// If the import ends with `.{A, B}`, the individual names.
    /// Empty if the import is a single symbol (the last path segment).
    pub names: Vec<Spanned<String>>,
    /// Optional alias: `import foo.Bar as Baz`
    pub alias: Option<Spanned<String>>,
}

/// `[pub] function name(params): RetTy -> body`
#[derive(Debug, Clone)]
pub struct FunctionItem {
    pub is_pub: bool,
    pub is_async: bool,
    pub name: Spanned<String>,
    pub params: Vec<Spanned<Param>>,
    pub return_type: Option<Spanned<TypeExpr>>,
    pub body: Spanned<Expr>,
}

/// `[pub] define Name(params) -> @node { body }`
#[derive(Debug, Clone)]
pub struct DefineItem {
    pub is_pub: bool,
    pub name: Spanned<String>,
    pub params: Vec<Spanned<Param>>,
    /// The return domain hint, e.g. `@html` in `-> @html`.
    pub return_domain: Option<Spanned<NodeName>>,
    pub body: Spanned<Expr>,
}

/// `[pub] struct Name { fields }`
#[derive(Debug, Clone)]
pub struct StructItem {
    pub is_pub: bool,
    pub name: Spanned<String>,
    pub fields: Vec<Spanned<StructField>>,
}

/// A single field in a struct definition.
#[derive(Debug, Clone)]
pub struct StructField {
    pub name: Spanned<String>,
    pub ty: Spanned<TypeExpr>,
}

/// `[pub] enum Name { variants }`
#[derive(Debug, Clone)]
pub struct EnumItem {
    pub is_pub: bool,
    pub name: Spanned<String>,
    pub variants: Vec<Spanned<EnumVariant>>,
}

/// A single variant in an enum definition.
#[derive(Debug, Clone)]
pub struct EnumVariant {
    pub name: Spanned<String>,
    /// Optional payload types, e.g. `Ok(T)`.
    pub fields: Vec<Spanned<TypeExpr>>,
}

/// `[pub] type Name = Type`
#[derive(Debug, Clone)]
pub struct TypeAliasItem {
    pub is_pub: bool,
    pub name: Spanned<String>,
    pub ty: Spanned<TypeExpr>,
}

// ── Params ──────────────────────────────────────────────────────────────────

/// A function or define parameter.
#[derive(Debug, Clone)]
pub struct Param {
    pub name: Spanned<String>,
    pub ty: Option<Spanned<TypeExpr>>,
    pub default: Option<Spanned<Expr>>,
}

// ── Statements ──────────────────────────────────────────────────────────────

/// A statement within a block.
#[derive(Debug, Clone)]
pub enum Stmt {
    /// `let [mut] [sig] name [: Type] = expr`
    Binding(BindingStmt),
    /// `return expr`
    Return(Option<Spanned<Expr>>),
    /// `if cond { body } [else { body }]`
    If(IfStmt),
    /// `for pattern of iterable { body }`
    For(ForStmt),
    /// `while cond { body }`
    While(WhileStmt),
    /// A bare expression used as a statement.
    Expr(Spanned<Expr>),
    /// Placeholder for a statement that failed to parse.
    Error,
}

/// `let [mut] [sig] name [: Type] = expr`
#[derive(Debug, Clone)]
#[expect(clippy::struct_excessive_bools)]
pub struct BindingStmt {
    pub is_pub: bool,
    pub is_const: bool,
    pub is_mut: bool,
    pub is_sig: bool,
    pub name: Spanned<String>,
    pub ty: Option<Spanned<TypeExpr>>,
    pub value: Option<Spanned<Expr>>,
}

/// `if cond { body } [else if cond { body }]* [else { body }]`
#[derive(Debug, Clone)]
pub struct IfStmt {
    pub condition: Spanned<Expr>,
    pub then_body: Spanned<Expr>,
    pub else_body: Option<Spanned<Expr>>,
}

/// `for pattern of iterable { body }`
#[derive(Debug, Clone)]
pub struct ForStmt {
    pub binding: Spanned<String>,
    pub iterable: Spanned<Expr>,
    pub body: Spanned<Expr>,
}

/// `while cond { body }`
#[derive(Debug, Clone)]
pub struct WhileStmt {
    pub condition: Spanned<Expr>,
    pub body: Spanned<Expr>,
}

#[derive(Debug, Clone)]
pub struct WhenArm {
    pub pattern: Spanned<Pattern>,
    pub body: Spanned<Expr>,
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Wildcard,
    Binding(String),
    IntLiteral(i64),
    FloatLiteral(f64),
    StringLiteral(String),
    BoolLiteral(bool),
    Void,
    Variant {
        path: Vec<Spanned<String>>,
        fields: Vec<Spanned<Pattern>>,
    },
    Error,
}

// ── Expressions ─────────────────────────────────────────────────────────────

/// An expression.
#[derive(Debug, Clone)]
#[expect(clippy::use_self)]
pub enum Expr {
    /// Integer literal: `42`
    IntLiteral(i64),
    /// Float literal: `3.14`
    FloatLiteral(f64),
    /// String literal: `"hello"`
    StringLiteral(String),
    /// Interpolated string: `"Hello {name}!"`
    StringInterp(Vec<StringPart>),
    /// Boolean literal: `true` / `false`
    BoolLiteral(bool),
    /// `void`
    Void,
    /// An identifier: `foo`
    Ident(String),
    /// Binary operation: `a + b`
    Binary {
        left: Box<Spanned<Expr>>,
        op: Spanned<BinOp>,
        right: Box<Spanned<Expr>>,
    },
    /// Unary operation: `-x`, `!x`
    Unary {
        op: Spanned<UnaryOp>,
        operand: Box<Spanned<Expr>>,
    },
    /// Assignment: `x = expr`, `x += expr`
    Assign {
        target: Box<Spanned<Expr>>,
        op: Spanned<AssignOp>,
        value: Box<Spanned<Expr>>,
    },
    /// Function call: `foo(args)`
    Call {
        callee: Box<Spanned<Expr>>,
        args: Vec<Spanned<CallArg>>,
    },
    /// Field access: `a.b`
    Field {
        object: Box<Spanned<Expr>>,
        field: Spanned<String>,
    },
    /// Index access: `a[b]`
    Index {
        object: Box<Spanned<Expr>>,
        index: Box<Spanned<Expr>>,
    },
    /// Block: `{ stmts... }`
    Block(Vec<Spanned<Stmt>>),
    /// Pattern matching: `when subject { pattern -> expr }`
    When {
        subject: Box<Spanned<Expr>>,
        arms: Vec<Spanned<WhenArm>>,
    },
    /// Object literal: `{ key: value, ... }`
    Object(Vec<Spanned<ObjectField>>),
    /// HashMap literal: `#{ key: value, ... }`
    Map(Vec<Spanned<ObjectField>>),
    /// Array literal: `[a, b, c]`
    Array(Vec<Spanned<Expr>>),
    /// Node expression: `@name tokens... { body }`
    Node(Box<NodeExpr>),
    /// Parenthesized expression: `(expr)`
    Paren(Box<Spanned<Expr>>),
    /// `await expr`
    Await(Box<Spanned<Expr>>),
    /// Placeholder for an expression that failed to parse.
    Error,
}

/// A part of an interpolated string.
#[derive(Debug, Clone)]
pub enum StringPart {
    /// A literal text segment.
    Lit(String),
    /// An interpolated expression.
    Expr(Spanned<Expr>),
}

/// A call argument, possibly named.
#[derive(Debug, Clone)]
pub struct CallArg {
    /// Named argument: `foo(name=value)`.
    pub name: Option<Spanned<String>>,
    pub value: Spanned<Expr>,
}

/// A key-value pair in an object literal.
#[derive(Debug, Clone)]
pub struct ObjectField {
    pub key: Spanned<String>,
    pub value: Spanned<Expr>,
}

// ── Node expressions ────────────────────────────────────────────────────────

/// A dot-separated node name, e.g. `io.out`, `html`, `response`.
#[derive(Debug, Clone)]
pub struct NodeName {
    pub segments: Vec<Spanned<String>>,
}

impl std::fmt::Display for NodeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, seg) in self.segments.iter().enumerate() {
            if i > 0 {
                write!(f, ".")?;
            }
            write!(f, "{}", seg.node())?;
        }
        Ok(())
    }
}

/// `@name positional_tokens... %props... { body }`
#[derive(Debug, Clone)]
pub struct NodeExpr {
    /// The node name after `@`, e.g. `io.out`, `button`, `route`.
    pub name: Spanned<NodeName>,
    /// Positional tokens: string literals, integers, identifiers, paths,
    /// and other bare tokens that appear before `{` or end of line.
    pub positional: Vec<Spanned<Expr>>,
    /// Inline properties: `%key=value` on the same line.
    pub properties: Vec<Spanned<Property>>,
    /// The body block, if present: `{ children and statements }`.
    pub body: Option<Box<Spanned<Expr>>>,
}

/// A `%key=value` property binding on a node.
#[derive(Debug, Clone)]
pub struct Property {
    pub name: Spanned<String>,
    pub value: Spanned<Expr>,
}

// ── Types ───────────────────────────────────────────────────────────────────

/// A type expression in an annotation.
#[derive(Debug, Clone)]
pub enum TypeExpr {
    /// A simple named type: `i32`, `string`, `User`
    Named(String),
    /// A nullable type: `T?`
    Nullable(Box<Spanned<TypeExpr>>),
    /// A generic type: `Vec<T>`, `HashMap<K, V>`
    Generic {
        name: Spanned<String>,
        args: Vec<Spanned<TypeExpr>>,
    },
    /// A function type: `(A, B) -> C`
    Function {
        params: Vec<Spanned<TypeExpr>>,
        ret: Box<Spanned<TypeExpr>>,
    },
    /// A node type reference: `@html`
    Node(Spanned<NodeName>),
    /// Placeholder for a type that failed to parse.
    Error,
}

// ── Operators ───────────────────────────────────────────────────────────────

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
    Pipe,
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

/// Assignment operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    AddAssign,
    SubAssign,
}

// ── Display helpers for AST dump ────────────────────────────────────────────

impl std::fmt::Display for BinOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Add => write!(f, "+"),
            Self::Sub => write!(f, "-"),
            Self::Mul => write!(f, "*"),
            Self::Div => write!(f, "/"),
            Self::Eq => write!(f, "=="),
            Self::NotEq => write!(f, "!="),
            Self::Lt => write!(f, "<"),
            Self::LtEq => write!(f, "<="),
            Self::Gt => write!(f, ">"),
            Self::GtEq => write!(f, ">="),
            Self::And => write!(f, "&&"),
            Self::Or => write!(f, "||"),
            Self::Pipe => write!(f, "|>"),
        }
    }
}

impl std::fmt::Display for UnaryOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Neg => write!(f, "-"),
            Self::Not => write!(f, "!"),
        }
    }
}

impl std::fmt::Display for AssignOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Assign => write!(f, "="),
            Self::AddAssign => write!(f, "+="),
            Self::SubAssign => write!(f, "-="),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orv_span::{FileId, Span, Spanned};

    #[test]
    fn node_name_display() {
        let file = FileId::new(0);
        let name = NodeName {
            segments: vec![
                Spanned::new("io".to_owned(), Span::new(file, 0, 2)),
                Spanned::new("out".to_owned(), Span::new(file, 3, 6)),
            ],
        };
        assert_eq!(name.to_string(), "io.out");
    }

    #[test]
    fn single_segment_node_name() {
        let file = FileId::new(0);
        let name = NodeName {
            segments: vec![Spanned::new("button".to_owned(), Span::new(file, 0, 6))],
        };
        assert_eq!(name.to_string(), "button");
    }

    #[test]
    fn binop_display() {
        assert_eq!(BinOp::Add.to_string(), "+");
        assert_eq!(BinOp::And.to_string(), "&&");
        assert_eq!(BinOp::Pipe.to_string(), "|>");
    }

    #[test]
    fn assign_op_display() {
        assert_eq!(AssignOp::Assign.to_string(), "=");
        assert_eq!(AssignOp::AddAssign.to_string(), "+=");
        assert_eq!(AssignOp::SubAssign.to_string(), "-=");
    }
}
