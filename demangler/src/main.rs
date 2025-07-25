use {
    rustc_demangle::demangle,
    serde::{
        Deserialize,
        Serialize,
    },
    std::{
        env,
        fs,
    },
};

#[derive(Debug, Deserialize, Serialize)]
struct CoverageExport {
    data: Vec<ExportData>,
    #[serde(flatten)]
    other: serde_json::Value,
}

#[derive(Debug, Deserialize, Serialize)]
struct ExportData {
    functions: Vec<Function>,
    #[serde(flatten)]
    other: serde_json::Value,
}

#[derive(Debug, Deserialize, Serialize)]
struct Function {
    name: String,
    #[serde(flatten)]
    other: serde_json::Value,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: {} <input.json> <output.json>", args[0]);
        std::process::exit(1);
    }

    let input_path = &args[1];
    let output_path = &args[2];

    let json_str = fs::read_to_string(input_path)?;
    let mut export: CoverageExport = serde_json::from_str(&json_str)?;

    for data in &mut export.data {
        for func in &mut data.functions {
            func.name = format!("{:#}", demangle(&func.name));
        }
    }

    let output = serde_json::to_string_pretty(&export)?;
    fs::write(output_path, output)?;

    println!("✅ Wrote demangled JSON to {}", output_path);
    Ok(())
}
