const path = require("node:path");

function node(type, ...children) {
  return { type, children };
}

global.grammar = (definition) => {
  const symbols = new Proxy(
    {},
    {
      get: (_target, name) => node("symbol", String(name)),
    },
  );
  for (const [name, rule] of Object.entries(definition.rules)) {
    if (typeof rule === "function") {
      rule(symbols);
    } else {
      throw new Error(`rule ${name} must be a function`);
    }
  }
  return definition;
};
global.seq = (...children) => node("seq", ...children);
global.choice = (...children) => node("choice", ...children);
global.repeat = (child) => node("repeat", child);
global.optional = (child) => node("optional", child);
global.field = (name, child) => node("field", name, child);
global.token = (child) => node("token", child);
global.token.immediate = (child) => node("token.immediate", child);
global.prec = (value, child) => node("prec", value, child);
global.prec.left = (value, child) => node("prec.left", value, child);
global.prec.right = (value, child) => node("prec.right", value, child);

const grammar = require(path.join(__dirname, "..", "grammar.js"));
const requiredRules = [
  "source_file",
  "statement",
  "let_statement",
  "function_declaration",
  "struct_declaration",
  "route_declaration",
  "respond_statement",
  "expression",
  "type",
];

for (const rule of requiredRules) {
  if (!grammar.rules[rule]) {
    throw new Error(`missing required rule ${rule}`);
  }
}

console.log(`tree-sitter-orv grammar smoke ok: ${requiredRules.length} rules checked`);
