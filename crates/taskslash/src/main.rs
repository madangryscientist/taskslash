//! The `taskslash` command line. Exit codes: 0 ok, 2 a usage error.

use std::process::ExitCode;

const USAGE: &str = "usage:
  taskslash greet <name>   greet someone
  taskslash --version      print the version
  taskslash --help         print this help
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["--version" | "-V"] => {
            println!("taskslash {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        ["--help" | "-h"] => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        ["greet", name] => {
            println!("{}", taskslash_core::greet(name));
            ExitCode::SUCCESS
        }
        _ => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
