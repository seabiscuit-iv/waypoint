//! Sanitizer for model-written ```svg blocks. The SVG is parsed and rebuilt
//! from an allowlist, so only known elements and attributes reach the
//! webview: scripts, event handlers, foreignObject, images, links, external
//! references, and <style> (which would restyle the whole app) never survive.
//! Ids are prefixed per diagram so they can't collide with the app's own ids
//! or with another diagram's.

use std::sync::{Arc, OnceLock};

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use resvg::{tiny_skia, usvg};

use crate::markdown;

const MAX_SOURCE_BYTES: usize = 100_000;
const MAX_ELEMENTS: usize = 4000;
const MAX_DEPTH: usize = 48;
const MAX_ATTR_BYTES: usize = 20_000;
/// Longest side of a review preview, in pixels.
const PREVIEW_MAX_PX: f32 = 1000.0;

const ELEMENTS: &[&str] = &[
    "svg",
    "g",
    "defs",
    "symbol",
    "use",
    "title",
    "desc",
    "path",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "text",
    "tspan",
    "textPath",
    "marker",
    "linearGradient",
    "radialGradient",
    "stop",
    "pattern",
    "clipPath",
    "mask",
];

/// Elements whose character data is kept. Text anywhere else is whitespace
/// between tags and is dropped.
const TEXT_ELEMENTS: &[&str] = &["text", "tspan", "textPath", "title", "desc"];

/// Presentation properties: allowed both as attributes and inside `style`.
const PROPERTIES: &[&str] = &[
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-opacity",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-dasharray",
    "stroke-dashoffset",
    "stroke-miterlimit",
    "opacity",
    "color",
    "vector-effect",
    "paint-order",
    "font-size",
    "font-family",
    "font-weight",
    "font-style",
    "text-anchor",
    "dominant-baseline",
    "alignment-baseline",
    "baseline-shift",
    "letter-spacing",
    "marker-start",
    "marker-mid",
    "marker-end",
    "stop-color",
    "stop-opacity",
    "clip-path",
    "clip-rule",
    "mask",
];

const ATTRIBUTES: &[&str] = &[
    "id",
    "class",
    "style",
    "href",
    "viewBox",
    "preserveAspectRatio",
    "width",
    "height",
    "x",
    "y",
    "x1",
    "y1",
    "x2",
    "y2",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "d",
    "points",
    "transform",
    "dx",
    "dy",
    "rotate",
    "textLength",
    "lengthAdjust",
    "startOffset",
    "markerWidth",
    "markerHeight",
    "markerUnits",
    "refX",
    "refY",
    "orient",
    "offset",
    "gradientUnits",
    "gradientTransform",
    "fx",
    "fy",
    "spreadMethod",
    "patternUnits",
    "patternContentUnits",
    "patternTransform",
    "clipPathUnits",
    "maskUnits",
    "maskContentUnits",
];

/// Theme-aware colour classes; each sets `color` for currentColor (app.css).
const PALETTE: &[&str] = &[
    "ink", "muted", "accent", "paper", "red", "orange", "yellow", "green", "blue", "purple",
];

/// Math labels are HTML in a foreignObject this size, centred on the text's
/// anchor point, so the label can grow in any direction without clipping.
const LABEL_BOX_WIDTH: f64 = 2000.0;
const LABEL_BOX_HEIGHT: f64 = 1000.0;

