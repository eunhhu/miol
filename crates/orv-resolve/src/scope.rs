//! Scope tree: a hierarchy of lexical scopes that map names to symbol IDs.

use std::collections::{HashMap, hash_map::Entry};

use crate::symbol::SymbolId;

/// A unique identifier for a scope within the scope tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScopeId(u32);

impl ScopeId {
    /// Creates a new `ScopeId` from a raw index.
    pub(crate) const fn new(raw: u32) -> Self {
        Self(raw)
    }

    /// Returns the underlying raw value.
    pub const fn raw(self) -> u32 {
        self.0
    }
}

/// What kind of syntactic construct introduced this scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    /// The top-level module scope.
    Module,
    /// A function body scope.
    Function,
    /// A define (component) body scope.
    Define,
    /// A block expression `{ ... }`.
    Block,
    /// An `if` branch body.
    IfBranch,
    /// A `for` loop body.
    ForLoop,
    /// A `while` loop body.
    WhileLoop,
    /// A single `when` arm body.
    WhenArm,
}

/// A single lexical scope containing name-to-symbol bindings.
#[derive(Debug)]
pub struct Scope {
    /// What introduced this scope.
    kind: ScopeKind,
    /// The parent scope, if any (`None` for the module root).
    parent: Option<ScopeId>,
    /// Names declared directly in this scope, mapped to their symbol IDs.
    bindings: HashMap<String, SymbolId>,
}

impl Scope {
    /// Creates a new scope with the given kind and optional parent.
    fn new(kind: ScopeKind, parent: Option<ScopeId>) -> Self {
        Self {
            kind,
            parent,
            bindings: HashMap::new(),
        }
    }

    /// Returns what introduced this scope.
    pub const fn kind(&self) -> ScopeKind {
        self.kind
    }

    /// Returns the parent scope, if any.
    pub const fn parent(&self) -> Option<ScopeId> {
        self.parent
    }

    /// Inserts a name binding into this scope. Returns `Some(old_id)` if
    /// the name was already bound in this scope (duplicate declaration).
    ///
    /// The original binding is preserved so later lookups continue to resolve
    /// to the first declaration after a duplicate-definition diagnostic.
    pub fn insert(&mut self, name: String, id: SymbolId) -> Option<SymbolId> {
        match self.bindings.entry(name) {
            Entry::Occupied(entry) => Some(*entry.get()),
            Entry::Vacant(entry) => {
                entry.insert(id);
                None
            }
        }
    }

    /// Looks up a name in this scope only (not parents).
    pub fn lookup_local(&self, name: &str) -> Option<SymbolId> {
        self.bindings.get(name).copied()
    }

    /// Returns an iterator over all bindings in this scope.
    pub fn bindings(&self) -> impl Iterator<Item = (&str, SymbolId)> {
        self.bindings.iter().map(|(k, v)| (k.as_str(), *v))
    }

    /// Returns the number of bindings in this scope.
    pub fn binding_count(&self) -> usize {
        self.bindings.len()
    }
}

/// A tree of lexical scopes, stored as a flat arena.
#[derive(Debug, Default)]
pub struct ScopeMap {
    scopes: Vec<Scope>,
}

impl ScopeMap {
    /// Creates an empty scope map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a new scope with the given kind and optional parent. Returns its ID.
    ///
    /// # Panics
    ///
    /// Panics if the parent scope is invalid or the number of scopes exceeds
    /// `u32::MAX`.
    pub fn add(&mut self, kind: ScopeKind, parent: Option<ScopeId>) -> ScopeId {
        if let Some(parent) = parent {
            self.assert_valid_scope_id(parent);
        }
        let id = u32::try_from(self.scopes.len()).expect("too many scopes");
        self.scopes.push(Scope::new(kind, parent));
        ScopeId::new(id)
    }

    /// Returns a reference to the scope with the given ID.
    pub fn get(&self, id: ScopeId) -> &Scope {
        self.assert_valid_scope_id(id);
        &self.scopes[id.raw() as usize]
    }

    /// Inserts a binding into the given scope. Returns the previous symbol if
    /// the name was already declared in that same scope.
    pub fn insert(&mut self, scope: ScopeId, name: String, id: SymbolId) -> Option<SymbolId> {
        self.get_mut(scope).insert(name, id)
    }

    /// Looks up a name in the given scope only (not parents).
    pub fn lookup_local(&self, scope: ScopeId, name: &str) -> Option<SymbolId> {
        self.get(scope).lookup_local(name)
    }

    /// Returns a mutable reference to the scope with the given ID.
    fn get_mut(&mut self, id: ScopeId) -> &mut Scope {
        self.assert_valid_scope_id(id);
        &mut self.scopes[id.raw() as usize]
    }

    /// Looks up a name starting from the given scope, walking up the parent
    /// chain until found or the root is reached.
    pub fn lookup(&self, start: ScopeId, name: &str) -> Option<SymbolId> {
        let mut current = Some(start);
        let mut depth = 0_usize;
        while let Some(scope_id) = current {
            assert!(
                depth < self.scopes.len(),
                "cycle detected in scope parent chain"
            );
            let scope = self.get(scope_id);
            if let Some(sym_id) = scope.lookup_local(name) {
                return Some(sym_id);
            }
            current = scope.parent();
            depth += 1;
        }
        None
    }

    /// Returns the total number of scopes.
    pub const fn len(&self) -> usize {
        self.scopes.len()
    }

    /// Returns the scope id at the given arena index, if it exists.
    pub fn scope_id_at(&self, index: usize) -> Option<ScopeId> {
        if index < self.scopes.len() {
            let raw = u32::try_from(index).expect("too many scopes");
            Some(ScopeId::new(raw))
        } else {
            None
        }
    }

