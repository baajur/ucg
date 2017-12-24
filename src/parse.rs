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
use std::str::FromStr;
use std::error::Error;
use std::borrow::Borrow;

use nom;
use nom::IResult;
use nom::InputLength;

use ast::*;
use tokenizer::*;
use error as E;

macro_rules! eat_space {
  ($i:expr, $($args:tt)*) => (
    {
      sep!($i, emptyspace, $($args)*)
    }
  )
}

type ParseResult<O> = Result<O, Box<Error>>;

fn symbol_to_value(s: Token) -> ParseResult<Value> {
    Ok(Value::Symbol(value_node!(s.fragment.to_string(), s.pos)))
}

// symbol is a bare unquoted field.
named!(symbol( Span ) -> Value, map_res!(barewordtok, symbol_to_value));

fn str_to_value(s: Token) -> ParseResult<Value> {
    Ok(Value::String(value_node!(s.fragment.to_string(), s.pos)))
}

// quoted_value is a quoted string.
named!(quoted_value( Span ) -> Value,
       map_res!(strtok, str_to_value)
);

// Helper function to make the return types work for down below.
fn triple_to_number(v: (Option<Token>, Option<Token>, Option<Token>)) -> ParseResult<Value> {
    let (pref, mut pref_pos) = match v.0 {
        None => ("", Position::new(0, 0)),
        Some(ref bs) => (bs.fragment.borrow(), bs.pos.clone()),
    };

    let has_dot = v.1.is_some();

    if v.0.is_some() && !has_dot && v.2.is_none() {
        return Ok(Value::Int(value_node!(try!(FromStr::from_str(pref)), pref_pos)));
    }

    if v.0.is_none() && has_dot {
        pref_pos = v.1.unwrap().pos;
    }

    let suf = match v.2 {
        None => "",
        Some(ref bs) => &bs.fragment,
    };

    let to_parse = pref.to_string() + "." + suf;
    // TODO(jwall): if there is an error we should report where that error occured.
    let f = try!(FromStr::from_str(&to_parse));
    return Ok(Value::Float(value_node!(f, pref_pos)));
}

// NOTE(jwall): HERE THERE BE DRAGONS. The order for these matters
// alot. We need to process alternatives in order of decreasing
// specificity.  Unfortunately this means we are required to go in a
// decreasing size order which messes with alt!'s completion logic. To
// work around this we have to force Incomplete to be Error so that
// alt! will try the next in the series instead of aborting.
//
// *IMPORTANT*
// It also means this combinator is risky when used with partial
// inputs. So handle with care.
named!(number( Span ) -> Value,
       map_res!(alt!(
           complete!(do_parse!( // 1.0
               prefix: digittok >>
               has_dot: dottok >>
               suffix: digittok >>
               peek!(not!(digittok)) >>
               (Some(prefix), Some(has_dot), Some(suffix))
           )) |
           complete!(do_parse!( // 1.
               prefix: digittok >>
               has_dot: dottok >>
               peek!(not!(digittok)) >>
               (Some(prefix), Some(has_dot), None)
           )) |
           complete!(do_parse!( // .1
               has_dot: dottok >>
               suffix: digittok >>
               peek!(not!(digittok)) >>
               (None, Some(has_dot), Some(suffix))
           )) |
           do_parse!( // 1
               prefix: digittok >>
// The peek!(not!(..)) make this whole combinator slightly
// safer for partial inputs.
               peek!(not!(digittok)) >>
               (Some(prefix), None, None)
           )),
           triple_to_number
       )
);

named!(
    #[doc="Capture a field and value pair composed of `<symbol> = <value>,`"],
    field_value( Span ) -> (Token, Expression),
    do_parse!(
        field: barewordtok >>
            eat_space!(equaltok) >>
            value: expression >>
            (field, value)
    )
);

// Helper function to make the return types work for down below.
fn vec_to_tuple(t: (Span, FieldList)) -> ParseResult<Value> {
    Ok(Value::Tuple(value_node!(t.1, t.0.line as usize, t.0.offset as usize)))
}

named!(field_list( Span ) -> FieldList,
       separated_list!(commatok, eat_space!(field_value)));

named!(
    #[doc="Capture a tuple of named fields with values. {<field>=<value>,...}"],
    tuple( Span ) -> Value,
    map_res!(
        do_parse!(
            pos: position!() >>
            v: delimited!(lbracetok,
                   eat_space!(field_list),
                   rbracetok) >>
                   (pos, v)
        ),
        vec_to_tuple
    )
);

fn tuple_to_list<Sp: Into<Position>>(t: (Sp, Vec<Expression>)) -> ParseResult<Value> {
    return Ok(Value::List(ListDef {
        elems: t.1,
        pos: t.0.into(),
    }));
}

named!(list_value( Span ) -> Value,
       map_res!(
           do_parse!(
               pos: position!() >>
               leftsquarebracket >>
               elements: eat_space!(separated_list!(eat_space!(commatok), expression)) >>
               rightsquarebracket >>
               (pos, elements)
           ),
           tuple_to_list
       )
);

named!(value( Span ) -> Value,
    alt!(
        number |
        quoted_value |
        list_value |
        tuple |
        selector_value ));

fn value_to_expression(v: Value) -> ParseResult<Expression> {
    Ok(Expression::Simple(v))
}

named!(simple_expression( Span ) -> Expression,
       map_res!(
           value,
           value_to_expression
       )
);

fn tuple_to_binary_expression(tpl: (Span, BinaryExprType, Value, Expression))
                              -> ParseResult<Expression> {
    Ok(Expression::Binary(BinaryOpDef {
        kind: tpl.1,
        left: tpl.2,
        right: Box::new(tpl.3),
        pos: Position::new(tpl.0.line as usize, tpl.0.offset as usize),
    }))
}

macro_rules! do_binary_expr {
    ($i:expr, $fn:expr, $typ:expr) => {
        // NOTE(jwall): Nom macros do magic with their inputs. They in fact
        // rewrite your macro argumets for you. Which means we require this $i
        // paramater even though we don't explicitely pass it below. I don't
        // particularly like this but I'm living with it for now.
        map_res!(
            $i, do_parse!(
                pos: position!() >>
                left: value >>
                    eat_space!($fn) >>
                    right: expression >>
                    (pos, $typ, left, right)
            ),
            tuple_to_binary_expression
        )
    }
}

named!(add_expression( Span ) -> Expression,
       do_binary_expr!(plustok, BinaryExprType::Add)
);

named!(sub_expression( Span ) -> Expression,
       do_binary_expr!(dashtok, BinaryExprType::Sub)
);