type Attrs = Vec<(&'static str, String)>;

struct Frame {
    name: &'static str,
    anchor: Option<String>,
    baseline: Option<String>,
}

/// A `<text>` element as it's copied out. If it holds only character data
/// with `$…$` math, it's replaced by an HTML label rendered by our own LaTeX
/// renderer.
struct Label {
    start: usize,
    depth: usize,
    attrs: Attrs,
    anchor: Option<String>,
    baseline: Option<String>,
    text: String,
    nested: bool,
}

/// Sanitized SVG markup, or None if the source isn't a complete, well-formed
/// `<svg>` element within the size limits.
pub fn sanitize(source: &str) -> Option<String> {
    sanitize_with(source, true)
}

const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";

/// The diagram as a self-contained .svg file: the SVG namespace, the light
/// theme's colours and label halo embedded, a white background, and `$…$`
/// labels as plain text, since a standalone file has no HTML to typeset them.
pub fn standalone(source: &str) -> Option<String> {
    sanitize_with(source, false)
}

/// Embedded in standalone files: app.css's light theme and label halo.
const STANDALONE_STYLE: &str = "svg{color:#23262B;background:#fff;font-family:Georgia,'Times New Roman',serif}\
text{paint-order:stroke;stroke:#fff;stroke-width:4px;stroke-linejoin:round}\
.dg-ink{color:#23262B}.dg-muted{color:#8B9099}.dg-accent{color:#14756B}.dg-paper{color:#fff}\
.dg-red{color:#C2453A}.dg-orange{color:#CF6D1D}.dg-yellow{color:#B8890F}\
.dg-green{color:#3A8A4F}.dg-blue{color:#2F6DB5}.dg-purple{color:#7A55B0}";

/// A PNG of the diagram for the review pass, drawn from the standalone file
/// so the reviewer sees what gets exported, and nothing the model wrote can
/// reach the filesystem.
pub fn preview_png(source: &str) -> Option<Vec<u8>> {
    let markup = standalone(source)?;
    let options = usvg::Options {
        fontdb: fonts(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(&markup, &options).ok()?;
    let size = tree.size();
    let scale = (PREVIEW_MAX_PX / size.width())
        .min(PREVIEW_MAX_PX / size.height())
        .min(4.0);
    let width = (size.width() * scale).ceil().max(1.0) as u32;
    let height = (size.height() * scale).ceil().max(1.0) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
    pixmap.fill(tiny_skia::Color::WHITE);
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap.encode_png().ok()
}

fn fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
        .clone()
}

/// `inline` is markup for the app's own page; otherwise it's a standalone file.
fn sanitize_with(source: &str, inline: bool) -> Option<String> {
    if source.len() > MAX_SOURCE_BYTES {
        return None;
    }
    let prefix = format!("dg{:x}-", fnv1a(source));
    let mut reader = Reader::from_str(source);
    let mut out = String::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut label: Option<Label> = None;
    let mut skip_depth = 0usize;
    let mut elements = 0usize;

    loop {
        match reader.read_event().ok()? {
            Event::Start(e) => {
                if skip_depth > 0 {
                    skip_depth += 1;
                    continue;
                }
                match allowed_element(&e, stack.is_empty())? {
                    Some(name) => {
                        elements += 1;
                        if elements > MAX_ELEMENTS || stack.len() >= MAX_DEPTH {
                            return None;
                        }
                        let attrs = clean_attrs(&e, &prefix, stack.is_empty());
                        let parent = stack.last();
                        let anchor = attr(&attrs, "text-anchor")
                            .map(str::to_string)
                            .or_else(|| parent.and_then(|f| f.anchor.clone()));
                        let baseline = attr(&attrs, "dominant-baseline")
                            .map(str::to_string)
                            .or_else(|| parent.and_then(|f| f.baseline.clone()));

                        if let Some(l) = label.as_mut() {
                            l.nested = true;
                        } else if name == "text" {
                            label = Some(Label {
                                start: out.len(),
                                depth: stack.len() + 1,
                                attrs: attrs.clone(),
                                anchor: anchor.clone(),
                                baseline: baseline.clone(),
                                text: String::new(),
                                nested: false,
                            });
                        }

                        write_open(&mut out, name, &attrs);
                        let is_root = stack.is_empty();
                        if is_root && !inline {
                            push_attr(&mut out, "xmlns", SVG_NAMESPACE);
                        }
                        out.push('>');
                        if is_root && !inline {
                            out.push_str("<style>");
                            out.push_str(STANDALONE_STYLE);
                            out.push_str("</style>");
                        }
                        stack.push(Frame { name, anchor, baseline });
                    }
                    None => skip_depth = 1,
                }
            }
            Event::Empty(e) => {
                if skip_depth > 0 || stack.is_empty() {
                    continue;
                }
                if let Some(name) = allowed_element(&e, false)? {
                    elements += 1;
                    if elements > MAX_ELEMENTS {
                        return None;
                    }
                    if let Some(l) = label.as_mut() {
                        l.nested = true;
                    }
                    let attrs = clean_attrs(&e, &prefix, false);
                    write_open(&mut out, name, &attrs);
                    out.push_str("/>");
                }
            }
            Event::End(_) => {
                if skip_depth > 0 {
                    skip_depth -= 1;
                    continue;
                }
                let frame = stack.pop()?;
                out.push_str("</");
                out.push_str(frame.name);
                out.push('>');
                if label.as_ref().is_some_and(|l| l.depth == stack.len() + 1) {
                    let l = label.take()?;
                    if inline {
                        if let Some(html) = math_label(&l) {
                            out.truncate(l.start);
                            out.push_str(&html);
                        }
                    } else if let Some(text) = plain_label_text(&l) {
                        out.truncate(l.start);
                        write_open(&mut out, "text", &l.attrs);
                        out.push('>');
                        escape_into(&mut out, &text);
                        out.push_str("</text>");
                    }
                }
                if stack.is_empty() {
                    return Some(out);
                }
            }
            Event::Text(t) if skip_depth == 0 && in_text(&stack) => {
                let text = t.decode().ok()?;
                push_label_text(&mut label, &stack, &text);
                escape_into(&mut out, &text);
            }
            Event::GeneralRef(r) if skip_depth == 0 && in_text(&stack) => {
                let ch = if r.is_char_ref() {
                    r.resolve_char_ref().ok().flatten()
                } else {
                    match &*r.decode().ok()? {
                        "amp" => Some('&'),
                        "lt" => Some('<'),
                        "gt" => Some('>'),
                        "quot" => Some('"'),
                        "apos" => Some('\''),
                        _ => None,
                    }
                };
                if let Some(ch) = ch.filter(|c| *c != '\0') {
                    let text = ch.encode_utf8(&mut [0; 4]).to_string();
                    push_label_text(&mut label, &stack, &text);
                    escape_into(&mut out, &text);
                }
            }
            // The root never closed: incomplete (mid-stream) or malformed.
            Event::Eof => return None,
            _ => {}
        }
    }
}

/// The canonical element name if it's allowed. The first element must be
/// `<svg>`; anything else there rejects the whole block (outer None).
fn allowed_element(e: &BytesStart, is_root: bool) -> Option<Option<&'static str>> {
    let qname = e.name();
    let name = std::str::from_utf8(qname.as_ref()).ok()?;
    let found = ELEMENTS.iter().find(|el| **el == name).copied();
    if is_root && found != Some("svg") {
        return None;
    }
    Some(found)
}

fn in_text(stack: &[Frame]) -> bool {
    stack.last().is_some_and(|f| TEXT_ELEMENTS.contains(&f.name))
}

fn push_label_text(label: &mut Option<Label>, stack: &[Frame], text: &str) {
    if let Some(l) = label.as_mut() {
        if stack.len() == l.depth {
            l.text.push_str(text);
        }
    }
}

fn attr<'a>(attrs: &'a Attrs, key: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_str())
}

