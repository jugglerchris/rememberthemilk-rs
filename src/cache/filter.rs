//! Implementation of RTM filter expressions over local data.

use std::{borrow::Cow, collections::HashMap};

use anyhow::{anyhow, bail};
use chrono::{Datelike, Local, NaiveDate, TimeDelta, TimeZone};
use nom::{
    branch::alt,
    bytes::complete::{tag, tag_no_case, take_while_m_n},
    character::complete::{alpha1, digit1, multispace0, multispace1, none_of, space1},
    combinator::{fail, map, map_res, recognize},
    error::ParseError,
    multi::{many0, separated_list1},
    sequence::delimited,
    Mode, Parser,
};

/// Represent a date from an RTM filter
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum RtmDate {
    /// A time relative to the current time
    RelativeTime(chrono::TimeDelta),
    /// A day relative to today
    RelativeDay(i32),
    /// A time relative to the start of a day
    RelativeDayStart(i32),
    /// A fixed date.
    AbsoluteDate(chrono::NaiveDate),
    /// A fixed date and time
    AbsoluteDatetime(chrono::NaiveDateTime),
    /// The given time either today or tomorrow (if we've passed it today).
    NextTime(chrono::NaiveTime),
    /// A month/day indicating the next one coming.
    /// Both month and day start at 1.
    NextDate { month: u8, day: u8 },
    /// A month/day indicating the next one coming.
    /// Both month and day start at 1.  Indicates the
    /// time at the beginning of the day.
    NextDateStart { month: u8, day: u8 },
    /// A month/day indicating the next one coming.
    /// Both month and day start at 1.  Indicates the
    /// time at the end of the day.
    NextDateEnd { month: u8, day: u8 },
}

impl RtmDate {
    /// Convert to a time suitable for use in SQL statements.
    fn to_sql_date(&self, context: &FilterContext) -> String {
        match self {
            RtmDate::RelativeTime(time_delta) => (context.now + *time_delta).to_rfc3339(),
            RtmDate::RelativeDay(offset) => {
                let d = if *offset >= 0 {
                    context.now.date_naive() + chrono::Days::new(*offset as u64)
                } else {
                    context.now.date_naive() - chrono::Days::new(offset.unsigned_abs() as u64)
                };
                d.format("%Y-%m-%d").to_string()
            }
            RtmDate::RelativeDayStart(offset) => {
                let d = if *offset >= 0 {
                    context.now.date_naive() + chrono::Days::new(*offset as u64)
                } else {
                    context.now.date_naive() - chrono::Days::new(offset.unsigned_abs() as u64)
                };
                d.format("%Y-%m-%dT00:00:00").to_string()
            }
            RtmDate::NextDate { month, day } => {
                let today = context.now.date_naive();
                let m32 = *month as u32;
                let day32 = *day as u32;
                let d = if today.month() > m32 || ((today.month() == m32) && (today.day() > day32))
                {
                    NaiveDate::from_ymd_opt(today.year() + 1, m32, day32).unwrap()
                } else {
                    NaiveDate::from_ymd_opt(today.year(), m32, day32).unwrap()
                };
                d.format("%Y-%m-%d").to_string()
            }
            RtmDate::NextDateStart { month, day } => {
                let today = context.now.date_naive();
                let m32 = *month as u32;
                let day32 = *day as u32;
                let d = if today.month() > m32 || ((today.month() == m32) && (today.day() > day32))
                {
                    NaiveDate::from_ymd_opt(today.year() + 1, m32, day32).unwrap()
                } else {
                    NaiveDate::from_ymd_opt(today.year(), m32, day32).unwrap()
                };
                d.format("%Y-%m-%dT00:00:00").to_string()
            }
            RtmDate::NextDateEnd { month, day } => {
                let today = context.now.date_naive();
                let m32 = *month as u32;
                let day32 = *day as u32;
                let d = if today.month() > m32 || ((today.month() == m32) && (today.day() > day32))
                {
                    NaiveDate::from_ymd_opt(today.year() + 1, m32, day32).unwrap()
                } else {
                    NaiveDate::from_ymd_opt(today.year(), m32, day32).unwrap()
                };
                d.format("%Y-%m-%dT23:59:59").to_string()
            }
            RtmDate::NextTime(t) => {
                let n_today = context.now.with_time(*t).unwrap();
                let nt = if n_today > context.now {
                    n_today
                } else {
                    n_today + TimeDelta::days(1)
                };
                nt.to_rfc3339()
            }
            RtmDate::AbsoluteDate(d) => d.format("%Y-%m-%d").to_string(),
            RtmDate::AbsoluteDatetime(dt) => Local.from_local_datetime(dt).unwrap().to_rfc3339(),
        }
    }

