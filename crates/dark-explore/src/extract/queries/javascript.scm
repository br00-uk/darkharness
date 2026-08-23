; JavaScript tags. Grounded against tree-sitter-javascript 0.25.0's own
; node-types.json.

(function_declaration
    name: (identifier) @name) @definition.function

(generator_function_declaration
    name: (identifier) @name) @definition.function

(
    (method_definition
        name: (property_identifier) @name) @definition.method
    (#not-eq? @name "constructor")
)

(class_declaration
    name: (identifier) @name) @definition.class

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
