use std::path::{Path, PathBuf};

const GENERATED_HEADER: &str = "// Generated from crates/fugue-contract. Do not edit by hand.\n";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let destination = root.join("frontend/src/runtime/generated.ts");
    let check = std::env::args()
        .skip(1)
        .any(|argument| argument == "--check");

    if check {
        let candidate = root.join("target/fugue-contract/generated.ts");
        generate(&candidate)?;
        let expected = std::fs::read_to_string(&candidate)?;
        let actual = std::fs::read_to_string(&destination).map_err(|error| {
            format!(
                "{} is missing or unreadable ({error}); run ./fugue contracts",
                destination.display()
            )
        })?;
        if actual != expected {
            return Err(
                format!("{} is stale; run ./fugue contracts", destination.display()).into(),
            );
        }
        println!("contracts=up-to-date");
    } else {
        generate(&destination)?;
        println!("contracts=generated");
        println!("path={}", destination.display());
    }

    Ok(())
}

fn generate(destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let generated = with_specta_header(destination)?;
    let contents = std::fs::read_to_string(&generated)?;
    std::fs::write(
        destination,
        format!("{GENERATED_HEADER}{}\n", canonicalize(&contents)),
    )?;
    if generated != destination {
        std::fs::remove_file(generated)?;
    }
    Ok(())
}

fn canonicalize(contents: &str) -> String {
    let mut blocks = contents.trim().split("\n\n");
    let header = blocks.next().unwrap_or_default();
    let mut declarations = blocks.collect::<Vec<_>>();
    declarations.sort_unstable();
    format!("{header}\n\n{}", declarations.join("\n\n"))
}

fn with_specta_header(destination: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let raw = destination.with_extension("specta.ts");
    fugue_contract::export_typescript(
        raw.to_str()
            .ok_or("contract output path is not valid UTF-8")?,
    )?;
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::canonicalize;

    #[test]
    fn contract_declarations_have_a_stable_order() {
        let first = "// generated\n\nexport type Zebra = string\n\nexport type Apple = number\n";
        let second = "// generated\n\nexport type Apple = number\n\nexport type Zebra = string\n";
        assert_eq!(canonicalize(first), canonicalize(second));
    }
}
