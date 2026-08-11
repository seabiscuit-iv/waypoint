use pulldown_cmark::{html, Event, Options, Parser};

/// Render markdown to HTML. Raw HTML in the source is escaped (emitted as
/// text) so model output can never inject markup into the webview.
pub fn render(md: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_FOOTNOTES);

    let parser = Parser::new_ext(md, opts).map(|ev| match ev {
        Event::Html(s) => Event::Text(s),
        Event::InlineHtml(s) => Event::Text(s),
        other => other,
    });

    let mut out = String::new();
    html::push_html(&mut out, parser);
    out
}
