mod error;
mod isolation;
mod runtime;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: sigil <command> [args...]");
        std::process::exit(1);
    }

    match runtime::supervisor::spawn(args) {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            eprintln!("sigil failed: {}", err);
            std::process::exit(1);
        }
    }
}
