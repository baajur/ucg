// Copyright 2017 Jeremy Wall <jeremy@marzhillstudios.com>
//
//  Licensed under the Apache License, Version 2.0 (the "License");
//  you may not use this file except in compliance with the License.
//  You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
//  Unless required by applicable law or agreed to in writing, software
//  distributed under the License is distributed on an "AS IS" BASIS,
//  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//  See the License for the specific language governing permissions and
//  limitations under the License.

//! Bottom up parser for precedence parsing of expressions separated by binary
//! operators.
use abortable_parser::combinators::eoi;
use abortable_parser::{Error, Result, SliceIter};

use super::{non_op_expression, ParseResult};
use crate::ast::*;

/// Defines the intermediate stages of our bottom up parser for precedence parsing.
#[derive(Debug, PartialEq, Clone)]
pub enum Element {
    Expr(Expression),
    Op(BinaryExprType),
}

make_fn!(
    dot_op_type<SliceIter<Token>, Element>,
    do_each!(
        _ => punct!("."),
        (Element::Op(BinaryExprType::DOT)))
);

make_fn!(
    math_op_type<SliceIter<Token>, Element>,
    either!(
        do_each!(
            _ => punct!("+"),
            (Element::Op(BinaryExprType::Add))),
        do_each!(
            _ => punct!("-"),
            (Element::Op(BinaryExprType::Sub))),
        do_each!(
            _ => punct!("*"),
            (Element::Op(BinaryExprType::Mul))),
        do_each!(
            _ => punct!("/"),
            (Element::Op(BinaryExprType::Div)))
    )
);

fn parse_expression(i: SliceIter<Element>) -> Result<SliceIter<Element>, Expression> {
    let mut i_ = i.clone();
    if eoi(i_.clone()).is_complete() {
        return Result::Abort(Error::new(
            "Expected Expression found End Of Input",
            Box::new(i_),
        ));
    }
    let el = i_.next();
    if let Some(&Element::Expr(ref expr)) = el {
        return Result::Complete(i_.clone(), expr.clone());
    }
    return Result::Fail(Error::new(
        format!(
            "Error while parsing Binary Expression Expected Expression got {:?}",
            el
        ),
        Box::new(i_),
    ));
}

fn parse_dot_operator(i: SliceIter<Element>) -> Result<SliceIter<Element>, BinaryExprType> {
    let mut i_ = i.clone();
    if eoi(i_.clone()).is_complete() {
        return Result::Fail(Error::new(
            format!("Expected Expression found End Of Input"),
            Box::new(i_),
        ));
    }
    let el = i_.next();
    if let Some(&Element::Op(ref op)) = el {
        match op {
            &BinaryExprType::DOT => {
                return Result::Complete(i_.clone(), op.clone());
            }
            _other => {
                // noop
            }
        };
    }
    return Result::Fail(Error::new(
        format!(
            "Error while parsing Binary Expression Unexpected Operator {:?}",
            el
        ),
        Box::new(i_),
    ));
}

fn parse_sum_operator(i: SliceIter<Element>) -> Result<SliceIter<Element>, BinaryExprType> {
    let mut i_ = i.clone();
    if eoi(i_.clone()).is_complete() {
        return Result::Fail(Error::new(
            format!("Expected Expression found End Of Input"),
            Box::new(i_),
        ));
    }
    let el = i_.next();
    if let Some(&Element::Op(ref op)) = el {
        match op {
            &BinaryExprType::Add => {
                return Result::Complete(i_.clone(), op.clone());
            }
            &BinaryExprType::Sub => {
                return Result::Complete(i_.clone(), op.clone());
            }
            _other => {
                // noop
            }
        };
    }
    return Result::Fail(Error::new(
        format!(
            "Error while parsing Binary Expression Unexpected Operator {:?}",
            el
        ),
        Box::new(i_),
    ));
}

fn tuple_to_binary_expression(
    kind: BinaryExprType,
    left: Expression,
    right: Expression,
) -> Expression {
    let pos = left.pos().clone();
    Expression::Binary(BinaryOpDef {
        kind: kind,
        left: Box::new(left),
        right: Box::new(right),
        pos: pos,
    })
}

