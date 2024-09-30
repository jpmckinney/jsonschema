use core::fmt;
use std::collections::VecDeque;

use fluent_uri::UriRef;
use serde_json::Value;

use crate::{uri, Draft, Error, Registry, ResourceRef};

/// A reference resolver.
///
/// Resolves references against the base URI and looks up the result in the registry.
#[derive(Clone)]
pub struct Resolver<'r> {
    pub(crate) registry: &'r Registry,
    base_uri: UriRef<String>,
    scopes: VecDeque<UriRef<String>>,
}

impl<'r> PartialEq for Resolver<'r> {
    fn eq(&self, other: &Self) -> bool {
        self.base_uri == other.base_uri
    }
}
impl<'r> Eq for Resolver<'r> {}

impl<'r> fmt::Debug for Resolver<'r> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Resolver")
            .field("base_uri", &self.base_uri.as_str())
            .field("scopes", &{
                let mut buf = String::from("[");
                let mut values = self.scopes.iter();
                if let Some(value) = values.next() {
                    buf.push_str(value.as_str());
                }
                for value in values {
                    buf.push_str(", ");
                    buf.push_str(value.as_str());
                }
                buf.push(']');
                buf
            })
            .finish()
    }
}

impl<'r> Resolver<'r> {
    /// Create a new `Resolver` with the given registry and base URI.
    pub(crate) fn new(registry: &'r Registry, base_uri: UriRef<String>) -> Self {
        Self {
            registry,
            base_uri,
            scopes: VecDeque::new(),
        }
    }
    pub(crate) fn from_parts(
        registry: &'r Registry,
        base_uri: UriRef<String>,
        scopes: VecDeque<UriRef<String>>,
    ) -> Self {
        Self {
            registry,
            base_uri,
            scopes,
        }
    }
    #[must_use]
    pub fn base_uri(&self) -> UriRef<&str> {
        self.base_uri.borrow()
    }
    /// Resolve a reference to the resource it points to.
    ///
    /// # Errors
    ///
    /// If the reference cannot be resolved or is invalid.
    pub fn lookup(&self, reference: &str) -> Result<Resolved<'r>, Error> {
        let (uri, fragment) = if let Some(reference) = reference.strip_prefix('#') {
            (self.base_uri.clone(), reference)
        } else {
            let (uri, fragment) = if let Some((uri, fragment)) = reference.rsplit_once('#') {
                (uri, fragment)
            } else {
                (reference, "")
            };
            if self.base_uri.as_str().is_empty() {
                (uri::from_str(uri)?, fragment)
            } else {
                let uri = uri::resolve_against(&self.base_uri.borrow(), uri)?;
                (uri, fragment)
            }
        };

        let retrieved = self.registry.get_or_retrieve(&uri)?;

        if fragment.starts_with('/') {
            let resolver = self.evolve(uri);
            return retrieved.pointer(fragment, resolver);
        }

        if !fragment.is_empty() {
            let retrieved = self.registry.anchor(&uri, fragment)?;
            let resolver = self.evolve(uri);
            return retrieved.resolve(resolver);
        }

        let resolver = self.evolve(uri);
        Ok(Resolved::new(
            retrieved.contents(),
            resolver,
            retrieved.draft(),
        ))
    }
    /// Resolve a recursive reference.
    ///
    /// This method implements the recursive reference resolution algorithm
    /// as specified in JSON Schema Draft 2019-09.
    ///
    /// It starts by resolving "#" and then follows the dynamic scope,
    /// looking for resources with `$recursiveAnchor: true`.
    ///
    /// # Errors
    ///
    /// This method can return any error that [`Resolver::lookup`] can return.
    pub fn lookup_recursive_ref(&self) -> Result<Resolved<'r>, Error> {
        let mut resolved = self.lookup("#")?;

        if let Value::Object(obj) = resolved.contents {
            if obj
                .get("$recursiveAnchor")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                for uri in self.dynamic_scope() {
                    let next_resolved = self.lookup(uri.as_str())?;

                    match next_resolved.contents {
                        Value::Object(next_obj) => {
                            if !next_obj
                                .get("$recursiveAnchor")
                                .and_then(Value::as_bool)
                                .unwrap_or(false)
                            {
                                break;
                            }
                        }
                        _ => break,
                    }

                    resolved = next_resolved;
                }
            }
        }

        Ok(resolved)
    }
    /// Create a resolver for a subresource.
    ///
    /// # Errors
    ///
    /// Returns an error if the resource id cannot be resolved against the base URI of this resolver.
    pub fn in_subresource(&self, subresource: ResourceRef) -> Result<Self, Error> {
        if let Some(id) = subresource.id() {
            let base_uri = uri::resolve_against(&self.base_uri.borrow(), id)?;
            Ok(Resolver {
                registry: self.registry,
                base_uri,
                scopes: self.scopes.clone(),
            })
        } else {
            Ok(self.clone())
        }
    }
    #[must_use]
    pub fn dynamic_scope(&self) -> impl ExactSizeIterator<Item = &UriRef<String>> {
        self.scopes.iter()
    }
    fn evolve(&self, base_uri: UriRef<String>) -> Resolver<'r> {
        if !self.base_uri.as_str().is_empty()
            && (self.scopes.is_empty() || base_uri != self.base_uri)
        {
            let mut scopes = self.scopes.clone();
            scopes.push_front(self.base_uri.clone());
            Resolver {
                registry: self.registry,
                base_uri,
                scopes,
            }
        } else {
            Resolver {
                registry: self.registry,
                base_uri,
                scopes: self.scopes.clone(),
            }
        }
    }
}

/// A reference resolved to its contents by a [`Resolver`].
#[derive(Debug)]
pub struct Resolved<'r> {
    /// The contents of the resolved reference.
    contents: &'r Value,
    /// The resolver that resolved this reference, which can be used for further resolutions.
    resolver: Resolver<'r>,
    draft: Draft,
}

impl<'r> Resolved<'r> {
    pub(crate) fn new(contents: &'r Value, resolver: Resolver<'r>, draft: Draft) -> Self {
        Self {
            contents,
            resolver,
            draft,
        }
    }
    /// Resolved contents.
    #[must_use]
    pub fn contents(&self) -> &Value {
        self.contents
    }
    /// Resolver used to resolve this contents.
    #[must_use]
    pub fn resolver(&self) -> &Resolver<'r> {
        &self.resolver
    }

    #[must_use]
    pub fn draft(&self) -> Draft {
        self.draft
    }
    #[must_use]
    pub fn into_inner(self) -> (&'r Value, Resolver<'r>, Draft) {
        (self.contents, self.resolver, self.draft)
    }
}