    /// Returns `true` if there are no scopes.
    pub const fn is_empty(&self) -> bool {
        self.scopes.is_empty()
    }

    fn assert_valid_scope_id(&self, id: ScopeId) {
        assert!(
            (id.raw() as usize) < self.scopes.len(),
            "invalid scope id {}",
            id.raw()
        );
    }
}

#[cfg(test)]
mod tests {
    use orv_span::{FileId, Span};
    use pretty_assertions::assert_eq;

    use crate::symbol::{Symbol, SymbolKind, SymbolTable, Visibility};

    use super::*;

    fn dummy_span() -> Span {
        Span::new(FileId::new(0), 0, 1)
    }

    fn make_symbol(table: &mut SymbolTable, name: &str, kind: SymbolKind) -> SymbolId {
        table.add(Symbol {
            name: name.into(),
            kind,
            visibility: Visibility::Private,
            def_span: dummy_span(),
        })
    }

    #[test]
    fn scope_insert_and_lookup_local() {
        let mut symbols = SymbolTable::new();
        let sym_x = make_symbol(&mut symbols, "x", SymbolKind::Variable);

        let mut scope_map = ScopeMap::new();
        let root = scope_map.add(ScopeKind::Module, None);
        scope_map.insert(root, "x".into(), sym_x);

        assert_eq!(scope_map.lookup_local(root, "x"), Some(sym_x));
        assert_eq!(scope_map.lookup_local(root, "y"), None);
    }

    #[test]
    fn scope_parent_chain_lookup() {
        let mut symbols = SymbolTable::new();
        let sym_a = make_symbol(&mut symbols, "a", SymbolKind::Variable);
        let sym_b = make_symbol(&mut symbols, "b", SymbolKind::Variable);

        let mut scope_map = ScopeMap::new();
        let root = scope_map.add(ScopeKind::Module, None);
        scope_map.insert(root, "a".into(), sym_a);

        let child = scope_map.add(ScopeKind::Block, Some(root));
        scope_map.insert(child, "b".into(), sym_b);

        // `b` found in child scope
        assert_eq!(scope_map.lookup(child, "b"), Some(sym_b));
        // `a` found by walking up to parent
        assert_eq!(scope_map.lookup(child, "a"), Some(sym_a));
        // `b` not visible from root
        assert_eq!(scope_map.lookup(root, "b"), None);
    }

    #[test]
    fn scope_shadowing() {
        let mut symbols = SymbolTable::new();
        let sym_outer = make_symbol(&mut symbols, "x", SymbolKind::Variable);
        let sym_inner = make_symbol(&mut symbols, "x", SymbolKind::Variable);

        let mut scope_map = ScopeMap::new();
        let root = scope_map.add(ScopeKind::Module, None);
        scope_map.insert(root, "x".into(), sym_outer);

        let child = scope_map.add(ScopeKind::Block, Some(root));
        scope_map.insert(child, "x".into(), sym_inner);

        // inner scope sees the shadow
        assert_eq!(scope_map.lookup(child, "x"), Some(sym_inner));
        // outer scope sees original
        assert_eq!(scope_map.lookup(root, "x"), Some(sym_outer));
    }

    #[test]
    fn scope_duplicate_insert_returns_old() {
        let mut symbols = SymbolTable::new();
        let sym1 = make_symbol(&mut symbols, "dup", SymbolKind::Variable);
        let sym2 = make_symbol(&mut symbols, "dup", SymbolKind::Variable);

        let mut scope_map = ScopeMap::new();
        let root = scope_map.add(ScopeKind::Module, None);

        let first = scope_map.insert(root, "dup".into(), sym1);
        assert!(first.is_none());

        let second = scope_map.insert(root, "dup".into(), sym2);
        assert_eq!(second, Some(sym1));
        assert_eq!(scope_map.lookup(root, "dup"), Some(sym1));
    }

    #[test]
    fn scope_kind_and_binding_count() {
        let mut scope_map = ScopeMap::new();
        let root = scope_map.add(ScopeKind::Module, None);
        assert_eq!(scope_map.get(root).kind(), ScopeKind::Module);
        assert_eq!(scope_map.get(root).binding_count(), 0);
        assert!(scope_map.get(root).parent().is_none());
    }

    #[test]
    fn scope_map_len() {
        let mut scope_map = ScopeMap::new();
        assert!(scope_map.is_empty());
        scope_map.add(ScopeKind::Module, None);
        scope_map.add(ScopeKind::Block, Some(ScopeId::new(0)));
        assert_eq!(scope_map.len(), 2);
    }

    #[test]
    fn deeply_nested_lookup() {
        let mut symbols = SymbolTable::new();
        let sym = make_symbol(&mut symbols, "deep", SymbolKind::Function);

        let mut scope_map = ScopeMap::new();
        let s0 = scope_map.add(ScopeKind::Module, None);
        scope_map.insert(s0, "deep".into(), sym);

        let s1 = scope_map.add(ScopeKind::Function, Some(s0));
        let s2 = scope_map.add(ScopeKind::Block, Some(s1));
        let s3 = scope_map.add(ScopeKind::IfBranch, Some(s2));

        // Can find `deep` 3 levels up
        assert_eq!(scope_map.lookup(s3, "deep"), Some(sym));
        // Unknown name fails
        assert_eq!(scope_map.lookup(s3, "nonexistent"), None);
    }

    #[test]
    #[should_panic(expected = "invalid scope id")]
    fn add_rejects_invalid_parent_scope() {
        let mut scope_map = ScopeMap::new();
        scope_map.add(ScopeKind::Module, None);
        scope_map.add(ScopeKind::Block, Some(ScopeId::new(99)));
    }
}