named!(mul_expression( Span ) -> Expression,
       do_binary_expr!(startok, BinaryExprType::Mul)
);

named!(div_expression( Span ) -> Expression,
       do_binary_expr!(slashtok, BinaryExprType::Div)
);

fn expression_to_grouped_expression(e: Expression) -> ParseResult<Expression> {
    Ok(Expression::Grouped(Box::new(e)))
}

named!(grouped_expression( Span ) -> Expression,
       map_res!(
           preceded!(lparentok, terminated!(expression, rparentok)),
           expression_to_grouped_expression
       )
);

fn symbol_or_expression(input: Span) -> IResult<Span, Expression> {
    let sym = do_parse!(input,
        sym: symbol >>
        (sym)
    );

    match sym {
        IResult::Incomplete(i) => {
            return IResult::Incomplete(i);
        }
        IResult::Error(_) => {
            // TODO(jwall): Still missing some. But we need to avoid recursion
            return grouped_expression(input);
        }
        IResult::Done(rest, val) => {
            return IResult::Done(rest, Expression::Simple(val));
        }
    }
}

fn selector_list(input: Span) -> IResult<Span, SelectorList> {
    let (rest, head) = match symbol_or_expression(input) {
        IResult::Done(rest, val) => {
            (rest, val)
        }
        IResult::Error(e) => {
            return IResult::Error(e);
        }
        IResult::Incomplete(i) => {
            return IResult::Incomplete(i);
        }
    };
    
    let (rest, is_dot) = match dottok(rest) {
        IResult::Done(rest, _) => {
            (rest, true)
        }
        IResult::Incomplete(i) => {
            return IResult::Incomplete(i);
        }
        IResult::Error(_) => {
            (rest, false)
        }
    };
    
    let (rest, list) = if is_dot {
        let (rest, list) = match separated_list!(rest, dottok, alt!(barewordtok | digittok)) {
            IResult::Done(rest, val) => {
                (rest, val)
            }
            IResult::Incomplete(i) => {
                return IResult::Incomplete(i);
            }
            IResult::Error(e) => {
                return IResult::Error(e);
            }
        };

        if list.is_empty() {
            return IResult::Error(nom::ErrorKind::Custom(0));
        } else {
            (rest, Some(list))
        }
    } else {
        (rest, None)
    };
    
    let sel_list = SelectorList{
            head: Box::new(head),
            tail: list,
    };

    return IResult::Done(rest, sel_list);
}

fn tuple_to_copy(t: (Span, SelectorDef, FieldList)) -> ParseResult<Expression> {
    Ok(Expression::Copy(CopyDef {
        selector: t.1,
        fields: t.2,
        pos: Position::new(t.0.line as usize, t.0.offset as usize),
    }))
}

named!(copy_expression( Span ) -> Expression,
       map_res!(
           do_parse!(
               pos: position!() >>
               selector: selector_list >>
               lbracetok >>
               fields: eat_space!(field_list) >>
               rbracetok >>
               (pos, SelectorDef::new(selector, pos.line as usize, pos.offset as usize), fields)
           ),
           tuple_to_copy
       )
);

fn tuple_to_macro(mut t: (Span, Vec<Value>, Value)) -> ParseResult<Expression> {
    match t.2 {
        Value::Tuple(v) => {
            Ok(Expression::Macro(MacroDef {
                argdefs: t.1
                    .drain(0..)
                    .map(|s| {
                        Positioned {
                            pos: s.pos().clone(),
                            val: s.to_string(),
                        }
                    })
                    .collect(),
                fields: v.val,
                pos: Position::new(t.0.line as usize, t.0.offset as usize),
            }))
        }
        // TODO(jwall): Show a better version of the unexpected parsed value.
        val => {
            Err(Box::new(E::Error::new(format!("Expected Tuple Got {:?}", val),
                                       E::ErrorType::UnexpectedToken,
                                       Position {
                                           line: t.0.line as usize,
                                           column: t.0.offset as usize,
                                       })))
        }
    }
}

named!(arglist( Span ) -> Vec<Value>, separated_list!(eat_space!(commatok), symbol));

named!(macro_expression( Span ) -> Expression,
       map_res!(
           do_parse!(
                pos: position!() >>
                macrotok >>
                eat_space!(lparentok) >>
                arglist: eat_space!(arglist) >>
                rparentok >>
                eat_space!(fatcommatok) >>
                map: tuple >>
                (pos, arglist, map)
           ),
           tuple_to_macro
       )
);

fn tuple_to_select(t: (Span, Expression, Expression, Value)) -> ParseResult<Expression> {
    match t.3 {
        Value::Tuple(v) => {
            Ok(Expression::Select(SelectDef {
                val: Box::new(t.1),
                default: Box::new(t.2),
                tuple: v.val,
                pos: Position::new(t.0.line as usize, t.0.offset as usize),
            }))
        }
        val => {
            Err(Box::new(E::Error::new(format!("Expected Tuple Got {:?}", val),
                                       E::ErrorType::UnexpectedToken,
                                       Position {
                                           line: t.0.line as usize,
                                           column: t.0.offset as usize,
                                       })))
        }
    }
}

named!(select_expression( Span ) -> Expression,
       map_res!(
           do_parse!(
               pos: position!() >>
               selecttok >>
               val: eat_space!(terminated!(expression, commatok)) >>
               default: eat_space!(terminated!(expression, commatok)) >>
               map: eat_space!(tuple) >>
               (pos, val, default, map)
           ),
           tuple_to_select
       )
);

fn tuple_to_format(t: (Token, Vec<Expression>)) -> ParseResult<Expression> {
    Ok(Expression::Format(FormatDef {
        template: t.0.fragment.to_string(),
        args: t.1,
        pos: t.0.pos,
    }))
}

named!(format_expression( Span ) -> Expression,
       map_res!(
           do_parse!(
               tmpl: eat_space!(strtok) >>
                   eat_space!(pcttok) >>
                   lparentok >>
                   args: eat_space!(separated_list!(eat_space!(commatok), expression)) >>
                   rparentok >>
                   (tmpl, args)
           ),
           tuple_to_format
       )
);

fn tuple_to_call(t: (Span, Value, Vec<Expression>)) -> ParseResult<Expression> {
    if let Value::Selector(def) = t.1 {
        Ok(Expression::Call(CallDef {
            macroref: def,
            arglist: t.2,
            pos: Position::new(t.0.line as usize, t.0.offset as usize),
        }))
    } else {
        Err(Box::new(E::Error::new(format!("Expected Selector Got {:?}", t.0),
                                   E::ErrorType::UnexpectedToken,
                                   Position {
                                       line: t.0.line as usize,
                                       column: t.0.offset as usize,
                                   })))
    }
}

