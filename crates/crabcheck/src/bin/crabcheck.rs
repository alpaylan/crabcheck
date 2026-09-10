//! The `crabcheck` command. `crabcheck serve [--dir DIR] [--port PORT]
//! [--cargo-args "…"] [--no-instrument] [--target-dir DIR]` serves the
//! coverage grid pages, reports and targets a test run leaves under DIR
//! (default `.crabcheck`) at http://127.0.0.1:PORT/ (default 7878), and
//! runs `cargo` for the property or test picked on the page.

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("serve") => serve(&args[1..]),
        None | Some("--help" | "-h" | "help") => usage(0),
        Some(other) => {
            eprintln!("unknown command `{other}`");
            usage(2);
        },
    }
}

fn usage(code: i32) -> ! {
    eprintln!(
        "usage: crabcheck serve [--dir DIR] [--port PORT] [--cargo-args ARGS] [--no-instrument] [--target-dir DIR]\n\n\
         Serves DIR/coverage/*.html (written by CRABCHECK_COVERAGE_HTML=DIR/coverage),\n\
         DIR/reports.jsonl (CRABCHECK_REPORT_JSON), DIR/targets.jsonl and DIR/tests.json on\n\
         127.0.0.1:PORT, and runs cargo for the property or test picked on the page.\n\
         DIR defaults to .crabcheck and PORT to 7878. Start it inside the workspace.\n\n\
         --cargo-args ARGS   extra arguments for every cargo run, e.g. \"--features coverage\"\n\
         --no-instrument     do not append -C instrument-coverage to RUSTFLAGS\n\
         --target-dir DIR    CARGO_TARGET_DIR for the runs (default: the environment's, else target/cov)"
    );
    std::process::exit(code);
}

fn serve(args: &[String]) {
    let mut options = crabcheck::serve::Options::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dir" if i + 1 < args.len() => {
                options.dir = PathBuf::from(&args[i + 1]);
                i += 2;
            },
            "--port" if i + 1 < args.len() => {
                options.port = match args[i + 1].parse() {
                    Ok(port) => port,
                    Err(_) => {
                        eprintln!("--port needs a number");
                        std::process::exit(2);
                    },
                };
                i += 2;
            },
            "--cargo-args" if i + 1 < args.len() => {
                options.cargo_args.extend(args[i + 1].split_whitespace().map(String::from));
                i += 2;
            },
            "--target-dir" if i + 1 < args.len() => {
                options.target_dir = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            },
            "--no-instrument" => {
                options.instrument = false;
                i += 1;
            },
            other => {
                eprintln!("unknown argument `{other}`");
                usage(2);
            },
        }
    }
    let dir = options.dir.clone();
    let cargo_args = options.cargo_args.join(" ");
    let instrument = options.instrument;
    let server = match crabcheck::serve::start(options) {
        Ok(server) => server,
        Err(e) => {
            eprintln!("crabcheck serve: could not listen: {e}");
            std::process::exit(1);
        },
    };
    println!("crabcheck serve: {}", server.url());
    println!(
        "  pages   {}/coverage/*.html   (CRABCHECK_COVERAGE_HTML={}/coverage)",
        dir.display(),
        dir.display()
    );
    println!(
        "  reports {}/reports.jsonl     (CRABCHECK_REPORT_JSON={}/reports.jsonl)",
        dir.display(),
        dir.display()
    );
    println!("  targets {}/targets.jsonl     (read by the next run)", dir.display());
    println!("  tests   {}/tests.json        (written by \"Discover tests\")", dir.display());
    println!(
        "  runs    cargo {}{} from {}",
        if cargo_args.is_empty() { "…".to_string() } else { format!("… {cargo_args}") },
        if instrument { " with -C instrument-coverage" } else { "" },
        std::env::current_dir().map_or("here".to_string(), |d| d.display().to_string())
    );
    println!("Press Ctrl-C to stop.");
    loop {
        std::thread::park();
    }
}
