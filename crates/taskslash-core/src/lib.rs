//! The TaskSlash engine. The binary in `crates/taskslash` is its command line.

/// The greeting the `greet` command prints.
pub fn greet(name: &str) -> String {
    format!("Hello, {name}, from TaskSlash.")
}

#[cfg(test)]
mod tests {
    use super::greet;

    #[test]
    fn greets_by_name() {
        assert_eq!(greet("Ada"), "Hello, Ada, from TaskSlash.");
    }
}
