#![forbid(unsafe_code)]
//! Unit tests for the Safe Rust YAML subset & Markdown frontmatter parser.

use serde_json::json;

use super::{parse_markdown_frontmatter, parse_yaml_str};

#[test]
fn parses_block_scalars_literal_and_folded_with_chomping() {
    let yaml = "\
title: Example
body: |
  # Heading

  Line one # not comment
  - bullet
trimmed: |-
  alpha
  beta
folded: >
  hello
  world

  next paragraph
    indented line
  end
folded_strip: >-
  one
  two
nextKey: 42
";
    let val = parse_yaml_str(yaml, "test.yaml").expect("parse block scalars");
    assert_eq!(val["title"], json!("Example"));
    assert_eq!(
        val["body"],
        json!("# Heading\n\nLine one # not comment\n- bullet\n")
    );
    assert_eq!(val["trimmed"], json!("alpha\nbeta"));
    assert_eq!(
        val["folded"],
        json!("hello world\nnext paragraph\n  indented line\nend\n")
    );
    assert_eq!(val["folded_strip"], json!("one two"));
    assert_eq!(val["nextKey"], json!(42));
}

#[test]
fn parses_non_empty_flow_mappings_and_nested_flow_arrays() {
    let yaml = "\
telemetry: { enabled: true, sampleRate: 0.5, tags: [\"a, b\", 'c'] }
matrix: [[1, 2], [3, [4, 5]]]
items: [{ id: 1, nested: { ok: true } }, { id: 2 }]
emptyMap: {}
emptyArr: []
";
    let val = parse_yaml_str(yaml, "flow.yaml").expect("parse flow collections");
    assert_eq!(
        val["telemetry"],
        json!({ "enabled": true, "sampleRate": 0.5, "tags": ["a, b", "c"] })
    );
    assert_eq!(val["matrix"], json!([[1, 2], [3, [4, 5]]]));
    assert_eq!(
        val["items"],
        json!([{ "id": 1, "nested": { "ok": true } }, { "id": 2 }])
    );
    assert_eq!(val["emptyMap"], json!({}));
    assert_eq!(val["emptyArr"], json!([]));
}

#[test]
fn parses_variable_sequence_indents_and_same_indent_nested_sequences() {
    let yaml = "\
steps:
  -   name: step1
      cmds:
      - echo 1
      - echo 2
      meta:
        retries: 2
      summary: |-
        done
  - name: step2
    cmds:
    - echo 3
    extra:
      - nested
";
    let val = parse_yaml_str(yaml, "seq.yaml").expect("parse variable sequences");
    assert_eq!(
        val,
        json!({
            "steps": [
                {
                    "name": "step1",
                    "cmds": ["echo 1", "echo 2"],
                    "meta": { "retries": 2 },
                    "summary": "done"
                },
                {
                    "name": "step2",
                    "cmds": ["echo 3"],
                    "extra": ["nested"]
                }
            ]
        })
    );
}

#[test]
fn parses_markdown_frontmatter_and_rejects_malformed_yaml() {
    let md = "---\nid: TSK-0001\ntags: [a, b]\n---\n\n# Body\n";
    let (fm, body) = parse_markdown_frontmatter(md, "task.md").expect("parse frontmatter");
    assert_eq!(fm, Some(json!({ "id": "TSK-0001", "tags": ["a", "b"] })));
    assert_eq!(body, "# Body");

    assert!(parse_yaml_str("a: [1, 2\n", "bad1.yaml").is_err());
    assert!(parse_yaml_str("a: { b: 1\n", "bad2.yaml").is_err());
    assert!(parse_yaml_str("a:\n\t- 1\n", "bad3.yaml").is_err());
}

#[test]
fn parses_block_scalar_on_first_key_of_sequence_item_mapping() {
    let yaml = "\
steps:
  - summary: |-
      block in first key
      second line
    name: s1
  - summary: >
      folded first
      key line
    name: s2
";
    let val = parse_yaml_str(yaml, "seq_first_block.yaml").expect("parse first-key block scalar");
    assert_eq!(
        val,
        json!({
            "steps": [
                { "summary": "block in first key\nsecond line", "name": "s1" },
                { "summary": "folded first key line\n", "name": "s2" }
            ]
        })
    );
}