fn vec_to_selector_value(t: (Span, SelectorList)) -> ParseResult<Value> {
    Ok(Value::Selector(SelectorDef::new(t.1, t.0.line as usize, t.0.offset as usize)))
}

named!(selector_value( Span ) -> Value,
       map_res!(
           do_parse!(
               pos: position!() >>
               sl: eat_space!(selector_list) >>
               (pos, sl)
           ),
           vec_to_selector_value
       )
);

named!(call_expression( Span ) -> Expression,
       map_res!(
           do_parse!(
               pos: position!() >>
               macroname: selector_value >>
               lparentok >>
               args: eat_space!(separated_list!(eat_space!(commatok), expression)) >>
               rparentok >>
               (pos, macroname, args)
           ),
           tuple_to_call
       )
);

// NOTE(jwall): HERE THERE BE DRAGONS. The order for these matters
// alot. We need to process alternatives in order of decreasing
// specificity.  Unfortunately this means we are required to go in a
// decreasing size order which messes with alt!'s completion logic. To
// work around this we have to force Incomplete to be Error so that
// alt! will try the next in the series instead of aborting.
//
// *IMPORTANT*
// It also means this combinator is risky when used with partial
// inputs. So handle with care.
named!(expression( Span ) -> Expression,
       alt!(
           complete!(add_expression) |
           complete!(sub_expression) |
           complete!(mul_expression) |
           complete!(div_expression) |
           complete!(grouped_expression) |
           complete!(macro_expression) |
           complete!(format_expression) |
           complete!(select_expression) |
           complete!(call_expression) |
           complete!(copy_expression) |
           eat_space!(simple_expression)
       )
);

fn expression_to_statement(v: Expression) -> ParseResult<Statement> {
    Ok(Statement::Expression(v))
}

named!(expression_statement( Span ) -> Statement,
       map_res!(
           terminated!(eat_space!(expression), semicolontok),
           expression_to_statement
       )
);

fn tuple_to_let(t: (Token, Expression)) -> ParseResult<Statement> {
    Ok(Statement::Let(LetDef {
        name: t.0,
        value: t.1,
    }))
}

named!(let_statement( Span ) -> Statement,
       map_res!(
           terminated!(do_parse!(
               lettok >>
                   name: eat_space!(barewordtok) >>
                   equaltok >>
                   val: eat_space!(expression) >>
                   (name, val)
           ), semicolontok),
           tuple_to_let
       )
);

fn tuple_to_import(t: (Token, Token)) -> ParseResult<Statement> {
    Ok(Statement::Import(ImportDef {
        name: t.0,
        path: t.1,
    }))
}

named!(import_statement( Span ) -> Statement,
       map_res!(
           terminated!(do_parse!(
               importtok >>
                   path: eat_space!(strtok) >>
                   astok >>
                   name: eat_space!(barewordtok) >>
                   (name, path)
           ), semicolontok),
           tuple_to_import
       )
);

named!(statement( Span ) -> Statement,
       alt_complete!(
           import_statement |
           let_statement |
           expression_statement
       )
);

pub fn parse(input: Span) -> IResult<Span, Vec<Statement>> {
    let mut out = Vec::new();
    let mut i = input;
    loop {
        match eat_space!(i, statement) {
            IResult::Error(e) => {
                return IResult::Error(e);
            }
            IResult::Incomplete(i) => {
                return IResult::Incomplete(i);
            }
            IResult::Done(rest, stmt) => {
                out.push(stmt);
                i = rest;
                if i.input_len() == 0 {
                    break;
                }
            }
        }
    }
    return IResult::Done(i, out);
}

// named!(pub parse( Span ) -> Vec<Statement>, many1!());

#[cfg(test)]
mod test {
    use super::{Statement, Expression, Value, MacroDef, SelectDef, CallDef};
    use super::{number, symbol, parse, field_value, selector_value, tuple,
                grouped_expression, list_value};
    use super::{copy_expression, macro_expression, select_expression};
    use super::{format_expression, call_expression, expression};
    use super::{expression_statement, let_statement, import_statement, statement};
    use ast::*;
    use nom_locate::LocatedSpan;

    use nom::{Needed, IResult};

    #[test]
    fn test_symbol_parsing() {
        assert_eq!(symbol(LocatedSpan::new("foo")),
               IResult::Done(LocatedSpan{fragment: "", offset: 3, line: 1},
               Value::Symbol(value_node!("foo".to_string(), 1, 1))) );
        assert_eq!(symbol(LocatedSpan::new("foo-bar")),
               IResult::Done(LocatedSpan{fragment: "", offset: 7, line: 1},
               Value::Symbol(value_node!("foo-bar".to_string(), 1, 1))) );
        assert_eq!(symbol(LocatedSpan::new("foo_bar")),
               IResult::Done(LocatedSpan{fragment: "", offset: 7, line: 1},
               Value::Symbol(value_node!("foo_bar".to_string(), 1, 1))) );
    }

    #[test]
    fn test_selector_parsing() {
        assert_eq!(selector_value(LocatedSpan::new("foo.")),
            IResult::Incomplete(Needed::Unknown)
        );
        assert_eq!(selector_value(LocatedSpan::new("foo.bar ")),
          IResult::Done(LocatedSpan{fragment: "", offset: 8, line: 1},
          Value::Selector(make_selector!(make_expr!("foo".to_string(), 1, 1) => [
                                          Token::new("bar", 1, 5)] =>
                                        1, 0)))
        );
        assert_eq!(selector_value(LocatedSpan::new("foo.0 ")),
          IResult::Done(LocatedSpan{fragment: "", offset: 6, line: 1},
          Value::Selector(make_selector!(make_expr!("foo".to_string(), 1, 1) => [
                                          Token::new("0", 1, 5)] =>
                                        1, 0)))
        );
        assert_eq!(selector_value(LocatedSpan::new("foo.bar;")),
            IResult::Done(LocatedSpan{fragment: ";", offset: 7, line: 1},
            Value::Selector(make_selector!(make_expr!("foo", 1, 1) =>
                                            [
                                               Token{fragment:"bar".to_string(), pos: Position::new(1, 5)}
                                            ] =>
                                            1, 0)))
        );

        assert_eq!(selector_value(LocatedSpan::new("({foo=1}).foo ")),
            IResult::Done(LocatedSpan{fragment: "", offset: 14, line: 1},
            Value::Selector(make_selector!(Expression::Grouped(Box::new(Expression::Simple(
                Value::Tuple(value_node!(
                    vec![(make_tok!("foo", 1, 3), Expression::Simple(Value::Int(Positioned::new(1, 1, 7))))],
                    1, 3))
                ))) => [ make_tok!("foo", 1, 11) ] => 1, 0)
        )));
    }

