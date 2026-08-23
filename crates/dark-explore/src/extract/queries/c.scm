; C tags. Grounded against tree-sitter-c 0.24.2's own node-types.json.
; `exported` reads whether `static` appears in the source text between the
; definition node's start and its name, in the `c` adapter, rather than a
; capture here: `storage_class_specifier` is a sibling inside the same
; `function_definition` node, ahead of the declarator that carries the name.

(function_definition
    declarator: (function_declarator
        declarator: (identifier) @name)) @definition.function

(struct_specifier
    name: (type_identifier) @name
    body: (_)) @definition.class

(union_specifier
    name: (type_identifier) @name
    body: (_)) @definition.class

(enum_specifier
    name: (type_identifier) @name
    body: (_)) @definition.enum

(type_definition
    declarator: (type_identifier) @name) @definition.type

(call_expression
    function: (identifier) @name) @reference.call

(preproc_include) @import
