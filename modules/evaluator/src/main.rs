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
use std::panic::{catch_unwind, AssertUnwindSafe};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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

fn deserialize_f64_flexible<'de, D>(deserializer: D) -> std::result::Result<f64, D::Error>
where D: serde::Deserializer<'de> {
    use serde::Deserialize;
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum FlexF64 { Num(f64), Str(String) }
    match FlexF64::deserialize(deserializer)? {
        FlexF64::Num(n) => Ok(n),
        FlexF64::Str(s) => {
            let s_lower = s.to_lowercase().trim().to_string();
            match s_lower.as_str() {
                "nan" => return Ok(f64::NAN),
                "inf" | "infinity" | "pos_infinity" | "+inf" => return Ok(f64::INFINITY),
                "-inf" | "-infinity" | "neg_infinity" => return Ok(f64::NEG_INFINITY),
                "pi" | "π" => return Ok(std::f64::consts::PI),
                "e" => return Ok(std::f64::consts::E),
                _ => {}
            }
            let cleaned: String = s_lower.chars()
                .take_while(|c| c.is_digit(10) || *c == '.' || *c == '-' || *c == '+' || *c == 'e' || *c == 'E')
                .collect();
            if !cleaned.is_empty() {
                if let Ok(n) = cleaned.parse::<f64>() {
                    return Ok(n);
                }
            }
            s.parse().map_err(serde::de::Error::custom)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Expr {
    #[serde(alias = "NUMBER", alias = "NUM", alias = "FLOAT", alias = "LITERAL", alias = "CONSTANT", alias = "NUMERIC_LITERAL", alias = "VAL")]
    Number(#[serde(deserialize_with = "deserialize_f64_flexible")] f64),
    #[serde(alias = "INTEGER", alias = "INT")]
    Integer(#[serde(deserialize_with = "deserialize_f64_flexible")] f64),
    #[serde(alias = "BOOLEAN", alias = "BOOL")]
    Boolean(bool),
    #[serde(alias = "COMPLEX", alias = "IMAGINARY")]
    Complex {
        #[serde(alias = "RE", alias = "real")] re: f64,
        #[serde(alias = "IM", alias = "imag", alias = "imaginary")] im: f64,
    },
    #[serde(alias = "INFINITY", alias = "INF", alias = "POS_INFINITY")]
    Infinity,
    #[serde(alias = "NEG_INFINITY", alias = "NEG_INF")]
    NegInfinity,
    #[serde(alias = "NAN")]
    NaN,
    #[serde(alias = "VARIABLE", alias = "VAR", alias = "IDENT", alias = "ID", alias = "NAME")]
    Variable(String),
    #[serde(alias = "BIN_OP", alias = "BINARY", alias = "OP", alias = "BINARY_OP", alias = "BINARY_EXPRESSION", alias = "BINOP")]
    BinOp {
        #[serde(alias = "OP", alias = "operator")] op: BinOp,
        #[serde(alias = "LHS", alias = "left", alias = "left_hand_side")] lhs: Box<Expr>,
        #[serde(alias = "RHS", alias = "right", alias = "right_hand_side")] rhs: Box<Expr>,
    },
    #[serde(alias = "UNARY_OP", alias = "UNARY", alias = "UNARY_EXPRESSION", alias = "UNOP")]
    UnaryOp {
        #[serde(alias = "OP", alias = "operator")] op: UnaryOp,
        #[serde(alias = "EXPR", alias = "operand", alias = "expr")] expr: Box<Expr>,
    },
    #[serde(alias = "FUNCTION_CALL", alias = "CALL", alias = "FUNC", alias = "FUNCTION", alias = "CALL_EXPRESSION")]
    FunctionCall {
        #[serde(alias = "NAME", alias = "func", alias = "id")] name: String,
        #[serde(alias = "ARGS", alias = "arguments", alias = "params")] args: Vec<Expr>,
    },
    #[serde(alias = "SEQUENCE", alias = "BLOCK", alias = "LIST", alias = "EXPRESSION", alias = "ARRAY")]
    Sequence(Vec<Expr>),

    #[serde(alias = "ADD", alias = "PLUS", alias = "ADDITION", alias = "+", alias = "SUM")]
    Add { #[serde(alias = "left", alias = "lhs")] lhs: Box<Expr>, #[serde(alias = "right", alias = "rhs")] rhs: Box<Expr> },
    #[serde(alias = "SUB", alias = "MINUS", alias = "SUBTRACTION", alias = "-", alias = "DIFFERENCE")]
    Sub { #[serde(alias = "left", alias = "lhs")] lhs: Box<Expr>, #[serde(alias = "right", alias = "rhs")] rhs: Box<Expr> },
    #[serde(alias = "MUL", alias = "MULTIPLY", alias = "MULTIPLICATION", alias = "*", alias = "PRODUCT")]
    Mul { #[serde(alias = "left", alias = "lhs")] lhs: Box<Expr>, #[serde(alias = "right", alias = "rhs")] rhs: Box<Expr> },
    #[serde(alias = "DIV", alias = "DIVIDE", alias = "DIVISION", alias = "/", alias = "QUOTIENT")]
    Div { #[serde(alias = "left", alias = "lhs")] lhs: Box<Expr>, #[serde(alias = "right", alias = "rhs")] rhs: Box<Expr> },
    #[serde(alias = "POW", alias = "POWER", alias = "^", alias = "**")]
    Pow { #[serde(alias = "left", alias = "lhs")] lhs: Box<Expr>, #[serde(alias = "right", alias = "rhs")] rhs: Box<Expr> },
    #[serde(alias = "MOD", alias = "MODULO", alias = "%", alias = "REM", alias = "REMAINDER")]
    Mod { #[serde(alias = "left", alias = "lhs")] lhs: Box<Expr>, #[serde(alias = "right", alias = "rhs")] rhs: Box<Expr> },
    #[serde(alias = "LOG", alias = "LN")]
    Log { #[serde(alias = "expr", alias = "value")] expr: Box<Expr>, #[serde(default)] base: Option<Box<Expr>> },
    #[serde(alias = "SQRT", alias = "√")]
    Sqrt { #[serde(alias = "expr", alias = "value")] expr: Box<Expr> },
    #[serde(alias = "NEG", alias = "NEGATIVE")]
    Neg { #[serde(alias = "expr", alias = "value")] expr: Box<Expr> },
    #[serde(alias = "PAREN", alias = "GROUP", alias = "BRACKET")]
    Paren(Box<Expr>),
}

impl Drop for Expr {
    fn drop(&mut self) {
        match self {
            Expr::Number(_) | Expr::Integer(_) | Expr::Boolean(_) | Expr::Infinity | Expr::NegInfinity | Expr::NaN | Expr::Variable(_) | Expr::Complex { .. } => return,
            _ => {}
        }
        let mut stack = Vec::new();
        let mut current = mem::replace(self, Expr::Number(0.0));
        loop {
            match &mut current {
                Expr::BinOp { lhs, rhs, .. } | Expr::Add { lhs, rhs } | Expr::Sub { lhs, rhs } | Expr::Mul { lhs, rhs } | Expr::Div { lhs, rhs } | Expr::Pow { lhs, rhs } | Expr::Mod { lhs, rhs } => {
                    stack.push(*mem::replace(lhs, Box::new(Expr::Number(0.0))));
                    stack.push(*mem::replace(rhs, Box::new(Expr::Number(0.0))));
                }
                Expr::UnaryOp { expr, .. } | Expr::Sqrt { expr } | Expr::Neg { expr } | Expr::Paren(expr) => {
                    stack.push(*mem::replace(expr, Box::new(Expr::Number(0.0))));
                }
                Expr::Log { expr, base } => {
                    stack.push(*mem::replace(expr, Box::new(Expr::Number(0.0))));
                    if let Some(b) = base { stack.push(*mem::replace(b, Box::new(Expr::Number(0.0)))); }
                }
                Expr::FunctionCall { args, .. } => { stack.extend(args.drain(..)); }
                Expr::Sequence(exprs) => { stack.extend(exprs.drain(..)); }
                _ => {}
            }
            if let Some(next) = stack.pop() { current = next; } else { break; }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BinOp {
    #[serde(alias = "+", alias = "PLUS", alias = "ADD")] Add, 
    #[serde(alias = "-", alias = "MINUS", alias = "SUB")] Sub, 
    #[serde(alias = "*", alias = "MUL", alias = "MULTIPLY", alias = "TIMES")] Mul, 
    #[serde(alias = "/", alias = "DIV", alias = "DIVIDE")] Div, 
    #[serde(alias = "//", alias = "FLOOR_DIV")] FloorDiv, 
    #[serde(alias = "**", alias = "^", alias = "POW", alias = "POWER")] Pow, 
    #[serde(alias = "%", alias = "MOD", alias = "MODULO", alias = "REM", alias = "REMAINDER")] Mod, 
    Eq, Ne, Lt, Gt, Le, Ge, And, Or, Assign, 
    #[serde(alias = "&")] BitAnd, #[serde(alias = "|")] BitOr, #[serde(alias = "^^")] BitXor, 
    #[serde(alias = "<<")] Shl, #[serde(alias = ">>")] Shr, Range, At,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UnaryOp { 
    #[serde(alias = "-", alias = "NEG")] Neg, #[serde(alias = "+", alias = "POS")] Pos, 
    #[serde(alias = "!", alias = "FACT")] Fact, #[serde(alias = "%", alias = "PERCENT")] Percent, 
    Not, #[serde(alias = "SQRT")] Sqrt, #[serde(alias = "LOG", alias = "LN")] Log 
}

#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    #[error("division by zero")] DivisionByZero,
    #[error("overflow: {0}")] Overflow(String),
    #[error("unknown function: {0}")] UnknownFunction(String),
    #[error("unknown variable: {0}")] UnknownVariable(String),
    #[error("invalid argument: {0}")] InvalidArgument(String),
    #[error("stack error: {0}")] StackError(String),
    #[error("evaluation panicked")] Panic,
}

#[derive(Debug, Clone, Copy)]
struct ComplexVal { re: f64, im: f64 }

impl ComplexVal {
    fn new(re: f64, im: f64) -> Self { Self { re, im } }
    fn from_real(re: f64) -> Self { Self::new(re, 0.0) }
    fn add(self, other: Self) -> Self { Self::new(self.re + other.re, self.im + other.im) }
    fn sub(self, other: Self) -> Self { Self::new(self.re - other.re, self.im - other.im) }
    fn mul(self, other: Self) -> Self { Self::new(self.re * other.re - self.im * other.im, self.re * other.im + self.im * other.re) }
    fn div(self, other: Self) -> std::result::Result<Self, EvalError> {
        let mag_sq = other.re * other.re + other.im * other.im;
        if mag_sq == 0.0 || mag_sq.is_nan() { return Err(EvalError::DivisionByZero); }
        if other.re.abs() >= other.im.abs() {
            let r = other.im / other.re;
            let den = other.re + r * other.im;
            if den == 0.0 { return Err(EvalError::DivisionByZero); }
            Ok(Self::new((self.re + self.im * r) / den, (self.im - self.re * r) / den))
        } else {
            let r = other.re / other.im;
            let den = other.im + r * other.re;
            if den == 0.0 { return Err(EvalError::DivisionByZero); }
            Ok(Self::new((self.re * r + self.im) / den, (self.im * r - self.re) / den))
        }
    }
    fn pow(self, other: Self) -> std::result::Result<Self, EvalError> {
        let r = self.re.hypot(self.im);
        if r == 0.0 {
            if other.re > 0.0 { return Ok(Self::new(0.0, 0.0)); }
            else if other.re < 0.0 || other.im != 0.0 { return Err(EvalError::DivisionByZero); }
            else { return Ok(Self::new(1.0, 0.0)); }
        }
        let theta = self.im.atan2(self.re);
        let ln_r = r.ln();
        let res_re = other.re * ln_r - other.im * theta;
        let res_im = other.re * theta + other.im * ln_r;
        let exp_r = res_re.exp();
        Ok(Self::new(exp_r * res_im.cos(), exp_r * res_im.sin()))
    }
    fn sqrt(self) -> Self {
        let r = self.re.hypot(self.im);
        let re = ((r + self.re) / 2.0).sqrt();
        let im = self.im.signum() * ((r - self.re) / 2.0).sqrt();
        if self.im == 0.0 && self.re < 0.0 { Self::new(0.0, r.sqrt()) } else { Self::new(re, im) }
    }
    fn ln(self) -> std::result::Result<Self, EvalError> {
        let r = self.re.hypot(self.im);
        if r == 0.0 { return Err(EvalError::DivisionByZero); }
        Ok(Self::new(r.ln(), self.im.atan2(self.re)))
    }
    fn abs(self) -> f64 { self.re.hypot(self.im) }
    fn to_f64_best_effort(self) -> f64 { if self.im.abs() < 1e-15 { self.re } else { self.re.hypot(self.im) } }
}

fn check_result(v: f64) -> std::result::Result<f64, EvalError> {
    if v.is_nan() { return Ok(v); }
    if v.is_infinite() {
        if v.is_sign_positive() { return Err(EvalError::Overflow("result is positive infinity".to_string())); }
        else { return Err(EvalError::Overflow("result is negative infinity".to_string())); }
    }
    Ok(v)
}

fn check_complex(v: ComplexVal) -> std::result::Result<ComplexVal, EvalError> {
    check_result(v.re)?;
    check_result(v.im)?;
    Ok(v)
}

fn gamma(z: f64) -> f64 {
    let p = [676.5203681218851, -1259.1392167224028, 771.3234287776531, -176.61502916214059, 12.507343278686905, -0.13857109526572012, 9.9843695780195716e-6, 1.5056327351493116e-7];
    if z < 0.5 { std::f64::consts::PI / ((std::f64::consts::PI * z).sin() * gamma(1.0 - z)) } 
    else { let z = z - 1.0; let mut x = 0.99999999999980993; for (i, val) in p.iter().enumerate() { x += val / (z + i as f64 + 1.0); } let t = z + 7.0 + 0.5; (2.0 * std::f64::consts::PI).sqrt() * t.powf(z + 0.5) * (-t).exp() * x }
}

pub fn evaluate(root: &Expr) -> std::result::Result<f64, EvalError> {
    let result = catch_unwind(AssertUnwindSafe(|| evaluate_internal(root)));
    match result { Ok(res) => res, Err(_) => Err(EvalError::Panic) }
}

fn evaluate_internal(root: &Expr) -> std::result::Result<f64, EvalError> {
    enum Task<'a> { Eval(&'a Expr), ComputeBinOp(BinOp), ComputeUnaryOp(UnaryOp), ComputeFunction(String, usize), HandleSequence(usize), ComputeLog(bool) }
    const MAX_STACK: usize = 50000;
    let mut tasks = vec![Task::Eval(root)];
    let mut values: Vec<ComplexVal> = vec![];
    while let Some(task) = tasks.pop() {
        if tasks.len() > MAX_STACK || values.len() > MAX_STACK { return Err(EvalError::StackError("complexity limit exceeded".to_string())); }
        match task {
            Task::Eval(expr) => match expr {
                Expr::Number(n) | Expr::Integer(n) => values.push(ComplexVal::from_real(check_result(*n)?)),
                Expr::Boolean(b) => values.push(ComplexVal::from_real(if *b { 1.0 } else { 0.0 })),
                Expr::Complex { re, im } => values.push(check_complex(ComplexVal::new(*re, *im))?),
                Expr::Infinity => values.push(ComplexVal::from_real(f64::INFINITY)),
                Expr::NegInfinity => values.push(ComplexVal::from_real(f64::NEG_INFINITY)),
                Expr::NaN => values.push(ComplexVal::from_real(f64::NAN)),
                Expr::Variable(name) => {
                    let name_l = name.to_lowercase();
                    let val = match name_l.as_str() {
                        "e" => ComplexVal::from_real(std::f64::consts::E), "pi" | "π" => ComplexVal::from_real(std::f64::consts::PI),
                        "tau" | "τ" => ComplexVal::from_real(std::f64::consts::TAU), "phi" | "φ" => ComplexVal::from_real(1.618033988749895),
                        "i" | "j" => ComplexVal::new(0.0, 1.0),
                        _ => {
                            if name_l.ends_with('i') || name_l.ends_with('j') {
                                let p = &name_l[..name_l.len()-1];
                                if p.is_empty() { ComplexVal::new(0.0, 1.0) } else if let Ok(n) = p.parse::<f64>() { ComplexVal::new(0.0, n) }
                                else { return Err(EvalError::UnknownVariable(name.clone())); }
                            } else {
                                let cleaned: String = name_l.chars().take_while(|c| c.is_digit(10) || *c == '.' || *c == '-' || *c == '+').collect();
                                if !cleaned.is_empty() { if let Ok(n) = cleaned.parse::<f64>() { ComplexVal::from_real(n) } else { return Err(EvalError::UnknownVariable(name.clone())); } }
                                else { return Err(EvalError::UnknownVariable(name.clone())); }
                            }
                        }
                    };
                    values.push(check_complex(val)?);
                }
                Expr::BinOp { op, lhs, rhs } => { tasks.push(Task::ComputeBinOp(*op)); tasks.push(Task::Eval(rhs)); tasks.push(Task::Eval(lhs)); }
                Expr::UnaryOp { op, expr } => { tasks.push(Task::ComputeUnaryOp(*op)); tasks.push(Task::Eval(expr)); }
                Expr::FunctionCall { name, args } => { tasks.push(Task::ComputeFunction(name.clone(), args.len())); for a in args.iter().rev() { tasks.push(Task::Eval(a)); } }
                Expr::Sequence(exprs) => { if exprs.is_empty() { values.push(ComplexVal::from_real(0.0)); } else { tasks.push(Task::HandleSequence(exprs.len())); for e in exprs.iter().rev() { tasks.push(Task::Eval(e)); } } }
                Expr::Add { lhs, rhs } => { tasks.push(Task::ComputeBinOp(BinOp::Add)); tasks.push(Task::Eval(rhs)); tasks.push(Task::Eval(lhs)); }
                Expr::Sub { lhs, rhs } => { tasks.push(Task::ComputeBinOp(BinOp::Sub)); tasks.push(Task::Eval(rhs)); tasks.push(Task::Eval(lhs)); }
                Expr::Mul { lhs, rhs } => { tasks.push(Task::ComputeBinOp(BinOp::Mul)); tasks.push(Task::Eval(rhs)); tasks.push(Task::Eval(lhs)); }
                Expr::Div { lhs, rhs } => { tasks.push(Task::ComputeBinOp(BinOp::Div)); tasks.push(Task::Eval(rhs)); tasks.push(Task::Eval(lhs)); }
                Expr::Pow { lhs, rhs } => { tasks.push(Task::ComputeBinOp(BinOp::Pow)); tasks.push(Task::Eval(rhs)); tasks.push(Task::Eval(lhs)); }
                Expr::Mod { lhs, rhs } => { tasks.push(Task::ComputeBinOp(BinOp::Mod)); tasks.push(Task::Eval(rhs)); tasks.push(Task::Eval(lhs)); }
                Expr::Log { expr, base } => { tasks.push(Task::ComputeLog(base.is_some())); if let Some(b) = base { tasks.push(Task::Eval(b)); } tasks.push(Task::Eval(expr)); }
                Expr::Sqrt { expr } => { tasks.push(Task::ComputeUnaryOp(UnaryOp::Sqrt)); tasks.push(Task::Eval(expr)); }
                Expr::Neg { expr } => { tasks.push(Task::ComputeUnaryOp(UnaryOp::Neg)); tasks.push(Task::Eval(expr)); }
                Expr::Paren(expr) => { tasks.push(Task::Eval(expr)); }
            },
            Task::ComputeBinOp(op) => {
                let rhs = values.pop().ok_or_else(|| EvalError::StackError("missing rhs".to_string()))?;
                let lhs = values.pop().ok_or_else(|| EvalError::StackError("missing lhs".to_string()))?;
                let res = match op {
                    BinOp::Add => lhs.add(rhs), BinOp::Sub => lhs.sub(rhs), BinOp::Mul => lhs.mul(rhs),
                    BinOp::Div | BinOp::FloorDiv => { let r = lhs.div(rhs)?; if op == BinOp::FloorDiv { ComplexVal::from_real(r.re.floor()) } else { r } }
                    BinOp::Pow => lhs.pow(rhs)?, BinOp::Mod => { if rhs.re == 0.0 { return Err(EvalError::DivisionByZero); } if lhs.im != 0.0 || rhs.im != 0.0 { return Err(EvalError::InvalidArgument("modulo not defined for complex".to_string())); } ComplexVal::from_real(lhs.re % rhs.re) }
                    _ => ComplexVal::from_real(match op { BinOp::Eq => if (lhs.re-rhs.re).abs()<f64::EPSILON && (lhs.im-rhs.im).abs()<f64::EPSILON { 1.0 } else { 0.0 }, BinOp::Ne => if (lhs.re-rhs.re).abs()>=f64::EPSILON || (lhs.im-rhs.im).abs()>=f64::EPSILON { 1.0 } else { 0.0 }, BinOp::Lt => if lhs.re < rhs.re { 1.0 } else { 0.0 }, BinOp::Gt => if lhs.re > rhs.re { 1.0 } else { 0.0 }, BinOp::Le => if lhs.re <= rhs.re { 1.0 } else { 0.0 }, BinOp::Ge => if lhs.re >= rhs.re { 1.0 } else { 0.0 }, BinOp::And => if lhs.re != 0.0 && rhs.re != 0.0 { 1.0 } else { 0.0 }, BinOp::Or => if lhs.re != 0.0 || rhs.re != 0.0 { 1.0 } else { 0.0 }, BinOp::BitAnd => ((lhs.re as i64) & (rhs.re as i64)) as f64, BinOp::BitOr => ((lhs.re as i64) | (rhs.re as i64)) as f64, BinOp::BitXor => ((lhs.re as i64) ^ (rhs.re as i64)) as f64, BinOp::Shl => ((lhs.re as i64) << (rhs.re as i64).clamp(0, 63)) as f64, BinOp::Shr => ((lhs.re as i64) >> (rhs.re as i64).clamp(0, 63)) as f64, _ => 0.0 }),
                };
                values.push(check_complex(res)?);
            }
            Task::ComputeUnaryOp(op) => {
                let v = values.pop().ok_or_else(|| EvalError::StackError("missing operand".to_string()))?;
                let res = match op { UnaryOp::Neg => ComplexVal::new(-v.re, -v.im), UnaryOp::Pos => v, UnaryOp::Fact => { if v.im != 0.0 { return Err(EvalError::InvalidArgument("factorial not defined for complex".to_string())); } if v.re < 0.0 && v.re.fract() == 0.0 { return Err(EvalError::DivisionByZero); } if v.re > 170.0 { return Err(EvalError::Overflow("factorial too large".to_string())); } if v.re.fract() == 0.0 { let mut r = 1.0; for i in 1..=(v.re as u64) { r *= i as f64; } ComplexVal::from_real(r) } else { ComplexVal::from_real(gamma(v.re + 1.0)) } } UnaryOp::Percent => ComplexVal::new(v.re / 100.0, v.im / 100.0), UnaryOp::Sqrt => v.sqrt(), UnaryOp::Log => v.ln()?, _ => v };
                values.push(check_complex(res)?);
            }
            Task::ComputeLog(has_base) => { if has_base { let base_val = values.pop().ok_or_else(|| EvalError::StackError("missing log base".to_string()))?; let expr_val = values.pop().ok_or_else(|| EvalError::StackError("missing log expr".to_string()))?; values.push(check_complex(expr_val.ln()?.div(base_val.ln()?)?)?); } else { let val = values.pop().ok_or_else(|| EvalError::StackError("missing log operand".to_string()))?; values.push(check_complex(val.ln()?)?); } }
            Task::ComputeFunction(name, count) => {
                let mut args = Vec::with_capacity(count); for _ in 0..count { args.push(values.pop().ok_or_else(|| EvalError::StackError("missing arg".to_string()))?); } args.reverse();
                let name_l = name.to_lowercase(); let name_clean = name_l.split(':').last().unwrap_or(&name_l).split('.').last().unwrap_or(&name_l);
                let res = match name_clean {
                    "sin" | "sine" => { let v = args.first().copied().unwrap_or(ComplexVal::from_real(0.0)); ComplexVal::new(v.re.sin() * v.im.cosh(), v.re.cos() * v.im.sinh()) },
                    "sind" | "sin_deg" => { let v = args.first().copied().unwrap_or(ComplexVal::from_real(0.0)); let r = v.re.to_radians(); ComplexVal::from_real(r.sin()) },
                    "cos" | "cosine" => { let v = args.first().copied().unwrap_or(ComplexVal::from_real(0.0)); ComplexVal::new(v.re.cos() * v.im.cosh(), -v.re.sin() * v.im.sinh()) },
                    "cosd" | "cos_deg" => { let v = args.first().copied().unwrap_or(ComplexVal::from_real(0.0)); let r = v.re.to_radians(); ComplexVal::from_real(r.cos()) },
                    "tan" | "tangent" => { let v = args.first().copied().unwrap_or(ComplexVal::from_real(0.0)); let s = ComplexVal::new(v.re.sin() * v.im.cosh(), v.re.cos() * v.im.sinh()); let c = ComplexVal::new(v.re.cos() * v.im.cosh(), -v.re.sin() * v.im.sinh()); s.div(c)? },
                    "log" | "log10" => { if args.len() == 1 { args[0].ln()?.div(ComplexVal::from_real(10.0).ln()?)? } else if args.len() >= 2 { args[1].ln()?.div(args[0].ln()?)? } else { return Err(EvalError::InvalidArgument("log needs arg".to_string())); } },
                    "ln" => if !args.is_empty() { args[0].ln()? } else { return Err(EvalError::InvalidArgument("ln needs arg".to_string())); },
                    "sqrt" => args.first().ok_or_else(|| EvalError::InvalidArgument("sqrt needs arg".to_string()))?.sqrt(),
                    "abs" => ComplexVal::from_real(args.first().copied().unwrap_or(ComplexVal::from_real(0.0)).abs()),
                    "exp" => { let v = args.first().copied().unwrap_or(ComplexVal::from_real(0.0)); let r = v.re.exp(); ComplexVal::new(r * v.im.cos(), r * v.im.sin()) },
                    "pow" => if args.len() >= 2 { args[0].pow(args[1])? } else { return Err(EvalError::InvalidArgument("pow needs 2 args".to_string())); },
                    "mod" | "remainder" => if args.len() >= 2 { if args[1].re == 0.0 { return Err(EvalError::DivisionByZero); } ComplexVal::from_real(args[0].re % args[1].re) } else { return Err(EvalError::InvalidArgument("mod needs 2 args".to_string())); },
                    _ => return Err(EvalError::UnknownFunction(name)),
                };
                values.push(check_complex(res)?);
            }
            Task::HandleSequence(count) => { if count == 0 { values.push(ComplexVal::from_real(0.0)); } else { let last = values.pop().ok_or_else(|| EvalError::StackError("empty seq".to_string()))?; for _ in 1..count { values.pop(); } values.push(last); } }
        }
    }
    let final_res = values.pop().ok_or_else(|| EvalError::StackError("empty eval stack".to_string()))?; check_result(final_res.to_f64_best_effort())
}

async fn send_response<W>(writer: &mut W, request_id: String, output: Option<String>, error: Option<ModuleError>, processing_ms: u64)
where W: tokio::io::AsyncWrite + Unpin {
    let response = ModuleResponse { request_id, output, error, processing_ms };
    if let Ok(payload) = serde_json::to_vec(&response) { let mut p = payload; p.push(b'\n'); let _ = writer.write_all(&p).await; let _ = writer.flush().await; }
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let addr_or_path = env::args().nth(1).unwrap_or_else(|| "/tmp/genesis-core/evaluator.sock".to_string());
    let listener = compat::UnixListener::bind(&addr_or_path)?;
    tracing::info!("evaluator listening on {}", addr_or_path);
    loop { match listener.accept().await { Ok((s, _)) => { tokio::spawn(async move { let _ = handle_client(s).await; }); } Err(e) => { tracing::error!("accept error: {}", e); } } }
}

fn init_tracing() { let _ = tracing_subscriber::fmt().try_init(); }

async fn handle_client<S>(stream: S) -> Result<()>
where S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static {
    let (r, mut w) = tokio::io::split(stream);
    let mut reader = BufReader::new(r);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) => break,
            Ok(_) => {
                if line.len() > 10 * 1024 * 1024 { send_response(&mut w, "unknown".to_string(), None, Some(ModuleError { code: "MODULE_CRASH".to_string(), message: "Request too large".to_string(), input_position: None }), 0).await; break; }
                let start = std::time::Instant::now();
                let request: ModuleRequest = match serde_json::from_str(&line) { Ok(req) => req, Err(_) => { send_response(&mut w, "unknown".to_string(), None, Some(ModuleError { code: "MODULE_CRASH".to_string(), message: "Failed to parse request".to_string(), input_position: None }), 0).await; continue; } };
                let expr_r: std::result::Result<Expr, _> = serde_json::from_str(&request.input);
                match expr_r {
                    Ok(expr) => match evaluate(&expr) {
                        Ok(v) => send_response(&mut w, request.request_id, Some(v.to_string()), None, start.elapsed().as_millis() as u64).await,
                        Err(e) => {
                            let code = match &e { EvalError::DivisionByZero => "DIVISION_BY_ZERO", EvalError::Overflow(_) | EvalError::InvalidArgument(_) => "OVERFLOW", EvalError::Panic | EvalError::StackError(_) => "MODULE_CRASH", EvalError::UnknownFunction(_) | EvalError::UnknownVariable(_) => "UNKNOWN_PATTERN" };
                            send_response(&mut w, request.request_id, None, Some(ModuleError { code: code.to_string(), message: e.to_string(), input_position: None }), start.elapsed().as_millis() as u64).await;
                        }
                    },
                    Err(e) => send_response(&mut w, request.request_id, None, Some(ModuleError { code: "UNKNOWN_PATTERN".to_string(), message: format!("AST Parse Error: {}", e), input_position: None }), start.elapsed().as_millis() as u64).await,
                }
            }
            Err(_) => break,
        }
    }
    Ok(())
}