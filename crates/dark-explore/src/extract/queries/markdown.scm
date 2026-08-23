; Markdown tags. Grounded against tree-sitter-md 0.5.3's own
; node-types.json.
;
; The block grammar leaves a paragraph's inline content (an inline link,
; `[text](url)`, included) inside one opaque `inline` node: reading it needs
; a second parse with `tree_sitter_md::INLINE_LANGUAGE`, which this pass
; does not attempt. The `markdown` adapter extracts only what the block
; grammar itself structures: headings as definitions, and reference-style
; link definitions (`[label]: url`) as imports.

(atx_heading
    heading_content: (inline) @name) @definition.section

(setext_heading
    heading_content: (paragraph) @name) @definition.section

(link_reference_definition
    (link_label) @name
    (link_destination)) @import
