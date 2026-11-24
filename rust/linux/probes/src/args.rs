//! `--name value` command lines, small enough to need no dependency.
use anyhow::{Context, Result, bail};
use std::{collections::HashMap, str::FromStr};

pub struct Args {
    values: HashMap<String, String>,
    flags: Vec<String>,
}

impl Args {
    /// Parses the process's arguments. `flags` are the names that take no value.
    pub fn parse(flags: &[&str]) -> Result<Self> {
        Self::from(std::env::args().skip(1), flags)
    }

    pub fn from(args: impl Iterator<Item = String>, flags: &[&str]) -> Result<Self> {
        let mut values = HashMap::new();
        let mut seen = Vec::new();
        let mut args = args;
        while let Some(arg) = args.next() {
            let Some(name) = arg.strip_prefix("--") else {
                bail!("unexpected argument {arg:?}");
            };
            if flags.contains(&name) {
                seen.push(name.to_owned());
            } else {
                let value = args
                    .next()
                    .with_context(|| format!("--{name} needs a value"))?;
                values.insert(name.to_owned(), value);
            }
        }
        Ok(Self {
            values,
            flags: seen,
        })
    }

    pub fn flag(&self, name: &str) -> bool {
        self.flags.iter().any(|f| f == name)
    }

    pub fn text(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    pub fn get<T: FromStr>(&self, name: &str, default: T) -> Result<T> {
        match self.values.get(name) {
            None => Ok(default),
            Some(text) => text
                .parse()
                .ok()
                .with_context(|| format!("--{name}: cannot read {text:?}")),
        }
    }

    /// `2560x1440` as a width and a height.
    pub fn size(&self, name: &str, default: (u32, u32)) -> Result<(u32, u32)> {
        let Some(text) = self.values.get(name) else {
            return Ok(default);
        };
        let parsed = text
            .split_once('x')
            .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
        parsed.with_context(|| format!("--{name}: expected WIDTHxHEIGHT, got {text:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::Args;

    fn args(line: &str, flags: &[&str]) -> anyhow::Result<Args> {
        Args::from(line.split_whitespace().map(str::to_owned), flags)
    }

    #[test]
    fn values_flags_and_defaults() {
        let a = args("--size 3840x2160 --paced --fps 60", &["paced"]).unwrap();
        assert_eq!(a.size("size", (1, 1)).unwrap(), (3840, 2160));
        assert_eq!(a.get("fps", 30u32).unwrap(), 60);
        assert_eq!(a.get("frames", 600u32).unwrap(), 600);
        assert!(a.flag("paced"));
        assert!(!a.flag("caps"));
    }

    #[test]
    fn bad_input_is_refused() {
        assert!(args("--fps", &[]).is_err());
        assert!(args("fps 60", &[]).is_err());
        assert!(args("--fps sixty", &[]).unwrap().get("fps", 1u32).is_err());
        assert!(
            args("--size 4k", &[])
                .unwrap()
                .size("size", (1, 1))
                .is_err()
        );
    }
}
