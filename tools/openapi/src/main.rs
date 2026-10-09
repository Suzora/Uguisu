//! Writes `docs/api/openapi.json`, or checks that it is current.
//!
//! ```text
//! cargo run -p openapi-doc              # write the document
//! cargo run -p openapi-doc -- --check    # fail when it is stale
//! ```

// A developer tool that reports on stdout and stderr like every other one.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::PathBuf;
use std::process::ExitCode;

use uguisu_server::openapi::ApiDoc;
use utoipa::OpenApi;

/// The document as the file holds it: pretty JSON with a trailing newline.
///
/// A derived document that will not serialize is a bug in this tool's own
/// types, not something a caller can act on, so it is reported and nothing is
/// written.
fn document() -> Result<String, String> {
    // utoipa builds the schemas recursively, and the event enum alone nests
    // deep enough to overflow Windows' 1 MiB main-thread stack in a debug
    // build; a thread with a stack of its own does not depend on that.
    let worker = std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| ApiDoc::openapi().to_pretty_json())
        .map_err(|e| format!("cannot start the document thread: {e}"))?;
    let mut json = worker
        .join()
        .map_err(|_| "the document thread panicked".to_owned())?
        .map_err(|e| format!("the derived document does not serialize: {e}"))?;
    json.push('\n');
    Ok(json)
}

fn target() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/api/openapi.json")
        .canonicalize()
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/api/openapi.json")
        })
}

fn main() -> ExitCode {
    let check = std::env::args().any(|a| a == "--check");
    let path = target();
    let generated = match document() {
        Ok(json) => json,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    if check {
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        if committed == generated {
            return ExitCode::SUCCESS;
        }
        eprintln!(
            "{} is not what the code produces; run `cargo run -p openapi-doc`",
            path.display()
        );
        return ExitCode::FAILURE;
    }
    if let Err(e) = std::fs::write(&path, &generated) {
        eprintln!("cannot write {}: {e}", path.display());
        return ExitCode::FAILURE;
    }
    println!("{}", path.display());
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use super::{document, target};

    /// The committed file is the contract. When this fails, the code changed a
    /// wire shape and `cargo run -p openapi-doc` is the fix — never the file by
    /// hand.
    #[test]
    fn the_document_matches_the_committed_file() {
        let committed = std::fs::read_to_string(target()).unwrap_or_default();
        assert_eq!(
            committed,
            document().unwrap(),
            "docs/api/openapi.json is stale; run `cargo run -p openapi-doc`"
        );
    }

    /// A document that serializes differently twice would make `--check` fail
    /// at random, which is worse than no check at all.
    #[test]
    fn the_document_is_byte_stable() {
        assert_eq!(document().unwrap(), document().unwrap());
    }

    /// A schema is named after the type's last path segment, so two types with
    /// the same name in different crates silently become one component and the
    /// document describes the wrong shape. `#[schema(as = ...)]` is the fix, and
    /// this is what says one is needed.
    #[test]
    fn no_two_types_share_a_schema_name() {
        let mut seen: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for file in rust_files(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../crates")) {
            let source = std::fs::read_to_string(&file).unwrap();
            for name in schema_names(&source) {
                seen.entry(name).or_default().push(
                    file.to_string_lossy()
                        .rsplit("crates/")
                        .next()
                        .unwrap_or_default()
                        .to_owned(),
                );
            }
        }
        let shared: Vec<_> = seen.iter().filter(|(_, at)| at.len() > 1).collect();
        assert!(shared.is_empty(), "one schema name, two types: {shared:?}");
    }

    fn rust_files(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(rust_files(&path));
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
        out
    }

    /// The name each `ToSchema` derive in `source` contributes to the document:
    /// the type's own name, or the one `#[schema(as = ...)]` gives it.
    fn schema_names(source: &str) -> Vec<String> {
        let lines: Vec<&str> = source.lines().collect();
        let mut names = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            if !(line.starts_with("#[derive(") && line.contains("ToSchema")) {
                continue;
            }
            let mut renamed = None;
            for following in &lines[index + 1..] {
                let following = without_visibility(following.trim());
                if let Some(rest) = following.strip_prefix("#[schema(as = ") {
                    renamed = rest.split(')').next().map(str::to_owned);
                } else if let Some(rest) = following
                    .strip_prefix("struct ")
                    .or_else(|| following.strip_prefix("enum "))
                {
                    let own = rest
                        .split(['(', '{', '<', ' ', ';'])
                        .next()
                        .unwrap_or_default()
                        .to_owned();
                    names.push(renamed.unwrap_or(own));
                    break;
                } else if ITEMS.iter().any(|item| following.starts_with(item)) {
                    break;
                }
            }
        }
        names
    }

    /// Items that end a derive's run of attributes without being the type.
    const ITEMS: [&str; 8] = [
        "fn ",
        "async fn ",
        "impl ",
        "const ",
        "static ",
        "type ",
        "trait ",
        "mod ",
    ];

    fn without_visibility(line: &str) -> &str {
        let Some(rest) = line.strip_prefix("pub") else {
            return line;
        };
        rest.strip_prefix('(')
            .and_then(|scoped| scoped.split_once(')'))
            .map_or(rest, |(_, after)| after)
            .trim_start()
    }
}
