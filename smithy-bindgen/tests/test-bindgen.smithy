// bindgen-test.smithy

metadata package = [ {
    namespace: "org.wasmcloud.test.bindgen",
    //crate: "wasmcloud_interface_factorial",
} ]

namespace org.wasmcloud.test.bindgen

structure Thing {
    @required
    value: String,
}

