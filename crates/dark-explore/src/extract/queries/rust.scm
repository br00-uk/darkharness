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

; A type named in a type position: `dyn Engine`, `Vec<Session>`,
; `-> Result<Chunk>`, `impl Engine for RealEngine`. Without these a
; blast radius over Rust is a call graph, which misses most of what a
; change to a trait or a struct actually reaches.
;
; This bare pattern also matches the name node of every type definition
; above — `struct_item name: (type_identifier)` and its siblings. Those
; are not references to anything, and `extract::file::partition_tags`
; drops a reference whose name node is a definition's own name node. It
; is dropped there rather than avoided here because the same is true of
; every grammar, and enumerating each type position in each grammar is
; both long and easy to leave incomplete.
(type_identifier) @name @reference.type

; The type a path leads with: the `Event` of `Event::TurnStart`, the
; `Path` of `Path::new`. A variant or an associated function is reached
; through the type that owns it, so a change to that type reaches here.
(scoped_identifier
    path: (identifier) @name) @reference.type

(use_declaration) @import