fn clean_attrs(e: &BytesStart, prefix: &str, is_root: bool) -> Attrs {
    let mut attrs: Attrs = Vec::new();
    let mut has_view_box = false;
    let (mut width, mut height) = (None, None);
    for attr in e.attributes() {
        let Ok(attr) = attr else { continue };
        let Ok(raw_key) = std::str::from_utf8(attr.key.as_ref()) else { continue };
        let key = if raw_key == "xlink:href" { "href" } else { raw_key };
        let Some(key) = ATTRIBUTES
            .iter()
            .chain(PROPERTIES)
            .find(|a| **a == key)
            .copied()
        else {
            continue;
        };
        if attrs.iter().any(|(k, _)| *k == key) {
            continue;
        }
        let Ok(value) = attr.normalized_value(XmlVersion::Implicit1_0) else { continue };
        let cleaned = if key == "style" {
            clean_style(&value, prefix)
        } else {
            clean_value(key, &value, prefix)
        };
        let Some(cleaned) = cleaned else { continue };

        if is_root {
            match key {
                "viewBox" => has_view_box = true,
                "width" => width = parse_length(&cleaned),
                "height" => height = parse_length(&cleaned),
                _ => {}
            }
        }
        attrs.push((key, cleaned));
    }

    if is_root {
        // Without a viewBox the drawing can't scale down to fit the column.
        if !has_view_box {
            if let (Some(w), Some(h)) = (width, height) {
                attrs.push(("viewBox", format!("0 0 {w} {h}")));
            }
        }
        // SVG's default fill is black, which vanishes on the dark theme. As
        // an attribute rather than CSS, the model's own fill still wins.
        if !attrs.iter().any(|(k, _)| *k == "fill") {
            attrs.push(("fill", "currentColor".to_string()));
        }
    }
    attrs
}

