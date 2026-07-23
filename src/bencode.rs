use nom::{
    IResult, Parser as _,
    branch::alt,
    bytes::complete::take,
    character::complete::char,
    multi::{fold_many0, many0},
    sequence::{delimited, preceded},
};

use std::{collections::BTreeMap, io::Write};

use super::parse_number;

#[derive(Debug)]
pub(crate) enum Value {
    String(Vec<u8>),
    Integer(i64),
    List(Vec<Value>),
    Dict(BTreeMap<Vec<u8>, Value>),
}

impl Value {
    pub fn try_into_string(self) -> Option<Vec<u8>> {
        if let Self::String(v) = self {
            Some(v)
        } else {
            None
        }
    }

    pub fn try_into_integer(self) -> Option<i64> {
        if let Self::Integer(v) = self {
            Some(v)
        } else {
            None
        }
    }

    pub fn try_into_list(self) -> Option<Vec<Value>> {
        if let Self::List(v) = self {
            Some(v)
        } else {
            None
        }
    }

    pub fn try_into_dict(self) -> Option<BTreeMap<Vec<u8>, Value>> {
        if let Self::Dict(v) = self {
            Some(v)
        } else {
            None
        }
    }

    pub fn seralize(&self) -> Vec<u8> {
        let mut result = Vec::new();
        self.serialize_impl(&mut result);
        result
    }

    fn serialize_impl(&self, result: &mut Vec<u8>) {
        let write_bytes = |arr: &mut Vec<u8>, bytes: &[u8]| {
            write!(arr, "{}:", bytes.len()).unwrap();
            arr.write_all(bytes).expect("Writing to vec cannot fail.");
        };
        match self {
            Value::String(bytes) => write_bytes(result, bytes),
            Value::Integer(x) => write!(result, "i{x}e").unwrap(),
            Value::List(bencoded_values) => {
                result.push(b'l');
                for value in bencoded_values {
                    value.serialize_impl(result);
                }
                result.push(b'e');
            }
            Value::Dict(map) => {
                result.push(b'd');
                for (key, val) in map.iter() {
                    write_bytes(result, key);
                    val.serialize_impl(result);
                }
                result.push(b'e');
            }
        };
    }

    pub fn as_string(&self) -> Option<&Vec<u8>> {
        if let Self::String(v) = self {
            Some(v)
        } else {
            None
        }
    }

    pub fn as_integer(&self) -> Option<&i64> {
        if let Self::Integer(v) = self {
            Some(v)
        } else {
            None
        }
    }

    pub fn as_list(&self) -> Option<&Vec<Value>> {
        if let Self::List(v) = self {
            Some(v)
        } else {
            None
        }
    }

    pub fn as_dict(&self) -> Option<&BTreeMap<Vec<u8>, Value>> {
        if let Self::Dict(v) = self {
            Some(v)
        } else {
            None
        }
    }
}

impl From<Value> for serde_json::Value {
    fn from(value: Value) -> Self {
        match value {
            Value::String(s) => serde_json::Value::String(String::from_utf8_lossy_owned(s)),
            Value::Integer(x) => serde_json::Value::Number(x.into()),
            Value::List(l) => serde_json::Value::Array(l.into_iter().map(|v| v.into()).collect()),
            Value::Dict(dict) => {
                let mut map: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
                for (key, value) in dict.into_iter() {
                    map.insert(String::from_utf8_lossy_owned(key), value.into());
                }
                serde_json::Value::Object(map)
            }
        }
    }
}

fn parse_string(s: &[u8]) -> IResult<&[u8], Value> {
    let (rest, len) = parse_number::<usize>(s)?;
    preceded(char(':'), take(len))
        .map(Vec::from)
        .map(Value::String)
        .parse(rest)
}

fn parse_ben_integer(s: &[u8]) -> IResult<&[u8], Value> {
    delimited(char('i'), parse_number::<i64>, char('e'))
        .map(Value::Integer)
        .parse(s)
}

// The trait bounds of foldmany0 require the initializer to be an FnMut, idk why
#[allow(clippy::redundant_closure)]
fn parse_dict(s: &[u8]) -> IResult<&[u8], Value> {
    delimited(
        char('d'),
        fold_many0(
            (parse_string, parse_bencoded_value),
            || BTreeMap::new(),
            |mut map, (new_key, new_val)| {
                let s = new_key.try_into_string().unwrap();
                map.insert(s, new_val);
                map
            },
        ),
        char('e'),
    )
    .map(Value::Dict)
    .parse(s)
}

fn parse_list(s: &[u8]) -> IResult<&[u8], Value> {
    delimited(char('l'), many0(parse_bencoded_value), char('e'))
        .map(Value::List)
        .parse(s)
}

fn parse_bencoded_value(s: &[u8]) -> IResult<&[u8], Value> {
    alt((parse_string, parse_ben_integer, parse_list, parse_dict)).parse(s)
}

pub(crate) fn parse(s: &[u8]) -> anyhow::Result<Value> {
    Ok(parse_bencoded_value(s)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .1)
}