    #[test]
    fn test_statement_parse() {
        let mut stmt = "import \"foo\" as foo;";
        let input = LocatedSpan::new(stmt);
        assert_eq!(statement(input),
               IResult::Done(
                   LocatedSpan{
                       offset: stmt.len(),
                       line: 1,
                       fragment: "",
                   },
                   Statement::Import(ImportDef{
                       path: Token{
                           fragment: "foo".to_string(),
                           pos: Position::new(1,8)
                       },
                       name: Token{
                          fragment: "foo".to_string(),
                          pos: Position::new(1,17),
                      }
                   })
               )
        );

        assert!(statement(LocatedSpan::new("import foo")).is_err() );

        stmt = "let foo = 1.0 ;";
        let input = LocatedSpan::new(stmt);
        assert_eq!(statement(input),
                IResult::Done(
                    LocatedSpan{
                        offset: stmt.len(),
                        line: 1,
                        fragment: "",
                    },
                    Statement::Let(LetDef{
                        name: Token{
                            fragment: "foo".to_string(),
                            pos: Position::new(1,5),
                       },
                       value: Expression::Simple(Value::Float(value_node!(1.0, 1, 11)))
        })));
        stmt = "1.0;";
        let input = LocatedSpan::new(stmt);
        assert_eq!(statement(input),
                IResult::Done(
                    LocatedSpan{
                        offset: stmt.len(),
                        line: 1,
                        fragment: "",
                    },
                Statement::Expression(
                    Expression::Simple(Value::Float(value_node!(1.0, 1, 1))))));
    }

    #[test]
    fn test_import_parse() {
        assert!(import_statement(LocatedSpan::new("import")).is_incomplete());
        assert!(import_statement(LocatedSpan::new("import \"foo\"")).is_incomplete());
        assert!(import_statement(LocatedSpan::new("import \"foo\" as")).is_incomplete());
        assert!(import_statement(LocatedSpan::new("import \"foo\" as foo")).is_incomplete());

        let import_stmt = "import \"foo\" as foo;";
        assert_eq!(import_statement(LocatedSpan::new(import_stmt)),
               IResult::Done(LocatedSpan{
                        fragment: "",
                        line: 1,
                        offset: import_stmt.len(),
                    },
                    Statement::Import(ImportDef{
                        path: Token{
                                fragment: "foo".to_string(),
                                pos: Position::new(1, 8),
                            },
                        name: Token{
                                fragment: "foo".to_string(),
                                pos: Position::new(1,17),
                            }
                    })
               )
    );
    }

    #[test]
    fn test_let_statement_parse() {
        assert!(let_statement(LocatedSpan::new("foo")).is_err() );
        assert!(let_statement(LocatedSpan::new("let \"foo\"")).is_err() );
        assert!(let_statement(LocatedSpan::new("let 1")).is_err() );
        assert!(let_statement(LocatedSpan::new("let")).is_incomplete() );
        assert!(let_statement(LocatedSpan::new("let foo")).is_incomplete() );
        assert!(let_statement(LocatedSpan::new("let foo =")).is_incomplete() );
        assert!(let_statement(LocatedSpan::new("let foo = ")).is_incomplete() );
        assert!(let_statement(LocatedSpan::new("let foo = 1")).is_incomplete() );

        let mut let_stmt = "let foo = 1.0 ;";
        assert_eq!(let_statement(LocatedSpan::new(let_stmt)),
               IResult::Done(LocatedSpan{
                    fragment: "",
                    offset: let_stmt.len(),
                    line: 1,
                },
                Statement::Let(LetDef{name: Token{
                    fragment: "foo".to_string(),
                    pos: Position::new(1,5),
                    },
                    value: Expression::Simple(Value::Float(value_node!(1.0, 1, 11)))
                })));

        let_stmt = "let foo= 1.0;";
        assert_eq!(let_statement(LocatedSpan::new(let_stmt)),
               IResult::Done(LocatedSpan{
                    fragment: "",
                    offset: let_stmt.len(),
                    line: 1,
                },
                Statement::Let(LetDef{name: Token{
                    fragment: "foo".to_string(),
                    pos: Position::new(1,5),
                },
                value: Expression::Simple(Value::Float(value_node!(1.0, 1, 10)))})));
        let_stmt = "let foo =1.0;";
        assert_eq!(let_statement(LocatedSpan::new(let_stmt)),
               IResult::Done(LocatedSpan{
                    fragment: "",
                    offset: let_stmt.len(),
                    line: 1,
                },
                Statement::Let(LetDef{name: Token{
                    fragment: "foo".to_string(),
                    pos: Position::new(1,5),
                },
                value: Expression::Simple(Value::Float(value_node!(1.0, 1, 10)))})));
    }

    #[test]
    fn test_expression_statement_parse() {
        assert!(expression_statement(LocatedSpan::new("foo")).is_incomplete() );
        assert_eq!(expression_statement(LocatedSpan::new("1.0;")),
               IResult::Done(LocatedSpan{
                   fragment: "",
                   offset: 4,
                   line: 1,
               },
                  Statement::Expression(
                      Expression::Simple(Value::Float(value_node!(1.0, 1, 1))))));
        assert_eq!(expression_statement(LocatedSpan::new("1.0 ;")),
               IResult::Done(LocatedSpan {
                  fragment: "",
                  offset: 5,
                  line: 1,
                  },
                  Statement::Expression(
                      Expression::Simple(Value::Float(value_node!(1.0, 1, 1))))));
        assert_eq!(expression_statement(LocatedSpan::new(" 1.0;")),
               IResult::Done(LocatedSpan {
                  fragment: "",
                  offset: 5,
                  line: 1,
                  },
                  Statement::Expression(
                      Expression::Simple(Value::Float(value_node!(1.0, 1, 2))))));
        assert_eq!(expression_statement(LocatedSpan::new("foo;")),
               IResult::Done(LocatedSpan {
                  fragment: "",
                  offset: 4,
                  line: 1,
                  },
                  Statement::Expression(
                      Expression::Simple(Value::Selector(make_selector!(make_expr!("foo", 1, 1), 1, 0))))));
        assert_eq!(expression_statement(LocatedSpan::new("foo ;")),
               IResult::Done(LocatedSpan {
                  fragment: "",
                  offset: 5,
                  line: 1,
                  },
                  Statement::Expression(
                      Expression::Simple(Value::Selector(make_selector!(make_expr!("foo", 1, 2), 1, 0))))));
        assert_eq!(expression_statement(LocatedSpan::new(" foo;")),
               IResult::Done(LocatedSpan {
                  fragment: "",
                  offset: 5,
                  line: 1,
                  },
                  Statement::Expression(
                      Expression::Simple(Value::Selector(make_selector!(make_expr!("foo", 1, 2), 1, 1))))));
        assert_eq!(expression_statement(LocatedSpan::new("\"foo\";")),
               IResult::Done(LocatedSpan {
                  fragment: "",
                  offset: 6,
                  line: 1,
                  },
                  Statement::Expression(
                      Expression::Simple(Value::String(value_node!("foo".to_string(), 1, 1))))));
        assert_eq!(expression_statement(LocatedSpan::new("\"foo\" ;")),
               IResult::Done(LocatedSpan {
                  fragment: "",
                  offset: 7,
                  line: 1,
                  },
                  Statement::Expression(
                      Expression::Simple(Value::String(value_node!("foo".to_string(), 1, 1))))));
        assert_eq!(expression_statement(LocatedSpan::new(" \"foo\";")),
               IResult::Done(LocatedSpan {
                  fragment: "",
                  offset: 7,
                  line: 1,
                  },
                  Statement::Expression(
                     Expression::Simple(Value::String(value_node!("foo".to_string(), 1, 2))))));
    }

