// =============================================================================
// # CMP Module Charter
//
// What:
//   Receive an AST and return the calculation result (f64).
//
// Invariants:
//   - Division by zero must return an error (panics are prohibited).
//   - Overflow must return an error (silent ignores are prohibited).
//   - The calculation result must be returned with the same f64 precision as the input.
//
// Boundaries:
//   - Dependencies: parser
//   - Dependents: orchestrator (final output)
//
// Extensible:
//   - Evaluation of new node types (e.g., function calls, variable references).
//
// Why:
//   Isolate calculation logic so that changes to the parser do not propagate to the evaluator.
// =============================================================================

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::env;
use std::mem;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
#[cfg(unix)]
use tokio::net::UnixListener;

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
pub enum Expr {
    #[serde(alias = "NUMBER", alias = "NUM", alias = "FLOAT", alias = "LITERAL")]
    Number(f64),
    #[serde(alias = "INTEGER", alias = "INT")]
    Integer(f64),
    #[serde(alias = "BOOLEAN", alias = "BOOL")]
    Boolean(bool),
    #[serde(alias = "COMPLEX", alias = "IMAGINARY")]
    Complex {
        #[serde(alias = "RE", alias = "real")]
        re: f64,
        #[serde(alias = "IM", alias = "imag", alias = "imaginary")]
        im: f64,
    },
    #[serde(alias = "INFINITY", alias = "INF", alias = "POS_INFINITY")]
    Infinity,
    #[serde(alias = "NEG_INFINITY", alias = "NEG_INF")]
    NegInfinity,
    #[serde(alias = "NAN")]
    NaN,
    #[serde(alias = "VARIABLE", alias = "VAR", alias = "IDENT", alias = "ID")]
    Variable(String),
    #[serde(alias = "BIN_OP", alias = "BINARY", alias = "OP", alias = "BINARY_OP")]
    BinOp {
        #[serde(alias = "OP", alias = "operator")]
        op: BinOp,
        #[serde(alias = "LHS", alias = "left")]
        lhs: Box<Expr>,
        #[serde(alias = "RHS", alias = "right")]
        rhs: Box<Expr>,
    },
    #[serde(alias = "UNARY_OP", alias = "UNARY")]
    UnaryOp {
        #[serde(alias = "OP", alias = "operator")]
        op: UnaryOp,
        #[serde(alias = "EXPR", alias = "operand", alias = "expr")]
        expr: Box<Expr>,
    },
    #[serde(alias = "FUNCTION_CALL", alias = "CALL", alias = "FUNC", alias = "FUNCTION")]
    FunctionCall {
        #[serde(alias = "NAME", alias = "func")]
        name: String,
        #[serde(alias = "ARGS", alias = "arguments", alias = "params")]
        args: Vec<Expr>,
    },
    #[serde(alias = "SEQUENCE", alias = "BLOCK", alias = "LIST")]
    Sequence(Vec<Expr>),

    // Flattened variants for improved parser compatibility
    #[serde(alias = "ADD", alias = "PLUS", alias = "+")]
    Add {
        #[serde(alias = "left", alias = "lhs")]
        lhs: Box<Expr>,
        #[serde(alias = "right", alias = "rhs")]
        rhs: Box<Expr>
    },
    #[serde(alias = "SUB", alias = "MINUS", alias = "-")]
    Sub {
        #[serde(alias = "left", alias = "lhs")]
        lhs: Box<Expr>,
        #[serde(alias = "right", alias = "rhs")]
        rhs: Box<Expr>
    },
    #[serde(alias = "MUL", alias = "MULTIPLY", alias = "*")]
    Mul {
        #[serde(alias = "left", alias = "lhs")]
        lhs: Box<Expr>,
        #[serde(alias = "right", alias = "rhs")]
        rhs: Box<Expr>
    },
    #[serde(alias = "DIV", alias = "DIVIDE", alias = "/")]
    Div {
        #[serde(alias = "left", alias = "lhs")]
        lhs: Box<Expr>,
        #[serde(alias = "right", alias = "rhs")]
        rhs: Box<Expr>
    },
    #[serde(alias = "POW", alias = "POWER", alias = "^", alias = "**")]
    Pow {
        #[serde(alias = "left", alias = "lhs")]
        lhs: Box<Expr>,
        #[serde(alias = "right", alias = "rhs")]
        rhs: Box<Expr>
    },
    #[serde(alias = "MOD", alias = "MODULO", alias = "%")]
    Mod {
        #[serde(alias = "left", alias = "lhs")]
        lhs: Box<Expr>,
        #[serde(alias = "right", alias = "rhs")]
        rhs: Box<Expr>
    },
    #[serde(alias = "LOG", alias = "LN")]
    Log {
        #[serde(alias = "operand", alias = "expr")]
        expr: Box<Expr>,
        #[serde(default)]
        base: Option<Box<Expr>>
    },
    #[serde(alias = "SQRT", alias = "√")]
    Sqrt {
        #[serde(alias = "operand", alias = "expr")]
        expr: Box<Expr>
    },
    #[serde(alias = "NEG", alias = "NEGATIVE")]
    Neg {
        #[serde(alias = "operand", alias = "expr")]
        expr: Box<Expr>
    },
}

