pub type SymbolRef = u32;
pub type ScopeRef = u32;

#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub symbol: Option<SymbolRef>,
    pub kind: ItemKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    Import(ImportItem),
    Function(FunctionItem),
    Define(DefineItem),
    Struct(StructItem),
    Enum(EnumItem),
    TypeAlias(TypeAliasItem),
    Binding(Binding),
    Stmt(Stmt),
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportItem {
    pub path: Vec<String>,
    pub names: Vec<String>,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionItem {
    pub name: String,
    pub is_pub: bool,
    pub is_async: bool,
    pub scope: ScopeRef,
    pub params: Vec<Param>,
    pub return_type: Option<Type>,
    pub body: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DefineItem {
    pub name: String,
    pub is_pub: bool,
    pub scope: ScopeRef,
    pub params: Vec<Param>,
    pub return_domain: Option<String>,
    pub body: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructItem {
    pub name: String,
    pub is_pub: bool,
    pub fields: Vec<StructField>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructField {
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumItem {
    pub name: String,
    pub is_pub: bool,
    pub variants: Vec<EnumVariant>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariant {
    pub name: String,
    pub fields: Vec<Type>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeAliasItem {
    pub name: String,
    pub is_pub: bool,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub symbol: Option<SymbolRef>,
    pub name: String,
    pub ty: Option<Type>,
    pub default: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
#[expect(clippy::struct_excessive_bools)]
pub struct Binding {
    pub symbol: Option<SymbolRef>,
    pub name: String,
    pub is_pub: bool,
    pub is_const: bool,
    pub is_mut: bool,
    pub is_sig: bool,
    pub ty: Option<Type>,
    pub value: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Binding(Binding),
    Return(Option<Expr>),
    If(IfStmt),
    For(ForStmt),
    While(WhileStmt),
    Expr(Expr),
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IfStmt {
    pub condition: Expr,
    pub then_scope: ScopeRef,
    pub then_body: Expr,
    pub else_scope: Option<ScopeRef>,
    pub else_body: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForStmt {
    pub scope: ScopeRef,
    pub binding: String,
    pub binding_symbol: Option<SymbolRef>,
    pub iterable: Expr,
    pub body: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WhileStmt {
    pub scope: ScopeRef,
    pub condition: Expr,
    pub body: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    IntLiteral(i64),
    FloatLiteral(f64),
    StringLiteral(String),
    StringInterp(Vec<StringPart>),
    BoolLiteral(bool),
    Void,
    Ident(ResolvedName),
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Assign {
        target: Box<Expr>,
        op: AssignOp,
        value: Box<Expr>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<CallArg>,
    },
    Field {
        object: Box<Expr>,
        field: String,
    },
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
    },
    Block {
        scope: ScopeRef,
        stmts: Vec<Stmt>,
    },
    Object(Vec<ObjectField>),
    Array(Vec<Expr>),
    Node(NodeExpr),
    Paren(Box<Expr>),
    Await(Box<Expr>),
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StringPart {
    Lit(String),
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedName {
    pub name: String,
    pub symbol: Option<SymbolRef>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CallArg {
    pub name: Option<String>,
    pub value: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObjectField {
    pub key: String,
    pub value: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NodeExpr {
    pub name: String,
    pub positional: Vec<Expr>,
    pub properties: Vec<Property>,
    pub body: Option<Box<Expr>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub name: String,
    pub value: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Named(String),
    Nullable(Box<Type>),
    Generic { name: String, args: Vec<Type> },
    Function { params: Vec<Type>, ret: Box<Type> },
    Node(String),
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    AddAssign,
    SubAssign,
}