    #[test]
    fn test_expression_parse() {
        assert_eq!(expression(LocatedSpan::new("1")),
              IResult::Done(LocatedSpan {
                 fragment: "",
                 offset: 1,
                 line: 1,
                 },
              Expression::Simple(Value::Int(value_node!(1, 1, 1)))));
        assert_eq!(expression(LocatedSpan::new("foo ")),
              IResult::Done(LocatedSpan {
                 fragment: "",
                 offset: 4,
                 line: 1,
                 },
              Expression::Simple(Value::Selector(make_selector!(make_expr!("foo", 1, 1), 1, 0)))));
        assert_eq!(expression(LocatedSpan::new("foo.bar ")),
               IResult::Done(LocatedSpan {
                  fragment: "",
                  offset: 8,
                  line: 1,
                  },
               Expression::Simple(Value::Selector(make_selector!(make_expr!("foo", 1, 1) =>
                                                                 [ Token::new("bar", 1, 5) ] =>
                                                                 1, 0)))));
        assert_eq!(expression(LocatedSpan::new("1 + 1")),
               IResult::Done(LocatedSpan {
                  fragment: "",
                  offset: 5,
                  line: 1,
                  },
                Expression::Binary(BinaryOpDef{
                    kind: BinaryExprType::Add,
                    left: Value::Int(value_node!(1, 1, 1)),
                    right: Box::new(Expression::Simple(Value::Int(value_node!(1, 1, 5)))),
                    pos: Position::new( 1, 0 ),
                })));
        assert_eq!(expression(LocatedSpan::new("1 - 1")),
               IResult::Done(LocatedSpan {
                  fragment: "",
                  offset: 5,
                  line: 1,
                  },
                Expression::Binary(BinaryOpDef{
                    kind: BinaryExprType::Sub,
                    left: Value::Int(value_node!(1, 1, 1)),
                    right: Box::new(Expression::Simple(Value::Int(value_node!(1, 1, 5)))),
                    pos: Position::new(1, 0),
                })));
        assert_eq!(expression(LocatedSpan::new("1 * 1")),
                IResult::Done(LocatedSpan {
                  fragment: "",
                  offset: 5,
                  line: 1,
                  },
                Expression::Binary(BinaryOpDef{
                    kind: BinaryExprType::Mul,
                    left: Value::Int(value_node!(1, 1, 1)),
                    right: Box::new(Expression::Simple(Value::Int(value_node!(1, 1, 5)))),
                    pos: Position::new(1, 0),
                })));
        assert_eq!(expression(LocatedSpan::new("1 / 1")),
                IResult::Done(LocatedSpan {
                  fragment: "",
                  offset: 5,
                  line: 1,
                  },
                Expression::Binary(BinaryOpDef{
                    kind: BinaryExprType::Div,
                    left: Value::Int(value_node!(1, 1, 1)),
                    right: Box::new(Expression::Simple(Value::Int(value_node!(1, 1, 5)))),
                    pos: Position::new(1, 0),
                })));

        assert_eq!(expression(LocatedSpan::new("1+1")),
                IResult::Done(LocatedSpan {
                  fragment: "",
                  offset: 3,
                  line: 1,
                  },
                Expression::Binary(BinaryOpDef{
                    kind: BinaryExprType::Add,
                    left: Value::Int(value_node!(1, 1, 1)),
                    right: Box::new(Expression::Simple(Value::Int(value_node!(1, 1, 3)))),
                    pos: Position::new(1, 0),
                })));
        assert_eq!(expression(LocatedSpan::new("1-1")),
                IResult::Done(LocatedSpan {
                  fragment: "",
                  offset: 3,
                  line: 1,
                  },
                Expression::Binary(BinaryOpDef{
                    kind: BinaryExprType::Sub,
                    left: Value::Int(value_node!(1, 1, 1)),
                    right: Box::new(Expression::Simple(Value::Int(value_node!(1, 1, 3)))),
                    pos: Position::new(1, 0),
                })));
        assert_eq!(expression(LocatedSpan::new("1*1")),
                IResult::Done(LocatedSpan {
                  fragment: "",
                  offset: 3,
                  line: 1,
                  },
                Expression::Binary(BinaryOpDef{
                    kind: BinaryExprType::Mul,
                    left: Value::Int(value_node!(1, 1, 1)),
                    right: Box::new(Expression::Simple(Value::Int(value_node!(1, 1, 3)))),
                    pos: Position::new(1, 0),
                })));
        assert_eq!(expression(LocatedSpan::new("1/1")),
                IResult::Done(LocatedSpan {
                  fragment: "",
                  offset: 3,
                  line: 1,
                  },
                Expression::Binary(BinaryOpDef{
                    kind: BinaryExprType::Div,
                    left: Value::Int(value_node!(1, 1, 1)),
                    right: Box::new(Expression::Simple(Value::Int(value_node!(1, 1, 3)))),
                    pos: Position::new(1, 0),
                })));
        let macro_expr = "macro (arg1, arg2) => { foo = arg1 }";
        assert_eq!(expression(LocatedSpan::new(macro_expr)),
                IResult::Done(LocatedSpan {
                  fragment: "",
                  offset: macro_expr.len(),
                  line: 1,
                  },
                Expression::Macro(MacroDef{
                    argdefs: vec![
                        value_node!("arg1".to_string(), 1, 8),
                        value_node!("arg2".to_string(), 1, 14),
                    ],
                    fields: vec![
                        (Token::new("foo", 1, 25),
                         Expression::Simple(Value::Selector(make_selector!(make_expr!("arg1", 1, 31), 1, 30)))),
                    ],
                    pos: Position::new(1, 0),
                })
               )
    );
        let select_expr = "select foo, 1, { foo = 2 }";
        assert_eq!(expression(LocatedSpan::new(select_expr)),
            IResult::Done(LocatedSpan {
              fragment: "",
              offset: select_expr.len(),
              line: 1,
              },
            Expression::Select(SelectDef{
                val: Box::new(Expression::Simple(Value::Selector(make_selector!(make_expr!("foo", 1, 8), 1, 7)))),
                default: Box::new(Expression::Simple(Value::Int(value_node!(1, 1, 13)))),
                tuple: vec![
                    (Token::new("foo", 1, 18),
                     Expression::Simple(Value::Int(value_node!(2, 1, 24))))
                ],
                pos: Position::new(1, 0),
            })
           )
    );
        let call_expr = "foo.bar (1, \"foo\")";
        assert_eq!(expression(LocatedSpan::new(call_expr)),
            IResult::Done(LocatedSpan {
               fragment: "",
               offset: call_expr.len(),
               line: 1,
               },
            Expression::Call(CallDef{
                macroref: make_selector!(make_expr!("foo", 1, 1)  =>
                                         [ Token::new("bar", 1, 5) ] =>
                                         1, 0),
                arglist: vec![
                    Expression::Simple(Value::Int(value_node!(1, 1, 10))),
                    Expression::Simple(Value::String(value_node!("foo".to_string(), 1, 13))),
                ],
                pos: Position::new(1, 0),
            })
           )
    );
        assert_eq!(expression(LocatedSpan::new("(1 + 1)")),
            IResult::Done(LocatedSpan {
              fragment: "",
              offset: 7,
              line: 1,
              },
            Expression::Grouped(
                Box::new(
                    Expression::Binary(
                        BinaryOpDef{
                            kind: BinaryExprType::Add,
                            left: Value::Int(value_node!(1, 1, 2)),
                            right: Box::new(Expression::Simple(Value::Int(value_node!(1, 1, 6)))),
                            pos: Position::new(1, 1),
                        }
                    )
                )
            )
        )
    );
        assert_eq!(expression(LocatedSpan::new("[1, 1]")),
            IResult::Done(LocatedSpan{fragment: "", offset: 6, line: 1},
                Expression::Simple(Value::List(
                    ListDef{
                        elems: vec![
                            Expression::Simple(Value::Int(value_node!(1, 1, 2))),
                            Expression::Simple(Value::Int(value_node!(1, 1, 5))),
                        ],
                        pos: Position::new(1, 1),
                    }
                )
            )
        ));
    }

