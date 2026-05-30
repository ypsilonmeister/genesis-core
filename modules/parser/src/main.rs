// # CMP Module Charter
//
// What:
//   Convert a sequence of tokens into an Abstract Syntax Tree (AST) considering operator precedence.
//
// Invariants:
//   - Operator precedence: ^ (power) > * / % (mod) > + -
//   - Handle priority changes by parentheses correctly
//   - Return an error for invalid syntax (consecutive operators, mismatched parentheses, etc.)
//
// Boundaries:
//   - Dependencies: tokenizer
//   - Dependents: evaluator
//
// Extensible:
//   - Addition of new operators and function call syntax
//
// Why:
//   Isolate syntax parsing so that the evaluator can focus on pure calculation logic.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::env;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use compat::UnixListener;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleRequest {
    pub request_id: String,
    pub input: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleResponse {
    pub request_id: String,
    pub output: Option<String>,
    pub error: Option<ModuleError>,
    pub processing_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleError {
    pub code: String,
    pub message: String,
    pub input_position: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Token {
    Number(f64),
    NaN,
    Infinity,
    Plus,
    Minus,
    PlusMinus,
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
    Factorial,
    Question,
    Colon,
    Dot,
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
    Mod,
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
    Integral,
    Differential(String),
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Sinh,
    Cosh,
    Tanh,
    Log,
    Log10,
    Log2,
    Ln,
    Exp,
    Abs,
    Floor,
    Ceil,
    Round,
    I,
    J,
    Imaginary(f64),
    Pow,
    Function(String),
    String(String),
    Variable(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Expr {
    Number(f64),
    String(String),
    Variable(String),
    Infinity,
    NegInfinity,
    NaN,
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
    PlusMinus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UnaryOp {
    Neg,
    Pos,
    Fact,
    Percent,
    Not,
    Sqrt,
    Cbrt,
    Log,
    PlusMinus,
    Log10,
    Log2,
    Ln,
    Exp,
    Abs,
    Floor,
    Ceil,
    Round,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Sinh,
    Cosh,
    Tanh,
}

const MAX_RECURSION_DEPTH: usize = 512;

pub struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    depth: usize,
}

fn normalize_str(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| match c {
            '０'..='９' => std::char::from_u32(c as u32 - 0xFF10 + '0' as u32).unwrap_or(c),
            '．' | '・' | '·' | '⸱' => '.',
            '＋' => '+',
            '－' | '−' | 'ー' | '—' | '‐' | '‑' => '-',
            '＊' | '×' | '✕' | '✖' | '⋅' | '∗' => '*',
            '／' | '÷' | '∕' | '∖' => '/',
            '（' => '(',
            '）' => ')',
            '［' => '[',
            '］' => ']',
            '｛' => '{',
            '｝' => '}',
            '＾' => '^',
            '＝' => '=',
            '，' => ',',
            '；' => ';',
            '！' => '!',
            '％' => '%',
            'π' | '∏' => 'π',
            '∞' => '∞',
            'Σ' | '∑' => 'σ',
            _ => c,
        })
        .collect::<String>()
        .to_lowercase()
}

impl<'a> Parser<'a> {
    pub fn new(tokens: &'a [Token]) -> Self {
        Self {
            tokens,
            pos: 0,
            depth: 0,
        }
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.tokens.len() {
            if let Token::Variable(s) = &self.tokens[self.pos] {
                if s.trim().is_empty() {
                    self.pos += 1;
                    continue;
                }
            }
            break;
        }
    }

    fn peek(&mut self) -> Option<&'a Token> {
        self.skip_whitespace();
        self.tokens.get(self.pos)
    }

    fn peek_next(&mut self) -> Option<&'a Token> {
        self.skip_whitespace();
        let current_pos = self.pos;
        self.pos += 1;
        self.skip_whitespace();
        let res = self.tokens.get(self.pos);
        self.pos = current_pos;
        res
    }

    fn next(&mut self) -> Option<&'a Token> {
        self.skip_whitespace();
        let t = self.tokens.get(self.pos);
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn check_depth(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > MAX_RECURSION_DEPTH {
            return Err("Max recursion/complexity depth exceeded".to_string());
        }
        Ok(())
    }

    fn exit_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    pub fn parse(&mut self) -> Result<Expr, String> {
        if self.tokens.is_empty() {
            return Err("Empty token stream".to_string());
        }
        let mut exprs = Vec::new();
        loop {
            while let Some(t) = self.peek() {
                match t {
                    Token::Comma | Token::Semicolon => { self.next(); }
                    Token::Variable(s) if normalize_str(s) == "," || normalize_str(s) == ";" => { self.next(); }
                    _ => break,
                }
            }
            if self.peek().is_none() { break; }
            exprs.push(self.parse_expression()?);
            if self.peek().is_none() { break; }
        }
        if self.pos < self.tokens.len() {
            self.skip_whitespace();
            if self.pos < self.tokens.len() {
                return Err(format!("Unexpected token at position {}: {:?}", self.pos, self.tokens[self.pos]));
            }
        }
        if exprs.is_empty() { return Err("No expression found".to_string()); }
        if exprs.len() == 1 { Ok(exprs.remove(0)) }
        else { Ok(Expr::FunctionCall { name: "sequence".to_string(), args: exprs }) }
    }

    fn parse_expression(&mut self) -> Result<Expr, String> {
        self.parse_assignment()
    }

    fn parse_assignment(&mut self) -> Result<Expr, String> {
        self.check_depth()?;
        let mut lhs = self.parse_logical_or()?;
        while let Some(t) = self.peek() {
            let matches = match t {
                Token::Assign => true,
                Token::Variable(s) if normalize_str(s) == "=" => true,
                _ => false,
            };
            if matches {
                self.next();
                let rhs = self.parse_assignment()?;
                lhs = Expr::BinOp { op: BinOp::Assign, lhs: Box::new(lhs), rhs: Box::new(rhs) };
            } else { break; }
        }
        self.exit_depth();
        Ok(lhs)
    }

    fn parse_logical_or(&mut self) -> Result<Expr, String> {
        self.check_depth()?;
        let mut lhs = self.parse_logical_and()?;
        while let Some(t) = self.peek() {
            let op = match t {
                Token::LogicalOr => Some(BinOp::Or),
                Token::Variable(s) => {
                    let n = normalize_str(s);
                    if n == "||" || n == "or" { Some(BinOp::Or) } else { None }
                }
                _ => None,
            };
            if let Some(op) = op {
                self.next();
                let rhs = self.parse_logical_and()?;
                lhs = Expr::BinOp { op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
            } else { break; }
        }
        self.exit_depth();
        Ok(lhs)
    }

    fn parse_logical_and(&mut self) -> Result<Expr, String> {
        self.check_depth()?;
        let mut lhs = self.parse_bitwise_or()?;
        while let Some(t) = self.peek() {
            let op = match t {
                Token::LogicalAnd => Some(BinOp::And),
                Token::Variable(s) => {
                    let n = normalize_str(s);
                    if n == "&&" || n == "and" { Some(BinOp::And) } else { None }
                }
                _ => None,
            };
            if let Some(op) = op {
                self.next();
                let rhs = self.parse_bitwise_or()?;
                lhs = Expr::BinOp { op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
            } else { break; }
        }
        self.exit_depth();
        Ok(lhs)
    }

    fn parse_bitwise_or(&mut self) -> Result<Expr, String> {
        self.check_depth()?;
        let mut lhs = self.parse_bitwise_xor()?;
        while let Some(t) = self.peek() {
            let matches = match t {
                Token::Pipe => true,
                Token::Variable(s) if normalize_str(s) == "|" => true,
                _ => false,
            };
            if matches {
                self.next();
                let rhs = self.parse_bitwise_xor()?;
                lhs = Expr::BinOp { op: BinOp::BitOr, lhs: Box::new(lhs), rhs: Box::new(rhs) };
            } else { break; }
        }
        self.exit_depth();
        Ok(lhs)
    }

    fn parse_bitwise_xor(&mut self) -> Result<Expr, String> {
        self.check_depth()?;
        let mut lhs = self.parse_bitwise_and()?;
        while let Some(t) = self.peek() {
            let matches = match t {
                Token::Variable(s) if normalize_str(s) == "xor" => true, 
                _ => false,
            };
            if matches {
                self.next();
                let rhs = self.parse_bitwise_and()?;
                lhs = Expr::BinOp { op: BinOp::BitXor, lhs: Box::new(lhs), rhs: Box::new(rhs) };
            } else { break; }
        }
        self.exit_depth();
        Ok(lhs)
    }

    fn parse_bitwise_and(&mut self) -> Result<Expr, String> {
        self.check_depth()?;
        let mut lhs = self.parse_comparison()?;
        while let Some(t) = self.peek() {
            let matches = match t {
                Token::Ampersand => true,
                Token::Variable(s) if normalize_str(s) == "&" => true,
                _ => false,
            };
            if matches {
                self.next();
                let rhs = self.parse_comparison()?;
                lhs = Expr::BinOp { op: BinOp::BitAnd, lhs: Box::new(lhs), rhs: Box::new(rhs) };
            } else { break; }
        }
        self.exit_depth();
        Ok(lhs)
    }

    fn parse_comparison(&mut self) -> Result<Expr, String> {
        self.check_depth()?;
        let mut lhs = self.parse_range()?;
        while let Some(t) = self.peek() {
            let op = match t {
                Token::Eq => BinOp::Eq,
                Token::Ne => BinOp::Ne,
                Token::Lt => BinOp::Lt,
                Token::Gt => BinOp::Gt,
                Token::Le => BinOp::Le,
                Token::Ge => BinOp::Ge,
                Token::Variable(s) => {
                    let n = normalize_str(s);
                    match n.as_str() {
                        "==" => BinOp::Eq,
                        "!=" | "<>" => BinOp::Ne,
                        "<" => BinOp::Lt,
                        ">" => BinOp::Gt,
                        "<=" => BinOp::Le,
                        ">=" => BinOp::Ge,
                        _ => break,
                    }
                }
                _ => break,
            };
            self.next();
            let rhs = self.parse_range()?;
            lhs = Expr::BinOp { op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        self.exit_depth();
        Ok(lhs)
    }

    fn parse_range(&mut self) -> Result<Expr, String> {
        self.check_depth()?;
        let mut lhs = self.parse_shift()?;
        while let Some(t) = self.peek() {
            let matches = match t {
                Token::DotDot => true,
                Token::Variable(s) if normalize_str(s) == ".." => true,
                _ => false,
            };
            if matches {
                self.next();
                let rhs = self.parse_shift()?;
                lhs = Expr::BinOp { op: BinOp::Range, lhs: Box::new(lhs), rhs: Box::new(rhs) };
            } else { break; }
        }
        self.exit_depth();
        Ok(lhs)
    }

    fn parse_shift(&mut self) -> Result<Expr, String> {
        self.check_depth()?;
        let mut lhs = self.parse_term()?;
        while let Some(t) = self.peek() {
            let op = match t {
                Token::LShift => BinOp::Shl,
                Token::RShift => BinOp::Shr,
                Token::Variable(s) => {
                    let n = normalize_str(s);
                    match n.as_str() {
                        "<<" => BinOp::Shl,
                        ">>" => BinOp::Shr,
                        _ => break,
                    }
                }
                _ => break,
            };
            self.next();
            let rhs = self.parse_term()?;
            lhs = Expr::BinOp { op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        self.exit_depth();
        Ok(lhs)
    }

    fn parse_term(&mut self) -> Result<Expr, String> {
        self.check_depth()?;
        let mut lhs = self.parse_factor()?;
        while let Some(t) = self.peek() {
            let op = match t {
                Token::Plus => BinOp::Add,
                Token::Minus => BinOp::Sub,
                Token::PlusMinus => BinOp::PlusMinus,
                Token::Variable(s) => {
                    let n = normalize_str(s);
                    match n.as_str() {
                        "+" => BinOp::Add,
                        "-" | "−" | "ー" | "—" | "‐" | "‑" => BinOp::Sub,
                        "±" => BinOp::PlusMinus,
                        _ => break,
                    }
                }
                _ => break,
            };
            self.next();
            let rhs = self.parse_factor()?;
            lhs = Expr::BinOp { op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        self.exit_depth();
        Ok(lhs)
    }

    fn parse_factor(&mut self) -> Result<Expr, String> {
        self.check_depth()?;
        let mut lhs = self.parse_implicit_factor()?;
        while let Some(t) = self.peek() {
            let (op, consume) = match t {
                Token::Star => (BinOp::Mul, true),
                Token::Slash => {
                    if self.peek_next().map_or(false, |nt| matches!(nt, Token::Slash) || matches!(nt, Token::DoubleSlash) || matches!(nt, Token::Variable(s) if normalize_str(s) == "/")) {
                        self.next(); (BinOp::FloorDiv, true)
                    } else { (BinOp::Div, true) }
                }
                Token::DoubleSlash => (BinOp::FloorDiv, true),
                Token::Mod => (BinOp::Mod, true),
                Token::At => (BinOp::At, true),
                Token::Variable(s) => {
                    let n = normalize_str(s);
                    match n.as_str() {
                        "*" | "×" => (BinOp::Mul, true),
                        "/" | "÷" => {
                            if self.peek_next().map_or(false, |nt| matches!(nt, Token::Slash) || matches!(nt, Token::DoubleSlash) || matches!(nt, Token::Variable(s2) if normalize_str(s2) == "/")) {
                                self.next(); (BinOp::FloorDiv, true)
                            } else { (BinOp::Div, true) }
                        }
                        "mod" => (BinOp::Mod, true),
                        "//" => (BinOp::FloorDiv, true),
                        "%" => {
                            if let Some(next) = self.peek_next() {
                                if Self::is_expression_start(next) && !matches!(next, Token::Plus | Token::Minus | Token::RParen | Token::RBracket | Token::RBrace | Token::Comma | Token::Semicolon) {
                                    (BinOp::Mod, true)
                                } else { break; }
                            } else { break; }
                        }
                        _ => break,
                    }
                }
                Token::Percent => {
                    if let Some(next) = self.peek_next() {
                        if Self::is_expression_start(next) && !matches!(next, Token::Plus | Token::Minus | Token::RParen | Token::RBracket | Token::RBrace | Token::Comma | Token::Semicolon) {
                            (BinOp::Mod, true)
                        } else { break; }
                    } else { break; }
                }
                _ => break,
            };
            if consume { self.next(); }
            let rhs = self.parse_implicit_factor()?;
            lhs = Expr::BinOp { op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
        }
        self.exit_depth();
        Ok(lhs)
    }

    fn parse_implicit_factor(&mut self) -> Result<Expr, String> {
        let mut lhs = self.parse_power()?;
        while let Some(t) = self.peek() {
            if Self::is_implicit_mul_start(t) {
                let rhs = self.parse_power()?;
                lhs = Expr::BinOp { op: BinOp::Mul, lhs: Box::new(lhs), rhs: Box::new(rhs) };
            } else { break; }
        }
        Ok(lhs)
    }

    fn parse_power(&mut self) -> Result<Expr, String> {
        self.check_depth()?;
        let mut lhs = self.parse_unary()?;
        while let Some(t) = self.peek() {
            let op = match t {
                Token::StarStar | Token::Caret | Token::Pow | Token::BitXor => Some(BinOp::Pow),
                Token::Star if self.peek_next().map_or(false, |nt| matches!(nt, Token::Star) || matches!(nt, Token::Variable(s) if normalize_str(s) == "*")) => {
                    self.next(); Some(BinOp::Pow)
                }
                Token::Variable(s) => {
                    let n = normalize_str(s);
                    if n == "^" || n == "**" || n == "pow" { Some(BinOp::Pow) } 
                    else if n == "*" && self.peek_next().map_or(false, |nt| matches!(nt, Token::Star) || matches!(nt, Token::Variable(s2) if normalize_str(s2) == "*")) {
                        self.next(); Some(BinOp::Pow)
                    }
                    else { None }
                }
                _ => None,
            };
            if let Some(op) = op {
                self.next();
                let rhs = self.parse_power()?; 
                lhs = Expr::BinOp { op, lhs: Box::new(lhs), rhs: Box::new(rhs) };
            } else { break; }
        }
        self.exit_depth();
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        self.check_depth()?;
        if let Some(t) = self.peek() {
            let op = match t {
                Token::Plus => Some(UnaryOp::Pos),
                Token::Minus => Some(UnaryOp::Neg),
                Token::PlusMinus => Some(UnaryOp::PlusMinus),
                Token::Exclamation => Some(UnaryOp::Not),
                Token::Variable(s) => {
                    let n = normalize_str(s);
                    match n.as_str() {
                        "+" => Some(UnaryOp::Pos),
                        "-" | "−" | "ー" | "—" | "‐" | "‑" => Some(UnaryOp::Neg),
                        "!" | "not" => Some(UnaryOp::Not),
                        "±" => Some(UnaryOp::PlusMinus),
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some(op) = op {
                self.next();
                let expr = self.parse_unary()?;
                self.exit_depth();
                return Ok(Expr::UnaryOp { op, expr: Box::new(expr) });
            }
        }
        let res = self.parse_postfix();
        self.exit_depth();
        res
    }

    fn parse_postfix(&mut self) -> Result<Expr, String> {
        self.check_depth()?;
        let mut lhs = self.parse_primary()?;
        while let Some(t) = self.peek() {
            let (op, count) = match t {
                Token::Factorial | Token::Exclamation => (UnaryOp::Fact, 1),
                Token::Percent => {
                    if let Some(next) = self.peek_next() {
                        if Self::is_expression_start(next) && !matches!(next, Token::Plus | Token::Minus | Token::RParen | Token::RBracket | Token::RBrace | Token::Comma | Token::Semicolon) {
                            break; 
                        }
                    }
                    (UnaryOp::Percent, 1)
                }
                Token::Variable(s) => {
                    let n = normalize_str(s);
                    if n.chars().all(|c| c == '!') && !n.is_empty() { (UnaryOp::Fact, n.len()) }
                    else if n == "%" {
                        if let Some(next) = self.peek_next() {
                            if Self::is_expression_start(next) && !matches!(next, Token::Plus | Token::Minus | Token::RParen | Token::RBracket | Token::RBrace | Token::Comma | Token::Semicolon) {
                                break; 
                            }
                        }
                        (UnaryOp::Percent, 1)
                    } else { break; }
                }
                _ => break,
            };
            self.next();
            for _ in 0..count { lhs = Expr::UnaryOp { op, expr: Box::new(lhs) }; }
        }
        self.exit_depth();
        Ok(lhs)
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        self.check_depth()?;
        let t = self.next().ok_or_else(|| "Unexpected end of input".to_string())?;
        let res = match t {
            Token::Number(n) => self.parse_number(*n),
            Token::NaN => Ok(Expr::NaN),
            Token::Infinity => Ok(Expr::Infinity),
            Token::Pi => {
                if self.is_next_call_paren() { self.parse_function_call(&Token::Pi) }
                else { Ok(Expr::Number(std::f64::consts::PI)) }
            }
            Token::E => {
                if self.is_next_call_paren() { self.parse_function_call(&Token::E) }
                else { Ok(Expr::Number(std::f64::consts::E)) }
            }
            Token::I => Ok(Expr::Variable("i".to_string())),
            Token::J => Ok(Expr::Variable("j".to_string())),
            Token::Imaginary(n) => Ok(Expr::FunctionCall { name: "imaginary".to_string(), args: vec![Expr::Number(*n)] }),
            Token::String(s) => Ok(Expr::String(s.clone())),
            Token::LParen => self.parse_grouping(")", Token::RParen),
            Token::LBracket => self.parse_grouping("]", Token::RBracket),
            Token::LBrace => self.parse_grouping("}", Token::RBrace),
            Token::Dot => {
                if let Some(next_t) = self.peek() {
                    if let Some(val) = Self::get_number_value(next_t) {
                        let digits = self.get_literal_digits(next_t);
                        self.next();
                        Ok(Expr::Number(val / (10.0f64.powi(digits as i32))))
                    } else { Ok(Expr::Variable(".".to_string())) }
                } else { Ok(Expr::Variable(".".to_string())) }
            }
            Token::Dollar => Ok(Expr::Variable("$".to_string())),
            _ if Self::is_function(t) => self.parse_function_call(t),
            Token::Function(s) | Token::Variable(s) => {
                let name = normalize_str(s);
                if name == "(" { return self.parse_grouping(")", Token::RParen); }
                if name == "[" { return self.parse_grouping("]", Token::RBracket); }
                if name == "{" { return self.parse_grouping("}", Token::RBrace); }
                if name == "π" || name == "pi" {
                    if self.is_next_call_paren() { return self.parse_function_call(&Token::Variable(name.clone())); }
                    return Ok(Expr::Number(std::f64::consts::PI));
                }
                if name == "e" {
                    if self.is_next_call_paren() { return self.parse_function_call(&Token::Variable(name.clone())); }
                    return Ok(Expr::Number(std::f64::consts::E));
                }
                if name == "∞" || name == "inf" || name == "infinity" { return Ok(Expr::Infinity); }
                if name == "nan" { return Ok(Expr::NaN); }
                if name == "." {
                    if let Some(next_t) = self.peek() {
                        if let Some(val) = Self::get_number_value(next_t) {
                            let digits = self.get_literal_digits(next_t);
                            self.next();
                            return Ok(Expr::Number(val / (10.0f64.powi(digits as i32))));
                        }
                    }
                    return Ok(Expr::Variable(".".to_string()));
                }
                if let Ok(val) = name.parse::<f64>() { self.parse_number(val) }
                else if name.chars().next().map_or(false, |c| c.is_ascii_digit()) {
                    if let Some(pos) = name.find(|c: char| !c.is_ascii_digit() && c != '.') {
                        let (num_part, unit_part) = name.split_at(pos);
                        if let Ok(val) = num_part.parse::<f64>() {
                            let num_expr = Expr::Number(val);
                            let resolved_unit = match unit_part {
                                "π" | "pi" => Expr::Number(std::f64::consts::PI),
                                "e" => Expr::Number(std::f64::consts::E),
                                "%" => return Ok(Expr::UnaryOp { op: UnaryOp::Percent, expr: Box::new(num_expr) }),
                                "!" => return Ok(Expr::UnaryOp { op: UnaryOp::Fact, expr: Box::new(num_expr) }),
                                _ => Expr::Variable(unit_part.to_string()),
                            };
                            return Ok(Expr::BinOp { op: BinOp::Mul, lhs: Box::new(num_expr), rhs: Box::new(resolved_unit) });
                        }
                    }
                    Ok(Expr::Variable(name))
                } else if self.is_next_call_paren() || Self::is_function_name(&name) { self.parse_function_call(&Token::Variable(name)) }
                else if let Some(nt) = self.peek() {
                    if Self::is_expression_start(nt) && !(matches!(nt, Token::Plus | Token::Minus) || matches!(nt, Token::Variable(v) if normalize_str(v) == "+" || normalize_str(v) == "-")) { 
                        return self.parse_function_call(&Token::Variable(name)); 
                    }
                    Ok(Expr::Variable(name))
                }
                else { Ok(Expr::Variable(name)) }
            }
            _ => Err(format!("Unexpected token at position {}: {:?}", self.pos - 1, t)),
        };
        self.exit_depth();
        res
    }

    fn parse_number(&mut self, n: f64) -> Result<Expr, String> {
        let mut val = n;
        while let Some(nt) = self.peek() {
            let is_dot = match nt { Token::Dot => true, Token::Variable(s) if normalize_str(s) == "." => true, _ => false };
            if is_dot && val.fract() == 0.0 {
                if let Some(next_t) = self.peek_next() {
                    if let Some(f_val_num) = Self::get_number_value(next_t) {
                        let digits = self.get_literal_digits(next_t);
                        self.next(); self.next();
                        val = val + f_val_num / (10.0f64.powi(digits as i32));
                        continue;
                    }
                }
            }
            let is_e = match nt { Token::E => true, Token::Variable(s) => normalize_str(s) == "e", _ => false };
            if is_e {
                let current_pos = self.pos; self.next();
                let mut sign = 1.0;
                if let Some(st) = self.peek() {
                    match st {
                        Token::Plus => { self.next(); }
                        Token::Minus => { sign = -1.0; self.next(); }
                        Token::Variable(s) => {
                            let ns = normalize_str(s);
                            if ns == "+" { self.next(); } else if ns == "-" { sign = -1.0; self.next(); }
                        }
                        _ => {}
                    }
                }
                if let Some(et) = self.peek() {
                    if let Some(exp) = Self::get_number_value(et) {
                        self.next(); val = val * 10.0f64.powf(sign * exp); continue;
                    }
                }
                self.pos = current_pos;
            }
            break;
        }
        Ok(Expr::Number(val))
    }

    fn get_literal_digits(&self, t: &Token) -> usize {
        match t {
            Token::Number(n) => {
                let s = n.to_string();
                if let Some(pos) = s.find('.') { s.len() - pos - 1 } else { s.len() }
            }
            Token::Variable(s) => s.chars().filter(|c| c.is_ascii_digit()).count(),
            _ => 1,
        }
    }

    fn is_next_call_paren(&mut self) -> bool {
        self.peek().map_or(false, |pt| matches!(pt, Token::LParen) || matches!(pt, Token::Variable(v) if normalize_str(v) == "("))
    }

    fn parse_grouping(&mut self, expected_label: &str, expected_token: Token) -> Result<Expr, String> {
        let expr = self.parse_expression()?;
        let next_t = self.next();
        if !next_t.map_or(false, |nt| nt == &expected_token || matches!(nt, Token::Variable(v) if normalize_str(v) == expected_label)) {
            return Err(format!("Expected '{}'", expected_label));
        }
        Ok(expr)
    }

    fn get_number_value(t: &Token) -> Option<f64> {
        match t { Token::Number(n) => Some(*n), Token::Variable(s) => normalize_str(s).parse::<f64>().ok(), _ => None }
    }

    fn is_function(t: &Token) -> bool {
        match t {
            Token::Sin | Token::Cos | Token::Tan | Token::Asin | Token::Acos | Token::Atan
            | Token::Sinh | Token::Cosh | Token::Tanh | Token::Log | Token::Log10 | Token::Log2
            | Token::Ln | Token::Exp | Token::Abs | Token::Sqrt | Token::Cbrt | Token::Sum
            | Token::Integral | Token::Mod | Token::Pow | Token::Pi | Token::E | Token::Differential(_)
            | Token::Floor | Token::Ceil | Token::Round => true,
            Token::Function(s) | Token::Variable(s) => {
                let n = normalize_str(s);
                Self::is_function_name(&n)
            }
            _ => false,
        }
    }

    fn is_function_name(name: &str) -> bool {
        matches!(name, "sin" | "cos" | "tan" | "asin" | "acos" | "atan" | "sinh" | "cosh" | "tanh" | "log" | "log10" | "log2" | "ln" | "exp" | "abs" | "sqrt" | "cbrt" | "sum" | "σ" | "sigma" | "integral" | "mod" | "pow" | "pi" | "π" | "e" | "floor" | "ceil" | "round")
    }

    fn parse_function_call(&mut self, t: &Token) -> Result<Expr, String> {
        let name = match t {
            Token::Sin => "sin".to_string(), Token::Cos => "cos".to_string(), Token::Tan => "tan".to_string(),
            Token::Asin => "asin".to_string(), Token::Acos => "acos".to_string(), Token::Atan => "atan".to_string(),
            Token::Sinh => "sinh".to_string(), Token::Cosh => "cosh".to_string(), Token::Tanh => "tanh".to_string(),
            Token::Log => "log".to_string(), Token::Log10 => "log10".to_string(), Token::Log2 => "log2".to_string(),
            Token::Ln => "ln".to_string(), Token::Exp => "exp".to_string(), Token::Abs => "abs".to_string(),
            Token::Sqrt => "sqrt".to_string(), Token::Cbrt => "cbrt".to_string(), Token::Sum => "sum".to_string(),
            Token::Integral => "integral".to_string(), Token::Mod => "mod".to_string(), Token::Pow => "pow".to_string(),
            Token::Pi => "pi".to_string(), Token::E => "e".to_string(),
            Token::Floor => "floor".to_string(), Token::Ceil => "ceil".to_string(), Token::Round => "round".to_string(),
            Token::Differential(s) | Token::Function(s) | Token::Variable(s) => {
                let n = normalize_str(s);
                match n.as_str() {
                    "σ" | "sigma" | "sum" => "sum".to_string(),
                    _ => n,
                }
            },
            _ => return Err(format!("Unexpected function token: {:?}", t)),
        };
        if !self.is_next_call_paren() {
            if let Some(next_t) = self.peek() {
                if Self::is_expression_start(next_t) && !(matches!(next_t, Token::Plus | Token::Minus) || matches!(next_t, Token::Variable(v) if normalize_str(v) == "+" || normalize_str(v) == "-")) {
                    let arg = self.parse_unary()?; return self.finalize_function_call(name, vec![arg]);
                }
            }
            if name == "pi" || name == "π" { return Ok(Expr::Number(std::f64::consts::PI)); }
            if name == "e" { return Ok(Expr::Number(std::f64::consts::E)); }
            return Ok(Expr::Variable(name));
        }
        self.next();
        let mut args = Vec::new();
        if !self.peek().map_or(false, |pt| matches!(pt, Token::RParen) || matches!(pt, Token::Variable(v) if normalize_str(v) == ")")) {
            args.push(self.parse_assignment()?);
            while let Some(t) = self.peek() {
                if matches!(t, Token::Comma) || matches!(t, Token::Variable(s) if normalize_str(s) == ",") {
                    self.next(); args.push(self.parse_assignment()?);
                } else { break; }
            }
        }
        let next_t = self.next();
        if !next_t.map_or(false, |nt| nt == &Token::RParen || matches!(nt, Token::Variable(v) if normalize_str(v) == ")")) {
            return Err("Expected ')'".to_string());
        }
        self.finalize_function_call(name, args)
    }

    fn finalize_function_call(&self, name: String, mut args: Vec<Expr>) -> Result<Expr, String> {
        if args.len() == 2 && name == "pow" {
            return Ok(Expr::BinOp { op: BinOp::Pow, lhs: Box::new(args.remove(0)), rhs: Box::new(args.remove(0)) });
        }
        if args.len() == 1 {
            match name.as_str() {
                "sqrt" => return Ok(Expr::UnaryOp { op: UnaryOp::Sqrt, expr: Box::new(args.remove(0)) }),
                "cbrt" => return Ok(Expr::UnaryOp { op: UnaryOp::Cbrt, expr: Box::new(args.remove(0)) }),
                "log" => return Ok(Expr::UnaryOp { op: UnaryOp::Log, expr: Box::new(args.remove(0)) }),
                "log10" => return Ok(Expr::UnaryOp { op: UnaryOp::Log10, expr: Box::new(args.remove(0)) }),
                "log2" => return Ok(Expr::UnaryOp { op: UnaryOp::Log2, expr: Box::new(args.remove(0)) }),
                "ln" => return Ok(Expr::UnaryOp { op: UnaryOp::Ln, expr: Box::new(args.remove(0)) }),
                "exp" => return Ok(Expr::UnaryOp { op: UnaryOp::Exp, expr: Box::new(args.remove(0)) }),
                "abs" => return Ok(Expr::UnaryOp { op: UnaryOp::Abs, expr: Box::new(args.remove(0)) }),
                "floor" => return Ok(Expr::UnaryOp { op: UnaryOp::Floor, expr: Box::new(args.remove(0)) }),
                "ceil" => return Ok(Expr::UnaryOp { op: UnaryOp::Ceil, expr: Box::new(args.remove(0)) }),
                "round" => return Ok(Expr::UnaryOp { op: UnaryOp::Round, expr: Box::new(args.remove(0)) }),
                "sin" => return Ok(Expr::UnaryOp { op: UnaryOp::Sin, expr: Box::new(args.remove(0)) }),
                "cos" => return Ok(Expr::UnaryOp { op: UnaryOp::Cos, expr: Box::new(args.remove(0)) }),
                "tan" => return Ok(Expr::UnaryOp { op: UnaryOp::Tan, expr: Box::new(args.remove(0)) }),
                "asin" => return Ok(Expr::UnaryOp { op: UnaryOp::Asin, expr: Box::new(args.remove(0)) }),
                "acos" => return Ok(Expr::UnaryOp { op: UnaryOp::Acos, expr: Box::new(args.remove(0)) }),
                "atan" => return Ok(Expr::UnaryOp { op: UnaryOp::Atan, expr: Box::new(args.remove(0)) }),
                "sinh" => return Ok(Expr::UnaryOp { op: UnaryOp::Sinh, expr: Box::new(args.remove(0)) }),
                "cosh" => return Ok(Expr::UnaryOp { op: UnaryOp::Cosh, expr: Box::new(args.remove(0)) }),
                "tanh" => return Ok(Expr::UnaryOp { op: UnaryOp::Tanh, expr: Box::new(args.remove(0)) }),
                "pi" | "π" => return Ok(Expr::BinOp { op: BinOp::Mul, lhs: Box::new(Expr::Number(std::f64::consts::PI)), rhs: Box::new(args.remove(0)) }),
                "sum" => return Ok(args.remove(0)),
                _ => {}
            }
        }
        Ok(Expr::FunctionCall { name, args })
    }

    fn is_expression_start(t: &Token) -> bool {
        match t {
            Token::Number(_) | Token::NaN | Token::Infinity | Token::LParen | Token::LBracket | Token::LBrace
            | Token::Plus | Token::Minus | Token::PlusMinus | Token::Exclamation | Token::Variable(_)
            | Token::Function(_) | Token::Pi | Token::E | Token::I | Token::J | Token::Imaginary(_)
            | Token::String(_) | Token::Sqrt | Token::Cbrt | Token::Abs | Token::Log | Token::Log10
            | Token::Log2 | Token::Ln | Token::Exp | Token::Sin | Token::Cos | Token::Tan
            | Token::Asin | Token::Acos | Token::Atan | Token::Sinh | Token::Cosh | Token::Tanh
            | Token::Sum | Token::Integral | Token::Differential(_) | Token::At | Token::Dollar
            | Token::Dot | Token::Mod | Token::Pow | Token::Percent | Token::Floor | Token::Ceil | Token::Round => true,
            _ => false,
        }
    }

    fn is_implicit_mul_start(t: &Token) -> bool {
        match t {
            Token::Number(_) | Token::NaN | Token::Infinity | Token::LParen | Token::LBracket | Token::LBrace
            | Token::Pi | Token::E | Token::I | Token::J | Token::Imaginary(_) | Token::Sqrt | Token::Cbrt 
            | Token::Abs | Token::Log | Token::Log10 | Token::Log2 | Token::Ln | Token::Exp | Token::Sin
            | Token::Cos | Token::Tan | Token::Asin | Token::Acos | Token::Atan | Token::Sinh
            | Token::Cosh | Token::Tanh | Token::Sum | Token::Integral | Token::Differential(_) => true,
            Token::Function(_) => true,
            Token::Variable(s) => {
                let n = normalize_str(s);
                !matches!(n.as_str(), "+" | "-" | "±" | "*" | "×" | "/" | "÷" | "mod" | "^" | "**" | "==" | "!=" | "<>" | "<" | ">" | "<=" | ">=" | "&&" | "||" | "and" | "or" | "=" | "!" | "%" | "," | ";" | ")" | "]" | "}")
            }
            _ => false,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let socket_path = env::args().nth(1).unwrap_or_else(|| "/tmp/genesis-core/parser.sock".to_string());
    let _ = std::fs::remove_file(&socket_path);
    if let Some(parent) = std::path::Path::new(&socket_path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(&socket_path)?;
    tracing::info!("parser listening on {}", socket_path);
    loop {
        let (stream, _) = listener.accept().await?;
        tokio::spawn(async move {
            let (reader, mut writer) = tokio::io::split(stream);
            let mut reader = tokio::io::BufReader::new(reader);
            let mut line = String::new();
            while let Ok(n) = reader.read_line(&mut line).await {
                if n == 0 { break; }
                let start = std::time::Instant::now();
                let request: ModuleRequest = match serde_json::from_str(&line) {
                    Ok(req) => req,
                    Err(_) => { line.clear(); continue; }
                };
                let tokens: Vec<Token> = match serde_json::from_str(&request.input) {
                    Ok(t) => t,
                    Err(_) => { line.clear(); continue; }
                };
                let mut parser = Parser::new(&tokens);
                let (output, error) = match parser.parse() {
                    Ok(expr) => (Some(serde_json::to_string(&expr).unwrap()), None),
                    Err(e) => (None, Some(ModuleError { code: "UNKNOWN_PATTERN".to_string(), message: e, input_position: None })),
                };
                let response = ModuleResponse { request_id: request.request_id, output, error, processing_ms: start.elapsed().as_millis() as u64 };
                if let Ok(payload) = serde_json::to_vec(&response) {
                    let mut payload = payload; payload.push(b'\n');
                    let _ = writer.write_all(&payload).await;
                }
                line.clear();
            }
            Ok::<(), anyhow::Error>(())
        });
    }
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt().try_init();
}