#![feature(prelude_import)]
#![allow(clippy::missing_safety_doc)]
#[prelude_import]
use std::prelude::rust_2021::*;
#[macro_use]
extern crate std;
#[allow(dead_code)]
pub mod wasi {
    #[allow(dead_code)]
    pub mod config {
        #[allow(dead_code, clippy::all)]
        pub mod runtime {
            use super::super::super::_rt;
            /// An error type that encapsulates the different errors that can occur fetching config
            pub enum ConfigError {
                /// This indicates an error from an "upstream" config source.
                /// As this could be almost _anything_ (such as Vault, Kubernetes ConfigMaps, KeyValue buckets, etc),
                /// the error message is a string.
                Upstream(_rt::String),
                /// This indicates an error from an I/O operation.
                /// As this could be almost _anything_ (such as a file read, network connection, etc),
                /// the error message is a string.
                /// Depending on how this ends up being consumed,
                /// we may consider moving this to use the `wasi:io/error` type instead.
                /// For simplicity right now in supporting multiple implementations, it is being left as a string.
                Io(_rt::String),
            }
            #[automatically_derived]
            impl ::core::clone::Clone for ConfigError {
                #[inline]
                fn clone(&self) -> ConfigError {
                    match self {
                        ConfigError::Upstream(__self_0) => {
                            ConfigError::Upstream(::core::clone::Clone::clone(__self_0))
                        }
                        ConfigError::Io(__self_0) => {
                            ConfigError::Io(::core::clone::Clone::clone(__self_0))
                        }
                    }
                }
            }
            impl ::core::fmt::Debug for ConfigError {
                fn fmt(
                    &self,
                    f: &mut ::core::fmt::Formatter<'_>,
                ) -> ::core::fmt::Result {
                    match self {
                        ConfigError::Upstream(e) => {
                            f.debug_tuple("ConfigError::Upstream").field(e).finish()
                        }
                        ConfigError::Io(e) => {
                            f.debug_tuple("ConfigError::Io").field(e).finish()
                        }
                    }
                }
            }
            impl ::core::fmt::Display for ConfigError {
                fn fmt(
                    &self,
                    f: &mut ::core::fmt::Formatter<'_>,
                ) -> ::core::fmt::Result {
                    f.write_fmt(format_args!("{0:?}", self))
                }
            }
            impl std::error::Error for ConfigError {}
            #[allow(unused_unsafe, clippy::all)]
            /// Gets a single opaque config value set at the given key if it exists
            pub fn get(key: &str) -> Result<Option<_rt::String>, ConfigError> {
                unsafe {
                    #[repr(align(4))]
                    struct RetArea([::core::mem::MaybeUninit<u8>; 16]);
                    let mut ret_area = RetArea([::core::mem::MaybeUninit::uninit(); 16]);
                    let vec0 = key;
                    let ptr0 = vec0.as_ptr().cast::<u8>();
                    let len0 = vec0.len();
                    let ptr1 = ret_area.0.as_mut_ptr().cast::<u8>();
                    #[cfg(not(target_arch = "wasm32"))]
                    fn wit_import(_: *mut u8, _: usize, _: *mut u8) {
                        ::core::panicking::panic(
                            "internal error: entered unreachable code",
                        )
                    }
                    wit_import(ptr0.cast_mut(), len0, ptr1);
                    let l2 = i32::from(*ptr1.add(0).cast::<u8>());
                    match l2 {
                        0 => {
                            let e = {
                                let l3 = i32::from(*ptr1.add(4).cast::<u8>());
                                match l3 {
                                    0 => None,
                                    1 => {
                                        let e = {
                                            let l4 = *ptr1.add(8).cast::<*mut u8>();
                                            let l5 = *ptr1.add(12).cast::<usize>();
                                            let len6 = l5;
                                            let bytes6 = _rt::Vec::from_raw_parts(
                                                l4.cast(),
                                                len6,
                                                len6,
                                            );
                                            _rt::string_lift(bytes6)
                                        };
                                        Some(e)
                                    }
                                    _ => _rt::invalid_enum_discriminant(),
                                }
                            };
                            Ok(e)
                        }
                        1 => {
                            let e = {
                                let l7 = i32::from(*ptr1.add(4).cast::<u8>());
                                let v14 = match l7 {
                                    0 => {
                                        let e14 = {
                                            let l8 = *ptr1.add(8).cast::<*mut u8>();
                                            let l9 = *ptr1.add(12).cast::<usize>();
                                            let len10 = l9;
                                            let bytes10 = _rt::Vec::from_raw_parts(
                                                l8.cast(),
                                                len10,
                                                len10,
                                            );
                                            _rt::string_lift(bytes10)
                                        };
                                        ConfigError::Upstream(e14)
                                    }
                                    n => {
                                        if true {
                                            match (&n, &1) {
                                                (left_val, right_val) => {
                                                    if !(*left_val == *right_val) {
                                                        let kind = ::core::panicking::AssertKind::Eq;
                                                        ::core::panicking::assert_failed(
                                                            kind,
                                                            &*left_val,
                                                            &*right_val,
                                                            ::core::option::Option::Some(
                                                                format_args!("invalid enum discriminant"),
                                                            ),
                                                        );
                                                    }
                                                }
                                            };
                                        }
                                        let e14 = {
                                            let l11 = *ptr1.add(8).cast::<*mut u8>();
                                            let l12 = *ptr1.add(12).cast::<usize>();
                                            let len13 = l12;
                                            let bytes13 = _rt::Vec::from_raw_parts(
                                                l11.cast(),
                                                len13,
                                                len13,
                                            );
                                            _rt::string_lift(bytes13)
                                        };
                                        ConfigError::Io(e14)
                                    }
                                };
                                v14
                            };
                            Err(e)
                        }
                        _ => _rt::invalid_enum_discriminant(),
                    }
                }
            }
            #[allow(unused_unsafe, clippy::all)]
            /// Gets a list of all set config data
            pub fn get_all() -> Result<
                _rt::Vec<(_rt::String, _rt::String)>,
                ConfigError,
            > {
                unsafe {
                    #[repr(align(4))]
                    struct RetArea([::core::mem::MaybeUninit<u8>; 16]);
                    let mut ret_area = RetArea([::core::mem::MaybeUninit::uninit(); 16]);
                    let ptr0 = ret_area.0.as_mut_ptr().cast::<u8>();
                    #[cfg(not(target_arch = "wasm32"))]
                    fn wit_import(_: *mut u8) {
                        ::core::panicking::panic(
                            "internal error: entered unreachable code",
                        )
                    }
                    wit_import(ptr0);
                    let l1 = i32::from(*ptr0.add(0).cast::<u8>());
                    match l1 {
                        0 => {
                            let e = {
                                let l2 = *ptr0.add(4).cast::<*mut u8>();
                                let l3 = *ptr0.add(8).cast::<usize>();
                                let base10 = l2;
                                let len10 = l3;
                                let mut result10 = _rt::Vec::with_capacity(len10);
                                for i in 0..len10 {
                                    let base = base10.add(i * 16);
                                    let e10 = {
                                        let l4 = *base.add(0).cast::<*mut u8>();
                                        let l5 = *base.add(4).cast::<usize>();
                                        let len6 = l5;
                                        let bytes6 = _rt::Vec::from_raw_parts(
                                            l4.cast(),
                                            len6,
                                            len6,
                                        );
                                        let l7 = *base.add(8).cast::<*mut u8>();
                                        let l8 = *base.add(12).cast::<usize>();
                                        let len9 = l8;
                                        let bytes9 = _rt::Vec::from_raw_parts(
                                            l7.cast(),
                                            len9,
                                            len9,
                                        );
                                        (_rt::string_lift(bytes6), _rt::string_lift(bytes9))
                                    };
                                    result10.push(e10);
                                }
                                _rt::cabi_dealloc(base10, len10 * 16, 4);
                                result10
                            };
                            Ok(e)
                        }
                        1 => {
                            let e = {
                                let l11 = i32::from(*ptr0.add(4).cast::<u8>());
                                let v18 = match l11 {
                                    0 => {
                                        let e18 = {
                                            let l12 = *ptr0.add(8).cast::<*mut u8>();
                                            let l13 = *ptr0.add(12).cast::<usize>();
                                            let len14 = l13;
                                            let bytes14 = _rt::Vec::from_raw_parts(
                                                l12.cast(),
                                                len14,
                                                len14,
                                            );
                                            _rt::string_lift(bytes14)
                                        };
                                        ConfigError::Upstream(e18)
                                    }
                                    n => {
                                        if true {
                                            match (&n, &1) {
                                                (left_val, right_val) => {
                                                    if !(*left_val == *right_val) {
                                                        let kind = ::core::panicking::AssertKind::Eq;
                                                        ::core::panicking::assert_failed(
                                                            kind,
                                                            &*left_val,
                                                            &*right_val,
                                                            ::core::option::Option::Some(
                                                                format_args!("invalid enum discriminant"),
                                                            ),
                                                        );
                                                    }
                                                }
                                            };
                                        }
                                        let e18 = {
                                            let l15 = *ptr0.add(8).cast::<*mut u8>();
                                            let l16 = *ptr0.add(12).cast::<usize>();
                                            let len17 = l16;
                                            let bytes17 = _rt::Vec::from_raw_parts(
                                                l15.cast(),
                                                len17,
                                                len17,
                                            );
                                            _rt::string_lift(bytes17)
                                        };
                                        ConfigError::Io(e18)
                                    }
                                };
                                v18
                            };
                            Err(e)
                        }
                        _ => _rt::invalid_enum_discriminant(),
                    }
                }
            }
        }
    }
    #[allow(dead_code)]
    pub mod keyvalue {
        #[allow(dead_code, clippy::all)]
        pub mod store {
            use super::super::super::_rt;
            /// The set of errors which may be raised by functions in this package
            pub enum Error {
                /// The host does not recognize the store identifier requested.
                NoSuchStore,
                /// The requesting component does not have access to the specified store
                /// (which may or may not exist).
                AccessDenied,
                /// Some implementation-specific error has occurred (e.g. I/O)
                Other(_rt::String),
            }
            #[automatically_derived]
            impl ::core::clone::Clone for Error {
                #[inline]
                fn clone(&self) -> Error {
                    match self {
                        Error::NoSuchStore => Error::NoSuchStore,
                        Error::AccessDenied => Error::AccessDenied,
                        Error::Other(__self_0) => {
                            Error::Other(::core::clone::Clone::clone(__self_0))
                        }
                    }
                }
            }
            impl ::core::fmt::Debug for Error {
                fn fmt(
                    &self,
                    f: &mut ::core::fmt::Formatter<'_>,
                ) -> ::core::fmt::Result {
                    match self {
                        Error::NoSuchStore => {
                            f.debug_tuple("Error::NoSuchStore").finish()
                        }
                        Error::AccessDenied => {
                            f.debug_tuple("Error::AccessDenied").finish()
                        }
                        Error::Other(e) => {
                            f.debug_tuple("Error::Other").field(e).finish()
                        }
                    }
                }
            }
            impl ::core::fmt::Display for Error {
                fn fmt(
                    &self,
                    f: &mut ::core::fmt::Formatter<'_>,
                ) -> ::core::fmt::Result {
                    f.write_fmt(format_args!("{0:?}", self))
                }
            }
            impl std::error::Error for Error {}
            /// A response to a `list-keys` operation.
            pub struct KeyResponse {
                /// The list of keys returned by the query.
                pub keys: _rt::Vec<_rt::String>,
                /// The continuation token to use to fetch the next page of keys. If this is `null`, then
                /// there are no more keys to fetch.
                pub cursor: Option<u64>,
            }
            #[automatically_derived]
            impl ::core::clone::Clone for KeyResponse {
                #[inline]
                fn clone(&self) -> KeyResponse {
                    KeyResponse {
                        keys: ::core::clone::Clone::clone(&self.keys),
                        cursor: ::core::clone::Clone::clone(&self.cursor),
                    }
                }
            }
            impl ::core::fmt::Debug for KeyResponse {
                fn fmt(
                    &self,
                    f: &mut ::core::fmt::Formatter<'_>,
                ) -> ::core::fmt::Result {
                    f.debug_struct("KeyResponse")
                        .field("keys", &self.keys)
                        .field("cursor", &self.cursor)
                        .finish()
                }
            }
            /// A bucket is a collection of key-value pairs. Each key-value pair is stored as a entry in the
            /// bucket, and the bucket itself acts as a collection of all these entries.
            ///
            /// It is worth noting that the exact terminology for bucket in key-value stores can very
            /// depending on the specific implementation. For example:
            ///
            /// 1. Amazon DynamoDB calls a collection of key-value pairs a table
            /// 2. Redis has hashes, sets, and sorted sets as different types of collections
            /// 3. Cassandra calls a collection of key-value pairs a column family
            /// 4. MongoDB calls a collection of key-value pairs a collection
            /// 5. Riak calls a collection of key-value pairs a bucket
            /// 6. Memcached calls a collection of key-value pairs a slab
            /// 7. Azure Cosmos DB calls a collection of key-value pairs a container
            ///
            /// In this interface, we use the term `bucket` to refer to a collection of key-value pairs
            #[repr(transparent)]
            pub struct Bucket {
                handle: _rt::Resource<Bucket>,
            }
            #[automatically_derived]
            impl ::core::fmt::Debug for Bucket {
                #[inline]
                fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
                    ::core::fmt::Formatter::debug_struct_field1_finish(
                        f,
                        "Bucket",
                        "handle",
                        &&self.handle,
                    )
                }
            }
            impl Bucket {
                #[doc(hidden)]
                pub unsafe fn from_handle(handle: u32) -> Self {
                    Self {
                        handle: _rt::Resource::from_handle(handle),
                    }
                }
                #[doc(hidden)]
                pub fn take_handle(&self) -> u32 {
                    _rt::Resource::take_handle(&self.handle)
                }
                #[doc(hidden)]
                pub fn handle(&self) -> u32 {
                    _rt::Resource::handle(&self.handle)
                }
            }
            unsafe impl _rt::WasmResource for Bucket {
                #[inline]
                unsafe fn drop(_handle: u32) {
                    ::core::panicking::panic("internal error: entered unreachable code");
                }
            }
            #[allow(unused_unsafe, clippy::all)]
            /// Get the bucket with the specified identifier.
            ///
            /// `identifier` must refer to a bucket provided by the host.
            ///
            /// `error::no-such-store` will be raised if the `identifier` is not recognized.
            pub fn open(identifier: &str) -> Result<Bucket, Error> {
                unsafe {
                    #[repr(align(4))]
                    struct RetArea([::core::mem::MaybeUninit<u8>; 16]);
                    let mut ret_area = RetArea([::core::mem::MaybeUninit::uninit(); 16]);
                    let vec0 = identifier;
                    let ptr0 = vec0.as_ptr().cast::<u8>();
                    let len0 = vec0.len();
                    let ptr1 = ret_area.0.as_mut_ptr().cast::<u8>();
                    #[cfg(not(target_arch = "wasm32"))]
                    fn wit_import(_: *mut u8, _: usize, _: *mut u8) {
                        ::core::panicking::panic(
                            "internal error: entered unreachable code",
                        )
                    }
                    wit_import(ptr0.cast_mut(), len0, ptr1);
                    let l2 = i32::from(*ptr1.add(0).cast::<u8>());
                    match l2 {
                        0 => {
                            let e = {
                                let l3 = *ptr1.add(4).cast::<i32>();
                                Bucket::from_handle(l3 as u32)
                            };
                            Ok(e)
                        }
                        1 => {
                            let e = {
                                let l4 = i32::from(*ptr1.add(4).cast::<u8>());
                                let v8 = match l4 {
                                    0 => Error::NoSuchStore,
                                    1 => Error::AccessDenied,
                                    n => {
                                        if true {
                                            match (&n, &2) {
                                                (left_val, right_val) => {
                                                    if !(*left_val == *right_val) {
                                                        let kind = ::core::panicking::AssertKind::Eq;
                                                        ::core::panicking::assert_failed(
                                                            kind,
                                                            &*left_val,
                                                            &*right_val,
                                                            ::core::option::Option::Some(
                                                                format_args!("invalid enum discriminant"),
                                                            ),
                                                        );
                                                    }
                                                }
                                            };
                                        }
                                        let e8 = {
                                            let l5 = *ptr1.add(8).cast::<*mut u8>();
                                            let l6 = *ptr1.add(12).cast::<usize>();
                                            let len7 = l6;
                                            let bytes7 = _rt::Vec::from_raw_parts(
                                                l5.cast(),
                                                len7,
                                                len7,
                                            );
                                            _rt::string_lift(bytes7)
                                        };
                                        Error::Other(e8)
                                    }
                                };
                                v8
                            };
                            Err(e)
                        }
                        _ => _rt::invalid_enum_discriminant(),
                    }
                }
            }
            impl Bucket {
                #[allow(unused_unsafe, clippy::all)]
                /// Get the value associated with the specified `key`
                ///
                /// The value is returned as an option. If the key-value pair exists in the
                /// store, it returns `Ok(value)`. If the key does not exist in the
                /// store, it returns `Ok(none)`.
                ///
                /// If any other error occurs, it returns an `Err(error)`.
                pub fn get(&self, key: &str) -> Result<Option<_rt::Vec<u8>>, Error> {
                    unsafe {
                        #[repr(align(4))]
                        struct RetArea([::core::mem::MaybeUninit<u8>; 16]);
                        let mut ret_area = RetArea(
                            [::core::mem::MaybeUninit::uninit(); 16],
                        );
                        let vec0 = key;
                        let ptr0 = vec0.as_ptr().cast::<u8>();
                        let len0 = vec0.len();
                        let ptr1 = ret_area.0.as_mut_ptr().cast::<u8>();
                        #[cfg(not(target_arch = "wasm32"))]
                        fn wit_import(_: i32, _: *mut u8, _: usize, _: *mut u8) {
                            ::core::panicking::panic(
                                "internal error: entered unreachable code",
                            )
                        }
                        wit_import((self).handle() as i32, ptr0.cast_mut(), len0, ptr1);
                        let l2 = i32::from(*ptr1.add(0).cast::<u8>());
                        match l2 {
                            0 => {
                                let e = {
                                    let l3 = i32::from(*ptr1.add(4).cast::<u8>());
                                    match l3 {
                                        0 => None,
                                        1 => {
                                            let e = {
                                                let l4 = *ptr1.add(8).cast::<*mut u8>();
                                                let l5 = *ptr1.add(12).cast::<usize>();
                                                let len6 = l5;
                                                _rt::Vec::from_raw_parts(l4.cast(), len6, len6)
                                            };
                                            Some(e)
                                        }
                                        _ => _rt::invalid_enum_discriminant(),
                                    }
                                };
                                Ok(e)
                            }
                            1 => {
                                let e = {
                                    let l7 = i32::from(*ptr1.add(4).cast::<u8>());
                                    let v11 = match l7 {
                                        0 => Error::NoSuchStore,
                                        1 => Error::AccessDenied,
                                        n => {
                                            if true {
                                                match (&n, &2) {
                                                    (left_val, right_val) => {
                                                        if !(*left_val == *right_val) {
                                                            let kind = ::core::panicking::AssertKind::Eq;
                                                            ::core::panicking::assert_failed(
                                                                kind,
                                                                &*left_val,
                                                                &*right_val,
                                                                ::core::option::Option::Some(
                                                                    format_args!("invalid enum discriminant"),
                                                                ),
                                                            );
                                                        }
                                                    }
                                                };
                                            }
                                            let e11 = {
                                                let l8 = *ptr1.add(8).cast::<*mut u8>();
                                                let l9 = *ptr1.add(12).cast::<usize>();
                                                let len10 = l9;
                                                let bytes10 = _rt::Vec::from_raw_parts(
                                                    l8.cast(),
                                                    len10,
                                                    len10,
                                                );
                                                _rt::string_lift(bytes10)
                                            };
                                            Error::Other(e11)
                                        }
                                    };
                                    v11
                                };
                                Err(e)
                            }
                            _ => _rt::invalid_enum_discriminant(),
                        }
                    }
                }
            }
            impl Bucket {
                #[allow(unused_unsafe, clippy::all)]
                /// Set the value associated with the key in the store. If the key already
                /// exists in the store, it overwrites the value.
                ///
                /// If the key does not exist in the store, it creates a new key-value pair.
                ///
                /// If any other error occurs, it returns an `Err(error)`.
                pub fn set(&self, key: &str, value: &[u8]) -> Result<(), Error> {
                    unsafe {
                        #[repr(align(4))]
                        struct RetArea([::core::mem::MaybeUninit<u8>; 16]);
                        let mut ret_area = RetArea(
                            [::core::mem::MaybeUninit::uninit(); 16],
                        );
                        let vec0 = key;
                        let ptr0 = vec0.as_ptr().cast::<u8>();
                        let len0 = vec0.len();
                        let vec1 = value;
                        let ptr1 = vec1.as_ptr().cast::<u8>();
                        let len1 = vec1.len();
                        let ptr2 = ret_area.0.as_mut_ptr().cast::<u8>();
                        #[cfg(not(target_arch = "wasm32"))]
                        fn wit_import(
                            _: i32,
                            _: *mut u8,
                            _: usize,
                            _: *mut u8,
                            _: usize,
                            _: *mut u8,
                        ) {
                            ::core::panicking::panic(
                                "internal error: entered unreachable code",
                            )
                        }
                        wit_import(
                            (self).handle() as i32,
                            ptr0.cast_mut(),
                            len0,
                            ptr1.cast_mut(),
                            len1,
                            ptr2,
                        );
                        let l3 = i32::from(*ptr2.add(0).cast::<u8>());
                        match l3 {
                            0 => {
                                let e = ();
                                Ok(e)
                            }
                            1 => {
                                let e = {
                                    let l4 = i32::from(*ptr2.add(4).cast::<u8>());
                                    let v8 = match l4 {
                                        0 => Error::NoSuchStore,
                                        1 => Error::AccessDenied,
                                        n => {
                                            if true {
                                                match (&n, &2) {
                                                    (left_val, right_val) => {
                                                        if !(*left_val == *right_val) {
                                                            let kind = ::core::panicking::AssertKind::Eq;
                                                            ::core::panicking::assert_failed(
                                                                kind,
                                                                &*left_val,
                                                                &*right_val,
                                                                ::core::option::Option::Some(
                                                                    format_args!("invalid enum discriminant"),
                                                                ),
                                                            );
                                                        }
                                                    }
                                                };
                                            }
                                            let e8 = {
                                                let l5 = *ptr2.add(8).cast::<*mut u8>();
                                                let l6 = *ptr2.add(12).cast::<usize>();
                                                let len7 = l6;
                                                let bytes7 = _rt::Vec::from_raw_parts(
                                                    l5.cast(),
                                                    len7,
                                                    len7,
                                                );
                                                _rt::string_lift(bytes7)
                                            };
                                            Error::Other(e8)
                                        }
                                    };
                                    v8
                                };
                                Err(e)
                            }
                            _ => _rt::invalid_enum_discriminant(),
                        }
                    }
                }
            }
            impl Bucket {
                #[allow(unused_unsafe, clippy::all)]
                /// Delete the key-value pair associated with the key in the store.
                ///
                /// If the key does not exist in the store, it does nothing.
                ///
                /// If any other error occurs, it returns an `Err(error)`.
                pub fn delete(&self, key: &str) -> Result<(), Error> {
                    unsafe {
                        #[repr(align(4))]
                        struct RetArea([::core::mem::MaybeUninit<u8>; 16]);
                        let mut ret_area = RetArea(
                            [::core::mem::MaybeUninit::uninit(); 16],
                        );
                        let vec0 = key;
                        let ptr0 = vec0.as_ptr().cast::<u8>();
                        let len0 = vec0.len();
                        let ptr1 = ret_area.0.as_mut_ptr().cast::<u8>();
                        #[cfg(not(target_arch = "wasm32"))]
                        fn wit_import(_: i32, _: *mut u8, _: usize, _: *mut u8) {
                            ::core::panicking::panic(
                                "internal error: entered unreachable code",
                            )
                        }
                        wit_import((self).handle() as i32, ptr0.cast_mut(), len0, ptr1);
                        let l2 = i32::from(*ptr1.add(0).cast::<u8>());
                        match l2 {
                            0 => {
                                let e = ();
                                Ok(e)
                            }
                            1 => {
                                let e = {
                                    let l3 = i32::from(*ptr1.add(4).cast::<u8>());
                                    let v7 = match l3 {
                                        0 => Error::NoSuchStore,
                                        1 => Error::AccessDenied,
                                        n => {
                                            if true {
                                                match (&n, &2) {
                                                    (left_val, right_val) => {
                                                        if !(*left_val == *right_val) {
                                                            let kind = ::core::panicking::AssertKind::Eq;
                                                            ::core::panicking::assert_failed(
                                                                kind,
                                                                &*left_val,
                                                                &*right_val,
                                                                ::core::option::Option::Some(
                                                                    format_args!("invalid enum discriminant"),
                                                                ),
                                                            );
                                                        }
                                                    }
                                                };
                                            }
                                            let e7 = {
                                                let l4 = *ptr1.add(8).cast::<*mut u8>();
                                                let l5 = *ptr1.add(12).cast::<usize>();
                                                let len6 = l5;
                                                let bytes6 = _rt::Vec::from_raw_parts(
                                                    l4.cast(),
                                                    len6,
                                                    len6,
                                                );
                                                _rt::string_lift(bytes6)
                                            };
                                            Error::Other(e7)
                                        }
                                    };
                                    v7
                                };
                                Err(e)
                            }
                            _ => _rt::invalid_enum_discriminant(),
                        }
                    }
                }
            }
            impl Bucket {
                #[allow(unused_unsafe, clippy::all)]
                /// Check if the key exists in the store.
                ///
                /// If the key exists in the store, it returns `Ok(true)`. If the key does
                /// not exist in the store, it returns `Ok(false)`.
                ///
                /// If any other error occurs, it returns an `Err(error)`.
                pub fn exists(&self, key: &str) -> Result<bool, Error> {
                    unsafe {
                        #[repr(align(4))]
                        struct RetArea([::core::mem::MaybeUninit<u8>; 16]);
                        let mut ret_area = RetArea(
                            [::core::mem::MaybeUninit::uninit(); 16],
                        );
                        let vec0 = key;
                        let ptr0 = vec0.as_ptr().cast::<u8>();
                        let len0 = vec0.len();
                        let ptr1 = ret_area.0.as_mut_ptr().cast::<u8>();
                        #[cfg(not(target_arch = "wasm32"))]
                        fn wit_import(_: i32, _: *mut u8, _: usize, _: *mut u8) {
                            ::core::panicking::panic(
                                "internal error: entered unreachable code",
                            )
                        }
                        wit_import((self).handle() as i32, ptr0.cast_mut(), len0, ptr1);
                        let l2 = i32::from(*ptr1.add(0).cast::<u8>());
                        match l2 {
                            0 => {
                                let e = {
                                    let l3 = i32::from(*ptr1.add(4).cast::<u8>());
                                    _rt::bool_lift(l3 as u8)
                                };
                                Ok(e)
                            }
                            1 => {
                                let e = {
                                    let l4 = i32::from(*ptr1.add(4).cast::<u8>());
                                    let v8 = match l4 {
                                        0 => Error::NoSuchStore,
                                        1 => Error::AccessDenied,
                                        n => {
                                            if true {
                                                match (&n, &2) {
                                                    (left_val, right_val) => {
                                                        if !(*left_val == *right_val) {
                                                            let kind = ::core::panicking::AssertKind::Eq;
                                                            ::core::panicking::assert_failed(
                                                                kind,
                                                                &*left_val,
                                                                &*right_val,
                                                                ::core::option::Option::Some(
                                                                    format_args!("invalid enum discriminant"),
                                                                ),
                                                            );
                                                        }
                                                    }
                                                };
                                            }
                                            let e8 = {
                                                let l5 = *ptr1.add(8).cast::<*mut u8>();
                                                let l6 = *ptr1.add(12).cast::<usize>();
                                                let len7 = l6;
                                                let bytes7 = _rt::Vec::from_raw_parts(
                                                    l5.cast(),
                                                    len7,
                                                    len7,
                                                );
                                                _rt::string_lift(bytes7)
                                            };
                                            Error::Other(e8)
                                        }
                                    };
                                    v8
                                };
                                Err(e)
                            }
                            _ => _rt::invalid_enum_discriminant(),
                        }
                    }
                }
            }
            impl Bucket {
                #[allow(unused_unsafe, clippy::all)]
                /// Get all the keys in the store with an optional cursor (for use in pagination). It
                /// returns a list of keys. Please note that for most KeyValue implementations, this is a
                /// can be a very expensive operation and so it should be used judiciously. Implementations
                /// can return any number of keys in a single response, but they should never attempt to
                /// send more data than is reasonable (i.e. on a small edge device, this may only be a few
                /// KB, while on a large machine this could be several MB). Any response should also return
                /// a cursor that can be used to fetch the next page of keys. See the `key-response` record
                /// for more information.
                ///
                /// Note that the keys are not guaranteed to be returned in any particular order.
                ///
                /// If the store is empty, it returns an empty list.
                ///
                /// MAY show an out-of-date list of keys if there are concurrent writes to the store.
                ///
                /// If any error occurs, it returns an `Err(error)`.
                pub fn list_keys(
                    &self,
                    cursor: Option<u64>,
                ) -> Result<KeyResponse, Error> {
                    unsafe {
                        #[repr(align(8))]
                        struct RetArea([::core::mem::MaybeUninit<u8>; 32]);
                        let mut ret_area = RetArea(
                            [::core::mem::MaybeUninit::uninit(); 32],
                        );
                        let (result0_0, result0_1) = match cursor {
                            Some(e) => (1i32, _rt::as_i64(e)),
                            None => (0i32, 0i64),
                        };
                        let ptr1 = ret_area.0.as_mut_ptr().cast::<u8>();
                        #[cfg(not(target_arch = "wasm32"))]
                        fn wit_import(_: i32, _: i32, _: i64, _: *mut u8) {
                            ::core::panicking::panic(
                                "internal error: entered unreachable code",
                            )
                        }
                        wit_import((self).handle() as i32, result0_0, result0_1, ptr1);
                        let l2 = i32::from(*ptr1.add(0).cast::<u8>());
                        match l2 {
                            0 => {
                                let e = {
                                    let l3 = *ptr1.add(8).cast::<*mut u8>();
                                    let l4 = *ptr1.add(12).cast::<usize>();
                                    let base8 = l3;
                                    let len8 = l4;
                                    let mut result8 = _rt::Vec::with_capacity(len8);
                                    for i in 0..len8 {
                                        let base = base8.add(i * 8);
                                        let e8 = {
                                            let l5 = *base.add(0).cast::<*mut u8>();
                                            let l6 = *base.add(4).cast::<usize>();
                                            let len7 = l6;
                                            let bytes7 = _rt::Vec::from_raw_parts(
                                                l5.cast(),
                                                len7,
                                                len7,
                                            );
                                            _rt::string_lift(bytes7)
                                        };
                                        result8.push(e8);
                                    }
                                    _rt::cabi_dealloc(base8, len8 * 8, 4);
                                    let l9 = i32::from(*ptr1.add(16).cast::<u8>());
                                    KeyResponse {
                                        keys: result8,
                                        cursor: match l9 {
                                            0 => None,
                                            1 => {
                                                let e = {
                                                    let l10 = *ptr1.add(24).cast::<i64>();
                                                    l10 as u64
                                                };
                                                Some(e)
                                            }
                                            _ => _rt::invalid_enum_discriminant(),
                                        },
                                    }
                                };
                                Ok(e)
                            }
                            1 => {
                                let e = {
                                    let l11 = i32::from(*ptr1.add(8).cast::<u8>());
                                    let v15 = match l11 {
                                        0 => Error::NoSuchStore,
                                        1 => Error::AccessDenied,
                                        n => {
                                            if true {
                                                match (&n, &2) {
                                                    (left_val, right_val) => {
                                                        if !(*left_val == *right_val) {
                                                            let kind = ::core::panicking::AssertKind::Eq;
                                                            ::core::panicking::assert_failed(
                                                                kind,
                                                                &*left_val,
                                                                &*right_val,
                                                                ::core::option::Option::Some(
                                                                    format_args!("invalid enum discriminant"),
                                                                ),
                                                            );
                                                        }
                                                    }
                                                };
                                            }
                                            let e15 = {
                                                let l12 = *ptr1.add(12).cast::<*mut u8>();
                                                let l13 = *ptr1.add(16).cast::<usize>();
                                                let len14 = l13;
                                                let bytes14 = _rt::Vec::from_raw_parts(
                                                    l12.cast(),
                                                    len14,
                                                    len14,
                                                );
                                                _rt::string_lift(bytes14)
                                            };
                                            Error::Other(e15)
                                        }
                                    };
                                    v15
                                };
                                Err(e)
                            }
                            _ => _rt::invalid_enum_discriminant(),
                        }
                    }
                }
            }
        }
        #[allow(dead_code, clippy::all)]
        pub mod atomics {
            use super::super::super::_rt;
            pub type Bucket = super::super::super::wasi::keyvalue::store::Bucket;
            pub type Error = super::super::super::wasi::keyvalue::store::Error;
            #[allow(unused_unsafe, clippy::all)]
            /// Atomically increment the value associated with the key in the store by the given delta. It
            /// returns the new value.
            ///
            /// If the key does not exist in the store, it creates a new key-value pair with the value set
            /// to the given delta.
            ///
            /// If any other error occurs, it returns an `Err(error)`.
            pub fn increment(
                bucket: &Bucket,
                key: &str,
                delta: u64,
            ) -> Result<u64, Error> {
                unsafe {
                    #[repr(align(8))]
                    struct RetArea([::core::mem::MaybeUninit<u8>; 24]);
                    let mut ret_area = RetArea([::core::mem::MaybeUninit::uninit(); 24]);
                    let vec0 = key;
                    let ptr0 = vec0.as_ptr().cast::<u8>();
                    let len0 = vec0.len();
                    let ptr1 = ret_area.0.as_mut_ptr().cast::<u8>();
                    #[cfg(not(target_arch = "wasm32"))]
                    fn wit_import(_: i32, _: *mut u8, _: usize, _: i64, _: *mut u8) {
                        ::core::panicking::panic(
                            "internal error: entered unreachable code",
                        )
                    }
                    wit_import(
                        (bucket).handle() as i32,
                        ptr0.cast_mut(),
                        len0,
                        _rt::as_i64(&delta),
                        ptr1,
                    );
                    let l2 = i32::from(*ptr1.add(0).cast::<u8>());
                    match l2 {
                        0 => {
                            let e = {
                                let l3 = *ptr1.add(8).cast::<i64>();
                                l3 as u64
                            };
                            Ok(e)
                        }
                        1 => {
                            let e = {
                                let l4 = i32::from(*ptr1.add(8).cast::<u8>());
                                use super::super::super::wasi::keyvalue::store::Error as V8;
                                let v8 = match l4 {
                                    0 => V8::NoSuchStore,
                                    1 => V8::AccessDenied,
                                    n => {
                                        if true {
                                            match (&n, &2) {
                                                (left_val, right_val) => {
                                                    if !(*left_val == *right_val) {
                                                        let kind = ::core::panicking::AssertKind::Eq;
                                                        ::core::panicking::assert_failed(
                                                            kind,
                                                            &*left_val,
                                                            &*right_val,
                                                            ::core::option::Option::Some(
                                                                format_args!("invalid enum discriminant"),
                                                            ),
                                                        );
                                                    }
                                                }
                                            };
                                        }
                                        let e8 = {
                                            let l5 = *ptr1.add(12).cast::<*mut u8>();
                                            let l6 = *ptr1.add(16).cast::<usize>();
                                            let len7 = l6;
                                            let bytes7 = _rt::Vec::from_raw_parts(
                                                l5.cast(),
                                                len7,
                                                len7,
                                            );
                                            _rt::string_lift(bytes7)
                                        };
                                        V8::Other(e8)
                                    }
                                };
                                v8
                            };
                            Err(e)
                        }
                        _ => _rt::invalid_enum_discriminant(),
                    }
                }
            }
        }
    }
    #[allow(dead_code)]
    pub mod logging {
        #[allow(dead_code, clippy::all)]
        pub mod logging {
            /// A log level, describing a kind of message.
            #[repr(u8)]
            pub enum Level {
                /// Describes messages about the values of variables and the flow of
                /// control within a program.
                Trace,
                /// Describes messages likely to be of interest to someone debugging a
                /// program.
                Debug,
                /// Describes messages likely to be of interest to someone monitoring a
                /// program.
                Info,
                /// Describes messages indicating hazardous situations.
                Warn,
                /// Describes messages indicating serious errors.
                Error,
                /// Describes messages indicating fatal errors.
                Critical,
            }
            #[automatically_derived]
            impl ::core::clone::Clone for Level {
                #[inline]
                fn clone(&self) -> Level {
                    *self
                }
            }
            #[automatically_derived]
            impl ::core::marker::Copy for Level {}
            #[automatically_derived]
            impl ::core::cmp::Eq for Level {
                #[inline]
                #[doc(hidden)]
                #[coverage(off)]
                fn assert_receiver_is_total_eq(&self) -> () {}
            }
            #[automatically_derived]
            impl ::core::marker::StructuralPartialEq for Level {}
            #[automatically_derived]
            impl ::core::cmp::PartialEq for Level {
                #[inline]
                fn eq(&self, other: &Level) -> bool {
                    let __self_tag = ::core::intrinsics::discriminant_value(self);
                    let __arg1_tag = ::core::intrinsics::discriminant_value(other);
                    __self_tag == __arg1_tag
                }
            }
            impl ::core::fmt::Debug for Level {
                fn fmt(
                    &self,
                    f: &mut ::core::fmt::Formatter<'_>,
                ) -> ::core::fmt::Result {
                    match self {
                        Level::Trace => f.debug_tuple("Level::Trace").finish(),
                        Level::Debug => f.debug_tuple("Level::Debug").finish(),
                        Level::Info => f.debug_tuple("Level::Info").finish(),
                        Level::Warn => f.debug_tuple("Level::Warn").finish(),
                        Level::Error => f.debug_tuple("Level::Error").finish(),
                        Level::Critical => f.debug_tuple("Level::Critical").finish(),
                    }
                }
            }
            impl Level {
                #[doc(hidden)]
                pub unsafe fn _lift(val: u8) -> Level {
                    if !true {
                        return ::core::mem::transmute(val);
                    }
                    match val {
                        0 => Level::Trace,
                        1 => Level::Debug,
                        2 => Level::Info,
                        3 => Level::Warn,
                        4 => Level::Error,
                        5 => Level::Critical,
                        _ => {
                            ::core::panicking::panic_fmt(
                                format_args!("invalid enum discriminant"),
                            );
                        }
                    }
                }
            }
            #[allow(unused_unsafe, clippy::all)]
            /// Emit a log message.
            ///
            /// A log message has a `level` describing what kind of message is being
            /// sent, a context, which is an uninterpreted string meant to help
            /// consumers group similar messages, and a string containing the message
            /// text.
            pub fn log(level: Level, context: &str, message: &str) {
                unsafe {
                    let vec0 = context;
                    let ptr0 = vec0.as_ptr().cast::<u8>();
                    let len0 = vec0.len();
                    let vec1 = message;
                    let ptr1 = vec1.as_ptr().cast::<u8>();
                    let len1 = vec1.len();
                    #[cfg(not(target_arch = "wasm32"))]
                    fn wit_import(_: i32, _: *mut u8, _: usize, _: *mut u8, _: usize) {
                        ::core::panicking::panic(
                            "internal error: entered unreachable code",
                        )
                    }
                    wit_import(
                        level.clone() as i32,
                        ptr0.cast_mut(),
                        len0,
                        ptr1.cast_mut(),
                        len1,
                    );
                }
            }
        }
    }
}
#[allow(dead_code)]
pub mod wasmcloud {
    #[allow(dead_code)]
    pub mod messaging {
        #[allow(dead_code, clippy::all)]
        pub mod types {
            use super::super::super::_rt;
            /// A message sent to or received from a broker
            pub struct BrokerMessage {
                pub subject: _rt::String,
                pub body: _rt::Vec<u8>,
                pub reply_to: Option<_rt::String>,
            }
            #[automatically_derived]
            impl ::core::clone::Clone for BrokerMessage {
                #[inline]
                fn clone(&self) -> BrokerMessage {
                    BrokerMessage {
                        subject: ::core::clone::Clone::clone(&self.subject),
                        body: ::core::clone::Clone::clone(&self.body),
                        reply_to: ::core::clone::Clone::clone(&self.reply_to),
                    }
                }
            }
            impl ::core::fmt::Debug for BrokerMessage {
                fn fmt(
                    &self,
                    f: &mut ::core::fmt::Formatter<'_>,
                ) -> ::core::fmt::Result {
                    f.debug_struct("BrokerMessage")
                        .field("subject", &self.subject)
                        .field("body", &self.body)
                        .field("reply-to", &self.reply_to)
                        .finish()
                }
            }
        }
        #[allow(dead_code, clippy::all)]
        pub mod consumer {
            use super::super::super::_rt;
            pub type BrokerMessage = super::super::super::wasmcloud::messaging::types::BrokerMessage;
            #[allow(unused_unsafe, clippy::all)]
            /// Perform a request operation on a subject
            pub fn request(
                subject: &str,
                body: &[u8],
                timeout_ms: u32,
            ) -> Result<BrokerMessage, _rt::String> {
                unsafe {
                    #[repr(align(4))]
                    struct RetArea([::core::mem::MaybeUninit<u8>; 32]);
                    let mut ret_area = RetArea([::core::mem::MaybeUninit::uninit(); 32]);
                    let vec0 = subject;
                    let ptr0 = vec0.as_ptr().cast::<u8>();
                    let len0 = vec0.len();
                    let vec1 = body;
                    let ptr1 = vec1.as_ptr().cast::<u8>();
                    let len1 = vec1.len();
                    let ptr2 = ret_area.0.as_mut_ptr().cast::<u8>();
                    #[cfg(not(target_arch = "wasm32"))]
                    fn wit_import(
                        _: *mut u8,
                        _: usize,
                        _: *mut u8,
                        _: usize,
                        _: i32,
                        _: *mut u8,
                    ) {
                        ::core::panicking::panic(
                            "internal error: entered unreachable code",
                        )
                    }
                    wit_import(
                        ptr0.cast_mut(),
                        len0,
                        ptr1.cast_mut(),
                        len1,
                        _rt::as_i32(&timeout_ms),
                        ptr2,
                    );
                    let l3 = i32::from(*ptr2.add(0).cast::<u8>());
                    match l3 {
                        0 => {
                            let e = {
                                let l4 = *ptr2.add(4).cast::<*mut u8>();
                                let l5 = *ptr2.add(8).cast::<usize>();
                                let len6 = l5;
                                let bytes6 = _rt::Vec::from_raw_parts(
                                    l4.cast(),
                                    len6,
                                    len6,
                                );
                                let l7 = *ptr2.add(12).cast::<*mut u8>();
                                let l8 = *ptr2.add(16).cast::<usize>();
                                let len9 = l8;
                                let l10 = i32::from(*ptr2.add(20).cast::<u8>());
                                super::super::super::wasmcloud::messaging::types::BrokerMessage {
                                    subject: _rt::string_lift(bytes6),
                                    body: _rt::Vec::from_raw_parts(l7.cast(), len9, len9),
                                    reply_to: match l10 {
                                        0 => None,
                                        1 => {
                                            let e = {
                                                let l11 = *ptr2.add(24).cast::<*mut u8>();
                                                let l12 = *ptr2.add(28).cast::<usize>();
                                                let len13 = l12;
                                                let bytes13 = _rt::Vec::from_raw_parts(
                                                    l11.cast(),
                                                    len13,
                                                    len13,
                                                );
                                                _rt::string_lift(bytes13)
                                            };
                                            Some(e)
                                        }
                                        _ => _rt::invalid_enum_discriminant(),
                                    },
                                }
                            };
                            Ok(e)
                        }
                        1 => {
                            let e = {
                                let l14 = *ptr2.add(4).cast::<*mut u8>();
                                let l15 = *ptr2.add(8).cast::<usize>();
                                let len16 = l15;
                                let bytes16 = _rt::Vec::from_raw_parts(
                                    l14.cast(),
                                    len16,
                                    len16,
                                );
                                _rt::string_lift(bytes16)
                            };
                            Err(e)
                        }
                        _ => _rt::invalid_enum_discriminant(),
                    }
                }
            }
            #[allow(unused_unsafe, clippy::all)]
            /// Publish a message to a subject without awaiting a response
            pub fn publish(msg: &BrokerMessage) -> Result<(), _rt::String> {
                unsafe {
                    #[repr(align(4))]
                    struct RetArea([::core::mem::MaybeUninit<u8>; 12]);
                    let mut ret_area = RetArea([::core::mem::MaybeUninit::uninit(); 12]);
                    let super::super::super::wasmcloud::messaging::types::BrokerMessage {
                        subject: subject0,
                        body: body0,
                        reply_to: reply_to0,
                    } = msg;
                    let vec1 = subject0;
                    let ptr1 = vec1.as_ptr().cast::<u8>();
                    let len1 = vec1.len();
                    let vec2 = body0;
                    let ptr2 = vec2.as_ptr().cast::<u8>();
                    let len2 = vec2.len();
                    let (result4_0, result4_1, result4_2) = match reply_to0 {
                        Some(e) => {
                            let vec3 = e;
                            let ptr3 = vec3.as_ptr().cast::<u8>();
                            let len3 = vec3.len();
                            (1i32, ptr3.cast_mut(), len3)
                        }
                        None => (0i32, ::core::ptr::null_mut(), 0usize),
                    };
                    let ptr5 = ret_area.0.as_mut_ptr().cast::<u8>();
                    #[cfg(not(target_arch = "wasm32"))]
                    fn wit_import(
                        _: *mut u8,
                        _: usize,
                        _: *mut u8,
                        _: usize,
                        _: i32,
                        _: *mut u8,
                        _: usize,
                        _: *mut u8,
                    ) {
                        ::core::panicking::panic(
                            "internal error: entered unreachable code",
                        )
                    }
                    wit_import(
                        ptr1.cast_mut(),
                        len1,
                        ptr2.cast_mut(),
                        len2,
                        result4_0,
                        result4_1,
                        result4_2,
                        ptr5,
                    );
                    let l6 = i32::from(*ptr5.add(0).cast::<u8>());
                    match l6 {
                        0 => {
                            let e = ();
                            Ok(e)
                        }
                        1 => {
                            let e = {
                                let l7 = *ptr5.add(4).cast::<*mut u8>();
                                let l8 = *ptr5.add(8).cast::<usize>();
                                let len9 = l8;
                                let bytes9 = _rt::Vec::from_raw_parts(
                                    l7.cast(),
                                    len9,
                                    len9,
                                );
                                _rt::string_lift(bytes9)
                            };
                            Err(e)
                        }
                        _ => _rt::invalid_enum_discriminant(),
                    }
                }
            }
        }
    }
}
#[allow(dead_code)]
pub mod exports {
    #[allow(dead_code)]
    pub mod wasmcloud {
        #[allow(dead_code)]
        pub mod messaging {
            #[allow(dead_code, clippy::all)]
            pub mod handler {
                use super::super::super::super::_rt;
                pub type BrokerMessage = super::super::super::super::wasmcloud::messaging::types::BrokerMessage;
                #[doc(hidden)]
                #[allow(non_snake_case)]
                pub unsafe fn _export_handle_message_cabi<T: Guest>(
                    arg0: *mut u8,
                    arg1: usize,
                    arg2: *mut u8,
                    arg3: usize,
                    arg4: i32,
                    arg5: *mut u8,
                    arg6: usize,
                ) -> *mut u8 {
                    let len0 = arg1;
                    let bytes0 = _rt::Vec::from_raw_parts(arg0.cast(), len0, len0);
                    let len1 = arg3;
                    let result3 = T::handle_message(super::super::super::super::wasmcloud::messaging::types::BrokerMessage {
                        subject: _rt::string_lift(bytes0),
                        body: _rt::Vec::from_raw_parts(arg2.cast(), len1, len1),
                        reply_to: match arg4 {
                            0 => None,
                            1 => {
                                let e = {
                                    let len2 = arg6;
                                    let bytes2 = _rt::Vec::from_raw_parts(
                                        arg5.cast(),
                                        len2,
                                        len2,
                                    );
                                    _rt::string_lift(bytes2)
                                };
                                Some(e)
                            }
                            _ => _rt::invalid_enum_discriminant(),
                        },
                    });
                    let ptr4 = _RET_AREA.0.as_mut_ptr().cast::<u8>();
                    match result3 {
                        Ok(_) => {
                            *ptr4.add(0).cast::<u8>() = (0i32) as u8;
                        }
                        Err(e) => {
                            *ptr4.add(0).cast::<u8>() = (1i32) as u8;
                            let vec5 = (e.into_bytes()).into_boxed_slice();
                            let ptr5 = vec5.as_ptr().cast::<u8>();
                            let len5 = vec5.len();
                            ::core::mem::forget(vec5);
                            *ptr4.add(8).cast::<usize>() = len5;
                            *ptr4.add(4).cast::<*mut u8>() = ptr5.cast_mut();
                        }
                    };
                    ptr4
                }
                #[doc(hidden)]
                #[allow(non_snake_case)]
                pub unsafe fn __post_return_handle_message<T: Guest>(arg0: *mut u8) {
                    let l0 = i32::from(*arg0.add(0).cast::<u8>());
                    match l0 {
                        0 => {}
                        _ => {
                            let l1 = *arg0.add(4).cast::<*mut u8>();
                            let l2 = *arg0.add(8).cast::<usize>();
                            _rt::cabi_dealloc(l1, l2, 1);
                        }
                    }
                }
                pub trait Guest {
                    /// Callback handled to invoke a function when a message is received from a subscription
                    fn handle_message(msg: BrokerMessage) -> Result<(), _rt::String>;
                }
                #[doc(hidden)]
                pub(crate) use __export_wasmcloud_messaging_handler_0_2_0_cabi;
                #[repr(align(4))]
                struct _RetArea([::core::mem::MaybeUninit<u8>; 12]);
                static mut _RET_AREA: _RetArea = _RetArea(
                    [::core::mem::MaybeUninit::uninit(); 12],
                );
            }
        }
    }
}
mod _rt {
    pub use alloc_crate::string::String;
    pub use alloc_crate::vec::Vec;
    pub unsafe fn string_lift(bytes: Vec<u8>) -> String {
        if true {
            String::from_utf8(bytes).unwrap()
        } else {
            String::from_utf8_unchecked(bytes)
        }
    }
    pub unsafe fn invalid_enum_discriminant<T>() -> T {
        if true {
            {
                ::core::panicking::panic_fmt(format_args!("invalid enum discriminant"));
            }
        } else {
            core::hint::unreachable_unchecked()
        }
    }
    pub unsafe fn cabi_dealloc(ptr: *mut u8, size: usize, align: usize) {
        if size == 0 {
            return;
        }
        let layout = alloc::Layout::from_size_align_unchecked(size, align);
        alloc::dealloc(ptr as *mut u8, layout);
    }
    use core::fmt;
    use core::marker;
    use core::sync::atomic::{AtomicU32, Ordering::Relaxed};
    /// A type which represents a component model resource, either imported or
    /// exported into this component.
    ///
    /// This is a low-level wrapper which handles the lifetime of the resource
    /// (namely this has a destructor). The `T` provided defines the component model
    /// intrinsics that this wrapper uses.
    ///
    /// One of the chief purposes of this type is to provide `Deref` implementations
    /// to access the underlying data when it is owned.
    ///
    /// This type is primarily used in generated code for exported and imported
    /// resources.
    #[repr(transparent)]
    pub struct Resource<T: WasmResource> {
        handle: AtomicU32,
        _marker: marker::PhantomData<T>,
    }
    /// A trait which all wasm resources implement, namely providing the ability to
    /// drop a resource.
    ///
    /// This generally is implemented by generated code, not user-facing code.
    #[allow(clippy::missing_safety_doc)]
    pub unsafe trait WasmResource {
        /// Invokes the `[resource-drop]...` intrinsic.
        unsafe fn drop(handle: u32);
    }
    impl<T: WasmResource> Resource<T> {
        #[doc(hidden)]
        pub unsafe fn from_handle(handle: u32) -> Self {
            if true {
                if !(handle != u32::MAX) {
                    ::core::panicking::panic("assertion failed: handle != u32::MAX")
                }
            }
            Self {
                handle: AtomicU32::new(handle),
                _marker: marker::PhantomData,
            }
        }
        /// Takes ownership of the handle owned by `resource`.
        ///
        /// Note that this ideally would be `into_handle` taking `Resource<T>` by
        /// ownership. The code generator does not enable that in all situations,
        /// unfortunately, so this is provided instead.
        ///
        /// Also note that `take_handle` is in theory only ever called on values
        /// owned by a generated function. For example a generated function might
        /// take `Resource<T>` as an argument but then call `take_handle` on a
        /// reference to that argument. In that sense the dynamic nature of
        /// `take_handle` should only be exposed internally to generated code, not
        /// to user code.
        #[doc(hidden)]
        pub fn take_handle(resource: &Resource<T>) -> u32 {
            resource.handle.swap(u32::MAX, Relaxed)
        }
        #[doc(hidden)]
        pub fn handle(resource: &Resource<T>) -> u32 {
            resource.handle.load(Relaxed)
        }
    }
    impl<T: WasmResource> fmt::Debug for Resource<T> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("Resource").field("handle", &self.handle).finish()
        }
    }
    impl<T: WasmResource> Drop for Resource<T> {
        fn drop(&mut self) {
            unsafe {
                match self.handle.load(Relaxed) {
                    u32::MAX => {}
                    other => T::drop(other),
                }
            }
        }
    }
    pub unsafe fn bool_lift(val: u8) -> bool {
        if true {
            match val {
                0 => false,
                1 => true,
                _ => {
                    ::core::panicking::panic_fmt(
                        format_args!("invalid bool discriminant"),
                    );
                }
            }
        } else {
            val != 0
        }
    }
    pub fn as_i64<T: AsI64>(t: T) -> i64 {
        t.as_i64()
    }
    pub trait AsI64 {
        fn as_i64(self) -> i64;
    }
    impl<'a, T: Copy + AsI64> AsI64 for &'a T {
        fn as_i64(self) -> i64 {
            (*self).as_i64()
        }
    }
    impl AsI64 for i64 {
        #[inline]
        fn as_i64(self) -> i64 {
            self as i64
        }
    }
    impl AsI64 for u64 {
        #[inline]
        fn as_i64(self) -> i64 {
            self as i64
        }
    }
    pub fn as_i32<T: AsI32>(t: T) -> i32 {
        t.as_i32()
    }
    pub trait AsI32 {
        fn as_i32(self) -> i32;
    }
    impl<'a, T: Copy + AsI32> AsI32 for &'a T {
        fn as_i32(self) -> i32 {
            (*self).as_i32()
        }
    }
    impl AsI32 for i32 {
        #[inline]
        fn as_i32(self) -> i32 {
            self as i32
        }
    }
    impl AsI32 for u32 {
        #[inline]
        fn as_i32(self) -> i32 {
            self as i32
        }
    }
    impl AsI32 for i16 {
        #[inline]
        fn as_i32(self) -> i32 {
            self as i32
        }
    }
    impl AsI32 for u16 {
        #[inline]
        fn as_i32(self) -> i32 {
            self as i32
        }
    }
    impl AsI32 for i8 {
        #[inline]
        fn as_i32(self) -> i32 {
            self as i32
        }
    }
    impl AsI32 for u8 {
        #[inline]
        fn as_i32(self) -> i32 {
            self as i32
        }
    }
    impl AsI32 for char {
        #[inline]
        fn as_i32(self) -> i32 {
            self as i32
        }
    }
    impl AsI32 for usize {
        #[inline]
        fn as_i32(self) -> i32 {
            self as i32
        }
    }
    extern crate alloc as alloc_crate;
    pub use alloc_crate::alloc;
}
#[doc(inline)]
pub(crate) use __export_hello_impl as export;
const _: &[u8] = b"package wasi:config@0.2.0-draft;\n\nworld imports {\n    /// The runtime interface for config\n    import runtime;\n}";
const _: &[u8] = b"interface runtime {\n    /// An error type that encapsulates the different errors that can occur fetching config\n    variant config-error {\n        /// This indicates an error from an \"upstream\" config source. \n        /// As this could be almost _anything_ (such as Vault, Kubernetes ConfigMaps, KeyValue buckets, etc), \n        /// the error message is a string.\n        upstream(string),\n        /// This indicates an error from an I/O operation. \n        /// As this could be almost _anything_ (such as a file read, network connection, etc), \n        /// the error message is a string. \n        /// Depending on how this ends up being consumed, \n        /// we may consider moving this to use the `wasi:io/error` type instead. \n        /// For simplicity right now in supporting multiple implementations, it is being left as a string.\n        io(string),\n    }\n\n    /// Gets a single opaque config value set at the given key if it exists\n    get: func(\n        /// A string key to fetch\n        key: string\n    ) -> result<option<string>, config-error>;\n\n    /// Gets a list of all set config data\n    get-all: func() -> result<list<tuple<string, string>>, config-error>;\n}\n";
const _: &[u8] = b"/// A keyvalue interface that provides eventually consistent key-value operations.\n/// \n/// Each of these operations acts on a single key-value pair.\n/// \n/// The value in the key-value pair is defined as a `u8` byte array and the intention is that it is\n/// the common denominator for all data types defined by different key-value stores to handle data,\n/// ensuring compatibility between different key-value stores. Note: the clients will be expecting\n/// serialization/deserialization overhead to be handled by the key-value store. The value could be\n/// a serialized object from JSON, HTML or vendor-specific data types like AWS S3 objects.\n/// \n/// Data consistency in a key value store refers to the guarantee that once a write operation\n/// completes, all subsequent read operations will return the value that was written.\n/// \n/// Any implementation of this interface must have enough consistency to guarantee \"reading your\n/// writes.\" In particular, this means that the client should never get a value that is older than\n/// the one it wrote, but it MAY get a newer value if one was written around the same time. These\n/// guarantees only apply to the same client (which will likely be provided by the host or an\n/// external capability of some kind). In this context a \"client\" is referring to the caller or\n/// guest that is consuming this interface. Once a write request is committed by a specific client,\n/// all subsequent read requests by the same client will reflect that write or any subsequent\n/// writes. Another client running in a different context may or may not immediately see the result\n/// due to the replication lag. As an example of all of this, if a value at a given key is A, and\n/// the client writes B, then immediately reads, it should get B. If something else writes C in\n/// quick succession, then the client may get C. However, a client running in a separate context may\n/// still see A or B\ninterface store {\n    /// The set of errors which may be raised by functions in this package\n    variant error {\n        /// The host does not recognize the store identifier requested.\n        no-such-store,\n\n        /// The requesting component does not have access to the specified store\n        /// (which may or may not exist).\n        access-denied,\n\n        /// Some implementation-specific error has occurred (e.g. I/O)\n        other(string)\n    }\n\n    /// A response to a `list-keys` operation.\n    record key-response {\n        /// The list of keys returned by the query.\n        keys: list<string>,\n        /// The continuation token to use to fetch the next page of keys. If this is `null`, then\n        /// there are no more keys to fetch.\n        cursor: option<u64>\n    }\n\n    /// Get the bucket with the specified identifier.\n    ///\n    /// `identifier` must refer to a bucket provided by the host.\n    ///\n    /// `error::no-such-store` will be raised if the `identifier` is not recognized.\n    open: func(identifier: string) -> result<bucket, error>;\n\n    /// A bucket is a collection of key-value pairs. Each key-value pair is stored as a entry in the\n    /// bucket, and the bucket itself acts as a collection of all these entries.\n    ///\n    /// It is worth noting that the exact terminology for bucket in key-value stores can very\n    /// depending on the specific implementation. For example:\n    ///\n    /// 1. Amazon DynamoDB calls a collection of key-value pairs a table\n    /// 2. Redis has hashes, sets, and sorted sets as different types of collections\n    /// 3. Cassandra calls a collection of key-value pairs a column family\n    /// 4. MongoDB calls a collection of key-value pairs a collection\n    /// 5. Riak calls a collection of key-value pairs a bucket\n    /// 6. Memcached calls a collection of key-value pairs a slab\n    /// 7. Azure Cosmos DB calls a collection of key-value pairs a container\n    ///\n    /// In this interface, we use the term `bucket` to refer to a collection of key-value pairs\n    resource bucket {\n        /// Get the value associated with the specified `key`\n        ///\n        /// The value is returned as an option. If the key-value pair exists in the\n        /// store, it returns `Ok(value)`. If the key does not exist in the\n        /// store, it returns `Ok(none)`. \n        ///\n        /// If any other error occurs, it returns an `Err(error)`.\n        get: func(key: string) -> result<option<list<u8>>, error>;\n\n        /// Set the value associated with the key in the store. If the key already\n        /// exists in the store, it overwrites the value.\n        ///\n        /// If the key does not exist in the store, it creates a new key-value pair.\n        /// \n        /// If any other error occurs, it returns an `Err(error)`.\n        set: func(key: string, value: list<u8>) -> result<_, error>;\n\n        /// Delete the key-value pair associated with the key in the store.\n        /// \n        /// If the key does not exist in the store, it does nothing.\n        ///\n        /// If any other error occurs, it returns an `Err(error)`.\n        delete: func(key: string) -> result<_, error>;\n\n        /// Check if the key exists in the store.\n        /// \n        /// If the key exists in the store, it returns `Ok(true)`. If the key does\n        /// not exist in the store, it returns `Ok(false)`.\n        /// \n        /// If any other error occurs, it returns an `Err(error)`.\n        exists: func(key: string) -> result<bool, error>;\n\n        /// Get all the keys in the store with an optional cursor (for use in pagination). It\n        /// returns a list of keys. Please note that for most KeyValue implementations, this is a\n        /// can be a very expensive operation and so it should be used judiciously. Implementations\n        /// can return any number of keys in a single response, but they should never attempt to\n        /// send more data than is reasonable (i.e. on a small edge device, this may only be a few\n        /// KB, while on a large machine this could be several MB). Any response should also return\n        /// a cursor that can be used to fetch the next page of keys. See the `key-response` record\n        /// for more information.\n        /// \n        /// Note that the keys are not guaranteed to be returned in any particular order.\n        /// \n        /// If the store is empty, it returns an empty list.\n        /// \n        /// MAY show an out-of-date list of keys if there are concurrent writes to the store.\n        /// \n        /// If any error occurs, it returns an `Err(error)`.\n        list-keys: func(cursor: option<u64>) -> result<key-response, error>;\n    }\n}\n";
const _: &[u8] = b"package wasi:keyvalue@0.2.0-draft;\n\n/// The `wasi:keyvalue/imports` world provides common APIs for interacting with key-value stores.\n/// Components targeting this world will be able to do:\n/// \n/// 1. CRUD (create, read, update, delete) operations on key-value stores.\n/// 2. Atomic `increment` and CAS (compare-and-swap) operations.\n/// 3. Batch operations that can reduce the number of round trips to the network.\nworld imports {\n\t/// The `store` capability allows the component to perform eventually consistent operations on\n\t/// the key-value store.\n\timport store;\n\n\t/// The `atomic` capability allows the component to perform atomic / `increment` and CAS\n\t/// (compare-and-swap) operations.\n\timport atomics;\n\n\t/// The `batch` capability allows the component to perform eventually consistent batch\n\t/// operations that can reduce the number of round trips to the network.\n\timport batch;\n}\n\nworld watch-service {\n\tinclude imports;\n\texport watcher;\n}";
const _: &[u8] = b"/// A keyvalue interface that provides batch operations.\n/// \n/// A batch operation is an operation that operates on multiple keys at once.\n/// \n/// Batch operations are useful for reducing network round-trip time. For example, if you want to\n/// get the values associated with 100 keys, you can either do 100 get operations or you can do 1\n/// batch get operation. The batch operation is faster because it only needs to make 1 network call\n/// instead of 100.\n/// \n/// A batch operation does not guarantee atomicity, meaning that if the batch operation fails, some\n/// of the keys may have been modified and some may not. \n/// \n/// This interface does has the same consistency guarantees as the `store` interface, meaning that\n/// you should be able to \"read your writes.\"\n/// \n/// Please note that this interface is bare functions that take a reference to a bucket. This is to\n/// get around the current lack of a way to \"extend\" a resource with additional methods inside of\n/// wit. Future version of the interface will instead extend these methods on the base `bucket`\n/// resource.\ninterface batch {\n    use store.{bucket, error};\n\n    /// Get the key-value pairs associated with the keys in the store. It returns a list of\n    /// key-value pairs.\n    ///\n    /// If any of the keys do not exist in the store, it returns a `none` value for that pair in the\n    /// list.\n    /// \n    /// MAY show an out-of-date value if there are concurrent writes to the store.\n    /// \n    /// If any other error occurs, it returns an `Err(error)`.\n    get-many: func(bucket: borrow<bucket>, keys: list<string>) -> result<list<option<tuple<string, list<u8>>>>, error>;\n\n    /// Set the values associated with the keys in the store. If the key already exists in the\n    /// store, it overwrites the value. \n    /// \n    /// Note that the key-value pairs are not guaranteed to be set in the order they are provided. \n    ///\n    /// If any of the keys do not exist in the store, it creates a new key-value pair.\n    /// \n    /// If any other error occurs, it returns an `Err(error)`. When an error occurs, it does not\n    /// rollback the key-value pairs that were already set. Thus, this batch operation does not\n    /// guarantee atomicity, implying that some key-value pairs could be set while others might\n    /// fail. \n    /// \n    /// Other concurrent operations may also be able to see the partial results.\n    set-many: func(bucket: borrow<bucket>, key-values: list<tuple<string, list<u8>>>) -> result<_, error>;\n\n    /// Delete the key-value pairs associated with the keys in the store.\n    /// \n    /// Note that the key-value pairs are not guaranteed to be deleted in the order they are\n    /// provided.\n    /// \n    /// If any of the keys do not exist in the store, it skips the key.\n    /// \n    /// If any other error occurs, it returns an `Err(error)`. When an error occurs, it does not\n    /// rollback the key-value pairs that were already deleted. Thus, this batch operation does not\n    /// guarantee atomicity, implying that some key-value pairs could be deleted while others might\n    /// fail.\n    /// \n    /// Other concurrent operations may also be able to see the partial results.\n    delete-many: func(bucket: borrow<bucket>, keys: list<string>) -> result<_, error>;\n}\n";
const _: &[u8] = b"/// A keyvalue interface that provides watch operations.\n/// \n/// This interface is used to provide event-driven mechanisms to handle\n/// keyvalue changes.\ninterface watcher {\n\t/// A keyvalue interface that provides handle-watch operations.\n\tuse store.{bucket};\n\n\t/// Handle the `set` event for the given bucket and key. It includes a reference to the `bucket`\n\t/// that can be used to interact with the store.\n\ton-set: func(bucket: bucket, key: string, value: list<u8>);\n\n\t/// Handle the `delete` event for the given bucket and key. It includes a reference to the\n\t/// `bucket` that can be used to interact with the store.\n\ton-delete: func(bucket: bucket, key: string);\n}";
const _: &[u8] = b"/// A keyvalue interface that provides atomic operations.\n/// \n/// Atomic operations are single, indivisible operations. When a fault causes an atomic operation to\n/// fail, it will appear to the invoker of the atomic operation that the action either completed\n/// successfully or did nothing at all.\n/// \n/// Please note that this interface is bare functions that take a reference to a bucket. This is to\n/// get around the current lack of a way to \"extend\" a resource with additional methods inside of\n/// wit. Future version of the interface will instead extend these methods on the base `bucket`\n/// resource.\ninterface atomics {\n  \tuse store.{bucket, error};\n\n  \t/// Atomically increment the value associated with the key in the store by the given delta. It\n\t/// returns the new value.\n\t///\n\t/// If the key does not exist in the store, it creates a new key-value pair with the value set\n\t/// to the given delta. \n\t///\n\t/// If any other error occurs, it returns an `Err(error)`.\n\tincrement: func(bucket: borrow<bucket>, key: string, delta: u64) -> result<u64, error>;\n}";
const _: &[u8] = b"package wasi:logging;\n\nworld imports {\n    import logging;\n}\n";
const _: &[u8] = b"/// WASI Logging is a logging API intended to let users emit log messages with\n/// simple priority levels and context values.\ninterface logging {\n    /// A log level, describing a kind of message.\n    enum level {\n       /// Describes messages about the values of variables and the flow of\n       /// control within a program.\n       trace,\n\n       /// Describes messages likely to be of interest to someone debugging a\n       /// program.\n       debug,\n\n       /// Describes messages likely to be of interest to someone monitoring a\n       /// program.\n       info,\n\n       /// Describes messages indicating hazardous situations.\n       warn,\n\n       /// Describes messages indicating serious errors.\n       error,\n\n       /// Describes messages indicating fatal errors.\n       critical,\n    }\n\n    /// Emit a log message.\n    ///\n    /// A log message has a `level` describing what kind of message is being\n    /// sent, a context, which is an uninterpreted string meant to help\n    /// consumers group similar messages, and a string containing the message\n    /// text.\n    log: func(level: level, context: string, message: string);\n}\n";
const _: &[u8] = b"package wasmcloud:messaging@0.2.0;\n\n// Types common to message broker interactions\ninterface types {\n    // A message sent to or received from a broker\n    record broker-message {\n        subject: string,\n        body: list<u8>,\n        reply-to: option<string>,\n    }\n}\n\ninterface handler {\n    use types.{broker-message};\n\n    // Callback handled to invoke a function when a message is received from a subscription\n    handle-message: func(msg: broker-message) -> result<_, string>;\n}\n\ninterface consumer {\n    use types.{broker-message};\n\n    // Perform a request operation on a subject\n    request: func(subject: string, body: list<u8>, timeout-ms: u32) -> result<broker-message, string>;\n    // Publish a message to a subject without awaiting a response\n    publish: func(msg: broker-message) -> result<_, string>;\n}\n";
const _: &[u8] = b"package wasmcloud:demo;\n\nworld hello {\n  import wasi:logging/logging;\n  import wasi:config/runtime@0.2.0-draft;\n  import wasi:keyvalue/atomics@0.2.0-draft;\n  import wasi:keyvalue/store@0.2.0-draft;\n  import wasmcloud:messaging/consumer@0.2.0;\n\n  export wasmcloud:messaging/handler@0.2.0;\n}\n";
use crate::exports::wasmcloud::messaging::handler::Guest as NatsKvDemoGuest;
use crate::wasi::keyvalue::{store, atomics};
use crate::wasmcloud::messaging::{consumer, types};
use crate::wasi::logging::logging::{log, Level};
struct NatsKvDemo;
const DEFAULT_BUCKET: &str = "WASMCLOUD";
const DEFAULT_COUNT: u64 = 1;
const DEFAULT_PUB_SUBJECT: &str = "nats.atomic";
impl NatsKvDemoGuest for NatsKvDemo {
    fn handle_message(msg: types::BrokerMessage) -> Result<(), String> {
        let bucket_name = match crate::wasi::config::runtime::get("bucke") {
            Ok(Some(value)) => value,
            Ok(None) => DEFAULT_BUCKET.to_string(),
            Err(_) => return Err("Failed to get bucket name".to_string()),
        };
        log(
            Level::Info,
            "kv-demo",
            {
                let res = ::alloc::fmt::format(
                    format_args!("Bucket name: {0}", bucket_name),
                );
                res
            }
                .as_str(),
        );
        let count = match crate::wasi::config::runtime::get("count") {
            Ok(Some(value)) => value.parse::<u64>().unwrap_or(DEFAULT_COUNT),
            Ok(None) => DEFAULT_COUNT,
            Err(_) => return Err("Failed to get repetition count".to_string()),
        };
        log(
            Level::Info,
            "kv-demo",
            {
                let res = ::alloc::fmt::format(format_args!("Count: {0}", count));
                res
            }
                .as_str(),
        );
        let pub_subject = match crate::wasi::config::runtime::get("pub_subject") {
            Ok(Some(value)) => value,
            Ok(None) => DEFAULT_PUB_SUBJECT.to_string(),
            Err(_) => return Err("Failed to get publish subject".to_string()),
        };
        log(
            Level::Info,
            "kv-demo",
            {
                let res = ::alloc::fmt::format(
                    format_args!("Publish subject: {0}", pub_subject),
                );
                res
            }
                .as_str(),
        );
        let key = match String::from_utf8(msg.body) {
            Ok(value) => value,
            Err(_) => return Err("Failed to convert message body to string".to_string()),
        };
        log(
            Level::Info,
            "kv-demo",
            {
                let res = ::alloc::fmt::format(format_args!("Key: {0}", key));
                res
            }
                .as_str(),
        );
        let bucket: store::Bucket = store::open(&bucket_name)
            .expect("failed to open bucket");
        for _ in 1..=count {
            let counter = atomics::increment(&bucket, &key, 1);
            if let Ok(count) = counter {
                log(
                    Level::Info,
                    "kv-demo",
                    {
                        let res = ::alloc::fmt::format(
                            format_args!("Incremented key {0} to {1}", key, count),
                        );
                        res
                    }
                        .as_str(),
                );
            }
        }
        match bucket.get(&key) {
            Ok(Some(value)) => {
                if let Err(_) = consumer::publish(
                    &types::BrokerMessage {
                        subject: pub_subject.clone(),
                        reply_to: None,
                        body: value.clone(),
                    },
                ) {
                    log(Level::Error, "kv-demo", "Failed to publish message");
                }
                match String::from_utf8(value) {
                    Ok(value_string) => {
                        log(
                            Level::Info,
                            "kv-demo",
                            {
                                let res = ::alloc::fmt::format(
                                    format_args!(
                                        "published key {0} with value {1} to NATS {2} subject",
                                        key.clone(),
                                        value_string,
                                        pub_subject,
                                    ),
                                );
                                res
                            }
                                .as_str(),
                        );
                    }
                    Err(_) => {
                        log(Level::Error, "kv-demo", "Failed to convert value to string")
                    }
                }
            }
            Ok(None) => {
                log(
                    Level::Info,
                    "kv-demo",
                    {
                        let res = ::alloc::fmt::format(
                            format_args!("No value found for key {0}", key.clone()),
                        );
                        res
                    }
                        .as_str(),
                )
            }
            Err(_) => return Err("Failed to get key value".to_string()),
        };
        if let Err(_) = bucket.delete(&key) {
            log(
                Level::Error,
                "kv-demo",
                {
                    let res = ::alloc::fmt::format(
                        format_args!("Failed to delete key {0}", key),
                    );
                    res
                }
                    .as_str(),
            );
        } else {
            log(
                Level::Info,
                "kv-demo",
                {
                    let res = ::alloc::fmt::format(format_args!("Deleted key {0}", key));
                    res
                }
                    .as_str(),
            );
        }
        Ok(())
    }
}
const _: () = {
    #[export_name = "wasmcloud:messaging/handler@0.2.0#handle-message"]
    unsafe extern "C" fn export_handle_message(
        arg0: *mut u8,
        arg1: usize,
        arg2: *mut u8,
        arg3: usize,
        arg4: i32,
        arg5: *mut u8,
        arg6: usize,
    ) -> *mut u8 {
        self::exports::wasmcloud::messaging::handler::_export_handle_message_cabi::<
            NatsKvDemo,
        >(arg0, arg1, arg2, arg3, arg4, arg5, arg6)
    }
    #[export_name = "cabi_post_wasmcloud:messaging/handler@0.2.0#handle-message"]
    unsafe extern "C" fn _post_return_handle_message(arg0: *mut u8) {
        self::exports::wasmcloud::messaging::handler::__post_return_handle_message::<
            NatsKvDemo,
        >(arg0)
    }
};