    #[test]
    fn test_format_parse() {
        assert!(format_expression(LocatedSpan::new("\"foo")).is_err() );
        assert!(format_expression(LocatedSpan::new("\"foo\"")).is_incomplete() );
        assert!(format_expression(LocatedSpan::new("\"foo\" %")).is_incomplete() );
        assert!(format_expression(LocatedSpan::new("\"foo\" % (1, 2")).is_incomplete() );

        let mut fmt_expr = "\"foo @ @\" % (1, 2)";
        assert_eq!(format_expression(LocatedSpan::new(fmt_expr)),
               IResult::Done(LocatedSpan{
                        fragment: "",
                        offset: fmt_expr.len(),
                        line: 1
                    },
                    Expression::Format(
                        FormatDef{
                            template: "foo @ @".to_string(),
                            args: vec![Expression::Simple(Value::Int(value_node!(1, 1, 14))),
                                       Expression::Simple(Value::Int(value_node!(2, 1, 17)))],
                            pos: Position::new(1, 1),
                        }
                    )
               )
        );

        fmt_expr = "\"foo @ @\"%(1, 2)";
        assert_eq!(format_expression(LocatedSpan::new(fmt_expr)),
            IResult::Done(LocatedSpan{
                     fragment: "",
                     offset: fmt_expr.len(),
                     line: 1,
                },
                Expression::Format(
                    FormatDef{
                        template: "foo @ @".to_string(),
                        args: vec![Expression::Simple(Value::Int(value_node!(1, 1, 12))),
                                   Expression::Simple(Value::Int(value_node!(2, 1, 15)))],
                        pos: Position::new(1, 1),
                    }
                )
            )
        );
    }

    #[test]
    fn test_call_parse() {
        assert!(call_expression(LocatedSpan::new("foo")).is_incomplete() );
        assert!(call_expression(LocatedSpan::new("foo (")).is_incomplete() );
        assert!(call_expression(LocatedSpan::new("foo (1")).is_incomplete() );
        assert!(call_expression(LocatedSpan::new("foo (1,")).is_incomplete() );
        assert!(call_expression(LocatedSpan::new("foo (1,2")).is_incomplete() );

        let mut copy_expr = "foo (1, \"foo\")";
        assert_eq!(call_expression(LocatedSpan::new(copy_expr)),
               IResult::Done(
                    LocatedSpan{
                        fragment: "",
                        line: 1,
                        offset: copy_expr.len(),
                    },
                    Expression::Call(CallDef{
                        macroref: make_selector!(make_expr!("foo")),
                        arglist: vec![
                            Expression::Simple(Value::Int(value_node!(1, 1, 6))),
                            Expression::Simple(Value::String(value_node!("foo".to_string(), 1, 9))),
                        ],
                        pos: Position::new(1, 0),
                    })
               )
        );

        copy_expr = "foo.bar (1, \"foo\")";
        assert_eq!(call_expression(LocatedSpan::new(copy_expr)),
               IResult::Done(
                    LocatedSpan{
                        fragment: "",
                        line: 1,
                        offset: copy_expr.len(),
                    },
                    Expression::Call(CallDef{
                        macroref: make_selector!(make_expr!("foo") => [ make_tok!("bar", 1, 5) ] => 1, 0),
                        arglist: vec![
                            Expression::Simple(Value::Int(value_node!(1, 1, 10))),
                            Expression::Simple(Value::String(value_node!("foo".to_string(), 1, 13))),
                        ],
                        pos: Position::new(1, 0),
                    })
               )
        );
    }

