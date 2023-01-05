use std::borrow::Cow;

use debugid::DebugId;
use yoke::{Yoke, Yokeable};

use crate::{shared::AddressInfo, Error, FileLocation};

pub struct SymbolMap<FL: FileLocation> {
    debug_file_location: FL,
    pub(crate) inner: Box<dyn SymbolMapTrait>,
}

impl<FL: FileLocation> SymbolMap<FL> {
    pub(crate) fn new(debug_file_location: FL, inner: Box<dyn SymbolMapTrait>) -> Self {
        Self {
            debug_file_location,
            inner,
        }
    }

    pub fn debug_file_location(&self) -> &FL {
        &self.debug_file_location
    }

    pub fn debug_id(&self) -> debugid::DebugId {
        self.inner.debug_id()
    }

    pub fn symbol_count(&self) -> usize {
        self.inner.symbol_count()
    }

    pub fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_> {
        self.inner.iter_symbols()
    }

    pub fn lookup(&self, address: u32) -> Option<AddressInfo> {
        self.inner.lookup(address)
    }
}

pub trait SymbolMapTrait {
    fn debug_id(&self) -> DebugId;

    fn symbol_count(&self) -> usize;

    fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_>;

    fn lookup(&self, address: u32) -> Option<AddressInfo>;
}

pub trait SymbolMapDataOuterTrait {
    fn make_symbol_map_data_mid(&self) -> Result<Box<dyn SymbolMapDataMidTrait + '_>, Error>;
}

pub trait SymbolMapDataMidTrait {
    fn make_symbol_map_inner(&self) -> Result<SymbolMapInnerWrapper<'_>, Error>;
}

#[derive(Yokeable)]
pub struct SymbolMapDataMidWrapper<'data>(Box<dyn SymbolMapDataMidTrait + 'data>);

struct SymbolMapDataOuterAndMid<SMDO: SymbolMapDataOuterTrait>(
    Yoke<SymbolMapDataMidWrapper<'static>, Box<SMDO>>,
);

pub struct GenericSymbolMap<SMDO: SymbolMapDataOuterTrait>(
    Yoke<SymbolMapInnerWrapper<'static>, Box<SymbolMapDataOuterAndMid<SMDO>>>,
);

impl<SMDO: SymbolMapDataOuterTrait> GenericSymbolMap<SMDO> {
    pub fn new(outer: SMDO) -> Result<Self, Error> {
        let outer_and_mid = SymbolMapDataOuterAndMid(
            Yoke::<SymbolMapDataMidWrapper<'static>, _>::try_attach_to_cart(
                Box::new(outer),
                |outer| {
                    outer
                        .make_symbol_map_data_mid()
                        .map(SymbolMapDataMidWrapper)
                },
            )?,
        );
        let outer_and_mid_and_inner = Yoke::<SymbolMapInnerWrapper, _>::try_attach_to_cart(
            Box::new(outer_and_mid),
            |outer_and_mid| {
                let mid = outer_and_mid.0.get();
                mid.0.make_symbol_map_inner()
            },
        )?;
        Ok(GenericSymbolMap(outer_and_mid_and_inner))
    }
}

#[derive(Yokeable)]
pub struct SymbolMapInnerWrapper<'data>(pub Box<dyn SymbolMapTrait + 'data>);

impl<SMDO: SymbolMapDataOuterTrait> SymbolMapTrait for GenericSymbolMap<SMDO> {
    fn debug_id(&self) -> debugid::DebugId {
        self.0.get().0.debug_id()
    }

    fn symbol_count(&self) -> usize {
        self.0.get().0.symbol_count()
    }

    fn iter_symbols(&self) -> Box<dyn Iterator<Item = (u32, Cow<'_, str>)> + '_> {
        self.0.get().0.iter_symbols()
    }

    fn lookup(&self, address: u32) -> Option<AddressInfo> {
        self.0.get().0.lookup(address)
    }
}
