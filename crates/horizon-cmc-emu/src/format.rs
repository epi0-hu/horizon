//! Just enough of C's printf to turn a core's log line into text.
//!
//! The core's log callback is variadic. We receive the first integer-class arguments as plain
//! words; floating point arguments live in other registers and can't be seen, so `%f` & co. print
//! a `?`. On targets other than x86-64 the extra arguments aren't reachable at all, so the caller
//! prints the format string as it is.

/// Format `fmt` with the integer-class `args`; `read_str` turns a `%s` argument (a pointer) into text.
pub fn format_c(fmt: &str, args: &[usize], read_str: &dyn Fn(usize) -> Option<String>) -> String {
    let b = fmt.as_bytes();
    let mut out = String::with_capacity(fmt.len() + 32);
    let mut ai = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'%' {
            let start = i;
            while i < b.len() && b[i] != b'%' {
                i += 1;
            }
            out.push_str(&fmt[start..i]);
            continue;
        }
        let spec_start = i;
        i += 1;
        if i >= b.len() {
            break;
        }
        if b[i] == b'%' {
            out.push('%');
            i += 1;
            continue;
        }
        let (mut left, mut plus, mut space, mut alt, mut zero) =
            (false, false, false, false, false);
        while i < b.len() && b"-+ #0".contains(&b[i]) {
            match b[i] {
                b'-' => left = true,
                b'+' => plus = true,
                b' ' => space = true,
                b'#' => alt = true,
                _ => zero = true,
            }
            i += 1;
        }
        let mut width = 0usize;
        while i < b.len() && b[i].is_ascii_digit() {
            width = width * 10 + (b[i] - b'0') as usize;
            i += 1;
        }
        let mut precision: Option<usize> = None;
        if i < b.len() && b[i] == b'.' {
            i += 1;
            let mut p = 0;
            while i < b.len() && b[i].is_ascii_digit() {
                p = p * 10 + (b[i] - b'0') as usize;
                i += 1;
            }
            precision = Some(p);
        }
        let (mut longs, mut wide) = (0, false);
        while i < b.len() && b"hlLqjzt".contains(&b[i]) {
            match b[i] {
                b'l' => longs += 1,
                b'h' => {}
                _ => wide = true,
            }
            i += 1;
        }
        // 'long' alone is 32-bit on Windows, 64-bit elsewhere.
        let long_long = wide || longs >= 2 || (longs == 1 && !cfg!(windows));
        if i >= b.len() {
            break;
        }
        let conv = b[i] as char;
        i += 1;
        if ai >= args.len() || !"diuxXoscpfFeEgG".contains(conv) {
            out.push_str(&fmt[spec_start..i]);
            continue;
        }
        let a = args[ai];
        ai += 1;
        let body: String = match conv {
            'd' | 'i' => {
                let v: i64 = if long_long {
                    a as i64
                } else {
                    a as u32 as i32 as i64
                };
                let mut s = v.unsigned_abs().to_string();
                if let Some(p) = precision {
                    while s.len() < p {
                        s.insert(0, '0');
                    }
                }
                let sign = if v < 0 {
                    "-"
                } else if plus {
                    "+"
                } else if space {
                    " "
                } else {
                    ""
                };
                pad_number(sign, s, width, left, zero && precision.is_none())
            }
            'u' | 'x' | 'X' | 'o' => {
                let v: u64 = if long_long { a as u64 } else { a as u32 as u64 };
                let mut s = match conv {
                    'u' => v.to_string(),
                    'x' => format!("{v:x}"),
                    'X' => format!("{v:X}"),
                    _ => format!("{v:o}"),
                };
                if let Some(p) = precision {
                    while s.len() < p {
                        s.insert(0, '0');
                    }
                }
                let prefix = match (alt, conv, v) {
                    (true, 'x', 1..) => "0x",
                    (true, 'X', 1..) => "0X",
                    _ => "",
                };
                pad_number(prefix, s, width, left, zero && precision.is_none())
            }
            'c' => pad_text((a as u8 as char).to_string(), width, left),
            'p' => pad_text(format!("0x{a:x}"), width, left),
            's' => {
                let mut s = if a == 0 {
                    "(null)".to_string()
                } else {
                    read_str(a).unwrap_or_else(|| "?".into())
                };
                if let Some(p) = precision {
                    s = s.chars().take(p).collect();
                }
                pad_text(s, width, left)
            }
            _ => pad_text("?".into(), width, left),
        };
        out.push_str(&body);
    }
    out
}

fn pad_text(s: String, width: usize, left: bool) -> String {
    let n = s.chars().count();
    if n >= width {
        s
    } else if left {
        format!("{s}{}", " ".repeat(width - n))
    } else {
        format!("{}{s}", " ".repeat(width - n))
    }
}

fn pad_number(sign: &str, digits: String, width: usize, left: bool, zero: bool) -> String {
    let n = sign.len() + digits.len();
    if n >= width {
        format!("{sign}{digits}")
    } else if left {
        format!("{sign}{digits}{}", " ".repeat(width - n))
    } else if zero {
        format!("{sign}{}{digits}", "0".repeat(width - n))
    } else {
        format!("{}{sign}{digits}", " ".repeat(width - n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(f: &str, args: &[usize]) -> String {
        format_c(f, args, &|p| Some(format!("str{p}")))
    }

    #[test]
    fn plain_text_and_percent() {
        assert_eq!(fmt("100%% done\n", &[]), "100% done\n");
        assert_eq!(fmt("no args", &[]), "no args");
    }

    #[test]
    fn integers() {
        assert_eq!(fmt("%d items", &[(-5i32) as u32 as usize]), "-5 items");
        assert_eq!(fmt("%u", &[u32::MAX as usize]), "4294967295");
        assert_eq!(fmt("%x %X %#x", &[255, 255, 255]), "ff FF 0xff");
        assert_eq!(
            fmt("[%5d] [%-5d] [%05d]", &[42, 42, 42]),
            "[   42] [42   ] [00042]"
        );
        assert_eq!(fmt("%zu bytes", &[33554432]), "33554432 bytes");
        assert_eq!(fmt("%llu", &[1 << 40]), "1099511627776");
        assert_eq!(fmt("%+d", &[7]), "+7");
    }

    #[test]
    fn strings_chars_and_pointers() {
        assert_eq!(fmt("loading %s", &[7]), "loading str7");
        assert_eq!(fmt("%s", &[0]), "(null)");
        assert_eq!(fmt("%.2s|%5s|%-5s|", &[7, 7, 7]), "st| str7|str7 |");
        assert_eq!(fmt("%c%c", &[b'o' as usize, b'k' as usize]), "ok");
        assert_eq!(fmt("%p", &[0x1000]), "0x1000");
    }

    #[test]
    fn floats_are_unreachable_and_missing_args_stay_literal() {
        assert_eq!(fmt("x=%f", &[0]), "x=?");
        assert_eq!(fmt("%d %d", &[1]), "1 %d");
        assert_eq!(fmt("trailing %", &[]), "trailing ");
    }

    #[cfg(not(windows))]
    #[test]
    fn long_is_64_bit_off_windows() {
        assert_eq!(fmt("%lu", &[1 << 40]), "1099511627776");
    }
}
