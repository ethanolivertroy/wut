use std::process::ExitCode;

fn main() -> ExitCode {
    match wut::run(std::env::args_os().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("wut: {}", error.message());
            if let Some(hint) = error.hint_text() {
                eprintln!("hint: {hint}");
            }
            ExitCode::FAILURE
        }
    }
}
