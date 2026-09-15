use std::ops::Range;

use pulldown_cmark::{html, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use pulldown_latex::config::DisplayMode;
use pulldown_latex::{push_mathml, Parser as LatexParser, ParserError, RenderConfig, Storage};

use crate::svg;

/// Render markdown to HTML. Raw HTML in the source is escaped (emitted as
/// text) so model output can never inject markup into the webview.
///
/// `$…$` / `$$…$$` are converted to MathML here rather than in the webview:
/// it keeps the strict CSP intact (no math library, no external fonts) and
/// the emitted markup is ours, not the model's.
///
/// Fenced `svg` code blocks become diagrams, rebuilt by the sanitizer in svg.rs.
pub fn render(md: &str) -> String {
    render_with(md, false)
}

fn options() -> Options {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_MATH);
    opts
}

pub struct SvgBlock {
    /// The whole fenced block, fences included.
    pub block: Range<usize>,
    /// Where the contents sit in the source, when they sit there verbatim
    /// (not, say, in an indented block in a list), so they can be replaced.
    pub contents: Option<Range<usize>>,
    pub source: String,
}

/// The fenced svg blocks in `md`, in order.
pub fn svg_blocks(md: &str) -> Vec<SvgBlock> {
    let mut blocks = Vec::new();
    let mut current: Option<SvgBlock> = None;
    for (ev, range) in Parser::new_ext(md, options()).into_offset_iter() {
        match ev {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(ref lang))) if is_svg(lang) => {
                current = Some(SvgBlock {
                    block: range,
                    contents: None,
                    source: String::new(),
                });
            }
            Event::Text(t) => {
                if let Some(b) = current.as_mut() {
                    let start = b.contents.as_ref().map_or(range.start, |s| s.start);
                    b.contents = Some(start..range.end);
                    b.source.push_str(&t);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(mut b) = current.take() {
                    let verbatim = b
                        .contents
                        .as_ref()
                        .is_some_and(|r| md.get(r.clone()) == Some(b.source.as_str()));
                    if !verbatim {
                        b.contents = None;
                    }
                    blocks.push(b);
                }
            }
            _ => {}
        }
    }
    blocks
}

/// `md` with every fenced svg block swapped for `replace(block)`, fences
/// included. Returning None keeps that block as it was.
pub fn replace_svg_blocks(md: &str, mut replace: impl FnMut(&SvgBlock) -> Option<String>) -> String {
    let mut out = String::new();
    let mut last = 0;
    for b in svg_blocks(md) {
        if b.block.start < last {
            continue;
        }
        if let Some(replacement) = replace(&b) {
            out.push_str(&md[last..b.block.start]);
            out.push_str(&replacement);
            last = b.block.end;
        }
    }
    out.push_str(&md[last..]);
    out
}

/// `md` with every fenced svg block removed, fences included.
pub fn strip_svg_blocks(md: &str) -> String {
    replace_svg_blocks(md, |_| Some(String::new()))
}

fn render_with(md: &str, streaming: bool) -> String {
    let parser = Parser::new_ext(md, options()).map(|ev| match ev {
        Event::Html(s) => Event::Text(s),
        Event::InlineHtml(s) => Event::Text(s),
        Event::InlineMath(s) => render_math(&s, DisplayMode::Inline),
        Event::DisplayMath(s) => render_math(&s, DisplayMode::Block),
        other => other,
    });

    let mut events = Vec::new();
    let mut svg_src: Option<String> = None;
    for ev in parser {
        match ev {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(ref lang))) if is_svg(lang) => {
                svg_src = Some(String::new());
            }
            Event::Text(t) if svg_src.is_some() => svg_src.get_or_insert_default().push_str(&t),
            Event::End(TagEnd::CodeBlock) if svg_src.is_some() => {
                let source = svg_src.take().unwrap_or_default();
                // Mid-stream a diagram is unfinished or, once complete, about
                // to be reviewed, so it stays a placeholder until the step is done.
                if streaming {
                    events.push(Event::Html("<div class=\"diagram-pending\"></div>\n".into()));
                    continue;
                }
                match svg::sanitize(&source) {
                    Some(markup) => {
                        events.push(Event::Html(format!("<div class=\"diagram\">{markup}</div>\n").into()));
                    }
                    None => {
                        events.push(Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced("svg".into()))));
                        events.push(Event::Text(source.into()));
                        events.push(Event::End(TagEnd::CodeBlock));
                    }
                }
            }
            other => events.push(other),
        }
    }

    let mut out = String::new();
    html::push_html(&mut out, events.into_iter());
    out
}

fn is_svg(lang: &str) -> bool {
    lang.split_whitespace()
        .next()
        .is_some_and(|l| l.eq_ignore_ascii_case("svg"))
}

/// How far back a still-unclosed `$` is allowed to be before we assume it is
/// a literal dollar sign rather than math the model is mid-way through.
const MAX_OPEN_MATH_TAIL: usize = 160;