fn write_open(out: &mut String, name: &str, attrs: &Attrs) {
    out.push('<');
    out.push_str(name);
    for (key, value) in attrs {
        push_attr(out, key, value);
    }
}

/// The foreignObject replacing a `<text>` whose content has `$…$` math, or
/// None to keep the plain SVG text.
fn math_label(l: &Label) -> Option<String> {
    let text = l.text.split_whitespace().collect::<Vec<_>>().join(" ");
    if l.nested || text.matches('$').count() < 2 {
        return None;
    }
    let x = single_number(attr(&l.attrs, "x").unwrap_or("0"))?;
    let y = single_number(attr(&l.attrs, "y").unwrap_or("0"))?;

    let tx = match l.anchor.as_deref() {
        Some("middle") => "-50%",
        Some("end") => "-100%",
        _ => "0",
    };
    // y is a baseline by default: centre the label a little above it.
    let ty = match l.baseline.as_deref() {
        Some("middle" | "central") => "-50%",
        Some("hanging" | "text-before-edge" | "text-top") => "0",
        Some("text-after-edge" | "ideographic" | "text-bottom") => "-100%",
        _ => "calc(-50% - 0.35em)",
    };
    let mut style = format!(
        "left:{}px;top:{}px;transform:translate({tx},{ty})",
        LABEL_BOX_WIDTH / 2.0,
        LABEL_BOX_HEIGHT / 2.0
    );
    for (key, prop) in [
        ("font-size", "font-size"),
        ("font-family", "font-family"),
        ("font-weight", "font-weight"),
        ("font-style", "font-style"),
        ("fill", "color"),
    ] {
        let Some(value) = attr(&l.attrs, key) else { continue };
        if value.contains([';', '{', '}']) || value.contains("url(") || value == "currentColor" {
            continue;
        }
        match parse_length(value).filter(|_| key == "font-size") {
            Some(px) => style.push_str(&format!(";{prop}:{px}px")),
            None => style.push_str(&format!(";{prop}:{value}")),
        }
    }

    let mut out = String::from("<foreignObject");
    push_attr(&mut out, "x", &(x - LABEL_BOX_WIDTH / 2.0).to_string());
    push_attr(&mut out, "y", &(y - LABEL_BOX_HEIGHT / 2.0).to_string());
    push_attr(&mut out, "width", &LABEL_BOX_WIDTH.to_string());
    push_attr(&mut out, "height", &LABEL_BOX_HEIGHT.to_string());
    for key in ["class", "transform", "opacity"] {
        if let Some(value) = attr(&l.attrs, key) {
            push_attr(&mut out, key, value);
        }
    }
    out.push_str("><div class=\"dg-label\"");
    push_attr(&mut out, "style", &style);
    out.push('>');

    // Even pieces are prose, odd pieces are math; a trailing unpaired `$`
    // stays literal.
    let pieces: Vec<&str> = text.split('$').collect();
    for (i, piece) in pieces.iter().enumerate() {
        if i % 2 == 0 {
            escape_into(&mut out, piece);
        } else if i == pieces.len() - 1 {
            out.push('$');
            escape_into(&mut out, piece);
        } else if let Some(math) = markdown::inline_math(piece) {
            out.push_str(&math);
        } else {
            out.push('$');
            escape_into(&mut out, piece);
            out.push('$');
        }
    }
    out.push_str("</div></foreignObject>");
    Some(out)
}

