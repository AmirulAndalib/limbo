use winnow::error::{ContextError, ErrMode};
use winnow::stream::Stream;
use winnow::PResult;

use super::ast::{Column, Expression};
use super::operators::peek_operator;
use super::utils::expect_token;
use super::{SqlToken, SqlTokenStream};

pub(crate) fn parse_expr(input: &mut SqlTokenStream, min_precedence: u8) -> PResult<Expression> {
    let mut lhs = parse_atom(input)?;

    loop {
        let (op, precedence) = match peek_operator(input) {
            Some((op, prec)) if prec >= min_precedence => (op, prec),
            _ => break,
        };

        input.next_token(); // Consume the operator

        let rhs = parse_expr(input, precedence + 1)?;

        lhs = Expression::Binary {
            lhs: Box::new(lhs),
            op,
            rhs: Box::new(rhs),
        };
    }

    Ok(lhs)
}

fn parse_atom(input: &mut SqlTokenStream) -> PResult<Expression> {
    match input.get(0) {
        Some(SqlToken::Identifier(name)) => {
            input.next_token();
            Ok(Expression::Column(Column {
                name: std::str::from_utf8(name).unwrap().to_string(),
                alias: None,
                table_no: None,
                column_no: None,
            }))
        }
        Some(SqlToken::Literal(value)) => {
            input.next_token();
            Ok(Expression::Literal(
                std::str::from_utf8(value).unwrap().to_string(),
            ))
        }
        Some(SqlToken::ParenL) => {
            input.next_token();
            let expr = parse_expr(input, 0)?;
            expect_token(input, SqlToken::ParenR)?;
            Ok(Expression::Parenthesized(Box::new(expr)))
        }
        _ => Err(ErrMode::Backtrack(ContextError::new())),
    }
}
