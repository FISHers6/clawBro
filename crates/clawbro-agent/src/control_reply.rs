#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlReply {
    Final(String),
    Noop,
}

impl ControlReply {
    pub fn final_text(&self) -> Option<&str> {
        match self {
            Self::Final(text) => Some(text.as_str()),
            Self::Noop => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ControlReply;

    #[test]
    fn final_reply_carries_text() {
        let reply = ControlReply::Final("ok".to_string());
        assert_eq!(reply.final_text(), Some("ok"));
    }

    #[test]
    fn noop_reply_has_no_text() {
        let reply = ControlReply::Noop;
        assert_eq!(reply.final_text(), None);
    }
}
