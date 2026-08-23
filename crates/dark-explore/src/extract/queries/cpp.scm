; C++ tags. Grounded against tree-sitter-cpp 0.23.4's own node-types.json.
; `exported` reads the source text preceding the definition's name, in the
; `cpp` adapter, the same way the `c` adapter does.

(function_definition
    declarator: (function_declarator
        declarator: (identifier) @name)) @definition.function

(function_definition
    declarator: (function_declarator
        declarator: (field_identifier) @name)) @definition.method

(function_definition
    declarator: (function_declarator
        declarator: (qualified_identifier
            name: (identifier) @name))) @definition.method

(struct_specifier
    name: (type_identifier) @name
    body: (_)) @definition.class

(class_specifier
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

(namespace_definition
    name: (namespace_identifier) @name) @definition.module

(call_expression
    function: (identifier) @name) @reference.call

(call_expression
    function: (field_expression
        field: (field_identifier) @name)) @reference.call

(preproc_include) @import

(using_declaration) @import
