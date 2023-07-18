use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{parse_macro_input, Error, Path, Token};

struct Invocation {
    target: Path,
    handlers: Vec<Path>,
}

impl Parse for Invocation {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let span = input.span();
        let mut stream = Punctuated::<Path, Token![,]>::parse_terminated(input)?.into_iter();
        let target: Path = stream
            .next()
            .ok_or_else(|| Error::new(span, "target struct must be specified"))?;
        let handlers: Vec<Path> = stream.collect();
        if handlers.is_empty() {
            Err(Error::new(
                span,
                "at least one handler trait must be specified",
            ))
        } else {
            Ok(Self { target, handlers })
        }
    }
}

#[proc_macro]
pub fn export_actor(input: TokenStream) -> TokenStream {
    let Invocation { target, handlers } = parse_macro_input!(input);
    quote! {
        // version of the host-actor API
        #[no_mangle]
        pub extern "C" fn __wasmbus_rpc_version() -> u32 {
            1
        }

        #[no_mangle]
        pub extern "C" fn __guest_call(op_len: i32, pld_len: i32) -> i32 {
            #[link(wasm_import_module = "wasmbus")]
            extern "C" {
                pub fn __guest_request(op_ptr: *mut u8, pld_ptr: *mut u8);
                pub fn __guest_response(ptr: *const u8, len: usize);
                pub fn __guest_error(ptr: *const u8, len: usize);
            }

            let op_len = op_len.try_into().expect("operation too long");
            let pld_len = pld_len.try_into().expect("payload too long");

            let mut op = vec![0; op_len];
            let mut pld = vec![0; pld_len];

            unsafe { __guest_request(op.as_mut_ptr(), pld.as_mut_ptr()) };

            let op = match String::from_utf8(op) {
                Ok(op) => op,
                Err(e) => {
                    let e = format!("operation is not valid UTF-8: {}", e);
                    unsafe {
                        __guest_error(e.as_ptr(), e.len() as _);
                    }
                    return 0
                },
            };

            let handler = #target::default();
            #(
                match ::wasmcloud_actor::Handler::<dyn #handlers>::handle(&handler, &op, pld) {
                    Some(Ok(res)) => {
                        unsafe {
                            __guest_response(res.as_ptr(), res.len() as _);
                        }
                        return 1
                    }
                    Some(Err(e)) => {
                        let e = format!("failed to call `{}`: {}", op, e);
                        unsafe {
                            __guest_error(e.as_ptr(), e.len() as _);
                        }
                        return 0
                    }
                    None => {},
                }
            )*;

            let e = format!("no handler found for operation `{}`", op);
            unsafe {
                __guest_error(e.as_ptr(), e.len() as _);
            }
            0
        }
    }
    .into()
}