    #[test]
    fn test_select_parse() {
        assert!(select_expression(LocatedSpan::new("select")).is_incomplete());
        assert!(select_expression(LocatedSpan::new("select foo")).is_incomplete());
        assert!(select_expression(LocatedSpan::new("select foo, 1")).is_incomplete());
        assert!(select_expression(LocatedSpan::new("select foo, 1, {")).is_incomplete());

        let select_expr = "select foo, 1, { foo = 2 }";
        assert_eq!(select_expression(LocatedSpan::new(select_expr)),
                IResult::Done(LocatedSpan {
                  fragment: "",
                  offset: select_expr.len(),
                  line: 1,
                  },
                Expression::Select(SelectDef{
                    val: Box::new(Expression::Simple(Value::Selector(make_selector!(make_expr!("foo", 1, 8), 1, 7)))),
                    default: Box::new(Expression::Simple(Value::Int(value_node!(1, 1, 13)))),
                    tuple: vec![
                        (Token::new("foo", 1, 18), Expression::Simple(Value::Int(value_node!(2, 1, 24))))
                    ],
                    pos: Position::new(1, 0),
                })
               )
    );
    }

    #[test]
    fn test_macro_expression_parsing() {
        assert!(macro_expression(LocatedSpan::new("foo")).is_err() );
        assert!(macro_expression(LocatedSpan::new("macro \"foo\"")).is_err() );
        assert!(macro_expression(LocatedSpan::new("macro 1")).is_err() );
        assert!(macro_expression(LocatedSpan::new("macro")).is_incomplete() );
        assert!(macro_expression(LocatedSpan::new("macro (")).is_incomplete() );
        assert!(macro_expression(LocatedSpan::new("macro (arg")).is_incomplete() );
        assert!(macro_expression(LocatedSpan::new("macro (arg, arg2")).is_incomplete() );
        assert!(macro_expression(LocatedSpan::new("macro (arg1, arg2) =>")).is_incomplete() );
        assert!(macro_expression(LocatedSpan::new("macro (arg1, arg2) => {")).is_incomplete() );
        assert!(macro_expression(LocatedSpan::new("macro (arg1, arg2) => { foo")).is_incomplete() );
        assert!(macro_expression(LocatedSpan::new("macro (arg1, arg2) => { foo =")).is_incomplete() );

        let macro_expr = "macro (arg1, arg2) => {foo=1,bar=2}";
        assert_eq!(macro_expression(LocatedSpan::new(macro_expr)),
               IResult::Done(
                    LocatedSpan{
                        fragment: "",
                        offset: macro_expr.len(),
                        line: 1
                    },
                    Expression::Macro(MacroDef{
                        argdefs: vec![value_node!("arg1".to_string(), 1, 8),
                                      value_node!("arg2".to_string(), 1, 14)],
                        fields: vec![(Token::new("foo", 1, 24), Expression::Simple(Value::Int(value_node!(1, 1, 28)))),
                                     (Token::new("bar", 1, 30), Expression::Simple(Value::Int(value_node!(2, 1, 34))))
                        ],
                        pos: Position::new(1, 0),
                    })
               )
    );
    }

    #[test]
    fn test_copy_parse() {
        assert!(copy_expression(LocatedSpan::new("{}")).is_err() );
        assert!(copy_expression(LocatedSpan::new("foo")).is_incomplete() );
        assert!(copy_expression(LocatedSpan::new("foo{")).is_incomplete() );

        let mut copy_expr = "foo{}";
        assert_eq!(copy_expression(LocatedSpan::new(copy_expr)),
               IResult::Done(
                    LocatedSpan{
                        fragment: "",
                        offset: copy_expr.len(),
                        line: 1
                    },
                    Expression::Copy(CopyDef{
                        selector: make_selector!(make_expr!("foo")),
                        fields: Vec::new(),
                        pos: Position::new(1, 0),
                    })
               )
        );

        copy_expr = "foo{bar=1}";
        assert_eq!(copy_expression(LocatedSpan::new(copy_expr)),
            IResult::Done(
                LocatedSpan{
                    fragment: "",
                    offset: copy_expr.len(),
                    line: 1
                },
                Expression::Copy(CopyDef{
                    selector: make_selector!(make_expr!("foo")),
                    fields: vec![(Token::new("bar", 1, 5),
                                  Expression::Simple(Value::Int(value_node!(1, 1, 9))))],
                    pos: Position::new(1, 0),
                })
            )
        );
    }

    #[test]
    fn test_grouped_expression_parse() {
        assert!(grouped_expression(LocatedSpan::new("foo")).is_err() );
        assert!(grouped_expression(LocatedSpan::new("(foo")).is_incomplete() );
        assert_eq!(grouped_expression(LocatedSpan::new("(foo)")),
            IResult::Done(LocatedSpan{fragment: "", offset: 5, line: 1},
                          Expression::Grouped(
                              Box::new(
                                  Expression::Simple(
                                      Value::Selector(make_selector!(make_expr!("foo", 1, 2), 1, 1))))))
    );
        assert_eq!(grouped_expression(LocatedSpan::new("(1 + 1)")),
            IResult::Done(LocatedSpan{fragment: "", offset: 7, line: 1},
                          Expression::Grouped(
                              Box::new(
                                  Expression::Binary(
                                      BinaryOpDef{
                                          kind: BinaryExprType::Add,
                                          left: Value::Int(value_node!(1, 1, 2)),
                                          right: Box::new(Expression::Simple(
                                              Value::Int(value_node!(1, 1, 6)))),
                                          pos: Position::new(1, 1),
                                      }
                                  )
                              )
                          )
            )
    );
    }

    #[test]
    fn test_list_value_parse() {
        assert!(list_value(LocatedSpan::new("foo")).is_err() );
        assert!(list_value(LocatedSpan::new("[foo")).is_incomplete() );
       assert_eq!(list_value(LocatedSpan::new("[foo]")),
            IResult::Done(LocatedSpan{fragment: "", offset: 5, line: 1},
                          Value::List(
                              ListDef{
                                      elems: vec![
                                                Expression::Simple(Value::Selector(make_selector!(make_expr!("foo", 1, 2), 1, 1)))
                                             ],
                                      pos: Position::new(1, 1),
                                     }
                          )
            )
        );

        assert_eq!(list_value(LocatedSpan::new("[1, 1]")),
            IResult::Done(LocatedSpan{fragment: "", offset: 6, line: 1},
                Value::List(
                    ListDef{
                        elems: vec![
                            Expression::Simple(Value::Int(value_node!(1, 1, 2))),
                            Expression::Simple(Value::Int(value_node!(1, 1, 5))),
                        ],
                        pos: Position::new(1, 1),
                    }
                )
            )
        );
    }

