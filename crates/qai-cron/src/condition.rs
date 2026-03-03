/// Conditions that gate whether a cron job fires.
///
/// Parsed from the `condition` field in `CronJobConfig` / `CronJob`,
/// e.g. `"idle_gt_seconds = 3600"` or `"idle_gt_seconds=300"`.
#[derive(Debug, Clone, PartialEq)]
pub enum CronCondition {
    /// Fire only if the target session has been idle for longer than N seconds.
    IdleGtSeconds(u64),
}

impl CronCondition {
    /// Parse a condition string into a `CronCondition`.
    ///
    /// Returns `None` if the string is empty, unrecognised, or malformed.
    ///
    /// Supported formats:
    /// - `"idle_gt_seconds = 3600"`
    /// - `"idle_gt_seconds=300"`
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if let Some(rest) = s.strip_prefix("idle_gt_seconds") {
            let n: u64 = rest.trim_start_matches([' ', '=']).parse().ok()?;
            return Some(CronCondition::IdleGtSeconds(n));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_idle_gt_seconds() {
        assert_eq!(
            CronCondition::parse("idle_gt_seconds = 3600"),
            Some(CronCondition::IdleGtSeconds(3600))
        );
        assert_eq!(
            CronCondition::parse("idle_gt_seconds=300"),
            Some(CronCondition::IdleGtSeconds(300))
        );
        assert_eq!(CronCondition::parse("always"), None);
        assert_eq!(CronCondition::parse(""), None);
    }

    #[test]
    fn test_parse_idle_gt_seconds_with_spaces() {
        assert_eq!(
            CronCondition::parse("  idle_gt_seconds = 7200  "),
            Some(CronCondition::IdleGtSeconds(7200))
        );
    }

    #[test]
    fn test_parse_idle_gt_seconds_no_spaces() {
        assert_eq!(
            CronCondition::parse("idle_gt_seconds=0"),
            Some(CronCondition::IdleGtSeconds(0))
        );
    }

    #[test]
    fn test_parse_unknown_condition_returns_none() {
        assert_eq!(CronCondition::parse("random_condition = 100"), None);
        assert_eq!(CronCondition::parse("never"), None);
    }
}
