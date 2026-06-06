//! A tiny JSON parser + compact serializer — zero crates. eldr writes JSON by hand
//! everywhere; this is the one place it needs to *read* JSON (incoming JSON-RPC requests
//! for the MCP server). Enough to navigate a few fields and echo a value back verbatim.

/// A parsed JSON value. Objects keep insertion order (they're tiny).
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// Parse a complete JSON document. `None` on malformed input or trailing garbage.
    pub fn parse(s: &str) -> Option<Json> {
        let mut p = Parser {
            b: s.as_bytes(),
            i: 0,
        };
        p.ws();
        let v = p.value()?;
        p.ws();
        if p.i == p.b.len() { Some(v) } else { None }
    }

    /// Look up a key on an object.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(kvs) => kvs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Num(n) => Some(*n as i64),
            _ => None,
        }
    }

    /// Compact re-serialization, used to echo a JSON-RPC `id` back verbatim.
    pub fn to_compact(&self) -> String {
        match self {
            Json::Null => "null".into(),
            Json::Bool(b) => b.to_string(),
            Json::Num(n) => {
                if n.is_finite() && n.fract() == 0.0 {
                    format!("{}", *n as i64)
                } else {
                    format!("{n}")
                }
            }
            Json::Str(s) => format!("\"{}\"", escape(s)),
            Json::Arr(a) => {
                let items: Vec<String> = a.iter().map(Json::to_compact).collect();
                format!("[{}]", items.join(","))
            }
            Json::Obj(o) => {
                let items: Vec<String> = o
                    .iter()
                    .map(|(k, v)| format!("\"{}\":{}", escape(k), v.to_compact()))
                    .collect();
                format!("{{{}}}", items.join(","))
            }
        }
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn value(&mut self) -> Option<Json> {
        self.ws();
        match self.b.get(self.i)? {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => self.string().map(Json::Str),
            b't' => self.lit("true", Json::Bool(true)),
            b'f' => self.lit("false", Json::Bool(false)),
            b'n' => self.lit("null", Json::Null),
            _ => self.number(),
        }
    }

    fn lit(&mut self, kw: &str, v: Json) -> Option<Json> {
        if self.b[self.i..].starts_with(kw.as_bytes()) {
            self.i += kw.len();
            Some(v)
        } else {
            None
        }
    }

    fn object(&mut self) -> Option<Json> {
        self.i += 1; // consume '{'
        let mut kvs = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b'}') {
            self.i += 1;
            return Some(Json::Obj(kvs));
        }
        loop {
            self.ws();
            let k = self.string()?;
            self.ws();
            if self.b.get(self.i) != Some(&b':') {
                return None;
            }
            self.i += 1;
            let v = self.value()?;
            kvs.push((k, v));
            self.ws();
            match self.b.get(self.i)? {
                b',' => self.i += 1,
                b'}' => {
                    self.i += 1;
                    break;
                }
                _ => return None,
            }
        }
        Some(Json::Obj(kvs))
    }

    fn array(&mut self) -> Option<Json> {
        self.i += 1; // consume '['
        let mut arr = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b']') {
            self.i += 1;
            return Some(Json::Arr(arr));
        }
        loop {
            let v = self.value()?;
            arr.push(v);
            self.ws();
            match self.b.get(self.i)? {
                b',' => self.i += 1,
                b']' => {
                    self.i += 1;
                    break;
                }
                _ => return None,
            }
        }
        Some(Json::Arr(arr))
    }

    /// Parse a string, accumulating raw bytes so multi-byte UTF-8 survives intact.
    fn string(&mut self) -> Option<String> {
        if self.b.get(self.i) != Some(&b'"') {
            return None;
        }
        self.i += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let c = *self.b.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let e = *self.b.get(self.i)?;
                    self.i += 1;
                    match e {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0c),
                        b'u' => {
                            let hex = self.b.get(self.i..self.i + 4)?;
                            let code =
                                u16::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?;
                            self.i += 4;
                            let ch = char::from_u32(code as u32).unwrap_or('\u{fffd}');
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        _ => return None,
                    }
                }
                _ => out.push(c),
            }
        }
        String::from_utf8(out).ok()
    }

    fn number(&mut self) -> Option<Json> {
        let start = self.i;
        if self.b.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        while self.i < self.b.len()
            && matches!(
                self.b[self.i],
                b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-'
            )
        {
            self.i += 1;
        }
        let s = std::str::from_utf8(&self.b[start..self.i]).ok()?;
        s.parse::<f64>().ok().map(Json::Num)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_objects_arrays_scalars() {
        let j = Json::parse(r#"{"a":1,"b":"hi","c":[true,null,2.5],"d":{"e":-3}}"#).unwrap();
        assert_eq!(j.get("a").and_then(Json::as_i64), Some(1));
        assert_eq!(j.get("b").and_then(Json::as_str), Some("hi"));
        assert_eq!(
            j.get("c").and_then(|c| match c {
                Json::Arr(a) => Some(a.len()),
                _ => None,
            }),
            Some(3)
        );
        assert_eq!(
            j.get("d").and_then(|d| d.get("e")).and_then(Json::as_i64),
            Some(-3)
        );
    }

    #[test]
    fn handles_escapes_and_unicode() {
        let j = Json::parse(r#"{"k":"a\"b\\c\né ñ"}"#).unwrap();
        assert_eq!(j.get("k").and_then(Json::as_str), Some("a\"b\\c\né ñ"));
    }

    #[test]
    fn rejects_garbage_and_trailing() {
        assert!(Json::parse("{").is_none());
        assert!(Json::parse("123 456").is_none());
        assert!(Json::parse("").is_none());
    }

    #[test]
    fn id_roundtrips_compact() {
        // JSON-RPC ids can be numbers or strings; both echo back unchanged.
        assert_eq!(Json::parse("42").unwrap().to_compact(), "42");
        assert_eq!(Json::parse(r#""abc""#).unwrap().to_compact(), "\"abc\"");
    }
}
