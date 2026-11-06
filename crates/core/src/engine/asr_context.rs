use std::ffi::OsStr;

pub(super) const OVERRIDE: &str = "OMASCRIBE_ASR_STREAM_HOTWORDS";

/// Only the pinned small model on Apple Silicon has passed both the paired
/// recognition and dynamic-update gates. Do not infer eligibility from the
/// model directory name, GPU detection, or an unrecognized environment value.
pub(super) fn use_stream_by_default(
    os: &str,
    arch: &str,
    model: &str,
    inherited: Option<&OsStr>,
) -> bool {
    inherited.is_none() && os == "macos" && arch == "aarch64" && model == "zipformer-small"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_validated_target_and_model_get_the_default() {
        for os in ["macos", "windows", "linux"] {
            for arch in ["aarch64", "x86_64"] {
                for model in ["zipformer-small", "zipformer-bilingual", "custom"] {
                    assert_eq!(
                        use_stream_by_default(os, arch, model, None),
                        (os, arch, model) == ("macos", "aarch64", "zipformer-small"),
                        "unexpected policy for {os}/{arch}/{model}"
                    );
                }
            }
        }
    }

    #[test]
    fn explicit_rollback_and_unknown_settings_are_never_replaced() {
        for value in ["0", "1", "", "false", "unknown"] {
            assert!(!use_stream_by_default(
                "macos",
                "aarch64",
                "zipformer-small",
                Some(OsStr::new(value)),
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_override_is_not_mistaken_for_unset() {
        use std::os::unix::ffi::OsStrExt;
        assert!(!use_stream_by_default(
            "macos",
            "aarch64",
            "zipformer-small",
            Some(OsStr::from_bytes(&[0xff])),
        ));
    }
}
