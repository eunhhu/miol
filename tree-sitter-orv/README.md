# tree-sitter-orv

Source grammar and baseline editor queries for `.orv` files.

This package intentionally keeps generated parser artifacts out of the repo for now. Run `tree-sitter generate` from this directory when the Tree-sitter CLI is available, then wire the generated parser/bindings into editor distributions.

Local smoke check:

```sh
npm run test:grammar
```
