//! Every relative link in the repository's Markdown files resolves to an
//! existing file or directory. External links (with a URL scheme) and
//! same-page anchors are not checked, and neither are anchor targets. Fenced
//! code blocks and inline code spans are skipped.

use std::path::{Path, PathBuf};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn markdown_files(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy();
        if path.is_dir() {
            if !name.starts_with('.') && name != "target" {
                markdown_files(&path, found);
            }
        } else if name.ends_with(".md") {
            found.push(path);
        }
    }
}

/// Text outside fenced code blocks and inline code spans.
fn prose(text: &str) -> String {
    let mut out = String::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let mut in_code = false;
        for c in line.chars() {
            if c == '`' {
                in_code = !in_code;
            } else if !in_code {
                out.push(c);
            }
        }
        out.push('\n');
    }
    out
}

/// Targets of inline links `[text](target)`, without titles.
fn link_targets(text: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("](") {
        rest = &rest[start + 2..];
        let end = rest.find(')').unwrap_or(rest.len());
        let target = rest[..end].split_whitespace().next().unwrap_or("");
        if !target.is_empty() {
            targets.push(target.to_owned());
        }
        rest = &rest[end..];
    }
    targets
}

fn is_external(target: &str) -> bool {
    target.starts_with('#')
        || target.split_once(':').is_some_and(|(scheme, _)| {
            !scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_alphabetic())
        })
}

#[test]
fn relative_markdown_links_resolve() {
    let root = repository_root();
    let mut files = Vec::new();
    markdown_files(&root, &mut files);
    assert!(
        files.iter().any(|file| file.ends_with("README.md")),
        "no Markdown found under {}",
        root.display()
    );
    let mut broken = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        for target in link_targets(&prose(&text)) {
            if is_external(&target) {
                continue;
            }
            let path = target.split('#').next().unwrap();
            // A trailing ":line" suffix names a line in a source file.
            let path = path
                .rsplit_once(':')
                .filter(|(_, line)| !line.is_empty() && line.chars().all(|c| c.is_ascii_digit()))
                .map_or(path, |(path, _)| path);
            if !file.parent().unwrap().join(path).exists() {
                broken.push(format!(
                    "{}: {target}",
                    file.strip_prefix(&root).unwrap().display()
                ));
            }
        }
    }
    assert!(broken.is_empty(), "broken links:\n{}", broken.join("\n"));
}

#[test]
fn link_extraction_skips_code_and_external_targets() {
    let text = "a [x](docs/a.md) `[y](nope.md)` [z](https://e.com) [w](#top)\n```\n[v](gone.md)\n```\n[u](b.md#part \"title\")";
    let targets = link_targets(&prose(text));
    assert_eq!(targets, ["docs/a.md", "https://e.com", "#top", "b.md#part"]);
    let checked: Vec<_> = targets.iter().filter(|t| !is_external(t)).collect();
    assert_eq!(checked, ["docs/a.md", "b.md#part"]);
}
