use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataDirectoryError {
    MissingValue,
    Repeated,
    Relative,
}

pub fn resolve_data_directory<I>(
    args: I,
    ordinary_directory: &Path,
) -> Result<PathBuf, DataDirectoryError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    let _executable = args.next();
    let mut selected = None;
    while let Some(argument) = args.next() {
        if argument == "--resonance-data-dir" {
            if selected.is_some() {
                return Err(DataDirectoryError::Repeated);
            }
            let value = args.next().ok_or(DataDirectoryError::MissingValue)?;
            if value == "--resonance-data-dir" || value.to_string_lossy().starts_with("--") {
                return Err(DataDirectoryError::MissingValue);
            }
            let path = PathBuf::from(value);
            if !path.is_absolute() {
                return Err(DataDirectoryError::Relative);
            }
            selected = Some(path);
        }
    }
    Ok(selected.unwrap_or_else(|| ordinary_directory.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn uses_tauri_application_data_by_default() {
        let ordinary = Path::new("/private/application-data/dev.resonance.desktop");
        assert_eq!(
            resolve_data_directory(args(&["resonance"]), ordinary),
            Ok(ordinary.into())
        );
    }

    #[test]
    fn accepts_one_absolute_backend_override() {
        assert_eq!(
            resolve_data_directory(
                args(&["resonance", "--resonance-data-dir", "/tmp/resonance-two"]),
                Path::new("/default"),
            ),
            Ok(PathBuf::from("/tmp/resonance-two"))
        );
    }

    #[test]
    fn rejects_relative_repeated_and_missing_values() {
        assert_eq!(
            resolve_data_directory(
                args(&["resonance", "--resonance-data-dir", "relative"]),
                Path::new("/default")
            ),
            Err(DataDirectoryError::Relative)
        );
        assert_eq!(
            resolve_data_directory(
                args(&[
                    "resonance",
                    "--resonance-data-dir",
                    "/one",
                    "--resonance-data-dir",
                    "/two"
                ]),
                Path::new("/default"),
            ),
            Err(DataDirectoryError::Repeated)
        );
        assert_eq!(
            resolve_data_directory(
                args(&["resonance", "--resonance-data-dir"]),
                Path::new("/default")
            ),
            Err(DataDirectoryError::MissingValue)
        );
    }
}
