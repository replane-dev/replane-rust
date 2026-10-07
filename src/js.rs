//! JavaScript-compatible conversions.
//!
//! Override evaluation must produce the same result as the Replane server
//! (written in TypeScript), including type casting and segmentation hashing,
//! so these follow ECMAScript `Number::toString` and `StringToNumber`.

/// `String(x)` for a JS number.
pub(crate) fn number_to_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x == 0.0 {
        return "0".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if x < 0.0 {
        return format!("-{}", number_to_string(-x));
    }

    // Rust's `{:e}` yields the shortest round-tripping digits, like JS does.
    let exp_repr = format!("{x:e}");
    let (mantissa, exp) = exp_repr.split_once('e').expect("LowerExp always has 'e'");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n = exp.parse::<i32>().expect("valid exponent") + 1;

    if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        let (int, frac) = digits.split_at(n as usize);
        format!("{int}.{frac}")
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let sign = if n - 1 < 0 { '-' } else { '+' };
        let e = (n - 1).abs();
        if k == 1 {
            format!("{digits}e{sign}{e}")
        } else {
            format!("{}.{}e{sign}{e}", &digits[..1], &digits[1..])
        }
    }
}

/// `Number(s)` for a JS string. Returns NaN when the string isn't numeric.
pub(crate) fn string_to_number(s: &str) -> f64 {
    let s = s.trim_matches(is_js_whitespace);
    if s.is_empty() {
        return 0.0;
    }

    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(rest) = s.strip_prefix(prefix) {
            return parse_radix(rest, radix);
        }
    }

    let (sign, unsigned) = match s.as_bytes()[0] {
        b'-' => (-1.0, &s[1..]),
        b'+' => (1.0, &s[1..]),
        _ => (1.0, s),
    };
    if unsigned == "Infinity" {
        return sign * f64::INFINITY;
    }
    if !is_decimal_literal(unsigned) {
        return f64::NAN;
    }
    unsigned.parse::<f64>().map_or(f64::NAN, |v| sign * v)
}

fn parse_radix(digits: &str, radix: u32) -> f64 {
    if digits.is_empty() {
        return f64::NAN;
    }
    let mut value = 0.0;
    for c in digits.chars() {
        match c.to_digit(radix) {
            Some(d) => value = value * radix as f64 + d as f64,
            None => return f64::NAN,
        }
    }
    value
}

/// Matches `digits [. digits] [e [+-] digits]` where at least one mantissa digit exists.
fn is_decimal_literal(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut mantissa_digits = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
        mantissa_digits += 1;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
            mantissa_digits += 1;
        }
    }
    if mantissa_digits == 0 {
        return false;
    }
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        i += 1;
        if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
            i += 1;
        }
        let exp_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == exp_start {
            return false;
        }
    }
    i == bytes.len()
}

fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// FNV-1a 32-bit hash over the UTF-8 bytes of `input`.
pub(crate) fn fnv1a32(input: &str) -> u32 {
    input.bytes().fold(0x811c_9dc5u32, |hash, byte| {
        (hash ^ byte as u32).wrapping_mul(0x0100_0193)
    })
}

/// Maps `input` to `[0, 1)` for percentage bucketing.
pub(crate) fn fnv1a32_to_unit(input: &str) -> f64 {
    fnv1a32(input) as f64 / 4_294_967_296.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_to_string_matches_js() {
        let cases = [
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (-42.0, "-42"),
            (2.75, "2.75"),
            (0.1, "0.1"),
            (100.0, "100"),
            (123456789.0, "123456789"),
            (1e20, "100000000000000000000"),
            (1e21, "1e+21"),
            (1.5e21, "1.5e+21"),
            (0.000001, "0.000001"),
            (1e-7, "1e-7"),
            (1.25e-7, "1.25e-7"),
            (0.1 + 0.2, "0.30000000000000004"),
            (f64::NAN, "NaN"),
            (f64::INFINITY, "Infinity"),
            (f64::NEG_INFINITY, "-Infinity"),
            (9007199254740993.0, "9007199254740992"),
        ];
        for (input, expected) in cases {
            assert_eq!(number_to_string(input), expected, "String({input})");
        }
    }

    #[test]
    fn string_to_number_matches_js() {
        let cases = [
            ("42", 42.0),
            ("  42\n", 42.0),
            ("", 0.0),
            ("   ", 0.0),
            ("2.75", 2.75),
            (".5", 0.5),
            ("5.", 5.0),
            ("-1e3", -1000.0),
            ("+7", 7.0),
            ("0x1f", 31.0),
            ("0b101", 5.0),
            ("0o17", 15.0),
            ("Infinity", f64::INFINITY),
            ("-Infinity", f64::NEG_INFINITY),
        ];
        for (input, expected) in cases {
            assert_eq!(string_to_number(input), expected, "Number({input:?})");
        }
        for nan in [
            "abc", "1a", "inf", "NaN", "nan", ".", "1e", "--1", "-0x10", "0x", "1_000",
        ] {
            assert!(
                string_to_number(nan).is_nan(),
                "Number({nan:?}) should be NaN"
            );
        }
    }

    #[test]
    fn fnv1a32_known_values() {
        assert_eq!(fnv1a32(""), 0x811c9dc5);
        assert_eq!(fnv1a32("a"), 0xe40c292c);
        assert_eq!(fnv1a32("foobar"), 0xbf9cf968);
    }

    #[test]
    fn fnv1a32_to_unit_in_range() {
        for input in ["", "0", "user-123seed", "🎉", &"a".repeat(1000)] {
            let unit = fnv1a32_to_unit(input);
            assert!((0.0..1.0).contains(&unit));
        }
    }
}
