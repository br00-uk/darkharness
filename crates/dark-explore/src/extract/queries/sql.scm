; SQL tags. Grounded against tree-sitter-sequel 0.3.11's own
; node-types.json. SQL has no import statement, so the `sql` grammar
; adapter always returns an empty `imports[]`. A `CREATE TABLE` or
; `CREATE VIEW` is close enough to a structural record type that the `sql`
; adapter maps both onto [`DefKind::Class`]; `CREATE FUNCTION` maps onto
; [`DefKind::Function`].

(create_table
    (object_reference
        name: (identifier) @name)) @definition.class

(create_view
    (object_reference
        name: (identifier) @name)) @definition.class

(create_function
    (object_reference
        name: (identifier) @name)) @definition.function

(relation
    (object_reference
        name: (identifier) @name)) @reference.table
