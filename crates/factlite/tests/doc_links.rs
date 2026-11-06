//! Every relative link and image in the repository's Markdown resolves to an
//! existing file or directory inside the repository, with exact case.
//!
//! Links are parsed with pulldown-cmark, so code blocks, code spans, reference
//! links and angle-bracket destinations follow CommonMark. Not checked:
//! - `http`, `https` and `mailto` URLs;
//! - same-page anchors and anchor targets;
//! - raw HTML links.
//!
//! A target starting with `/` is resolved from the repository root, as GitHub
//! does. A query string, an anchor, and a `:line` or `:line-line` suffix are
//! removed before checking. Directories named `target`, hidden directories (such
//! as `.git` and `.github`) and symbolic links are not walked.

use std::path::{Component, Path, PathBuf};

use pulldown_cmark::{Event, Parser, Tag};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// Markdown files under `dir`, sorted, without following symbolic links.
fn markdown_files(dir: &Path, found: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let kind = std::fs::symlink_metadata(&path).unwrap().file_type();
        if kind.is_dir() && !name.starts_with('.') && name != "target" {
            markdown_files(&path, found);
        } else if kind.is_file() && name.to_ascii_lowercase().ends_with(".md") {
            found.push(path);
        }
    }
}

/// Link and image destinations in `text`, in document order.
fn destinations(text: &str) -> Vec<String> {
    Parser::new(text)
        .filter_map(|event| match event {
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                Some(dest_url.into_string())
            }
            _ => None,
        })
        .collect()
}

fn is_external(target: &str) -> bool {
    target.starts_with('#')
        || ["http://", "https://", "mailto:"]
            .iter()
            .any(|scheme| target.starts_with(scheme))
}

/// The path part of a local target: without query, anchor or `:line` suffix,
/// percent-decoded.
fn local_path(target: &str) -> Option<String> {
    let path = target.split(['?', '#']).next().unwrap_or("");
    let path = match path.rsplit_once(':') {
        Some((head, line))
            if !line.is_empty() && line.chars().all(|c| c.is_ascii_digit() || c == '-') =>
        {
            head
        }
        _ => path,
    };
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

/// Resolves `path` from `dir` (or from `root` when absolute) without touching
/// the filesystem, and fails if it leaves `root`.
fn resolve(root: &Path, dir: &Path, path: &str) -> Option<PathBuf> {
    let (mut resolved, relative) = match path.strip_prefix('/') {
        Some(rest) => (root.to_path_buf(), rest),
        None => (dir.to_path_buf(), path),
    };
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(part) => resolved.push(part),
            Component::ParentDir => {
                if !resolved.pop() || !resolved.starts_with(root) {
                    return None;
                }
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    resolved.starts_with(root).then_some(resolved)
}

/// Whether `path` exists with exactly this case in every component below
/// `root`. Plain `exists` ignores case on macOS and Windows, where a link
/// that would break on Linux and GitHub would still pass.
fn exists_exactly(root: &Path, path: &Path) -> bool {
    let mut current = root.to_path_buf();
    for component in path.strip_prefix(root).unwrap().components() {
        let Component::Normal(part) = component else {
            return false;
        };
        let Ok(entries) = std::fs::read_dir(&current) else {
            return false;
        };
        if !entries
            .filter_map(|entry| entry.ok())
            .any(|entry| entry.file_name() == part)
        {
            return false;
        }
        current.push(part);
    }
    true
}

/// Broken links under `root` as `file: target` lines, and how many local
/// links were checked.
fn check(root: &Path) -> (Vec<String>, usize) {
    let mut files = Vec::new();
    markdown_files(root, &mut files);
    let mut broken = Vec::new();
    let mut checked = 0;
    for file in &files {
        let name = file.strip_prefix(root).unwrap().display().to_string();
        let Ok(text) = std::fs::read_to_string(file) else {
            broken.push(format!("{name}: not UTF-8"));
            continue;
        };
        for target in destinations(&text) {
            if is_external(&target) {
                continue;
            }
            checked += 1;
            let ok = local_path(&target)
                .filter(|path| !path.is_empty())
                .and_then(|path| resolve(root, file.parent().unwrap(), &path))
                .is_some_and(|path| exists_exactly(root, &path));
            if !ok {
                broken.push(format!("{name}: {target}"));
            }
        }
    }
    (broken, checked)
}

#[test]
fn relative_markdown_links_resolve() {
    let (broken, checked) = check(&repository_root());
    assert!(broken.is_empty(), "broken links:\n{}", broken.join("\n"));
    // Guard against the checker silently finding nothing: the repository has
    // well over 50 local links.
    assert!(checked >= 50, "only {checked} local links were checked");
}

#[test]
fn checker_follows_commonmark_and_repository_rules() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("docs/a b")).unwrap();
    std::fs::create_dir_all(root.join("target")).unwrap();
    for file in ["README.md", "docs/a(b).md", "docs/a b/c.md", "src.rs"] {
        std::fs::write(root.join(file), "").unwrap();
    }
    std::fs::write(root.join("target/ignored.md"), "[x](missing.md)").unwrap();
    std::fs::write(
        root.join("docs/guide.md"),
        r#"
[ok](../README.md) [paren](a(b).md) [angle](<a b/c.md>) [encoded](a%20b/c.md)
[anchor](../README.md#part) [line](../src.rs:42) [range](../src.rs:3-9)
[root](/README.md) [query](../README.md?plain=1) ![img](../README.md)
[web](https://example.com/missing) [self](#top) [ref][r]

[r]: ../README.md

`[span](missing-span.md)` ``[double `tick`](missing-double.md)``

```
[fenced](missing-fenced.md)
```

~~~
[tilde](missing-tilde.md)
~~~

    [indented](missing-indented.md)

[broken](nope.md) [case](../readme.md) [outside](../../escape.md) [badref][b]
[empty](?only=query)

[b]: missing-ref.md
"#,
    )
    .unwrap();
    let (broken, checked) = check(&root);
    assert_eq!(
        broken,
        [
            "docs/guide.md: nope.md",
            "docs/guide.md: ../readme.md",
            "docs/guide.md: ../../escape.md",
            "docs/guide.md: missing-ref.md",
            "docs/guide.md: ?only=query",
        ],
    );
    assert_eq!(checked, 16);
}
