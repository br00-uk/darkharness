; C# tags. Grounded against tree-sitter-c-sharp 0.23.5's own
; node-types.json. `exported` and `abstract class` both read the `modifier`
; children's own text, in the `csharp` adapter, rather than a capture here.

(class_declaration
    name: (identifier) @name) @definition.class

(method_declaration
    name: (identifier) @name) @definition.method

(interface_declaration
    name: (identifier) @name) @definition.interface

(enum_declaration
    name: (identifier) @name) @definition.enum

(namespace_declaration
    name: (identifier) @name) @definition.module

(invocation_expression
    function: (identifier) @name) @reference.call

(invocation_expression
    function: (member_access_expression
        name: (identifier) @name)) @reference.call

(object_creation_expression
    type: (identifier) @name) @reference.call

(using_directive) @import
