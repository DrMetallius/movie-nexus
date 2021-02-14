use nom::{
    character::complete::{char, digit1},
    bytes::complete::{tag, take_while},
    branch::alt,
    IResult,
    combinator::{map, map_res, opt, value},
    error::{context, ContextError, FromExternalError, ParseError},
    multi::{many0_count, separated_list1},
    sequence::{preceded, tuple},
};
use std::num::ParseIntError;

pub fn parse_range<'a, E>(input: &'a str) -> IResult<&'a str, Vec<ByteRange>, E>
    where E: ParseError<&'a str> + ContextError<&'a str> + FromExternalError<&'a str, ParseIntError> {
    map(
        tuple((
            byte_range_set_start,
            separated_list1(
                tuple((sp, char(','), sp)),
                alt((byte_range_spec, suffix_byte_range_spec)),
            )
        )),
        |(_, ranges)| ranges,
    )(input)
}

fn byte_range_set_start<'a, E: ParseError<&'a str> + ContextError<&'a str>>(
    input: &'a str,
) -> IResult<&'a str, (), E> {
    context(
        "byte-range-set-start",
        value((), tuple((tag("bytes="), many0_count(preceded(char(','), sp))))),
    )(input)
}

fn sp<'a, E: ParseError<&'a str>>(input: &'a str) -> IResult<&'a str, &'a str, E> {
    take_while(|ch| ch == ' ' || ch == '\t')(input)
}

fn byte_range_spec<'a, E: ParseError<&'a str> + ContextError<&'a str> + FromExternalError<&'a str, ParseIntError>>(
    input: &'a str,
) -> IResult<&'a str, ByteRange, E> {
    context(
        "byte-range-spec",
        map(
            tuple((
                map_res(digit1, |s: &str| s.parse()),
                char('-'),
                opt(map_res(digit1, |s: &str| s.parse()))
            )),
            |(start, _, end)| {
                match end {
                    None => ByteRange::StartingAt(start),
                    Some(end) => ByteRange::FromToIncluding(start, end)
                }
            },
        ),
    )(input)
}

fn suffix_byte_range_spec<'a, E: ParseError<&'a str> + ContextError<&'a str> + FromExternalError<&'a str, ParseIntError>>(
    input: &'a str,
) -> IResult<&'a str, ByteRange, E> {
    context(
        "suffix-range-spec",
        preceded(
            char('-'),
            map(map_res(digit1, |s: &str| s.parse()), ByteRange::Last),
        ),
    )(input)
}

#[derive(Clone, Debug)]
pub enum ByteRange {
    StartingAt(u64),
    Last(u64),
    FromToIncluding(u64, u64),
}