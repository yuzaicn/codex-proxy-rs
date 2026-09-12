use gateway_admin::workers::intelligence_detection::{extract_html_document, matched_phrases};

#[test]
fn matches_degraded_phrases_case_insensitively() {
    let text = "这里是内嵌 SVG 和 CSS 的说明，还提到了 Inline svg。";
    let matched = matched_phrases(text);
    assert_eq!(matched, vec!["内嵌 SVG 和 CSS", "inline SVG"]);
    assert!(matched_phrases("一切正常的 HTML 动画。").is_empty());
}

#[test]
fn extracts_fenced_html_block_first() {
    let text = "说明\n```html\n<!DOCTYPE html><html><body>ok</body></html>\n```\n结尾";
    assert_eq!(
        extract_html_document(text),
        "<!DOCTYPE html><html><body>ok</body></html>"
    );
}

#[test]
fn extracts_bare_html_document() {
    let text = "前置说明 <HTML><body>x</body></HTML> 之后的内容";
    assert_eq!(extract_html_document(text), "<HTML><body>x</body></HTML>");
}

#[test]
fn keeps_full_text_without_html_structure() {
    let text = "纯文字答复，没有任何标记。";
    assert_eq!(extract_html_document(text), text);
}
