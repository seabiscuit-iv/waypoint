use latex2mathml::{latex_to_mathml, DisplayStyle};
use pulldown_cmark::{html, Event, Options, Parser};

/// Render markdown to HTML. Raw HTML in the source is escaped (emitted as
/// text) so model output can never inject markup into the webview.
///
/// `$…$` / `$$…$$` are converted to MathML here rather than in the webview:
/// it keeps the strict CSP intact (no math library, no external fonts) and
/// the emitted markup is ours, not the model's.
pub fn render(md: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_MATH);

    let parser = Parser::new_ext(md, opts).map(|ev| match ev {
        Event::Html(s) => Event::Text(s),
        Event::InlineHtml(s) => Event::Text(s),
        Event::InlineMath(s) => render_math(&s, DisplayStyle::Inline),
        Event::DisplayMath(s) => render_math(&s, DisplayStyle::Block),
        other => other,
    });

    let mut out = String::new();
    html::push_html(&mut out, parser);
    out
}

/// How far back a still-unclosed `$` is allowed to be before we assume it is
/// a literal dollar sign rather than math the model is mid-way through.
const MAX_OPEN_MATH_TAIL: usize = 160;

/// Render for a stream in progress. Identical to [`render`] except that a
/// half-received math span is withheld rather than shown as raw LaTeX that
/// snaps into a formula a frame later.
pub fn render_stream(md: &str) -> String {
    render(trim_incomplete_math(md))
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
fn render_math(latex: &str, style: DisplayStyle) -> Event<'static> {
    match latex_to_mathml(latex, style) {
        Ok(mathml) => {
            let markup = match style {
                DisplayStyle::Block => format!(r#"<span class="math-block">{mathml}</span>"#),
                DisplayStyle::Inline => format!(r#"<span class="math-inline">{mathml}</span>"#),
            };
            Event::InlineHtml(markup.into())
        }
        Err(_) => {
            let fallback = match style {
                DisplayStyle::Block => format!("$${latex}$$"),
                DisplayStyle::Inline => format!("${latex}$"),
            };
            Event::Text(fallback.into())
        }
    }
}