    /// Convert a date to a time at the start of the day
    /// Dates with time are not affected.
    fn start_of_day(&self) -> Self {
        use RtmDate::*;
        match self {
            RelativeDay(offs) => RelativeDayStart(*offs),
            AbsoluteDate(d) => AbsoluteDatetime(d.and_hms_opt(0, 0, 0).unwrap()),
            NextDate { month, day } => NextDateStart {
                month: *month,
                day: *day,
            },
            // If we have a time, nothing changes.
            d @ (RelativeTime(_)
            | RelativeDayStart(_)
            | AbsoluteDatetime(_)
            | NextTime(_)
            | NextDateStart { .. }
            | NextDateEnd { .. }) => *d,
        }
    }

    /// Convert a date to a time at the end of the day
    /// Dates with time are not affected.
    fn end_of_day(&self) -> RtmDate {
        use RtmDate::*;
        match self {
            RelativeDay(offs) => RelativeDayStart(*offs + 1),
            AbsoluteDate(d) => AbsoluteDatetime(d.and_hms_opt(23, 59, 59).unwrap()),
            NextDate { month, day } => NextDateEnd {
                month: *month,
                day: *day,
            },
            // If we have a time, nothing changes.
            d @ (RelativeTime(_)
            | RelativeDayStart(_)
            | AbsoluteDatetime(_)
            | NextTime(_)
            | NextDateStart { .. }
            | NextDateEnd { .. }) => *d,
        }
    }
}

#[derive(PartialEq, Eq, Debug)]
/// An RTM Filter expression
pub enum RtmFilter {
    /// Match on the whether the task is completed or not.
    Complete(bool),
    /// Match on the contents of the name.
    Name(String),
    /// Match on the contents of the name.
    List(String),
    /// Match on a tag.
    Tag(String),
    /// Match item with no due date
    DueNever,
    /// Match value due before a time
    DueBefore(RtmDate),
    /// Match value due before a time
    DueWithin(RtmDate, RtmDate),
    /// Start within a day, or no start date
    Start(Option<chrono::NaiveDate>),
    /// Start before a certain time
    StartBefore(RtmDate),
    /// Match all of the sub expressions
    And(Vec<RtmFilter>),
    /// Match all of the sub expressions
    Or(Vec<RtmFilter>),
    /// Negated filter
    Not(Box<RtmFilter>),
    /// Given by
    GivenBy(String),
}

/// Context required when interpreting filters
#[derive(Default)]
pub struct FilterContext {
    /// Mapping from list names to list id
    pub lists_name_to_id: HashMap<String, String>,
    /// The current time
    pub now: chrono::DateTime<Local>,
}

