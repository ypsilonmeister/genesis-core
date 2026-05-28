#[derive(Debug, PartialEq)]
pub enum Token {
    Plus,
    Minus,
    Star,
    Slash,
}

#[derive(Debug, PartialEq)]
pub enum ParseError {
    UnknownPattern,
}

pub fn tokenize(input: &str) -> Result<Vec<Token>, ParseError> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' | '\r' | '\n' => {
                chars.next();
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
                chars.next();
                if chars.peek() == Some(&'*') {
                    return Err(ParseError::UnknownPattern);
                }
                tokens.push(Token::Star);
            }
            '/' => {
                // If slash is not allowed? Or is it allowed?
                tokens.push(Token::Slash);
                chars.next();
            }
            _ => {
                return Err(ParseError::UnknownPattern);
            }
        }
    }
    Ok(tokens)
}

fn main() {}