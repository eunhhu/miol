[
  "import"
  "let"
  "mut"
  "sig"
  "function"
  "struct"
  "enum"
  "type"
  "define"
  "return"
] @keyword

[
  "true"
  "false"
  "null"
] @constant.builtin

(comment) @comment
(string) @string
(number) @number
(escape_sequence) @string.escape

(attribute) @attribute
(route_attribute) @attribute
(route_method) @constant.builtin
(route_path) @string.special

(function_declaration name: (identifier) @function)
(define_declaration name: (identifier) @function)
(call_expression function: (identifier) @function.call)

(struct_declaration name: (identifier) @type)
(enum_declaration name: (identifier) @type)
(type_alias name: (identifier) @type)
(generic_type (identifier) @type)

(parameter name: (identifier) @variable.parameter)
(field_declaration name: (identifier) @property)
(pair key: (identifier) @property)
(member_expression field: (identifier) @property)
