; Rust tags. Grounded against tree-sitter-rust 0.24.2's own node-types.json;
; see the `rust` grammar adapter for the classification this feeds.

(function_item
    name: (identifier) @name) @definition.function

; A function nested directly inside a `declaration_list` sits inside an
; `impl`, a `trait`, or a plain `mod` body — the grammar gives all three the
; same body node kind, so this pattern cannot tell a method from a function
; nested in a module. The `rust` adapter accepts the resulting `Method`
; over-classification for the `mod` case: it does not change name-based
; resolution, and it matches the imprecision in the grammar's own upstream
; `tags.scm`.
(declaration_list
    (function_item
        name: (identifier) @name) @definition.method)

(struct_item
    name: (type_identifier) @name) @definition.class

(union_item
    name: (type_identifier) @name) @definition.class

(enum_item
    name: (type_identifier) @name) @definition.enum

(type_item
    name: (type_identifier) @name) @definition.type

(trait_item
    name: (type_identifier) @name) @definition.interface

(mod_item
    name: (identifier) @name) @definition.module

(macro_definition
    name: (identifier) @name) @definition.macro

(call_expression
    function: (identifier) @name) @reference.call

(call_expression
    function: (field_expression
        field: (field_identifier) @name)) @reference.call

(call_expression
    function: (scoped_identifier
        name: (identifier) @name)) @reference.call

(macro_invocation
    macro: (identifier) @name) @reference.call

(use_declaration) @import
