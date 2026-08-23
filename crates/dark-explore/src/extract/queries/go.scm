; Go tags. Grounded against tree-sitter-go 0.25.0's own node-types.json.

(function_declaration
    name: (identifier) @name) @definition.function

(method_declaration
    name: (field_identifier) @name) @definition.method

; `type Foo interface { ... }` and `type Foo struct { ... }` share the
; `type_spec` node; the `go` adapter tells them apart, and every other
; `type_spec` shape, by inspecting the `type` field itself.
(type_declaration
    (type_spec
        name: (type_identifier) @name)) @definition.type

(const_declaration
    (const_spec
        name: (identifier) @name)) @definition.constant

(var_declaration
    (var_spec
        name: (identifier) @name)) @definition.variable

(call_expression
    function: (identifier) @name) @reference.call

(call_expression
    function: (selector_expression
        field: (field_identifier) @name)) @reference.call

(import_declaration) @import