impl RtmFilter {
    /// Return a SQL expression for a where clause, and some values to bind.
    /// The values should correspond to '?' markers in the expression.
    pub(crate) fn to_sqlite_where_clause(
        &self,
        context: &FilterContext,
    ) -> Result<(String, Vec<String>), anyhow::Error> {
        let result = match self {
            RtmFilter::Complete(val) => {
                if *val {
                    (r#"jsonb_extract(t.data, "$.completed") <> """#.to_string(), Vec::new())
                } else {
                    (r#"jsonb_extract(t.data, "$.completed") = """#.to_string(), Vec::new())
                }
            }
            RtmFilter::Name(s) => {
                (r#"jsonb_extract(ts.data, "$.name") LIKE ?"#.into(),
                vec![format!("%{s}%")])
            }
            RtmFilter::Tag(s) => {
                (r#"EXISTS (SELECT * FROM json_each(jsonb_extract(ts.data,'$.tags')) WHERE json_each.value = ?)"#.into(), vec![s.to_string()])
            }
            RtmFilter::And(rtm_filters) => {
                let mut result = String::new();
                let mut binds = Vec::new();
                for filt in rtm_filters {
                    result.push('(');
                    let (sub_where, sub_binds) = filt.to_sqlite_where_clause(context)?;
                    result += &sub_where;
                    binds.extend(sub_binds);

                    result.push_str(") AND ");
                }
                debug_assert!(result.ends_with(") AND "));
                for _ in 0..5 {
                    // Remove the last " AND "
                    result.pop().unwrap();
                }
                (result, binds)
            }
            RtmFilter::Or(rtm_filters) => {
                let mut result = String::new();
                let mut binds = Vec::new();
                for filt in rtm_filters {
                    result.push('(');
                    let (sub_where, sub_binds) = filt.to_sqlite_where_clause(context)?;
                    result += &sub_where;
                    binds.extend(sub_binds);
                    result.push_str(") OR ");
                }
                debug_assert!(result.ends_with(") OR "));
                for _ in 0..4 {
                    // Remove the last " OR "
                    result.pop().unwrap();
                }
                (result, binds)
            }
            RtmFilter::DueNever => {
                (r#"jsonb_extract(t.data, "$.due") = """#.into(), vec![])
            }
            RtmFilter::DueBefore(time) => {
                (r#"jsonb_extract(t.data, "$.due") <> "" AND jsonb_extract(t.data, "$.due") < ?"#.into(),
                    vec![time.to_sql_date(context)])
            }
            RtmFilter::DueWithin(from, to) => {
                (format!(
                    r#"jsonb_extract(t.data, "$.due") <> "" AND jsonb_extract(t.data, "$.due") < "{}" AND jsonb_extract(t.data, "$.due") >= "{}""#,
                    to.to_sql_date(context),
                    from.to_sql_date(context)
                ), Vec::new())
            }
            RtmFilter::Start(starttime) => {
                if starttime.is_none() {
                    (r#"jsonb_extract(t.data, "$.start") == """#.into(), Vec::new())
                } else {
                    bail!("Relative start: not supported");
                }
            }
            RtmFilter::StartBefore(starttime) => {
                (r#"jsonb_extract(t.data, "$.start") <> "" AND jsonb_extract(t.data, "$.start") < ?"#.into(),
                    vec![starttime.to_sql_date(context)])
            }
            RtmFilter::Not(filt) => {
                let (clause, binds) = filt.to_sqlite_where_clause(context)?;
                (format!("NOT {}", clause), binds)
            }
            RtmFilter::List(listname) => {
                match context.lists_name_to_id.get(listname) {
                    Some(id) => {
                        let id: u64 = id.parse()?;

                        (r#"t.list_id = ?"#.into(), vec![id.to_string()])
                    }
                    None => {
                        log::warn!("Invalid list name: {listname}");
                        // Since the list doesn't exist, this is equivalent to false.
                        ("FALSE".into(), Vec::new())
                    }
                }
            }
            RtmFilter::GivenBy(_name) => {
                // This is not possible to work out from the API.
                (r#"(jsonb_extract(ts.data, "$.participants.contact") is not NULL AND json_array_length(jsonb_extract(ts.data, "$.participants.contact")) >= 1)"#.into(), Vec::new())
            }
        };
        Ok(result)
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
                let date = parse_date(&self.value)?;
                RtmFilter::DueBefore(date.start_of_day())
            }
            "dueWithin" => {
                if self.value.as_ref() == "1 day of today" {
                    RtmFilter::DueWithin(RtmDate::RelativeDay(0), RtmDate::RelativeDay(1))
                } else {
                    bail!("Unknown date format {}", self.value);
                }
            }
            "due" => {
                if self.value == "never" {
                    RtmFilter::DueNever
                } else {
                    let date = parse_date(&self.value)?;
                    RtmFilter::DueBefore(date.end_of_day())
                }
            }
            "start" => {
                let v = self.value.as_ref();
                if v == "never" {
                    RtmFilter::Start(None)
                } else {
                    bail!("Unsupported date format {}", self.value);
                }
            }
            "startBefore" => {
                let v = self.value.as_ref();
                if v == "1 hour" {
                    RtmFilter::StartBefore(RtmDate::RelativeTime(chrono::TimeDelta::hours(1)))
                } else {
                    bail!("Unsupported time format {}", self.value);
                }
            }
            "list" => RtmFilter::List(self.value.to_string()),
            "tag" => RtmFilter::Tag(self.value.to_string()),
            "givenBy" => {
                // We don't seem to have enough information in the API
                // yet.
                RtmFilter::GivenBy(self.value.to_string())
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
    Not(Box<SubExpr<'a>>),
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
            SubExpr::Not(sub_expr) => Ok(RtmFilter::Not(Box::new(sub_expr.to_filt()?))),
        }
    }
}

fn quoted(s: &str) -> nom::IResult<&str, Cow<'_, str>> {
    log::trace!("quoted({s:?})");
    let result = delimited(tag("\""), recognize(many0(none_of("\""))), tag("\""))
        .parse(s)
        .map(|(rest, s)| (rest, s.into()));
    log::trace!("quoted => {result:?}");
    result
}

fn unquoted_arg(s: &str) -> nom::IResult<&str, Cow<'_, str>> {
    log::trace!("unquoted({s:?})");
    let result = alpha1.parse(s).map(|(rest, s)| (rest, s.into()));
    log::trace!("unquoted => {result:?}");
    result
}

fn possibly_quoted(s: &str) -> nom::IResult<&str, Cow<'_, str>> {
    log::trace!("possibly_quoted({s:?})");
    let result = alt((unquoted_arg, quoted)).parse(s);
    log::trace!("possibly_quoted => {result:?}");
    result
}

fn parse_not(s: &str) -> nom::IResult<&str, SubExpr<'_>> {
    let (rest, _not) = tag_no_case("not").parse(s)?;
    let (rest, _) = space1(rest)?;
    let (rest, subexpr) = parse_term(rest)?;
    Ok((rest, SubExpr::Not(Box::new(subexpr))))
}

fn trace_parse_not(s: &str) -> nom::IResult<&str, SubExpr<'_>> {
    log::trace!("parse_not({s:?})");
    let result = parse_not(s);
    log::trace!("parse_not => {result:?}");
    result
}

fn parse_simple(s: &str) -> nom::IResult<&str, SubExpr<'_>> {
    let (rest, k) = alpha1(s)?;
    let (rest, _) = tag(":")(rest)?;
    let (rest, v) = possibly_quoted(rest)?;

    Ok((rest, SubExpr::Term(Term { key: k, value: v })))
}

fn trace_parse_simple(s: &str) -> nom::IResult<&str, SubExpr<'_>> {
    log::trace!("parse_simple({s:?})");
    let result = parse_simple(s);
    log::trace!("parse_simple => {result:?}");
    result
}

