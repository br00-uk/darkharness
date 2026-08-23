; Java tags. Grounded against tree-sitter-java 0.23.5's own node-types.json.
; `exported` and the `abstract class` half of `is_interface_like` both read
; the `modifiers` child's own text, in the `java` adapter, rather than a
; capture here: the keyword tokens inside `modifiers` are anonymous nodes,
; invisible to a query pattern that names them.

(class_declaration
    name: (identifier) @name) @definition.class

(method_declaration
    name: (identifier) @name) @definition.method

(interface_declaration
    name: (identifier) @name) @definition.interface

(enum_declaration
    name: (identifier) @name) @definition.enum

(method_invocation
    name: (identifier) @name) @reference.call

(object_creation_expression
    type: (type_identifier) @name) @reference.call

(import_declaration) @import