fn parse_product_operator(i: SliceIter<Element>) -> Result<SliceIter<Element>, BinaryExprType> {
    let mut i_ = i.clone();
    if eoi(i_.clone()).is_complete() {
        return Result::Fail(Error::new(
            format!("Expected Expression found End Of Input"),
            Box::new(i_),
        ));
    }
    let el = i_.next();
    if let Some(&Element::Op(ref op)) = el {
        match op {
            &BinaryExprType::Mul => {
                return Result::Complete(i_.clone(), op.clone());
            }
            &BinaryExprType::Div => {
                return Result::Complete(i_.clone(), op.clone());
            }
            _other => {
                // noop
            }
        };
    }
    return Result::Fail(Error::new(
        format!(
            "Error while parsing Binary Expression Unexpected Operator {:?}",
            el
        ),
        Box::new(i_),
    ));
}

/// do_binary_expr implements precedence based parsing where the more tightly bound
/// parsers are passed in as lowerrule parsers. We default to any non_op_expression
/// as the most tightly bound expressions.
macro_rules! do_binary_expr {
    ($i:expr, $oprule:ident, $lowerrule:ident) => {
        do_binary_expr!($i, run!($oprule), $lowerrule)
    };

    ($i:expr, $oprule:ident, $lowerrule:ident!( $($lowerargs:tt)* )) => {
        do_binary_expr!($i, run!($oprule), $lowerrule!($($lowerargs)*))
    };

    ($i:expr, $oprule:ident) => {
        do_binary_expr!($i, run!($oprule))
    };

    ($i:expr, $oprule:ident!( $($args:tt)* )) => {
        do_binary_expr!($i, $oprule!($($args)*), parse_expression)
    };

    ($i:expr, $oprule:ident!( $($args:tt)* ), $lowerrule:ident) => {
        do_binary_expr!($i, $oprule!($($args)*), run!($lowerrule))
    };

    ($i:expr, $oprule:ident!( $($args:tt)* ), $lowerrule:ident!( $($lowerargs:tt)* )) => {
        do_each!($i,
            left => $lowerrule!($($lowerargs)*),
                typ => $oprule!($($args)*),
                right => $lowerrule!($($lowerargs)*),
                (tuple_to_binary_expression(typ, left, right))
        )
    };
}

make_fn!(
    sum_expression<SliceIter<Element>, Expression>,
    do_binary_expr!(
        parse_sum_operator,
        either!(
            trace_parse!(product_expression),
            trace_parse!(dot_expression),
            trace_parse!(parse_expression)
        )
    )
);

make_fn!(
    product_expression<SliceIter<Element>, Expression>,
    do_binary_expr!(
        parse_product_operator,
        either!(trace_parse!(dot_expression), trace_parse!(parse_expression))
    )
);

make_fn!(
    math_expression<SliceIter<Element>, Expression>,
    either!(
        trace_parse!(sum_expression),
        trace_parse!(product_expression)
    )
);

make_fn!(
    compare_op_type<SliceIter<Token>, Element>,
    either!(
        do_each!(_ => punct!("=="), (Element::Op(BinaryExprType::Equal))),
        do_each!(_ => punct!("!="), (Element::Op(BinaryExprType::NotEqual))),
        do_each!(_ => punct!("<="), (Element::Op(BinaryExprType::LTEqual))),
        do_each!(_ => punct!(">="), (Element::Op(BinaryExprType::GTEqual))),
        do_each!(_ => punct!("<"),  (Element::Op(BinaryExprType::LT))),
        do_each!(_ => punct!(">"),  (Element::Op(BinaryExprType::GT)))
    )
);

fn parse_compare_operator(i: SliceIter<Element>) -> Result<SliceIter<Element>, BinaryExprType> {
    let mut i_ = i.clone();
    if eoi(i_.clone()).is_complete() {
        return Result::Fail(Error::new(
            format!("Expected Expression found End Of Input"),
            Box::new(i_),
        ));
    }
    let el = i_.next();
    if let Some(&Element::Op(ref op)) = el {
        match op {
            &BinaryExprType::GT
            | &BinaryExprType::GTEqual
            | &BinaryExprType::LT
            | &BinaryExprType::LTEqual
            | &BinaryExprType::NotEqual
            | &BinaryExprType::Equal => {
                return Result::Complete(i_.clone(), op.clone());
            }
            _other => {
                // noop
            }
        };
    }
    return Result::Fail(Error::new(
        format!(
            "Error while parsing Binary Expression Unexpected Operator {:?}",
            el
        ),
        Box::new(i),
    ));
}

