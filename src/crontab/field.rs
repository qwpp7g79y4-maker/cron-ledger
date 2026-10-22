//! Parsing of a single vixie-cron schedule field (minute, hour, day-of-month,
//! month, or day-of-week) into a [`Field`].

use super::model::Field;

/// Bounds and name table for one schedule field.
pub struct FieldSpec {
    pub min: u32,
    pub max: u32,
    /// `Some` for month/day-of-week, which accept 3-letter names.
    pub names: Option<&'static [(&'static str, u32)]>,
    /// Day-of-week only: fold the upper bound (7) onto the lower bound (0),
    /// both meaning Sunday.
    pub fold_max_to_min: bool,
}

pub const MINUTE: FieldSpec = FieldSpec {
    min: 0,
    max: 59,
    names: None,
    fold_max_to_min: false,
};
pub const HOUR: FieldSpec = FieldSpec {
    min: 0,
    max: 23,
    names: None,
    fold_max_to_min: false,
};
pub const DOM: FieldSpec = FieldSpec {
    min: 1,
    max: 31,
    names: None,
    fold_max_to_min: false,
};
pub const MONTH: FieldSpec = FieldSpec {
    min: 1,
    max: 12,
    names: Some(&[
        ("jan", 1),
        ("feb", 2),
        ("mar", 3),
        ("apr", 4),
        ("may", 5),
        ("jun", 6),
        ("jul", 7),
        ("aug", 8),
        ("sep", 9),
        ("oct", 10),
        ("nov", 11),
        ("dec", 12),
    ]),
    fold_max_to_min: false,
};
// 0 and 7 both mean Sunday (vixie-cron). The field is parsed over 0..=7 and
// folded down to 0..=6 afterward.
pub const DOW: FieldSpec = FieldSpec {
    min: 0,
    max: 7,
    names: Some(&[
        ("sun", 0),
        ("mon", 1),
        ("tue", 2),
        ("wed", 3),
        ("thu", 4),
        ("fri", 5),
        ("sat", 6),
    ]),
    fold_max_to_min: true,
};

fn resolve_value(spec: &FieldSpec, token: &str) -> Result<u32, String> {
    if token.chars().all(|c| c.is_ascii_digit()) && !token.is_empty() {
        return token
            .parse::<u32>()
            .map_err(|_| format!("'{token}' is not a number"));
    }
    if let Some(names) = spec.names {
        let lower = token.to_ascii_lowercase();
        if let Some((_, v)) = names.iter().find(|(name, _)| *name == lower) {
            return Ok(*v);
        }
        return Err(format!("'{token}' is not a recognized name"));
    }
    Err(format!("'{token}' is not a number"))
}

fn check_bounds(spec: &FieldSpec, v: u32, token: &str) -> Result<(), String> {
    if v < spec.min || v > spec.max {
        return Err(format!(
            "'{token}' ({v}) is out of range {}-{}",
            spec.min, spec.max
        ));
    }
    Ok(())
}

fn fold(spec: &FieldSpec, v: u32) -> u32 {
    if spec.fold_max_to_min && v == spec.max {
        spec.min
    } else {
        v
    }
}

