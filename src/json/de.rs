use std::char;
use std::io;
use std::num::Float;
use std::str;

use unicode::str::Utf16Item;

use de;
use super::error::{Error, ErrorCode};

pub struct Deserializer<Iter> {
    rdr: Iter,
    ch: Option<u8>,
    line: usize,
    col: usize,
    str_buf: Vec<u8>,
}

impl<Iter> Deserializer<Iter>
    where Iter: Iterator<Item=io::Result<u8>>,
{
    /// Creates the JSON parser from an `std::iter::Iterator`.
    #[inline]
    pub fn new(rdr: Iter) -> Result<Deserializer<Iter>, Error> {
        let mut deserializer = Deserializer {
            rdr: rdr,
            ch: None,
            line: 1,
            col: 0,
            str_buf: Vec::with_capacity(128),
        };

        try!(deserializer.bump());

        Ok(deserializer)
    }

    #[inline]
    pub fn end(&mut self) -> Result<(), Error> {
        try!(self.parse_whitespace());
        if self.eof() {
            Ok(())
        } else {
            Err(self.error(ErrorCode::TrailingCharacters))
        }
    }

    fn eof(&self) -> bool { self.ch.is_none() }

    fn ch_or_null(&self) -> u8 { self.ch.unwrap_or(b'\x00') }

    fn bump(&mut self) -> Result<(), Error> {
        self.ch = match self.rdr.next() {
            Some(Err(err)) => { return Err(Error::IoError(err)); }
            Some(Ok(ch)) => Some(ch),
            None => None,
        };

        if self.ch_is(b'\n') {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }

        Ok(())
    }

    fn next_char(&mut self) -> Result<Option<u8>, Error> {
        try!(self.bump());
        Ok(self.ch)
    }

    fn ch_is(&self, c: u8) -> bool {
        self.ch == Some(c)
    }

    fn error(&mut self, reason: ErrorCode) -> Error {
        Error::SyntaxError(reason, self.line, self.col)
    }

    fn parse_whitespace(&mut self) -> Result<(), Error> {
        while self.ch_is(b' ') ||
              self.ch_is(b'\n') ||
              self.ch_is(b'\t') ||
              self.ch_is(b'\r') { try!(self.bump()); }

        Ok(())
    }

    fn parse_value<V>(&mut self, mut visitor: V) -> Result<V::Value, Error>
        where V: de::Visitor,
    {
        try!(self.parse_whitespace());

        if self.eof() {
            return Err(self.error(ErrorCode::EOFWhileParsingValue));
        }

        match self.ch_or_null() {
            b'n' => {
                try!(self.parse_ident(b"ull"));
                visitor.visit_unit()
            }
            b't' => {
                try!(self.parse_ident(b"rue"));
                visitor.visit_bool(true)
            }
            b'f' => {
                try!(self.parse_ident(b"alse"));
                visitor.visit_bool(false)
            }
            b'0' ... b'9' | b'-' => self.parse_number(visitor),
            b'"' => {
                try!(self.parse_string());
                let s = str::from_utf8(&self.str_buf).unwrap();
                visitor.visit_str(s)
            }
            b'[' => {
                try!(self.bump());
                visitor.visit_seq(SeqVisitor::new(self))
            }
            b'{' => {
                try!(self.bump());
                visitor.visit_map(MapVisitor::new(self))
            }
            _ => {
                Err(self.error(ErrorCode::ExpectedSomeValue))
            }
        }
    }

    fn parse_ident(&mut self, ident: &[u8]) -> Result<(), Error> {
        for c in ident {
            if Some(*c) != try!(self.next_char()) {
                return Err(self.error(ErrorCode::ExpectedSomeIdent));
            }
        }

        try!(self.bump());
        Ok(())
    }

    fn parse_number<V>(&mut self, mut visitor: V) -> Result<V::Value, Error>
        where V: de::Visitor,
    {
        let mut neg = false;

        if self.ch_is(b'-') {
            try!(self.bump());
            neg = true;
        }

        let res = try!(self.parse_integer());

        if self.ch_is(b'.') || self.ch_is(b'e') || self.ch_is(b'E') {
            let mut res = res as f64;

            if self.ch_is(b'.') {
                res = try!(self.parse_decimal(res));
            }

            if self.ch_is(b'e') || self.ch_is(b'E') {
                res = try!(self.parse_exponent(res));
            }

            if neg {
                visitor.visit_f64(-res)
            } else {
                visitor.visit_f64(res)
            }
        } else {
            if neg {
                let res = -(res as i64);

                // Make sure we didn't underflow.
                if res > 0 {
                    Err(self.error(ErrorCode::InvalidNumber))
                } else {
                    visitor.visit_i64(res)
                }
            } else {
                visitor.visit_u64(res)
            }
        }
    }

    fn parse_integer(&mut self) -> Result<u64, Error> {
        let mut res = 0;

        match self.ch_or_null() {
            b'0' => {
                try!(self.bump());

                // There can be only one leading '0'.
                match self.ch_or_null() {
                    b'0' ... b'9' => {
                        return Err(self.error(ErrorCode::InvalidNumber));
                    }
                    _ => ()
                }
            },
            b'1' ... b'9' => {
                while !self.eof() {
                    match self.ch_or_null() {
                        c @ b'0' ... b'9' => {
                            res *= 10;
                            res += (c as u64) - (b'0' as u64);
                            try!(self.bump());
                        }
                        _ => break,
                    }
                }
            }
            _ => { return Err(self.error(ErrorCode::InvalidNumber)); }
        }

        Ok(res)
    }

    fn parse_decimal(&mut self, res: f64) -> Result<f64, Error> {
        try!(self.bump());

        // Make sure a digit follows the decimal place.
        match self.ch_or_null() {
            b'0' ... b'9' => (),
             _ => { return Err(self.error(ErrorCode::InvalidNumber)); }
        }

        let mut res = res;
        let mut dec = 1.0;
        while !self.eof() {
            match self.ch_or_null() {
                c @ b'0' ... b'9' => {
                    dec /= 10.0;
                    res += (((c as u64) - (b'0' as u64)) as f64) * dec;
                    try!(self.bump());
                }
                _ => break,
            }
        }

        Ok(res)
    }

    fn parse_exponent(&mut self, mut res: f64) -> Result<f64, Error> {
        try!(self.bump());

        let mut exp = 0;
        let mut neg_exp = false;

        if self.ch_is(b'+') {
            try!(self.bump());
        } else if self.ch_is(b'-') {
            try!(self.bump());
            neg_exp = true;
        }

        // Make sure a digit follows the exponent place.
        match self.ch_or_null() {
            b'0' ... b'9' => (),
            _ => { return Err(self.error(ErrorCode::InvalidNumber)); }
        }
        while !self.eof() {
            match self.ch_or_null() {
                c @ b'0' ... b'9' => {
                    exp *= 10;
                    exp += (c as i32) - (b'0' as i32);

                    try!(self.bump());
                }
                _ => break
            }
        }

        let exp: f64 = 10_f64.powi(exp);
        if neg_exp {
            res /= exp;
        } else {
            res *= exp;
        }

        Ok(res)
    }

    fn decode_hex_escape(&mut self) -> Result<u16, Error> {
        let mut i = 0;
        let mut n = 0u16;
        while i < 4 && !self.eof() {
            try!(self.bump());
            n = match self.ch_or_null() {
                c @ b'0' ... b'9' => n * 16_u16 + ((c as u16) - (b'0' as u16)),
                b'a' | b'A' => n * 16_u16 + 10_u16,
                b'b' | b'B' => n * 16_u16 + 11_u16,
                b'c' | b'C' => n * 16_u16 + 12_u16,
                b'd' | b'D' => n * 16_u16 + 13_u16,
                b'e' | b'E' => n * 16_u16 + 14_u16,
                b'f' | b'F' => n * 16_u16 + 15_u16,
                _ => { return Err(self.error(ErrorCode::InvalidEscape)); }
            };

            i += 1;
        }

        // Error out if we didn't parse 4 digits.
        if i != 4 {
            return Err(self.error(ErrorCode::InvalidEscape));
        }

        Ok(n)
    }

    fn parse_string(&mut self) -> Result<(), Error> {
        self.str_buf.clear();

        let mut escape = false;

        loop {
            let ch = match try!(self.next_char()) {
                Some(ch) => ch,
                None => { return Err(self.error(ErrorCode::EOFWhileParsingString)); }
            };

            if escape {
                match ch {
                    b'"' => self.str_buf.push(b'"'),
                    b'\\' => self.str_buf.push(b'\\'),
                    b'/' => self.str_buf.push(b'/'),
                    b'b' => self.str_buf.push(b'\x08'),
                    b'f' => self.str_buf.push(b'\x0c'),
                    b'n' => self.str_buf.push(b'\n'),
                    b'r' => self.str_buf.push(b'\r'),
                    b't' => self.str_buf.push(b'\t'),
                    b'u' => {
                        let c = match try!(self.decode_hex_escape()) {
                            0xDC00 ... 0xDFFF => {
                                return Err(self.error(ErrorCode::LoneLeadingSurrogateInHexEscape));
                            }

                            // Non-BMP characters are encoded as a sequence of
                            // two hex escapes, representing UTF-16 surrogates.
                            n1 @ 0xD800 ... 0xDBFF => {
                                let c1 = try!(self.next_char());
                                let c2 = try!(self.next_char());
                                match (c1, c2) {
                                    (Some(b'\\'), Some(b'u')) => (),
                                    _ => {
                                        return Err(self.error(ErrorCode::UnexpectedEndOfHexEscape));
                                    }
                                }

                                let buf = &[n1, try!(self.decode_hex_escape())];
                                match ::unicode::str::utf16_items(buf).next() {
                                    Some(Utf16Item::ScalarValue(c)) => c,
                                    _ => {
                                        return Err(self.error(ErrorCode::LoneLeadingSurrogateInHexEscape));
                                    }
                                }
                            }

                            n => match char::from_u32(n as u32) {
                                Some(c) => c,
                                None => {
                                    return Err(self.error(ErrorCode::InvalidUnicodeCodePoint));
                                }
                            }
                        };

                        let buf = &mut [0; 4];
                        let len = c.encode_utf8(buf).unwrap_or(0);
                        self.str_buf.extend(buf[..len].iter().map(|b| *b));
                    }
                    _ => {
                        return Err(self.error(ErrorCode::InvalidEscape));
                    }
                }
                escape = false;
            } else {
                match ch {
                    b'"' => {
                        try!(self.bump());
                        return Ok(());
                    }
                    b'\\' => {
                        escape = true;
                    }
                    ch => {
                        self.str_buf.push(ch);
                    }
                }
            }
        }
    }

    fn parse_object_colon(&mut self) -> Result<(), Error> {
        try!(self.parse_whitespace());

        if self.ch_is(b':') {
            try!(self.bump());
            Ok(())
        } else if self.eof() {
            Err(self.error(ErrorCode::EOFWhileParsingObject))
        } else {
            Err(self.error(ErrorCode::ExpectedColon))
        }
    }
}