/// For standalone files (exports and the review preview): a math label's text with each `$…$` span turned
/// into plain characters about as wide as the typeset math, since the
/// renderer can't typeset it. None if the label has no math.
fn plain_label_text(l: &Label) -> Option<String> {
    let text = l.text.split_whitespace().collect::<Vec<_>>().join(" ");
    if l.nested || text.matches('$').count() < 2 {
        return None;
    }
    let pieces: Vec<&str> = text.split('$').collect();
    let mut out = String::new();
    for (i, piece) in pieces.iter().enumerate() {
        if i % 2 == 0 || i == pieces.len() - 1 {
            out.push_str(piece);
        } else {
            out.push_str(&latex_to_plain(piece));
        }
    }
    Some(out)
}

/// Symbols keep their glyph; other commands (\mathbf, \frac, \left) vanish
/// but their arguments stay; grouping and script markers are dropped.
fn latex_to_plain(latex: &str) -> String {
    let mut out = String::new();
    let mut chars = latex.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let mut name = String::new();
                while let Some(&n) = chars.peek() {
                    if !n.is_ascii_alphabetic() {
                        break;
                    }
                    name.push(n);
                    chars.next();
                }
                if name.is_empty() {
                    match chars.next() {
                        Some(p @ ('{' | '}' | '%' | '_' | '$' | '#' | '&')) => out.push(p),
                        _ => {}
                    }
                } else if let Some(sym) = latex_symbol(&name) {
                    out.push(sym);
                }
            }
            '{' | '}' | '_' | '^' => {}
            '~' => out.push(' '),
            _ => out.push(c),
        }
    }
    out
}

fn latex_symbol(name: &str) -> Option<char> {
    Some(match name {
        "alpha" => 'α',
        "beta" => 'β',
        "gamma" => 'γ',
        "delta" => 'δ',
        "epsilon" | "varepsilon" => 'ε',
        "zeta" => 'ζ',
        "eta" => 'η',
        "theta" | "vartheta" => 'θ',
        "iota" => 'ι',
        "kappa" => 'κ',
        "lambda" => 'λ',
        "mu" => 'μ',
        "nu" => 'ν',
        "xi" => 'ξ',
        "pi" => 'π',
        "rho" => 'ρ',
        "sigma" => 'σ',
        "tau" => 'τ',
        "upsilon" => 'υ',
        "phi" | "varphi" => 'φ',
        "chi" => 'χ',
        "psi" => 'ψ',
        "omega" => 'ω',
        "Gamma" => 'Γ',
        "Delta" => 'Δ',
        "Theta" => 'Θ',
        "Lambda" => 'Λ',
        "Xi" => 'Ξ',
        "Pi" => 'Π',
        "Sigma" => 'Σ',
        "Phi" => 'Φ',
        "Psi" => 'Ψ',
        "Omega" => 'Ω',
        "int" => '∫',
        "iint" => '∬',
        "oint" => '∮',
        "sum" => '∑',
        "prod" => '∏',
        "partial" => '∂',
        "nabla" => '∇',
        "infty" => '∞',
        "cdot" => '·',
        "times" => '×',
        "pm" => '±',
        "leq" | "le" => '≤',
        "geq" | "ge" => '≥',
        "neq" | "ne" => '≠',
        "approx" => '≈',
        "sim" => '∼',
        "propto" => '∝',
        "in" => '∈',
        "to" | "rightarrow" => '→',
        "leftarrow" => '←',
        "cdots" | "ldots" | "dots" => '…',
        "perp" => '⊥',
        "parallel" => '∥',
        "circ" => '∘',
        "ell" => 'ℓ',
        "hbar" => 'ℏ',
        _ => return None,
    })
}

