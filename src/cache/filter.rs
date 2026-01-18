//! Implementation of RTM filter expressions over local data.

use std::borrow::Cow;

use anyhow::{anyhow, bail};
use chrono::{NaiveTime, Utc};
use nom::{
    branch::alt,
    bytes::complete::tag,
    character::complete::{alpha1, none_of, space1},
    combinator::recognize,
    error::ParseError,
    multi::{many0, separated_list1},
    sequence::delimited,
    Mode, Parser,
};

//filter = "status:incomplete AND (dueBefore:today OR due:today)"
#[derive(PartialEq, Eq, Debug)]
/// An RTM Filter expression
pub enum RtmFilter {
    /// Match on the whether the task is completed or not.
    Complete(bool),
    /// Match on the contents of the name.
    Name(String),
    /// Match value due before a time
    DueBefore(chrono::DateTime<chrono::Utc>),
    /// Match all of the sub expressions
    And(Vec<RtmFilter>),
    /// Match all of the sub expressions
    Or(Vec<RtmFilter>),
}

impl RtmFilter {
    /*
        pub fn all() -> Self {
            RtmFilter::And(vec![])
        }

    */
    pub(crate) fn to_sqlite_where_clause(&self) -> String {
        let result = match self {
            RtmFilter::Complete(val) => {
                if *val {
                    r#"jsonb_extract(t.data, "$.completed") <> """#.into()
                } else {
                    r#"jsonb_extract(t.data, "$.completed") = """#.into()
                }
            }
            RtmFilter::Name(_s) => todo!(),
            RtmFilter::And(rtm_filters) => {
                let mut result = String::new();
                for filt in rtm_filters {
                    result.push('(');
                    result += &filt.to_sqlite_where_clause();
                    result.push_str(") AND ");
                }
                debug_assert!(result.ends_with(") AND "));
                for _ in 0..5 {
                    // Remove the last " AND "
                    result.pop().unwrap();
                }
                result
            }
            RtmFilter::Or(rtm_filters) => {
                let mut result = String::new();
                for filt in rtm_filters {
                    result.push('(');
                    result += &filt.to_sqlite_where_clause();
                    result.push_str(") OR ");
                }
                debug_assert!(result.ends_with(") OR "));
                for _ in 0..4 {
                    // Remove the last " OR "
                    result.pop().unwrap();
                }
                result
            }
            RtmFilter::DueBefore(time) => {
                format!(
                    r#"jsonb_extract(t.data, "$.due") <> "" AND jsonb_extract(t.data, "$.due") < "{}""#,
                    time.to_rfc3339()
                )
            }
        };
        result
    }
}

#[derive(Debug)]
struct Term<'a> {
    key: &'a str,
    value: Cow<'a, str>,
}
impl<'a> Term<'a> {
    fn to_filt(&self) -> Result<RtmFilter, anyhow::Error> {
        let filt = match self.key {
            "status" => match self.value.as_ref() {
                "completed" => RtmFilter::Complete(true),
                "incomplete" => RtmFilter::Complete(false),
                unknown => bail!("Unexpected status {unknown} in filter"),
            },
            "name" => RtmFilter::Name(self.value.to_string()),
            "dueBefore" => {
                if self.value.as_ref() == "today" {
                    let today = Utc::now()
                        .with_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap())
                        .unwrap();
                    RtmFilter::DueBefore(today)
                } else {
                    bail!("Unknown date format {}", self.value);
                }
            }
            "due" => {
                if self.value.as_ref() == "today" {
                    let today = Utc::now()
                        .with_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap())
                        .unwrap();
                    RtmFilter::DueBefore(today + chrono::Duration::days(1))
                } else {
                    bail!("Unknown date format {}", self.value);
                }
            }
            key => bail!("Unknown filter type {key}"),
        };
        Ok(filt)
    }
}

#[derive(Debug)]
enum SubExpr<'a> {
    Term(Term<'a>),
    And(Vec<SubExpr<'a>>),
    Or(Vec<SubExpr<'a>>),
}
impl<'a> SubExpr<'a> {
    fn to_filt(&self) -> Result<RtmFilter, anyhow::Error> {
        match self {
            SubExpr::Term(term) => term.to_filt(),
            SubExpr::And(sub_exprs) => {
                let mut filts = sub_exprs
                    .iter()
                    .map(|se| se.to_filt())
                    .collect::<Result<Vec<RtmFilter>, anyhow::Error>>()?;
                if filts.len() == 1 {
                    Ok(filts.pop().unwrap())
                } else {
                    Ok(RtmFilter::And(filts))
                }
            }
            SubExpr::Or(sub_exprs) => {
                let mut filts = sub_exprs
                    .iter()
                    .map(|se| se.to_filt())
                    .collect::<Result<Vec<RtmFilter>, anyhow::Error>>()?;
                if filts.len() == 1 {
                    Ok(filts.pop().unwrap())
                } else {
                    Ok(RtmFilter::Or(filts))
                }
            }
        }
    }
}

fn quoted(rest: &str) -> nom::IResult<&str, Cow<'_, str>> {
    delimited(tag("\""), recognize(many0(none_of("\""))), tag("\""))
        .parse(rest)
        .map(|(rest, s)| (rest, s.into()))
}

fn unquoted_arg(rest: &str) -> nom::IResult<&str, Cow<'_, str>> {
    alpha1.parse(rest).map(|(rest, s)| (rest, s.into()))
}

