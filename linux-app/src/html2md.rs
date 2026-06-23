//! HTML → Markdown conversion for captured Poe.com conversation pages (M3).
//!
//! The extension ships the raw rendered Poe DOM (see `docs/PROTOCOL.md` §5); this
//! module turns it into clean Obsidian Markdown body text. It emits NO YAML
//! frontmatter — that is the vault writer's job (M4). Image `src` URLs are kept
//! verbatim (M4 rewrites them to local asset paths) and collected for download.
//!
//! The public entry point is [`html_to_markdown`]; it never panics on malformed
//! input.

use std::collections::HashSet;
use std::fmt::Write as _;

use scraper::{ElementRef, Html, Node, Selector};

// ---------------------------------------------------------------------------
// Poe-DOM-dependent selectors. CENTRALIZED here on purpose: Poe ships CSS-module
// hashed class names (e.g. `ChatMessage_chatMessage__xkgHx`), so we match on
// substrings/prefixes. These were DERIVED FROM REAL 2026-06 CAPTURES of
// poe.com conversation pages (see `tests/fixtures/poe_real_sample.html`); they
// will drift if Poe reships its frontend and must be re-verified against fresh
// captures. (See PROTOCOL.md §5.)
// ---------------------------------------------------------------------------

/// Outer wrapper around all chat messages. Used only as a sanity anchor.
const SEL_MESSAGES_VIEW: &str = r#"[class*="ChatMessagesView"]"#;
/// Primary per-message block selector. Real class e.g.
/// `ChatMessage_chatMessage__xkgHx`; ~one per message in document order.
const SEL_CHAT_MESSAGE: &str = r#"[class*="ChatMessage_chatMessage"]"#;
/// Fallback message-row selector (one row may wrap one chat message).
const SEL_MESSAGE_ROW: &str = r#"[class*="Message_row"]"#;
/// Last-resort message selector by stable id attribute / prefix.
const SEL_MESSAGE_ID_ATTR: &str = r#"[data-message-id]"#;
const SEL_MESSAGE_ID_PREFIX: &str = r#"div[id^="message-"]"#;

/// ROLE marker (HUMAN): a human message's block contains a descendant/self with
/// a class containing this substring. Real class e.g.
/// `ChatMessage_rightSideMessageWrapper__r0roB`.
const SEL_ROLE_HUMAN: &str = r#"[class*="rightSideMessage"]"#;
/// ROLE marker (BOT): a bot message's block contains a descendant with a class
/// containing this substring. Real class e.g. `BotMessageHeader_wrapper__gvvdw`.
const SEL_ROLE_BOT: &str = r#"[class*="BotMessageHeader"]"#;

/// Inner prose/markdown container holding the actual rendered message body.
/// Real class e.g. `Prose_presets_prose__0keUd`. We render ONLY from here to
/// exclude Poe chrome (action bars, buttons, headers, follow-up actions).
const SEL_PROSE_BODY: &str = r#"[class*="Prose_presets_prose"]"#;

/// The text container inside a bot header; its first `<p>` holds the bot's
/// display name. Real class e.g. `BotHeader_textContainer__kVf_I`. A sibling
/// `BotHeader_subText` `<p>` (e.g. "Private") must be excluded.
const SEL_BOT_NAME_CONTAINER: &str = r#"[class*="BotHeader_textContainer"]"#;
/// Sub-text under a bot name (e.g. "Private"/"Official") — chrome, not the name.
const SEL_BOT_SUBTEXT: &str = r#"[class*="BotHeader_subText"]"#;

// --- Code-block selectors (real class prefixes from 2026-06 captures) -------

/// A fenced/syntax-highlighted code block container.
/// Real class e.g. `MarkdownCodeBlock_container__nRn2j`.
const SEL_CODE_BLOCK: &str = r#"[class*="MarkdownCodeBlock_container"]"#;
/// The language label inside a code block, e.g. `MarkdownCodeBlock_languageName__…`.
const SEL_CODE_LANG: &str = r#"[class*="MarkdownCodeBlock_languageName"]"#;

/// Host fragment identifying Poe-hosted assets we should download (M4).
const ASSET_HOST_MARKER: &str = "poecdn";