fn parse_paren(s: &str) -> nom::IResult<&str, SubExpr<'_>> {
    log::trace!("parse_paren({s:?})");
    let result = delimited(tag("("), parse_expr, tag(")")).parse(s);
    log::trace!("parse_paren => {result:?}");
    result
}

fn parse_term(s: &str) -> nom::IResult<&str, SubExpr<'_>> {
    log::trace!("parse_term({s:?})");
    let result = alt((parse_paren, trace_parse_simple, trace_parse_not)).parse(s);
    log::trace!("parse_term => {result:?}");
    result
}

fn parse_ands(s: &str) -> nom::IResult<&str, SubExpr<'_>> {
    let (rest, parts) =
        separated_list1(delimited(space1, tag_no_case("AND"), space1), parse_term).parse(s)?;
    Ok((rest, SubExpr::And(parts)))
}

fn trace_parse_ands(s: &str) -> nom::IResult<&str, SubExpr<'_>> {
    log::trace!("parse_ands({s:?})");
    let result = parse_ands(s);
    log::trace!("parse_ands => {result:?}");
    result
}

fn parse_ors(s: &str) -> nom::IResult<&str, SubExpr<'_>> {
    let (rest, parts) =
        separated_list1(delimited(space1, tag_no_case("OR"), space1), parse_term).parse(s)?;
    Ok((rest, SubExpr::Or(parts)))
}