impl<Iter> de::Deserializer for Deserializer<Iter>
    where Iter: Iterator<Item=io::Result<u8>>,
{
    type Error = Error;

    #[inline]
    fn visit<V>(&mut self, visitor: V) -> Result<V::Value, Error>
        where V: de::Visitor,
    {
        self.parse_value(visitor)
    }

    #[inline]
    fn visit_option<V>(&mut self, mut visitor: V) -> Result<V::Value, Error>
        where V: de::Visitor,
    {
        try!(self.parse_whitespace());

        if self.eof() {
            return Err(self.error(ErrorCode::EOFWhileParsingValue));
        }

        if self.ch_is(b'n') {
            try!(self.parse_ident(b"ull"));
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }

    #[inline]
    fn visit_enum<V>(&mut self, _name: &str, mut visitor: V) -> Result<V::Value, Error>
        where V: de::EnumVisitor,
    {
        try!(self.parse_whitespace());

        if self.ch_is(b'{') {
            try!(self.bump());
            try!(self.parse_whitespace());

            let value = {
                try!(visitor.visit(&mut *self))
            };

            try!(self.parse_whitespace());

            if self.ch_is(b'}') {
                try!(self.bump());
                Ok(value)
            } else {
                Err(self.error(ErrorCode::ExpectedSomeValue))
            }
        } else {
            Err(self.error(ErrorCode::ExpectedSomeValue))
        }
    }
}

struct SeqVisitor<'a, Iter: 'a> {
    de: &'a mut Deserializer<Iter>,
    first: bool,
}

impl<'a, Iter> SeqVisitor<'a, Iter> {
    fn new(de: &'a mut Deserializer<Iter>) -> Self {
        SeqVisitor {
            de: de,
            first: true,
        }
    }
}

impl<'a, Iter> de::SeqVisitor for SeqVisitor<'a, Iter>
    where Iter: Iterator<Item=io::Result<u8>>,
{
    type Error = Error;

    fn visit<T>(&mut self) -> Result<Option<T>, Error>
        where T: de::Deserialize,
    {
        try!(self.de.parse_whitespace());

        if self.de.ch_is(b']') {
            return Ok(None);
        }

        if self.first {
            self.first = false;
        } else {
            if self.de.ch_is(b',') {
                try!(self.de.bump());
            } else if self.de.eof() {
                return Err(self.de.error(ErrorCode::EOFWhileParsingList));
            } else {
                return Err(self.de.error(ErrorCode::ExpectedListCommaOrEnd));
            }
        }

        let value = try!(de::Deserialize::deserialize(self.de));
        Ok(Some(value))
    }

    fn end(&mut self) -> Result<(), Error> {
        try!(self.de.parse_whitespace());

        if self.de.ch_is(b']') {
            self.de.bump()
        } else if self.de.eof() {
            Err(self.de.error(ErrorCode::EOFWhileParsingList))
        } else {
            Err(self.de.error(ErrorCode::TrailingCharacters))
        }
    }
}

struct MapVisitor<'a, Iter: 'a> {
    de: &'a mut Deserializer<Iter>,
    first: bool,
}

impl<'a, Iter> MapVisitor<'a, Iter> {
    fn new(de: &'a mut Deserializer<Iter>) -> Self {
        MapVisitor {
            de: de,
            first: true,
        }
    }
}

impl<'a, Iter> de::MapVisitor for MapVisitor<'a, Iter>
    where Iter: Iterator<Item=io::Result<u8>>
{
    type Error = Error;

    fn visit_key<K>(&mut self) -> Result<Option<K>, Error>
        where K: de::Deserialize,
    {
        try!(self.de.parse_whitespace());

        if self.de.ch_is(b'}') {
            return Ok(None);
        }

        if self.first {
            self.first = false;
        } else {
            if self.de.ch_is(b',') {
                try!(self.de.bump());
                try!(self.de.parse_whitespace());
            } else if self.de.eof() {
                return Err(self.de.error(ErrorCode::EOFWhileParsingObject));
            } else {
                return Err(self.de.error(ErrorCode::ExpectedObjectCommaOrEnd));
            }
        }

        if self.de.eof() {
            return Err(self.de.error(ErrorCode::EOFWhileParsingValue));
        }

        if !self.de.ch_is(b'"') {
            return Err(self.de.error(ErrorCode::KeyMustBeAString));
        }

        Ok(Some(try!(de::Deserialize::deserialize(self.de))))
    }

    fn visit_value<V>(&mut self) -> Result<V, Error>
        where V: de::Deserialize,
    {
        try!(self.de.parse_object_colon());

        Ok(try!(de::Deserialize::deserialize(self.de)))
    }

    fn end(&mut self) -> Result<(), Error> {
        try!(self.de.parse_whitespace());

        if self.de.ch_is(b'}') {
            try!(self.de.bump());
            Ok(())
        } else if self.de.eof() {
            Err(self.de.error(ErrorCode::EOFWhileParsingObject))
        } else {
            Err(self.de.error(ErrorCode::TrailingCharacters))
        }
    }

    fn missing_field<V>(&mut self, _field: &'static str) -> Result<V, Error>
        where V: de::Deserialize,
    {
        // See if the type can deserialize from a unit.
        struct UnitDeserializer;

        impl de::Deserializer for UnitDeserializer {
            type Error = Error;

            fn visit<V>(&mut self, mut visitor: V) -> Result<V::Value, Error>
                where V: de::Visitor,
            {
                visitor.visit_unit()
            }

            fn visit_option<V>(&mut self, mut visitor: V) -> Result<V::Value, Error>
                where V: de::Visitor,
            {
                visitor.visit_none()
            }
        }

        Ok(try!(de::Deserialize::deserialize(&mut UnitDeserializer)))
    }
}

impl<Iter> de::VariantVisitor for Deserializer<Iter>
    where Iter: Iterator<Item=io::Result<u8>>,
{
    type Error = Error;

    fn visit_variant<V>(&mut self) -> Result<V, Error>
        where V: de::Deserialize
    {
        de::Deserialize::deserialize(self)
    }

    fn visit_value<V>(&mut self, visitor: V) -> Result<V::Value, Error>
        where V: de::Visitor,
    {
        try!(self.parse_object_colon());

        de::Deserializer::visit(self, visitor)
    }
}

/// Decodes a json value from a `std::io::Read`.
pub fn from_iter<I, T>(iter: I) -> Result<T, Error>
    where I: Iterator<Item=io::Result<u8>>,
          T: de::Deserialize,
{
    let mut de = try!(Deserializer::new(iter));
    let value = try!(de::Deserialize::deserialize(&mut de));

    // Make sure the whole stream has been consumed.
    try!(de.end());
    Ok(value)
}

/// Decodes a json value from a `std::io::Read`.
pub fn from_reader<R, T>(rdr: R) -> Result<T, Error>
    where R: io::Read,
          T: de::Deserialize,
{
    from_iter(rdr.bytes())
}

/// Decodes a json value from a `&str`.
pub fn from_slice<T>(v: &[u8]) -> Result<T, Error>
    where T: de::Deserialize
{
    from_iter(v.iter().map(|byte| Ok(*byte)))
}

/// Decodes a json value from a `&str`.
pub fn from_str<T>(s: &str) -> Result<T, Error>
    where T: de::Deserialize
{
    from_slice(s.as_bytes())
}
