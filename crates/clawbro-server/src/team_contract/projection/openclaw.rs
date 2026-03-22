use anyhow::anyhow;

pub fn normalize_openclaw_helper_action(action: &str) -> anyhow::Result<&'static str> {
    match action {
        "checkpoint_task" => Ok("checkpoint_task"),
        "submit_task_result" => Ok("submit_task_result"),
        "block_task" => Ok("block_task"),
        "request_help" => Ok("request_help"),
        other => Err(anyhow!("unsupported helper action: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_actions_are_normalized() {
        assert_eq!(
            normalize_openclaw_helper_action("submit_task_result").unwrap(),
            "submit_task_result"
        );
        assert_eq!(
            normalize_openclaw_helper_action("request_help").unwrap(),
            "request_help"
        );
    }

    #[test]
    fn unsupported_actions_are_rejected() {
        let err = normalize_openclaw_helper_action("create_task")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unsupported helper action"));
    }
}
