mod config;
mod error;
mod isolation;
mod runtime;

fn main() {
    let config = match config::parse_args() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("{}", err);
            std::process::exit(1);
        }
    };

    match runtime::supervisor::spawn(config) {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            eprintln!("sigil failed: {}", err);
            std::process::exit(1);
        }
    }
}
