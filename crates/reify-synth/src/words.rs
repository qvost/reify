use std::collections::BTreeSet;

pub(crate) fn tokens(value: &str) -> Vec<String> {
    let mut expanded = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    let mut previous = None;
    while let Some(c) = chars.next() {
        if c.is_ascii_uppercase()
            && previous.is_some_and(|p: char| {
                p.is_ascii_lowercase()
                    || (p.is_ascii_uppercase()
                        && chars.peek().is_some_and(char::is_ascii_lowercase))
            })
        {
            expanded.push(' ');
        }
        expanded.push(c.to_ascii_lowercase());
        previous = Some(c);
    }
    expanded
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
pub(crate) fn snake(value: &str) -> String {
    let name = tokens(value).join("_");
    if name.is_empty() {
        "value".into()
    } else if name.starts_with(|c: char| c.is_ascii_digit()) {
        format!("value_{name}")
    } else {
        name
    }
}
pub(crate) fn singular(word: &str) -> String {
    if let Some(base) = word.strip_suffix("ies") {
        format!("{base}y")
    } else if word.ends_with('s')
        && !word.ends_with("ss")
        && !word.ends_with("us")
        && word.len() > 3
    {
        word[..word.len() - 1].to_owned()
    } else {
        word.to_owned()
    }
}
pub(crate) fn stem(word: &str) -> String {
    match word {
        "find" | "lookup" | "searching" | "searched" => "search".into(),
        "booking" | "booked" | "reserve" | "reservation" | "reservations" => "book".into(),
        "fetch" | "retrieve" | "read" => "get".into(),
        _ => singular(word),
    }
}
pub(crate) fn stems(value: &str) -> BTreeSet<String> {
    tokens(value).iter().map(|w| stem(w)).collect()
}
