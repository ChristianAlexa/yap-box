use regex::Regex;
use std::sync::OnceLock;

fn cached_re(pattern: &'static str) -> &'static Regex {
    static CACHE: OnceLock<std::sync::Mutex<std::collections::HashMap<&'static str, &'static Regex>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut map = cache.lock().unwrap();
    if let Some(re) = map.get(pattern) {
        return re;
    }
    let compiled: &'static Regex = Box::leak(Box::new(Regex::new(pattern).unwrap()));
    map.insert(pattern, compiled);
    compiled
}

pub fn strip_markdown(text: &str) -> String {
    let mut out: String = text.to_string();

    out = cached_re(r"(?s)```[^\n]*\n.*?\n[ \t]*```[ \t]*").replace_all(&out, "").into_owned();
    out = cached_re(r"(?s)~~~[^\n]*\n.*?\n[ \t]*~~~[ \t]*").replace_all(&out, "").into_owned();

    out = cached_re(r"(?m)^[ \t]*>[ \t]?").replace_all(&out, "").into_owned();

    out = cached_re(r"(?m)^[ \t]*[-*_]{3,}[ \t]*$").replace_all(&out, "").into_owned();

    out = cached_re(r"(?m)^[ \t]*#{1,6}[ \t]+").replace_all(&out, "").into_owned();

    out = cached_re(r"!\[([^\]]*)\]\([^)]*\)").replace_all(&out, "$1").into_owned();
    out = cached_re(r"\[([^\]]*)\]\([^)]*\)").replace_all(&out, "$1").into_owned();

    out = cached_re(r"\*\*\*([^*\n]+?)\*\*\*").replace_all(&out, "$1").into_owned();
    out = cached_re(r"___([^_\n]+?)___").replace_all(&out, "$1").into_owned();
    out = cached_re(r"\*\*([^*\n]+?)\*\*").replace_all(&out, "$1").into_owned();
    out = cached_re(r"__([^_\n]+?)__").replace_all(&out, "$1").into_owned();
    out = cached_re(r"\*([^*\n]+?)\*").replace_all(&out, "$1").into_owned();

    // Underscore italic: only at word boundaries so snake_case survives.
    // JS used lookahead `(?=\W|$)`; regex crate has no lookahead, so we
    // capture the trailing non-word char (or end) and re-emit it.
    out = cached_re(r"(^|\W)_([^_\n]+?)_(\W|$)").replace_all(&out, "$1$2$3").into_owned();

    out = cached_re(r"`([^`]*)`").replace_all(&out, "$1").into_owned();

    out = cached_re(r"(?m)^[ \t]*[-*+][ \t]+").replace_all(&out, "").into_owned();
    out = cached_re(r"(?m)^[ \t]*\d+\.[ \t]+").replace_all(&out, "").into_owned();

    out = cached_re(r"\n{3,}").replace_all(&out, "\n\n").into_owned();

    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::strip_markdown;

    #[test]
    fn plain_text_is_identity() {
        assert_eq!(strip_markdown("Hello world."), "Hello world.");
    }

    #[test]
    fn atx_headings_lose_markers() {
        assert_eq!(strip_markdown("# Chapter One"), "Chapter One");
        assert_eq!(strip_markdown("### Section 3"), "Section 3");
    }

    #[test]
    fn bold_and_italic_removed() {
        assert_eq!(
            strip_markdown("This is **bold** and *italic*."),
            "This is bold and italic."
        );
        assert_eq!(
            strip_markdown("__also bold__ and _also italic_."),
            "also bold and also italic."
        );
    }

    #[test]
    fn fenced_code_block_removed_entirely() {
        let input = "Before the code.\n\n```python\nprint(\"x\")\n```\n\nAfter the code.";
        let out = strip_markdown(input);
        assert!(out.contains("Before the code."));
        assert!(out.contains("After the code."));
        assert!(!out.contains("print"), "python body leaked: {out}");
        assert!(!out.contains("```"), "fence markers leaked: {out}");
        assert!(!out.contains('`'), "stray backtick: {out}");
    }

    #[test]
    fn inline_code_stripped() {
        assert_eq!(strip_markdown("Use the `yap` tool."), "Use the yap tool.");
    }

    #[test]
    fn nested_bulleted_list_keeps_text() {
        let input = "- outer one\n  - inner\n- outer two";
        let out = strip_markdown(input);
        assert!(out.contains("outer one"));
        assert!(out.contains("inner"));
        assert!(out.contains("outer two"));
        for line in out.lines() {
            assert!(!line.starts_with('-'), "bullet survived: {line}");
        }
    }

    #[test]
    fn link_becomes_text() {
        assert_eq!(
            strip_markdown("See [the docs](https://example.com)."),
            "See the docs."
        );
    }

    #[test]
    fn image_keeps_alt() {
        assert_eq!(strip_markdown("![a cat](cat.png)"), "a cat");
    }

    #[test]
    fn block_quote_stripped() {
        assert_eq!(strip_markdown("> quoted thought"), "quoted thought");
    }

    #[test]
    fn numbered_list_markers_stripped() {
        let input = "1. first\n2. second\n3. third";
        let out = strip_markdown(input);
        assert!(out.contains("first"));
        assert!(out.contains("second"));
        assert!(out.contains("third"));
        assert!(
            !regex::Regex::new(r"\d\.").unwrap().is_match(&out),
            "numbered marker remains: {out}"
        );
    }

    #[test]
    fn snake_case_not_mangled() {
        assert_eq!(
            strip_markdown("Edit yap_spec.md and read it."),
            "Edit yap_spec.md and read it."
        );
    }
}