make_fn!(
    binary_expression<SliceIter<Element>, Expression>,
    either!(
        compare_expression,
        math_expression,
        dot_expression,
        parse_expression
    )
);

make_fn!(
    dot_expression<SliceIter<Element>, Expression>,
    do_binary_expr!(parse_dot_operator, trace_parse!(parse_expression))
);

make_fn!(
    compare_expression<SliceIter<Element>, Expression>,
    do_binary_expr!(
        parse_compare_operator,
        either!(
            trace_parse!(math_expression),
            trace_parse!(dot_expression),
            trace_parse!(parse_expression)
        )
    )
);

/// Parse a list of expressions separated by operators into a Vec<Element>.
fn parse_operand_list<'a>(i: SliceIter<'a, Token>) -> ParseResult<'a, Vec<Element>> {
    // 1. First try to parse a non_op_expression,
    let mut _i = i.clone();
    let mut list = Vec::new();
    // 1. loop
    let mut firstrun = true;
    loop {
        // 2. Parse a non_op_expression.
        match non_op_expression(_i.clone()) {
            Result::Fail(e) => {
                // A failure to parse an expression
                // is always an error.
                return Result::Fail(e);
            }
            Result::Abort(e) => {
                // A failure to parse an expression
                // is always an error.
                return Result::Abort(e);
            }
            Result::Incomplete(i) => {
                return Result::Incomplete(i);
            }
            Result::Complete(rest, expr) => {
                list.push(Element::Expr(expr));
                _i = rest.clone();
            }
        }
        // 3. Parse an operator.
        // TODO(jwall): Parse the dot operator.
        match either!(_i.clone(), dot_op_type, math_op_type, compare_op_type) {
            Result::Fail(e) => {
                if firstrun {
                    // If we don't find an operator in our first
                    // run then this is not an operand list.
                    return Result::Fail(e);
                }
                // if we don't find one on subsequent runs then
                // that's the end of the operand list.
                break;
            }
            Result::Abort(e) => {
                // A failure to parse an expression
                // is always an error.
                return Result::Abort(e);
            }
            Result::Incomplete(i) => {
                return Result::Incomplete(i);
            }
            Result::Complete(rest, el) => {
                list.push(el);
                _i = rest.clone();
            }
        }
        firstrun = false;
    }
    return Result::Complete(_i, list);
}

/// Parse a binary operator expression.
pub fn op_expression<'a>(i: SliceIter<'a, Token>) -> Result<SliceIter<Token>, Expression> {
    let preparse = parse_operand_list(i.clone());
    match preparse {
        Result::Fail(e) => {
            let err = Error::caused_by(
                "Failed while parsing operator expression",
                Box::new(e),
                Box::new(i),
            );
            Result::Fail(err)
        }
        Result::Abort(e) => {
            let err = Error::caused_by(
                "Failed while parsing operator expression",
                Box::new(e),
                Box::new(i),
            );
            Result::Fail(err)
        }
        Result::Incomplete(i) => Result::Incomplete(i),
        Result::Complete(rest, oplist) => {
            let i_ = SliceIter::new(&oplist);
            let parse_result = binary_expression(i_);

            match parse_result {
                Result::Fail(_e) => {
                    // TODO(jwall): It would be good to be able to use caused_by here.
                    let err = Error::new(
                        "Failed while parsing operator expression",
                        Box::new(rest.clone()),
                    );
                    Result::Fail(err)
                }
                Result::Abort(_e) => {
                    let err = Error::new(
                        "Failed while parsing operator expression",
                        Box::new(rest.clone()),
                    );
                    Result::Abort(err)
                }
                Result::Incomplete(_) => Result::Incomplete(i.clone()),
                Result::Complete(_, expr) => Result::Complete(rest.clone(), expr),
            }
        }
    }
}
