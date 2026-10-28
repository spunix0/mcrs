//! Flattens chat components (sent as network NBT) to plain text. Translation keys are shown as-is with their arguments.

use mc_nbt::Tag;

pub fn plain(tag: &Tag) -> String {
    let mut out = String::new();
    append(tag, &mut out);
    out
}

fn append(tag: &Tag, out: &mut String) {
    match tag {
        Tag::String(s) => out.push_str(s),
        Tag::List(items) => items.iter().for_each(|t| append(t, out)),
        Tag::Compound(_) => {
            if let Some(s) = tag.get("text").and_then(Tag::as_str) {
                out.push_str(s);
            } else if let Some(key) = tag.get("translate").and_then(Tag::as_str) {
                out.push_str(key);
                if let Some(args) = tag.get("with").and_then(Tag::as_list) {
                    let parts: Vec<String> = args.iter().map(plain).collect();
                    out.push_str(&format!(" [{}]", parts.join(", ")));
                }
            }
            if let Some(extra) = tag.get("extra").and_then(Tag::as_list) {
                extra.iter().for_each(|t| append(t, out));
            }
        }
        other => out.push_str(&format!("{other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_message() {
        let tag = Tag::Compound(vec![
            ("translate".into(), Tag::String("multiplayer.player.joined".into())),
            ("with".into(), Tag::List(vec![Tag::Compound(vec![("text".into(), Tag::String("Player".into()))])])),
        ]);
        assert_eq!(plain(&tag), "multiplayer.player.joined [Player]");
    }
}
