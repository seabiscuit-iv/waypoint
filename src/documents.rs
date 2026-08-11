//! Text extraction for documents attached as topic seed context (§5.11).
//!
//! Everything is reduced to plain text here — the seed is prompt material,
//! not something the app renders, so layout and formatting are discarded.

use crate::error::CmdError;
use std::io::Read;
use std::path::Path;

/// Formats offered in the file picker and accepted on drop.
pub const TEXT_EXTS: &[&str] = &[
    "md", "markdown", "txt", "text", "rst", "org", "csv", "tsv", "json", "yaml", "yml", "toml",
    "rs", "py", "js", "ts", "c", "h", "cpp", "hpp", "glsl", "hlsl", "tex", "bib",
];
pub const DOC_EXTS: &[&str] = &["pdf", "docx"];

/// Per-file ceiling. Well beyond what the prompt budget can use, but low
/// enough that a pathological file can't exhaust memory.
const MAX_FILE_CHARS: usize = 400_000;

fn ext_of(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

pub fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Extracts plain text from one file, dispatching on extension.
pub fn extract(path: &Path) -> Result<String, CmdError> {
    let name = display_name(path);
    let text = match ext_of(path).as_str() {
        "pdf" => extract_pdf(path, &name)?,
        "docx" => extract_docx(path, &name)?,
        _ => std::fs::read_to_string(path).map_err(|_| {
            CmdError::new(
                "invalid",
                format!("{name} isn't plain text (UTF-8) and isn't a PDF or .docx."),
            )
        })?,
    };

    let text = normalize(&text);
    if text.trim().is_empty() {
        return Err(CmdError::new(
            "invalid",
            format!("No text could be read from {name}. If it's a scanned PDF the pages are images, not text."),
        ));
    }
    Ok(truncate_chars(&text, MAX_FILE_CHARS))
}

/// Collapses the ragged whitespace that PDF and DOCX extraction produce, so
/// the seed doesn't waste prompt budget on blank space.
fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank_run = 0;
    for line in s.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
            out.push('\n');
        } else {
            blank_run = 0;
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

fn extract_pdf(path: &Path, name: &str) -> Result<String, CmdError> {
    // pdf-extract panics on some malformed files rather than returning Err.
    let path = path.to_path_buf();
    let result = std::panic::catch_unwind(move || pdf_extract::extract_text(&path));
    match result {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(_)) | Err(_) => Err(CmdError::new(
            "invalid",
            format!("Couldn't read text from {name}. It may be encrypted, damaged, or a scan."),
        )),
    }
}

/// A .docx is a zip; the body lives in word/document.xml. Paragraph and break
/// tags become newlines, everything else is discarded.
fn extract_docx(path: &Path, name: &str) -> Result<String, CmdError> {
    use quick_xml::events::Event as XmlEvent;

    let bad = |_e: &dyn std::fmt::Debug| {
        CmdError::new("invalid", format!("Couldn't read {name} as a .docx file."))
    };

    let file = std::fs::File::open(path).map_err(|e| bad(&e))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| bad(&e))?;
    let mut doc = zip.by_name("word/document.xml").map_err(|e| bad(&e))?;
    let mut xml = String::new();
    doc.read_to_string(&mut xml).map_err(|e| bad(&e))?;

    let mut reader = quick_xml::Reader::from_str(&xml);
    let mut out = String::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Text(t)) => out.push_str(&t.decode().unwrap_or_default()),
            Ok(XmlEvent::End(e)) if e.name().as_ref() == b"w:p" => out.push('\n'),
            Ok(XmlEvent::Empty(e)) if matches!(e.name().as_ref(), b"w:br" | b"w:cr") => {
                out.push('\n')
            }
            Ok(XmlEvent::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(bad(&e)),
        }
        buf.clear();
    }
    Ok(out)
}
