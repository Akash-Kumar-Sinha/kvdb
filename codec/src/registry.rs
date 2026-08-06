use std::collections::BTreeMap;

use crate::{BincodeCodec, Codec, JsonCodec};

/// An in-memory, name-keyed lookup table from codec name to [`Codec`] instance.
///
/// This is a small, in-process associative container, not a storage engine —
/// it holds a handful of codecs for the lifetime of the process, with no I/O
/// and no persistence. It lets a caller choose a wire format at runtime from
/// a string (an environment variable, a config file) instead of a compile-time
/// type parameter.
///
/// # Examples
///
/// ```
/// use codec::CodecRegistry;
///
/// let registry = CodecRegistry::default();
/// let codec = registry.create("json").expect("json ships built in");
/// assert_eq!(codec.name(), "json");
/// ```
#[derive(Debug, Clone)]
pub struct CodecRegistry {
    codecs: BTreeMap<&'static str, Box<dyn Codec>>,
}

impl CodecRegistry {
    /// An empty registry, with none of the built-in codecs registered.
    #[must_use]
    pub fn new() -> Self {
        CodecRegistry {
            codecs: BTreeMap::new(),
        }
    }

    /// Registers `codec`, keyed by its own [`Codec::name`].
    ///
    /// Returns whatever codec was previously registered under that name, if any.
    pub fn register(&mut self, codec: impl Codec + 'static) -> Option<Box<dyn Codec>> {
        self.codecs.insert(codec.name(), Box::new(codec))
    }

    /// Borrows the codec registered under `name`, if any.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&dyn Codec> {
        self.codecs.get(name).map(Box::as_ref)
    }

    /// Clones the codec registered under `name` into an owned `Box<dyn Codec>`.
    ///
    /// This is the method `KvDb::open_with_codec` needs, since it takes
    /// ownership of a boxed codec rather than a borrow.
    #[must_use]
    pub fn create(&self, name: &str) -> Option<Box<dyn Codec>> {
        self.get(name).map(Codec::boxed_clone)
    }

    /// Every registered codec's name, sorted.
    pub fn names(&self) -> impl Iterator<Item = &'static str> {
        self.codecs.keys().copied()
    }

    /// Every registered codec, sorted by name.
    pub fn codecs(&self) -> impl Iterator<Item = &dyn Codec> {
        self.codecs.values().map(Box::as_ref)
    }
}

impl Default for CodecRegistry {
    /// A registry pre-populated with every codec this crate ships:
    /// [`BincodeCodec`] and [`JsonCodec`].
    fn default() -> Self {
        let mut registry = CodecRegistry::new();
        registry.register(BincodeCodec);
        registry.register(JsonCodec);
        registry
    }
}
