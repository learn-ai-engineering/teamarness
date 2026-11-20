// Cargo's PROFILE is the inherited base profile (debug or release), including
// custom profiles. DEBUG describes symbols, not release intent. Unknown/missing
// profiles fail closed when production-release is selected.
pub fn production_release(feature_enabled: bool, profile: Option<&str>) -> bool {
    feature_enabled && profile != Some("debug")
}

#[cfg(test)]
mod tests {
    use super::production_release;

    #[test]
    fn development_is_not_production_even_without_symbols() {
        assert!(!production_release(true, Some("debug")));
    }

    #[test]
    fn release_and_unknown_profiles_keep_production_guards() {
        for profile in [Some("release"), Some("custom"), None, Some("")] {
            assert!(production_release(true, profile));
        }
    }

    #[test]
    fn ordinary_builds_do_not_enable_production() {
        for profile in [Some("debug"), Some("release"), None] {
            assert!(!production_release(false, profile));
        }
    }
}
