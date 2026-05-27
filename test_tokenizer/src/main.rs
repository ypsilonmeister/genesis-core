use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum Token {
    Number(f64),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    LParen,
    RParen,
    Comma,
    Exclamation,
    Question,
    Colon,
    DotDot,
    LShift,
    Gt,
    Lt,
    Percent,
    Sqrt,
    Cbrt,
    Function(String),
}

#[derive(Debug, thiserror::Error)]
pub enum TokenizeError {
    #[error("unknown token at position {position}: {character:?}")]
    UnknownToken { position: usize, character: char },
    #[error("unknown pattern at position {position}")]
    UnknownPattern { position: usize },
    #[error("empty input")]
    Empty,
}

pub fn tokenize(input: &str) -> Result<Vec<Token>, TokenizeError> {
    if input.is_empty() {
        return Err(TokenizeError::Empty);
    }

    let mut tokens = Vec::new();
    let mut chars = input.char_indices().peekable();

    while let Some(&(idx, c)) = chars.peek() {
        match c {
            ' ' | '\t' | '\n' | '\r' => {
                chars.next();
            }
            '0'..='9' | '.' => {
                if c == '.' {
                    let mut temp = chars.clone();
                    temp.next();
                    if let Some(&(_, '.')) = temp.peek() {
                        tokens.push(Token::DotDot);
                        chars.next();
                        chars.next();
                        continue;
                    }
                }

                let mut buf = String::new();
                let mut has_dot = false;
                let mut has_e = false;
                let mut has_exponent_dot = false;
                let start_idx = idx;

                while let Some(&(_, c)) = chars.peek() {
                    if c.is_ascii_digit() {
                        buf.push(c);
                        chars.next();
                    } else if c == '.' {
                        let mut temp = chars.clone();
                        temp.next();
                        if let Some(&(_, '.')) = temp.peek() {
                            break;
                        }
                        if !has_dot && !has_e {
                            buf.push(c);
                            chars.next();
                            has_dot = true;
                        } else if has_e && !has_exponent_dot {
                            buf.push(c);
                            chars.next();
                            has_exponent_dot = true;
                        } else {
                            break;
                        }
                    } else if (c == 'e' || c == 'E') && !has_e {
                        buf.push(c);
                        chars.next();
                        has_e = true;
                        if let Some(&(_, next_c)) = chars.peek() {
                            if next_c == '+' || next_c == '-' {
                                buf.push(next_c);
                                chars.next();
                            }
                        }
                    } else {
                        break;
                    }
                }

                let n: f64 = if has_exponent_dot {
                    let parts: Vec<&str> = buf.split(|c| ['e', 'E'].contains(&c)).collect();
                    if parts.len() == 2 {
                        let mantissa: f64 =
                            parts[0]
                                .parse()
                                .map_err(|_| TokenizeError::UnknownPattern {
                                    position: start_idx,
                                })?;
                        let exponent: f64 =
                            parts[1]
                                .parse()
                                .map_err(|_| TokenizeError::UnknownPattern {
                                    position: start_idx,
                                })?;
                        mantissa * 10.0_f64.powf(exponent)
                    } else {
                        return Err(TokenizeError::UnknownPattern {
                            position: start_idx,
                        });
                    }
                } else {
                    buf.parse().map_err(|_| TokenizeError::UnknownPattern {
                        position: start_idx,
                    })?
                };

                if !n.is_finite() {
                    return Err(TokenizeError::UnknownPattern {
                        position: start_idx,
                    });
                }
                tokens.push(Token::Number(n));
            }
            'a'..='z' | 'A'..='Z' => {
                let mut buf = String::new();
                while let Some(&(_, c)) = chars.peek() {
                    if c.is_ascii_alphanumeric() || c == '_' {
                        buf.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(Token::Function(buf));
            }
            '+' => {
                tokens.push(Token::Plus);
                chars.next();
            }
            '-' => {
                tokens.push(Token::Minus);
                chars.next();
            }
            '*' => {
                tokens.push(Token::Star);
                chars.next();
            }
            '/' => {
                tokens.push(Token::Slash);
                chars.next();
            }
            '^' => {
                tokens.push(Token::Caret);
                chars.next();
            }
            '(' => {
                tokens.push(Token::LParen);
                chars.next();
            }
            ')' => {
                tokens.push(Token::RParen);
                chars.next();
            }
            ',' => {
                tokens.push(Token::Comma);
                chars.next();
            }
            '!' => {
                tokens.push(Token::Exclamation);
                chars.next();
            }
            '?' => {
                tokens.push(Token::Question);
                chars.next();
            }
            ':' => {
                tokens.push(Token::Colon);
                chars.next();
            }
            '>' => {
                tokens.push(Token::Gt);
                chars.next();
            }
            '<' => {
                chars.next();
                if let Some(&(_, '<')) = chars.peek() {
                    tokens.push(Token::LShift);
                    chars.next();
                } else {
                    tokens.push(Token::Lt);
                }
            }
            '%' => {
                tokens.push(Token::Percent);
                chars.next();
            }
            '√' => {
                tokens.push(Token::Sqrt);
                chars.next();
            }
            '∛' => {
                tokens.push(Token::Cbrt);
                chars.next();
            }
            _ => {
                return Err(TokenizeError::UnknownPattern { position: idx });
            }
        }
    }
    Ok(tokens)
}

fn main() {
    for input in &[
        "π^2 - e",
        "5 / (3 + 2) ** 1",
        "4 + 5 ! 2",
        "2 ** 3 * 4 / 2",
        "sin(pi/2) + cos(pi/2) * 10",
    ] {
        println!("{:?}: {:?}", input, tokenize(input));
    }
}
