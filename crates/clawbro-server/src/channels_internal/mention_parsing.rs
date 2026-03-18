use std::collections::HashSet;

pub(crate) fn extract_agent_mentions(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut mentions = Vec::new();
    for token in text.split_whitespace() {
        if !token.starts_with('@') {
            continue;
        }
        let mention = token
            .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
            .to_string();
        if mention.len() <= 1 {
            continue;
        }
        if seen.insert(mention.clone()) {
            mentions.push(mention);
        }
    }
    mentions
}

pub(crate) fn derive_fanout_message_id(base_id: &str, target_agent: Option<&str>) -> String {
    match target_agent {
        Some(target) => {
            let stable_target = target
                .trim()
                .trim_start_matches('@')
                .chars()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                        ch
                    } else {
                        '_'
                    }
                })
                .collect::<String>();
            format!("{base_id}#target={stable_target}")
        }
        None => base_id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_agent_mentions_preserves_order_and_dedupes() {
        let mentions = extract_agent_mentions("@alpha hi @beta go @alpha again");
        assert_eq!(mentions, vec!["@alpha".to_string(), "@beta".to_string()]);
    }

    #[test]
    fn derive_fanout_message_id_is_stable() {
        assert_eq!(
            derive_fanout_message_id("m1", Some("@alpha")),
            "m1#target=alpha"
        );
        assert_eq!(derive_fanout_message_id("m1", None), "m1");
    }
}