fn possibly_quoted(rest: &str) -> nom::IResult<&str, Cow<'_, str>> {
    alt((unquoted_arg, quoted)).parse(rest)
}

fn parse_simple(s: &str) -> nom::IResult<&str, SubExpr<'_>> {
    let (rest, k) = alpha1(s)?;
    let (rest, _) = tag(":")(rest)?;
    let (rest, v) = possibly_quoted(rest)?;

    Ok((rest, SubExpr::Term(Term { key: k, value: v })))
}

fn parse_paren(s: &str) -> nom::IResult<&str, SubExpr<'_>> {
    delimited(tag("("), parse_expr, tag(")")).parse(s)
}

fn parse_term(s: &str) -> nom::IResult<&str, SubExpr<'_>> {
    alt((parse_paren, parse_simple)).parse(s)
}

fn parse_ands(s: &str) -> nom::IResult<&str, SubExpr<'_>> {
    let (rest, parts) =
        separated_list1(delimited(space1, tag("AND"), space1), parse_term).parse(s)?;
    Ok((rest, SubExpr::And(parts)))
}

fn parse_ors(s: &str) -> nom::IResult<&str, SubExpr<'_>> {
    let (rest, parts) =
        separated_list1(delimited(space1, tag("OR"), space1), parse_term).parse(s)?;
    Ok((rest, SubExpr::Or(parts)))
}

struct ExprConsuming<F> {
    parser: F,
}

impl<'a, F> Parser<&'a str> for ExprConsuming<F>
where
    F: Parser<&'a str>,
{
    type Output = <F as Parser<&'a str>>::Output;

    type Error = <F as Parser<&'a str>>::Error;

    fn process<OM: nom::OutputMode>(
        &mut self,
        input: &'a str,
    ) -> nom::PResult<OM, &'a str, Self::Output, Self::Error> {
        let (rest, val) = self.parser.process::<OM>(input)?;
        let trimmed_rest = rest.trim();
        if !(trimmed_rest.is_empty() || trimmed_rest.starts_with(')')) {
            Err(nom::Err::Error(OM::Error::bind(|| {
                <F as Parser<&'a str>>::Error::from_error_kind(input, nom::error::ErrorKind::Eof)
            })))
        } else {
            Ok((rest, val))
        }
    }
}

// Causes the sub parser to fail if it hasn't consumed everything,
// or up to a ')'.
fn expr_consuming<'a, E: nom::error::ParseError<&'a str>, F>(
    parser: F,
) -> impl Parser<&'a str, Output = <F as Parser<&'a str>>::Output, Error = E>
where
    F: Parser<&'a str, Error = E>,
{
    ExprConsuming { parser }
}

fn parse_expr(filter: &str) -> nom::IResult<&'_ str, SubExpr<'_>> {
    alt((expr_consuming(parse_ands), expr_consuming(parse_ors))).parse(filter)
}

pub fn parse_filter(filter: &str) -> Result<RtmFilter, anyhow::Error> {
    let (rest, expr) =
        parse_expr(filter.trim()).map_err(|e| anyhow!("Error parsing filter: {e}"))?;
    if !rest.is_empty() {
        bail!("Text left after filter spec {expr:?}: {rest:?}");
    }

    expr.to_filt()
}

#[cfg(test)]
mod tests {
    use super::{parse_filter, RtmFilter};
    use RtmFilter::*;

    #[test]
    fn test_status() -> Result<(), anyhow::Error> {
        for (s, f) in &[
            ("status:completed", Complete(true)),
            ("status:incomplete", Complete(false)),
            ("name:a", Name("a".into())),
            ("name:b", Name("b".into())),
            (
                "name:a AND name:b",
                And(vec![Name("a".into()), Name("b".into())]),
            ),
            (
                "name:a AND name:b AND name:c",
                And(vec![Name("a".into()), Name("b".into()), Name("c".into())]),
            ),
            (
                "name:a OR name:b",
                Or(vec![Name("a".into()), Name("b".into())]),
            ),
            (
                "name:a OR name:b OR name:c",
                Or(vec![Name("a".into()), Name("b".into()), Name("c".into())]),
            ),
            (
                "name:a OR (name:b AND name:c)",
                Or(vec![
                    Name("a".into()),
                    And(vec![Name("b".into()), Name("c".into())]),
                ]),
            ),
        ] {
            eprintln!("Testing expr: {s}");
            assert_eq!(parse_filter(s)?, *f);
        }
        Ok(())
    }
}
