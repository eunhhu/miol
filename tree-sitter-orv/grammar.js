const PREC = {
  call: 9,
  member: 8,
  unary: 7,
  multiplicative: 6,
  additive: 5,
  comparison: 4,
  logical: 3,
  assignment: 1,
};

module.exports = grammar({
  name: "orv",

  extras: ($) => [/\s/, $.comment],

  word: ($) => $.identifier,

  supertypes: ($) => [$.statement, $.expression, $.type],

  rules: {
    source_file: ($) => repeat($.statement),

    comment: () => token(choice(seq("//", /.*/), seq("/*", /[^]*?\*\//))),

    statement: ($) =>
      choice(
        $.import_statement,
        $.let_statement,
        $.function_declaration,
        $.struct_declaration,
        $.enum_declaration,
        $.type_alias,
        $.define_declaration,
        $.domain_block,
        $.route_declaration,
        $.listen_statement,
        $.respond_statement,
        $.return_statement,
        $.expression_statement,
      ),

    import_statement: ($) => seq("import", field("path", $.string)),

    let_statement: ($) =>
      seq(
        "let",
        optional("mut"),
        optional("sig"),
        field("name", $.identifier),
        optional(seq(":", field("type", $.type))),
        optional(seq("=", field("value", $.expression))),
      ),

    function_declaration: ($) =>
      seq(
        "function",
        field("name", $.identifier),
        field("parameters", $.parameter_list),
        optional(seq(":", field("return_type", $.type))),
        choice(seq("->", field("body", $.expression)), field("body", $.block)),
      ),

    parameter_list: ($) =>
      seq("(", optional(commaSep($.parameter)), optional(","), ")"),

    parameter: ($) => seq(field("name", $.identifier), ":", field("type", $.type)),

    struct_declaration: ($) =>
      seq("struct", field("name", $.identifier), field("body", $.field_block)),

    field_block: ($) => seq("{", repeat(choice($.field_declaration, ",")), "}"),

    field_declaration: ($) =>
      seq(field("name", $.identifier), ":", field("type", $.type)),

    enum_declaration: ($) =>
      seq("enum", field("name", $.identifier), "{", repeat(choice($.identifier, ",")), "}"),

    type_alias: ($) => seq("type", field("name", $.identifier), "=", field("type", $.type)),

    define_declaration: ($) =>
      seq("define", field("name", $.identifier), optional($.parameter_list), field("body", $.block)),

    domain_block: ($) => seq($.attribute, field("body", $.block)),

    route_declaration: ($) =>
      seq(
        $.route_attribute,
        field("method", $.route_method),
        field("path", $.route_path),
        field("body", $.block),
      ),

    listen_statement: ($) => seq("@listen", field("port", $.number)),

    respond_statement: ($) =>
      seq("@respond", field("status", $.number), field("body", $.object_literal)),

    return_statement: ($) => seq("return", field("value", $.expression)),

    expression_statement: ($) => $.expression,

    block: ($) => seq("{", repeat($.statement), "}"),

    expression: ($) =>
      choice(
        $.identifier,
        $.number,
        $.string,
        $.boolean,
        $.null,
        $.attribute,
        $.object_literal,
        $.array_literal,
        $.call_expression,
        $.member_expression,
        $.unary_expression,
        $.binary_expression,
        $.assignment_expression,
        $.parenthesized_expression,
      ),

    call_expression: ($) =>
      prec(PREC.call, seq(field("function", $.expression), field("arguments", $.argument_list))),

    argument_list: ($) => seq("(", optional(commaSep($.expression)), optional(","), ")"),

    member_expression: ($) =>
      prec(PREC.member, seq(field("object", $.expression), ".", field("field", $.identifier))),

    unary_expression: ($) =>
      prec(PREC.unary, seq(field("operator", choice("!", "-", "@")), field("argument", $.expression))),

    binary_expression: ($) =>
      choice(
        ...[
          ["*", PREC.multiplicative],
          ["/", PREC.multiplicative],
          ["%", PREC.multiplicative],
          ["+", PREC.additive],
          ["-", PREC.additive],
          ["==", PREC.comparison],
          ["!=", PREC.comparison],
          ["<", PREC.comparison],
          ["<=", PREC.comparison],
          [">", PREC.comparison],
          [">=", PREC.comparison],
          ["&&", PREC.logical],
          ["||", PREC.logical],
        ].map(([operator, precedence]) =>
          prec.left(
            precedence,
            seq(field("left", $.expression), field("operator", operator), field("right", $.expression)),
          ),
        ),
      ),

    assignment_expression: ($) =>
      prec.right(
        PREC.assignment,
        seq(
          field("left", $.expression),
          field("operator", choice("=", "+=", "-=", "*=", "/=")),
          field("right", $.expression),
        ),
      ),

    parenthesized_expression: ($) => seq("(", $.expression, ")"),

    object_literal: ($) => seq("{", optional(commaSep($.pair)), optional(","), "}"),

    pair: ($) => seq(field("key", choice($.identifier, $.string)), ":", field("value", $.expression)),

    array_literal: ($) => seq("[", optional(commaSep($.expression)), optional(","), "]"),

    type: ($) =>
      choice(
        $.identifier,
        $.nullable_type,
        $.array_type,
        $.generic_type,
      ),

    nullable_type: ($) => seq(choice($.identifier, $.array_type, $.generic_type), "?"),

    array_type: ($) => seq("[", $.type, "]"),

    generic_type: ($) => seq($.identifier, "<", commaSep($.type), optional(","), ">"),

    attribute: ($) => /@[A-Za-z_][A-Za-z0-9_.]*/,

    route_attribute: () => "@route",

    route_method: () => choice("GET", "POST", "PUT", "PATCH", "DELETE"),

    route_path: () => token(seq("/", /[A-Za-z0-9_./:{}-]*/)),

    boolean: () => choice("true", "false"),

    null: () => "null",

    identifier: () => /[A-Za-z_][A-Za-z0-9_]*/,

    number: () => /\d+(\.\d+)?/,

    string: ($) => seq('"', repeat(choice($.escape_sequence, /[^"\\]/)), '"'),

    escape_sequence: () => token(seq("\\", /./)),
  },
});

function commaSep(rule) {
  return seq(rule, repeat(seq(",", rule)));
}
