//! The SlashProduct engine. The binary in `crates/slashproduct` is its command line.

/// The greeting the `greet` command prints.
pub fn greet(name: &str) -> String {
    format!("Hello, {name}, from SlashProduct.")
}

#[cfg(test)]
mod tests {
    use super::greet;

    #[test]
    fn greets_by_name() {
        assert_eq!(greet("Ada"), "Hello, Ada, from SlashProduct.");
    }
}
