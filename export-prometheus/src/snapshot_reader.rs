// SPDX-License-Identifier: Apache-2.0
// Historical wire fixtures are parsed only by the owning exporter.
use busbar_contract::export_calls::{kind_of, Family, Sample};

/// THE RECORDER SNAPSHOT (K9a S6): a host recorder's text exposition — each family's `# HELP`
pub fn parse_families(exposition: &str) -> Result<Vec<Family>, String> {
    if !exposition.is_empty() && !exposition.ends_with('\n') {
        return Err("the exposition does not end with a newline".into());
    }
    let mut families = Vec::new();
    let mut open: Option<(Family, bool)> = None;
    for (n, line) in exposition.split_terminator('\n').enumerate() {
        let at = |why: &str| format!("exposition line {}: {why}", n + 1);
        if line.is_empty() {
            let (f, _) = open
                .take()
                .ok_or_else(|| at("a blank line closes no family"))?;
            families.push(f);
        } else if let Some(rest) = line.strip_prefix("# HELP ") {
            if open.is_some() {
                return Err(at("a HELP line inside an unclosed family"));
            }
            let (name, help) = rest
                .split_once(' ')
                .ok_or_else(|| at("a HELP line with no text"))?;
            open = Some((family(name, Some(unescape(help, false)?)), false));
        } else if let Some(rest) = line.strip_prefix("# TYPE ") {
            let (name, word) = rest
                .split_once(' ')
                .ok_or_else(|| at("a TYPE line with no type"))?;
            let kind = kind_of(word).ok_or_else(|| at(&format!("the unknown type '{word}'")))?;
            match &mut open {
                None => {
                    let mut f = family(name, None);
                    f.kind = kind;
                    open = Some((f, true));
                }
                Some((f, typed)) if f.name == name && !*typed => {
                    f.kind = kind;
                    *typed = true;
                }
                Some(_) => return Err(at("a TYPE line that does not open or type its family")),
            }
        } else {
            let (f, _) = open
                .as_mut()
                .filter(|(_, typed)| *typed)
                .ok_or_else(|| at("a sample outside a typed family"))?;
            f.samples.push(sample(line).map_err(|why| at(&why))?);
        }
    }
    match open {
        Some((f, _)) => Err(format!("the family '{}' is not closed", f.name)),
        None => Ok(families),
    }
}

fn family(name: &str, help: Option<String>) -> Family {
    Family {
        name: name.to_string(),
        help,
        unit: None,
        kind: busbar_contract::abi::export::SCRAPE_KIND_UNTYPED,
        samples: Vec::new(),
    }
}

/// One sample line: `name[{k="v",…}] value`, the label values and the value kept as written.
fn sample(line: &str) -> Result<Sample, String> {
    let name_end = line.find(['{', ' ']).ok_or("a sample with no value")?;
    let mut sample = Sample {
        name: line[..name_end].to_string(),
        labels: Vec::new(),
        value: String::new(),
    };
    let mut rest = &line[name_end..];
    if let Some(mut body) = rest.strip_prefix('{') {
        loop {
            let (key, tail) = body.split_once("=\"").ok_or("a label with no value")?;
            let end = value_end(tail).ok_or("an unterminated label value")?;
            sample
                .labels
                .push((key.to_string(), unescape(&tail[..end], true)?));
            match tail[end + 1..].split_at_checked(1) {
                Some((",", next)) => body = next,
                Some(("}", after)) => {
                    rest = after;
                    break;
                }
                _ => return Err("a label set that does not close".into()),
            }
        }
    }
    let value = rest.strip_prefix(' ').ok_or("a sample with no value")?;
    let bad_key = sample
        .labels
        .iter()
        .any(|(k, _)| k.is_empty() || k.contains([',', '{', '}', '"', ' ']));
    if sample.name.is_empty() || bad_key || value.is_empty() || value.contains(' ') {
        return Err("a sample that is not `name[{labels}] value`".into());
    }
    sample.value = value.to_string();
    Ok(sample)
}

/// Where an escaped label value ends: the first `"` no backslash escapes.
fn value_end(escaped: &str) -> Option<usize> {
    let mut quoted = false;
    for (i, c) in escaped.char_indices() {
        match (quoted, c) {
            (true, _) => quoted = false,
            (false, '\\') => quoted = true,
            (false, '"') => return Some(i),
            _ => {}
        }
    }
    None
}

fn unescape(value: &str, quotes: bool) -> Result<String, String> {
    let mut out = zeroize::Zeroizing::new(String::with_capacity(value.len()));
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            out.push(match chars.next() {
                Some('\\') => '\\',
                Some('n') => '\n',
                Some('"') if quotes => '"',
                _ => return Err("invalid exposition escape".into()),
            });
        } else {
            out.push(c);
        }
    }
    Ok(std::mem::take(&mut *out))
}
