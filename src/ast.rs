use crate::lexer::Span;

#[derive(Debug, Clone)]
pub struct Program {
    pub block: Block,
}

pub type Block = Vec<Stmt>;

#[derive(Debug, Clone)]
pub enum Stmt {
    Local(LocalDecl),
    State(StateDecl),
    Function(FunctionDecl),
    Object(ObjectDecl),
    Enum(EnumDecl),
    Signal(SignalDecl),
    Fire(FireStmt),
    SignalHandler(SignalHandlerStmt),
    Watch(WatchStmt),
    Assignment(Assignment),
    CompoundAssignment {
        span: Span,
        target: AssignTarget,
        op: CompoundOp,
        value: Expr,
    },
    NullishAssignment {
        span: Span,
        target: AssignTarget,
        value: Expr,
    },
    Call(Expr, Span),
    Return(Vec<Expr>, Span),
    If(IfStmt),
    ComptimeIf(IfStmt),
    Switch(SwitchStmt),
    ComptimeSwitch(SwitchStmt),
    Match(MatchStmt),
    While(WhileStmt),
    Repeat(RepeatStmt),
    ForNumeric(ForNumeric),
    ForGeneric(ForGeneric),
    Do(Block, Span),
    Break(Span),
    Continue(Span),
    Fallthrough(Span),
    Spawn(SpawnStmt),
    TypeAlias {
        raw: String,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub struct StateDecl {
    pub span: Span,
    pub binding: Binding,
    pub value: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct SignalDecl {
    pub span: Span,
    pub name: String,
    pub params: Vec<SignalParam>,
}

#[derive(Debug, Clone)]
pub struct SignalParam {
    pub name: String,
    pub annotation: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FireStmt {
    pub span: Span,
    pub signal: Expr,
    pub args: Vec<Expr>,
}

#[derive(Debug, Clone)]
pub struct SignalHandlerStmt {
    pub span: Span,
    pub signal: Expr,
    pub params: Vec<String>,
    pub body: Block,
    pub once: bool,
}

#[derive(Debug, Clone)]
pub struct WatchStmt {
    pub span: Span,
    pub name: String,
    pub params: Vec<String>,
    pub body: Block,
}

#[derive(Debug, Clone)]
pub struct ObjectDecl {
    pub span: Span,
    pub name: String,
    pub extends: Option<String>,
    pub fields: Vec<ObjectField>,
    pub methods: Vec<ObjectMethod>,
}

#[derive(Debug, Clone)]
pub struct ObjectField {
    pub name: String,
    pub annotation: String,
}

#[derive(Debug, Clone)]
pub struct ObjectMethod {
    pub name: String,
    pub is_static: bool,
    pub generics: Option<String>,
    pub params: Vec<Param>,
    pub return_type: Option<String>,
    pub body: Block,
}

#[derive(Debug, Clone)]
pub struct SpawnStmt {
    pub span: Span,
    pub call: Expr,
    pub then_handler: Option<SpawnHandler>,
    pub catch_handler: Option<SpawnHandler>,
}

#[derive(Debug, Clone)]
pub struct SpawnHandler {
    pub params: Vec<String>,
    pub block: Block,
}

#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub span: Span,
    pub name: String,
    pub base_type: Option<String>,
    pub members: Vec<EnumMember>,
}

#[derive(Debug, Clone)]
pub struct EnumMember {
    pub name: String,
    pub value: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct LocalDecl {
    pub span: Span,
    pub is_const: bool,
    pub is_comptime: bool,
    pub bindings: Vec<Binding>,
    pub values: Vec<Expr>,
}

#[derive(Debug, Clone)]
pub struct Binding {
    pub pattern: Pattern,
    pub type_annotation: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Name(String),
    Table {
        entries: Vec<TablePatternEntry>,
        rest: Option<String>,
    },
    Array {
        items: Vec<ArrayPatternItem>,
        rest: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct TablePatternEntry {
    pub key: String,
    pub binding: PatternBinding,
}

#[derive(Debug, Clone)]
pub struct ArrayPatternItem {
    pub binding: Option<PatternBinding>,
}

#[derive(Debug, Clone)]
pub struct PatternBinding {
    pub target: Pattern,
    pub default_value: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct Assignment {
    pub span: Span,
    pub targets: Vec<AssignTarget>,
    pub values: Vec<Expr>,
}

#[derive(Debug, Clone)]
pub enum AssignTarget {
    Name(String),
    Field { object: Box<Expr>, field: String },
    Index { object: Box<Expr>, index: Box<Expr> },
}

#[derive(Debug, Clone)]
pub struct FunctionDecl {
    pub span: Span,
    pub local_name: bool,
    pub is_task: bool,
    pub is_comptime: bool,
    pub name: FunctionName,
    pub generics: Option<String>,
    pub params: Vec<Param>,
    pub return_type: Option<String>,
    pub body: Block,
}

#[derive(Debug, Clone)]
pub struct FunctionName {
    pub root: String,
    pub fields: Vec<String>,
    pub method: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Param {
    Binding(Binding),
    VarArg(Option<String>),
}

#[derive(Debug, Clone)]
pub struct IfStmt {
    pub span: Span,
    pub branches: Vec<(Expr, Block)>,
    pub else_block: Option<Block>,
}

#[derive(Debug, Clone)]
pub struct SwitchStmt {
    pub span: Span,
    pub value: Expr,
    pub cases: Vec<SwitchCase>,
    pub default: Option<Block>,
}

#[derive(Debug, Clone)]
pub struct SwitchCase {
    pub value: Expr,
    pub block: Block,
    pub fallthrough: bool,
}

#[derive(Debug, Clone)]
pub struct MatchStmt {
    pub span: Span,
    pub value: Expr,
    pub cases: Vec<MatchCase>,
}

#[derive(Debug, Clone)]
pub struct MatchCase {
    pub pattern: MatchPattern,
    pub guard: Option<Expr>,
    pub block: Block,
}

#[derive(Debug, Clone)]
pub struct WhileStmt {
    pub span: Span,
    pub condition: Expr,
    pub block: Block,
}

#[derive(Debug, Clone)]
pub struct RepeatStmt {
    pub span: Span,
    pub block: Block,
    pub condition: Expr,
}

#[derive(Debug, Clone)]
pub enum MatchPattern {
    Literal(Expr),
    Bind(String),
    Table(Vec<MatchFieldPattern>),
}

#[derive(Debug, Clone)]
pub struct MatchFieldPattern {
    pub key: String,
    pub pattern: MatchPattern,
}

#[derive(Debug, Clone)]
pub struct ForNumeric {
    pub span: Span,
    pub name: String,
    pub start: Expr,
    pub end: Expr,
    pub step: Option<Expr>,
    pub block: Block,
}

#[derive(Debug, Clone)]
pub struct ForGeneric {
    pub span: Span,
    pub bindings: Vec<Binding>,
    pub iterables: Vec<Expr>,
    pub block: Block,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Nil,
    Bool(bool),
    Number(String),
    String(String),
    Pattern(String),
    VarArg,
    Name(String),
    Table(Vec<TableField>),
    Function(FunctionExpr),
    Freeze(Box<Expr>),
    Comptime(Box<Expr>),
    Yield(Box<Expr>),
    IfElse {
        branches: Vec<(Expr, Expr)>,
        else_expr: Box<Expr>,
    },
    DoExpr {
        block: Block,
        result: Box<Expr>,
    },
    SwitchExpr {
        value: Box<Expr>,
        cases: Vec<SwitchExprCase>,
        default: Box<Expr>,
    },
    Paren(Box<Expr>),
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    TypeAssertion {
        expr: Box<Expr>,
        annotation: String,
    },
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },
    Ternary {
        condition: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
    },
    Chain {
        base: Box<Expr>,
        segments: Vec<ChainSegment>,
    },
    Pipe {
        left: Box<Expr>,
        stages: Vec<PipeStage>,
    },
    Comprehension(Box<TableComprehension>),
    SignalHandler(Box<SignalHandlerExpr>),
}

#[derive(Debug, Clone)]
pub struct SignalHandlerExpr {
    pub signal: Expr,
    pub params: Vec<String>,
    pub body: Block,
    pub once: bool,
}

#[derive(Debug, Clone)]
pub struct SwitchExprCase {
    pub value: Expr,
    pub result: Expr,
}

#[derive(Debug, Clone)]
pub enum TableField {
    Named(String, Expr),
    Indexed(Expr, Expr),
    Value(Expr),
}

#[derive(Debug, Clone)]
pub struct TableComprehension {
    pub kind: TableComprehensionKind,
    pub clauses: Vec<ComprehensionClause>,
}

#[derive(Debug, Clone)]
pub enum TableComprehensionKind {
    Array { value: Box<Expr> },
    Map { key: Box<Expr>, value: Box<Expr> },
}

#[derive(Debug, Clone)]
pub enum ComprehensionClause {
    GenericFor {
        bindings: Vec<Binding>,
        iterables: Vec<Expr>,
    },
    NumericFor {
        name: String,
        start: Expr,
        end: Expr,
        step: Option<Expr>,
    },
    Filter(Expr),
}

#[derive(Debug, Clone)]
pub struct FunctionExpr {
    pub generics: Option<String>,
    pub params: Vec<Param>,
    pub return_type: Option<String>,
    pub body: Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Negate,
    Not,
    Length,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Or,
    And,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
    Concat,
    Add,
    Subtract,
    Multiply,
    Divide,
    FloorDivide,
    Modulo,
    Power,
    Nullish,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompoundOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    FloorDivide,
    Modulo,
    Power,
    Concat,
}

#[derive(Debug, Clone)]
pub enum ChainSegment {
    Field {
        name: String,
        safe: bool,
    },
    Index {
        expr: Box<Expr>,
        safe: bool,
    },
    Call {
        type_args: Option<Vec<String>>,
        args: Vec<Expr>,
    },
    MethodCall {
        name: String,
        type_args: Option<Vec<String>>,
        args: Vec<Expr>,
        safe: bool,
    },
}

#[derive(Debug, Clone)]
pub enum PipeStage {
    Method { name: String, args: Vec<Expr> },
    Expr { callee: Box<Expr> },
    Call { callee: Box<Expr>, args: Vec<Expr> },
}
