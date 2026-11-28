//! `ai-eval`: compare the two agent harnesses on a Theme Studio brief set.
//! See `vyasa_ai_eval::cli::USAGE`.

use std::process::ExitCode;

use vyasa_ai_eval::cli::{execute, parse, USAGE};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse(&args) {
        Ok(options) => options,
        Err(why) => {
            eprintln!("{why}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let env = |name: &str| std::env::var(name).ok();
    match execute(&options, &env, &mut std::io::stdout()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("{why}");
            ExitCode::FAILURE
        }
    }
}