/// Parse one comma-separated schedule field, e.g. `*/15`, `1-5`, `mon-fri`,
/// `9,12,15`, `*`.
pub fn parse_field(spec: &FieldSpec, text: &str) -> Result<Field, String> {
    if text.is_empty() {
        return Err("field is empty".to_string());
    }
    let mut values = std::collections::BTreeSet::new();
    for item in text.split(',') {
        if item.is_empty() {
            return Err("empty item in list".to_string());
        }
        let (range_part, step_part) = match item.split_once('/') {
            Some((r, s)) => (r, Some(s)),
            None => (item, None),
        };
        let step: u32 = match step_part {
            Some(s) => s
                .parse::<u32>()
                .map_err(|_| format!("'{s}' is not a valid step"))?,
            None => 1,
        };
        if step == 0 {
            return Err("step cannot be zero".to_string());
        }

        if range_part == "*" {
            let mut v = spec.min;
            while v <= spec.max {
                values.insert(fold(spec, v));
                v = match v.checked_add(step) {
                    Some(next) => next,
                    None => return Err(format!("step '{step}' overflows while stepping '*'")),
                };
            }
            continue;
        }

        let (start, end) = if let Some((a, b)) = range_part.split_once('-') {
            let a = resolve_value(spec, a)?;
            let b = resolve_value(spec, b)?;
            check_bounds(spec, a, range_part)?;
            check_bounds(spec, b, range_part)?;
            if a > b {
                return Err(format!(
                    "'{range_part}' is a backwards range ({a} > {b}), wrap-around is not supported"
                ));
            }
            (a, b)
        } else {
            let a = resolve_value(spec, range_part)?;
            check_bounds(spec, a, range_part)?;
            if step_part.is_some() {
                // "a/s" means "a through the field's max, stepping by s".
                (a, spec.max)
            } else {
                (a, a)
            }
        };

        let mut v = start;
        while v <= end {
            values.insert(fold(spec, v));
            v = match v.checked_add(step) {
                Some(next) => next,
                None => return Err(format!("step '{step}' overflows in '{item}'")),
            };
        }
    }

    if values.is_empty() {
        return Err("field produced no values".to_string());
    }
    // A field that, after folding (dow 0==7), ends up covering every legal
    // value is treated the same as an explicit `*` so that the vixie
    // day-of-month/day-of-week OR rule (see [`super::model::matches_day`])
    // can tell "unrestricted" apart from "restricted but happens to match
    // every current value".
    let full_range: std::collections::BTreeSet<u32> = if spec.fold_max_to_min {
        (spec.min..spec.max).collect()
    } else {
        (spec.min..=spec.max).collect()
    };
    if text.contains('*') && values == full_range {
        return Ok(Field::Star);
    }
    Ok(Field::Values(values.into_iter().map(|v| v as u8).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn values(v: &[u8]) -> Field {
        Field::Values(v.iter().copied().collect::<BTreeSet<_>>())
    }

    #[test]
    fn star() {
        assert_eq!(parse_field(&MINUTE, "*").unwrap(), Field::Star);
    }

    #[test]
    fn single_value() {
        assert_eq!(parse_field(&MINUTE, "5").unwrap(), values(&[5]));
    }

    #[test]
    fn list() {
        assert_eq!(parse_field(&MINUTE, "1,2,3").unwrap(), values(&[1, 2, 3]));
    }

    #[test]
    fn range() {
        assert_eq!(
            parse_field(&HOUR, "9-12").unwrap(),
            values(&[9, 10, 11, 12])
        );
    }

    #[test]
    fn step_over_star() {
        assert_eq!(
            parse_field(&MINUTE, "*/15").unwrap(),
            values(&[0, 15, 30, 45])
        );
    }

    #[test]
    fn range_with_step() {
        assert_eq!(parse_field(&HOUR, "0-10/3").unwrap(), values(&[0, 3, 6, 9]));
    }

    #[test]
    fn open_ended_step() {
        // "a/s" means "a through max, step s".
        assert_eq!(parse_field(&HOUR, "5/5").unwrap(), values(&[5, 10, 15, 20]));
    }

    #[test]
    fn month_names() {
        assert_eq!(parse_field(&MONTH, "jan,MAR").unwrap(), values(&[1, 3]));
    }

    #[test]
    fn month_name_range() {
        assert_eq!(parse_field(&MONTH, "jan-mar").unwrap(), values(&[1, 2, 3]));
    }

    #[test]
    fn dow_names() {
        assert_eq!(
            parse_field(&DOW, "mon-fri").unwrap(),
            values(&[1, 2, 3, 4, 5])
        );
    }

    #[test]
    fn dow_seven_is_sunday() {
        assert_eq!(parse_field(&DOW, "7").unwrap(), values(&[0]));
        assert_eq!(parse_field(&DOW, "0,7").unwrap(), values(&[0]));
    }

    #[test]
    fn dow_explicit_full_range_stays_restricted() {
        // "0-7" is an explicit range, not a literal `*`: vixie-cron's
        // day-of-month/day-of-week OR rule treats it as restricted, even
        // though the resulting value set covers every day.
        assert_eq!(
            parse_field(&DOW, "0-7").unwrap(),
            values(&[0, 1, 2, 3, 4, 5, 6])
        );
    }

    #[test]
    fn out_of_range_is_error() {
        assert!(parse_field(&HOUR, "24").is_err());
        assert!(parse_field(&DOM, "0").is_err());
        assert!(parse_field(&MONTH, "13").is_err());
    }

    #[test]
    fn backwards_range_is_error() {
        assert!(parse_field(&HOUR, "22-2").is_err());
    }

    #[test]
    fn zero_step_is_error() {
        assert!(parse_field(&MINUTE, "*/0").is_err());
    }

    #[test]
    fn step_overflow_is_an_error_not_a_panic_or_wrong_value() {
        // Regression: `v += step` with step near u32::MAX overflowed the
        // stepping loop's accumulator, panicking in debug and silently
        // wrapping to an out-of-range value (0) in release.
        assert!(parse_field(&DOM, "1/4294967295").is_err());
        assert!(parse_field(&DOM, "*/4294967295").is_err());
    }

    #[test]
    fn names_not_allowed_outside_month_dow() {
        assert!(parse_field(&MINUTE, "jan").is_err());
        assert!(parse_field(&DOM, "mon").is_err());
    }

    #[test]
    fn garbage_is_error() {
        assert!(parse_field(&MINUTE, "banana").is_err());
        assert!(parse_field(&MINUTE, "").is_err());
        assert!(parse_field(&MINUTE, "1,,2").is_err());
    }
}