/// Render for a stream in progress. Identical to [`render`] except that a
/// half-received math span is withheld rather than shown as raw LaTeX that
/// snaps into a formula a frame later.
pub fn render_stream(md: &str) -> String {
    render_with(trim_incomplete_math(md), true)
}

fn trim_incomplete_math(md: &str) -> &str {
    let bytes = md.as_bytes();
    let mut i = 0;
    let mut open_at: Option<usize> = None;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'$' => {
                let delim = if bytes.get(i + 1) == Some(&b'$') { 2 } else { 1 };
                open_at = if open_at.is_some() { None } else { Some(i) };
                i += delim;
            }
            _ => i += 1,
        }
    }
    match open_at {
        Some(idx) if md.len() - idx <= MAX_OPEN_MATH_TAIL && md.is_char_boundary(idx) => &md[..idx],
        _ => md,
    }
}

/// Converts one math span. Malformed LaTeX falls back to the delimited
/// source as plain text, which push_html escapes.
fn render_math(latex: &str, mode: DisplayMode) -> Event<'static> {
    match math_span(latex, mode) {
        Some(html) => Event::InlineHtml(html.into()),
        None => {
            let fallback = match mode {
                DisplayMode::Block => format!("$${latex}$$"),
                DisplayMode::Inline => format!("${latex}$"),
            };
            Event::Text(fallback.into())
        }
    }
}

/// Inline math as HTML for diagram labels; None if the LaTeX doesn't parse.
pub fn inline_math(latex: &str) -> Option<String> {
    math_span(latex, DisplayMode::Inline)
}

fn math_span(latex: &str, mode: DisplayMode) -> Option<String> {
    let mathml = to_mathml(latex, mode)?;
    let class = match mode {
        DisplayMode::Block => "math-block",
        DisplayMode::Inline => "math-inline",
    };
    Some(format!(r#"<span class="{class}">{mathml}</span>"#))
}

fn to_mathml(latex: &str, mode: DisplayMode) -> Option<String> {
    let storage = Storage::new();
    // Collected up front so a parse error anywhere in the span rejects the
    // whole thing, rather than rendering a half-formed equation.
    let events = LatexParser::new(latex, &storage)
        .collect::<Result<Vec<_>, ParserError>>()
        .ok()?;

    let config = RenderConfig {
        display_mode: mode,
        xml: true,
        ..RenderConfig::default()
    };
    let mut out = String::new();
    push_mathml(&mut out, events.into_iter().map(Ok::<_, ParserError>), config).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{render, render_stream};

    /// Cases drawn from real generated steps. The first renderer used here
    /// (latex2mathml) returned Ok(..) with "[PARSE ERROR: ..]" embedded in
    /// the MathML, so a fallback keyed on Result alone shipped error text
    /// straight into the lesson.
    const SHOULD_RENDER: &[&str] = &[
        r"$w_k(x) = \frac{1}{\dfrac{\lVert x - x_k \rVert}{R_k} + \sqrt{1 - n(x)\cdot n(x_k)}}$",
        r"$E(x) \approx \frac{\sum_k w_k(x)\, E(x_k)}{\sum_k w_k(x)}$",
        r"$\lVert x - x_k\rVert$",
        r"$\dfrac{a}{b}$",
        r"$\int_0^\infty e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2}$",
        r"$\left( \frac{a}{b} \right)$",
        r"$\hat{n} \cdot \vec{\omega}_i$",
        r"$\mathbf{v} \times \mathbf{w}$",
        r"$$\begin{cases} 1 & x > 0 \\ 0 & x \le 0 \end{cases}$$",
        r"$$\begin{align} a &= b \\ c &= d \end{align}$$",
    ];

    #[test]
    fn renders_math_without_leaking_errors() {
        for case in SHOULD_RENDER {
            let html = render(case);
            assert!(html.contains("<math"), "expected MathML for {case}: {html}");
            assert!(
                !html.contains("PARSE ERROR") && !html.contains("merror"),
                "renderer leaked an error for {case}: {html}"
            );
        }
    }

    #[test]
    fn unparseable_math_falls_back_to_source() {
        let html = render(r"$\bogus{x}$");
        assert!(!html.contains("<math"), "expected no MathML: {html}");
        assert!(html.contains("bogus"), "source should survive: {html}");
    }

    #[test]
    fn incomplete_math_is_withheld_while_streaming() {
        assert!(!render_stream(r"The weight is $\frac{1}{2").contains("frac"));
        let literal = format!("It costs $5 {}", "and then some prose. ".repeat(12));
        assert!(render_stream(&literal).contains("prose"));
    }

    #[test]
    fn model_html_is_still_escaped() {
        let html = render("<script>alert(1)</script>");
        assert!(!html.contains("<script"), "raw HTML must not survive: {html}");
    }
}
