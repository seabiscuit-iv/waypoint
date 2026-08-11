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
