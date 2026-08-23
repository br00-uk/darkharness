; TypeScript tags. Grounded against tree-sitter-typescript 0.23.2's own
; node-types.json. The `typescript` grammar adapter compiles this same text
; against both the plain TypeScript grammar and the TSX grammar: the two
; share the node kinds this query names, so one file serves both, the way
; the upstream `tree-sitter-typescript` package itself ships one `tags.scm`
; for both.

(function_declaration
    name: (identifier) @name) @definition.function

(
    (method_definition
        name: (property_identifier) @name) @definition.method
    (#not-eq? @name "constructor")
)

(abstract_method_signature
    name: (property_identifier) @name) @definition.method

(method_signature
    name: (property_identifier) @name) @definition.method

; A plain `class` is `class_declaration`; the grammar gives `abstract class`
; a distinct node kind, `abstract_class_declaration`, rather than a modifier
; on `class_declaration`.
(class_declaration
    name: (type_identifier) @name) @definition.class

(abstract_class_declaration
    name: (type_identifier) @name) @definition.class

(interface_declaration
    name: (type_identifier) @name) @definition.interface

; `type Foo = { ... }` and `type Foo = string` share `type_alias_declaration`;
; the `typescript` adapter tells an object-shape alias apart from any other
; by inspecting the `value` field.
(type_alias_declaration
    name: (type_identifier) @name) @definition.type

(module
    name: (identifier) @name) @definition.module

(internal_module
    name: (identifier) @name) @definition.module

(
    (lexical_declaration
        (variable_declarator
            name: (identifier) @name
            value: [(arrow_function) (function_expression)]) @definition.function)
)

(call_expression
    function: (identifier) @name) @reference.call

(call_expression
    function: (member_expression
        property: (property_identifier) @name)) @reference.call

(new_expression
    constructor: (identifier) @name) @reference.call

(import_statement) @import

(export_statement
    source: (string)) @import
