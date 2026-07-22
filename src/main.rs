use std::{collections::BTreeMap, env, str::FromStr};

use nom::{
    IResult, Parser,
    branch::alt,
    bytes::complete::take,
    character::complete::{char, digit1},
    combinator::{opt, recognize},
    multi::{fold_many0, many0},
    sequence::{delimited, preceded},
};

enum BencodedValue {
    String(String),
    Number(i64),
    List(Vec<BencodedValue>),
    Dict(BTreeMap<String, BencodedValue>),
}

impl From<BencodedValue> for serde_json::Value {
    fn from(value: BencodedValue) -> Self {
        match value {
            BencodedValue::String(s) => serde_json::Value::String(s),
            BencodedValue::Number(x) => serde_json::Value::Number(x.into()),
            BencodedValue::List(l) => {
                serde_json::Value::Array(l.into_iter().map(|v| v.into()).collect())
            }
            BencodedValue::Dict(dict) => {
                let mut map: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
                for (key, value) in dict.into_iter() {
                    map.insert(key, value.into());
                }
                serde_json::Value::Object(map)
            }
        }
    }
}

fn parse_number<I: FromStr>(s: &str) -> IResult<&str, I> {
    recognize((opt(char('-')), digit1))
        .map_res(str::parse::<I>)
        .parse(s)
}

fn parse_string(s: &str) -> IResult<&str, BencodedValue> {
    let (rest, len) = parse_number::<usize>(s)?;
    preceded(char(':'), take(len))
        .map(String::from)
        .map(BencodedValue::String)
        .parse(rest)
}

fn parse_ben_integer(s: &str) -> IResult<&str, BencodedValue> {
    delimited(char('i'), parse_number::<i64>, char('e'))
        .map(BencodedValue::Number)
        .parse(s)
}

// The trait bounds of foldmany0 require the initializer to be an FnMut, idk why
#[allow(clippy::redundant_closure)]
fn parse_dict(s: &str) -> IResult<&str, BencodedValue> {
    delimited(
        char('d'),
        fold_many0(
            (parse_string, parse_bencoded_value),
            || BTreeMap::new(),
            |mut map, (new_key, new_val)| {
                let key_string = match new_key {
                    BencodedValue::String(s) => s,
                    _ => unreachable!(),
                };
                map.insert(key_string, new_val);
                map
            },
        ),
        char('e'),
    )
    .map(BencodedValue::Dict)
    .parse(s)
}

fn parse_list(s: &str) -> IResult<&str, BencodedValue> {
    delimited(char('l'), many0(parse_bencoded_value), char('e'))
        .map(BencodedValue::List)
        .parse(s)
}

fn parse_bencoded_value(s: &str) -> IResult<&str, BencodedValue> {
    alt((parse_string, parse_ben_integer, parse_list, parse_dict)).parse(s)
}

fn decode_bencoded_value(s: &str) -> BencodedValue {
    parse_bencoded_value(s).unwrap().1
}

// Usage: your_program.sh decode "<encoded_value>"
fn main() {
    let args: Vec<String> = env::args().collect();
    let command = &args[1];

    if command == "decode" {
        let encoded_value = &args[2];
        let decoded_value: serde_json::Value = decode_bencoded_value(encoded_value).into();
        println!("{}", decoded_value);
    } else {
        println!("unknown command: {}", args[1])
    }
}
