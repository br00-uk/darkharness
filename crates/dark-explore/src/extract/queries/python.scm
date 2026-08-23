; Python tags. Grounded against tree-sitter-python 0.25.0's own
; node-types.json.
;
; A `function_definition` inside a class body's `block` also matches the
; bare `@definition.function` pattern below, because the pattern below
; carries no positional constraint. Extraction dedups by node identity and
; keeps the more specific `@definition.method` capture; see
; `dedup_defs_by_node`.

(class_definition
    name: (identifier) @name) @definition.class

(class_definition
    body: (block
        (function_definition
            name: (identifier) @name) @definition.method))

(function_definition
    name: (identifier) @name) @definition.function

(call
    function: [
        (identifier) @name
        (attribute
            attribute: (identifier) @name)
    ]) @reference.call

(import_statement) @import

(import_from_statement) @import
