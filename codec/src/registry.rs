use std::collections::BTreeMap;

use crate::{BincodeCodec, Codec, JsonCodec};

#[derive(Debug, Clone)]
pub struct CodecRegistry {
    codecs: BTreeMap<&'static str, Box<dyn Codec>>,
}

impl CodecRegistry {
    pub fn new() -> Self {
        CodecRegistry {
            codecs: BTreeMap::new(),
        }
    }

    pub fn register(&mut self, codec: impl Codec + 'static) -> Option<Box<dyn Codec>> {
        self.codecs.insert(codec.name(), Box::new(codec))
    }

    pub fn get(&self, name: &str) -> Option<&dyn Codec> {
        self.codecs.get(name).map(Box::as_ref)
    }

    pub fn create(&self, name: &str) -> Option<Box<dyn Codec>> {
        self.get(name).map(Codec::boxed_clone)
    }

    pub fn names(&self) -> impl Iterator<Item = &'static str> {
        self.codecs.keys().copied()
    }

    pub fn codecs(&self) -> impl Iterator<Item = &dyn Codec> {
        self.codecs.values().map(Box::as_ref)
    }
}

impl Default for CodecRegistry {
    fn default() -> Self {
        let mut registry = CodecRegistry::new();
        registry.register(BincodeCodec);
        registry.register(JsonCodec);
        registry
    }
}