    #[test]
    fn test_tuple_parse() {
        assert!(tuple(LocatedSpan::new("{")).is_incomplete() );
        assert!(tuple(LocatedSpan::new("{ foo")).is_incomplete() );
        assert!(tuple(LocatedSpan::new("{ foo =")).is_incomplete() );
        assert!(tuple(LocatedSpan::new("{ foo = 1")).is_incomplete() );
        assert!(tuple(LocatedSpan::new("{ foo = 1,")).is_incomplete() );
        assert!(tuple(LocatedSpan::new("{ foo = 1, bar =")).is_incomplete() );

        let mut tuple_expr = "{ }";
        assert_eq!(tuple(LocatedSpan::new(tuple_expr)),
               IResult::Done(LocatedSpan {
                  fragment: "",
                  offset: tuple_expr.len(),
                  line: 1,
                  },
                          Value::Tuple(
                              value_node!(vec![], 1, 0))));

        tuple_expr = "{ foo = 1 }";
        assert_eq!(tuple(LocatedSpan::new(tuple_expr)),
               IResult::Done(LocatedSpan {
                  fragment: "",
                  offset: tuple_expr.len(),
                  line: 1,
                  },
                          Value::Tuple(
                              value_node!(vec![
                                  (Token::new("foo", 1, 3),
                                   Expression::Simple(Value::Int(value_node!(1, 1, 9))))
                              ], 1, 0))));

        tuple_expr = "{ foo = 1, bar = \"1\" }";
        assert_eq!(tuple(LocatedSpan::new(tuple_expr)),
               IResult::Done(LocatedSpan {
                  fragment: "",
                  offset: tuple_expr.len(),
                  line: 1,
                  },
                          Value::Tuple(
                              value_node!(vec![
                                  (Token::new("foo", 1, 3),
                                   Expression::Simple(Value::Int(value_node!(1, 1, 9)))),
                                  (Token::new("bar", 1, 12),
                                   Expression::Simple(Value::String(value_node!("1".to_string(), Position::new(1, 18)))))
                              ], 1, 0))));
        tuple_expr = "{ foo = 1, bar = {} }";
        assert_eq!(tuple(LocatedSpan::new(tuple_expr)),
               IResult::Done(LocatedSpan {
                  fragment: "",
                  offset: tuple_expr.len(),
                  line: 1,
                  },
                          Value::Tuple(
                              value_node!(vec![
                                  (Token::new("foo", 1, 3),
                                   Expression::Simple(Value::Int(value_node!(1, Position::new(1, 9))))),
                                  (Token::new("bar", 1, 12),
                                   Expression::Simple(Value::Tuple(value_node!(Vec::new(), Position::new(1, 17)))))
                              ], 1, 0))));
    }

    #[test]
    fn test_field_value_parse() {
        assert!(field_value(LocatedSpan::new("foo")).is_incomplete() );
        assert!(field_value(LocatedSpan::new("foo =")).is_incomplete() );

        //assert_eq!(field_value(LocatedSpan::new("foo = 1")),
        //       IResult::Done(LocatedSpan { offset: 7, line: 1, fragment: "" },
        //       (Token::new("foo", 1, 1),
        //        Expression::Simple(Value::Int(value_node!(1, 1, 7))))) );
        //assert_eq!(field_value(LocatedSpan::new("foo = \"1\"")),
        //       IResult::Done(LocatedSpan { offset: 9, line: 1, fragment: "" },
        //       (Token::new("foo", 1, 1),
        //        Expression::Simple(Value::String(value_node!("1".to_string(), 1, 7))))) );
        //assert_eq!(field_value(LocatedSpan::new("foo = bar")),
        //       IResult::Done(LocatedSpan { offset: 9, line: 1, fragment: "" },
        //       (Token::new("foo", 1, 1),
        //        Expression::Simple(Value::Symbol(value_node!("bar".to_string(), 1, 7))))) );
        //assert_eq!(field_value(LocatedSpan::new("foo = bar ")),
        //       IResult::Done(LocatedSpan { offset: 10, line: 1, fragment: "" },
        //       (Token::new("foo", 1, 1),
        //        Expression::Simple(Value::Symbol(value_node!("bar".to_string(), 1, 7))))) );
        //assert_eq!(field_value(LocatedSpan::new("foo = bar.baz ")),
        //       IResult::Done(LocatedSpan { offset: 14, line: 1, fragment: "" },
        //       (Token::new("foo", 1, 1),
        //       Expression::Simple(Value::Selector(make_selector!(make_expr!("bar") => "baz"))))));
    }

    #[test]
    fn test_number_parsing() {
        assert!(number(LocatedSpan::new(".")).is_err() );
        assert!(number(LocatedSpan::new(". ")).is_err() );
        assert_eq!(number(LocatedSpan::new("1.0")),
               IResult::Done(LocatedSpan{fragment: "", offset: 3, line: 1},
               Value::Float(value_node!(1.0, 1, 1))) );
        assert_eq!(number(LocatedSpan::new("1.")),
               IResult::Done(LocatedSpan{fragment: "", offset: 2, line: 1},
               Value::Float(value_node!(1.0, 1, 1))) );
        assert_eq!(number(LocatedSpan::new("1")),
               IResult::Done(LocatedSpan{fragment: "", offset: 1, line: 1},
               Value::Int(value_node!(1, 1, 1))) );
        assert_eq!(number(LocatedSpan::new(".1")),
               IResult::Done(LocatedSpan{fragment: "", offset: 2, line: 1},
               Value::Float(value_node!(0.1, 1, 1))) );
    }

    #[test]
    fn test_parse() {
        let bad_input = LocatedSpan::new("import mylib as lib;");
        let bad_result = parse(bad_input);
        assert!(bad_result.is_err() );

        // Valid parsing tree
        let input = LocatedSpan::new("import \"mylib\" as lib;let foo = 1;1+1;");
        let result = parse(input);
        assert!(result.is_done() );
        let tpl = result.unwrap();
        assert_eq!(tpl.0.fragment, "");
        assert_eq!(tpl.1,
               vec![
                   Statement::Import(ImportDef{
                       path: Token{
                           fragment: "mylib".to_string(),
                           pos: Position::new(1, 8),
                       },
                       name: Token{
                           fragment: "lib".to_string(),
                           pos: Position::new(1, 19),
                       }
                   }),
                   Statement::Let(LetDef{
                       name: Token{
                           fragment: "foo".to_string(),
                           pos: Position::new(1, 27),
                       },
                       value: Expression::Simple(Value::Int(value_node!(1, 1, 33)))
                   }),
                   Statement::Expression(
                       Expression::Binary(
                           BinaryOpDef{
                               kind: BinaryExprType::Add,
                               left: Value::Int(value_node!(1, 1, 35)),
                               right: Box::new(Expression::Simple(Value::Int(value_node!(1, 1, 37)))),
                               pos: Position::new(1, 34),
                           })
                   )
               ]);
    }
}