fn trace_parse_ors(s: &str) -> nom::IResult<&str, SubExpr<'_>> {
    log::trace!("parse_ors({s:?})");
    let result = parse_ors(s);
    log::trace!("parse_ors => {result:?}");
    result
}

fn parse_date_today(s: &str) -> nom::IResult<&str, RtmDate> {
    let (rest, _) = alt((tag_no_case("today"), tag_no_case("tod"))).parse(s)?;
    Ok((rest, RtmDate::RelativeDay(0)))
}

fn parse_date_tomorrow(s: &str) -> nom::IResult<&str, RtmDate> {
    let (rest, _) = alt((tag_no_case("tomorrow"), tag_no_case("tom"))).parse(s)?;
    Ok((rest, RtmDate::RelativeDay(1)))
}

fn parse_date_yesterday(s: &str) -> nom::IResult<&str, RtmDate> {
    let (rest, _) = tag_no_case("yesterday")(s)?;
    Ok((rest, RtmDate::RelativeDay(-1)))
}

fn parse_mon(s: &str) -> nom::IResult<&str, u8> {
    alt((
        map(tag_no_case("jan"), |_| 1),
        map(tag_no_case("feb"), |_| 2),
        map(tag_no_case("mar"), |_| 3),
        map(tag_no_case("apr"), |_| 4),
        map(tag_no_case("may"), |_| 5),
        map(tag_no_case("jun"), |_| 6),
        map(tag_no_case("jul"), |_| 7),
        map(tag_no_case("aug"), |_| 8),
        map(tag_no_case("sep"), |_| 9),
        map(tag_no_case("oct"), |_| 10),
        map(tag_no_case("nov"), |_| 11),
        map(tag_no_case("dec"), |_| 12),
    ))
    .parse(s)
}

fn parse_day(s: &str) -> nom::IResult<&str, u8> {
    let (rest, v) = map_res(digit1, str::parse).parse(s)?;
    if !(1..=31).contains(&v) {
        return fail().parse(s);
    } else {
        Ok((rest, v))
    }
}

fn parse_date_day_month(s: &str) -> nom::IResult<&str, RtmDate> {
    let (rest, day) = parse_day(s)?;
    let (rest, _) = multispace1(rest)?;
    let (rest, month) = parse_mon(rest)?;
    Ok((rest, RtmDate::NextDate { month, day }))
}

fn parse_date_month_day(s: &str) -> nom::IResult<&str, RtmDate> {
    let (rest, month) = parse_mon(s)?;
    let (rest, _) = multispace1(rest)?;
    let (rest, day) = parse_day(rest)?;
    Ok((rest, RtmDate::NextDate { month, day }))
}

fn parse_date_yyyy_mm_dd(s: &str) -> nom::IResult<&str, RtmDate> {
    let (rest, y) =
        map_res(take_while_m_n(4, 4, nom::AsChar::is_dec_digit), str::parse).parse(s)?;
    let (rest, _) = tag("-")(rest)?;
    let (rest, m) =
        map_res(take_while_m_n(2, 2, nom::AsChar::is_dec_digit), str::parse).parse(rest)?;
    let (rest, _) = tag("-")(rest)?;
    let (rest, d) =
        map_res(take_while_m_n(2, 2, nom::AsChar::is_dec_digit), str::parse).parse(rest)?;

    Ok((
        rest,
        RtmDate::AbsoluteDate(NaiveDate::from_ymd_opt(y, m, d).ok_or_else(|| {
            nom::Err::Error(nom::error::Error::new(s, nom::error::ErrorKind::Fail))
        })?),
    ))
}

