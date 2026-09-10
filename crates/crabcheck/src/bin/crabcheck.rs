//! The `crabcheck` command. `crabcheck serve [--dir DIR] [--port PORT]`
//! serves the coverage grid pages, reports and targets a test run leaves
//! under DIR (default `.crabcheck`) at http://127.0.0.1:PORT/ (default 7878).

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
        "usage: crabcheck serve [--dir DIR] [--port PORT]\n\n\
         Serves DIR/coverage/*.html (written by CRABCHECK_COVERAGE_HTML=DIR/coverage),\n\
         DIR/reports.jsonl (CRABCHECK_REPORT_JSON) and DIR/targets.jsonl on 127.0.0.1:PORT.\n\
         DIR defaults to .crabcheck and PORT to 7878."
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
            other => {
                eprintln!("unknown argument `{other}`");
                usage(2);
            },
        }
    }
    let dir = options.dir.clone();
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
    println!("Press Ctrl-C to stop.");
    loop {
        std::thread::park();
    }
}