/// The role of a single conversation message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Human,
    Bot,
}

/// A converted Poe conversation, ready for the vault writer (M4).
#[derive(Debug, Clone, Default)]
pub struct Conversation {
    /// Title from `<title>`/`<h1>`, else the provided fallback.
    pub title: String,
    /// The conversation body as Obsidian Markdown (no frontmatter).
    pub markdown: String,
    /// Deduped poecdn image/attachment URLs found, in first-seen order.
    pub asset_urls: Vec<String>,
}

/// Convert captured Poe page HTML into Markdown. Never panics on malformed input.
pub fn html_to_markdown(html: &str, fallback_title: &str) -> Conversation {
    let doc = Html::parse_document(html);

    let title = extract_title(&doc).unwrap_or_else(|| fallback_title.to_string());

    let mut collector = AssetCollector::default();
    let mut sections: Vec<String> = Vec::new();

    for msg in find_message_blocks(&doc) {
        let role = detect_role(&msg);
        let body = render_message_body(&msg, &mut collector);
        let body = body.trim();
        if body.is_empty() {
            continue;
        }
        let header = match role {
            Role::Human => "### 🧑 You".to_string(),
            Role::Bot => {
                let name = extract_bot_name(&msg).unwrap_or_else(|| "Assistant".to_string());
                format!("### 🤖 {name}")
            }
        };
        sections.push(format!("{header}\n\n{body}"));
    }

    let markdown = sections.join("\n\n");

    Conversation {
        title,
        markdown,
        asset_urls: collector.into_urls(),
    }
}

// ---------------------------------------------------------------------------
// Title + message discovery
// ---------------------------------------------------------------------------

fn extract_title(doc: &Html) -> Option<String> {
    // Prefer <title>, then the first <h1>. Strip a trailing " - Poe"/" | Poe".
    if let Some(t) = first_text(doc, "title") {
        let t = strip_poe_suffix(&t);
        if !t.is_empty() {
            return Some(t);
        }
    }
    if let Some(h) = first_text(doc, "h1") {
        let h = h.trim().to_string();
        if !h.is_empty() {
            return Some(h);
        }
    }
    None
}

fn strip_poe_suffix(s: &str) -> String {
    let s = s.trim();
    for suffix in [" - Poe", " | Poe", " — Poe"] {
        if let Some(stripped) = s.strip_suffix(suffix) {
            return stripped.trim().to_string();
        }
    }
    s.to_string()
}

fn first_text(doc: &Html, css: &str) -> Option<String> {
    let sel = Selector::parse(css).ok()?;
    let el = doc.select(&sel).next()?;
    let text: String = el.text().collect::<String>();
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Find ordered message blocks. Tries the most specific selector first and only
/// falls back when it yields nothing, to avoid double-counting nested matches.
/// Searches inside the `ChatMessagesView` container when present (to avoid
/// matching stray off-conversation elements), else the whole document.
fn find_message_blocks(doc: &Html) -> Vec<ElementRef<'_>> {
    let root: ElementRef<'_> = Selector::parse(SEL_MESSAGES_VIEW)
        .ok()
        .and_then(|sel| doc.select(&sel).next())
        .unwrap_or_else(|| doc.root_element());

    for css in [
        SEL_CHAT_MESSAGE,
        SEL_MESSAGE_ROW,
        SEL_MESSAGE_ID_ATTR,
        SEL_MESSAGE_ID_PREFIX,
    ] {
        if let Ok(sel) = Selector::parse(css) {
            let found: Vec<ElementRef<'_>> = root.select(&sel).collect();
            if !found.is_empty() {
                return dedupe_nested(found);
            }
        }
    }
    Vec::new()
}

