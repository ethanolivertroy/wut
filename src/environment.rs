use std::ffi::OsString;

/// Selects a canonical WUT environment value, reading the legacy ASK value only
/// when the canonical variable is absent. This helper never writes either variable.
pub fn canonical_or_legacy(
    canonical: Option<OsString>,
    legacy_read_only: Option<OsString>,
) -> Option<OsString> {
    canonical.or(legacy_read_only)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::canonical_or_legacy;

    #[test]
    fn canonical_environment_value_wins_even_when_empty() {
        assert_eq!(
            canonical_or_legacy(Some(OsString::new()), Some(OsString::from("legacy"))),
            Some(OsString::new())
        );
    }

    #[test]
    fn legacy_environment_value_is_a_read_only_fallback() {
        assert_eq!(
            canonical_or_legacy(None, Some(OsString::from("legacy"))),
            Some(OsString::from("legacy"))
        );
        assert_eq!(canonical_or_legacy(None, None), None);
    }
}
