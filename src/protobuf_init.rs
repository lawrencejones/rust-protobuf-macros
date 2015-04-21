use std::clone::Clone;
use syntax::ast;
use syntax::codemap::Span;
use syntax::ext::base::{ExtCtxt, MacResult, MacEager, DummyResult};
use syntax::ext::build::AstBuilder;
use syntax::parse::{token,PResult};
use syntax::parse::parser::Parser;
use syntax::ptr::P;

#[derive(Debug)]
enum Value {
    SingleValue(P<ast::Expr>),
    MessageValue(Message),
    RepeatedValue(Vec<Value>),
}
#[derive(Debug)]
struct Field(ast::Ident, Value);
#[derive(Debug)]
struct Message(Vec<Field>);

fn parse_repeated(parser: &mut Parser) -> PResult<Vec<Value>> {
    try!(parser.expect(&token::OpenDelim(token::Bracket)));
    let mut values = Vec::new();

    while parser.token != token::CloseDelim(token::Bracket) {
        values.push(try!(parse_value(parser)));

        try!(parser.expect_one_of(
            &[token::Comma],
            &[token::CloseDelim(token::Bracket)]));
    }

    try!(parser.expect(&token::CloseDelim(token::Bracket)));

    Ok(values)
}

fn parse_compound(parser: &mut Parser) -> PResult<Value> {
    match parser.token {
        token::OpenDelim(token::Brace) => {
            Ok(Value::MessageValue(try!(parse_message(parser))))
        },
        token::OpenDelim(token::Bracket) => {
            Ok(Value::RepeatedValue(try!(parse_repeated(parser))))
        },
        _ => Err(parser.unexpected())
    }
}

fn parse_value(parser: &mut Parser) -> PResult<Value> {
    match parser.token {
        token::At => {
            try!(parser.bump());
            Ok(try!(parse_compound(parser)))
        }
        _ => Ok(Value::SingleValue(try!(parser.parse_expr_nopanic()))),
    }
}

fn parse_field(parser: &mut Parser) -> PResult<Field> {
    let ident = try!(parser.parse_ident());

    match parser.token {
        token::Colon => {
            try!(parser.bump());
            Ok(Field(ident, try!(parse_value(parser))))
        },
        token::FatArrow => {
            try!(parser.bump());
            Ok(Field(ident, try!(parse_compound(parser))))
        },
        _ => Err(parser.unexpected())
    }
}

fn parse_message(parser: &mut Parser) -> PResult<Message> {
    try!(parser.expect(&token::OpenDelim(token::Brace)));

    let mut fields = Vec::new();

    while parser.token != token::CloseDelim(token::Brace) {
        let f = try!(parse_field(parser));
        fields.push(f);

        try!(parser.expect_one_of(
            &[token::Comma],
            &[token::CloseDelim(token::Brace)]));
    }

    try!(parser.expect(&token::CloseDelim(token::Brace)));
    Ok(Message(fields))
}

fn parse_protobuf(cx: &mut ExtCtxt, tts: &[ast::TokenTree]) -> PResult<(P<ast::Expr>, Message)> {
    let mut parser = cx.new_parser_from_tts(&tts.to_vec());

    let expr = try!(parser.parse_expr_nopanic());
    try!(parser.expect(&token::Comma));
    let msg = try!(parse_message(&mut parser));
    try!(parser.expect(&token::Eof));

    return Ok((expr, msg))
}

fn emit_repeated(cx: &mut ExtCtxt, sp: Span, value: Value, parent: P<ast::Expr>) -> P<ast::Stmt> {
    let e_v = match value {
        Value::SingleValue(expr) => expr,
        Value::MessageValue(msg) => {
            let f_create = cx.ident_of("create");
            let e_msg = cx.expr_method_call(
                sp,
                parent.clone(),
                f_create,
                Vec::new()
            );
            emit_message(cx, sp, msg, e_msg)
        },
        Value::RepeatedValue(_) => panic!("Cannot nest repeated fields")
    };

    let i_value = cx.ident_of("value");
    let e_value = cx.expr_ident(sp, i_value);

    let f_push = cx.ident_of("push");
    let e_push = cx.expr_method_call(
        sp,
        parent,
        f_push,
        vec![e_value]);

    let stmts = vec![
        cx.stmt_let(sp, false, i_value, e_v),
        cx.stmt_expr(e_push)];

    let block = cx.block(sp, stmts, None);
    let e_block = cx.expr_block(block);

    cx.stmt_expr(e_block)
}

fn emit_field(cx: &mut ExtCtxt, sp: Span, field: Field, parent: P<ast::Expr>) -> P<ast::Expr> {
    let Field(field, value) = field;

    match value {
        Value::SingleValue(expr) => {
            let f_set_xxx = cx.ident_of(&format!("set_{}", field));
            cx.expr_method_call(
                sp,
                parent,
                f_set_xxx,
                vec![expr]
            )
        },
        Value::MessageValue(msg) => {
            let f_mut_xxx = cx.ident_of(&format!("mut_{}", field));
            let e_mut_xxx = cx.expr_method_call(
                sp,
                parent,
                f_mut_xxx,
                Vec::new()
            );
            emit_message(cx, sp, msg, e_mut_xxx)
        },
        Value::RepeatedValue(values) => {
            let mut stmts = Vec::new();

            let i_repeated = cx.ident_of("repeated");
            let e_repeated = cx.expr_ident(sp, i_repeated);
            let f_mut_xxx = cx.ident_of(&format!("mut_{}", field));
            let e_mut_xxx = cx.expr_method_call(
                sp,
                parent,
                f_mut_xxx,
                Vec::new()
            );
            stmts.push(cx.stmt_let(sp, true, i_repeated, e_mut_xxx));

            for v in values {
                stmts.push(emit_repeated(cx, sp, v, e_repeated.clone()))
            }
            let block = cx.block(sp, stmts, None);
            cx.expr_block(block)
        },
    }
}

fn emit_message(cx: &mut ExtCtxt, sp: Span, msg: Message, expr: P<ast::Expr>) -> P<ast::Expr>{
    let Message(fields) = msg;
    let i_msg = cx.ident_of("msg");
    let e_msg = cx.expr_ident(sp, i_msg);

    let mut stmts = Vec::new();
    stmts.push(cx.stmt_let(sp, true, i_msg, expr));

    for f in fields {
        let e_field = emit_field(cx, sp, f, e_msg.clone());
        stmts.push(cx.stmt_expr(e_field));
    }

    let block = cx.block(sp, stmts, Some(e_msg));
    cx.expr_block(block)
}

pub fn macro_protobuf_init(cx: &mut ExtCtxt, sp: Span, tts: &[ast::TokenTree]) -> Box<MacResult+'static> {
    match parse_protobuf(cx, tts) {
        Ok((expr, msg)) => {
            MacEager::expr(
                emit_message(cx, sp, msg, expr)
            )
        }
        Err(_) => {
           DummyResult::any(sp)
        }
    }
}