/// Remove elements that are descendants of another element already in the list
/// (document order is preserved by `scraper::select`).
fn dedupe_nested<'a>(els: Vec<ElementRef<'a>>) -> Vec<ElementRef<'a>> {
    let ids: HashSet<_> = els.iter().map(|e| e.id()).collect();
    els.into_iter()
        .filter(|e| {
            // Keep `e` unless one of its ancestors is also a selected element.
            !e.ancestors().any(|a| {
                ElementRef::wrap(a)
                    .map(|ae| ae.id() != e.id() && ids.contains(&ae.id()))
                    .unwrap_or(false)
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Role + bot name detection
// ---------------------------------------------------------------------------

/// Detect the role of a message block from its real Poe DOM structure.
///
/// Rule (verified 58/58 on a real capture): a HUMAN block contains a
/// descendant/self matching [`SEL_ROLE_HUMAN`] (`rightSideMessage…`); a BOT
/// block contains a descendant matching [`SEL_ROLE_BOT`] (`BotMessageHeader…`).
/// Human takes precedence; if neither is present we default to Bot.
fn detect_role(msg: &ElementRef<'_>) -> Role {
    // `SEL_ROLE_BOT` is not consulted explicitly: anything without the human
    // marker is treated as a bot (the documented default), which also covers the
    // `BotMessageHeader` case.
    if has_descendant_or_self(msg, SEL_ROLE_HUMAN) {
        Role::Human
    } else {
        // Bot blocks normally carry `SEL_ROLE_BOT` (`BotMessageHeader`); even
        // when that marker is absent we default to Bot per the documented rule.
        Role::Bot
    }
}

/// True if `msg` is a recognized bot block (carries the `BotMessageHeader`
/// marker, [`SEL_ROLE_BOT`]). A message can be the Bot role by default without
/// this marker, so this is a stronger "is a real bot header present" check.
fn is_bot_block(msg: &ElementRef<'_>) -> bool {
    has_descendant_or_self(msg, SEL_ROLE_BOT)
}

/// True if `msg` itself or any descendant matches `css`.
fn has_descendant_or_self(msg: &ElementRef<'_>, css: &str) -> bool {
    let Ok(sel) = Selector::parse(css) else {
        return false;
    };
    sel.matches(msg) || msg.select(&sel).next().is_some()
}

/// Extract the bot's display name from `BotHeader_textContainer`'s first `<p>`,
/// explicitly skipping the `BotHeader_subText` line (e.g. "Private").
fn extract_bot_name(msg: &ElementRef<'_>) -> Option<String> {
    // Only trust a name when a genuine bot header is present; otherwise the
    // caller falls back to "Assistant".
    if !is_bot_block(msg) {
        return None;
    }
    let sel = Selector::parse(SEL_BOT_NAME_CONTAINER).ok()?;
    let sub_sel = Selector::parse(SEL_BOT_SUBTEXT).ok();
    let p_sel = Selector::parse(":scope > p").ok()?;

    let container = msg.select(&sel).next()?;
    for p in container.select(&p_sel) {
        // Skip the sub-text paragraph if this <p> is (or is inside) one.
        let is_sub = sub_sel.as_ref().is_some_and(|s| s.matches(&p));
        if is_sub {
            continue;
        }
        let name: String = p.text().collect::<String>().trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Asset collection
// ---------------------------------------------------------------------------

#[derive(Default)]
struct AssetCollector {
    seen: HashSet<String>,
    urls: Vec<String>,
}

impl AssetCollector {
    fn add(&mut self, url: &str) {
        if url.contains(ASSET_HOST_MARKER) && self.seen.insert(url.to_string()) {
            self.urls.push(url.to_string());
        }
    }

    fn into_urls(self) -> Vec<String> {
        self.urls
    }
}

// ---------------------------------------------------------------------------
// Inner HTML → Markdown rendering
// ---------------------------------------------------------------------------

/// Render a message block to Markdown from its prose container only, so Poe
/// chrome (action bars, Copy buttons, headers, follow-up actions) never leaks.
/// Falls back to the whole block if no prose container is present.
fn render_message_body(msg: &ElementRef<'_>, assets: &mut AssetCollector) -> String {
    let root = Selector::parse(SEL_PROSE_BODY)
        .ok()
        .and_then(|sel| msg.select(&sel).next())
        .unwrap_or(*msg);

    let mut out = String::new();
    render_children(&root, &mut out, assets);
    normalize_blank_lines(&out)
}

/// Render the child nodes of `el` as block-level Markdown.
fn render_children(el: &ElementRef<'_>, out: &mut String, assets: &mut AssetCollector) {
    for child in el.children() {
        match child.value() {
            Node::Element(_) => {
                if let Some(c) = ElementRef::wrap(child) {
                    render_block(&c, out, assets);
                }
            }
            Node::Text(t) => {
                let s = collapse_ws(t);
                if !s.trim().is_empty() {
                    out.push_str(s.trim());
                    out.push_str("\n\n");
                }
            }
            _ => {}
        }
    }
}

/// Render a single block-level element.
fn render_block(el: &ElementRef<'_>, out: &mut String, assets: &mut AssetCollector) {
    // A Poe `MarkdownCodeBlock` container is a `<div>` wrapping the code along
    // with chrome (language label, Copy/expand buttons, header, footer). Handle
    // it before generic dispatch so NONE of that chrome can leak into the body.
    if matches_css(el, SEL_CODE_BLOCK) {
        render_code_block(el, out);
        return;
    }

    let name = el.value().name();
    match name {
        "p" => {
            let inline = render_inline(el, assets);
            if !inline.trim().is_empty() {
                out.push_str(inline.trim());
                out.push_str("\n\n");
            }
        }
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level = name[1..].parse::<usize>().unwrap_or(1);
            let inline = render_inline(el, assets);
            let _ = writeln!(out, "{} {}\n", "#".repeat(level), inline.trim());
        }
        "pre" => {
            render_pre(el, out);
        }
        "ul" | "ol" => {
            render_list(el, out, assets, 0);
            out.push('\n');
        }
        "table" => {
            render_table(el, out, assets);
        }
        "blockquote" => {
            let mut inner = String::new();
            render_children(el, &mut inner, assets);
            for line in normalize_blank_lines(&inner).lines() {
                if line.is_empty() {
                    out.push_str(">\n");
                } else {
                    let _ = writeln!(out, "> {line}");
                }
            }
            out.push('\n');
        }
        "br" => out.push('\n'),
        "hr" => out.push_str("---\n\n"),
        // Inline-level elements appearing directly under a block container (e.g.
        // a bare <img> or <a> child of the Markdown container): render inline,
        // then close the paragraph so block flow stays intact.
        "img" | "a" | "strong" | "em" | "b" | "i" | "code" => {
            let mut inline = String::new();
            render_inline_element(el, &mut inline, assets);
            if !inline.trim().is_empty() {
                out.push_str(inline.trim());
                out.push_str("\n\n");
            }
        }
        // Container/unknown blocks: recurse to find renderable content.
        _ => render_children(el, out, assets),
    }
}

/// True if `el`'s own class attribute matches the substring selector `css`.
fn matches_css(el: &ElementRef<'_>, css: &str) -> bool {
    Selector::parse(css).map(|s| s.matches(el)).unwrap_or(false)
}

/// Render a Poe `MarkdownCodeBlock_container` as a fenced code block.
///
/// The code text is the concatenated TEXT of the inner `<code>` element (its
/// `hljs-*` highlight spans contribute only their text). The language comes from
/// the `MarkdownCodeBlock_languageName` label, falling back to the inner code's
/// `language-XXX` class. All other descendants (Copy/expand `Button_*`, code
/// header/footer) are ignored, so their text can never appear in the output.
fn render_code_block(el: &ElementRef<'_>, out: &mut String) {
    let code_el = Selector::parse("code")
        .ok()
        .and_then(|s| el.select(&s).next());

    let lang = code_block_language(el, code_el.as_ref());
    let source = match code_el {
        Some(code) => raw_text(&code),
        None => String::new(),
    };

    let code = source.trim_matches('\n');
    let _ = writeln!(out, "```{lang}");
    out.push_str(code);
    out.push('\n');
    out.push_str("```\n\n");
}

/// Resolve a code block's language: prefer the `languageName` label, else the
/// inner `<code class="language-XXX">` class.
fn code_block_language(container: &ElementRef<'_>, code: Option<&ElementRef<'_>>) -> String {
    if let Ok(sel) = Selector::parse(SEL_CODE_LANG) {
        if let Some(label) = container.select(&sel).next() {
            let name: String = label.text().collect::<String>().trim().to_string();
            if !name.is_empty() {
                return name;
            }
        }
    }
    code.map(|c| language_of(c)).unwrap_or_default()
}

/// Render a bare `<pre>` (outside a Poe `MarkdownCodeBlock`, optionally wrapping
/// `<code class="language-xxx">`) as a fenced block, preserving the exact text.
fn render_pre(el: &ElementRef<'_>, out: &mut String) {
    let code_sel = Selector::parse("code").ok();
    let code_el = code_sel.as_ref().and_then(|s| el.select(s).next());

    let (lang, source) = match code_el {
        Some(code) => (language_of(&code), raw_text(&code)),
        None => (String::new(), raw_text(el)),
    };

    let code = source.trim_end_matches('\n');
    let _ = writeln!(out, "```{lang}");
    out.push_str(code);
    out.push('\n');
    out.push_str("```\n\n");
}

/// Extract the language from a code element's `language-xxx`/`lang-xxx` class.
fn language_of(code: &ElementRef<'_>) -> String {
    let classes = code.value().attr("class").unwrap_or("");
    for cls in classes.split_whitespace() {
        for prefix in ["language-", "lang-"] {
            if let Some(rest) = cls.strip_prefix(prefix) {
                if !rest.is_empty() {
                    return rest.to_string();
                }
            }
        }
    }
    String::new()
}

/// Concatenated raw text of an element (no entity re-escaping; `scraper` already
/// decoded entities). Preserves newlines/whitespace for code fidelity.
fn raw_text(el: &ElementRef<'_>) -> String {
    el.text().collect::<String>()
}

/// Render an `<ol>`/`<ul>` as Markdown, supporting nesting via `depth`.
fn render_list(el: &ElementRef<'_>, out: &mut String, assets: &mut AssetCollector, depth: usize) {
    let ordered = el.value().name() == "ol";
    let indent = "  ".repeat(depth);

    let li_sel = match Selector::parse(":scope > li") {
        Ok(s) => s,
        Err(_) => return,
    };

    for (idx, li) in (1usize..).zip(el.select(&li_sel)) {
        let marker = if ordered {
            format!("{idx}. ")
        } else {
            "- ".to_string()
        };

        // Inline text of the <li> itself (excluding nested lists).
        let mut inline = String::new();
        let mut nested = String::new();
        for child in li.children() {
            match child.value() {
                Node::Text(t) => inline.push_str(&collapse_ws(t)),
                Node::Element(_) => {
                    if let Some(c) = ElementRef::wrap(child) {
                        let cn = c.value().name();
                        if cn == "ul" || cn == "ol" {
                            render_list(&c, &mut nested, assets, depth + 1);
                        } else {
                            render_inline_element(&c, &mut inline, assets);
                        }
                    }
                }
                _ => {}
            }
        }

        let inline = inline.trim();
        let _ = writeln!(out, "{indent}{marker}{inline}");
        if !nested.is_empty() {
            out.push_str(nested.trim_end_matches('\n'));
            out.push('\n');
        }
    }
}

/// Render an HTML `<table>` as a GitHub-flavored Markdown table.
fn render_table(el: &ElementRef<'_>, out: &mut String, assets: &mut AssetCollector) {
    let tr_sel = match Selector::parse("tr") {
        Ok(s) => s,
        Err(_) => return,
    };
    let cell_sel = match Selector::parse("th, td") {
        Ok(s) => s,
        Err(_) => return,
    };

    let mut rows: Vec<Vec<String>> = Vec::new();
    for tr in el.select(&tr_sel) {
        let cells: Vec<String> = tr
            .select(&cell_sel)
            .map(|c| render_inline(&c, assets).trim().replace('|', "\\|"))
            .collect();
        if !cells.is_empty() {
            rows.push(cells);
        }
    }
    if rows.is_empty() {
        return;
    }

    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let header = &rows[0];
    let _ = writeln!(out, "| {} |", pad_row(header, cols).join(" | "));
    let sep: Vec<&str> = (0..cols).map(|_| "---").collect();
    let _ = writeln!(out, "| {} |", sep.join(" | "));
    for row in &rows[1..] {
        let _ = writeln!(out, "| {} |", pad_row(row, cols).join(" | "));
    }
    out.push('\n');
}

fn pad_row(row: &[String], cols: usize) -> Vec<String> {
    let mut r: Vec<String> = row.to_vec();
    while r.len() < cols {
        r.push(String::new());
    }
    r
}

// ---------------------------------------------------------------------------
// Inline rendering
// ---------------------------------------------------------------------------

/// Render the inline content of an element (bold/italic/code/links/images/text).
fn render_inline(el: &ElementRef<'_>, assets: &mut AssetCollector) -> String {
    let mut out = String::new();
    render_inline_children(el, &mut out, assets);
    out
}

/// Render all child nodes of `el` as inline Markdown.
fn render_inline_children(el: &ElementRef<'_>, out: &mut String, assets: &mut AssetCollector) {
    for child in el.children() {
        match child.value() {
            Node::Text(t) => out.push_str(&collapse_ws(t)),
            Node::Element(_) => {
                if let Some(c) = ElementRef::wrap(child) {
                    render_inline_element(&c, out, assets);
                }
            }
            _ => {}
        }
    }
}

fn render_inline_element(el: &ElementRef<'_>, out: &mut String, assets: &mut AssetCollector) {
    match el.value().name() {
        "strong" | "b" => {
            let inner = render_inline(el, assets);
            if !inner.trim().is_empty() {
                let _ = write!(out, "**{}**", inner.trim());
            }
        }
        "em" | "i" => {
            let inner = render_inline(el, assets);
            if !inner.trim().is_empty() {
                let _ = write!(out, "*{}*", inner.trim());
            }
        }
        "code" => {
            // Inline code: use raw text, do not recurse into markup.
            let txt = raw_text(el);
            let _ = write!(out, "`{txt}`");
        }
        "a" => {
            let href = el.value().attr("href").unwrap_or("");
            let text = render_inline(el, assets);
            let text = text.trim();
            if href.is_empty() {
                out.push_str(text);
            } else {
                let _ = write!(out, "[{text}]({href})");
            }
        }
        "img" => {
            let src = el.value().attr("src").unwrap_or("");
            let alt = el.value().attr("alt").unwrap_or("");
            if !src.is_empty() {
                let _ = write!(out, "![{alt}]({src})");
                assets.add(src);
            }
        }
        "br" => out.push('\n'),
        // Inline-level wrappers / unknowns: recurse.
        _ => render_inline_children(el, out, assets),
    }
}

// ---------------------------------------------------------------------------
// Whitespace helpers
// ---------------------------------------------------------------------------

/// Collapse runs of ASCII whitespace (incl. newlines) to single spaces, the way
/// a browser renders inline text. Used for non-code text only.
fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out
}

/// Collapse 3+ consecutive newlines down to a single blank line, and trim.
fn normalize_blank_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut newlines = 0usize;
    for ch in s.chars() {
        if ch == '\n' {
            newlines += 1;
            if newlines <= 2 {
                out.push('\n');
            }
        } else {
            newlines = 0;
            out.push(ch);
        }
    }
    out.trim().to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // SYNTHETIC fixture (uses the REAL Poe marker classes) for rendering paths
    // not present in the trimmed real capture: tables, blockquotes, nested
    // lists, links, `<pre><code>`.
    const SAMPLE: &str = include_str!("../tests/fixtures/poe_sample.html");

    // REAL fixture: a representative slice hand-trimmed from a 2026-06 poe.com
    // capture (one human + one bot message with a python code block, a list,
    // and an injected prose image). Ground truth for selectors.
    const REAL: &str = include_str!("../tests/fixtures/poe_real_sample.html");

    fn convert() -> Conversation {
        html_to_markdown(SAMPLE, "fallback")
    }

    #[test]
    fn title_extraction_strips_poe_suffix() {
        let c = convert();
        assert_eq!(c.title, "Rust async patterns");
    }

    #[test]
    fn title_falls_back_when_missing() {
        let c = html_to_markdown(
            "<html><body><p>no title here</p></body></html>",
            "MyFallback",
        );
        assert_eq!(c.title, "MyFallback");
    }

    #[test]
    fn messages_appear_in_order_with_role_headers() {
        let c = convert();
        let human = c.markdown.find("### 🧑 You").expect("human header");
        let bot = c.markdown.find("### 🤖").expect("bot header");
        assert!(human < bot, "human message must come before bot message");
    }

    #[test]
    fn bot_name_is_used_when_available() {
        let c = convert();
        assert!(
            c.markdown.contains("### 🤖 Claude-Sonnet"),
            "expected bot name header, got:\n{}",
            c.markdown
        );
        // The BotHeader_subText line ("Official") must NOT be in the name.
        assert!(!c.markdown.contains("Claude-Sonnetofficial"));
        assert!(!c.markdown.contains("### 🤖 Official"));
    }

    // -----------------------------------------------------------------------
    // REAL-capture assertions (poe_real_sample.html).
    // -----------------------------------------------------------------------

    #[test]
    fn real_roles_and_order() {
        let c = html_to_markdown(REAL, "fallback");
        let human = c.markdown.find("### 🧑 You").expect("human header");
        let bot = c
            .markdown
            .find("### 🤖 Claude-Sonnet-3.5")
            .expect("bot header with real name");
        assert!(human < bot, "human must precede bot\n{}", c.markdown);
        // Exactly one of each in this two-message slice.
        assert_eq!(c.markdown.matches("### 🧑 You").count(), 1);
        assert_eq!(c.markdown.matches("### 🤖").count(), 1);
        // The bot sub-text ("Private") must not leak into the header.
        assert!(!c.markdown.contains("### 🤖 Claude-Sonnet-3.5Private"));
    }

    #[test]
    fn real_blocks_classify_as_human_then_bot() {
        let doc = Html::parse_document(REAL);
        let blocks = find_message_blocks(&doc);
        assert_eq!(blocks.len(), 2, "expected exactly two message blocks");
        assert_eq!(detect_role(&blocks[0]), Role::Human);
        assert_eq!(detect_role(&blocks[1]), Role::Bot);
        // The first block is NOT a bot block; the second one is.
        assert!(!is_bot_block(&blocks[0]));
        assert!(is_bot_block(&blocks[1]));
    }

    #[test]
    fn real_code_block_is_fenced_with_language_and_exact_code() {
        let c = html_to_markdown(REAL, "fallback");
        assert!(
            c.markdown.contains("```python"),
            "fenced python block missing:\n{}",
            c.markdown
        );
        // Exact code text (hljs spans contributed only their text).
        assert!(
            c.markdown.contains("def count_vowels(s):"),
            "exact code missing:\n{}",
            c.markdown
        );
        assert!(c.markdown.contains("    vowels = 'aeiouAEIOU'"));
        assert!(c.markdown.contains("result = count_vowels(string)"));
    }

    #[test]
    fn real_output_has_no_chrome_or_class_noise() {
        let c = html_to_markdown(REAL, "fallback");
        for noise in [
            "hljs",
            "MarkdownCodeBlock",
            "Button_",
            "Prose_presets",
            "BotMessageHeader",
            "Copy",
            "languageName",
            "rightSideMessage",
        ] {
            assert!(
                !c.markdown.contains(noise),
                "chrome/class noise {noise:?} leaked into output:\n{}",
                c.markdown
            );
        }
    }

    #[test]
    fn real_image_url_is_collected() {
        let c = html_to_markdown(REAL, "fallback");
        let url = "https://qph.cf2.poecdn.net/main-thumb-sample-diagram.png";
        assert!(
            c.markdown
                .contains(&format!("![word count diagram]({url})")),
            "prose image missing:\n{}",
            c.markdown
        );
        assert!(c.asset_urls.contains(&url.to_string()));
        // The bot-header AVATAR (outside the prose body) must NOT be collected.
        assert!(
            !c.asset_urls.iter().any(|u| u.contains("main-thumb-pb")),
            "avatar leaked into assets: {:?}",
            c.asset_urls
        );
    }

    #[test]
    fn real_list_renders_in_bot_body() {
        let c = html_to_markdown(REAL, "fallback");
        assert!(
            c.markdown.contains("- "),
            "expected a bulleted list in the bot body:\n{}",
            c.markdown
        );
    }

    #[test]
    fn human_message_inline_formatting() {
        let c = convert();
        assert!(c.markdown.contains("**async**"));
        assert!(c.markdown.contains("*Rust*"));
    }

    #[test]
    fn code_block_is_fenced_with_language_and_exact_code() {
        let c = convert();
        assert!(c.markdown.contains("```rust"), "fenced rust block missing");
        // Exact code text, with entities decoded.
        assert!(
            c.markdown
                .contains("async fn fetch(url: &str) -> Result<String> {"),
            "exact code text missing:\n{}",
            c.markdown
        );
        assert!(c
            .markdown
            .contains("    let body = reqwest::get(url).await?.text().await?;"));
    }

    #[test]
    fn table_becomes_gfm_table() {
        let c = convert();
        assert!(c.markdown.contains("| Runtime | Notes |"));
        assert!(c.markdown.contains("| --- | --- |"));
        assert!(c.markdown.contains("| tokio | Most popular |"));
        assert!(c.markdown.contains("| async-std | std-like API |"));
    }

    #[test]
    fn lists_render_with_nesting() {
        let c = convert();
        assert!(c.markdown.contains("- Use `.await` on futures"));
        assert!(c.markdown.contains("- Pick a runtime:"));
        // Nested ordered list indented under the bullet.
        assert!(
            c.markdown.contains("  1. tokio for most apps"),
            "nested ordered list missing:\n{}",
            c.markdown
        );
    }

    #[test]
    fn blockquote_renders() {
        let c = convert();
        assert!(c.markdown.contains("> Remember: futures are lazy in Rust."));
    }

    #[test]
    fn link_renders() {
        let c = convert();
        assert!(c.markdown.contains("[tokio site](https://tokio.rs)"));
    }

    #[test]
    fn image_keeps_original_url_and_is_collected() {
        let c = convert();
        let url = "https://qph.cf2.poecdn.net/main-thumb-async-diagram.png";
        assert!(c.markdown.contains(&format!("![async diagram]({url})")));
        assert_eq!(c.asset_urls, vec![url.to_string()]);
    }

    #[test]
    fn asset_urls_are_deduped() {
        let html = r#"<html><body>
            <div class="ChatMessage_chatMessage__x Message_row__Bot__y">
              <div class="Markdown_markdownContainer__z">
                <p><img src="https://x.poecdn.net/a.png" alt="a"></p>
                <p><img src="https://x.poecdn.net/a.png" alt="a again"></p>
                <p><img src="https://x.poecdn.net/b.png" alt="b"></p>
              </div>
            </div>
        </body></html>"#;
        let c = html_to_markdown(html, "f");
        assert_eq!(
            c.asset_urls,
            vec![
                "https://x.poecdn.net/a.png".to_string(),
                "https://x.poecdn.net/b.png".to_string(),
            ]
        );
    }

    #[test]
    fn non_poecdn_images_are_not_collected_but_still_rendered() {
        let html = r#"<html><body>
            <div class="ChatMessage_chatMessage__x Message_row__Bot__y">
              <div class="Markdown_markdownContainer__z">
                <p><img src="https://example.com/c.png" alt="c"></p>
              </div>
            </div>
        </body></html>"#;
        let c = html_to_markdown(html, "f");
        assert!(c.markdown.contains("![c](https://example.com/c.png)"));
        assert!(c.asset_urls.is_empty());
    }

    #[test]
    fn empty_input_does_not_panic_and_is_empty() {
        let c = html_to_markdown("", "fallback");
        assert_eq!(c.title, "fallback");
        assert!(c.markdown.trim().is_empty());
        assert!(c.asset_urls.is_empty());
    }

    #[test]
    fn non_html_input_does_not_panic() {
        let c = html_to_markdown("just some random text \0\u{1}not html", "fb");
        assert_eq!(c.title, "fb");
        assert!(c.asset_urls.is_empty());
    }

    #[test]
    fn html_without_messages_yields_empty_markdown() {
        let c = html_to_markdown(
            "<html><head><title>X - Poe</title></head><body><nav>menu</nav></body></html>",
            "fb",
        );
        assert_eq!(c.title, "X");
        assert!(c.markdown.trim().is_empty());
    }

    #[test]
    fn messages_view_anchor_selector_is_valid() {
        // Guard: the centralized anchor selector must always parse.
        assert!(Selector::parse(SEL_MESSAGES_VIEW).is_ok());
    }
}
