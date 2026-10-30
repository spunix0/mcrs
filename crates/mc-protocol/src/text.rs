//! Flattens chat components (sent as network NBT) to plain text.
//! [`plain`] shows translation keys as-is; [`render`] fills them in from a language table like vanilla.

use mc_nbt::Tag;

pub fn plain(tag: &Tag) -> String {
    render(tag, &|_| None)
}

/// `lang` maps a translation key (e.g. `multiplayer.player.joined`) to its format string (`%s joined the game`).
pub fn render(tag: &Tag, lang: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    append(tag, lang, &mut out);
    out
}

/// Java `String.format`-style substitution of `%s` and `%1$s`; `%%` is a literal percent.
fn format(fmt: &str, args: &[String]) -> String {
    let mut out = String::new();
    let mut next = 0;
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let mut digits = String::new();
        while let Some(d) = chars.peek().filter(|d| d.is_ascii_digit()) {
            digits.push(*d);
            chars.next();
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some('$') if !digits.is_empty() => {
                chars.next(); // the 's'
                let i: usize = digits.parse().unwrap_or(1);
                out.push_str(args.get(i.saturating_sub(1)).map_or("", String::as_str));
            }
            Some('s') | Some('d') => {
                out.push_str(args.get(next).map_or("", String::as_str));
                next += 1;
            }
            Some(other) => {
                out.push('%');
                out.push_str(&digits);
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

fn append(tag: &Tag, lang: &dyn Fn(&str) -> Option<String>, out: &mut String) {
    match tag {
        Tag::String(s) => out.push_str(s),
        Tag::List(items) => items.iter().for_each(|t| append(t, lang, out)),
        Tag::Compound(_) => {
            // Mixed-type NBT lists wrap each element as `{"": value}`.
            if let Some(inner) = tag.get("") {
                return append(inner, lang, out);
            }
            if let Some(s) = tag.get("text").and_then(Tag::as_str) {
                out.push_str(s);
            } else if let Some(key) = tag.get("translate").and_then(Tag::as_str) {
                let args: Vec<String> = tag.get("with").and_then(Tag::as_list).map_or(Vec::new(), |a| a.iter().map(|t| render(t, lang)).collect());
                match lang(key).or_else(|| tag.get("fallback").and_then(Tag::as_str).map(str::to_string)) {
                    Some(fmt) => out.push_str(&format(&fmt, &args)),
                    None if args.is_empty() => out.push_str(key),
                    None => out.push_str(&format!("{key} [{}]", args.join(", "))),
                }
            }
            if let Some(extra) = tag.get("extra").and_then(Tag::as_list) {
                extra.iter().for_each(|t| append(t, lang, out));
            }
        }
        Tag::Byte(v) => out.push_str(&v.to_string()),
        Tag::Short(v) => out.push_str(&v.to_string()),
        Tag::Int(v) => out.push_str(&v.to_string()),
        Tag::Long(v) => out.push_str(&v.to_string()),
        Tag::Float(v) => out.push_str(&v.to_string()),
        Tag::Double(v) => out.push_str(&v.to_string()),
        other => out.push_str(&format!("{other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined() -> Tag {
        Tag::Compound(vec![
            ("translate".into(), Tag::String("multiplayer.player.joined".into())),
            ("with".into(), Tag::List(vec![Tag::Compound(vec![("text".into(), Tag::String("Player".into()))])])),
        ])
    }

    #[test]
    fn join_message() {
        assert_eq!(plain(&joined()), "multiplayer.player.joined [Player]");
        let lang = |k: &str| (k == "multiplayer.player.joined").then(|| "%s joined the game".to_string());
        assert_eq!(render(&joined(), &lang), "Player joined the game");
    }

    #[test]
    fn wrapped_and_numeric_args() {
        let tag = Tag::Compound(vec![
            ("translate".into(), Tag::String("k".into())),
            ("with".into(), Tag::List(vec![Tag::Compound(vec![("".into(), Tag::Int(1))]), Tag::Compound(vec![("".into(), Tag::String("x".into()))])])),
        ]);
        let lang = |_: &str| Some("%s and %s".to_string());
        assert_eq!(render(&tag, &lang), "1 and x");
    }

    #[test]
    fn positional_and_literal_percent() {
        let args = ["a".to_string(), "b".to_string()];
        assert_eq!(format("%2$s then %1$s, 100%%", &args), "b then a, 100%");
    }
}