fn parse_date_hhmm(s: &str) -> nom::IResult<&str, RtmDate> {
    let (rest, h) =
        map_res(take_while_m_n(1, 2, nom::AsChar::is_dec_digit), str::parse).parse(s)?;
    let (rest, _) = tag(":")(rest)?;
    let (rest, m) =
        map_res(take_while_m_n(2, 2, nom::AsChar::is_dec_digit), str::parse).parse(rest)?;

    Ok((
        rest,
        RtmDate::NextTime(chrono::NaiveTime::from_hms_opt(h, m, 0).ok_or_else(|| {
            nom::Err::Error(nom::error::Error::new(s, nom::error::ErrorKind::Fail))
        })?),
    ))
}

fn parse_date_days(s: &str) -> nom::IResult<&str, RtmDate> {
    let (rest, count) = map_res(digit1, str::parse).parse(s)?;
    let (rest, _) = multispace0(rest)?;
    let (rest, _) = alt((tag_no_case("days"), tag_no_case("day"))).parse(rest)?;
    Ok((rest, RtmDate::RelativeDay(count)))
}

fn parse_date_weeks(s: &str) -> nom::IResult<&str, RtmDate> {
    let (rest, count): (_, i32) = map_res(digit1, str::parse).parse(s)?;
    let (rest, _) = multispace0(rest)?;
    let (rest, _) = alt((tag_no_case("weeks"), tag_no_case("week"))).parse(rest)?;
    Ok((rest, RtmDate::RelativeDay(count * 7)))
}

fn parse_date_mins(s: &str) -> nom::IResult<&str, RtmDate> {
    let (rest, count) = map_res(digit1, str::parse).parse(s)?;
    let (rest, _) = multispace0(rest)?;
    let (rest, _) = alt((tag_no_case("mins"), tag_no_case("min"))).parse(rest)?;
    Ok((rest, RtmDate::RelativeTime(TimeDelta::minutes(count))))
}

fn parse_date_hours(s: &str) -> nom::IResult<&str, RtmDate> {
    let (rest, count) = map_res(digit1, str::parse).parse(s)?;
    let (rest, _) = multispace0(rest)?;
    let (rest, _) = alt((tag_no_case("hours"), tag_no_case("hour"))).parse(rest)?;
    Ok((rest, RtmDate::RelativeTime(TimeDelta::hours(count))))
}

fn parse_date(s: &str) -> Result<RtmDate, anyhow::Error> {
    expr_consuming(alt((
        parse_date_today,
        parse_date_tomorrow,
        parse_date_yesterday,
        parse_date_day_month,
        parse_date_month_day,
        parse_date_yyyy_mm_dd,
        parse_date_hhmm,
        parse_date_mins,
        parse_date_hours,
        parse_date_days,
        parse_date_weeks,
    )))
    .parse(s)
    .map(|(_rest, result)| result)
    .map_err(|e| anyhow!("Unknown date format: {e}"))
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
    log::trace!("parse_expr({filter:?})");
    let result = alt((
        expr_consuming(trace_parse_ands),
        expr_consuming(trace_parse_ors),
    ))
    .parse(filter);
    log::trace!("parse_expr => {result:?}");
    result
}

pub fn parse_filter(filter: &str) -> Result<RtmFilter, anyhow::Error> {
    log::trace!("parse_filter({filter:?})");
    let (rest, expr) =
        parse_expr(filter.trim()).map_err(|e| anyhow!("Error parsing filter: {e}"))?;
    if !rest.is_empty() {
        bail!("Text left after filter spec {expr:?}: {rest:?}");
    }
    log::trace!("parse_filter: expr={expr:?}");

    let result = expr.to_filt();
    log::trace!("parse_filter: result={result:?}");
    result
}

#[cfg(test)]
mod tests {
    use crate::cache::filter::RtmDate;

    use super::{parse_date, parse_filter, RtmFilter};
    use chrono::FixedOffset;
    use RtmFilter::*;

