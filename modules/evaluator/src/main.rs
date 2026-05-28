#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Number(i32),
    Binary(BinOp, Box<Expr>, Box<Expr>),
}

// Custom drop implementation to prevent stack overflow when dropping deep ASTs
impl Drop for Expr {
    fn drop(&mut self) {
        let mut stack = Vec::new();
        if let Expr::Binary(_, left, right) = self {
            stack.push(std::mem::replace(&mut **left, Expr::Number(0)));
            stack.push(std::mem::replace(&mut **right, Expr::Number(0)));
        }
        while let Some(mut expr) = stack.pop() {
            if let Expr::Binary(_, left, right) = &mut expr {
                stack.push(std::mem::replace(&mut **left, Expr::Number(0)));
                stack.push(std::mem::replace(&mut **right, Expr::Number(0)));
            }
        }
    }
}

enum Task<'a> {
    Eval(&'a Expr),
    Apply(BinOp),
}

/// Evaluates the given expression iteratively to prevent stack overflow.
pub fn evaluate(expr: &Expr) -> i32 {
    let mut tasks = vec![Task::Eval(expr)];
    let mut values = vec![];

    while let Some(task) = tasks.pop() {
        match task {
            Task::Eval(e) => match e {
                Expr::Number(n) => values.push(*n),
                Expr::Binary(op, lhs, rhs) => {
                    tasks.push(Task::Apply(*op));
                    tasks.push(Task::Eval(rhs));
                    tasks.push(Task::Eval(lhs));
                }
            },
            Task::Apply(op) => {
                let right = values.pop().unwrap_or(0);
                let left = values.pop().unwrap_or(0);
                let res = match op {
                    BinOp::Add => left.saturating_add(right),
                    BinOp::Sub => left.saturating_sub(right),
                    BinOp::Mul => left.saturating_mul(right),
                    BinOp::Div => {
                        if right == 0 {
                            0
                        } else {
                            left / right
                        }
                    }
                };
                values.push(res);
            }
        }
    }

    values.pop().unwrap_or(0)
}

/// Alias for evaluate.
pub fn eval(expr: &Expr) -> i32 {
    evaluate(expr)
}

fn main() {
    let expr = Expr::Binary(
        BinOp::Add,
        Box::new(Expr::Number(5)),
        Box::new(Expr::Number(10)),
    );
    println!("Result: {}", eval(&expr));
}