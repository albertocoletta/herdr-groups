use std::fs;
use std::path::Path;

const FILE: &str = "config.env";
const DEFAULT_DIVIDER_WIDTH: usize = 22;

#[derive(Debug, PartialEq, Eq)]
pub struct Config {
    pub divider_width: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self { divider_width: DEFAULT_DIVIDER_WIDTH }
    }
}

impl Config {
    pub fn load(config_dir: &Path) -> Self {
        Self::parse(&fs::read_to_string(config_dir.join(FILE)).unwrap_or_default())
    }

    fn parse(text: &str) -> Self {
        let divider_width = value(text, "DIVIDER_WIDTH")
            .and_then(|width| width.parse().ok())
            .filter(|&width| width > 0)
            .unwrap_or(DEFAULT_DIVIDER_WIDTH);
        Self { divider_width }
    }
}

fn value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .find(|(name, _)| name.trim() == key)
        .map(|(_, value)| value.trim().trim_matches('"'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn missing_file_uses_the_default() {
        let dir = TempDir::new().unwrap();
        assert_eq!(Config::load(dir.path()).divider_width, DEFAULT_DIVIDER_WIDTH);
    }

    #[test]
    fn reads_the_divider_width_from_the_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(FILE), "# groups\nDIVIDER_WIDTH=36\n").unwrap();
        assert_eq!(Config::load(dir.path()).divider_width, 36);
    }

    #[test]
    fn accepts_a_quoted_value() {
        assert_eq!(Config::parse("DIVIDER_WIDTH=\"36\"").divider_width, 36);
    }

    #[test]
    fn non_number_falls_back_to_the_default() {
        assert_eq!(Config::parse("DIVIDER_WIDTH=wide").divider_width, DEFAULT_DIVIDER_WIDTH);
    }

    #[test]
    fn zero_falls_back_to_the_default() {
        assert_eq!(Config::parse("DIVIDER_WIDTH=0").divider_width, DEFAULT_DIVIDER_WIDTH);
    }
}
