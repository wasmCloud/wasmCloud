#[cfg(feature = "module")]
pub mod host;

#[cfg(all(not(feature = "module"), feature = "component"))]
mod bindings {
    wit_bindgen::generate!("interfaces-compat0");
}

#[cfg(all(not(feature = "module"), feature = "component"))]
pub use bindings::wasmcloud::bus::host;
