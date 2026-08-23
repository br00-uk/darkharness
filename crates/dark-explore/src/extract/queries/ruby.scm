; Ruby tags. Grounded against tree-sitter-ruby 0.23.1's own node-types.json.
; Ruby has no file-scoped export keyword, so the `ruby` adapter reports
; every definition `exported`; see [`Def::exported`].

(method
    name: (identifier) @name) @definition.method

(singleton_method
    name: (identifier) @name) @definition.method

(class
    name: (constant) @name) @definition.class

(module
    name: (constant) @name) @definition.module

(
    (call
        method: (identifier) @name) @reference.call
    (#not-any-of? @name "require" "require_relative" "require_all" "load")
)

(
    (call
        method: (identifier) @name
        arguments: (argument_list (string))) @import
    (#any-of? @name "require" "require_relative" "require_all")
)
