use std::{env, str::FromStr};

use nom::{
    IResult, Parser,
    bytes::complete::take,
    character::complete::{char, digit1},
    combinator::{opt, recognize},
    sequence::delimited,
};

// Available if you need it!
// use serde_bencode

#[allow(dead_code)]
fn decode_bencoded_value(encoded_value: &str) -> serde_json::Value {
    // If encoded_value starts with a digit, it's a number
    if let Ok((_, string)) = parse_string(encoded_value) {
        serde_json::Value::String(string.to_string())
    } else if let Ok((_, number)) = parse_ben_integer(encoded_value) {
        serde_json::Value::Number(number.into())
    } else {
        panic!("Unhandled encoded value: {}", encoded_value)
    }
}

fn parse_string(s: &str) -> IResult<&str, &str> {
    let (rest, len) = parse_number::<usize>(s)?;
    (char(':'), take(len)).map(|(_, s)| s).parse(rest)
}

fn parse_number<I: FromStr>(s: &str) -> IResult<&str, I> {
    recognize((opt(char('-')), digit1))
        .map_res(str::parse::<I>)
        .parse(s)
}

fn parse_ben_integer(s: &str) -> IResult<&str, i64> {
    delimited(char('i'), parse_number::<i64>, char('e')).parse(s)
}

// Usage: your_program.sh decode "<encoded_value>"
fn main() {
    let args: Vec<String> = env::args().collect();
    let command = &args[1];

    if command == "decode" {
        let encoded_value = &args[2];
        let decoded_value = decode_bencoded_value(encoded_value);
        println!("{}", decoded_value);
    } else {
        println!("unknown command: {}", args[1])
    }
}