impl Drop for Expr {
    fn drop(&mut self) {
        let is_complex = |e: &Expr| {
            matches!(
                e,
                Expr::BinOp { .. }
                    | Expr::UnaryOp { .. }
                    | Expr::FunctionCall { .. }
                    | Expr::Sequence(_)
                    | Expr::Add { .. }
                    | Expr::Sub { .. }
                    | Expr::Mul { .. }
                    | Expr::Div { .. }
                    | Expr::Pow { .. }
                    | Expr::Mod { .. }
                    | Expr::Log { .. }
                    | Expr::Sqrt { .. }
                    | Expr::Neg { .. }
            )
        };

        match self {
            Expr::BinOp { lhs, rhs, .. } if is_complex(lhs) || is_complex(rhs) => {}
            Expr::UnaryOp { expr, .. } if is_complex(expr) => {}
            Expr::FunctionCall { args, .. } if args.iter().any(is_complex) => {}
            Expr::Sequence(exprs) if exprs.iter().any(is_complex) => {}
            Expr::Add { lhs, rhs } | Expr::Sub { lhs, rhs } | Expr::Mul { lhs, rhs } | Expr::Div { lhs, rhs } | Expr::Pow { lhs, rhs } | Expr::Mod { lhs, rhs }
                if is_complex(lhs) || is_complex(rhs) => {}
            Expr::Log { expr: child, .. } | Expr::Sqrt { expr: child } | Expr::Neg { expr: child } if is_complex(child) => {}
            _ => return,
        }

        let mut stack = Vec::new();
        stack.push(mem::replace(self, Expr::Number(0.0)));

        while let Some(mut expr) = stack.pop() {
            match &mut expr {
                Expr::BinOp { lhs, rhs, .. } | Expr::Add { lhs, rhs } | Expr::Sub { lhs, rhs } | Expr::Mul { lhs, rhs } | Expr::Div { lhs, rhs } | Expr::Pow { lhs, rhs } | Expr::Mod { lhs, rhs } => {
                    if is_complex(lhs) { stack.push(*mem::replace(lhs, Box::new(Expr::Number(0.0)))); }
                    if is_complex(rhs) { stack.push(*mem::replace(rhs, Box::new(Expr::Number(0.0)))); }
                }
                Expr::UnaryOp { expr: child, .. } | Expr::Log { expr: child, .. } | Expr::Sqrt { expr: child } | Expr::Neg { expr: child } => {
                    if is_complex(child) { stack.push(*mem::replace(child, Box::new(Expr::Number(0.0)))); }
                }
                Expr::FunctionCall { args, .. } => {
                    for arg in args.iter_mut() {
                        if is_complex(arg) { stack.push(mem::replace(arg, Expr::Number(0.0))); }
                    }
                }
                Expr::Sequence(exprs) => {
                    for e in exprs.iter_mut() {
                        if is_complex(e) { stack.push(mem::replace(e, Expr::Number(0.0))); }
                    }
                }
                _ => {}
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BinOp {
    #[serde(alias = "ADD", alias = "PLUS", alias = "+")]
    Add,
    #[serde(alias = "SUB", alias = "MINUS", alias = "-")]
    Sub,
    #[serde(alias = "MUL", alias = "MULTIPLY", alias = "*")]
    Mul,
    #[serde(alias = "DIV", alias = "DIVIDE", alias = "/")]
    Div,
    #[serde(alias = "FLOOR_DIV", alias = "INT_DIV", alias = "//")]
    FloorDiv,
    #[serde(alias = "POW", alias = "POWER", alias = "^", alias = "**")]
    Pow,
    #[serde(alias = "MOD", alias = "MODULO", alias = "REM", alias = "%")]
    Mod,
    #[serde(alias = "EQ", alias = "EQUAL", alias = "==")]
    Eq,
    #[serde(alias = "NE", alias = "NOT_EQUAL", alias = "!=")]
    Ne,
    #[serde(alias = "LT", alias = "LESS_THAN", alias = "<")]
    Lt,
    #[serde(alias = "GT", alias = "GREATER_THAN", alias = ">")]
    Gt,
    #[serde(alias = "LE", alias = "LESS_EQUAL", alias = "<=")]
    Le,
    #[serde(alias = "GE", alias = "GREATER_EQUAL", alias = ">=")]
    Ge,
    #[serde(alias = "AND", alias = "LOGICAL_AND", alias = "&&")]
    And,
    #[serde(alias = "OR", alias = "LOGICAL_OR", alias = "||")]
    Or,
    #[serde(alias = "ASSIGN", alias = "SET", alias = "=")]
    Assign,
    #[serde(alias = "BIT_AND", alias = "&")]
    BitAnd,
    #[serde(alias = "BIT_OR", alias = "|")]
    BitOr,
    #[serde(alias = "BIT_XOR", alias = "XOR", alias = "BXOR")]
    BitXor,
    #[serde(alias = "SHL", alias = "LSHIFT", alias = "<<")]
    Shl,
    #[serde(alias = "SHR", alias = "RSHIFT", alias = ">>")]
    Shr,
    Range,
    At,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UnaryOp {
    #[serde(alias = "NEG", alias = "NEGATIVE", alias = "UNARY_MINUS", alias = "-")]
    Neg,
    #[serde(alias = "POS", alias = "POSITIVE", alias = "UNARY_PLUS", alias = "+")]
    Pos,
    #[serde(alias = "FACT", alias = "FACTORIAL", alias = "!")]
    Fact,
    #[serde(alias = "PERCENT", alias = "%")]
    Percent,
    #[serde(alias = "NOT", alias = "LOGICAL_NOT")]
    Not,
    #[serde(alias = "SQRT", alias = "SQUARE_ROOT", alias = "√")]
    Sqrt,
    #[serde(alias = "LOG", alias = "LN", alias = "NATURAL_LOG")]
    Log,
}

#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    #[error("division by zero")]
    DivisionByZero,
    #[error("overflow: {0}")]
    Overflow(String),
    #[error("unknown function: {0}")]
    UnknownFunction(String),
    #[error("unknown variable: {0}")]
    UnknownVariable(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("unknown pattern: {0}")]
    UnknownPattern(String),
    #[error("stack error: {0}")]
    StackError(String),
}

#[derive(Debug, Clone, Copy)]
struct Complex {
    re: f64,
    im: f64,
}

impl Complex {
    fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    fn from_real(re: f64) -> Self {
        Self::new(re, 0.0)
    }

    fn add(self, other: Self) -> Self {
        Self::new(self.re + other.re, self.im + other.im)
    }

    fn sub(self, other: Self) -> Self {
        Self::new(self.re - other.re, self.im - other.im)
    }

    fn mul(self, other: Self) -> Self {
        Self::new(
            self.re * other.re - self.im * other.im,
            self.re * other.im + self.im * other.re,
        )
    }

    fn div(self, other: Self) -> Result<Self, EvalError> {
        let d = other.re * other.re + other.im * other.im;
        if d == 0.0 {
            return Err(EvalError::DivisionByZero);
        }
        Ok(Self::new(
            (self.re * other.re + self.im * other.im) / d,
            (self.im * other.re - self.re * other.im) / d,
        ))
    }

    fn pow(self, other: Self) -> Self {
        let r = (self.re * self.re + self.im * self.im).sqrt();
        if r == 0.0 {
            return if other.re > 0.0 {
                Self::new(0.0, 0.0)
            } else if other.re < 0.0 {
                Self::new(f64::INFINITY, 0.0)
            } else {
                Self::new(1.0, 0.0)
            };
        }
        let theta = self.im.atan2(self.re);
        let ln_r = r.ln();
        let res_re = other.re * ln_r - other.im * theta;
        let res_im = other.re * theta + other.im * ln_r;
        let exp_r = res_re.exp();
        Self::new(exp_r * res_im.cos(), exp_r * res_im.sin())
    }

    fn sqrt(self) -> Self {
        let r = (self.re * self.re + self.im * self.im).sqrt();
        let re = ((r + self.re) / 2.0).sqrt();
        let im = self.im.signum() * ((r - self.re) / 2.0).sqrt();
        if self.im == 0.0 && self.re < 0.0 {
            Self::new(0.0, r.sqrt())
        } else {
            Self::new(re, im)
        }
    }

    fn ln(self) -> Self {
        let r = (self.re * self.re + self.im * self.im).sqrt();
        let theta = self.im.atan2(self.re);
        Self::new(r.ln(), theta)
    }

    fn abs(self) -> f64 {
        (self.re * self.re + self.im * self.im).sqrt()
    }

    fn to_f64_best_effort(self) -> f64 {
        if self.im.abs() < 1e-15 {
            self.re
        } else {
            (self.re * self.re + self.im * self.im).sqrt()
        }
    }
}

fn check_result(v: f64) -> Result<f64, EvalError> {
    if v.is_infinite() {
        if v.is_sign_positive() {
            Err(EvalError::Overflow("result is positive infinity".to_string()))
        } else {
            Err(EvalError::Overflow("result is negative infinity".to_string()))
        }
    } else if v.is_nan() {
        Ok(v)
    } else {
        Ok(v)
    }
}

pub fn evaluate(root: &Expr) -> Result<f64, EvalError> {
    enum Task<'a> {
        Eval(&'a Expr),
        ComputeBinOp(BinOp),
        ComputeUnaryOp(UnaryOp),
        ComputeFunction(String, usize),
        HandleSequence(usize),
        ComputeLog(bool),
    }

    const MAX_TASKS: usize = 100000;
    let mut tasks = vec![Task::Eval(root)];
    let mut values: Vec<Complex> = vec![];

    while let Some(task) = tasks.pop() {
        if tasks.len() > MAX_TASKS {
            return Err(EvalError::StackError("expression complexity limit exceeded".to_string()));
        }

        match task {
            Task::Eval(expr) => match expr {
                Expr::Number(n) | Expr::Integer(n) => values.push(Complex::from_real(check_result(*n)?)),
                Expr::Boolean(b) => values.push(Complex::from_real(if *b { 1.0 } else { 0.0 })),
                Expr::Complex { re, im } => values.push(Complex::new(*re, *im)),
                Expr::Infinity => values.push(Complex::from_real(f64::INFINITY)),
                Expr::NegInfinity => values.push(Complex::from_real(f64::NEG_INFINITY)),
                Expr::NaN => values.push(Complex::from_real(f64::NAN)),
                Expr::Variable(name) => {
                    let name_lower = name.to_lowercase();
                    let val = match name_lower.as_str() {
                        "e" => Complex::from_real(std::f64::consts::E),
                        "pi" | "π" => Complex::from_real(std::f64::consts::PI),
                        "tau" | "τ" => Complex::from_real(std::f64::consts::TAU),
                        "phi" | "φ" => Complex::from_real(1.618033988749895),
                        "i" | "j" => Complex::new(0.0, 1.0),
                        _ => {
                            if name_lower.ends_with('i') || name_lower.ends_with('j') {
                                let prefix = &name_lower[..name_lower.len() - 1];
                                if let Ok(n) = prefix.parse::<f64>() {
                                    Complex::new(0.0, n)
                                } else if prefix.is_empty() {
                                    Complex::new(0.0, 1.0)
                                } else {
                                    return Err(EvalError::UnknownVariable(name.clone()));
                                }
                            } else {
                                return Err(EvalError::UnknownVariable(name.clone()));
                            }
                        }
                    };
                    values.push(val);
                }
                Expr::BinOp { op, lhs, rhs } => {
                    tasks.push(Task::ComputeBinOp(*op));
                    tasks.push(Task::Eval(rhs));
                    tasks.push(Task::Eval(lhs));
                }
                Expr::UnaryOp { op, expr } => {
                    tasks.push(Task::ComputeUnaryOp(*op));
                    tasks.push(Task::Eval(expr));
                }
                Expr::FunctionCall { name, args } => {
                    tasks.push(Task::ComputeFunction(name.clone(), args.len()));
                    for arg in args.iter().rev() { tasks.push(Task::Eval(arg)); }
                }
                Expr::Sequence(exprs) => {
                    if exprs.is_empty() { values.push(Complex::from_real(0.0)); }
                    else {
                        tasks.push(Task::HandleSequence(exprs.len()));
                        for expr in exprs.iter().rev() { tasks.push(Task::Eval(expr)); }
                    }
                }
                Expr::Add { lhs, rhs } => { tasks.push(Task::ComputeBinOp(BinOp::Add)); tasks.push(Task::Eval(rhs)); tasks.push(Task::Eval(lhs)); }
                Expr::Sub { lhs, rhs } => { tasks.push(Task::ComputeBinOp(BinOp::Sub)); tasks.push(Task::Eval(rhs)); tasks.push(Task::Eval(lhs)); }
                Expr::Mul { lhs, rhs } => { tasks.push(Task::ComputeBinOp(BinOp::Mul)); tasks.push(Task::Eval(rhs)); tasks.push(Task::Eval(lhs)); }
                Expr::Div { lhs, rhs } => { tasks.push(Task::ComputeBinOp(BinOp::Div)); tasks.push(Task::Eval(rhs)); tasks.push(Task::Eval(lhs)); }
                Expr::Pow { lhs, rhs } => { tasks.push(Task::ComputeBinOp(BinOp::Pow)); tasks.push(Task::Eval(rhs)); tasks.push(Task::Eval(lhs)); }
                Expr::Mod { lhs, rhs } => { tasks.push(Task::ComputeBinOp(BinOp::Mod)); tasks.push(Task::Eval(rhs)); tasks.push(Task::Eval(lhs)); }
                Expr::Log { expr, base } => {
                    tasks.push(Task::ComputeLog(base.is_some()));
                    if let Some(b) = base { tasks.push(Task::Eval(b)); }
                    tasks.push(Task::Eval(expr));
                }
                Expr::Sqrt { expr } => { tasks.push(Task::ComputeUnaryOp(UnaryOp::Sqrt)); tasks.push(Task::Eval(expr)); }
                Expr::Neg { expr } => { tasks.push(Task::ComputeUnaryOp(UnaryOp::Neg)); tasks.push(Task::Eval(expr)); }
            },
            Task::ComputeBinOp(op) => {
                let rhs = values.pop().ok_or_else(|| EvalError::StackError("missing rhs".to_string()))?;
                let lhs = values.pop().ok_or_else(|| EvalError::StackError("missing lhs".to_string()))?;
                let res = match op {
                    BinOp::Add => lhs.add(rhs),
                    BinOp::Sub => lhs.sub(rhs),
                    BinOp::Mul => lhs.mul(rhs),
                    BinOp::Div | BinOp::FloorDiv => {
                        let res = lhs.div(rhs)?;
                        if op == BinOp::FloorDiv { Complex::from_real(res.re.floor()) } else { res }
                    }
                    BinOp::Pow => lhs.pow(rhs),
                    BinOp::Mod => {
                        if rhs.re == 0.0 && rhs.im == 0.0 { return Err(EvalError::DivisionByZero); }
                        Complex::from_real(lhs.re % rhs.re)
                    }
                    BinOp::Eq => Complex::from_real(if (lhs.re - rhs.re).abs() < f64::EPSILON && (lhs.im - rhs.im).abs() < f64::EPSILON { 1.0 } else { 0.0 }),
                    _ => Complex::from_real(0.0),
                };
                check_result(res.re)?;
                values.push(res);
            }
            Task::ComputeUnaryOp(op) => {
                let val = values.pop().ok_or_else(|| EvalError::StackError("missing operand".to_string()))?;
                let res = match op {
                    UnaryOp::Neg => Complex::new(-val.re, -val.im),
                    UnaryOp::Pos => val,
                    UnaryOp::Fact => {
                        if val.re < 0.0 || val.re.fract() != 0.0 || val.im != 0.0 {
                            return Err(EvalError::InvalidArgument("factorial requires non-negative integer".to_string()));
                        }
                        let mut r = 1.0;
                        for i in 1..=(val.re as u64) { r *= i as f64; if r.is_infinite() { break; } }
                        Complex::from_real(r)
                    }
                    UnaryOp::Percent => Complex::new(val.re / 100.0, val.im / 100.0),
                    UnaryOp::Sqrt => val.sqrt(),
                    UnaryOp::Log => val.ln(),
                    _ => val,
                };
                check_result(res.re)?;
                values.push(res);
            }
            Task::ComputeLog(has_base) => {
                let val = values.pop().ok_or_else(|| EvalError::StackError("missing log operand".to_string()))?;
                let res = if has_base {
                    let base = values.pop().ok_or_else(|| EvalError::StackError("missing log base".to_string()))?;
                    val.ln().div(base.ln())?
                } else {
                    val.ln()
                };
                values.push(res);
            }
            Task::ComputeFunction(name, arg_count) => {
                let mut args = Vec::with_capacity(arg_count);
                for _ in 0..arg_count { args.push(values.pop().ok_or_else(|| EvalError::StackError("missing function argument".to_string()))?); }
                args.reverse();
                let res = match name.to_lowercase().as_str() {
                    "sin" => { let v = args.get(0).copied().unwrap_or(Complex::from_real(0.0)); Complex::new(v.re.sin() * v.im.cosh(), v.re.cos() * v.im.sinh()) },
                    "cos" => { let v = args.get(0).copied().unwrap_or(Complex::from_real(0.0)); Complex::new(v.re.cos() * v.im.cosh(), -v.re.sin() * v.im.sinh()) },
                    "tan" => {
                        let v = args.get(0).copied().unwrap_or(Complex::from_real(0.0));
                        let sin = Complex::new(v.re.sin() * v.im.cosh(), v.re.cos() * v.im.sinh());
                        let cos = Complex::new(v.re.cos() * v.im.cosh(), -v.re.sin() * v.im.sinh());
                        sin.div(cos)?
                    },
                    "log" | "ln" => {
                        let v = *args.get(0).ok_or_else(|| EvalError::InvalidArgument("log requires 1 or 2 args".to_string()))?;
                        if args.len() == 1 { v.ln() } else { v.ln().div(args[1].ln())? }
                    }
                    "sqrt" => args.get(0).ok_or_else(|| EvalError::InvalidArgument("sqrt requires 1 arg".to_string()))?.sqrt(),
                    "abs" => Complex::from_real(args.get(0).copied().unwrap_or(Complex::from_real(0.0)).abs()),
                    "exp" => {
                        let v = args.get(0).copied().unwrap_or(Complex::from_real(0.0));
                        let r = v.re.exp();
                        Complex::new(r * v.im.cos(), r * v.im.sin())
                    },
                    _ => return Err(EvalError::UnknownFunction(name)),
                };
                values.push(res);
            }
            Task::HandleSequence(count) => {
                let last = values.pop().ok_or_else(|| EvalError::StackError("empty sequence".to_string()))?;
                for _ in 1..count { values.pop(); }
                values.push(last);
            }
        }
    }
    let final_res = values.pop().ok_or_else(|| EvalError::StackError("empty evaluation stack".to_string()))?;
    let val = final_res.to_f64_best_effort();
    if val.is_nan() {
        return Err(EvalError::InvalidArgument("result is NaN".to_string()));
    }
    Ok(val)
}

async fn send_response<W>(writer: &mut W, request_id: String, output: Option<String>, error: Option<ModuleError>, processing_ms: u64)
where W: tokio::io::AsyncWrite + Unpin {
    let response = ModuleResponse { request_id, output, error, processing_ms };
    if let Ok(payload) = serde_json::to_vec(&response) {
        let mut payload = payload;
        payload.push(b'\n');
        let _ = writer.write_all(&payload).await;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let addr_or_path = env::args().nth(1).unwrap_or_else(|| "/tmp/genesis-core/evaluator.sock".to_string());
    if addr_or_path.starts_with("tcp://") {
        let addr = addr_or_path.strip_prefix("tcp://").unwrap();
        let listener = TcpListener::bind(addr).await?;
        loop {
            let (stream, _) = listener.accept().await?;
            tokio::spawn(async move { let _ = handle_client(stream).await; });
        }
    } else {
        #[cfg(unix)]
        {
            let uds_path = addr_or_path.strip_prefix("uds://").unwrap_or(&addr_or_path);
            let _ = std::fs::remove_file(uds_path);
            if let Some(parent) = std::path::Path::new(uds_path).parent() { let _ = std::fs::create_dir_all(parent); }
            let listener = UnixListener::bind(uds_path)?;
            loop {
                let (stream, _) = listener.accept().await?;
                tokio::spawn(async move { let _ = handle_client(stream).await; });
            }
        }
        #[cfg(not(unix))]
        {
            fn path_to_port(path: impl AsRef<std::path::Path>) -> u16 {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                let file_name = path
                    .as_ref()
                    .file_name()
                    .map(|f| f.to_string_lossy())
                    .unwrap_or_else(|| path.as_ref().to_string_lossy());
                file_name.hash(&mut hasher);
                let hash = hasher.finish();
                (10000 + (hash % 35000)) as u16
            }

            let port = path_to_port(&addr_or_path);
            let listener = TcpListener::bind(format!("127.0.0.1:{}", port)).await?;
            loop {
                let (stream, _) = listener.accept().await?;
                tokio::spawn(async move { let _ = handle_client(stream).await; });
            }
        }
    }
}

async fn handle_client<S>(stream: S) -> Result<()>
where S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static {
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) => break,
            Ok(_) => {
                let start = std::time::Instant::now();
                let request: ModuleRequest = match serde_json::from_str(&line) {
                    Ok(req) => req,
                    Err(_) => {
                        send_response(&mut writer, "unknown".to_string(), None, Some(ModuleError { code: "INVALID_REQUEST".to_string(), message: "Failed to parse request".to_string(), input_position: None }), 0).await;
                        continue;
                    }
                };
                let expr_result: Result<Expr, _> = serde_json::from_str(&request.input);
                match expr_result {
                    Ok(expr) => match evaluate(&expr) {
                        Ok(val) => send_response(&mut writer, request.request_id, Some(val.to_string()), None, start.elapsed().as_millis() as u64).await,
                        Err(e) => {
                            let code = match e {
                                EvalError::DivisionByZero => "DIVISION_BY_ZERO",
                                EvalError::Overflow(_) => "OVERFLOW",
                                _ => "INVALID_ARGUMENT",
                            };
                            send_response(&mut writer, request.request_id, None, Some(ModuleError { code: code.to_string(), message: e.to_string(), input_position: None }), start.elapsed().as_millis() as u64).await;
                        }
                    },
                    Err(e) => send_response(&mut writer, request.request_id, None, Some(ModuleError { code: "UNKNOWN_PATTERN".to_string(), message: format!("AST Parse Error: {}", e), input_position: None }), start.elapsed().as_millis() as u64).await,
                }
            }
            Err(_) => break,
        }
    }
    Ok(())
}

fn init_tracing() { let _ = tracing_subscriber::fmt().try_init(); }