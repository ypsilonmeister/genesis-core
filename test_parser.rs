use serde::{Deserialize, Serialize};
use std::cell::Cell;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum Token {
    Number(f64),
    NaN,
    Infinity,
    Plus,
    Minus,
    Star,
    StarStar,
    Slash,
    DoubleSlash,
    Caret,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Exclamation,
    Question,
    Colon,
    DotDot,
    LShift,
    RShift,
    Gt,
    Lt,
    Ge,
    Le,
    Eq,
    Ne,
    Percent,
    Sqrt,
    Cbrt,
    Pi,
    E,
    At,
    Dollar,
    Ampersand,
    Pipe,
    BitXor,
    LogicalAnd,
    LogicalOr,
    Assign,
    Semicolon,
    Sum,
    Sin,
    Cos,
    Tan,
    Log,
    Ln,
    Exp,
    Abs,
    Function(String),
    String(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Expr {
    Number(f64),
    Variable(String),
    BinOp {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    UnaryOp {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    FunctionCall {
        name: String,
        args: Vec<Expr>,
    },
    Sequence(Vec<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Pow,
    Mod,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
    Assign,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Range,
    At,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UnaryOp {
    Neg,
    Pos,
    Fact,
    Percent,
    Not,
}

// ... include Parser and other structs ...
// (Omitted for brevity in this thought, but I'll include them in the file)

fn main() {
    // Test logic here
}
