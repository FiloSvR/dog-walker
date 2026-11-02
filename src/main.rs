use std::process::ExitCode;

fn main() -> ExitCode {
    dog_walker::cli::run(
        std::env::args_os(),
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    )
}
