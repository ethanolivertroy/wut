use std::ffi::OsString;

pub fn canonical_or_legacy(
    canonical: Option<OsString>,
    legacy_read_only: Option<OsString>,
) -> Option<OsString> {
    canonical.or(legacy_read_only)
}