    fn log_init() {
        let _ = env_logger::builder().is_test(true).try_init();
    }

    #[test]
    fn test_parse() -> Result<(), anyhow::Error> {
        log_init();
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
            (
                "not name:a AND name:b AND not name:c",
                And(vec![
                    Not(Box::new(Name("a".into()))),
                    Name("b".into()),
                    Not(Box::new(Name("c".into()))),
                ]),
            ),
            ("NOT name:a", Not(Box::new(Name("a".into())))),
            ("(NOT name:a)", Not(Box::new(Name("a".into())))),
            ("NOT (name:a)", Not(Box::new(Name("a".into())))),
            ("list:foo", List("foo".into())),
            (r#"list:"Hello world""#, List("Hello world".into())),
        ] {
            eprintln!("Testing expr: {s}");
            assert_eq!(parse_filter(s)?, *f);
        }
        Ok(())
    }

    #[test]
    fn test_filter_sql() -> Result<(), anyhow::Error> {
        log_init();
        let context = super::FilterContext {
            lists_name_to_id: [
                ("foo".to_string(), "12345678".to_string()),
                ("My List".to_string(), "87654321".to_string()),
            ]
            .into(),
            now: chrono::DateTime::<FixedOffset>::parse_from_rfc3339("2000-01-01T01:02:03Z")
                .unwrap()
                .into(),
        };

        for (filt_s, expected, expected_binds) in &[
            (
                "status:completed",
                r#"jsonb_extract(t.data, "$.completed") <> """#,
                &[][..],
            ),
            ("list:foo", r#"t.list_id = ?"#, &["12345678"]),
            (r#"list:"My List""#, r#"t.list_id = ?"#, &["87654321"]),
            (
                "name:foo",
                r#"jsonb_extract(ts.data, "$.name") LIKE ?"#,
                &["%foo%"],
            ),
        ] {
            let filt = parse_filter(filt_s)?;
            let (clause, binds) = filt.to_sqlite_where_clause(&context)?;
            assert_eq!(&clause, expected);
            assert_eq!(binds.len(), expected_binds.len());
            for (b, eb) in binds.iter().zip(expected_binds.iter()) {
                assert_eq!(b, eb);
            }
        }
        Ok(())
    }

    #[test]
    fn test_parse_dates() -> Result<(), anyhow::Error> {
        log_init();
        for (s, d) in &[
            ("today", RtmDate::RelativeDay(0)),
            ("tod", RtmDate::RelativeDay(0)),
            ("tomorrow", RtmDate::RelativeDay(1)),
            ("tom", RtmDate::RelativeDay(1)),
            ("yesterday", RtmDate::RelativeDay(-1)),
            ("25 Apr", RtmDate::NextDate { month: 4, day: 25 }),
            ("Apr 25", RtmDate::NextDate { month: 4, day: 25 }),
            (
                "2000-01-02",
                RtmDate::AbsoluteDate(chrono::NaiveDate::from_ymd_opt(2000, 1, 2).unwrap()),
            ),
            (
                "18:07",
                RtmDate::NextTime(chrono::NaiveTime::from_hms_opt(18, 7, 0).unwrap()),
            ),
            ("1 hour", RtmDate::RelativeTime(chrono::TimeDelta::hours(1))),
            (
                "2 hours",
                RtmDate::RelativeTime(chrono::TimeDelta::hours(2)),
            ),
            (
                "1 min",
                RtmDate::RelativeTime(chrono::TimeDelta::minutes(1)),
            ),
            (
                "2 mins",
                RtmDate::RelativeTime(chrono::TimeDelta::minutes(2)),
            ),
            ("1 day", RtmDate::RelativeDay(1)),
            ("3 days", RtmDate::RelativeDay(3)),
            ("1 week", RtmDate::RelativeDay(7)),
            ("3 weeks", RtmDate::RelativeDay(21)),
        ] {
            eprintln!("Testing date: {s:?}");
            assert_eq!(parse_date(s)?, *d);
        }
        Ok(())
    }
}
