use crate::Error;
use crate::text::{Text, is_space, parse_json_string, trim};

pub const USAGE: &str = "Use complete, whitespace-separated arguments with matching quotes.";

pub fn split(input: &Text, usage: &str) -> Result<Vec<Text>, Error> {
    let text = trim(&input.0);
    let mut args = Vec::new();
    let mut position = 0;
    while position < text.len() {
        while text.get(position).is_some_and(|unit| is_space(*unit)) {
            position += 1;
        }
        let start = position;
        let Some(&first) = text.get(position) else {
            return Err(Error::type_error(usage));
        };
        if first == 34 || first == 39 {
            position += 1;
            loop {
                let unit = *text.get(position).ok_or_else(|| Error::type_error(usage))?;
                position += 1;
                if unit == first {
                    break;
                }
                if first == 34 && unit == 92 {
                    let escaped = *text.get(position).ok_or_else(|| Error::type_error(usage))?;
                    if matches!(escaped, 10 | 13 | 0x2028 | 0x2029) {
                        return Err(Error::type_error(usage));
                    }
                    position += 1;
                }
            }
        } else {
            while let Some(&unit) = text.get(position) {
                if is_space(unit) || unit == 34 || unit == 39 {
                    break;
                }
                position += 1;
            }
        }
        if text.get(position).is_some_and(|unit| !is_space(*unit)) {
            return Err(Error::type_error(usage));
        }
        let value = &text[start..position];
        args.push(match first {
            34 => parse_json_string(value).map_err(|_| Error::type_error(usage))?,
            39 => Text(value[1..value.len() - 1].to_vec()),
            _ => Text(value.to_vec()),
        });
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_quoted_empty_and_windows_arguments() -> Result<(), Error> {
        let args = split(&Text::from(" a '' \"b c\" 'C:\\repo\\file' "), USAGE)?;
        assert_eq!(
            args,
            ["a", "", "b c", "C:\\repo\\file"]
                .into_iter()
                .map(Text::from)
                .collect::<Vec<_>>()
        );
        Ok(())
    }

    #[test]
    fn refuses_unmatched_quotes_and_invalid_json_escapes() {
        for input in ["\"bad", "'bad", "a\"b\"", "\"\\x20\"", "\"a\"b", "\"a\nb\""] {
            assert_eq!(
                split(&Text::from(input), "invalid"),
                Err(Error::type_error("invalid"))
            );
        }
    }
}
