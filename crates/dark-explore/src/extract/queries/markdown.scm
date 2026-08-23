; Markdown tags. Grounded against tree-sitter-md 0.5.3's own
; node-types.json. The block grammar leaves a paragraph's inline content
; (links included) inside one opaque `inline` node; the `markdown` adapter
; re-parses that node's text with `tree_sitter_md::INLINE_LANGUAGE` to reach
; `inline_link` nodes. This query only locates the block-level structure:
; headings as definitions, and reference-style link definitions as imports.

(atx_heading
    heading_content: (inline) @name) @definition.section

(setext_heading
    heading_content: (inline) @name) @definition.section

(link_reference_definition
    (link_label) @name
    (link_destination)) @import