fn single_number(value: &str) -> Option<f64> {
    value.trim().parse::<f64>().ok().filter(|n| n.is_finite())
}

fn push_attr(out: &mut String, key: &str, value: &str) {
    out.push(' ');
    out.push_str(key);
    out.push_str("=\"");
    escape_into(out, value);
    out.push('"');
}

fn clean_value(key: &str, value: &str, prefix: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_ATTR_BYTES {
        return None;
    }
    let squashed: String = value
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    const BANNED: [&str; 6] = ["javascript:", "data:", "expression(", "@import", "\\", "<"];
    if BANNED.iter().any(|b| squashed.contains(b)) {
        return None;
    }

    match key {
        "id" => valid_id(value).then(|| format!("{prefix}{value}")),
        "href" => value
            .strip_prefix('#')
            .filter(|id| valid_id(id))
            .map(|id| format!("#{prefix}{id}")),
        "class" => {
            let classes: Vec<String> = value
                .split_whitespace()
                .filter(|c| PALETTE.contains(c))
                .map(|c| format!("dg-{c}"))
                .collect();
            (!classes.is_empty()).then(|| classes.join(" "))
        }
        "fill" | "stroke" | "color" | "stop-color" if is_black(value) => Some("currentColor".to_string()),
        // The CSS size caps can change the box's shape; "none" would then
        // stretch the drawing unevenly and skew every angle.
        "preserveAspectRatio" if value.split_whitespace().any(|t| t == "none") => None,
        _ => rewrite_urls(value, prefix),
    }
}

fn clean_style(value: &str, prefix: &str) -> Option<String> {
    let decls: Vec<String> = value
        .split(';')
        .filter_map(|decl| {
            let (prop, val) = decl.split_once(':')?;
            let prop = prop.trim();
            let prop = PROPERTIES.iter().find(|p| **p == prop)?;
            let val = clean_value(prop, val, prefix)?;
            Some(format!("{prop}:{val}"))
        })
        .collect();
    (!decls.is_empty()).then(|| decls.join(";"))
}

/// Only same-document `url(#id)` references are allowed; they're re-pointed
/// at the prefixed ids.
fn rewrite_urls(value: &str, prefix: &str) -> Option<String> {
    let mut out = String::new();
    let mut rest = value;
    while let Some(i) = rest.to_ascii_lowercase().find("url(") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 4..];
        let close = after.find(')')?;
        let inner = after[..close].trim().trim_matches(|c: char| c == '"' || c == '\'');
        let id = inner.strip_prefix('#').filter(|id| valid_id(id))?;
        out.push_str(&format!("url(#{prefix}{id})"));
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    Some(out)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

fn is_black(value: &str) -> bool {
    ["black", "#000", "#000000"]
        .iter()
        .any(|b| value.eq_ignore_ascii_case(b))
}

fn parse_length(value: &str) -> Option<f64> {
    value
        .trim_end_matches("px")
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && *n > 0.0)
}

fn escape_into(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
}

fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
    })
}
